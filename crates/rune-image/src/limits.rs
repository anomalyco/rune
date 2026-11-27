//! Per-load image decode budgets for [`crate::ImageCache::load_with_limits`] and
//! [`crate::ImageCache::load_scaled_with_limits`].
use crate::{MAX_ALLOC, MAX_PIXELS, MAX_SIDE};

/// Per-load decode limits for [`crate::ImageCache::load_with_limits`] and
/// [`crate::ImageCache::load_scaled_with_limits`].
///
/// Defaults to Rune's global raster ceiling (8192 px per side, 8192 × 8192 total pixels, and
/// 256 MiB decoded RGBA8 buffer). Callers that only render smaller preview surfaces can tighten
/// any limit with [`Self::with_max_side`], [`Self::with_max_pixels`], or [`Self::with_max_alloc`];
/// values above Rune's global ceiling are clamped so a per-load budget never raises memory limits
/// past the global cap.
///
/// For native AVIF decoding (`avif` feature), the effective pixel budget is enforced inside
/// `rav1d` (`Dav1dSettings::frame_size_limit`), in pre-decode ISO-BMFF/AV1 header preflight, and
/// again before allocating the RGBA8 conversion buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageLimits {
    max_side: u32,
    max_pixels: u64,
    max_alloc: u64,
}

impl ImageLimits {
    /// Rune's global raster decode limits (8192 px per side, 67,108,864 pixels, 256 MiB).
    pub const fn new() -> Self {
        Self {
            max_side: MAX_SIDE,
            max_pixels: MAX_PIXELS,
            max_alloc: MAX_ALLOC,
        }
    }

    /// Caps both image width and height to at most `max_side` pixels (clamped to 8192).
    pub const fn with_max_side(mut self, max_side: u32) -> Self {
        self.max_side = if max_side < MAX_SIDE { max_side } else { MAX_SIDE };
        self
    }

    /// Caps total decoded pixel count (`width * height`) to at most `max_pixels` (clamped to
    /// `8192 * 8192`).
    pub const fn with_max_pixels(mut self, max_pixels: u64) -> Self {
        self.max_pixels = if max_pixels < MAX_PIXELS { max_pixels } else { MAX_PIXELS };
        self
    }

    /// Caps decoded raster buffer bytes to at most `max_alloc` (clamped to 256 MiB).
    pub const fn with_max_alloc(mut self, max_alloc: u64) -> Self {
        self.max_alloc = if max_alloc < MAX_ALLOC { max_alloc } else { MAX_ALLOC };
        self
    }

    /// Maximum allowed width or height in pixels.
    pub const fn max_side(self) -> u32 { self.max_side }

    /// Maximum allowed total pixels (`width * height`).
    pub const fn max_pixels(self) -> u64 { self.max_pixels }

    /// Maximum allowed decoded buffer allocation in bytes.
    pub const fn max_alloc(self) -> u64 { self.max_alloc }

    /// Whether this matches Rune's global default limits.
    pub(crate) const fn is_default(self) -> bool {
        self.max_side == MAX_SIDE && self.max_pixels == MAX_PIXELS && self.max_alloc == MAX_ALLOC
    }

    /// Tightest total pixel budget implied by `max_side`, `max_pixels`, and `max_alloc / 4`.
    pub(crate) const fn effective_max_pixels(self) -> u64 {
        let side_sq = (self.max_side as u64) * (self.max_side as u64);
        let alloc_px = self.max_alloc / 4;
        let a = if self.max_pixels < side_sq { self.max_pixels } else { side_sq };
        if a < alloc_px { a } else { alloc_px }
    }

    /// Pixel count limit for `rav1d`'s `Dav1dSettings::frame_size_limit`.
    ///
    /// Returns `None` when the effective pixel budget is zero, because `0` disables the limit in
    /// `rav1d`; callers must reject the decode before opening `rav1d`.
    #[cfg_attr(not(any(test, all(feature = "avif", not(target_arch = "wasm32")))), allow(dead_code))]
    pub(crate) const fn rav1d_frame_size_limit(self) -> Option<u32> {
        let px = self.effective_max_pixels();
        if px == 0 { None } else { Some(px as u32) }
    }

    /// Returns whether `w × h` at `bytes_per_pixel` fits within `max_side`, `max_pixels`, and
    /// `max_alloc` without overflow.
    pub(crate) const fn fits(self, w: u32, h: u32, bytes_per_pixel: u64) -> bool {
        if w == 0 || h == 0 || w > self.max_side || h > self.max_side {
            return false;
        }
        let pixels = (w as u64) * (h as u64);
        if pixels > self.max_pixels {
            return false;
        }
        match pixels.checked_mul(bytes_per_pixel) {
            Some(bytes) => bytes <= self.max_alloc,
            None => false,
        }
    }
}

impl Default for ImageLimits {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
#[path = "limits_tests.rs"]
mod tests;
