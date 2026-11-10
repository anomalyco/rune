//! Inline code that keeps inside the line box its paragraph sets. A monospace span at the
//! text's line height has a box as tall as the strut's, but placed by its own ascent and
//! descent: at 13 px IBM Plex Mono's round to 13 and 4 where Inter's round to 13 and 3, so the
//! code's box reached a pixel under the strut's and a 13/16 line with code in it stood 17 tall,
//! a markdown table row 38 where Chromium's stands 36. Chromium's line keeps the strut's height
//! (a cell of 13/16 text with inline code is 16 tall), so each such span's line height is cut
//! until its box fits in the strut, as Parley lays boxes out with quantizing. Glyphs stay on the
//! shared baseline; only the box moves.
use std::ops::Range;

use parley::Layout;

use super::{Ink, Span};
use crate::tok::TextStyle;

/// Line heights (points) for the runs of `layout`'s monospace spans, and of the smaller spaces
/// padding them, whose box would stand out of the strut of proportional `style` text, by byte
/// range. Code larger than the text keeps its own box, which may grow the line as a browser's does.
pub(super) fn mono_fits(layout: &Layout<Ink>, text: &str, spans: &[Span], style: TextStyle, scale: f32, strut: (f32, f32)) -> Vec<(Range<usize>, f32)> {
    if !spans.iter().any(|s| s.mono) || style.size <= 0.0 || scale <= 0.0 {
        return Vec::new();
    }
    // Code, and the smaller spaces that stand for its padding: they make room and show nothing.
    let room = |s: &Span| s.size.is_some_and(|z| z < style.size) && text.get(s.start as usize..s.end as usize).is_some_and(|t| !t.is_empty() && t.chars().all(char::is_whitespace));
    let span_at = |at: usize| spans.iter().rev().find(|s| (s.mono || room(s)) && (s.start as usize) <= at && at < s.end as usize);
    let runs = || layout.lines().flat_map(|line| line.runs().collect::<Vec<_>>());
    // The strut: the text's own font at its size (`strut` is its ascent and descent per em).
    let (ascent, descent) = (strut.0 * style.size * scale, strut.1 * style.size * scale);
    let line = style.line_px() * scale;
    let (over, under) = quantized(ascent, descent, line);
    let mut out: Vec<(Range<usize>, f32)> = Vec::new();
    for run in runs() {
        let range = run.text_range();
        let Some(span) = (!range.is_empty()).then(|| span_at(range.start)).flatten() else { continue };
        let size = run.font_size();
        if size > style.size * scale + 1e-3 {
            continue;
        }
        // The span's line height as `layout_rich` gave it: the text's, scaled with a span size.
        let own = if span.size.is_some() { line / (style.size * scale) * size } else { line };
        let m = run.font_metrics();
        let Some(fit) = fit(m.ascent.round(), m.descent.round(), own, over, under) else { continue };
        let lh = fit / scale;
        match out.last_mut() {
            Some((last, v)) if last.end == range.start && *v == lh => last.end = range.end,
            _ => out.push((range, lh)),
        }
    }
    out
}

/// A span box's extent over and under the baseline as Parley lays it out with quantizing
/// (`BoxMetrics::from_font`): ascent and descent rounded, the half-leading above floored.
fn quantized(ascent: f32, descent: f32, line: f32) -> (f32, f32) {
    let (a, d) = (ascent.round(), descent.round());
    let over = a + ((line - a - d) / 2.0).floor();
    (over, line - over)
}

/// The tallest line height up to `own` at which a box of rounded `a` and `d` stays within the
/// strut's `over` and `under`, or None when `own` already does. With the half-leading above
/// floored to `k`, the box reaches `a + k` over the baseline and `h - a - k` under it, and `k`
/// holds for heights from `a + d + 2k` up to `a + d + 2k + 2`.
fn fit(a: f32, d: f32, own: f32, over: f32, under: f32) -> Option<f32> {
    let (o, u) = quantized(a, d, own);
    if o <= over && u <= under {
        return None;
    }
    let mut k = ((own - a - d) / 2.0).floor();
    for _ in 0..64 {
        let low = a + d + 2.0 * k;
        let h = own.min(under + a + k).min(low + 2.0 - 1.0 / 64.0);
        if a + k <= over && h >= low && h > 0.0 {
            return Some(h);
        }
        k -= 1.0;
    }
    Some(0.0)
}