//! AVIF stills (`avif` feature), decoded natively by `rav1d` (a pure-Rust port of `dav1d`) as
//! straight-alpha sRGB RGBA8. Animated AVIF (`avis`) is rejected natively; on the web the browser
//! decodes its first frame.
use crate::{ImageError, ImageLimits};

#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
mod codec;
#[cfg_attr(not(any(test, target_arch = "wasm32", feature = "avif")), allow(dead_code))]
mod limits;

/// Whether `bytes` are an ISO-BMFF file of an AVIF brand.
pub(super) fn is_avif(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis")
}

/// Decodes an AVIF still to straight-alpha sRGB RGBA8 under `limits`, enforcing the budget in
/// preflight, inside `rav1d` (`Dav1dSettings::frame_size_limit`), and again before allocating the
/// RGBA8 conversion buffer.
#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
pub(super) fn rgba_with_limits(bytes: &[u8], limits: ImageLimits) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let icc = limits::inspect_with(bytes, limits)?;
    let (w, h, mut pixels) = codec::decode_avif(bytes, limits)?;
    super::icc::convert_rgba_to_srgb(icc, &mut pixels);
    Ok((w, h, pixels))
}

#[cfg(not(all(feature = "avif", not(target_arch = "wasm32"))))]
pub(super) fn rgba_with_limits(_: &[u8], _: ImageLimits) -> Result<(u32, u32, Vec<u8>), ImageError> {
    Err(ImageError("AVIF images need rune's `avif` feature".into()))
}

#[cfg(test)]
pub(super) fn rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), ImageError> {
    rgba_with_limits(bytes, ImageLimits::new())
}

/// Validates decoded dimensions and checked RGBA8 byte size against [`ImageLimits`] before
/// allocating a conversion buffer.
#[cfg_attr(not(any(test, all(feature = "avif", not(target_arch = "wasm32")))), allow(dead_code))]
pub(super) fn checked_rgba_bytes_with(w: usize, h: usize, limits: ImageLimits) -> Result<(u32, u32, usize), ImageError> {
    checked_rgba_bytes_raw(w, h, limits.max_side(), limits.max_pixels(), limits.max_alloc())
}

#[cfg_attr(not(any(test, all(feature = "avif", not(target_arch = "wasm32")))), allow(dead_code))]
pub(super) fn checked_rgba_bytes_raw(
    w: usize,
    h: usize,
    max_dim: u32,
    max_pixels: u64,
    max_alloc: u64,
) -> Result<(u32, u32, usize), ImageError> {
    let (Ok(w32), Ok(h32)) = (u32::try_from(w), u32::try_from(h)) else { return Err(limits::too_large()); };
    if w32 == 0 || h32 == 0 || w32 > max_dim || h32 > max_dim { return Err(limits::too_large()); }
    let pixels = u64::from(w32).checked_mul(u64::from(h32)).filter(|&n| n <= max_pixels).ok_or_else(limits::too_large)?;
    let bytes = pixels
        .checked_mul(4)
        .filter(|&n| n <= max_alloc)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(limits::too_large)?;
    Ok((w32, h32, bytes))
}

#[cfg(test)]
pub(super) fn checked_rgba_bytes(w: usize, h: usize) -> Result<(u32, u32, usize), ImageError> {
    checked_rgba_bytes_with(w, h, ImageLimits::new())
}

/// Validates an AVIF container and its AV1 headers against `limits`.
#[cfg_attr(not(any(test, target_arch = "wasm32", feature = "avif")), allow(dead_code))]
pub(super) fn check_limits_with(bytes: &[u8], limits: ImageLimits) -> Result<(), ImageError> {
    limits::check_with(bytes, limits)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
