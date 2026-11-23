use super::check;
use crate::MAX_SIDE;

fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&[8, 6, 0, 0, 0]);
    out
}

fn jpeg_header(marker: u8, w: u16, h: u16) -> Vec<u8> {
    let mut out = vec![0xff, 0xd8];
    // APP0 JFIF segment skipped before SOF.
    out.extend_from_slice(&[0xff, 0xe0, 0x00, 0x06, b'J', b'F', b'I', b'F']);
    // Fill 0xFF bytes before the SOF marker.
    out.extend_from_slice(&[0xff, 0xff, marker]);
    out.extend_from_slice(&11u16.to_be_bytes());
    out.push(8); // precision
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&[1, 1, 0x11, 0]);
    out
}

fn gif_header(version: &[u8; 6], w: u16, h: u16) -> Vec<u8> {
    let mut out = version.to_vec();
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out
}

fn riff_webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    for (fourcc, payload) in chunks {
        body.extend_from_slice(*fourcc);
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(payload);
        if payload.len() & 1 != 0 { body.push(0); }
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn webp_vp8_payload(w: u16, h: u16) -> Vec<u8> {
    let mut p = vec![0x10, 0x00, 0x00, 0x9d, 0x01, 0x2a];
    p.extend_from_slice(&(w & 0x3fff).to_le_bytes());
    p.extend_from_slice(&(h & 0x3fff).to_le_bytes());
    p
}

fn webp_vp8l_payload(w: u32, h: u32) -> Vec<u8> {
    let bits = ((w - 1) & 0x3fff) | (((h - 1) & 0x3fff) << 14);
    let mut p = vec![0x2f];
    p.extend_from_slice(&bits.to_le_bytes());
    p
}

fn webp_vp8x_payload(w: u32, h: u32) -> Vec<u8> {
    let wm1 = w - 1;
    let hm1 = h - 1;
    vec![
        0x00, 0x00, 0x00, 0x00,
        (wm1 & 0xff) as u8, ((wm1 >> 8) & 0xff) as u8, ((wm1 >> 16) & 0xff) as u8,
        (hm1 & 0xff) as u8, ((hm1 >> 8) & 0xff) as u8, ((hm1 >> 16) & 0xff) as u8,
    ]
}

fn bmp_info_dib(w: i32, h: i32) -> Vec<u8> {
    let mut dib = vec![0u8; 40];
    dib[0..4].copy_from_slice(&40u32.to_le_bytes());
    dib[4..8].copy_from_slice(&w.to_le_bytes());
    dib[8..12].copy_from_slice(&h.to_le_bytes());
    dib
}

fn bmp_info(w: i32, h: i32) -> Vec<u8> {
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(&bmp_info_dib(w, h));
    out
}

fn bmp_core(w: u16, h: u16) -> Vec<u8> {
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(&12u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&[1, 0, 24, 0]);
    out
}

fn ico_with_payloads(kind: u16, payloads: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&(payloads.len() as u16).to_le_bytes());
    let mut offset = (6 + payloads.len() * 16) as u32;
    for payload in payloads {
        // Directory claims 16x16 so tests catch payloads smuggling larger dimensions.
        out.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += payload.len() as u32;
    }
    for payload in payloads {
        out.extend_from_slice(payload);
    }
    out
}

fn bmff_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn full_box(kind: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
    let mut full = vec![version, 0, 0, 0];
    full.extend_from_slice(body);
    bmff_box(kind, &full)
}

fn av1_reduced_seq_hdr(w: u32, h: u32, high_bit_depth: bool) -> Vec<u8> {
    let mut bits = Vec::new();
    let push = |bits: &mut Vec<bool>, val: u32, n: u8| {
        for shift in (0..n).rev() { bits.push(((val >> shift) & 1) != 0); }
    };
    push(&mut bits, 0, 3); // profile 0
    bits.push(true); // still_picture
    bits.push(true); // reduced_still_picture_header
    push(&mut bits, 0, 5); // level
    let w_bits = (32 - (w - 1).leading_zeros()).max(1) as u8;
    let h_bits = (32 - (h - 1).leading_zeros()).max(1) as u8;
    push(&mut bits, u32::from(w_bits - 1), 4);
    push(&mut bits, u32::from(h_bits - 1), 4);
    push(&mut bits, w - 1, w_bits);
    push(&mut bits, h - 1, h_bits);
    push(&mut bits, 0, 6); // superblock..restoration
    bits.push(high_bit_depth);
    bits.push(false); // monochrome
    bits.push(false); // color_description_present_flag
    bits.push(false); // color_range
    push(&mut bits, 0, 2); // chroma_sample_position
    bits.push(false); // separate_uv_delta_q
    bits.push(false); // film_grain_params_present
    bits.push(true); // trailing bit
    let mut payload = vec![0u8; bits.len().div_ceil(8)];
    for (i, &bit) in bits.iter().enumerate() {
        if bit { payload[i / 8] |= 1 << (7 - (i % 8)); }
    }
    let mut obu = vec![(1 << 3) | 0x02, payload.len() as u8];
    obu.extend_from_slice(&payload);
    obu
}

fn minimal_avif(brand: &[u8; 4], ispe: (u32, u32), av1: &[u8]) -> Vec<u8> {
    let mut ftyp_body = brand.to_vec();
    ftyp_body.extend_from_slice(b"\0\0\0\0mif1avif");
    let ftyp = bmff_box(b"ftyp", &ftyp_body);
    let pitm = full_box(b"pitm", 0, &1u16.to_be_bytes());
    let mut ispe_body = Vec::new();
    ispe_body.extend_from_slice(&ispe.0.to_be_bytes());
    ispe_body.extend_from_slice(&ispe.1.to_be_bytes());
    let ipco = bmff_box(b"ipco", &full_box(b"ispe", 0, &ispe_body));
    let mut ipma_body = Vec::new();
    ipma_body.extend_from_slice(&1u32.to_be_bytes());
    ipma_body.extend_from_slice(&1u16.to_be_bytes());
    ipma_body.extend_from_slice(&[1, 1]);
    let ipma = full_box(b"ipma", 0, &ipma_body);
    let iprp = bmff_box(b"iprp", &[ipco, ipma].concat());
    let build_iloc = |offset: u32| {
        let mut d = vec![0x44, 0x00];
        d.extend_from_slice(&1u16.to_be_bytes()); // item_count
        d.extend_from_slice(&1u16.to_be_bytes()); // item_id
        d.extend_from_slice(&0u16.to_be_bytes()); // data_ref
        d.extend_from_slice(&1u16.to_be_bytes()); // extent_count
        d.extend_from_slice(&offset.to_be_bytes());
        d.extend_from_slice(&(av1.len() as u32).to_be_bytes());
        full_box(b"iloc", 0, &d)
    };
    let placeholder_meta = full_box(b"meta", 0, &[pitm.clone(), iprp.clone(), build_iloc(0)].concat());
    let mdat_offset = (ftyp.len() + placeholder_meta.len() + 8) as u32;
    let meta = full_box(b"meta", 0, &[pitm, iprp, build_iloc(mdat_offset)].concat());
    let mdat = bmff_box(b"mdat", av1);
    [ftyp, meta, mdat].concat()
}

#[test]
fn png_header_bounds_width_and_height_before_decode() {
    assert!(check(&png_header(1, 1)).is_ok());
    assert!(check(&png_header(MAX_SIDE, MAX_SIDE)).is_ok());
    assert!(check(&png_header(MAX_SIDE + 1, 1)).is_err());
    assert!(check(&png_header(1, MAX_SIDE + 1)).is_err());
    assert!(check(&png_header(65536, 65536)).is_err());
    assert!(check(&png_header(0, 16)).is_err());
    assert!(check(&png_header(16, 0)).is_err());
    // Truncated or non-IHDR first chunk must fail.
    assert!(check(b"\x89PNG\r\n\x1a\n").is_err());
    let mut bad = png_header(16, 16);
    bad[12..16].copy_from_slice(b"IDAT");
    assert!(check(&bad).is_err());
}

#[test]
fn jpeg_header_bounds_sof_dimensions_and_skips_exif_thumbnails() {
    for marker in [0xc0, 0xc1, 0xc2] {
        assert!(check(&jpeg_header(marker, 16, 8)).is_ok());
        assert!(check(&jpeg_header(marker, MAX_SIDE as u16, MAX_SIDE as u16)).is_ok());
        assert!(check(&jpeg_header(marker, (MAX_SIDE + 1) as u16, 1)).is_err());
        assert!(check(&jpeg_header(marker, 1, (MAX_SIDE + 1) as u16)).is_err());
        assert!(check(&jpeg_header(marker, 65535, 65535)).is_err());
        // Zero height (DNL bypass attempt) must be refused.
        assert!(check(&jpeg_header(marker, 16, 0)).is_err());
    }
    // APP1 EXIF containing an embedded small JPEG thumbnail must not hide an oversized main SOF.
    let thumb = jpeg_header(0xc0, 16, 16);
    let mut with_thumb = vec![0xff, 0xd8, 0xff, 0xe1];
    with_thumb.extend_from_slice(&((thumb.len() + 2) as u16).to_be_bytes());
    with_thumb.extend_from_slice(&thumb);
    with_thumb.extend_from_slice(&jpeg_header(0xc0, (MAX_SIDE + 1) as u16, 16)[2..]);
    assert!(check(&with_thumb).is_err());
    // Missing SOF before SOS or EOI fails.
    assert!(check(&[0xff, 0xd8, 0xff, 0xda, 0x00, 0x02]).is_err());
    assert!(check(&[0xff, 0xd8, 0xff, 0xd9]).is_err());
}

#[test]
fn gif_header_bounds_logical_screen_and_first_frame_descriptor() {
    for version in [b"GIF87a", b"GIF89a"] {
        assert!(check(&gif_header(version, 16, 8)).is_ok());
        assert!(check(&gif_header(version, MAX_SIDE as u16, MAX_SIDE as u16)).is_ok());
        assert!(check(&gif_header(version, (MAX_SIDE + 1) as u16, 1)).is_err());
        assert!(check(&gif_header(version, 1, (MAX_SIDE + 1) as u16)).is_err());
        assert!(check(&gif_header(version, 0, 8)).is_err());
    }
    // Small logical screen with a 2-entry GCT and graphic control extension before an oversized frame.
    let mut spoofed = gif_header(b"GIF89a", 16, 16);
    spoofed.extend_from_slice(&[0x80, 0, 0]); // GCT flag set, size = 2^(0+1) = 2 entries (6 bytes)
    spoofed.extend_from_slice(&[0; 6]); // GCT
    spoofed.extend_from_slice(&[0x21, 0xf9, 4, 0, 0, 0, 0, 0]); // Graphic Control Extension
    spoofed.push(0x2c); // Image Descriptor
    spoofed.extend_from_slice(&0u16.to_le_bytes()); // left
    spoofed.extend_from_slice(&0u16.to_le_bytes()); // top
    spoofed.extend_from_slice(&((MAX_SIDE + 1) as u16).to_le_bytes()); // frame width
    spoofed.extend_from_slice(&16u16.to_le_bytes()); // frame height
    spoofed.push(0); // packed
    assert!(check(&spoofed).is_err());
    assert!(check(b"GIF89a\x01\x00").is_err());
}

#[test]
fn webp_header_bounds_vp8_vp8l_vp8x_and_nested_chunks() {
    // Lossy VP8.
    assert!(check(&riff_webp(&[(b"VP8 ", &webp_vp8_payload(16, 8))])).is_ok());
    assert!(check(&riff_webp(&[(b"VP8 ", &webp_vp8_payload(MAX_SIDE as u16, MAX_SIDE as u16))])).is_ok());
    assert!(check(&riff_webp(&[(b"VP8 ", &webp_vp8_payload((MAX_SIDE + 1) as u16, 1))])).is_err());
    assert!(check(&riff_webp(&[(b"VP8 ", &webp_vp8_payload(1, (MAX_SIDE + 1) as u16))])).is_err());
    // Lossless VP8L.
    assert!(check(&riff_webp(&[(b"VP8L", &webp_vp8l_payload(16, 8))])).is_ok());
    assert!(check(&riff_webp(&[(b"VP8L", &webp_vp8l_payload(MAX_SIDE, MAX_SIDE))])).is_ok());
    assert!(check(&riff_webp(&[(b"VP8L", &webp_vp8l_payload(MAX_SIDE + 1, 1))])).is_err());
    assert!(check(&riff_webp(&[(b"VP8L", &webp_vp8l_payload(1, MAX_SIDE + 1))])).is_err());
    // Extended VP8X.
    assert!(check(&riff_webp(&[(b"VP8X", &webp_vp8x_payload(16, 8))])).is_ok());
    assert!(check(&riff_webp(&[(b"VP8X", &webp_vp8x_payload(MAX_SIDE, MAX_SIDE))])).is_ok());
    assert!(check(&riff_webp(&[(b"VP8X", &webp_vp8x_payload(MAX_SIDE + 1, 1))])).is_err());
    assert!(check(&riff_webp(&[(b"VP8X", &webp_vp8x_payload(1, 16_000_000))])).is_err());
    // Small VP8X canvas hiding an oversized VP8L or ANMF chunk.
    assert!(check(&riff_webp(&[
        (b"VP8X", &webp_vp8x_payload(16, 16)),
        (b"VP8L", &webp_vp8l_payload(MAX_SIDE + 1, 16)),
    ])).is_err());
    let mut anmf = vec![0u8; 16];
    let wm1 = MAX_SIDE; // width = MAX_SIDE + 1
    anmf[6..9].copy_from_slice(&[(wm1 & 0xff) as u8, ((wm1 >> 8) & 0xff) as u8, ((wm1 >> 16) & 0xff) as u8]);
    assert!(check(&riff_webp(&[
        (b"VP8X", &webp_vp8x_payload(16, 16)),
        (b"ANMF", &anmf),
    ])).is_err());
}

#[test]
fn bmp_and_ico_headers_bound_dimensions_and_embedded_payloads() {
    assert!(check(&bmp_info(16, 8)).is_ok());
    assert!(check(&bmp_info(16, -8)).is_ok(), "top-down BMP with negative height");
    assert!(check(&bmp_info(MAX_SIDE as i32, -(MAX_SIDE as i32))).is_ok());
    assert!(check(&bmp_info((MAX_SIDE + 1) as i32, 1)).is_err());
    assert!(check(&bmp_info(1, -(MAX_SIDE as i32 + 1))).is_err());
    assert!(check(&bmp_info(1, i32::MIN)).is_err());
    assert!(check(&bmp_info(-16, 16)).is_err());
    assert!(check(&bmp_core(16, 8)).is_ok());
    assert!(check(&bmp_core((MAX_SIDE + 1) as u16, 1)).is_err());

    let ok_png = png_header(32, 32);
    let ok_dib = bmp_info_dib(32, 64); // ICO DIB height is 2x image height
    assert!(check(&ico_with_payloads(1, &[&ok_png, &ok_dib])).is_ok());
    assert!(check(&ico_with_payloads(2, &[&ok_png])).is_ok(), "CUR icon");
    let huge_png = png_header(MAX_SIDE + 1, 16);
    assert!(check(&ico_with_payloads(1, &[&ok_png, &huge_png])).is_err(), "smuggled huge PNG in ICO");
    let huge_dib = bmp_info_dib(16, (MAX_SIDE as i32 + 1) * 2);
    assert!(check(&ico_with_payloads(1, &[&huge_dib])).is_err(), "smuggled huge DIB in ICO");
}

#[test]
fn avif_header_reuses_native_limits_and_rejects_unrecognized_bytes() {
    let small_av1 = av1_reduced_seq_hdr(16, 16, false);
    assert!(check(&minimal_avif(b"avif", (16, 16), &small_av1)).is_ok());
    assert!(check(&minimal_avif(b"avif", (MAX_SIDE + 1, 1), &small_av1)).is_err());
    let huge_av1 = av1_reduced_seq_hdr(16384, 16384, false);
    assert!(check(&minimal_avif(b"avif", (16, 16), &huge_av1)).is_err(), "spoofed ispe with huge AV1");
    let hbd_av1 = av1_reduced_seq_hdr(MAX_SIDE, MAX_SIDE, true);
    assert!(check(&minimal_avif(b"avif", (MAX_SIDE, MAX_SIDE), &hbd_av1)).is_err(), "10-bit 8192x8192 exceeds 256 MiB");
    assert!(check(&minimal_avif(b"avis", (16, 16), &small_av1)).is_err(), "animated AVIF rejected like native");

    assert!(check(b"").is_err());
    assert!(check(b"not an image").is_err());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn real_encoded_images_in_every_raster_format_pass_header_check() {
    let rgba = image::RgbaImage::from_pixel(12, 8, image::Rgba([20, 40, 60, 255]));
    let rgb = image::RgbImage::from_pixel(12, 8, image::Rgb([20, 40, 60]));
    for format in [
        image::ImageFormat::Png,
        image::ImageFormat::Gif,
        image::ImageFormat::WebP,
        image::ImageFormat::Bmp,
        image::ImageFormat::Ico,
    ] {
        let mut buf = std::io::Cursor::new(Vec::new());
        rgba.write_to(&mut buf, format).unwrap();
        assert_eq!(check(&buf.into_inner()), Ok(()), "{format:?}");
    }
    let mut jpg = std::io::Cursor::new(Vec::new());
    rgb.write_to(&mut jpg, image::ImageFormat::Jpeg).unwrap();
    assert_eq!(check(&jpg.into_inner()), Ok(()), "Jpeg");
}
