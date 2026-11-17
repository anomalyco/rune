//! Text inside the accessibility tree: a text field's lines as text runs with each
//! character's place, so a reader moves by character, word and line and a magnifier follows
//! the caret; a selectable text's lines likewise; and a text's inline links as link nodes.

use accesskit::{Action as Ak, Node as AkNode, NodeId as AkId, Rect as AkRect, Role, TextDirection, TextPosition, TextSelection};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};

use super::*;
use super::a11y_tree::{LINES_BOX_CLASS, LINE_CLASS, drawn_line_id, element_id, line_id, lines_box_id, link_id, round};
use crate::text::TextFlow;

/// Text longer than this gets no per-character runs (a reader still gets its whole value). A field
/// past it marks its selection on itself, publishes only its drawn visual lines under an
/// intermediate container, and lets the vendored adapters answer its ranges and boxes on demand.
const RUN_MAX_BYTES: usize = 64 * 1024;
/// Cap on drawn visual lines published for one long field, applied before building cluster arrays.
const MAX_DRAWN_LINES: usize = 512;

/// One laid-out line: its bytes in the drawn text and each character's place in points.
pub(crate) struct Line {
    pub range: std::ops::Range<usize>,
    pub rect: [f32; 4],
    pub lengths: Vec<u8>,
    pub positions: Vec<f32>,
    pub widths: Vec<f32>,
    pub word_starts: Vec<u8>,
    pub rtl: bool,
}

/// `content`'s lines from `layout` (device px at `s`), placed from `origin` in points. A
/// by-paragraph field's newline is the last character of the line it ends, as in one layout.
pub(crate) fn lines(layout: TextFlow<'_>, content: &str, s: f32, origin: [f32; 2]) -> Vec<Line> {
    let mut out = Vec::new();
    for p in layout.pieces() {
        push_piece_lines_in(p, content, s, origin, None, usize::MAX, &mut out);
    }
    out
}

fn push_span_chunks(
    content: &str,
    span: std::ops::Range<usize>,
    pos: f32,
    width: f32,
    lengths: &mut Vec<u8>,
    positions: &mut Vec<f32>,
    widths: &mut Vec<f32>,
) {
    let mut cur = span.start.min(content.len());
    let end = span.end.min(content.len());
    let mut first = true;
    while cur < end {
        let rem = end - cur;
        let step = if rem <= 255 {
            rem
        } else {
            let cut = content.floor_char_boundary(cur + 255);
            if cut > cur { cut - cur } else { rem.min(255) }
        };
        lengths.push(step as u8);
        if first {
            positions.push(pos);
            widths.push(width);
            first = false;
        } else {
            positions.push(pos + width);
            widths.push(0.0);
        }
        cur += step;
    }
}

fn build_visual_line(
    p: &crate::text::Piece<'_>,
    line: parley::layout::Line<'_, crate::text::Ink>,
    content: &str,
    s: f32,
    origin: [f32; 2],
) -> Option<Line> {
    let m = line.metrics();
    let range = p.content(line.text_range().start)..p.content(line.text_range().end);
    if range.end > content.len() { return None; }
    let mut raw_clusters = Vec::new();
    let mut line_rtl = p.flow == crate::el::Flow::Rtl;
    let mut max_right = m.offset;
    for run in line.runs() {
        let mut cx = m.inline_min_coord
            + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset);
        for cl in run.visual_clusters() {
            let adv = cl.advance();
            let r = cl.text_range();
            let r = p.content(r.start)..p.content(r.end);
            if cl.is_rtl() { line_rtl = true; }
            if r.start < range.end && r.end <= range.end && !r.is_empty() {
                raw_clusters.push((r, (cx - m.offset) / s, adv / s, cl.is_word_boundary()));
            }
            cx += adv;
            max_right = max_right.max(cx);
        }
    }
    raw_clusters.sort_by_key(|(r, _, _, _)| r.start);
    let (mut lengths, mut positions, mut widths, mut word_starts) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut at = range.start;
    for (r, vis_pos, vis_w, wb) in raw_clusters {
        if r.end <= at { continue; }
        let start = r.start.max(at);
        if start > at {
            let prev_end = positions.last().zip(widths.last()).map_or(0.0, |(&px, &pw)| px + pw);
            push_span_chunks(content, at..start, prev_end, 0.0, &mut lengths, &mut positions, &mut widths);
        }
        if wb && !lengths.is_empty() && lengths.len() < 256 {
            word_starts.push(lengths.len() as u8);
        }
        push_span_chunks(content, start..r.end, vis_pos, vis_w, &mut lengths, &mut positions, &mut widths);
        at = r.end;
    }
    if at < range.end {
        let end_x = (max_right - m.offset).max(0.0) / s;
        push_span_chunks(content, at..range.end, end_x, 0.0, &mut lengths, &mut positions, &mut widths);
    }
    word_starts.retain(|&w| (w as usize) < lengths.len());
    let top = (p.y + m.block_min_coord) / s;
    let mut height = (m.block_max_coord - m.block_min_coord) / s;
    if !p.is_whole() && p.start == p.end && height <= 0.0 {
        height = p.height / s;
    }
    let rect = [origin[0] + m.offset / s, origin[1] + top, (max_right - m.offset).max(0.0) / s, height];
    Some(Line { range, rect, lengths, positions, widths, word_starts, rtl: line_rtl })
}

fn finish_piece_newline(
    p: &crate::text::Piece<'_>,
    content: &str,
    first: usize,
    included_last: bool,
    out: &mut Vec<Line>,
) {
    if p.is_whole() || !included_last { return; }
    if content.as_bytes().get(p.end) != Some(&b'\n') { return; }
    // Parley treats a `\r` at the end of a `\n`-split paragraph (`CRLF`) as a forced break that
    // adds a trailing empty `p.end..p.end` visual line; drop that spurious empty line before `\n`.
    if p.end > p.start
        && content.as_bytes().get(p.end - 1) == Some(&b'\r')
        && out.len() > first + 1
        && out.last().is_some_and(|l| l.range == (p.end..p.end))
    {
        out.pop();
    }
    if let Some(l) = out.last_mut() {
        l.range.end += 1;
        l.lengths.push(1);
        l.positions.push(if l.rtl { 0.0 } else { l.rect[2] });
        l.widths.push(0.0);
    }
}

fn push_piece_lines_in(
    p: crate::text::Piece<'_>,
    content: &str,
    s: f32,
    origin: [f32; 2],
    band: Option<(f32, f32)>,
    max_out: usize,
    out: &mut Vec<Line>,
) {
    if out.len() >= max_out { return; }
    let first = out.len();
    let total = p.layout.len();
    let lo = match band {
        Some((top_px, _)) => {
            let (mut l, mut h) = (0usize, total);
            while l < h {
                let mid = l + (h - l) / 2;
                if p.layout.get(mid).is_some_and(|ln| p.y + ln.metrics().block_max_coord < top_px) {
                    l = mid + 1;
                } else {
                    h = mid;
                }
            }
            l
        }
        None => 0,
    };
    let mut last_built = None;
    for li in lo..total {
        if out.len() >= max_out { break; }
        let Some(ln) = p.layout.get(li) else { break };
        if let Some((_, bot_px)) = band
            && p.y + ln.metrics().block_min_coord > bot_px
        {
            break;
        }
        if let Some(built) = build_visual_line(&p, ln, content, s, origin) {
            out.push(built);
            last_built = Some(li);
        } else {
            break;
        }
    }
    if !p.is_whole() && total == 0 && out.len() == first && out.len() < max_out {
        if band.is_none_or(|(top_px, bot_px)| p.y + p.height >= top_px && p.y <= bot_px) {
            out.push(Line {
                range: p.start..p.start,
                rect: [origin[0], origin[1] + p.y / s, 0.0, p.height / s],
                lengths: Vec::new(),
                positions: Vec::new(),
                widths: Vec::new(),
                word_starts: Vec::new(),
                rtl: p.flow == crate::el::Flow::Rtl,
            });
            finish_piece_newline(&p, content, first, true, out);
            return;
        }
    }
    finish_piece_newline(&p, content, first, total > 0 && last_built == Some(total - 1), out);
}

/// The line and character index of drawn byte `byte`.
fn position(lines: &[Line], byte: usize) -> Option<(usize, usize)> {
    let last = lines.len().checked_sub(1)?;
    let k = lines.iter().position(|l| byte < l.range.end).unwrap_or(last);
    let l = &lines[k];
    let mut at = l.range.start;
    for (c, len) in l.lengths.iter().enumerate() {
        if byte < at + *len as usize { return Some((k, c)); }
        at += *len as usize;
    }
    Some((k, l.lengths.len()))
}

/// Drawn byte at line `k`'s character `c`.
pub(crate) fn byte_at(lines: &[Line], k: usize, c: usize) -> Option<usize> {
    let l = lines.get(k)?;
    Some(l.range.start + l.lengths.iter().take(c).map(|&n| n as usize).sum::<usize>())
}

impl<S: 'static> Runtime<S> {
    /// Node `i`'s text as AccessKit sees it: the lines of a field or selectable text, and the
    /// links of any text, pushed after it as its children.
    pub(super) fn a11y_text_nodes(&self, i: usize, w: &a11y_tree::Walk, out: &mut Vec<(AkId, AkNode)>) {
        let n = &self.nodes[i];
        if let Some(g) = n.vis.grid.as_deref() {
            return self.a11y_grid_rows(i, g, w, out);
        }
        let Some(spec) = n.text.as_deref() else { return };
        let me = out.len() - 1;
        let runs = n.input.is_some() || (spec.selectable && !n.is_secure());
        let long_field = self.a11y_uses_long_field(i);
        let found = (!long_field && runs).then(|| self.a11y_lines(i)).flatten();
        if let Some((content, lines)) = found {
            for (k, l) in lines.iter().enumerate() {
                let id = line_id(n.id, k);
                let mut run = AkNode::new(Role::TextRun);
                run.set_value(content.get(l.range.clone()).unwrap_or_default());
                run.set_text_direction(if l.rtl { TextDirection::RightToLeft } else { TextDirection::LeftToRight });
                run.set_bounds(AkRect {
                    x0: round(l.rect[0] + w.shift[0]), y0: round(l.rect[1] + w.shift[1]),
                    x1: round(l.rect[0] + l.rect[2] + w.shift[0]), y1: round(l.rect[1] + l.rect[3] + w.shift[1]),
                });
                run.set_character_lengths(l.lengths.clone());
                run.set_character_positions(l.positions.clone());
                run.set_character_widths(l.widths.clone());
                run.set_word_starts(l.word_starts.clone());
                out[me].1.push_child(id);
                out.push((id, run));
            }
            if let Some(sel) = self.a11y_selection(i, &content, &lines) { out[me].1.set_text_selection(sel); }
        } else if long_field && let Some(input) = n.input.as_deref() {
            // A long field (past `RUN_MAX_BYTES`, waiting paragraphs, or clipped lines): its
            // selection in its value's bytes on the field itself, and one stable child container
            // (`lines_box_id`) holding its drawn visual lines so scrolling updates only that
            // container without rebuilding or copying the field's value.
            let at = |character_index| TextPosition { node: element_id(n.id), character_index };
            out[me].1.set_text_selection(TextSelection { anchor: at(input.edit.anchor), focus: at(input.edit.focus) });
            out[me].1.push_child(lines_box_id(n.id));
        } else if let Some(input) = n.input.as_deref() && !input.edit.is_secure() {
            let at = |character_index| TextPosition { node: element_id(n.id), character_index };
            out[me].1.set_text_selection(TextSelection { anchor: at(input.edit.anchor), focus: at(input.edit.focus) });
        }
        if spec.cold.links.is_empty() || spec.redacted { return; }
        let boxes = self.link_boxes(i, self.drawn_box(i));
        for (k, l) in spec.cold.links.iter().enumerate() {
            let Some(r) = boxes.iter().filter(|(j, _)| *j == k).fold(None, |u: Option<[f32; 4]>, (_, r)| {
                Some(u.map_or(*r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]))
            }) else { continue };
            let role = crate::a11y::roles::map(l.role, &crate::a11y::roles::RoleCtx {
                pressable: true, focusable: true, input: false, multiline: false, secure: false, scrolls: false, modal: false,
                default_button: false, labelled: true, keyed: true, has_text: true, image: false,
            }).unwrap_or(Role::Link);
            let mut link = AkNode::new(role);
            let words = spec.source_slice(l.start, l.end).unwrap_or("");
            link.set_label(l.label.as_deref().unwrap_or(words));
            link.set_author_id(l.id.as_str());
            if role == Role::Link && l.id.contains("://") { link.set_url(l.id.as_str()); }
            link.set_bounds(AkRect {
                x0: round(r[0] + w.shift[0]), y0: round(r[1] + w.shift[1]),
                x1: round(r[2] + w.shift[0]), y1: round(r[3] + w.shift[1]),
            });
            if self.allowed(i) {
                if spec.cold.on_link.is_some() { link.add_action(Ak::Click); }
                if self.can_focus(i) { link.add_action(Ak::Focus); }
                if spec.cold.on_link_secondary.is_some() { link.add_action(Ak::ShowContextMenu); }
            }
            let id = link_id(n.id, k);
            out[me].1.push_child(id);
            out.push((id, link));
        }
    }

    /// A terminal grid's rows as its text lines, top to bottom: each row's text (its pinned
    /// cells in their columns) with every character's place on the cell grid, so a reader moves
    /// through the screen by character, word and line.
    fn a11y_grid_rows(&self, i: usize, g: &crate::el::grid::Grid, w: &a11y_tree::Walk, out: &mut Vec<(AkId, AkNode)>) {
        let n = &self.nodes[i];
        let me = out.len() - 1;
        let r = self.drawn[i];
        let [cw, ch] = g.cell;
        let shown = g.rows.len().min((r[3] / ch.max(1.0)).ceil() as usize);
        for (k, row) in g.rows.iter().take(shown).enumerate() {
            let (mut text, mut places) = row.reading();
            let read = places.len();
            let y = r[1] + k as f32 * ch;
            // A row's line ends in a line break, as a text field's does, so a reader's text
            // calls (and braille, and say-all) keep rows apart; a wrapped row runs on into the
            // next. The break takes no room, at the row's end.
            let end = places.last().map_or(0, |&(c, w)| c + w.max(1));
            if !row.wraps && k + 1 < shown {
                text.push('\n');
                places.push((end, 0));
            }
            // A wrapped row keeps the spaces at its end: they are the line's, between its words
            // ("echo" at the edge, "ab" on the next row).
            if row.wraps {
                text.extend((end..g.cols).map(|_| ' '));
                places.extend((end..g.cols).map(|col| (col, 1)));
            }
            let mut run = AkNode::new(Role::TextRun);
            run.set_bounds(AkRect {
                x0: round(r[0] + w.shift[0]), y0: round(y + w.shift[1]),
                x1: round(r[0] + f32::from(g.cols) * cw + w.shift[0]), y1: round(y + ch + w.shift[1]),
            });
            run.set_character_lengths(text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<_>>());
            run.set_character_positions(places.iter().map(|&(col, _)| f32::from(col) * cw).collect::<Vec<_>>());
            run.set_character_widths(places.iter().map(|&(_, span)| f32::from(span) * cw).collect::<Vec<_>>());
            let chars: Vec<char> = text.chars().collect();
            let starts: Vec<u8> = (0..chars.len())
                .filter(|&c| !chars[c].is_whitespace() && (c == 0 || chars[c - 1].is_whitespace()))
                .map(|c| c.min(255) as u8)
                .collect();
            run.set_word_starts(starts);
            let id = line_id(n.id, k);
            // The cursor's cell is the caret, so a reader follows typing and output.
            if let Some((col, _)) = g.caret.filter(|c| usize::from(c.1) == k) {
                // Past the text, the caret sits before the line break, at the row's end.
                let at = TextPosition { node: id, character_index: crate::el::GridRow::char_at(&places[..read], col) };
                out[me].1.set_text_selection(TextSelection { anchor: at, focus: at });
            }
            run.set_value(text);
            out[me].1.push_child(id);
            out.push((id, run));
        }
    }

    /// What node `i` draws as text: a field's display text (its mask when secure), else its
    /// content.
    fn a11y_drawn_text(&self, i: usize) -> Option<Cow<'_, str>> {
        let n = &self.nodes[i];
        match n.input.as_deref() {
            Some(input) if !input.edit.is_secure() && input.edit.preedit().is_none() => Some(Cow::Borrowed(input.edit.text.as_str())),
            Some(input) => Some(Cow::Owned(input.edit.display().0)),
            None => n.text.as_deref().map(|t| Cow::Borrowed(t.content.as_str())),
        }
    }

    /// Single shared predicate deciding whether node `i` is a non-secret text field answered
    /// through the on-demand long-field path (past `RUN_MAX_BYTES`, waiting lazy paragraphs, or
    /// truncated/line-capped so runs would not cover the whole value).
    pub(super) fn a11y_uses_long_field(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        let (Some(spec), Some(tl), Some(input)) = (n.text.as_deref(), n.text_layout.as_ref(), n.input.as_deref()) else {
            return false;
        };
        if input.edit.is_secure() || n.elided.is_some() { return false; }
        let waiting = matches!(tl, crate::text::TextLayout::Paragraphs(p) if p.waiting());
        waiting
            || input.edit.text.len() > RUN_MAX_BYTES
            || spec.content.len() > RUN_MAX_BYTES
            || spec.max_lines > 0
            || spec.truncate
            || spec.truncate_fade
    }

    /// Lightweight O(1) fingerprint for long field `i`'s `lines_box_id` container (zero text hashing).
    pub(super) fn a11y_lines_box_print(&self, i: usize, w: &a11y_tree::Walk) -> u64 {
        let n = &self.nodes[i];
        let mut h = rustc_hash::FxHasher::default();
        n.text_layout.as_ref().map(crate::text::TextLayout::address).hash(&mut h);
        self.drawn[i].map(f32::to_bits).hash(&mut h);
        w.shift.map(f32::to_bits).hash(&mut h);
        w.clip_y.map(f32::to_bits).hash(&mut h);
        self.env.scale.to_bits().hash(&mut h);
        self.input_scroll.get(&n.path).map(|v| v.to_bits()).hash(&mut h);
        if let Some(input) = n.input.as_deref() {
            let e = &input.edit;
            let upstream = e.focused && e.anchor == e.focus && e.affinity == parley::Affinity::Upstream;
            (e.anchor, e.focus, e.focused, upstream, e.preedit().map(|p| (p.0.len(), p.1))).hash(&mut h);
        }
        h.finish()
    }

    /// Builds `(lines_box_id, box_node)` followed by its drawn `(drawn_line_id, line_node)`s.
    pub(super) fn a11y_build_lines_box(&self, i: usize, w: &a11y_tree::Walk) -> Vec<(AkId, AkNode)> {
        let n = &self.nodes[i];
        let box_id = lines_box_id(n.id);
        let mut box_node = AkNode::new(Role::GenericContainer);
        box_node.set_class_name(LINES_BOX_CLASS);
        let Some(input) = n.input.as_deref() else {
            return vec![(box_id, box_node)];
        };
        let content_key = n.text.as_deref().and_then(|t| t.shape.map(|(_, k)| k))
            .or_else(|| n.text_layout.as_ref().and_then(crate::text::TextLayout::content_key))
            .or_else(|| n.text_layout.as_ref().map(|tl| tl.address() as u64))
            .unwrap_or(0);
        let upstream = u8::from(
            input.edit.focused
                && input.edit.anchor == input.edit.focus
                && input.edit.affinity == parley::Affinity::Upstream,
        );
        box_node.set_author_id(format!("{content_key:x}:{upstream}"));

        let drawn = self.a11y_drawn_lines(i, w.clip_y);
        let preedit_info = input.edit.preedit().map(|(p, _)| {
            let r = input.edit.range();
            (r.0, r.1, p.len())
        });
        let model_len = input.edit.text.len();
        let map_disp = |d: usize, is_end: bool| -> usize {
            let Some((sel_a, sel_b, p_len)) = preedit_info else {
                return d.min(model_len);
            };
            if d <= sel_a {
                d.min(model_len)
            } else if d < sel_a + p_len {
                if is_end { sel_b.min(model_len) } else { sel_a.min(model_len) }
            } else {
                (d - sel_a - p_len + sel_b).min(model_len)
            }
        };

        let mut out = Vec::with_capacity(1 + drawn.len());
        let mut child_ids = Vec::with_capacity(drawn.len());
        for (k, l) in drawn.into_iter().take(MAX_DRAWN_LINES).enumerate() {
            let (a, b) = (map_disp(l.range.start, false), map_disp(l.range.end, true));
            let id = drawn_line_id(n.id, k);
            let mut line_node = AkNode::new(Role::GenericContainer);
            line_node.set_class_name(LINE_CLASS);
            line_node.set_author_id(format!("{a}:{b}"));
            line_node.set_text_direction(if l.rtl { TextDirection::RightToLeft } else { TextDirection::LeftToRight });
            line_node.set_bounds(AkRect {
                x0: round(l.rect[0] + w.shift[0]), y0: round(l.rect[1] + w.shift[1]),
                x1: round(l.rect[0] + l.rect[2] + w.shift[0]), y1: round(l.rect[1] + l.rect[3] + w.shift[1]),
            });
            if preedit_info.is_none() {
                line_node.set_character_lengths(l.lengths);
                line_node.set_character_positions(l.positions);
                line_node.set_character_widths(l.widths);
            }
            child_ids.push(id);
            out.push((id, line_node));
        }
        box_node.set_children(child_ids);
        out.insert(0, (box_id, box_node));
        out
    }

    /// The shaped visual lines of long field `i` inside window clip band `clip_y` (plus its
    /// focused caret visual line when outside `clip_y`, kept intentionally so Windows Magnifier
    /// can locate an off-screen caret), in points in its content space.
    fn a11y_drawn_lines(&self, i: usize, clip_y: [f32; 2]) -> Vec<Line> {
        let n = &self.nodes[i];
        let (Some(spec), Some(tl), Some(input)) = (n.text.as_deref(), n.text_layout.as_ref(), n.input.as_deref()) else {
            return Vec::new();
        };
        if n.elided.is_some() { return Vec::new(); }
        // `spec.content` is already the laid-out display string (including any IME preedit), so no
        // copy of a multi-megabyte buffer is ever made here.
        let content = spec.content.as_str();
        let layout = tl.view();
        let s = self.env.scale;
        let r = self.drawn[i];
        let (x0, x1) = (r[0] + n.inset[0], r[0] + r[2] - n.inset[2]);
        let mut x = text_x(spec, x0, x1, layout.full_width() / s, layout.flow());
        if input.single_line {
            x -= self.input_scroll.get(&n.path).copied().unwrap_or(0.0);
        }
        let origin = [x, r[1] + n.inset[1]];
        let layout_h = layout.spec_height(spec.style, s, spec.exact_lines);
        let raw_top = (clip_y[0] - origin[1]) * s;
        let raw_bot = (clip_y[1] - origin[1]) * s;
        let mut out = Vec::new();
        if clip_y[1] > clip_y[0] && raw_bot >= 0.0 && raw_top <= layout_h {
            let top_px = raw_top.max(0.0);
            let bot_px = raw_bot.max(top_px);
            for p in layout.pieces_in(top_px, bot_px).filter(|p| p.shaped) {
                if out.len() >= MAX_DRAWN_LINES { break; }
                push_piece_lines_in(p, content, s, origin, Some((top_px, bot_px)), MAX_DRAWN_LINES, &mut out);
            }
        }
        if input.edit.focused && out.len() < MAX_DRAWN_LINES {
            let caret = if input.edit.preedit().is_some() {
                caret_index(&input.edit, content)
            } else {
                input.edit.display_index(input.edit.focus)
            }
            .min(content.len());
            let covered = out.iter().any(|l| {
                (l.range.start <= caret && caret < l.range.end)
                    || (l.range.start == caret && l.range.end == caret)
                    || (caret == l.range.end && caret == content.len() && !content.ends_with('\n'))
            });
            if !covered {
                let cp = layout.piece_at(caret);
                if cp.shaped {
                    let shaped_caret = cp.shaped(caret);
                    let total = cp.layout.len();
                    if total == 0 && !cp.is_whole() {
                        let mut single = Vec::new();
                        push_piece_lines_in(cp, content, s, origin, None, 1, &mut single);
                        if let Some(cl) = single.pop() {
                            let pos = out.partition_point(|l| l.range.start < cl.range.start);
                            out.insert(pos, cl);
                        }
                    } else if total > 0 {
                        let li = (0..total)
                            .find(|&idx| {
                                cp.layout.get(idx).is_some_and(|ln| {
                                    let tr = ln.text_range();
                                    (tr.start <= shaped_caret && shaped_caret < tr.end)
                                        || (idx + 1 == total && shaped_caret == tr.end)
                                })
                            })
                            .unwrap_or(total - 1);
                        if let Some(ln) = cp.layout.get(li)
                            && let Some(mut cl) = build_visual_line(&cp, ln, content, s, origin)
                        {
                            if !cp.is_whole() && li + 1 == total && content.as_bytes().get(cp.end) == Some(&b'\n') {
                                cl.range.end += 1;
                                cl.lengths.push(1);
                                cl.positions.push(if cl.rtl { 0.0 } else { cl.rect[2] });
                                cl.widths.push(0.0);
                            }
                            let pos = out.partition_point(|l| l.range.start < cl.range.start);
                            out.insert(pos, cl);
                        }
                    }
                }
            }
        }
        out
    }

    /// Node `i`'s laid-out lines, in points in its scroll viewport's content space (before the
    /// walk's shift).
    pub(super) fn a11y_lines(&self, i: usize) -> Option<(Cow<'_, str>, Vec<Line>)> {
        if self.a11y_uses_long_field(i) { return None; }
        let n = &self.nodes[i];
        let (spec, layout) = (n.text.as_deref()?, n.text_layout.as_ref()?.view());
        if n.elided.is_some() { return None; }
        let content = self.a11y_drawn_text(i)?;
        if content.len() > RUN_MAX_BYTES { return None; }
        let s = self.env.scale;
        let r = self.drawn[i];
        let (x0, x1) = (r[0] + n.inset[0], r[0] + r[2] - n.inset[2]);
        let mut x = text_x(spec, x0, x1, layout.full_width() / s, layout.flow());
        if let Some(input) = n.input.as_deref() && input.single_line {
            x -= self.input_scroll.get(&n.path).copied().unwrap_or(0.0);
        }
        let lines = lines(layout, &content, s, [x, r[1] + n.inset[1]]);
        if n.input.is_some() && lines.iter().map(|l| l.range.len()).sum::<usize>() != content.len() {
            return None;
        }
        Some((content, lines))
    }

    /// A field's caret or selection, in its lines.
    fn a11y_selection(&self, i: usize, content: &str, lines: &[Line]) -> Option<TextSelection> {
        let n = &self.nodes[i];
        let input = n.input.as_deref()?;
        let edit = &input.edit;
        let (anchor, focus) = if edit.preedit().is_some() {
            let caret = caret_index(edit, content);
            (caret, caret)
        } else {
            (edit.display_index(edit.anchor), edit.display_index(edit.focus))
        };
        let at = |byte: usize| position(lines, byte).map(|(k, c)| TextPosition { node: line_id(n.id, k), character_index: c });
        Some(TextSelection { anchor: at(anchor)?, focus: at(focus)? })
    }

    /// The value and state a text field adds to its node.
    pub(super) fn a11y_input(&self, _i: usize, input: &crate::el::InputSpec, node: &mut AkNode) {
        if !input.placeholder.is_empty() { node.set_placeholder(input.placeholder.as_str()); }
        if !input.edit.is_secure() { node.set_value(input.edit.text.as_str()); }
        if input.access.is_none() { node.set_read_only(); }
    }
}
