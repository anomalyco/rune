//! Pre-decode image header validation for the web path, where the browser's `createImageBitmap`
//! otherwise decodes the full raster before Rune sees its dimensions. Compiles on every
//! platform so the same limits (`MAX_SIDE`, `MAX_ALLOC`) and AVIF validator run everywhere and
//! are unit-tested natively.
use super::{ImageError, MAX_ALLOC, MAX_SIDE, avif};

/// Checks the declared dimensions in `bytes` against [`MAX_SIDE`] and [`MAX_ALLOC`] before
/// handing encoded image bytes to the browser's decoder. Rejects oversized, zero-sized,
/// malformed and unrecognized inputs so no unvalidated raster reaches `createImageBitmap`.
pub(super) fn check(bytes: &[u8]) -> Result<(), ImageError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        check_png(bytes)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        check_jpeg(bytes)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        check_gif(bytes)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        check_webp(bytes)
    } else if bytes.starts_with(b"BM") {
        check_bmp(bytes)
    } else if is_ico(bytes) {
        check_ico(bytes)
    } else if avif::is_avif(bytes) {
        avif::check_limits(bytes)
    } else {
        Err(ImageError("unsupported image format".into()))
    }
}

fn check_dimensions(w: u32, h: u32) -> Result<(), ImageError> {
    if w == 0 || h == 0 {
        return Err(ImageError(format!("{w}×{h} has zero dimension")));
    }
    if w > MAX_SIDE || h > MAX_SIDE {
        return Err(ImageError(format!("{w}×{h} is over {MAX_SIDE} pixels a side")));
    }
    if (u64::from(w) * u64::from(h)).saturating_mul(4) > MAX_ALLOC {
        return Err(ImageError(format!("{w}×{h} exceeds maximum decoded size")));
    }
    Ok(())
}

fn check_png(bytes: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid PNG header".into());
    let head = bytes.get(..24).ok_or_else(invalid)?;
    let ihdr_len = u32::from_be_bytes(head[8..12].try_into().unwrap());
    if ihdr_len != 13 || &head[12..16] != b"IHDR" {
        return Err(invalid());
    }
    let w = u32::from_be_bytes(head[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(head[20..24].try_into().unwrap());
    check_dimensions(w, h)
}

fn check_jpeg(bytes: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid JPEG header".into());
    let mut i = 2usize;
    while i < bytes.len() {
        if bytes[i] != 0xff { return Err(invalid()); }
        while i < bytes.len() && bytes[i] == 0xff { i += 1; }
        if i >= bytes.len() { return Err(invalid()); }
        let marker = bytes[i];
        i += 1;
        match marker {
            // Standalone markers with no length payload.
            0x01 | 0xd0..=0xd8 => continue,
            // Byte stuffing outside entropy data, EOI, or SOS before any SOF.
            0x00 | 0xd9 | 0xda => return Err(invalid()),
            // Start of Frame markers (excluding 0xC4 DHT, 0xC8 JPG, 0xCC DAC).
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                let seg = i.checked_add(7).and_then(|end| bytes.get(i..end)).ok_or_else(invalid)?;
                let len = u16::from_be_bytes([seg[0], seg[1]]) as usize;
                if len < 8 { return Err(invalid()); }
                let h = u32::from(u16::from_be_bytes([seg[3], seg[4]]));
                let w = u32::from(u16::from_be_bytes([seg[5], seg[6]]));
                return check_dimensions(w, h);
            }
            // All other markers carry a 2-byte big-endian length (including the 2 length bytes).
            _ => {
                let len_bytes = i.checked_add(2).and_then(|end| bytes.get(i..end)).ok_or_else(invalid)?;
                let len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
                if len < 2 { return Err(invalid()); }
                i = i.checked_add(len).filter(|&end| end <= bytes.len()).ok_or_else(invalid)?;
            }
        }
    }
    Err(invalid())
}

fn check_gif(bytes: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid GIF header".into());
    let head = bytes.get(..10).ok_or_else(invalid)?;
    let w = u32::from(u16::from_le_bytes([head[6], head[7]]));
    let h = u32::from(u16::from_le_bytes([head[8], head[9]]));
    check_dimensions(w, h)?;
    // If the stream continues past the Logical Screen Descriptor, also bound the first image
    // descriptor so a small logical screen cannot hide an oversized frame.
    if bytes.len() < 13 { return Ok(()); }
    let packed = bytes[10];
    let mut i = 13usize;
    if packed & 0x80 != 0 {
        let gct = 3usize << ((packed & 0x07) + 1);
        i = match i.checked_add(gct) {
            Some(next) if next <= bytes.len() => next,
            _ => return Ok(()),
        };
    }
    while i < bytes.len() {
        match bytes[i] {
            0x21 => {
                i = match i.checked_add(2) { Some(next) => next, None => return Ok(()) };
                while i < bytes.len() {
                    let n = usize::from(bytes[i]);
                    i += 1;
                    if n == 0 { break; }
                    i = match i.checked_add(n) { Some(next) => next, None => return Ok(()) };
                }
            }
            0x2c => {
                let desc = i.checked_add(10).and_then(|end| bytes.get(i + 1..end)).ok_or_else(invalid)?;
                let fw = u32::from(u16::from_le_bytes([desc[4], desc[5]]));
                let fh = u32::from(u16::from_le_bytes([desc[6], desc[7]]));
                return check_dimensions(fw, fh);
            }
            _ => break,
        }
    }
    Ok(())
}

fn check_webp(bytes: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid WebP header".into());
    let (first_fourcc, first_payload, mut pos) = read_riff_chunk(bytes, 12).ok_or_else(invalid)?;
    match &first_fourcc {
        b"VP8 " => check_vp8(first_payload),
        b"VP8L" => check_vp8l(first_payload),
        b"VP8X" => {
            check_vp8x(first_payload)?;
            // Also bound any bitstream or animation frame chunks inside the extended container.
            while let Some((fourcc, payload, next)) = read_riff_chunk(bytes, pos) {
                match &fourcc {
                    b"VP8 " => check_vp8(payload)?,
                    b"VP8L" => check_vp8l(payload)?,
                    b"ANMF" => check_webp_anmf(payload)?,
                    _ => {}
                }
                pos = next;
            }
            Ok(())
        }
        _ => Err(invalid()),
    }
}

fn read_riff_chunk(bytes: &[u8], pos: usize) -> Option<([u8; 4], &[u8], usize)> {
    let header = bytes.get(pos..pos.checked_add(8)?)?;
    let fourcc: [u8; 4] = header[0..4].try_into().unwrap();
    let size = usize::try_from(u32::from_le_bytes(header[4..8].try_into().unwrap())).ok()?;
    let start = pos + 8;
    let avail = bytes.len() - start;
    let payload = &bytes[start..start + size.min(avail)];
    let next = start.saturating_add(size).saturating_add(size & 1).min(bytes.len());
    Some((fourcc, payload, next))
}

fn check_vp8(data: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid WebP VP8 header".into());
    if data.len() < 10 || data[0] & 1 != 0 || &data[3..6] != &[0x9d, 0x01, 0x2a] {
        return Err(invalid());
    }
    let w = u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3fff);
    let h = u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3fff);
    check_dimensions(w, h)
}

fn check_vp8l(data: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid WebP VP8L header".into());
    if data.len() < 5 || data[0] != 0x2f {
        return Err(invalid());
    }
    let bits = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
    let w = (bits & 0x3fff) + 1;
    let h = ((bits >> 14) & 0x3fff) + 1;
    check_dimensions(w, h)
}

fn check_vp8x(data: &[u8]) -> Result<(), ImageError> {
    if data.len() < 10 { return Err(ImageError("invalid WebP VP8X header".into())); }
    let w = 1 + u24_le(&data[4..7]);
    let h = 1 + u24_le(&data[7..10]);
    check_dimensions(w, h)
}

fn check_webp_anmf(data: &[u8]) -> Result<(), ImageError> {
    if data.len() < 16 { return Err(ImageError("invalid WebP ANMF chunk".into())); }
    let w = 1 + u24_le(&data[6..9]);
    let h = 1 + u24_le(&data[9..12]);
    check_dimensions(w, h)?;
    let mut pos = 16usize;
    while let Some((fourcc, payload, next)) = read_riff_chunk(data, pos) {
        match &fourcc {
            b"VP8 " => check_vp8(payload)?,
            b"VP8L" => check_vp8l(payload)?,
            _ => {}
        }
        pos = next;
    }
    Ok(())
}

fn u24_le(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn check_bmp(bytes: &[u8]) -> Result<(), ImageError> {
    let dib = bytes.get(14..).ok_or_else(|| ImageError("invalid BMP header".into()))?;
    check_bmp_dib(dib, false)
}

fn check_bmp_dib(dib: &[u8], ico_halved_height: bool) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid BMP header".into());
    let size = u32::from_le_bytes(dib.get(..4).ok_or_else(invalid)?.try_into().unwrap());
    let (w, mut h) = if size == 12 {
        let head = dib.get(4..8).ok_or_else(invalid)?;
        (u32::from(u16::from_le_bytes([head[0], head[1]])), u32::from(u16::from_le_bytes([head[2], head[3]])))
    } else if size >= 16 {
        let head = dib.get(4..12).ok_or_else(invalid)?;
        let w = i32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        if w <= 0 { return Err(invalid()); }
        let h = i32::from_le_bytes([head[4], head[5], head[6], head[7]]).unsigned_abs();
        (w as u32, h)
    } else {
        return Err(invalid());
    };
    if ico_halved_height { h /= 2; }
    check_dimensions(w, h)
}

fn is_ico(bytes: &[u8]) -> bool {
    bytes.len() >= 6 && bytes[0] == 0 && bytes[1] == 0 && matches!(bytes[2], 1 | 2) && bytes[3] == 0
}

fn check_ico(bytes: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid ICO header".into());
    let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    if count == 0 { return Err(invalid()); }
    let dir_end = count.checked_mul(16).and_then(|n| n.checked_add(6)).ok_or_else(invalid)?;
    if dir_end > bytes.len() { return Err(invalid()); }
    for idx in 0..count {
        let entry = &bytes[6 + idx * 16..6 + (idx + 1) * 16];
        let size = usize::try_from(u32::from_le_bytes(entry[8..12].try_into().unwrap())).map_err(|_| invalid())?;
        let offset = usize::try_from(u32::from_le_bytes(entry[12..16].try_into().unwrap())).map_err(|_| invalid())?;
        if size == 0 { return Err(invalid()); }
        let end = offset.checked_add(size).filter(|&end| end <= bytes.len()).ok_or_else(invalid)?;
        let payload = &bytes[offset..end];
        if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
            check_png(payload)?;
        } else {
            check_bmp_dib(payload, true)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "header_tests.rs"]
mod tests;
