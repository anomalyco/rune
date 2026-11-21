//! Heights and line fill read from a finished layout.

use parley::Layout;

use crate::Ink;
use rune_core::tok::TextStyle;

/// `px` device pixels rounded up to a 64th of one, the unit Chromium's layout keeps a text
/// run's width in (`LayoutUnit`): a label is as wide as its glyphs, so what follows it sits
/// where Chromium puts it, never up to a pixel further on.
#[doc(hidden)]
pub fn text_width(px: f32) -> f32 {
    ((px - 1e-3) * 64.0).ceil().max(0.0) / 64.0
}

/// Layout height in device px: whole lines at the style's line height (an empty text still
/// takes one line, like a browser).
pub fn layout_height(l: &Layout<Ink>, style: TextStyle, scale: f32) -> f32 {
    l.height().max(style.line_px() * scale).ceil()
}

/// [`layout_height`], or with `exact` the lines' own height kept fractional, to 1/64 px as a
/// browser's layout units keep a 22.4 px line (`El::exact_lines`, every text's default).
#[doc(hidden)]
pub fn spec_height(l: &Layout<Ink>, style: TextStyle, scale: f32, exact: bool) -> f32 {
    if !exact {
        return layout_height(l, style, scale);
    }
    (l.height().max(style.line_px() * scale) * 64.0).round() / 64.0
}

/// Fraction of the final line occupied by ink advance, excluding hanging whitespace.
/// Width and the layout use device pixels. No shaping or allocation occurs here.
pub fn last_line_fill(l: &Layout<Ink>, width: f32) -> f32 {
    let Some(line) = l.lines().next_back() else { return 0.0 };
    let m = line.metrics();
    ((m.advance - m.hanging_advance) / width.max(1.0)).clamp(0.0, 1.0)
}

/// Height held back while the last line fills (research/18 §7), in points.
pub fn stream_height_lag(l: &Layout<Ink>, width: f32, scale: f32) -> f32 {
    let Some(line) = l.lines().next_back() else { return 0.0 };
    let cap = if l.len() == 1 { 0.52 } else { 0.82 };
    let lag = line.metrics().line_height / scale * cap * (1.0 - last_line_fill(l, width));
    (lag * 64.0).round() / 64.0
}
