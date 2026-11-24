//! Text inside the accessibility tree: a text field's lines as text runs with each
//! character's place, so a reader moves by character, word and line and a magnifier follows
//! the caret; a selectable text's lines likewise; and a text's inline links as link nodes.

use accesskit::{Action as Ak, Node as AkNode, NodeId as AkId, Rect as AkRect, Role, TextDirection, TextPosition, TextSelection};
use std::borrow::Cow;

use super::tree::{LINK_BASE, Walk, element_id, line_id, lines_box_id, link_id, round};
use crate::el::TextSpec;
use crate::runtime::{RuntimeCore, caret_index, text_origin};
use crate::text::TextFlow;

mod long_field;
mod runs;

pub(super) use runs::{Line, byte_at, lines};

/// Text longer than this gets no per-character runs (a reader still gets its whole value). A field
/// past it marks its selection on itself, publishes only its drawn visual lines under an
/// intermediate container, and lets the vendored adapters answer its ranges and boxes on demand.
const RUN_MAX_BYTES: usize = 64 * 1024;
/// Whether a field without runs marks its selection on itself, in its value's bytes. Only the
/// vendored Windows and macOS adapters read that. AccessKit's AT-SPI adapter takes every
/// selection end to be a `TextRun` and panics on any other node, so on Linux such a field
/// publishes no selection.
const FIELD_SELECTION: bool = cfg!(any(target_os = "windows", target_os = "macos"));

impl RuntimeCore {
    /// Node `i`'s text as AccessKit sees it: the lines of a field or selectable text, and the
    /// links of any text, pushed after it as its children.
    pub(super) fn a11y_text_nodes(&self, i: usize, w: &Walk, out: &mut Vec<(AkId, AkNode)>) {
        let n = &self.tree.nodes[i];
        if n.is_secure() { return; }
        if let Some(g) = n.vis.grid.as_deref() {
            return self.a11y_grid_rows(i, g, w, out);
        }
        let Some(spec) = n.text.as_deref() else { return };
        let me = out.len() - 1;
        let runs = (n.input.is_some() || spec.selectable) && !n.is_secure();
        let long_field = self.a11y_uses_long_field(i);
        let found = (!long_field && runs).then(|| self.a11y_lines(i)).flatten();
        if let Some((content, lines)) = found {
            for (k, l) in lines.iter().enumerate() {
                let id = line_id(n.id, k);
                let mut run = AkNode::new(Role::TextRun);
                run.set_value(content.get(l.range.clone()).unwrap_or_default());
                run.set_text_direction(if l.rtl { TextDirection::RightToLeft } else { TextDirection::LeftToRight });
                if l.continues_line && k > 0 {
                    run.set_previous_on_line(line_id(n.id, k - 1));
                }
                if lines.get(k + 1).is_some_and(|next| next.continues_line) {
                    run.set_next_on_line(line_id(n.id, k + 1));
                }
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
            if FIELD_SELECTION {
                let at = |character_index| TextPosition { node: element_id(n.id), character_index };
                out[me].1.set_text_selection(TextSelection { anchor: at(input.edit.anchor), focus: at(input.edit.focus) });
            }
            out[me].1.push_child(lines_box_id(n.id));
        } else if FIELD_SELECTION && let Some(input) = n.input.as_deref() && !input.edit.is_secure() {
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
    fn a11y_grid_rows(&self, i: usize, g: &crate::el::grid::Grid, w: &Walk, out: &mut Vec<(AkId, AkNode)>) {
        let n = &self.tree.nodes[i];
        let me = out.len() - 1;
        let r = self.paint.drawn[i];
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

    /// What node `i` draws as text: none for a secure node, else a field's display text or the
    /// element's content.
    fn a11y_drawn_text(&self, i: usize) -> Option<Cow<'_, str>> {
        let n = &self.tree.nodes[i];
        if n.is_secure() { return None; }
        match n.input.as_deref() {
            Some(input) if input.edit.preedit().is_none() => Some(Cow::Borrowed(input.edit.text.as_str())),
            Some(input) => Some(Cow::Owned(input.edit.display().0)),
            None => n.text.as_deref().map(|t| Cow::Borrowed(t.content.as_str())),
        }
    }

    fn a11y_drawn_len(&self, i: usize) -> Option<usize> {
        let n = &self.tree.nodes[i];
        if n.is_secure() { return None; }
        match n.input.as_deref() {
            Some(input) => match input.edit.preedit() {
                None => Some(input.edit.text.len()),
                Some((pre, _)) => {
                    let (a, b) = input.edit.range();
                    Some(input.edit.text.len().saturating_sub(b.saturating_sub(a)) + pre.len())
                }
            },
            None => n.text.as_deref().map(|t| t.content.len()),
        }
    }

    /// Node `i`'s text origin in drawn points and its layout-px-per-drawn-point scale `s / z`,
    /// using the same `text_x` and single-line scroll rule as paint and `links::origin`.
    pub(super) fn a11y_text_origin(&self, i: usize, spec: &TextSpec, layout: TextFlow<'_>) -> ([f32; 2], f32) {
        let n = &self.tree.nodes[i];
        let s = self.env.scale;
        let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0).max(1e-4);
        let r = self.paint.drawn[i];
        let [mut x, y] = text_origin(n, spec, layout, [r[0], r[1], r[0] + r[2], r[1] + r[3]], s, z);
        if n.input.as_deref().is_some_and(|inp| inp.single_line) {
            x -= self.edit.input_scroll.get(&n.path).copied().unwrap_or(0.0) * z;
        }
        ([x, y], s / z)
    }

    /// Node `i`'s laid-out lines, in points in its scroll viewport's content space (before the
    /// walk's shift).
    pub(super) fn a11y_lines(&self, i: usize) -> Option<(Cow<'_, str>, Vec<Line>)> {
        if self.a11y_uses_long_field(i) { return None; }
        let n = &self.tree.nodes[i];
        let (spec, tl) = (n.text.as_deref()?, n.text_layout.as_ref()?);
        if n.elided.is_some() { return None; }
        if !spec.by_line && matches!(tl, crate::text::TextLayout::Paragraphs(p) if p.waiting()) {
            return None;
        }
        let layout = tl.view();
        let content = self.a11y_drawn_text(i)?;
        if (!spec.by_line && content.len() > RUN_MAX_BYTES)
            || runs::layout_end_byte(layout, &content) != Some(content.len())
        {
            return None;
        }
        let (origin, px) = self.a11y_text_origin(i, spec, layout);
        let lines = lines(layout, &content, px, origin);
        // `line_id` holds up to `LINK_BASE - 2` (`32,766`) runs per node; if a block exceeds that
        // or its runs don't cover `content`, publish its whole value/label without `TextRun`s.
        if lines.len() > (LINK_BASE as usize - 2)
            || lines.iter().map(|l| l.range.len()).sum::<usize>() != content.len()
        {
            return None;
        }
        Some((content, lines))
    }

    /// A field's caret or selection, in its lines.
    fn a11y_selection(&self, i: usize, content: &str, lines: &[Line]) -> Option<TextSelection> {
        let n = &self.tree.nodes[i];
        if n.is_secure() { return None; }
        let input = n.input.as_deref()?;
        let edit = &input.edit;
        let (anchor, focus) = if edit.preedit().is_some() {
            let caret = caret_index(edit, content);
            (caret, caret)
        } else {
            (edit.display_index(edit.anchor), edit.display_index(edit.focus))
        };
        let at = |byte: usize| runs::position(lines, byte).map(|(k, c)| TextPosition { node: line_id(n.id, k), character_index: c });
        Some(TextSelection { anchor: at(anchor)?, focus: at(focus)? })
    }

    /// The value and state a text field adds to its node.
    pub(super) fn a11y_input(&self, i: usize, input: &crate::el::InputSpec, node: &mut AkNode) {
        if !input.placeholder.is_empty() { node.set_placeholder(input.placeholder.as_str()); }
        if !self.tree.nodes[i].is_secure() { node.set_value(input.edit.text.as_str()); }
        if input.access.is_none() || self.is_node_read_only(i) { node.set_read_only(); }
    }
}
