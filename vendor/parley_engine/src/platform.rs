// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A face of last resort that the platform draws (Rune: vendor/parley_engine.patch). A host that
//! can shape and draw text itself (a browser's canvas, with the system's fonts, shaping and
//! colour emoji) registers a font that covers every character and gives its runs a shaper here.
//! A run in that font isn't shaped by HarfRust: the host gives one glyph per grapheme, its
//! advance measured as the platform lays the run out, and the glyph ids are the host's own, for
//! it to draw them.

use alloc::vec::Vec;

/// A run in the platform's face, for its shaper.
#[derive(Debug)]
pub struct PlatformText<'a> {
    /// The run's text.
    pub text: &'a str,
    /// Byte offsets into `text` where each grapheme starts, ascending, the first 0.
    pub graphemes: &'a [u32],
    /// Byte offsets where each unit the platform draws whole starts (at grapheme starts, between
    /// break opportunities), ascending, the first 0. A line never breaks inside one.
    pub units: &'a [u32],
    /// The font size, in the layout's units.
    pub size: f32,
    /// The weight asked for (`wght`, 100 to 900).
    pub weight: f32,
    /// Italic asked for.
    pub italic: bool,
    /// The run is right to left.
    pub rtl: bool,
    /// The run's language tag, if it has one.
    pub language: Option<&'a str>,
}

/// One glyph per grapheme of a [`PlatformText`], in logical order.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlatformGlyph {
    /// The host's id for what this glyph draws.
    pub id: u32,
    /// The grapheme's advance, in the layout's units.
    pub advance: f32,
    /// How far right of its pen the glyph draws, in the layout's units.
    pub x_offset: f32,
}

/// Fills `out` with one glyph per grapheme of the run, in logical order.
pub type PlatformShaper = fn(run: &PlatformText<'_>, out: &mut Vec<PlatformGlyph>);

#[cfg(feature = "std")]
static PLATFORM: std::sync::OnceLock<(u64, PlatformShaper)> = std::sync::OnceLock::new();

/// Makes the font whose data has id `font` (`Blob::id`) the platform's face, shaped by `shaper`,
/// once (a second call changes nothing). Needs `std`.
pub fn set_platform_face(font: u64, shaper: PlatformShaper) {
    #[cfg(feature = "std")]
    let _ = PLATFORM.set((font, shaper));
    #[cfg(not(feature = "std"))]
    let _ = (font, shaper);
}

/// The shaper for the font with data id `font`, if it's the platform's face.
pub(crate) fn shaper(font: u64) -> Option<PlatformShaper> {
    #[cfg(feature = "std")]
    return PLATFORM.get().filter(|(id, _)| *id == font).map(|(_, shaper)| *shaper);
    #[cfg(not(feature = "std"))]
    {
        let _ = font;
        None
    }
}
