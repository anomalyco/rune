//! Glyph rasterization, font smoothing and platform pipelines.

pub(crate) mod colr;
#[doc(hidden)]
pub mod coretext_match;
pub(crate) mod macos;
pub(crate) mod support;
pub(crate) mod synthetic_bold;
pub(crate) mod windfoil;
pub(crate) mod windows;
