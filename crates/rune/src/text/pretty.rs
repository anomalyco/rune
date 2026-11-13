//! `text-wrap-style: pretty` as Chromium wraps it (Blink's `ScoreLineBreaker`,
//! `third_party/blink/renderer/core/layout/inline/score_line_breaker.cc`): lines break greedily,
//! then a paragraph whose last line is one word shorter than a third of the line is broken again
//! over its last four lines (fewer when it has fewer), scoring each line by the square of the
//! room it leaves and penalising a break before the last word. The line count stays the greedy
//! one, so a text's height never moves. Pages that set it, as OpenCode's Desktop does on every
//! element, move a lone last word down with the one before it.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use icu_segmenter::LineSegmenter;
use icu_segmenter::options::LineBreakOptions;
use parley::{BreakReason, Layout, Line};

use super::Ink;

static PRETTY: AtomicBool = AtomicBool::new(false);

/// Wraps the app's text as `text-wrap-style: pretty` does in Chromium: a paragraph that would end
/// on a lone short word takes a word or more down with it. Text in a monospace family, justified,
/// with `wrap_anywhere`, in an editor, cut to a number of lines or opted out with
/// [`El::wrap_greedy`](crate::El::wrap_greedy) keeps greedy lines. Call it before any text is laid
/// out: layouts are cached.
pub fn set_pretty_wrap(on: bool) {
    PRETTY.store(on, Ordering::Relaxed);
}

pub(crate) fn pretty_wrap() -> bool {
    PRETTY.load(Ordering::Relaxed)
}

/// Blink's `kMaxLinesForOptimal`: only a paragraph's last lines are broken again.
const MAX_LINES: usize = 4;
/// `kOrphansPenalty`, on a break before the last word (CSS px squared, as the line scores).
const ORPHANS_PENALTY: f32 = 10_000.0;
/// `kScoreOverfull`.
const OVERFULL: f32 = 1e12;
/// `kLastLinePenaltyMultiplier`.
const LAST_LINE_PENALTY: f32 = 4.0;
/// `LayoutUnit::AddEpsilon`: a line fits up to 1/64 px past its width.
const EPSILON: f32 = 1.0 / 64.0;

/// A break opportunity: where the next line would start and where this one would end (before
/// the spaces it hangs), in CSS px from the start of the stretch, and the byte it breaks before.
#[derive(Clone, Copy)]
struct Candidate {
    at: usize,
    no_break: f32,
    if_break: f32,
    penalty: f32,
}

/// The room each line of a paragraph's last stretch gets to break as Chromium's pretty wrapping
/// does, for [`super::line_break::break_lines_capped`]: `((paragraph, line in it), room)` in
/// device px. Empty when every greedy line stands. `layout` is broken greedily at `max_width`
/// (device px) and unindented; `font_size` is its style's, in points.
pub(super) fn caps(layout: &Layout<Ink>, text: &str, font_size: f32, scale: f32, max_width: f32) -> Vec<((usize, usize), f32)> {
    let lines: Vec<Line<'_, Ink>> = layout.lines().collect();
    let mut out = Vec::new();
    let (mut start, mut para) = (0, 0);
    while start < lines.len() {
        let mut end = start;
        while end + 1 < lines.len() && lines[end].break_reason() != BreakReason::Explicit {
            end += 1;
        }
        if let Some(rooms) = paragraph(&lines[start..=end], text, font_size, scale, max_width) {
            out.extend(rooms.into_iter().map(|(line, room)| ((para, line), room)));
        }
        para += 1;
        start = end + 1;
    }
    out
}

/// `range` of `text` without the spaces and line break that end it.
fn trimmed(text: &str, range: Range<usize>) -> Range<usize> {
    let s = &text[range.clone()];
    range.start..range.start + s.trim_end().len()
}

/// The line breaks one paragraph's lines take instead, as (line in the paragraph, room).
fn paragraph(lines: &[Line<'_, Ink>], text: &str, font_size: f32, scale: f32, max_width: f32) -> Option<Vec<(usize, f32)>> {
    let n = lines.len();
    if n < 2 || lines.iter().any(|l| l.runs().any(|r| r.is_rtl())) {
        return None;
    }
    // `ShouldOptimize`: a last line shorter than a third of the line, with no break inside it.
    let last = &lines[n - 1];
    let m = last.metrics();
    if m.advance - m.hanging_advance >= max_width / 3.0 {
        return None;
    }
    let first = n.saturating_sub(MAX_LINES);
    let window = &lines[first..];
    let span = window[0].text_range().start..trimmed(text, last.text_range()).end;
    let last_words = trimmed(text, last.text_range());
    let segmenter = LineSegmenter::new_for_non_complex_scripts(LineBreakOptions::default());
    let breaks: Vec<usize> = segmenter.segment_str(&text[span.clone()]).map(|b| span.start + b).filter(|&b| b > span.start && b < span.end).collect();
    if breaks.iter().any(|&b| b > last_words.start && b < last_words.end) {
        return None;
    }
    // Each cluster's byte, x from the stretch's start and advance, and whether it's a space.
    let mut clusters: Vec<(usize, f32, f32, bool)> = Vec::new();
    let mut x = 0.0;
    for line in window {
        for run in line.runs() {
            for c in run.clusters() {
                let advance = c.advance();
                clusters.push((c.text_range().start, x, advance, c.is_space_or_nbsp() || c.is_hard_line_break()));
                x += advance;
            }
        }
    }
    // Where a line ending before cluster `i` ends: after the last cluster that isn't a space.
    let end_before = |i: usize| clusters[..i].iter().rev().find(|c| !c.3).map_or(0.0, |c| c.1 + c.2);
    let css = |px: f32| px / scale;
    let mut cands = vec![Candidate { at: span.start, no_break: 0.0, if_break: 0.0, penalty: 0.0 }];
    for b in breaks {
        let Ok(i) = clusters.binary_search_by_key(&b, |c| c.0) else { continue };
        cands.push(Candidate { at: b, no_break: css(clusters[i].1), if_break: css(end_before(i)), penalty: 0.0 });
    }
    let end = css(end_before(clusters.len()));
    cands.push(Candidate { at: span.end, no_break: end, if_break: end, penalty: 0.0 });
    // `Optimize`: at least three opportunities, and a penalty on the one before the last word.
    if cands.len() < 5 {
        return None;
    }
    let orphan = cands.len() - 2;
    cands[orphan].penalty += ORPHANS_PENALTY;
    let width = css(max_width);
    let fit = width + EPSILON;
    // `SetupParameters` for unjustified text: twice the width times the font size per hyphen, and
    // twice that per line.
    let line_penalty = width * font_size * 4.0;
    let breaks = optimal(&cands, fit, line_penalty);
    if breaks.len() != window.len() {
        return None;
    }
    let greedy = window[1..].iter().map(|l| l.text_range().start);
    if breaks[..breaks.len() - 1].iter().map(|&i| cands[i].at).eq(greedy) {
        return None;
    }
    // Each line but the last gets the room its words take, and a little more, so the greedy
    // breaker ends it where the scores did.
    let mut rooms = Vec::with_capacity(breaks.len() - 1);
    let mut from = 0;
    for (k, &b) in breaks[..breaks.len() - 1].iter().enumerate() {
        rooms.push((first + k, (cands[b].if_break - cands[from].no_break) * scale + 0.5));
        from = b;
    }
    Some(rooms)
}

/// `ComputeScores` and `ComputeBreakPoints`: the candidates the lines end at, in order.
fn optimal(cands: &[Candidate], fit: f32, line_penalty: f32) -> Vec<usize> {
    // (score, the candidate the line starts at, lines so far)
    let mut scores: Vec<(f32, usize, usize)> = vec![(0.0, 0, 0)];
    let mut active = 0;
    for end in 1..cands.len() {
        let last = end == cands.len() - 1;
        let (mut best, mut best_prev, mut best_hope) = (f32::MAX, 0, 0.0f32);
        let start_edge = cands[end].if_break - fit;
        for start in active..end {
            let start_score = scores[start].0;
            if start_score + best_hope >= best {
                continue;
            }
            let delta = cands[start].no_break - start_edge;
            let (mut width_score, mut extra) = (0.0, 0.0);
            if delta < 0.0 {
                width_score = OVERFULL;
            } else if last {
                extra = LAST_LINE_PENALTY * cands[start].penalty;
            } else {
                width_score = delta * delta;
            }
            if delta < 0.0 {
                active = start + 1;
            } else {
                best_hope = width_score;
            }
            let score = start_score + width_score + extra;
            if score <= best {
                best = score;
                best_prev = start;
            }
        }
        scores.push((best + cands[end].penalty + line_penalty, best_prev, scores[best_prev].2 + 1));
    }
    let mut out = Vec::new();
    let mut i = cands.len() - 1;
    while i > 0 {
        out.push(i);
        i = scores[i].1;
    }
    out.reverse();
    out
}
