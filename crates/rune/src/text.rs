//! Text shaping, layout, glyph rasterization and icon rendering, re-exported from [`rune_text`],
//! plus the GPU-backed CoreText pixel comparison harness (`pixel_compare`).

pub use rune_text::*;

#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod pixel_compare;

#[cfg(test)]
mod runtime_tests;
