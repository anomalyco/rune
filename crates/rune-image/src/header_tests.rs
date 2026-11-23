use super::{MAX_AVIS_DURATION_SECS, MAX_AVIS_FRAMES, MAX_AVIS_SCAN_BYTES, check};
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
    assert!(check(&minimal_avif(b"avis", (16, 16), &small_av1)).is_err(), "avis without moov tracks rejected");

    assert!(check(b"").is_err());
    assert!(check(b"not an image").is_err());
}

/// LSB-first bit writer for JPEG XL codestream SizeHeader fixtures.
struct JxlBitWriter {
    bytes: Vec<u8>,
    bit_pos: usize,
}

impl JxlBitWriter {
    fn new() -> Self { Self { bytes: vec![0xff, 0x0a], bit_pos: 16 } }

    fn push(&mut self, value: u32, n: usize) {
        for i in 0..n {
            if self.bit_pos % 8 == 0 { self.bytes.push(0); }
            if (value >> i) & 1 != 0 {
                self.bytes[self.bit_pos / 8] |= 1 << (self.bit_pos % 8);
            }
            self.bit_pos += 1;
        }
    }

    fn push_u2s(&mut self, value: u32) {
        let v = value - 1;
        if v < (1 << 9) {
            self.push(0, 2);
            self.push(v, 9);
        } else if v < (1 << 13) {
            self.push(1, 2);
            self.push(v, 13);
        } else if v < (1 << 18) {
            self.push(2, 2);
            self.push(v, 18);
        } else {
            self.push(3, 2);
            self.push(v, 30);
        }
    }

    fn finish(self) -> Vec<u8> { self.bytes }
}

fn jxl_codestream_small(w_div8: u32, h_div8: u32, ratio: u32) -> Vec<u8> {
    let mut bw = JxlBitWriter::new();
    bw.push(1, 1);
    bw.push(h_div8 - 1, 5);
    bw.push(ratio, 3);
    if ratio == 0 { bw.push(w_div8 - 1, 5); }
    bw.finish()
}

fn jxl_codestream(w: u32, h: u32) -> Vec<u8> {
    let mut bw = JxlBitWriter::new();
    bw.push(0, 1);
    bw.push_u2s(h);
    bw.push(0, 3);
    bw.push_u2s(w);
    bw.finish()
}

fn jxl_codestream_ratio(h: u32, ratio: u32) -> Vec<u8> {
    let mut bw = JxlBitWriter::new();
    bw.push(0, 1);
    bw.push_u2s(h);
    bw.push(ratio, 3);
    bw.finish()
}

fn jxl_container(version: u32, boxes: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a];
    let mut ftyp = b"jxl ".to_vec();
    ftyp.extend_from_slice(&version.to_be_bytes());
    ftyp.extend_from_slice(b"jxl ");
    out.extend_from_slice(&bmff_box(b"ftyp", &ftyp));
    for b in boxes { out.extend_from_slice(b); }
    out
}

fn jxlp_box(index: u32, last: bool, payload: &[u8]) -> Vec<u8> {
    let raw = index | if last { 0x8000_0000 } else { 0 };
    let mut body = raw.to_be_bytes().to_vec();
    body.extend_from_slice(payload);
    bmff_box(b"jxlp", &body)
}

#[test]
fn jxl_header_bounds_codestream_and_container_dimensions() {
    // Real WPT JPEG XL headers: 3x3 lossless, 3x3 lossy, 32x32 animated, 3x3 jxlp container.
    let real_3x3_lossless = [
        0xff, 0x0a, 0x10, 0x10, 0x10, 0x50, 0xb9, 0xb7, 0x8a, 0x22, 0x08, 0x04,
    ];
    let real_3x3_lossy = [
        0xff, 0x0a, 0x10, 0x10, 0x90, 0x50, 0xb9, 0xb7, 0x8a, 0x22, 0x00, 0x13,
    ];
    let real_32x32_anim = [
        0xff, 0x0a, 0x47, 0x04, 0x03, 0x88, 0x09, 0x08, 0x00, 0x92, 0x09, 0x00,
    ];
    let real_3x3_jxlp_container = [
        0x00, 0x00, 0x00, 0x0c, 0x4a, 0x58, 0x4c, 0x20, 0x0d, 0x0a, 0x87, 0x0a,
        0x00, 0x00, 0x00, 0x14, 0x66, 0x74, 0x79, 0x70, 0x6a, 0x78, 0x6c, 0x20,
        0x00, 0x00, 0x00, 0x00, 0x6a, 0x78, 0x6c, 0x20,
        0x00, 0x00, 0x00, 0x12, 0x6a, 0x78, 0x6c, 0x70, 0x00, 0x00, 0x00, 0x00,
        0xff, 0x0a, 0x10, 0x10, 0x10, 0x09,
    ];
    for real in [
        &real_3x3_lossless[..],
        &real_3x3_lossy[..],
        &real_32x32_anim[..],
        &real_3x3_jxlp_container[..],
    ] {
        assert_eq!(check(real), Ok(()));
    }

    assert_eq!(check(&jxl_codestream_small(4, 2, 0)), Ok(()));
    for ratio in 1..=7 {
        assert_eq!(check(&jxl_codestream_small(1, 3, ratio)), Ok(()), "small ratio {ratio}");
        assert_eq!(check(&jxl_codestream_ratio(100, ratio)), Ok(()), "ratio {ratio}");
    }
    assert_eq!(check(&jxl_codestream(1, 1)), Ok(()));
    assert_eq!(check(&jxl_codestream(MAX_SIDE, MAX_SIDE)), Ok(()));
    assert!(check(&jxl_codestream(MAX_SIDE + 1, 1)).is_err());
    assert!(check(&jxl_codestream(1, MAX_SIDE + 1)).is_err());
    assert!(check(&jxl_codestream(1 << 20, 1)).is_err(), "30-bit selector over limit");
    assert!(check(&jxl_codestream_ratio(5000, 7)).is_err(), "2:1 ratio yields width 10000");
    assert!(check(&[0xff, 0x0a]).is_err(), "truncated bare codestream");

    let ok_cs = jxl_codestream(64, 48);
    let huge_cs = jxl_codestream(MAX_SIDE + 1, 16);
    assert_eq!(check(&jxl_container(0, &[bmff_box(b"jxlc", &ok_cs)])), Ok(()));
    assert!(check(&jxl_container(0, &[bmff_box(b"jxlc", &huge_cs)])).is_err());
    let mut eof_jxlc = vec![0, 0, 0, 0];
    eof_jxlc.extend_from_slice(b"jxlc");
    eof_jxlc.extend_from_slice(&ok_cs);
    assert_eq!(check(&jxl_container(0, &[eof_jxlc])), Ok(()));
    let mut wide_jxlc = vec![0, 0, 0, 1];
    wide_jxlc.extend_from_slice(b"jxlc");
    wide_jxlc.extend_from_slice(&((16 + huge_cs.len()) as u64).to_be_bytes());
    wide_jxlc.extend_from_slice(&huge_cs);
    assert!(check(&jxl_container(0, &[wide_jxlc])).is_err());

    let split_ok = jxl_container(0, &[
        bmff_box(b"jxll", &[5]),
        jxlp_box(0, false, &ok_cs[..3]),
        bmff_box(b"Exif", &[0; 8]),
        jxlp_box(1, true, &ok_cs[3..]),
    ]);
    assert_eq!(check(&split_ok), Ok(()));
    let split_huge = jxl_container(0, &[
        jxlp_box(0, false, &huge_cs[..3]),
        jxlp_box(1, true, &huge_cs[3..]),
    ]);
    assert!(check(&split_huge).is_err());

    let ooo_boxes = [
        jxlp_box(1, true, &ok_cs[3..]),
        jxlp_box(0, false, &ok_cs[..3]),
    ];
    assert_eq!(check(&jxl_container(1, &ooo_boxes)), Ok(()));
    assert!(check(&jxl_container(0, &ooo_boxes)).is_err());

    assert!(check(&jxl_container(0, &[])).is_err());
    let mut no_ftyp = vec![0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a];
    no_ftyp.extend_from_slice(&bmff_box(b"jxlc", &ok_cs));
    assert!(check(&no_ftyp).is_err());
    let mut bad_brand = vec![0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a];
    bad_brand.extend_from_slice(&bmff_box(b"ftyp", b"avif\0\0\0\0"));
    bad_brand.extend_from_slice(&bmff_box(b"jxlc", &ok_cs));
    assert!(check(&bad_brand).is_err());
}

fn avis_track(
    av01: (u16, u16),
    tkhd: (u32, u32),
    timescale: u32,
    mdhd_dur: u32,
    stsz_frames: u32,
    stts_frames: u32,
    stts_delta: u32,
) -> Vec<u8> {
    let mut tkhd_body = vec![0u8; 80];
    tkhd_body[72..76].copy_from_slice(&(tkhd.0 << 16).to_be_bytes());
    tkhd_body[76..80].copy_from_slice(&(tkhd.1 << 16).to_be_bytes());
    let tkhd_box = full_box(b"tkhd", 0, &tkhd_body);

    let mut mdhd_body = vec![0u8; 20];
    mdhd_body[8..12].copy_from_slice(&timescale.to_be_bytes());
    mdhd_body[12..16].copy_from_slice(&mdhd_dur.to_be_bytes());
    let mdhd_box = full_box(b"mdhd", 0, &mdhd_body);

    let mut av01_body = vec![0u8; 78];
    av01_body[24..26].copy_from_slice(&av01.0.to_be_bytes());
    av01_body[26..28].copy_from_slice(&av01.1.to_be_bytes());
    let mut stsd_body = 1u32.to_be_bytes().to_vec();
    stsd_body.extend_from_slice(&bmff_box(b"av01", &av01_body));
    let stsd_box = full_box(b"stsd", 0, &stsd_body);

    let mut stsz_body = Vec::new();
    stsz_body.extend_from_slice(&16u32.to_be_bytes()); // constant sample_size
    stsz_body.extend_from_slice(&stsz_frames.to_be_bytes());
    let stsz_box = full_box(b"stsz", 0, &stsz_body);

    let mut stts_body = Vec::new();
    stts_body.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    stts_body.extend_from_slice(&stts_frames.to_be_bytes());
    stts_body.extend_from_slice(&stts_delta.to_be_bytes());
    let stts_box = full_box(b"stts", 0, &stts_body);

    let stbl = bmff_box(b"stbl", &[stsd_box, stsz_box, stts_box].concat());
    let minf = bmff_box(b"minf", &stbl);
    let mdia = bmff_box(b"mdia", &[mdhd_box, minf].concat());
    bmff_box(b"trak", &[tkhd_box, mdia].concat())
}

fn build_avis(major: &[u8; 4], compat: &[u8], ispe: Option<(u32, u32)>, mvhd: (u32, u32), tracks: &[Vec<u8>]) -> Vec<u8> {
    let mut ftyp_body = major.to_vec();
    ftyp_body.extend_from_slice(&[0; 4]);
    ftyp_body.extend_from_slice(compat);
    let mut out = bmff_box(b"ftyp", &ftyp_body);
    if let Some((w, h)) = ispe {
        let mut ispe_body = Vec::new();
        ispe_body.extend_from_slice(&w.to_be_bytes());
        ispe_body.extend_from_slice(&h.to_be_bytes());
        let ipco = bmff_box(b"ipco", &full_box(b"ispe", 0, &ispe_body));
        let iprp = bmff_box(b"iprp", &ipco);
        out.extend_from_slice(&full_box(b"meta", 0, &iprp));
    }
    let mut mvhd_body = vec![0u8; 96];
    mvhd_body[8..12].copy_from_slice(&mvhd.0.to_be_bytes());
    mvhd_body[12..16].copy_from_slice(&mvhd.1.to_be_bytes());
    let mut moov_body = full_box(b"mvhd", 0, &mvhd_body);
    for t in tracks { moov_body.extend_from_slice(t); }
    out.extend_from_slice(&bmff_box(b"moov", &moov_body));
    out
}

#[test]
fn avis_header_bounds_tracks_frames_duration_and_scan_limit() {
    // Real 32x32 2-frame 1s AVIF sequence header produced by ffmpeg (libaom-av1).
    let real_ffmpeg_avis: &[u8] = &[
        0x00, 0x00, 0x00, 0x2c, 0x66, 0x74, 0x79, 0x70, 0x61, 0x76, 0x69, 0x73, 0x00, 0x00, 0x00, 0x00, 0x61, 0x76, 0x69, 0x73,
        0x61, 0x76, 0x69, 0x66, 0x6d, 0x73, 0x66, 0x31, 0x69, 0x73, 0x6f, 0x38, 0x6d, 0x69, 0x66, 0x31, 0x6d, 0x69, 0x61, 0x66,
        0x4d, 0x41, 0x31, 0x42, 0x00, 0x00, 0x00, 0xf9, 0x6d, 0x65, 0x74, 0x61, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2f,
        0x68, 0x64, 0x6c, 0x72, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x69, 0x63, 0x74, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x69, 0x63, 0x74, 0x75, 0x72, 0x65, 0x48, 0x61, 0x6e, 0x64, 0x6c,
        0x65, 0x72, 0x00, 0x00, 0x00, 0x00, 0x0e, 0x70, 0x69, 0x74, 0x6d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x1e, 0x69, 0x6c, 0x6f, 0x63, 0x00, 0x00, 0x00, 0x00, 0x44, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x03, 0xe8, 0x00, 0x00, 0x00, 0xcb, 0x00, 0x00, 0x00, 0x28, 0x69, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x1a, 0x69, 0x6e, 0x66, 0x65, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x61, 0x76, 0x30,
        0x31, 0x43, 0x6f, 0x6c, 0x6f, 0x72, 0x00, 0x00, 0x00, 0x00, 0x6a, 0x69, 0x70, 0x72, 0x70, 0x00, 0x00, 0x00, 0x4b, 0x69,
        0x70, 0x63, 0x6f, 0x00, 0x00, 0x00, 0x14, 0x69, 0x73, 0x70, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00,
        0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x10, 0x70, 0x69, 0x78, 0x69, 0x00, 0x00, 0x00, 0x00, 0x03, 0x08, 0x08, 0x08, 0x00,
        0x00, 0x00, 0x0c, 0x61, 0x76, 0x31, 0x43, 0x81, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x13, 0x63, 0x6f, 0x6c, 0x72, 0x6e,
        0x63, 0x6c, 0x78, 0x00, 0x02, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x17, 0x69, 0x70, 0x6d, 0x61, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x04, 0x01, 0x02, 0x83, 0x04, 0x00, 0x00, 0x02, 0xbb, 0x6d, 0x6f, 0x6f,
        0x76, 0x00, 0x00, 0x00, 0x6c, 0x6d, 0x76, 0x68, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x02, 0x47, 0x74, 0x72, 0x61, 0x6b, 0x00, 0x00, 0x00,
        0x68, 0x74, 0x6b, 0x68, 0x64, 0x01, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x20, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x24, 0x65, 0x64, 0x74, 0x73, 0x00, 0x00, 0x00, 0x1c, 0x65, 0x6c, 0x73, 0x74, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01,
        0xb3, 0x6d, 0x64, 0x69, 0x61, 0x00, 0x00, 0x00, 0x20, 0x6d, 0x64, 0x68, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x40, 0x00, 0x55, 0xc4, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x2f, 0x68, 0x64, 0x6c, 0x72, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x69, 0x63, 0x74, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x69, 0x63, 0x74, 0x75, 0x72, 0x65, 0x48, 0x61, 0x6e, 0x64,
        0x6c, 0x65, 0x72, 0x00, 0x00, 0x00, 0x01, 0x5c, 0x6d, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x14, 0x76, 0x6d, 0x68, 0x64,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x64, 0x69, 0x6e, 0x66,
        0x00, 0x00, 0x00, 0x1c, 0x64, 0x72, 0x65, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0c,
        0x75, 0x72, 0x6c, 0x20, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x01, 0x1c, 0x73, 0x74, 0x62, 0x6c, 0x00, 0x00, 0x00, 0x9c,
        0x73, 0x74, 0x73, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x8c, 0x61, 0x76, 0x30, 0x31,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x20, 0x00, 0x48, 0x00, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x17, 0x4c, 0x61, 0x76, 0x63, 0x36, 0x33, 0x2e, 0x31, 0x2e, 0x31, 0x30, 0x32, 0x20, 0x6c, 0x69, 0x62, 0x61,
        0x6f, 0x6d, 0x2d, 0x61, 0x76, 0x31, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0xff, 0xff, 0x00, 0x00,
        0x00, 0x0c, 0x61, 0x76, 0x31, 0x43, 0x81, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x66, 0x69, 0x65, 0x6c, 0x01, 0x00,
        0x00, 0x00, 0x00, 0x10, 0x70, 0x61, 0x73, 0x70, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10,
        0x63, 0x63, 0x73, 0x74, 0x00, 0x00, 0x00, 0x00, 0x7c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x73, 0x74, 0x74, 0x73,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x14,
        0x73, 0x74, 0x73, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x1c,
        0x73, 0x74, 0x73, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x1c, 0x73, 0x74, 0x73, 0x7a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0xcb, 0x00, 0x00, 0x00, 0x41, 0x00, 0x00, 0x00, 0x14, 0x73, 0x74, 0x63, 0x6f,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x03, 0xe8,
    ];
    assert_eq!(check(real_ffmpeg_avis), Ok(()));

    let ok_track = avis_track((32, 32), (32, 32), 1000, 1000, 2, 2, 500);
    assert_eq!(check(&build_avis(b"avis", b"avismsf1", Some((32, 32)), (1000, 1000), &[ok_track.clone()])), Ok(()));
    assert_eq!(check(&build_avis(b"avis", b"avismsf1", None, (1000, 1000), &[ok_track.clone()])), Ok(()), "moov-only avis");
    assert_eq!(check(&build_avis(b"msf1", b"avismsf1", None, (1000, 1000), &[ok_track.clone()])), Ok(()), "compat brand avis");
    assert_eq!(check(&build_avis(b"avis", b"avis", Some((32, 32)), (1000, 1000), &[ok_track.clone(), ok_track.clone()])), Ok(()), "color + alpha tracks");

    // Large mdat after moov does not trip the header scan limit.
    let mut with_large_mdat = build_avis(b"avis", b"avis", Some((32, 32)), (1000, 1000), &[ok_track.clone()]);
    with_large_mdat.extend_from_slice(&bmff_box(b"mdat", &vec![0u8; MAX_AVIS_SCAN_BYTES + 64]));
    assert_eq!(check(&with_large_mdat), Ok(()));

    // Dimensions: exact limit passes; oversized av01, tkhd, ispe or alpha track fails.
    let limit_track = avis_track((MAX_SIDE as u16, MAX_SIDE as u16), (MAX_SIDE, MAX_SIDE), 1000, 1000, 1, 1, 1000);
    assert_eq!(check(&build_avis(b"avis", b"avis", Some((MAX_SIDE, MAX_SIDE)), (1000, 1000), &[limit_track])), Ok(()));
    let huge_av01 = avis_track((MAX_SIDE as u16 + 1, 16), (16, 16), 1000, 1000, 2, 2, 500);
    assert!(check(&build_avis(b"avis", b"avis", Some((16, 16)), (1000, 1000), &[huge_av01.clone()])).is_err());
    assert!(check(&build_avis(b"avis", b"avis", Some((16, 16)), (1000, 1000), &[ok_track.clone(), huge_av01])).is_err(), "oversized alpha track");
    let huge_tkhd = avis_track((16, 16), (MAX_SIDE + 1, 16), 1000, 1000, 2, 2, 500);
    assert!(check(&build_avis(b"avis", b"avis", Some((16, 16)), (1000, 1000), &[huge_tkhd])).is_err());
    assert!(check(&build_avis(b"avis", b"avis", Some((MAX_SIDE + 1, 16)), (1000, 1000), &[ok_track.clone()])).is_err());

    // Frame count limits.
    let max_frames = avis_track((16, 16), (16, 16), 60, MAX_AVIS_FRAMES, MAX_AVIS_FRAMES, MAX_AVIS_FRAMES, 1);
    assert_eq!(check(&build_avis(b"avis", b"avis", None, (60, MAX_AVIS_FRAMES), &[max_frames])), Ok(()));
    let too_many_stsz = avis_track((16, 16), (16, 16), 60, 60, MAX_AVIS_FRAMES + 1, 2, 30);
    assert!(check(&build_avis(b"avis", b"avis", None, (60, 60), &[too_many_stsz])).is_err());
    let too_many_stts = avis_track((16, 16), (16, 16), 60, 60, 2, MAX_AVIS_FRAMES + 1, 1);
    assert!(check(&build_avis(b"avis", b"avis", None, (60, 60), &[too_many_stts])).is_err());
    let zero_frames = avis_track((16, 16), (16, 16), 60, 60, 0, 0, 1);
    assert!(check(&build_avis(b"avis", b"avis", None, (60, 60), &[zero_frames])).is_err());

    // Duration limits.
    let max_dur = MAX_AVIS_DURATION_SECS as u32;
    let ok_dur = avis_track((16, 16), (16, 16), 1, max_dur, 1, 1, max_dur);
    assert_eq!(check(&build_avis(b"avis", b"avis", None, (1, max_dur), &[ok_dur])), Ok(()));
    let too_long_stts = avis_track((16, 16), (16, 16), 1, 1, 1, 1, max_dur + 1);
    assert!(check(&build_avis(b"avis", b"avis", None, (1, 1), &[too_long_stts])).is_err());
    let too_long_mdhd = avis_track((16, 16), (16, 16), 1, max_dur + 1, 1, 1, 1);
    assert!(check(&build_avis(b"avis", b"avis", None, (1, 1), &[too_long_mdhd])).is_err());
    assert!(check(&build_avis(b"avis", b"avis", None, (1, max_dur + 1), &[ok_track.clone()])).is_err());
    let zero_ts = avis_track((16, 16), (16, 16), 0, 1, 1, 1, 1);
    assert!(check(&build_avis(b"avis", b"avis", None, (1, 1), &[zero_ts])).is_err());

    // Header scan byte limit: moov hidden past MAX_AVIS_SCAN_BYTES is refused.
    let mut past_scan = bmff_box(b"ftyp", b"avis\0\0\0\0avis");
    past_scan.extend_from_slice(&bmff_box(b"free", &vec![0u8; MAX_AVIS_SCAN_BYTES]));
    past_scan.extend_from_slice(&bmff_box(b"moov", &ok_track));
    assert!(check(&past_scan).is_err());
}

#[test]
fn web_header_check_matches_chrome_supported_and_refused_formats() {
    // Every raster format Chrome 158 decodes passes with valid dimensions.
    let small_av1 = av1_reduced_seq_hdr(16, 16, false);
    let ok_png = png_header(16, 16);
    let ok_avis_track = avis_track((16, 16), (16, 16), 1000, 500, 2, 2, 250);
    let accepted: &[(&str, Vec<u8>)] = &[
        ("PNG", ok_png.clone()),
        ("JPEG", jpeg_header(0xc0, 16, 16)),
        ("GIF", gif_header(b"GIF89a", 16, 16)),
        ("WebP", riff_webp(&[(b"VP8L", &webp_vp8l_payload(16, 16))])),
        ("BMP", bmp_info(16, 16)),
        ("ICO", ico_with_payloads(1, &[&ok_png])),
        ("CUR", ico_with_payloads(2, &[&ok_png])),
        ("AVIF still", minimal_avif(b"avif", (16, 16), &small_av1)),
        ("AVIF sequence", build_avis(b"avis", b"avis", Some((16, 16)), (1000, 500), &[ok_avis_track])),
        ("JPEG XL codestream", jxl_codestream(16, 16)),
        ("JPEG XL container", jxl_container(0, &[bmff_box(b"jxlc", &jxl_codestream(16, 16))])),
    ];
    for (name, bytes) in accepted {
        assert_eq!(check(bytes), Ok(()), "{name} should pass");
    }

    // Formats Chrome 158 does not decode stay refused, citing Chrome.
    let refused: &[(&str, Vec<u8>)] = &[
        ("TIFF (LE)", vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 0, 0]),
        ("TIFF (BE)", vec![b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 0]),
        ("HEIC", bmff_box(b"ftyp", b"heic\0\0\0\0mif1heic")),
        ("HEIF", bmff_box(b"ftyp", b"mif1\0\0\0\0mif1heic")),
        ("JPEG 2000", vec![0x00, 0x00, 0x00, 0x0c, b'j', b'P', b' ', b' ', 0x0d, 0x0a, 0x87, 0x0a]),
        ("JPEG 2000 codestream", vec![0xff, 0x4f, 0xff, 0x51, 0x00, 0x2f]),
        ("XBM", b"#define icon_width 1\n#define icon_height 1\nstatic unsigned char icon_bits[] = { 0x01 };".to_vec()),
        ("ICNS", vec![b'i', b'c', b'n', b's', 0, 0, 0, 8]),
        ("QOI", vec![b'q', b'o', b'i', b'f', 0, 0, 0, 1, 0, 0, 0, 1, 4, 0]),
        ("OpenEXR", vec![0x76, 0x2f, 0x31, 0x01, 0x02, 0, 0, 0]),
        ("Radiance HDR", b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 1\n\0\0\0\0".to_vec()),
        ("DDS", vec![b'D', b'D', b'S', b' ', 0x7c, 0, 0, 0]),
        ("PSD", vec![b'8', b'B', b'P', b'S', 0, 1, 0, 0, 0, 0, 0, 0]),
        ("PPM", b"P6\n1 1\n255\n\0\0\0".to_vec()),
        ("Farbfeld", vec![b'f', b'a', b'r', b'b', b'f', b'e', b'l', b'd', 0, 0, 0, 1, 0, 0, 0, 1]),
        ("TGA (RLE)", vec![0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 24, 0]),
    ];
    for (name, bytes) in refused {
        let err = check(bytes).expect_err(&format!("{name} should be refused"));
        assert!(err.0.contains("Chrome"), "{name}: error should mention Chrome, got: {err}");
    }
    // Uncompressed true-color TGA starts with 00 00 02 00 (same magic as CUR) and is refused by ICO validation.
    let tga_uncompressed = [0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 24, 0];
    assert!(check(&tga_uncompressed).is_err());
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
