//! Caret and selection geometry on a laid-out paragraph: cursor rects, hit testing, line
//! edges and moves, and selection boxes.

use crate::*;

#[derive(Clone, Copy)]
struct VisCluster {
    start: usize,
    end: usize,
    x0: f32,
    x1: f32,
    rtl: bool,
    word_boundary: bool,
    space: bool,
}

pub(crate) fn line_has_mixed_dir(line: &parley::Line<'_, Ink>, base_rtl: bool) -> bool {
    line.runs().any(|r| r.is_rtl() != base_rtl)
}

/// Visual line holding byte `at` with `affinity`, resolving line breaks by byte range rather than
/// Parley's `visual_clusters` / `is_end_of_line` (which misplaces RTL line-0 byte 0 before an
/// explicit break and wrapped pills).
fn line_for_cursor(
    layout: &Layout<Ink>,
    at: usize,
    affinity: parley::layout::Affinity,
) -> Option<(usize, parley::Line<'_, Ink>)> {
    use parley::{BreakReason, layout::Affinity};
    let n = layout.len();
    if n == 0 {
        return None;
    }
    for (li, line) in layout.lines().enumerate() {
        let lr = line.text_range();
        if lr.start > lr.end {
            continue;
        }
        if at < lr.end {
            if at == lr.start
                && li > 0
                && affinity == Affinity::Upstream
                && let Some(prev) = layout.get(li - 1)
                && prev.break_reason() != BreakReason::Explicit
            {
                return Some((li - 1, prev));
            }
            return Some((li, line));
        }
        if at == lr.end {
            if li + 1 < n {
                if line.break_reason() != BreakReason::Explicit && affinity == Affinity::Upstream {
                    return Some((li, line));
                }
                continue;
            }
            return Some((li, line));
        }
    }
    let last = n - 1;
    Some((last, layout.get(last)?))
}

fn line_clusters(line: &parley::Line<'_, Ink>) -> Vec<VisCluster> {
    let m = line.metrics();
    let mut out = Vec::new();
    for run in line.runs() {
        let mut cx = m.inline_min_coord
            + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset);
        for cl in run.visual_clusters() {
            let adv = cl.advance();
            if !cl.is_hard_line_break() {
                let r = cl.text_range();
                out.push(VisCluster {
                    start: r.start,
                    end: r.end,
                    x0: cx,
                    x1: cx + adv,
                    rtl: cl.is_rtl(),
                    word_boundary: cl.is_word_boundary(),
                    space: cl.is_space_or_nbsp(),
                });
            }
            cx += adv;
        }
    }
    out
}

/// Primary (enclosing-direction) caret x on `line` at byte `at`, matching UAX #9 and Blink's
/// `BidiAdjustment`: at a boundary between opposite-direction runs, attaches to the lower-level
/// enclosing run; at a line start or end whose adjacent run runs opposite to `base_rtl`, places
/// the caret at the line's leading or trailing edge in `base_rtl`.
fn primary_x_on_line(
    clusters: &[VisCluster],
    base_rtl: bool,
    line: &parley::Line<'_, Ink>,
    at: usize,
) -> f32 {
    let m = line.metrics();
    if clusters.is_empty() {
        return m.inline_min_coord + m.offset;
    }
    let at = clusters.iter().find(|c| c.start < at && at < c.end).map_or(at, |c| c.start);
    let prev = clusters.iter().enumerate().filter(|(_, c)| c.end <= at).max_by_key(|(_, c)| c.end);
    let next = clusters.iter().enumerate().filter(|(_, c)| c.start >= at).min_by_key(|(_, c)| c.start);
    let trailing = |c: &VisCluster| if c.rtl { c.x0 } else { c.x1 };
    let leading = |c: &VisCluster| if c.rtl { c.x1 } else { c.x0 };

    match (prev, next) {
        (Some((pi, p)), Some((ni, n))) => {
            if p.rtl == n.rtl {
                trailing(p)
            } else {
                // By UAX #9 L2, logically adjacent clusters with opposite directions have
                // `pi > ni` iff their enclosing bidi level is odd (RTL).
                let enclosing_rtl = pi > ni;
                if p.rtl == enclosing_rtl { trailing(p) } else { leading(n) }
            }
        }
        (Some((_, p)), None) => {
            if p.rtl == base_rtl {
                trailing(p)
            } else if base_rtl {
                clusters.first().unwrap().x0
            } else {
                clusters.last().unwrap().x1
            }
        }
        (None, Some((_, n))) => {
            if n.rtl == base_rtl {
                leading(n)
            } else if base_rtl {
                clusters.last().unwrap().x1
            } else {
                clusters.first().unwrap().x0
            }
        }
        (None, None) => m.inline_min_coord + m.offset,
    }
}

fn line_stops(
    clusters: &[VisCluster],
    base_rtl: bool,
    line: &parley::Line<'_, Ink>,
) -> Vec<(usize, f32)> {
    if clusters.is_empty() {
        let m = line.metrics();
        return vec![(line.text_range().start, m.inline_min_coord + m.offset)];
    }
    let mut bytes = Vec::with_capacity(clusters.len() + 1);
    for c in clusters {
        bytes.push(c.start);
        bytes.push(c.end);
    }
    bytes.sort_unstable();
    bytes.dedup();
    let mut stops: Vec<(usize, f32)> = bytes
        .into_iter()
        .map(|b| (b, primary_x_on_line(clusters, base_rtl, line, b)))
        .collect();
    stops.sort_by(|a, b| {
        a.1.total_cmp(&b.1)
            .then_with(|| if base_rtl { b.0.cmp(&a.0) } else { a.0.cmp(&b.0) })
    });
    stops
}

/// Vertical line-box extent `(block_min_coord, block_max_coord)` in layout device px of the visual
/// line holding `cursor`, matching `TextFlow::line_span` and `TextFlow::block_lines`.
pub(crate) fn line_block_bounds(
    cursor: &parley::editing::Cursor,
    layout: &Layout<Ink>,
) -> Option<(f32, f32)> {
    let (_, line) = line_for_cursor(layout, cursor.index(), cursor.affinity())?;
    let m = line.metrics();
    Some((m.block_min_coord, m.block_max_coord))
}

/// Caret rectangle in layout device px, resolving the line by byte range and affinity and placing
/// bidi boundaries at their primary (enclosing-direction) visual edge. Also adds
/// `line.metrics().inline_min_coord` for indented blocks and moves carets outside pill chips.
#[doc(hidden)]
pub fn cursor_geometry(
    cursor: &parley::editing::Cursor,
    layout: &Layout<Ink>,
    size: f32,
) -> parley::BoundingBox {
    let at = cursor.index();
    let Some((_, line)) = line_for_cursor(layout, at, cursor.affinity()) else {
        return parley::BoundingBox::default();
    };
    let m = line.metrics();
    let clusters = line_clusters(&line);
    let x = primary_x_on_line(&clusters, layout.is_rtl(), &line, at);
    let mut bb = parley::BoundingBox::new(
        x as f64,
        m.block_min_coord.min(m.content_block_min_coord) as f64,
        (x + size) as f64,
        m.block_max_coord.max(m.content_block_max_coord) as f64,
    );
    // Parley puts a caret at a pill's edge against its label, inside the chip: move it outside.
    if let Some(px) = pill_edge(layout, &line, at) {
        let w = bb.x1 - bb.x0;
        bb.x0 = px as f64;
        bb.x1 = bb.x0 + w;
    }
    bb
}

/// The x just outside the pill chip whose label starts or ends at byte `at`, on `line`: after
/// the chip of a pill ending there, else before one starting there.
fn pill_edge(layout: &Layout<Ink>, line: &parley::Line<'_, Ink>, at: usize) -> Option<f32> {
    let ids: Vec<u64> = layout.inline_boxes().filter(|b| b.index == at && b.id & PILL_BOX != 0).map(|b| b.id).collect();
    if ids.is_empty() {
        return None;
    }
    let boxes: Vec<parley::PositionedInlineBox> = line
        .items()
        .filter_map(|it| match it {
            PositionedLayoutItem::InlineBox(b) if ids.contains(&b.id) => Some(b),
            _ => None,
        })
        .collect();
    if let Some(t) = boxes.iter().find(|b| b.id & 1 == 1) {
        return Some(t.x + t.width);
    }
    let lead = boxes.first()?;
    // If bidi reordering moved `lead` away from the pill's glyph run, step left from the run.
    let start_byte = layout.inline_boxes().find(|b| b.id == lead.id)?.index;
    let gr_x0 = line.items().find_map(|it| match it {
        PositionedLayoutItem::GlyphRun(gr) if gr.run().text_range().contains(&start_byte) => Some(gr.offset()),
        _ => None,
    });
    if let Some(gx) = gr_x0.filter(|&gx| (lead.x + lead.width - gx).abs() >= 1.0) {
        return Some(gx - lead.width);
    }
    Some(lead.x)
}

pub(crate) fn line_for_offset(layout: &Layout<Ink>, offset: f32) -> Option<(usize, parley::Line<'_, Ink>)> {
    if offset < 0.0 {
        return Some((0, layout.get(0)?));
    }
    for (i, line) in layout.lines().enumerate() {
        let m = line.metrics();
        if (m.block_min_coord..m.block_max_coord).contains(&offset) {
            return Some((i, line));
        }
    }
    None
}

/// Hit-tests `(x, y)` (layout device px) to a [`parley::editing::Cursor`], accounting for line
/// indentation and pill inline boxes (where Parley's `Cluster::from_point` falls through to the
/// end of the document when a line starts or ends with an inline box, or folds a trailing box's
/// width into the following cluster).
#[doc(hidden)]
pub fn cursor_from_point(layout: &Layout<Ink>, x: f32, y: f32) -> parley::editing::Cursor {
    use parley::{BreakReason, editing::Cursor, layout::Affinity};
    let rtl = layout.is_rtl();
    let mixed = layout.lines().any(|l| line_has_mixed_dir(&l, rtl));
    if !mixed && layout.inline_boxes().next().is_none() && layout.lines().all(|l| l.metrics().inline_min_coord == 0.0) {
        return Cursor::from_point(layout, x, y);
    }
    if layout.len() == 0 {
        return Cursor::from_byte_index(layout, 0, Affinity::Downstream);
    }
    let first = layout.get(0).unwrap();
    let fm = first.metrics();
    if !rtl && y <= fm.block_min_coord && x <= fm.inline_min_coord + fm.offset {
        return Cursor::from_byte_index(layout, 0, Affinity::Downstream);
    }
    let last_idx = layout.len() - 1;
    let last = layout.get(last_idx).unwrap();
    let lm = last.metrics();
    if !rtl && y >= lm.block_max_coord && x >= lm.inline_min_coord + lm.offset + lm.advance {
        return Cursor::from_byte_index(layout, last.text_range().end, Affinity::Upstream);
    }
    let (line_ix, line) = if y <= fm.block_min_coord {
        (0, first)
    } else if y >= lm.block_max_coord {
        (last_idx, last)
    } else {
        line_for_offset(layout, y).unwrap_or((last_idx, last))
    };
    let m = line.metrics();
    let r = line.text_range();
    for item in line.items() {
        if let PositionedLayoutItem::InlineBox(b) = item
            && x >= b.x
            && x <= b.x + b.width
            && let Some(ib) = layout.inline_boxes().find(|ib| ib.id == b.id)
        {
            if b.id & (PILL_BOX | SPAN_BOX) != 0 {
                // A replaced pill (`InlineImage` / `El::pill_inline_image`) occupies its whole
                // visual box inside the leading `PILL_BOX` (`baseline.is_some()`); clicking its
                // trailing half places the caret after the pill, matching glyph hit-testing.
                if b.id & PILL_BOX != 0 && b.id & 1 == 0 && ib.baseline.is_some() {
                    let trail_left = line.items().find_map(|it| match it {
                        PositionedLayoutItem::InlineBox(t) if t.id == (b.id | 1) => Some(t.x <= b.x),
                        _ => None,
                    }).unwrap_or(rtl);
                    let trailing_half = if trail_left { x < b.x + b.width * 0.5 } else { x > b.x + b.width * 0.5 };
                    if trailing_half
                        && let Some(trail) = layout.inline_boxes().find(|t| t.id == (b.id | 1))
                    {
                        return Cursor::from_byte_index(layout, trail.index, Affinity::Upstream);
                    }
                }
                let aff = if b.id & 1 == 0 { Affinity::Downstream } else { Affinity::Upstream };
                return Cursor::from_byte_index(layout, ib.index, aff);
            }
            if b.id & INLINE_BOX != 0 {
                let aff = if r.start <= r.end
                    && ib.index == r.end
                    && line.break_reason() != BreakReason::Explicit
                    && line_ix + 1 < layout.len()
                {
                    Affinity::Upstream
                } else {
                    Affinity::Downstream
                };
                return Cursor::from_byte_index(layout, ib.index, aff);
            }
        }
    }
    // A line holding only inline boxes (no glyph runs) reports `usize::MAX..0`; snap to the
    // nearest inline box's byte offset so clicks left or right of it stay in bounds.
    if r.start > r.end {
        let edge_box = if x <= m.inline_min_coord + m.offset {
            line.items().find_map(|it| match it {
                PositionedLayoutItem::InlineBox(b) => Some(b.id),
                _ => None,
            })
        } else {
            line.items().filter_map(|it| match it {
                PositionedLayoutItem::InlineBox(b) => Some(b.id),
                _ => None,
            }).last()
        };
        let idx = edge_box
            .and_then(|id| layout.inline_boxes().find(|ib| ib.id == id))
            .map_or(0, |ib| ib.index);
        return Cursor::from_byte_index(layout, idx, Affinity::Downstream);
    }
    if line_has_mixed_dir(&line, rtl) {
        let clusters = line_clusters(&line);
        let stops = line_stops(&clusters, rtl, &line);
        let chosen = if x <= stops[0].1 {
            stops[0].0
        } else if x >= stops.last().unwrap().1 {
            stops.last().unwrap().0
        } else {
            stops
                .windows(2)
                .find_map(|w| {
                    (x <= w[1].1).then(|| if x - w[0].1 <= (w[1].1 - w[0].1) * 0.5 { w[0].0 } else { w[1].0 })
                })
                .unwrap_or_else(|| stops.last().unwrap().0)
        };
        let aff = if chosen == r.end && line.break_reason() != BreakReason::Explicit && line_ix + 1 < layout.len() {
            Affinity::Upstream
        } else {
            Affinity::Downstream
        };
        return Cursor::from_byte_index(layout, chosen, aff);
    }
    // Left of a right-to-left line is its end, and right of it its start.
    if x <= m.inline_min_coord + m.offset {
        return if rtl { Cursor::from_byte_index(layout, r.end, Affinity::Upstream) } else { Cursor::from_byte_index(layout, r.start, Affinity::Downstream) };
    }
    for run in line.runs() {
        let mut cx = m.inline_min_coord + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset);
        for cl in run.visual_clusters() {
            let adv = cl.advance();
            if x <= cx + adv {
                let leading = x - cx <= adv * 0.5;
                let (idx, aff) = if cl.is_rtl() {
                    if leading {
                        (cl.text_range().end, Affinity::Upstream)
                    } else {
                        (cl.text_range().start, Affinity::Downstream)
                    }
                } else if leading || cl.is_hard_line_break() {
                    (cl.text_range().start, Affinity::Downstream)
                } else {
                    (cl.text_range().end, Affinity::Upstream)
                };
                return Cursor::from_byte_index(layout, idx, aff);
            }
            cx += adv;
        }
    }
    if line.break_reason() == BreakReason::Explicit && r.end > r.start {
        if let Some(cl) = parley::Cluster::from_byte_index(layout, r.end - 1) {
            return Cursor::from_byte_index(layout, cl.text_range().start, Affinity::Downstream);
        }
    }
    if rtl { Cursor::from_byte_index(layout, r.start, Affinity::Downstream) } else { Cursor::from_byte_index(layout, r.end, Affinity::Upstream) }
}

/// Start (`!end`) or end (`end`) byte of the visual line containing `byte`.
#[doc(hidden)]
pub fn line_edge(layout: &Layout<Ink>, byte: usize, end: bool) -> usize {
    line_edge_with_affinity(layout, byte, parley::layout::Affinity::Downstream, end).0
}

/// Start (`!end`) or end (`end`) byte and affinity of the visual line holding `(byte, side)`.
/// Soft-wrapped line ends return `Affinity::Upstream` so the caret stays visually at the end of
/// the wrapped line rather than jumping to the start of the following line.
#[doc(hidden)]
pub fn line_edge_with_affinity(
    layout: &Layout<Ink>,
    byte: usize,
    side: parley::layout::Affinity,
    end: bool,
) -> (usize, parley::layout::Affinity) {
    use parley::{BreakReason, layout::Affinity};
    let Some((line_ix, line)) = line_for_cursor(layout, byte, side) else {
        return (byte, Affinity::Downstream);
    };
    let r = line.text_range();
    if r.start > r.end {
        return (byte, Affinity::Downstream);
    }
    if !end {
        return (r.start, Affinity::Downstream);
    }
    if line.break_reason() == BreakReason::Explicit && r.end > r.start {
        let edge = parley::Cluster::from_byte_index(layout, r.end - 1).map_or(r.end - 1, |c| c.text_range().start);
        (edge, Affinity::Downstream)
    } else if line.break_reason() != BreakReason::Explicit && line_ix + 1 < layout.len() {
        (r.end, Affinity::Upstream)
    } else {
        (r.end, Affinity::Downstream)
    }
}

/// Byte index on the previous (`!down`) or next (`down`) visual line at `goal`'s x (the column
/// a run of Up and Down keeps), else `byte`'s, and that x; the start on the first line moving
/// up, `None` on the last moving down.
#[doc(hidden)]
pub fn move_line(layout: &Layout<Ink>, byte: usize, down: bool, goal: Option<f32>) -> Option<(usize, f32)> {
    use parley::{editing::Cursor, layout::Affinity};
    if layout.len() == 0 {
        return None;
    }
    let cur = Cursor::from_byte_index(layout, byte, Affinity::Downstream);
    let bb = cursor_geometry(&cur, layout, 0.0);
    let x = goal.unwrap_or(bb.x0 as f32);
    let y = ((bb.y0 + bb.y1) * 0.5) as f32;
    let limit = layout.len() - 1;
    let idx = line_for_offset(layout, y).map_or(limit, |(ix, _)| ix);
    if !down && idx == 0 {
        return Some((0, x));
    }
    if down && idx >= limit {
        return None;
    }
    let line = layout.get(if down { idx + 1 } else { idx - 1 })?;
    let m = line.metrics();
    Some((cursor_from_point(layout, x, (m.block_min_coord + m.block_max_coord) * 0.5).index(), x))
}

/// Like [`move_line`], but respects `side` affinity on wrapped lines and returns `None` at both
/// the first line moving up and the last line moving down so multi-text callers can cross into
/// adjacent sibling texts.
#[doc(hidden)]
pub fn move_line_strict(
    layout: &Layout<Ink>,
    byte: usize,
    side: parley::layout::Affinity,
    down: bool,
    goal: Option<f32>,
) -> Option<(usize, parley::layout::Affinity, f32)> {
    use parley::editing::Cursor;
    if layout.len() == 0 {
        return None;
    }
    let cur = Cursor::from_byte_index(layout, byte, side);
    let bb = cursor_geometry(&cur, layout, 0.0);
    let x = goal.unwrap_or(bb.x0 as f32);
    let (idx, _) = line_for_cursor(layout, byte, side)?;
    if (!down && idx == 0) || (down && idx + 1 >= layout.len()) {
        return None;
    }
    let target_idx = if down { idx + 1 } else { idx - 1 };
    let line = layout.get(target_idx)?;
    let m = line.metrics();
    let hit = cursor_from_point(layout, x, (m.block_min_coord + m.block_max_coord) * 0.5);
    Some((hit.index(), hit.affinity(), x))
}

/// Line boxes (`(BoundingBox, line_idx)`) covering bytes `a..b` of `layout`. When `pill_boxes`
/// is true, a selected pill's leading and trailing chip boxes are included in the highlight;
/// when false (underlines, strikethroughs, link hit boxes), inline boxes are excluded so they
/// never bleed into a neighbouring cluster's box.
#[doc(hidden)]
pub fn range_boxes(
    layout: &Layout<Ink>,
    a: usize,
    b: usize,
    pill_boxes: bool,
) -> Vec<(parley::BoundingBox, usize)> {
    use parley::BreakReason;
    if a >= b {
        return Vec::new();
    }
    let range = a..b;
    let plain = layout.inline_boxes().next().is_none();
    let justified = layout.alignment() == Some(Alignment::Justify);
    let mut rects = Vec::new();
    for (line_ix, line) in layout.lines().enumerate() {
        let lr = line.text_range();
        if lr.end <= a || lr.start >= b {
            continue;
        }
        let m = line.metrics();
        let line_min = m.block_min_coord.min(m.content_block_min_coord) as f64;
        let line_max = m.block_max_coord.max(m.content_block_max_coord) as f64;
        let (newline_ws, newline_rtl) = if line.break_reason() == BreakReason::Explicit
            && lr.end > lr.start
            && range.contains(&(lr.end - 1))
        {
            parley::Cluster::from_byte_index(layout, lr.end - 1).map_or((0.0, false), |c| {
                let run = c.run();
                let fm = run.font_metrics();
                ((fm.ascent as f64 + fm.descent as f64) * 0.25, c.is_rtl())
            })
        } else {
            (0.0, false)
        };
        let start = (m.inline_min_coord + m.offset) as f64;
        let end = if justified && plain {
            start + line.runs().map(|r| r.advance() as f64).sum::<f64>()
        } else {
            start + m.advance as f64
        };
        let mut intervals: Vec<(f64, f64)> = Vec::new();
        if plain && a <= lr.start && lr.end <= b {
            intervals.push((start, end));
        } else {
            for run in line.runs() {
                let mut cx = (m.inline_min_coord + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset)) as f64;
                for cl in run.visual_clusters() {
                    let adv = cl.advance() as f64;
                    if range.contains(&cl.text_range().start) && adv > 0.0 {
                        intervals.push((cx, cx + adv));
                    }
                    cx += adv;
                }
            }
            if pill_boxes {
                for item in line.items() {
                    if let PositionedLayoutItem::InlineBox(ib) = item
                        && ib.id & PILL_BOX != 0
                        && ib.width > 0.0
                        && layout.inline_boxes().find(|b| b.id == ib.id).is_some_and(|b| {
                            if ib.id & 1 == 0 {
                                range.contains(&b.index)
                            } else {
                                b.index > 0 && range.contains(&(b.index - 1))
                            }
                        })
                    {
                        intervals.push((ib.x as f64, (ib.x + ib.width) as f64));
                    }
                }
            }
        }
        if newline_ws > 0.0 {
            intervals.push(if newline_rtl { (start - newline_ws, start) } else { (end, end + newline_ws) });
        }
        intervals.sort_by(|p, q| p.0.total_cmp(&q.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (x0, x1) in intervals {
            match merged.last_mut() {
                Some(last) if x0 <= last.1 + 0.5 => last.1 = last.1.max(x1),
                _ => merged.push((x0, x1)),
            }
        }
        for (x0, x1) in merged {
            if x1 > x0 {
                rects.push((parley::BoundingBox::new(x0, line_min, x1, line_max), line_ix));
            }
        }
    }
    rects
}

/// Starting byte of the visual cluster directly under `(x, y)` on a mixed-direction line,
/// used by [`TextFlow::hit`] so double-click word selection picks the clicked cluster even
/// when primary-caret hit-testing maps the boundary to the opposite side of an embedded run.
pub(crate) fn cluster_start_at_point(layout: &Layout<Ink>, x: f32, y: f32) -> Option<usize> {
    if layout.len() == 0 {
        return None;
    }
    let first = layout.get(0)?;
    let last = layout.get(layout.len() - 1)?;
    let line = if y <= first.metrics().block_min_coord {
        first
    } else if y >= last.metrics().block_max_coord {
        last
    } else {
        line_for_offset(layout, y).map_or(last, |(_, l)| l)
    };
    if !line_has_mixed_dir(&line, layout.is_rtl()) {
        return None;
    }
    line_clusters(&line)
        .into_iter()
        .find(|c| x >= c.x0 && x <= c.x1)
        .map(|c| c.start)
}

/// One visual step left (`!right`) or right (`right`), by cluster or `word`, across the line
/// holding `(at, side)`. Returns `None` at the line's visual edge that way.
#[doc(hidden)]
pub fn visual_step(
    layout: &Layout<Ink>,
    at: usize,
    side: parley::layout::Affinity,
    right: bool,
    word: bool,
) -> Option<(usize, parley::layout::Affinity)> {
    use parley::{BreakReason, layout::Affinity};
    let (line_ix, line) = line_for_cursor(layout, at, side)?;
    let rtl = layout.is_rtl();
    let clusters = line_clusters(&line);
    let stops = line_stops(&clusters, rtl, &line);
    let snapped = clusters.iter().find(|c| c.start < at && at < c.end).map_or(at, |c| c.start);
    let mut pos = stops.iter().position(|&(b, _)| b == snapped).unwrap_or_else(|| {
        let x = primary_x_on_line(&clusters, rtl, &line, snapped);
        stops.iter().enumerate().min_by(|(_, a), (_, b)| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(0, |(i, _)| i)
    });
    let r = line.text_range();
    let stop_aff = |byte: usize| {
        if byte == r.end && line.break_reason() != BreakReason::Explicit && line_ix + 1 < layout.len() {
            Affinity::Upstream
        } else {
            Affinity::Downstream
        }
    };
    if !right {
        if pos == 0 {
            return None;
        }
        pos -= 1;
        if word && clusters.len() + 1 == stops.len() {
            while pos > 0 {
                let (left, right_cl) = (&clusters[pos - 1], &clusters[pos]);
                let hit = if left.rtl {
                    left.word_boundary && (left.space || (right_cl.word_boundary && !right_cl.space))
                } else {
                    right_cl.word_boundary && !right_cl.space
                };
                if hit {
                    break;
                }
                pos -= 1;
            }
        }
    } else {
        if pos + 1 >= stops.len() {
            return None;
        }
        pos += 1;
        if word && clusters.len() + 1 == stops.len() {
            while pos + 1 < stops.len() {
                let (left, right_cl) = (&clusters[pos - 1], &clusters[pos]);
                let hit = if left.rtl {
                    left.word_boundary && !left.space
                } else {
                    right_cl.word_boundary && !left.space
                };
                if hit {
                    break;
                }
                pos += 1;
            }
        }
    }
    let byte = stops[pos].0;
    Some((byte, stop_aff(byte)))
}

/// Whether the cluster visually immediately `right` (else left) of `(at, side)` is a space
/// the text runs into that way (LTR going right, RTL going left).
#[doc(hidden)]
pub fn space_ahead(layout: &Layout<Ink>, at: usize, side: parley::layout::Affinity, right: bool) -> bool {
    let Some((_, line)) = line_for_cursor(layout, at, side) else { return false };
    let rtl = layout.is_rtl();
    let clusters = line_clusters(&line);
    let stops = line_stops(&clusters, rtl, &line);
    if clusters.len() + 1 != stops.len() {
        let c = parley::editing::Cursor::from_byte_index(layout, at, side);
        return c.visual_clusters(layout)[usize::from(right)].as_ref().is_some_and(|cl| cl.is_space_or_nbsp() && cl.is_rtl() != right);
    }
    let snapped = clusters.iter().find(|c| c.start < at && at < c.end).map_or(at, |c| c.start);
    let Some(pos) = stops.iter().position(|&(b, _)| b == snapped) else { return false };
    let adj = if right { clusters.get(pos) } else { pos.checked_sub(1).and_then(|i| clusters.get(i)) };
    adj.is_some_and(|cl| cl.space && cl.rtl != right)
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod tests;
