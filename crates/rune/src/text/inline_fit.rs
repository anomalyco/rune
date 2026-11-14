//! The spaces that pad inline code keep inside the line box their paragraph sets.
//!
//! Inline code itself grows the line as Blink's does: Parley lays each run's box out with
//! quantizing (`BoxMetrics::from_font`: ascent and descent rounded to device px, the half-leading
//! over the text floored), and the line box is the largest reach above the baseline plus the
//! largest below. That is Chromium's rule to the device pixel: at 13/16 IBM Plex Mono reaches 4
//! under the baseline where Inter reaches 3, so a table cell with code is 17 tall at 1x and 2x
//! (33 device px) and 16 at 1.25x, 1.5x and 1.875x, and a 14/22.4 paragraph with code is 22.933
//! at 1.875x (`operation-detox` `proof/layout-cards/icline/`). The smaller spaces that stand for
//! the code's CSS padding have no box in a browser, so theirs is cut until it fits in the strut.
//! Glyphs stay on the shared baseline; only the box moves.
use std::ops::Range;

use parley::Layout;

use super::{Ink, Span};
use crate::tok::TextStyle;

/// Line heights (points) for the runs of the smaller spaces padding `layout`'s monospace spans
/// whose box would stand out of the strut of proportional `style` text, by byte range.
pub(super) fn mono_fits(layout: &Layout<Ink>, text: &str, spans: &[Span], style: TextStyle, scale: f32, strut: (f32, f32)) -> Vec<(Range<usize>, f32)> {
    if !spans.iter().any(|s| s.mono) || style.size <= 0.0 || scale <= 0.0 {
        return Vec::new();
    }
    // The smaller spaces that stand for the code's padding: they make room and show nothing.
    let room = |s: &Span| !s.mono && s.size.is_some_and(|z| z < style.size) && text.get(s.start as usize..s.end as usize).is_some_and(|t| !t.is_empty() && t.chars().all(char::is_whitespace));
    let span_at = |at: usize| spans.iter().rev().find(|s| room(s) && (s.start as usize) <= at && at < s.end as usize);
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