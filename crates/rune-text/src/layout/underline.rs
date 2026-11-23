//! Underline edges on whole device pixels.

use parley::PositionedLayoutItem;

use crate::Ink;

#[cfg(test)]
thread_local! {
    static LINE_END_ITEM_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn take_line_end_item_visits() -> usize {
    LINE_END_ITEM_VISITS.with(|c| c.replace(0))
}

/// Rightmost glyph-run edge on `line` in layout device px.
#[doc(hidden)]
pub fn line_end(line: &parley::Line<'_, Ink>) -> f32 {
    line.items().fold(0.0f32, |e, item| match item {
        PositionedLayoutItem::GlyphRun(gr) => {
            #[cfg(test)]
            LINE_END_ITEM_VISITS.with(|c| c.set(c.get() + 1));
            e.max(gr.offset() + gr.advance())
        }
        PositionedLayoutItem::InlineBox(_) => e,
    })
}

/// [`underline_edges`] given a precomputed [`line_end`] for `line`.
#[doc(hidden)]
pub fn underline_edges_with_end(end: f32, origin_x: f32, a: f32, b: f32) -> (f32, f32) {
    let r = if b >= end - 0.01 { b } else { b.round() };
    ((origin_x + a.round()).round(), (origin_x + r).round())
}

/// An underline's left and right edges (device px) under layout x `a..b` of `line`, its text
/// starting at `origin_x`. Both are on whole pixels, so runs that touch share an edge, and one
/// that ends its line runs on to where it ends, rounded as the next text box's start rounds (a
/// text box is as wide as its text to a 64th of a pixel and starts at its exact place): a link
/// split into a text element a word meets the next word's line with no gap or overlap at the space.
#[doc(hidden)]
pub fn underline_edges(line: &parley::Line<'_, Ink>, origin_x: f32, a: f32, b: f32) -> (f32, f32) {
    underline_edges_with_end(line_end(line), origin_x, a, b)
}
