//! Line breaking with paragraph indents, line caps and pills kept with their labels.

use crate::*;

/// Per-line breaking constraint applied by [`break_lines_capped`].
#[derive(Clone, Copy, PartialEq)]
enum LineCap {
    /// Available content advance in device pixels (narrowed to push a split pill onto the next
    /// line, or widened to keep an unbreakable pill or trailing box on this line).
    Width(f32),
    /// Exact Parley item/character count for [`parley::BreakLines::break_next_with_length`], used
    /// when `overflow-wrap` breaks inside a word after an inline box that starts the line (where
    /// Parley's soft break opportunity after the inline box would otherwise win over emergency
    /// breaks and strand the inline box alone on a zero-height line).
    Chars(u32),
}

/// First-line `text_indent` in whole device pixels (finite and clamped to keep layout arithmetic
/// finite even for huge inputs).
pub(crate) fn indent_px(text_indent: f32, scale: f32) -> f32 {
    if text_indent == 0.0 || !text_indent.is_finite() {
        0.0
    } else {
        (text_indent * scale).clamp(-1e7, 1e7).round()
    }
}

/// Breaks a shaped `layout` at `max_width` again and aligns its lines, as
/// [`TextSystem::layout_keyed`] breaks the layouts it shapes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebreak(layout: &mut Layout<Ink>, blocks: &[crate::Block], text_indent: f32, font_size: f32, scale: f32, max_width: Option<f32>, wrap_anywhere: bool, align: Alignment) {
    rebreak_at_y(layout, blocks, text_indent, font_size, scale, max_width, wrap_anywhere, align, 0.0);
}

/// [`rebreak`] starting at fractional vertical pixel phase `y_frac` (`y - y.floor()`), so a
/// wrapped paragraph in a [`ParagraphLayout`](super::ParagraphLayout) quantizes each sub-line's
/// `y` identically to [`Layout`](parley::Layout)'s accumulated `line_y`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebreak_at_y(layout: &mut Layout<Ink>, blocks: &[crate::Block], text_indent: f32, font_size: f32, scale: f32, max_width: Option<f32>, wrap_anywhere: bool, align: Alignment, y_frac: f64) {
    let indent = indent_px(text_indent, scale);
    layout.set_text_indent(indent, parley::IndentOptions::default());
    if blocks.iter().any(|&b| b != crate::Block::Paragraph) || layout.inline_boxes().next().is_some() {
        break_lines_with_blocks_at(layout, blocks, indent, font_size, scale, max_width, wrap_anywhere, Vec::new(), y_frac);
    } else if y_frac == 0.0 {
        layout.break_all_lines(max_width);
    } else {
        let mut breaker = layout.break_lines();
        breaker.state_mut().set_line_y(y_frac);
        breaker.break_remaining(max_width.unwrap_or(f32::MAX));
    }
    layout.align(align, parley::AlignmentOptions::default());
}

/// Breaks a layout [`TextSystem::layout_keyed`] shaped from `text`: by blocks and pills when
/// `indented` (it has either), else at `max_width`; then, when `pretty`, a plain one's paragraphs
/// that end on a lone short word again, as [`super::pretty`] scores them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn break_new(layout: &mut Layout<Ink>, text: &str, blocks: &[crate::Block], text_indent: f32, indented: bool, font_size: f32, scale: f32, max_width: Option<f32>, wrap_anywhere: bool, pretty: bool) {
    let indent = indent_px(text_indent, scale);
    layout.set_text_indent(indent, parley::IndentOptions::default());
    if indented {
        break_lines_with_blocks(layout, blocks, indent, font_size, scale, max_width, wrap_anywhere, Vec::new());
        // Pills (a link's icon, a mention's chip) wrap prettily too, as their inline boxes do in
        // Chromium; indented blocks (lists, quotes, code) keep the greedy lines.
        if let (true, Some(width)) = (pretty && blocks.iter().all(|&b| b == crate::Block::Paragraph), max_width) {
            let caps: Vec<_> = super::pretty::caps(layout, text, font_size, scale, width)
                .into_iter()
                .map(|(k, w)| (k, LineCap::Width(w)))
                .collect();
            if !caps.is_empty() {
                break_lines_with_blocks(layout, blocks, indent, font_size, scale, max_width, wrap_anywhere, caps);
            }
        }
        return;
    }
    layout.break_all_lines(max_width);
    if let (true, Some(width)) = (pretty, max_width) {
        let caps: Vec<_> = super::pretty::caps(layout, text, font_size, scale, width)
            .into_iter()
            .map(|(k, w)| (k, LineCap::Width(w)))
            .collect();
        if !caps.is_empty() {
            break_lines_capped(layout, &[], indent, font_size, scale, max_width, &caps, &mut Vec::new(), 0.0);
        }
    }
}

/// Breaks `layout` into lines, each paragraph indented by its block. A pill's chip that would
/// end a line apart from its label, or run past the line's end, moves to the next line. `caps`
/// start as the room given lines (pretty wrapping's); a pill's adjustments come after them.
#[allow(clippy::too_many_arguments)]
fn break_lines_with_blocks(
    layout: &mut Layout<Ink>,
    blocks: &[crate::Block],
    first_indent: f32,
    font_size: f32,
    scale: f32,
    max_width: Option<f32>,
    wrap_anywhere: bool,
    caps: Vec<((usize, usize), LineCap)>,
) {
    break_lines_with_blocks_at(layout, blocks, first_indent, font_size, scale, max_width, wrap_anywhere, caps, 0.0);
}

#[allow(clippy::too_many_arguments)]
fn break_lines_with_blocks_at(
    layout: &mut Layout<Ink>,
    blocks: &[crate::Block],
    first_indent: f32,
    font_size: f32,
    scale: f32,
    max_width: Option<f32>,
    wrap_anywhere: bool,
    mut caps: Vec<((usize, usize), LineCap)>,
    y_frac: f64,
) {
    let pills = layout.inline_boxes().filter(|b| b.id & PILL_BOX != 0).count() / 2;
    let span_boxes = layout.inline_boxes().filter(|b| b.id & SPAN_BOX != 0).count();
    // `(para_idx, line_in_para)` and the cap it gets instead: narrowed to break before a chip,
    // widened to keep an unbreakable chip with its label, or char-capped when `overflow-wrap`
    // breaks inside a long label after a leading box. Paragraphs break independently at `\n`, so
    // one pass can adjust the first misplaced pill or span-box spacer in every paragraph at once.
    let mut lines_meta = Vec::new();
    for _ in 0..=4 * (pills + span_boxes) {
        break_lines_capped(layout, blocks, first_indent, font_size, scale, max_width, &caps, &mut lines_meta, y_frac);
        if max_width.is_none() || (pills == 0 && span_boxes == 0) || !collect_misplaced_pills(layout, &lines_meta, wrap_anywhere, &mut caps) {
            return;
        }
    }
}

/// [`break_lines_with_blocks`] once, with `caps` setting the constraint on given
/// `(para_idx, line_in_para)` lines (the last for a line wins). Fills `out` with
/// `((para_idx, line_in_para), uncapped_content_room)`.
#[allow(clippy::too_many_arguments)]
fn break_lines_capped(
    layout: &mut Layout<Ink>,
    blocks: &[crate::Block],
    first_indent: f32,
    font_size: f32,
    scale: f32,
    max_width: Option<f32>,
    caps: &[((usize, usize), LineCap)],
    out: &mut Vec<((usize, usize), f32)>,
    y_frac: f64,
) {
    out.clear();
    let layout_max = max_width.unwrap_or(f32::MAX);
    let mut breaker = layout.break_lines();
    if y_frac != 0.0 {
        breaker.state_mut().set_line_y(y_frac);
    }
    breaker.state_mut().set_layout_max_advance(layout_max);
    let (mut para_idx, mut line_in_para) = (0usize, 0usize);
    while !breaker.is_done() {
        let blk = blocks.get(para_idx).copied().unwrap_or(crate::Block::Paragraph);
        let [indent, right_pad] = blk.pad_px(font_size, scale);
        let line_max = max_width
            .map_or(f32::MAX, |w| (w - indent - right_pad).max(1.0))
            .min(layout_max);
        let key = (para_idx, line_in_para);
        let text_indent = if key == (0, 0) { first_indent } else { 0.0 };
        let content_room = max_width.map_or(f32::MAX, |_| (line_max - text_indent).max(1.0));
        out.push((key, content_room));
        let cap = caps.iter().rfind(|c| c.0 == key).map(|c| c.1);
        match cap {
            Some(LineCap::Chars(n)) => {
                breaker.state_mut().set_line_x(indent);
                breaker.state_mut().set_layout_max_advance(layout_max);
                breaker.state_mut().set_line_max_advance(line_max);
                match breaker.break_next_with_length(n) {
                    Some(()) => {
                        if max_width.is_some() {
                            breaker.set_prior_line_width(line_max);
                        }
                        line_in_para += 1;
                    }
                    None => break,
                }
            }
            _ => {
                // Width caps express content advance; Parley subtracts `text_indent` from `line_max_advance`.
                let capped = match cap {
                    Some(LineCap::Width(c)) => c + text_indent,
                    _ => line_max,
                };
                breaker.state_mut().set_line_x(indent);
                // Parley wants no line wider than the layout: a widened line widens it while it breaks.
                breaker.state_mut().set_layout_max_advance(layout_max.max(capped));
                breaker.state_mut().set_line_max_advance(capped);
                match breaker.break_next() {
                    Some(parley::YieldData::LineBreak(data)) => {
                        // Keep the line's alignment box at `line_max` when a cap tightened/widened breaking.
                        if cap.is_some() && max_width.is_some() {
                            breaker.set_prior_line_width(line_max);
                        }
                        if data.reason == parley::BreakReason::Explicit {
                            para_idx += 1;
                            line_in_para = 0;
                        } else {
                            line_in_para += 1;
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        }
    }
    breaker.state_mut().set_layout_max_advance(layout_max);
}

/// Logical advance before `first_box` on `line`, and the advance of any contiguous non-whitespace
/// prefix immediately preceding `first_start` on `line` (such as an opening `(` in `(https://…)`).
fn logical_prefix_before(
    layout: &Layout<Ink>,
    line: &parley::Line<'_, Ink>,
    first_box: &parley::PositionedInlineBox,
    first_start: usize,
    at: impl Fn(f32) -> f32,
) -> (f32, f32) {
    let mut before: Vec<(usize, usize, f32, bool)> = Vec::new();
    let mut has_rtl = false;
    for run in line.runs() {
        has_rtl |= run.is_rtl();
        for c in run.clusters() {
            let r = c.text_range();
            if r.end <= first_start {
                before.push((r.start, r.end, c.advance(), c.is_space_or_nbsp() || c.is_hard_line_break()));
            }
        }
    }
    before.sort_by_key(|c| c.0);
    let logical_x = if has_rtl || layout.is_rtl() {
        let mut x: f32 = before.iter().map(|c| c.2).sum();
        let target_ord = layout.inline_boxes().position(|ib| ib.id == first_box.id);
        for item in line.items() {
            if let PositionedLayoutItem::InlineBox(b) = item
                && b.kind == parley::InlineBoxKind::InFlow
                && b.id != first_box.id
                && let Some(ord) = layout.inline_boxes().position(|ib| ib.id == b.id)
                && target_ord.is_some_and(|t| ord < t)
            {
                x += b.width;
            }
        }
        x
    } else {
        at(first_box.x)
    };
    let mut prefix_w = 0.0f32;
    let mut expected_end = first_start;
    for &(start, end, adv, is_ws) in before.iter().rev() {
        if end != expected_end || is_ws {
            break;
        }
        if expected_end != first_start
            && line.items().any(|it| match it {
                PositionedLayoutItem::InlineBox(b) if b.id != first_box.id => {
                    layout.inline_boxes().find(|ib| ib.id == b.id).is_some_and(|ib| ib.index == expected_end)
                }
                _ => false,
            })
        {
            break;
        }
        prefix_w += adv;
        expected_end = start;
    }
    (logical_x, prefix_w)
}

/// Whether `line` contains at least one visible (non-zero advance) cluster in `lead_start..trail_end`.
fn has_label_on_line(line: &parley::Line<'_, Ink>, lead_start: usize, trail_end: usize) -> bool {
    line.runs().any(|run| {
        let r = run.text_range();
        r.start < trail_end
            && lead_start < r.end
            && run.clusters().any(|c| {
                let cr = c.text_range();
                cr.start < trail_end && lead_start < cr.end && c.advance() > 0.0
            })
    })
}

/// Cap for a line starting with a leading inline box (and optional unbreakable prefix) that Parley
/// broke right after, stranding it apart from its word on `next`.
#[allow(clippy::too_many_arguments)]
fn orphan_line_fix(
    layout: &Layout<Ink>,
    line: &parley::Line<'_, Ink>,
    next: &parley::Line<'_, Ink>,
    room: f32,
    wrap_anywhere: bool,
    trail_id: Option<u64>,
    caps: &[((usize, usize), LineCap)],
    key: (usize, usize),
) -> Option<LineCap> {
    let m = line.metrics();
    let mut next_clusters: Vec<(usize, usize, f32, bool, bool)> = Vec::new();
    for run in next.runs() {
        for c in run.clusters() {
            let r = c.text_range();
            next_clusters.push((r.start, r.end, c.advance(), c.is_space_or_nbsp(), c.is_hard_line_break()));
        }
    }
    next_clusters.sort_by_key(|c| c.0);
    let first = next_clusters.iter().find(|c| !c.4)?;

    let mut word_w = 0.0f32;
    let mut word_end = first.0;
    for &(_, end, adv, is_ws, is_nl) in &next_clusters {
        if is_ws || is_nl {
            break;
        }
        word_w += adv;
        word_end = end;
    }
    let trail_w = trail_id
        .and_then(|tid| {
            let ib = layout.inline_boxes().find(|b| b.id == tid)?;
            (ib.index <= word_end).then(|| {
                next.items()
                    .find_map(|it| match it {
                        PositionedLayoutItem::InlineBox(b) if b.id == tid => Some(b.width),
                        _ => None,
                    })
                    .unwrap_or(0.0)
            })
        })
        .unwrap_or(0.0);

    if wrap_anywhere {
        let mut chars_on_line: u32 = line
            .items()
            .filter(|it| matches!(it, PositionedLayoutItem::InlineBox(b) if b.kind == parley::InlineBoxKind::InFlow))
            .count() as u32;
        for run in line.runs() {
            chars_on_line += run.clusters().count() as u32;
        }
        let mut used = m.advance;
        let mut add_chars = 0u32;
        let mut took_visible = false;
        let mut broke_mid_word = false;
        for &(_, _, adv, is_ws, is_nl) in &next_clusters {
            if is_ws || is_nl {
                break;
            }
            if took_visible && used + adv > room + 0.5 {
                broke_mid_word = true;
                break;
            }
            used += adv;
            add_chars += 1;
            if adv > 0.0 {
                took_visible = true;
            }
        }
        if broke_mid_word && add_chars > 0 {
            let mut n = chars_on_line + add_chars;
            if let Some(LineCap::Chars(prev_n)) = caps.iter().rfind(|c| c.0 == key).map(|c| c.1)
                && n <= prev_n
            {
                n = prev_n + 1;
            }
            return Some(LineCap::Chars(n));
        }
    }

    (word_w > 0.0).then_some(LineCap::Width(m.advance + word_w + trail_w + 1.0))
}

/// Appends the first misplaced pill per paragraph to `caps`, returning whether any was added.
fn collect_misplaced_pills(
    layout: &Layout<Ink>,
    lines_meta: &[((usize, usize), f32)],
    wrap_anywhere: bool,
    caps: &mut Vec<((usize, usize), LineCap)>,
) -> bool {
    let before = caps.len();
    let mut skip_para = None;
    let mut boxes: Vec<parley::PositionedInlineBox> = Vec::new();
    for (li, line) in layout.lines().enumerate() {
        let Some(&(key, room)) = lines_meta.get(li) else { break };
        if skip_para == Some(key.0) {
            continue;
        }
        boxes.clear();
        boxes.extend(line.items().filter_map(|it| match it {
            PositionedLayoutItem::InlineBox(b) if b.id & PILL_BOX != 0 => Some(b),
            _ => None,
        }));
        let m = line.metrics();
        // Device px from the start of the line's content, as the breaker counts.
        let at = |x: f32| x - m.inline_min_coord - m.offset;
        let mut adjusted = false;
        for (k, lead) in boxes.iter().enumerate().filter(|(_, b)| b.id & 1 == 0) {
            let trail = boxes.iter().find(|b| b.id == lead.id | 1);
            if trail.is_some_and(|t| at(t.x) + t.width <= room + 0.5) {
                continue;
            }
            let Some(lead_ib) = layout.inline_boxes().find(|ib| ib.id == lead.id) else { continue };
            let trail_end = layout
                .inline_boxes()
                .find(|ib| ib.id == lead.id | 1)
                .map_or(usize::MAX, |ib| ib.index);
            let bare_icon = lead.id & PILL_BARE != 0;
            let has_label = has_label_on_line(&line, lead_ib.index, trail_end);
            if bare_icon && has_label && m.advance - m.hanging_advance <= room + 0.5 {
                continue;
            }
            let mut first = k;
            while first >= 2
                && boxes[first - 1].id & 1 == 1
                && (boxes[first - 1].x + boxes[first - 1].width - boxes[first].x).abs() < 0.5
                && boxes[first - 2].id == boxes[first - 1].id & !1
            {
                first -= 2;
            }
            let first_start = layout
                .inline_boxes()
                .find(|ib| ib.id == boxes[first].id)
                .map_or(lead_ib.index, |ib| ib.index);
            let (logical_x, prefix_w) = logical_prefix_before(layout, &line, &boxes[first], first_start, at);
            let prefix_start = (logical_x - prefix_w).max(0.0);

            let fix = if prefix_start < 0.5 {
                if trail.is_some() {
                    None
                } else if !has_label {
                    layout
                        .get(li + 1)
                        .and_then(|next| orphan_line_fix(layout, &line, &next, room, wrap_anywhere, Some(lead.id | 1), caps, key))
                } else {
                    // A chip starting a line whose trail alone went on to the next: widen this line
                    // to hold the whole pill. A label that itself went on is wider than a line: it wraps.
                    layout.get(li + 1).and_then(|next| {
                        let nm = next.metrics();
                        let t = next.items().find_map(|it| match it {
                            PositionedLayoutItem::InlineBox(b) if b.id == lead.id | 1 => Some(b),
                            _ => None,
                        })?;
                        let x = t.x - nm.inline_min_coord - nm.offset;
                        (x < 0.5).then_some(LineCap::Width(m.advance + x + t.width + 1.0))
                    })
                }
            } else if prefix_w > 0.0 {
                // Keep non-whitespace prefix (such as `(` before a link) attached to the box.
                Some(LineCap::Width(logical_x - (prefix_w * 0.5).min(0.5)))
            } else if boxes[first].width > 0.0 {
                Some(LineCap::Width(logical_x + boxes[first].width * 0.5))
            } else {
                Some(LineCap::Width((logical_x - 0.5).max(0.0)))
            };
            if let Some(cap_val) = fix {
                let cap = (key, cap_val);
                if !caps.contains(&cap) {
                    caps.push(cap);
                    skip_para = Some(key.0);
                    adjusted = true;
                    break;
                }
            }
        }
        if adjusted {
            continue;
        }
        // Keep a padded `SpanBox`'s leading spacer with its first word and its trailing spacer
        // with its last word while still allowing multi-word span boxes to wrap inside.
        let lr = line.text_range();
        let orphaned_lead = line.items().find_map(|it| match it {
            PositionedLayoutItem::InlineBox(b) if b.id & SPAN_BOX != 0 && b.id & 1 == 0 => {
                let ib = layout.inline_boxes().find(|ib| ib.id == b.id)?;
                let empty_after = lr.start >= lr.end || ib.index >= lr.end;
                if !empty_after {
                    return None;
                }
                let (logical_x, prefix_w) = logical_prefix_before(layout, &line, &b, ib.index, at);
                let prefix_start = (logical_x - prefix_w).max(0.0);
                if prefix_start < 0.5 {
                    layout
                        .get(li + 1)
                        .and_then(|next| orphan_line_fix(layout, &line, &next, room, wrap_anywhere, Some(b.id | 1), caps, key))
                } else if prefix_w > 0.0 {
                    Some(LineCap::Width(logical_x - (prefix_w * 0.5).min(0.5)))
                } else {
                    Some(LineCap::Width(logical_x + b.width * 0.5))
                }
            }
            _ => None,
        });
        if let Some(cap_val) = orphaned_lead {
            let cap = (key, cap_val);
            if !caps.contains(&cap) {
                caps.push(cap);
                skip_para = Some(key.0);
                continue;
            }
        }
        if let Some(next) = layout.get(li + 1) {
            let nm = next.metrics();
            let nr = next.text_range();
            let orphaned_trail = next.items().find_map(|it| match it {
                PositionedLayoutItem::InlineBox(b) if b.id & SPAN_BOX != 0 && b.id & 1 == 1 => {
                    let ib = layout.inline_boxes().find(|ib| ib.id == b.id)?;
                    let x = b.x - nm.inline_min_coord - nm.offset;
                    (x < 0.5 && ib.index <= nr.start).then_some((x, b.width))
                }
                _ => None,
            });
            if let Some((x, tw)) = orphaned_trail {
                let shrink = (m.advance - 1.0).max(1.0);
                let cap = if m.advance > 1.5 && !caps.contains(&(key, LineCap::Width(shrink))) {
                    (key, LineCap::Width(shrink))
                } else {
                    (key, LineCap::Width(m.advance + x + tw + 1.0))
                };
                if !caps.contains(&cap) {
                    caps.push(cap);
                    skip_para = Some(key.0);
                }
            }
        }
    }
    caps.len() > before
}

#[cfg(test)]
#[path = "text_indent_tests.rs"]
mod tests;
