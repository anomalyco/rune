//! How Rune's glyphs approach CoreText's: the [`FontSmoothing`] modes and what each changes.

use super::ApplePipeline;

/// How glyphs are rasterised and placed: CoreGraphics' glyph pipeline, with or without font
/// smoothing's dilation. Set app-wide with `App::font_smoothing`, or at run time with
/// `Tx::set_font_smoothing`, which redraws with glyphs rasterised for the new mode.
///
/// The default is [`FontSmoothing::On`] on macOS and [`FontSmoothing::Off`] on every other
/// platform (Linux, Windows, the web), which has no font smoothing to match. Both work on any
/// OS (they are arithmetic on outlines, not calls into CoreText). Either way the pipeline
/// floors pens to CoreGraphics' subpixel steps (`quantisation_steps`), flattens curves and
/// hints small sizes as CoreGraphics does, and draws coverage as is. Windows blend in
/// sRGB-encoded space, as CoreGraphics does, unless the app sets `App::srgb_blend`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontSmoothing {
    /// Text that matches CoreText's with font smoothing on (macOS's default): the pipeline plus
    /// font smoothing's outline dilation, which grows outlines by a radius set by the text
    /// colour's luminance and the device ppem ([`ApplePipeline::SMOOTHED`]).
    On,
    /// Text that matches CoreText with font smoothing off, which is what macOS browsers draw
    /// for `-webkit-font-smoothing: antialiased`: the pipeline without the dilation
    /// ([`ApplePipeline::UNSMOOTHED`]). Lighter than `On`. The default off macOS.
    Off,
}

impl Default for FontSmoothing {
    fn default() -> Self {
        if cfg!(target_os = "macos") { FontSmoothing::On } else { FontSmoothing::Off }
    }
}

impl FontSmoothing {
    /// The CoreGraphics stages glyphs rasterise with: all of them for `On`, all but the
    /// dilation for `Off`.
    pub fn pipeline(self) -> ApplePipeline {
        match self {
            FontSmoothing::On => ApplePipeline::SMOOTHED,
            FontSmoothing::Off => ApplePipeline::UNSMOOTHED,
        }
    }
}

/// Glyph x phases are in sixtieths of a device pixel ([`PHASES`]), which hold every step
/// CoreGraphics quantises to (fifths to wholes).
pub(crate) const PHASES: f32 = 60.0;

/// How many x positions per device pixel CoreGraphics' subpixel quantisation draws a glyph of
/// `ppem` device px at: one more than the whole number of times `ppem` goes into 33⅓, at most
/// five. Fifths below 8⅓ ppem, quarters to 11.1, thirds to 16.7, halves to 33.3 and whole
/// pixels above, at every scale (measured on macOS 15.3: 1× and 2×, ppem 3–80, in ¼ steps
/// around each boundary). A ¼-point floor matches it only at 2× from 17 to 33 ppem.
pub(crate) fn quantisation_steps(ppem: f32) -> f32 {
    if ppem > 0.0 { (1.0 + (100.0 / 3.0 / ppem).floor()).min(5.0) } else { 5.0 }
}

/// The device pixel a glyph's quad starts at and its x phase in sixtieths of a pixel (its atlas
/// bin, [`PHASES`]), for a pen at `x` device px and a glyph of `ppem` device px. The pen floors
/// to [`quantisation_steps`] steps a pixel, as CoreGraphics' default subpixel quantisation does.
pub(crate) fn glyph_x(x: f32, ppem: f32) -> (f32, u8) {
    let steps = quantisation_steps(ppem);
    // A 1/1024-step allowance keeps pens that sit on the grid (monospaced text, runs split by
    // colour spans) from flooring a step down on f32 summation error. Parley's pens are whole
    // 1/256 px, so a pen just short of a step (353 - 1/256 at whole-pixel steps) must still floor.
    let x = (x * steps + 1.0 / 1024.0).floor() / steps;
    let fx = x.floor();
    let phase = ((x - fx) * steps).round();
    if phase >= steps { (fx + 1.0, 0) } else { (fx, (phase * (PHASES / steps)) as u8) }
}

#[cfg(test)]
#[path = "coretext_match_tests.rs"]
mod tests;
