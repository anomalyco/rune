use super::*;
use crate::icc::{MAX_PROFILE_BYTES, Transform};

fn display_p3_icc() -> Vec<u8> {
    moxcms::ColorProfile::new_display_p3().encode().unwrap()
}

fn adobe_rgb_icc() -> Vec<u8> {
    moxcms::ColorProfile::new_adobe_rgb().encode().unwrap()
}

fn srgb_icc() -> Vec<u8> {
    moxcms::ColorProfile::new_srgb().encode().unwrap()
}

fn encode_png_rgba(w: u32, h: u32, rgba: &[u8], icc: Option<Vec<u8>>) -> Vec<u8> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    let mut enc = image::codecs::png::PngEncoder::new(&mut out);
    if let Some(profile) = icc {
        enc.set_icc_profile(profile).unwrap();
    }
    enc.write_image(rgba, w, h, image::ExtendedColorType::Rgba8).unwrap();
    out
}

/// Wraps `raw` in a valid RFC 1950 zlib stream using uncompressed stored deflate blocks.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 32);
    out.extend_from_slice(&[0x78, 0x01]);
    let mut pos = 0;
    loop {
        let len = (raw.len() - pos).min(65_535);
        let is_final = pos + len == raw.len();
        out.push(u8::from(is_final));
        let len16 = len as u16;
        out.extend_from_slice(&len16.to_le_bytes());
        out.extend_from_slice(&(!len16).to_le_bytes());
        out.extend_from_slice(&raw[pos..pos + len]);
        pos += len;
        if is_final {
            break;
        }
    }
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in raw {
        s1 = (s1 + u32::from(b)) % 65_521;
        s2 = (s2 + s1) % 65_521;
    }
    out.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    out
}

fn push_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut c = !0u32;
    for &b in kind.iter().chain(body.iter()) {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1));
        }
    }
    out.extend_from_slice(&(!c).to_be_bytes());
}

/// Builds a valid 2-frame RGBA8 APNG (dispose=Background, blend=Source) with an optional `iCCP`
/// chunk, working on every target without extra crate dependencies.
fn build_apng(w: u32, h: u32, frame0: &[u8], frame1: &[u8], icc: Option<&[u8]>) -> Vec<u8> {
    let scanlines = |rgba: &[u8]| {
        let row_bytes = w as usize * 4;
        let mut raw = Vec::with_capacity((row_bytes + 1) * h as usize);
        for row in rgba.chunks_exact(row_bytes) {
            raw.push(0); // filter: None
            raw.extend_from_slice(row);
        }
        zlib_stored(&raw)
    };
    let fctl = |seq: u32| {
        let mut d = Vec::with_capacity(26);
        d.extend_from_slice(&seq.to_be_bytes());
        d.extend_from_slice(&w.to_be_bytes());
        d.extend_from_slice(&h.to_be_bytes());
        d.extend_from_slice(&0u32.to_be_bytes()); // x_offset
        d.extend_from_slice(&0u32.to_be_bytes()); // y_offset
        d.extend_from_slice(&100u16.to_be_bytes()); // delay_num
        d.extend_from_slice(&1000u16.to_be_bytes()); // delay_den
        d.push(1); // dispose_op = Background
        d.push(0); // blend_op = Source
        d
    };

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&w.to_be_bytes());
    ihdr[4..8].copy_from_slice(&h.to_be_bytes());
    ihdr[8] = 8; // 8-bit
    ihdr[9] = 6; // RGBA
    push_png_chunk(&mut png, b"IHDR", &ihdr);
    if let Some(profile) = icc {
        let mut iccp = b"ICC\0".to_vec();
        iccp.push(0); // deflate compression
        iccp.extend_from_slice(&zlib_stored(profile));
        push_png_chunk(&mut png, b"iCCP", &iccp);
    }
    let mut actl = Vec::with_capacity(8);
    actl.extend_from_slice(&2u32.to_be_bytes()); // num_frames
    actl.extend_from_slice(&0u32.to_be_bytes()); // num_plays (infinite)
    push_png_chunk(&mut png, b"acTL", &actl);
    push_png_chunk(&mut png, b"fcTL", &fctl(0));
    push_png_chunk(&mut png, b"IDAT", &scanlines(frame0));
    push_png_chunk(&mut png, b"fcTL", &fctl(1));
    let mut fdat = 2u32.to_be_bytes().to_vec();
    fdat.extend_from_slice(&scanlines(frame1));
    push_png_chunk(&mut png, b"fdAT", &fdat);
    push_png_chunk(&mut png, b"IEND", &[]);
    png
}

#[test]
fn display_p3_and_adobe_rgb_png_convert_to_srgb_reference_values() {
    let p3_pixels: [[u8; 4]; 7] = [
        [200, 120, 60, 255],
        [0, 200, 0, 255],
        [30, 200, 220, 255],
        [200, 60, 120, 255],
        [60, 120, 200, 255],
        [128, 128, 128, 255],
        [250, 250, 250, 255],
    ];
    // Exact CSS Color 4 Display P3 -> sRGB values (within <= 1 level of Apple ColorSync due to
    // fixed-point matrix / s15Fixed16 profile encoding):
    let expected_p3_srgb: [[u8; 4]; 7] = [
        [213, 115, 43, 255],
        [0, 204, 0, 255],
        [0, 204, 223, 255],
        [217, 42, 121, 255],
        [30, 122, 206, 255],
        [128, 128, 128, 255],
        [250, 250, 250, 255],
    ];
    let p3_raw: Vec<u8> = p3_pixels.into_iter().flatten().collect();
    let expected_raw: Vec<u8> = expected_p3_srgb.into_iter().flatten().collect();

    let p3_png = encode_png_rgba(7, 1, &p3_raw, Some(display_p3_icc()));
    let decoded = decode(&p3_png).unwrap();
    assert_eq!(decoded.mips[0].pixels, expected_raw, "decode() must convert Display P3 PNG to sRGB");
    assert_eq!(png_rgba8(&p3_png).unwrap().into_raw(), expected_raw, "png_rgba8() must convert Display P3 PNG to sRGB");

    // ICNS wrapping a Display P3 PNG also converts to sRGB.
    let mut icns = b"icns".to_vec();
    icns.extend_from_slice(&((p3_png.len() + 16) as u32).to_be_bytes());
    icns.extend_from_slice(b"icp4");
    icns.extend_from_slice(&((p3_png.len() + 8) as u32).to_be_bytes());
    icns.extend_from_slice(&p3_png);
    assert_eq!(decode(&icns).unwrap().mips[0].pixels, expected_raw);

    // Adobe RGB PNG matches reference sRGB values including dark tones.
    let adobe_raw = vec![200, 120, 60, 255, 10, 10, 10, 255];
    let adobe_png = encode_png_rgba(2, 1, &adobe_raw, Some(adobe_rgb_icc()));
    let adobe_decoded = decode(&adobe_png).unwrap();
    assert_eq!(adobe_decoded.mips[0].pixels, vec![224, 121, 53, 255, 2, 2, 2, 255]);
    assert_eq!(png_rgba8(&adobe_png).unwrap().into_raw(), vec![224, 121, 53, 255, 2, 2, 2, 255]);
}

#[test]
fn icc_conversion_preserves_alpha_and_runs_before_premultiplication_and_downsampling() {
    let alphas = [0u8, 1, 64, 128, 200, 255];
    let mut src = Vec::with_capacity(alphas.len() * 4);
    for a in alphas {
        src.extend_from_slice(&[200, 120, 60, a]);
    }
    let p3_png = encode_png_rgba(alphas.len() as u32, 1, &src, Some(display_p3_icc()));

    // Straight-alpha output from png_rgba8() keeps exact alpha and converts RGB to sRGB.
    let straight = png_rgba8(&p3_png).unwrap().into_raw();
    let mut expected_straight = Vec::with_capacity(alphas.len() * 4);
    for a in alphas {
        expected_straight.extend_from_slice(&[213, 115, 43, a]);
    }
    assert_eq!(straight, expected_straight);

    // decode() converts to sRGB BEFORE mip_chain() premultiplies in linear sRGB light.
    let decoded = decode(&p3_png).unwrap();
    let expected_premul = mip_chain(alphas.len() as u32, 1, &expected_straight).unwrap();
    assert_eq!(decoded.mips[0].pixels, expected_premul.mips[0].pixels);

    // load_scaled() converts to sRGB BEFORE linear-light downsampling.
    let solid_8x8 = [200u8, 120, 60, 255].repeat(8 * 8);
    let big_p3_png = encode_png_rgba(8, 8, &solid_8x8, Some(display_p3_icc()));
    let scaled = decode_scaled(&big_p3_png, 2).unwrap();
    assert_eq!((scaled.mips[0].width, scaled.mips[0].height), (2, 2));
    assert_eq!(scaled.mips[0].pixels, [213u8, 115, 43, 255].repeat(2 * 2));
}

#[test]
fn srgb_tagged_and_untagged_images_stay_byte_identical_via_identity_probe() {
    assert!(Transform::to_srgb(&srgb_icc()).is_none(), "sRGB profile must be skipped by the 125-colour probe");
    assert!(Transform::to_srgb(&display_p3_icc()).is_some(), "Display P3 must build a transform");

    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for i in 0u32..256 {
        pixels.extend_from_slice(&[(i as u8), ((i * 3) as u8), ((255 - i) as u8), if i % 5 == 0 { 128 } else { 255 }]);
    }
    let untagged = encode_png_rgba(16, 16, &pixels, None);
    let tagged = encode_png_rgba(16, 16, &pixels, Some(srgb_icc()));
    assert_eq!(decode(&tagged).unwrap().mips[0].pixels, decode(&untagged).unwrap().mips[0].pixels);
    assert_eq!(png_rgba8(&tagged).unwrap().into_raw(), png_rgba8(&untagged).unwrap().into_raw());
}

#[test]
fn non_rgb_oversized_empty_and_corrupt_profiles_fall_back_safely() {
    assert!(Transform::to_srgb(&[]).is_none());
    let oversized = vec![0u8; MAX_PROFILE_BYTES + 1];
    assert!(Transform::to_srgb(&oversized).is_none());

    // Grayscale ICC profile is non-RGB -> safe no-op.
    let gray_icc = moxcms::ColorProfile::new_gray_with_gamma(1.8).encode().unwrap();
    assert!(Transform::to_srgb(&gray_icc).is_none());
    let src = vec![200, 120, 60, 255, 80, 80, 80, 255];
    let gray_png = encode_png_rgba(2, 1, &src, Some(gray_icc));
    assert_eq!(decode(&gray_png).unwrap().mips[0].pixels, src);

    // CMYK colour space header ('CMYK' at offset 16..20) -> safe no-op.
    let mut cmyk_icc = display_p3_icc();
    cmyk_icc[16..20].copy_from_slice(b"CMYK");
    assert!(Transform::to_srgb(&cmyk_icc).is_none());

    // Corrupt, truncated, and fuzzed profiles never panic and never fail image decode.
    let base = display_p3_icc();
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut next_u64 = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for len in [1, 4, 32, 127, 128, 132, base.len() / 2, base.len() - 1] {
        let _ = Transform::to_srgb(&base[..len]);
    }
    for _ in 0..300 {
        let mut mutated = base.clone();
        let flips = (next_u64() % 16) + 1;
        for _ in 0..flips {
            let idx = (next_u64() as usize) % mutated.len();
            mutated[idx] = next_u64() as u8;
        }
        let _ = Transform::to_srgb(&mutated);
    }
    let broken_png = encode_png_rgba(2, 1, &src, Some(b"not a valid icc profile at all".to_vec()));
    assert_eq!(decode(&broken_png).unwrap().mips[0].pixels, src);

    // Empty and short slices passed directly to apply_rgba do not panic.
    let transform = Transform::to_srgb(&base).unwrap();
    let mut empty = Vec::new();
    transform.apply_rgba(&mut empty);
    let mut tail = vec![10, 20, 30];
    transform.apply_rgba(&mut tail);
    assert_eq!(tail, vec![10, 20, 30]);
}

#[test]
fn apng_converts_every_frame_from_display_p3_to_srgb() {
    let frame0: Vec<u8> = [[200, 120, 60, 255], [0, 200, 0, 128], [128, 128, 128, 255]]
        .into_iter()
        .flatten()
        .collect();
    let frame1: Vec<u8> = [[30, 200, 220, 255], [200, 60, 120, 255], [60, 120, 200, 255]]
        .into_iter()
        .flatten()
        .collect();

    let untagged = build_apng(3, 1, &frame0, &frame1, None);
    let raw_dec = decode(&untagged).unwrap();
    assert_eq!(&raw_dec.mips[0].pixels[0..4], &[200, 120, 60, 255]);
    assert_eq!(&raw_dec.motion.as_ref().unwrap().frames[0][0].pixels[0..4], &[30, 200, 220, 255]);

    let p3_apng = build_apng(3, 1, &frame0, &frame1, Some(&display_p3_icc()));
    let dec = decode(&p3_apng).unwrap();
    let expected_f0 = mip_chain(3, 1, &[213, 115, 43, 255, 0, 204, 0, 128, 128, 128, 128, 255]).unwrap();
    let expected_f1 = mip_chain(3, 1, &[0, 204, 223, 255, 217, 42, 121, 255, 30, 122, 206, 255]).unwrap();
    assert_eq!(dec.mips[0].pixels, expected_f0.mips[0].pixels, "APNG frame 0 converted to sRGB");
    let motion = dec.motion.as_ref().expect("APNG decoded as animation");
    assert_eq!(motion.frames.len(), 1);
    assert_eq!(motion.frames[0][0].pixels, expected_f1.mips[0].pixels, "APNG frame 1 converted to sRGB");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn jpeg_converts_display_p3_with_exif_orientation() {
    use image::ImageEncoder;
    let rgb = [0u8, 200, 0].repeat(16 * 8);
    let mut encoded = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 100);
    enc.set_icc_profile(display_p3_icc()).unwrap();
    enc.write_image(&rgb, 16, 8, image::ExtendedColorType::Rgb8).unwrap();

    // Splice an APP1 EXIF Orientation = 6 (90° CW) segment right after SOI to verify both EXIF
    // orientation and ICC conversion apply together.
    let mut tiff = b"II*\0".to_vec();
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    let mut jpeg = encoded[..2].to_vec();
    jpeg.extend_from_slice(&[0xff, 0xe1]);
    jpeg.extend_from_slice(&(app1.len() as u16 + 2).to_be_bytes());
    jpeg.extend_from_slice(&app1);
    jpeg.extend_from_slice(&encoded[2..]);

    let dec = decode(&jpeg).unwrap();
    assert_eq!((dec.mips[0].width, dec.mips[0].height), (8, 16));
    let px = &dec.mips[0].pixels[0..4];
    assert!(px[0] <= 2 && px[1].abs_diff(204) <= 2 && px[2] <= 2 && px[3] == 255, "unexpected Display P3 JPEG pixel: {px:?}");
}

#[cfg(not(target_arch = "wasm32"))]
fn build_lossless_gif(palette: &[[u8; 3]; 8], frames: &[&[u8]], icc: Option<&[u8]>) -> Vec<u8> {
    let w = frames[0].len() as u16;
    let h = 1u16;
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.push(0x82); // GCT present, 8 entries (2^(2+1))
    out.push(0);
    out.push(0);
    for rgb in palette {
        out.extend_from_slice(rgb);
    }
    if frames.len() > 1 {
        out.extend_from_slice(&[0x21, 0xff, 0x0b]);
        out.extend_from_slice(b"NETSCAPE2.0");
        out.extend_from_slice(&[0x03, 0x01, 0x00, 0x00, 0x00]);
    }
    if let Some(profile) = icc {
        out.extend_from_slice(&[0x21, 0xff, 0x0b]);
        out.extend_from_slice(b"ICCRGBG1012");
        for chunk in profile.chunks(255) {
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
        out.push(0);
    }
    for indices in frames {
        assert!(indices.len() <= 4, "4-bit LZW helper supports up to 4 pixels per frame");
        out.extend_from_slice(&[0x21, 0xf9, 0x04, 0x04, 10, 0, 0, 0]);
        out.push(0x2c);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.push(0);
        out.push(3); // LZW minimum code size = 3 -> 4-bit codes, clear=8, EOI=9
        let mut codes = Vec::with_capacity(indices.len() + 2);
        codes.push(8u8);
        codes.extend_from_slice(indices);
        codes.push(9u8);
        let mut packed = Vec::new();
        for pair in codes.chunks(2) {
            let lo = pair[0] & 0x0f;
            let hi = pair.get(1).copied().unwrap_or(0) & 0x0f;
            packed.push(lo | (hi << 4));
        }
        out.push(packed.len() as u8);
        out.extend_from_slice(&packed);
        out.push(0);
    }
    out.push(0x3b);
    out
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn gif_still_and_animated_convert_every_frame_from_display_p3_to_srgb() {
    let palette: [[u8; 3]; 8] = [
        [200, 120, 60],
        [0, 200, 0],
        [30, 200, 220],
        [200, 60, 120],
        [60, 120, 200],
        [128, 128, 128],
        [250, 250, 250],
        [0, 0, 0],
    ];
    let f0: &[u8] = &[0, 1, 5, 6];
    let f1: &[u8] = &[2, 3, 4, 5];

    // Untagged sanity check: raw values come through unchanged.
    let untagged = build_lossless_gif(&palette, &[f0, f1], None);
    let raw_dec = decode(&untagged).unwrap();
    assert_eq!(&raw_dec.mips[0].pixels[0..8], &[200, 120, 60, 255, 0, 200, 0, 255]);

    // Display P3 animated GIF: both frames convert to exact sRGB reference values.
    let p3_gif = build_lossless_gif(&palette, &[f0, f1], Some(&display_p3_icc()));
    let dec = decode(&p3_gif).unwrap();
    assert_eq!(
        dec.mips[0].pixels,
        vec![213, 115, 43, 255, 0, 204, 0, 255, 128, 128, 128, 255, 250, 250, 250, 255],
    );
    let motion = dec.motion.as_ref().expect("2-frame GIF decoded as animation");
    assert_eq!(motion.frames.len(), 1);
    assert_eq!(
        motion.frames[0][0].pixels,
        vec![0, 204, 223, 255, 217, 42, 121, 255, 30, 122, 206, 255, 128, 128, 128, 255],
    );

    // 1-frame Display P3 GIF (still) also converts to sRGB.
    let p3_still_gif = build_lossless_gif(&palette, &[f0], Some(&display_p3_icc()));
    let still_dec = decode(&p3_still_gif).unwrap();
    assert!(still_dec.motion.is_none());
    assert_eq!(
        still_dec.mips[0].pixels,
        vec![213, 115, 43, 255, 0, 204, 0, 255, 128, 128, 128, 255, 250, 250, 250, 255],
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn push_riff_chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn encode_webp_vp8l_chunk(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    use image::ImageEncoder;
    let mut still = Vec::new();
    let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut still);
    enc.write_image(rgba, w, h, image::ExtendedColorType::Rgba8).unwrap();
    // Skip 12-byte RIFF....WEBP header and return the VP8L chunk (header + payload + optional pad).
    assert_eq!(&still[12..16], b"VP8L");
    still[12..].to_vec()
}

#[cfg(not(target_arch = "wasm32"))]
fn build_animated_webp(w: u32, h: u32, frame0: &[u8], frame1: &[u8], icc: Option<&[u8]>) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    let mut vp8x = [0u8; 10];
    vp8x[0] = (1 << 4) | (1 << 1) | if icc.is_some() { 1 << 5 } else { 0 }; // alpha + anim (+ icc)
    let wm1 = (w - 1).to_le_bytes();
    let hm1 = (h - 1).to_le_bytes();
    vp8x[4..7].copy_from_slice(&wm1[..3]);
    vp8x[7..10].copy_from_slice(&hm1[..3]);
    push_riff_chunk(&mut body, b"VP8X", &vp8x);
    if let Some(profile) = icc {
        push_riff_chunk(&mut body, b"ICCP", profile);
    }
    push_riff_chunk(&mut body, b"ANIM", &[0, 0, 0, 0, 0, 0]);
    for frame in [frame0, frame1] {
        let mut anmf = Vec::new();
        anmf.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // x = 0, y = 0
        anmf.extend_from_slice(&wm1[..3]);
        anmf.extend_from_slice(&hm1[..3]);
        anmf.extend_from_slice(&100u32.to_le_bytes()[..3]); // 100 ms duration
        anmf.push(0x02); // do not alpha-blend over canvas
        anmf.extend_from_slice(&encode_webp_vp8l_chunk(w, h, frame));
        push_riff_chunk(&mut body, b"ANMF", &anmf);
    }
    let mut riff = b"RIFF".to_vec();
    riff.extend_from_slice(&(body.len() as u32).to_le_bytes());
    riff.extend_from_slice(&body);
    riff
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn webp_still_and_animated_convert_from_display_p3_to_srgb() {
    use image::ImageEncoder;
    let f0: Vec<u8> = [[200, 120, 60, 255], [0, 200, 0, 255], [128, 128, 128, 255], [250, 250, 250, 255]]
        .into_iter()
        .flatten()
        .collect();
    let f1: Vec<u8> = [[30, 200, 220, 255], [200, 60, 120, 255], [60, 120, 200, 255], [128, 128, 128, 255]]
        .into_iter()
        .flatten()
        .collect();

    // Still Display P3 WebP.
    let mut still = Vec::new();
    let mut enc = image::codecs::webp::WebPEncoder::new_lossless(&mut still);
    enc.set_icc_profile(display_p3_icc()).unwrap();
    enc.write_image(&f0, 4, 1, image::ExtendedColorType::Rgba8).unwrap();
    let still_dec = decode(&still).unwrap();
    assert!(still_dec.motion.is_none());
    assert_eq!(
        still_dec.mips[0].pixels,
        vec![213, 115, 43, 255, 0, 204, 0, 255, 128, 128, 128, 255, 250, 250, 250, 255],
    );

    // Animated Display P3 WebP: both frames convert to exact sRGB reference values.
    let untagged = build_animated_webp(4, 1, &f0, &f1, None);
    assert_eq!(&decode(&untagged).unwrap().mips[0].pixels[0..4], &[200, 120, 60, 255]);

    let p3_anim = build_animated_webp(4, 1, &f0, &f1, Some(&display_p3_icc()));
    let dec = decode(&p3_anim).unwrap();
    assert_eq!(
        dec.mips[0].pixels,
        vec![213, 115, 43, 255, 0, 204, 0, 255, 128, 128, 128, 255, 250, 250, 250, 255],
    );
    let motion = dec.motion.as_ref().expect("2-frame WebP decoded as animation");
    assert_eq!(motion.frames.len(), 1);
    assert_eq!(
        motion.frames[0][0].pixels,
        vec![0, 204, 223, 255, 217, 42, 121, 255, 30, 122, 206, 255, 128, 128, 128, 255],
    );
}
