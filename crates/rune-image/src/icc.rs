//! Embedded ICC colour-profile conversion to sRGB for decoded RGBA8 images.
//!
//! Matches browser and Skia image colour management: RGB ICC profiles are converted to sRGB
//! with relative colorimetric intent (clipping out-of-gamut colours and preserving alpha).
//! Untagged images, sRGB-equivalent profiles (detected via a 5×5×5 RGB identity probe),
//! non-RGB profiles (grey, CMYK, Lab, XYZ), oversized profiles (> 16 MiB), and corrupt or
//! unreadable profiles are safe no-ops that leave pixels unchanged.

use std::sync::{Arc, LazyLock};

use moxcms::{
    ColorProfile, DataColorSpace, Layout, ParsingOptions, RenderingIntent, Transform8BitExecutor,
    TransformOptions,
};

/// Upper bound on an embedded ICC profile accepted for parsing (16 MiB).
pub(crate) const MAX_PROFILE_BYTES: usize = 16 * 1024 * 1024;

/// Scratch chunk size for in-place RGBA8 conversion (1024 pixels = 4 KiB on the stack),
/// avoiding a full-image copy even on 8192×8192 (256 MiB) images.
const CHUNK_BYTES: usize = 1024 * 4;

/// 5×5×5 RGB cube (125 colours, opaque RGBA8) probed once per profile to detect sRGB identity
/// profiles without transforming the image.
const PROBE_LEVELS: [u8; 5] = [0, 32, 128, 192, 255];
const PROBE_RGBA: [u8; 125 * 4] = {
    let mut buf = [0u8; 125 * 4];
    let mut i = 0;
    let mut r = 0;
    while r < 5 {
        let mut g = 0;
        while g < 5 {
            let mut b = 0;
            while b < 5 {
                buf[i] = PROBE_LEVELS[r];
                buf[i + 1] = PROBE_LEVELS[g];
                buf[i + 2] = PROBE_LEVELS[b];
                buf[i + 3] = 255;
                i += 4;
                b += 1;
            }
            g += 1;
        }
        r += 1;
    }
    buf
};

static SRGB_PROFILE: LazyLock<ColorProfile> = LazyLock::new(ColorProfile::new_srgb);

/// A validated RGB-to-sRGB 8-bit RGBA transform that differs from identity.
pub(crate) struct Transform(Arc<Transform8BitExecutor>);

impl Transform {
    /// Parses `icc` and builds a relative-colorimetric RGB-to-sRGB transform.
    ///
    /// Returns `None` when `icc` is empty, exceeds [`MAX_PROFILE_BYTES`], cannot be parsed, is
    /// not an RGB profile, or maps the 125-colour probe identically to sRGB.
    pub(crate) fn to_srgb(icc: &[u8]) -> Option<Self> {
        if icc.is_empty() || icc.len() > MAX_PROFILE_BYTES {
            return None;
        }
        let parse_opts = ParsingOptions {
            max_profile_size: MAX_PROFILE_BYTES + 1,
            ..ParsingOptions::default()
        };
        let profile = ColorProfile::new_from_slice_with_options(icc, parse_opts).ok()?;
        if profile.color_space != DataColorSpace::Rgb {
            return None;
        }
        let options = TransformOptions {
            rendering_intent: RenderingIntent::RelativeColorimetric,
            ..TransformOptions::default()
        };
        let executor = profile
            .create_transform_8bit(Layout::Rgba, &SRGB_PROFILE, Layout::Rgba, options)
            .ok()?;
        let mut out = [0u8; PROBE_RGBA.len()];
        executor.transform(&PROBE_RGBA, &mut out).ok()?;
        if out == PROBE_RGBA {
            return None;
        }
        Some(Self(executor))
    }

    /// Converts straight-alpha RGBA8 pixels in place to sRGB, preserving alpha.
    pub(crate) fn apply_rgba(&self, rgba: &mut [u8]) {
        let valid_len = rgba.len() - (rgba.len() % 4);
        let mut scratch = [0u8; CHUNK_BYTES];
        for chunk in rgba[..valid_len].chunks_mut(CHUNK_BYTES) {
            let dst = &mut scratch[..chunk.len()];
            if self.0.transform(chunk, dst).is_ok() {
                chunk.copy_from_slice(dst);
            }
        }
    }
}

/// Converts `rgba` in place to sRGB when `icc` carries a non-identity RGB ICC profile.
pub(crate) fn convert_rgba_to_srgb(icc: Option<&[u8]>, rgba: &mut [u8]) {
    if let Some(transform) = icc.and_then(Transform::to_srgb) {
        transform.apply_rgba(rgba);
    }
}
