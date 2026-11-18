//! Glyph rasterization, font smoothing, platform pipelines and pixel comparison.

pub(in crate::text) mod colr;
pub(crate) mod coretext_match;
pub(in crate::text) mod macos;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod pixel_compare;
pub(in crate::text) mod support;
pub(in crate::text) mod synthetic_bold;
pub(in crate::text) mod windfoil;
pub(in crate::text) mod windows;
