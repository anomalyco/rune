//! Pre-decode image header validation for the web path, where the browser's `createImageBitmap`
//! otherwise decodes the full raster before Rune sees its dimensions. Compiles on every
//! platform so the same limits (`MAX_SIDE`, `MAX_ALLOC`) and format validators run everywhere and
//! are unit-tested natively.
//!
//! Product rule: on the web, Rune accepts and refuses the same raster formats as Google Chrome
//! (checked against Chrome 158): PNG (including APNG), JPEG, GIF, WebP (including animated
//! WebP), BMP, ICO/CUR, AVIF (stills and `avis` image sequences), and JPEG XL (bare codestream
//! and ISOBMFF container). SVG is handled separately before this check. The same Chrome list
//! applies in Safari and Firefox so an app accepts and refuses the same formats in every
//! browser instead of relying on browser-specific decoders such as TIFF, HEIC/HEIF or JPEG 2000.
use super::{ImageError, MAX_ALLOC, MAX_SIDE, avif};

/// 12-byte JPEG XL ISOBMFF container signature box (ISO/IEC 18181-2).
const JXL_CONTAINER_SIG: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a,
];
/// Maximum file prefix (in bytes) scanned for AVIF sequence (`avis`) container metadata.
pub(super) const MAX_AVIS_SCAN_BYTES: usize = 256 * 1024;
/// Maximum number of frames accepted in a web AVIF sequence (`avis`).
pub(super) const MAX_AVIS_FRAMES: u32 = 3_600;
/// Maximum single-play duration (in seconds) accepted in a web AVIF sequence (`avis`).
pub(super) const MAX_AVIS_DURATION_SECS: u64 = 300;

/// Checks the declared dimensions in `bytes` against [`MAX_SIDE`] and [`MAX_ALLOC`] before
/// handing encoded image bytes to the browser's decoder.
///
/// Web accepts what Chrome decodes (PNG, JPEG, GIF, WebP, BMP, ICO/CUR, AVIF stills and
/// sequences, and JPEG XL) in every browser, including Safari and Firefox, so format support is
/// uniform across browsers. Animated formats decoded through `createImageBitmap` (GIF, WebP,
/// animated AVIF and animated JPEG XL) show their first frame on the web. Rejects oversized,
/// zero-sized, malformed and non-Chrome formats so no unvalidated raster reaches
/// `createImageBitmap`.
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
    } else if let Some(kind) = avif_kind(bytes) {
        match kind {
            AvifKind::Still => avif::check_limits(bytes),
            AvifKind::Sequence => check_avis(bytes),
        }
    } else if bytes.starts_with(&[0xff, 0x0a]) {
        check_jxl_codestream(bytes)
    } else if bytes.starts_with(&JXL_CONTAINER_SIG) {
        check_jxl_container(bytes)
    } else {
        Err(ImageError("unsupported image format (web accepts only formats Chrome decodes)".into()))
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

enum AvifKind {
    Still,
    Sequence,
}

fn avif_kind(bytes: &[u8]) -> Option<AvifKind> {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return None;
    }
    let size32 = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    let (hdr, end) = match size32 {
        0 => (8usize, bytes.len()),
        1 => {
            let wide = bytes.get(8..16)?;
            let size = usize::try_from(u64::from_be_bytes(wide.try_into().unwrap())).ok()?;
            if size < 16 { return None; }
            (16, size.min(bytes.len()))
        }
        s if s < 8 => return None,
        s => (8, (s as usize).min(bytes.len())),
    };
    let payload = bytes.get(hdr..end)?;
    let major = payload.get(0..4)?;
    if major == b"avis" {
        return Some(AvifKind::Sequence);
    }
    if major == b"avif" {
        return Some(AvifKind::Still);
    }
    // Check up to 32 compatible brands, matching Chromium's AVIF sniffer.
    if payload.len() >= 12
        && payload[8..].chunks_exact(4).take(32).any(|brand| brand == b"avis")
    {
        return Some(AvifKind::Sequence);
    }
    None
}

fn read_bmff_box<'a>(
    bytes: &'a [u8],
    pos: usize,
    invalid: impl Fn() -> ImageError,
) -> Result<Option<([u8; 4], &'a [u8], usize)>, ImageError> {
    if pos >= bytes.len() {
        return Ok(None);
    }
    let header = pos.checked_add(8).and_then(|end| bytes.get(pos..end)).ok_or_else(&invalid)?;
    let size32 = u32::from_be_bytes(header[0..4].try_into().unwrap());
    let kind: [u8; 4] = header[4..8].try_into().unwrap();
    let (payload_start, end) = match size32 {
        0 => (pos + 8, bytes.len()),
        1 => {
            let wide = pos.checked_add(16).and_then(|end| bytes.get(pos + 8..end)).ok_or_else(&invalid)?;
            let size64 = u64::from_be_bytes(wide.try_into().unwrap());
            if size64 < 16 { return Err(invalid()); }
            let size = usize::try_from(size64).map_err(|_| invalid())?;
            let end = pos.checked_add(size).filter(|&end| end <= bytes.len()).ok_or_else(&invalid)?;
            (pos + 16, end)
        }
        s if s < 8 => return Err(invalid()),
        s => {
            let end = pos.checked_add(s as usize).filter(|&end| end <= bytes.len()).ok_or_else(&invalid)?;
            (pos + 8, end)
        }
    };
    Ok(Some((kind, &bytes[payload_start..end], end)))
}

fn check_avis(bytes: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid AVIF sequence header".into());
    let mut pos = 0usize;
    let mut saw_ftyp = false;
    let mut saw_meta = false;
    let mut saw_moov = false;
    let mut valid_av01_tracks = 0usize;

    while pos < bytes.len() && pos < MAX_AVIS_SCAN_BYTES {
        let Some((kind, payload, next)) = read_bmff_box(bytes, pos, invalid)? else { break };
        pos = next;
        if !saw_ftyp {
            if &kind != b"ftyp" || next > MAX_AVIS_SCAN_BYTES { return Err(invalid()); }
            saw_ftyp = true;
            continue;
        }
        match &kind {
            b"meta" => {
                if saw_meta { return Err(invalid()); }
                if next > MAX_AVIS_SCAN_BYTES {
                    return Err(ImageError("AVIF sequence metadata exceeds header scan limit".into()));
                }
                saw_meta = true;
                check_avis_meta(payload)?;
            }
            b"moov" => {
                if saw_moov { return Err(invalid()); }
                if next > MAX_AVIS_SCAN_BYTES {
                    return Err(ImageError("AVIF sequence metadata exceeds header scan limit".into()));
                }
                saw_moov = true;
                valid_av01_tracks = check_avis_moov(payload)?;
            }
            // Skip media data and auxiliary boxes without scanning their payload.
            _ => {}
        }
    }
    if !saw_moov || valid_av01_tracks == 0 {
        return Err(ImageError("AVIF sequence metadata missing or beyond header scan limit".into()));
    }
    Ok(())
}

fn check_avis_meta(data: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid AVIF sequence meta box".into());
    let head = data.get(..4).ok_or_else(invalid)?;
    if head != [0, 0, 0, 0] { return Err(invalid()); }
    let mut pos = 4usize;
    while let Some((kind, payload, next)) = read_bmff_box(data, pos, invalid)? {
        pos = next;
        if &kind == b"iprp" {
            let mut p = 0usize;
            while let Some((ckind, cpayload, cnext)) = read_bmff_box(payload, p, invalid)? {
                p = cnext;
                if &ckind == b"ipco" {
                    let mut q = 0usize;
                    while let Some((pkind, ppayload, pnext)) = read_bmff_box(cpayload, q, invalid)? {
                        q = pnext;
                        if &pkind == b"ispe" {
                            let ispe = ppayload.get(..12).ok_or_else(invalid)?;
                            if ispe[..4] != [0, 0, 0, 0] { return Err(invalid()); }
                            let w = u32::from_be_bytes(ispe[4..8].try_into().unwrap());
                            let h = u32::from_be_bytes(ispe[8..12].try_into().unwrap());
                            check_dimensions(w, h)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn check_avis_duration(timescale: u32, duration: u64) -> Result<(), ImageError> {
    if timescale == 0 {
        return Err(ImageError("AVIF sequence has zero timescale".into()));
    }
    let max_ticks = u128::from(timescale) * u128::from(MAX_AVIS_DURATION_SECS);
    if u128::from(duration) > max_ticks {
        return Err(ImageError(format!(
            "AVIF sequence duration exceeds {MAX_AVIS_DURATION_SECS}s limit"
        )));
    }
    Ok(())
}

/// Parses `(timescale, finite_duration)` from an ISO-BMFF v0 or v1 `mvhd`/`mdhd` FullBox.
fn read_bmff_timescale_duration(
    payload: &[u8],
    invalid: impl Fn() -> ImageError,
) -> Result<(u32, Option<u64>), ImageError> {
    let (ts, dur, unknown) = match *payload.first().ok_or_else(&invalid)? {
        0 => {
            let d = payload.get(..20).ok_or_else(&invalid)?;
            (
                u32::from_be_bytes(d[12..16].try_into().unwrap()),
                u64::from(u32::from_be_bytes(d[16..20].try_into().unwrap())),
                u64::from(u32::MAX),
            )
        }
        1 => {
            let d = payload.get(..32).ok_or_else(&invalid)?;
            (
                u32::from_be_bytes(d[20..24].try_into().unwrap()),
                u64::from_be_bytes(d[24..32].try_into().unwrap()),
                u64::MAX,
            )
        }
        _ => return Err(invalid()),
    };
    let finite = (dur > 0 && dur != unknown && dur != i64::MAX as u64).then_some(dur);
    Ok((ts, finite))
}

fn check_avis_moov(data: &[u8]) -> Result<usize, ImageError> {
    let invalid = || ImageError("invalid AVIF sequence moov box".into());
    let mut pos = 0usize;
    let mut valid_av01_tracks = 0usize;
    while let Some((kind, payload, next)) = read_bmff_box(data, pos, invalid)? {
        pos = next;
        match &kind {
            b"mvhd" => {
                let (ts, finite) = read_bmff_timescale_duration(payload, invalid)?;
                if let Some(dur) = finite { check_avis_duration(ts, dur)?; }
            }
            b"trak" => {
                if check_avis_track(payload)? { valid_av01_tracks += 1; }
            }
            _ => {}
        }
    }
    Ok(valid_av01_tracks)
}

fn check_avis_track(data: &[u8]) -> Result<bool, ImageError> {
    let invalid = || ImageError("invalid AVIF sequence track".into());
    let mut pos = 0usize;
    let mut timescale: Option<u32> = None;
    let mut has_av01 = false;
    let mut stsz_frames: Option<u32> = None;
    let mut stts: Option<(u32, u128)> = None;

    while let Some((kind, payload, next)) = read_bmff_box(data, pos, invalid)? {
        pos = next;
        match &kind {
            b"tkhd" => {
                let (off, need) = match *payload.first().ok_or_else(invalid)? {
                    0 => (76usize, 84usize),
                    1 => (88, 96),
                    _ => return Err(invalid()),
                };
                let d = payload.get(..need).ok_or_else(invalid)?;
                let w_fp = u32::from_be_bytes(d[off..off + 4].try_into().unwrap());
                let h_fp = u32::from_be_bytes(d[off + 4..off + 8].try_into().unwrap());
                if w_fp != 0 || h_fp != 0 {
                    check_dimensions(w_fp.saturating_add(0xffff) >> 16, h_fp.saturating_add(0xffff) >> 16)?;
                }
            }
            b"mdia" => {
                let mut mp = 0usize;
                while let Some((mkind, mpayload, mnext)) = read_bmff_box(payload, mp, invalid)? {
                    mp = mnext;
                    match &mkind {
                        b"mdhd" => {
                            let (ts, finite) = read_bmff_timescale_duration(mpayload, invalid)?;
                            if ts == 0 { return Err(ImageError("AVIF sequence has zero timescale".into())); }
                            if let Some(dur) = finite { check_avis_duration(ts, dur)?; }
                            timescale = Some(ts);
                        }
                        b"minf" => {
                            let mut np = 0usize;
                            while let Some((nkind, npayload, nnext)) = read_bmff_box(mpayload, np, invalid)? {
                                np = nnext;
                                if &nkind == b"stbl" {
                                    check_avis_stbl(npayload, &mut has_av01, &mut stsz_frames, &mut stts)?;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if !has_av01 {
        return Ok(false);
    }
    let ts = timescale.ok_or_else(invalid)?;
    let frames = stsz_frames.ok_or_else(invalid)?;
    let (stts_frames, total_ticks) = stts.ok_or_else(invalid)?;
    if frames == 0 || frames > MAX_AVIS_FRAMES || stts_frames == 0 || stts_frames > MAX_AVIS_FRAMES {
        return Err(ImageError(format!("AVIF sequence exceeds {MAX_AVIS_FRAMES} frame limit")));
    }
    let max_ticks = u128::from(ts) * u128::from(MAX_AVIS_DURATION_SECS);
    if total_ticks > max_ticks {
        return Err(ImageError(format!(
            "AVIF sequence duration exceeds {MAX_AVIS_DURATION_SECS}s limit"
        )));
    }
    Ok(true)
}

fn check_avis_stbl(
    data: &[u8],
    has_av01: &mut bool,
    stsz_frames: &mut Option<u32>,
    stts: &mut Option<(u32, u128)>,
) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid AVIF sequence sample table".into());
    let mut pos = 0usize;
    while let Some((kind, payload, next)) = read_bmff_box(data, pos, invalid)? {
        pos = next;
        match &kind {
            b"stsd" => {
                let entries = payload.get(8..).ok_or_else(invalid)?;
                let mut ep = 0usize;
                while let Some((ekind, epayload, enext)) = read_bmff_box(entries, ep, invalid)? {
                    ep = enext;
                    if &ekind == b"av01" {
                        let dims = epayload.get(24..28).ok_or_else(invalid)?;
                        let w = u32::from(u16::from_be_bytes([dims[0], dims[1]]));
                        let h = u32::from(u16::from_be_bytes([dims[2], dims[3]]));
                        check_dimensions(w, h)?;
                        *has_av01 = true;
                    }
                }
            }
            b"stsz" => {
                let head = payload.get(..12).ok_or_else(invalid)?;
                let sample_size = u32::from_be_bytes(head[4..8].try_into().unwrap());
                let sample_count = u32::from_be_bytes(head[8..12].try_into().unwrap());
                if sample_count == 0 || sample_count > MAX_AVIS_FRAMES {
                    return Err(ImageError(format!("AVIF sequence exceeds {MAX_AVIS_FRAMES} frame limit")));
                }
                if sample_size == 0 {
                    let needed = (sample_count as usize).checked_mul(4)
                        .and_then(|n| n.checked_add(12))
                        .ok_or_else(invalid)?;
                    if payload.len() < needed { return Err(invalid()); }
                }
                *stsz_frames = Some(sample_count);
            }
            b"stts" => {
                let head = payload.get(..8).ok_or_else(invalid)?;
                let entry_count = u32::from_be_bytes(head[4..8].try_into().unwrap());
                if entry_count == 0 || entry_count > MAX_AVIS_FRAMES {
                    return Err(ImageError(format!("AVIF sequence exceeds {MAX_AVIS_FRAMES} frame limit")));
                }
                let table = payload.get(8..).ok_or_else(invalid)?;
                let needed = (entry_count as usize).checked_mul(8).ok_or_else(invalid)?;
                if table.len() < needed { return Err(invalid()); }
                let mut total_frames = 0u32;
                let mut total_ticks = 0u128;
                for chunk in table[..needed].chunks_exact(8) {
                    let count = u32::from_be_bytes(chunk[0..4].try_into().unwrap());
                    let delta = u32::from_be_bytes(chunk[4..8].try_into().unwrap());
                    total_frames = total_frames.saturating_add(count);
                    if total_frames > MAX_AVIS_FRAMES {
                        return Err(ImageError(format!("AVIF sequence exceeds {MAX_AVIS_FRAMES} frame limit")));
                    }
                    total_ticks = total_ticks.saturating_add(u128::from(count) * u128::from(delta));
                }
                *stts = Some((total_frames, total_ticks));
            }
            _ => {}
        }
    }
    Ok(())
}

/// LSB-first bit reader for JPEG XL codestream `SizeHeader` (ISO/IEC 18181-1 §11.2).
struct JxlBits<'a> {
    bytes: &'a [u8],
    bit_pos: usize,
}

impl<'a> JxlBits<'a> {
    fn new(bytes: &'a [u8]) -> Self { Self { bytes, bit_pos: 0 } }

    fn read(&mut self, n: usize) -> Option<u32> {
        debug_assert!(n <= 30);
        let end = self.bit_pos.checked_add(n)?;
        if end > self.bytes.len().saturating_mul(8) { return None; }
        let mut val = 0u32;
        for i in 0..n {
            let pos = self.bit_pos + i;
            let bit = (self.bytes[pos / 8] >> (pos % 8)) & 1;
            val |= u32::from(bit) << i;
        }
        self.bit_pos = end;
        Some(val)
    }

    fn read_u2s(&mut self) -> Option<u32> {
        let bits = match self.read(2)? {
            0 => 9,
            1 => 13,
            2 => 18,
            _ => 30,
        };
        self.read(bits)
    }
}

fn check_jxl_codestream(bytes: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid JPEG XL header".into());
    if !bytes.starts_with(&[0xff, 0x0a]) {
        return Err(invalid());
    }
    let mut bits = JxlBits::new(&bytes[2..]);
    let small = bits.read(1).ok_or_else(invalid)? != 0;
    let ysize = if small {
        (bits.read(5).ok_or_else(invalid)? + 1) * 8
    } else {
        bits.read_u2s().ok_or_else(invalid)? + 1
    };
    let ratio = bits.read(3).ok_or_else(invalid)?;
    let xsize: u64 = match ratio {
        0 => {
            if small {
                u64::from(bits.read(5).ok_or_else(invalid)? + 1) * 8
            } else {
                u64::from(bits.read_u2s().ok_or_else(invalid)? + 1)
            }
        }
        1 => u64::from(ysize),
        2 => u64::from(ysize) * 12 / 10,
        3 => u64::from(ysize) * 4 / 3,
        4 => u64::from(ysize) * 3 / 2,
        5 => u64::from(ysize) * 16 / 9,
        6 => u64::from(ysize) * 5 / 4,
        _ => u64::from(ysize) * 2,
    };
    let w = u32::try_from(xsize).unwrap_or(u32::MAX);
    check_dimensions(w, ysize)
}

fn append_jxl_head(buf: &mut [u8; 16], len: &mut usize, chunk: &[u8]) {
    let take = (buf.len() - *len).min(chunk.len());
    buf[*len..*len + take].copy_from_slice(&chunk[..take]);
    *len += take;
}

fn check_jxl_container(bytes: &[u8]) -> Result<(), ImageError> {
    let invalid = || ImageError("invalid JPEG XL container".into());
    let mut pos = JXL_CONTAINER_SIG.len();
    let mut version: Option<u32> = None;
    let mut codestream = [0u8; 16];
    let mut codestream_len = 0usize;
    let mut saw_jxlp = false;
    let mut saw_last_jxlp = false;
    let mut next_jxlp = 0u32;
    let mut pending_jxlp: [Option<(&[u8], bool)>; 16] = [None; 16];

    while let Some((kind, payload, next)) = read_bmff_box(bytes, pos, invalid)? {
        pos = next;
        if version.is_none() {
            if &kind != b"ftyp" || payload.len() < 8 || &payload[0..4] != b"jxl " {
                return Err(invalid());
            }
            let ver = u32::from_be_bytes(payload[4..8].try_into().unwrap());
            if ver > 1 { return Err(invalid()); }
            version = Some(ver);
            continue;
        }
        match &kind {
            b"ftyp" => return Err(invalid()),
            b"jxlc" => {
                if saw_jxlp { return Err(invalid()); }
                return check_jxl_codestream(payload);
            }
            b"jxlp" => {
                if saw_last_jxlp || payload.len() < 4 { return Err(invalid()); }
                saw_jxlp = true;
                let raw = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                let is_last = (raw & 0x8000_0000) != 0;
                let idx = raw & 0x7fff_ffff;
                let chunk = &payload[4..];
                if idx < next_jxlp {
                    return Err(invalid());
                }
                if idx == next_jxlp {
                    append_jxl_head(&mut codestream, &mut codestream_len, chunk);
                    if is_last {
                        saw_last_jxlp = true;
                    } else {
                        next_jxlp = next_jxlp.checked_add(1).ok_or_else(invalid)?;
                        while let Some(slot) = pending_jxlp.get_mut(next_jxlp as usize) {
                            let Some((pchunk, plast)) = slot.take() else { break };
                            append_jxl_head(&mut codestream, &mut codestream_len, pchunk);
                            if plast {
                                saw_last_jxlp = true;
                                break;
                            }
                            next_jxlp = next_jxlp.checked_add(1).ok_or_else(invalid)?;
                        }
                    }
                } else {
                    if version != Some(1) { return Err(invalid()); }
                    let slot = pending_jxlp.get_mut(idx as usize).ok_or_else(invalid)?;
                    if slot.is_some() { return Err(invalid()); }
                    *slot = Some((chunk, is_last));
                }
                // Signature (2 B) + maximum SizeHeader (68 bits = 9 B) fits in 11 bytes.
                if codestream_len >= 11 {
                    return check_jxl_codestream(&codestream[..codestream_len]);
                }
            }
            _ => {}
        }
    }
    if codestream_len > 0 {
        check_jxl_codestream(&codestream[..codestream_len])
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
#[path = "header_tests.rs"]
mod tests;
