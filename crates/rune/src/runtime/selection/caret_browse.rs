//! Opt-in keyboard caret browsing on mounted `El::selectable(true)` text and
//! `El::selection_group(true)` containers (`El::caret_browsing(true)`).
//!
//! Follows Chrome's caret browsing behaviour and shares `rune-edit`'s grapheme, word, bidi,
//! selection-collapse and platform modifier rules so read-only text fields and caret-browsing
//! selectable documents move identically. Step 2 (unmounted `virtual_rows` extension) plugs a
//! `PendingCaretMove` state machine into the boundary outcomes returned by `step_vertical` /
//! `step_horizontal` / `step_page`.

use super::*;
use crate::edit::{CaretArrow, CaretLanding, CaretMove};
use crate::runtime::selectable::phase;
use crate::runtime::selectable::virtual_selection::EndRef;

/// Distance from `x` outside horizontal span `[x0, x1]`.
fn horiz_dist(x0: f32, x1: f32, x: f32) -> f32 {
    (x0 - x).max(x - x1).max(0.0)
}

/// Non-bare pill byte ranges on `spec` (including the FSI/LRI/RLI..PDI isolate marks
/// `apply_isolates` wraps around each pill), matching `TextEdit`'s atomic pill chips.
fn pill_ranges(spec: &crate::el::TextSpec) -> Vec<(usize, usize)> {
    let s = spec.content.as_str();
    spec.cold
        .pills
        .iter()
        .filter(|p| p.bare.is_none() && p.start < p.end)
        .map(|p| {
            let (mut a, mut b) = (p.start as usize, p.end as usize);
            if a >= 3 && s.get(..a).is_some_and(|pre| pre.ends_with(['\u{2066}', '\u{2067}', '\u{2068}'])) {
                a -= 3;
            }
            if s.get(b..).is_some_and(|suf| suf.starts_with('\u{2069}')) {
                b += 3;
            }
            (a, b)
        })
        .collect()
}

/// If `byte` falls strictly inside a pill chip, snaps out in the movement direction `forward`.
fn snap_out_of_pill(pills: &[(usize, usize)], byte: usize, forward: bool) -> usize {
    for &(a, b) in pills {
        if a < byte && byte < b {
            return if forward { b } else { a };
        }
    }
    byte
}

impl RuntimeCore {
    /// Whether node `n` is an opt-in caret-browsing host: a non-input `El::caret_browsing(true)`
    /// element that has not opted out of focus and is either a `selection_group` or selectable,
    /// non-redacted text.
    #[inline]
    pub(crate) fn is_caret_browsing_node(&self, n: &Node) -> bool {
        !n.focus.opt_out
            && n.focus.caret_browsing
            && n.input.is_none()
            && (n.selection_group || n.text.as_ref().is_some_and(|t| t.selectable && !t.redacted))
    }

    /// The caret-browsing host owning selectable text (or group) node `i`, if any.
    pub(in crate::runtime) fn caret_browsing_host_of(&self, i: usize) -> Option<usize> {
        if self.is_selectable(i) {
            if let Some(g) = self.group_of(i)
                && self.is_caret_browsing_node(&self.tree.nodes[g])
            {
                return Some(g);
            }
            if self.is_caret_browsing_node(&self.tree.nodes[i]) {
                return Some(i);
            }
            return None;
        }
        self.is_caret_browsing_node(&self.tree.nodes[i]).then_some(i)
    }

    /// The currently focused caret-browsing host, when mounted, focusable and not disabled.
    pub(crate) fn caret_browsing_focused_host(&self) -> Option<usize> {
        let f = self.focused_index()?;
        (self.is_caret_browsing_node(&self.tree.nodes[f]) && self.can_focus(f) && !self.node_disabled(f))
            .then_some(f)
    }

    /// Mounted selectable texts belonging to caret-browsing host `host`, in tree order.
    pub(crate) fn caret_browsing_texts(&self, host: usize) -> Vec<usize> {
        if self.tree.nodes[host].selection_group {
            self.group_texts(host)
        } else if self.is_selectable(host) && !self.subtree_exiting(host) && self.allowed(host) {
            vec![host]
        } else {
            Vec::new()
        }
    }

    /// Whether selectable text node `text_node` belongs to caret-browsing host `host`.
    pub(crate) fn caret_browsing_contains(&self, host: usize, text_node: usize) -> bool {
        if self.tree.nodes[host].selection_group {
            self.member(text_node, host)
        } else {
            text_node == host && self.is_selectable(host) && !self.subtree_exiting(host) && self.allowed(host)
        }
    }

    /// Whether the current selection already lives inside caret-browsing host `host`.
    pub(in crate::runtime) fn caret_browsing_selection_in(&self, host: usize) -> bool {
        let Some(h) = self.selection_holder_end() else { return false };
        let Some(f) = self.focus_holder_end(h) else { return false };
        let end_in = |end: EndRef| match end {
            EndRef::Mounted(j) => self.caret_browsing_contains(host, j),
            EndRef::Offscreen(..) => {
                self.tree.nodes[host].selection_group
                    && self.end_group_path(end) == Some(self.tree.nodes[host].path.as_str())
            }
        };
        end_in(h) && end_in(f)
    }

    /// Sets a caret-browsing selection across mounted selectable texts, called from assistive
    /// technology (`AccessKit::Action::SetTextSelection`).
    pub(crate) fn set_caret_browse_selection(
        &mut self,
        app: &mut dyn AppDriver,
        host_path: &str,
        anchor_path: String,
        anchor: usize,
        focus_path: String,
        focus: usize,
    ) -> bool {
        if self.kbd.focus.path() != Some(host_path) {
            self.set_focus(app, Some(host_path.to_owned()), true);
        }
        let (Some(ai), Some(fi)) = (self.live(&anchor_path), self.live(&focus_path)) else {
            return false;
        };
        let a_content = self.content_of(ai).to_owned();
        let a = a_content.floor_char_boundary(anchor.min(a_content.len()));
        let f_content = self.content_of(fi).to_owned();
        let f = f_content.floor_char_boundary(focus.min(f_content.len()));
        let mut sel = phase::Selection::new(
            anchor_path,
            a_content,
            (a, if ai == fi { f } else { a }),
            (a, a),
        );
        if ai != fi {
            sel.focus = f;
            sel.far = Some(phase::Far::new(focus_path, f_content));
        }
        self.select_step(phase::Event::Select(sel));
        self.edit.caret_follow = true;
        true
    }

    /// Ensures a newly focused caret-browsing host has a collapsed caret when focus enters it
    /// (e.g. via Tab, `Tx::focus`, or assistive tech) without an existing selection inside it.
    pub(in crate::runtime) fn enter_caret_browsing_focus(&mut self, host: usize) {
        if !self.is_caret_browsing_node(&self.tree.nodes[host]) || self.caret_browsing_selection_in(host) {
            return;
        }
        let texts = self.caret_browsing_texts(host);
        let Some(&first) = texts.first() else { return };
        let path = self.tree.nodes[first].path.clone();
        let content = self.content_of(first).to_owned();
        self.select_step(phase::Event::Select(phase::Selection::new(path, content, (0, 0), (0, 0))));
    }

    /// Focused caret-browsing host's mounted focus text node, clamped focus byte and affinity in
    /// `nodes`, when a caret-browsing host holds focus.
    pub(crate) fn caret_browse_focus_in(&self, nodes: &[Node]) -> Option<(usize, usize, parley::Affinity)> {
        let focus_path = self.kbd.focus.path()?;
        let host = nodes.iter().position(|n| n.path == focus_path)?;
        if !self.is_caret_browsing_node(&nodes[host])
            || !self.takes_focus(&nodes[host])
            || std::iter::successors(nodes[host].parent, |&p| nodes[p].parent)
                .any(|p| nodes[p].a11y.disabled && nodes[p].focus.opt_out)
        {
            return None;
        }
        let committed = std::ptr::eq(nodes, self.tree.nodes.as_slice());
        if committed && (!self.can_focus(host) || self.node_disabled(host)) {
            return None;
        }
        let sel = self.edit.selectable.phase.selection()?;
        let focus_i = if committed {
            let h = self.selection_holder_end()?;
            let EndRef::Mounted(fi) = self.focus_holder_end(h)? else { return None };
            if !self.caret_browsing_contains(host, fi) {
                return None;
            }
            fi
        } else {
            let target_path = sel.far.as_ref().map_or(sel.path.as_str(), |f| f.path.as_str());
            let fi = nodes.iter().position(|n| n.path == target_path)?;
            let fn_node = &nodes[fi];
            if fn_node.input.is_some() || !fn_node.text.as_ref().is_some_and(|t| t.selectable && !t.redacted) {
                return None;
            }
            let in_host = if nodes[host].selection_group {
                std::iter::successors(fn_node.parent, |&p| nodes[p].parent)
                    .find(|&p| nodes[p].selection_group)
                    == Some(host)
            } else {
                fi == host
            };
            if !in_host {
                return None;
            }
            fi
        };
        let content = nodes[focus_i].text.as_ref()?.content.as_str();
        let byte = content.floor_char_boundary(sel.focus.min(content.len()));
        Some((focus_i, byte, self.edit.selectable.affinity))
    }

    /// The collapsed caret `(byte, affinity)` to paint on selectable text node `i`, if node `i`
    /// holds a collapsed caret inside the currently focused caret-browsing host.
    pub(in crate::runtime) fn caret_browse_collapsed_on(&self, i: usize) -> Option<(usize, parley::Affinity)> {
        let sel = self.edit.selectable.phase.selection()?;
        if sel.far.is_some() || sel.anchor != sel.focus {
            return None;
        }
        let (fi, byte, affinity) = self.caret_browse_focus_in(&self.tree.nodes)?;
        (fi == i).then_some((byte, affinity))
    }

    /// Mounted `(host, anchor_node, anchor_byte, focus_node, focus_byte)` for the currently
    /// focused caret-browsing host, for AccessKit and the web accessibility mirror.
    pub(crate) fn caret_browse_endpoints(&self) -> Option<(usize, usize, usize, usize, usize)> {
        let host = self.caret_browsing_focused_host()?;
        let sel = self.edit.selectable.phase.selection()?;
        let EndRef::Mounted(ai) = self.selection_holder_end()? else { return None };
        let EndRef::Mounted(fi) = self.focus_holder_end(EndRef::Mounted(ai))? else { return None };
        if !self.caret_browsing_contains(host, ai) || !self.caret_browsing_contains(host, fi) {
            return None;
        }
        let a_str = self.content_of(ai);
        let f_str = self.content_of(fi);
        let a = a_str.floor_char_boundary(sel.anchor.min(a_str.len()));
        let f = f_str.floor_char_boundary(sel.focus.min(f_str.len()));
        Some((host, ai, a, fi, f))
    }

    /// Window-point caret line `[x, y, 1.5, h]` for caret-browsing selectable text node `i` at
    /// `(byte, affinity)` after scrolling, sticky shifts and growth stack deltas.
    pub(in crate::runtime) fn caret_browse_node_rect(
        &self,
        nodes: &[Node],
        i: usize,
        byte: usize,
        affinity: parley::Affinity,
    ) -> Option<[f32; 4]> {
        let n = &nodes[i];
        let (spec, laid) = (n.text.as_ref()?, n.text_layout.as_ref()?);
        let flow = laid.view();
        let s = self.env.scale;
        let t = sticky::anchored_target(nodes, &self.scroll.scrolls, i, self.viewport);
        let y = t[1] + self.stack_row_dy(nodes, i);
        let (x0, x1) = (t[0] + n.inset[0], t[2] - n.inset[2]);
        let (y0, y1) = (y + n.inset[1], y + (t[3] - t[1]) - n.inset[3]);
        let ox = text_x(spec, x0, x1, text_width(flow.full_width()) / s, flow.flow());
        let oy = text_y(spec, y0, y1, text_height(spec, flow, s) / s);
        let drawn_byte = n.elided.map_or(byte, |e| e.range(byte, byte).0);
        let bb = flow.caret(drawn_byte, affinity, 1.5 * s);
        let h = if bb.y1 > bb.y0 { (bb.y1 - bb.y0) as f32 / s } else { spec.style.line_px() };
        Some([ox + bb.x0 as f32 / s, oy + bb.y0 as f32 / s, 1.5, h])
    }

    /// Unscrolled root-space text origin `[x, y]` in points for node `i`.
    fn text_unscrolled_origin(&self, i: usize) -> Option<[f32; 2]> {
        let n = &self.tree.nodes[i];
        let (spec, laid) = (n.text.as_ref()?, n.text_layout.as_ref()?);
        let flow = laid.view();
        let s = self.env.scale;
        let (x0, x1) = (n.target[0] + n.inset[0], n.target[2] - n.inset[2]);
        let (y0, y1) = (n.target[1] + n.inset[1], n.target[3] - n.inset[3]);
        let ox = text_x(spec, x0, x1, text_width(flow.full_width()) / s, flow.flow());
        let oy = text_y(spec, y0, y1, text_height(spec, flow, s) / s);
        Some([ox, oy])
    }

    /// Unscrolled root-space `[caret_x, caret_mid_y, line_h]` in points for `(i, byte, affinity)`.
    fn caret_unscrolled_pos(&self, i: usize, byte: usize, affinity: parley::Affinity) -> Option<[f32; 3]> {
        let n = &self.tree.nodes[i];
        let (spec, laid) = (n.text.as_ref()?, n.text_layout.as_ref()?);
        let [ox, oy] = self.text_unscrolled_origin(i)?;
        let s = self.env.scale;
        let drawn_byte = n.elided.map_or(byte, |e| e.range(byte, byte).0);
        let bb = laid.view().caret(drawn_byte, affinity, 0.0);
        let h = if bb.y1 > bb.y0 { (bb.y1 - bb.y0) as f32 / s } else { spec.style.line_px() };
        let mid_y = oy + ((bb.y0 + bb.y1) * 0.5) as f32 / s;
        Some([ox + bb.x0 as f32 / s, mid_y, h])
    }

    /// Hit-tests unscrolled point `(x, y)` against node `i`'s laid-out text.
    fn hit_unscrolled(&self, i: usize, x: f32, y: f32) -> (usize, parley::Affinity) {
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_ref() else { return (0, parley::Affinity::Downstream) };
        let Some(laid) = n.text_layout.as_ref() else { return (0, parley::Affinity::Downstream) };
        let Some([ox, oy]) = self.text_unscrolled_origin(i) else { return (0, parley::Affinity::Downstream) };
        let s = self.env.scale;
        let hit = laid.view().hit((x - ox) * s, (y - oy) * s);
        let byte = n.elided.map_or(hit.index, |e| e.content(hit.index)).min(spec.content.len());
        (spec.content.floor_char_boundary(byte), hit.side)
    }

    /// Hit-tests unscrolled x on node `i`'s first (`first`) or last (`!first`) visual line.
    fn edge_hit_unscrolled(&self, i: usize, x: f32, first: bool) -> (usize, parley::Affinity) {
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_ref() else { return (0, parley::Affinity::Downstream) };
        let Some(laid) = n.text_layout.as_ref() else {
            return (if first { 0 } else { spec.content.len() }, parley::Affinity::Downstream);
        };
        let Some([ox, _]) = self.text_unscrolled_origin(i) else {
            return (if first { 0 } else { spec.content.len() }, parley::Affinity::Downstream);
        };
        let s = self.env.scale;
        let (idx, aff) = laid.view().edge_line_hit((x - ox) * s, first);
        let byte = n.elided.map_or(idx, |e| e.content(idx)).min(spec.content.len());
        (spec.content.floor_char_boundary(byte), aff)
    }

    /// Handles a keyboard navigation key while a `caret_browsing(true)` host holds focus.
    /// Never intercepts when a text input is focused.
    pub(in crate::runtime) fn caret_browse_key(&mut self, app: &dyn ViewDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
        if self.focused_input().is_some() {
            return false;
        }
        let Some(host) = self.caret_browsing_focused_host() else { return false };
        if !one.key_repeat && matches!(ev.key, crate::Key::Command('a' | 'c')) && self.selection_key(app, ev, one) {
            one.live_only();
            return true;
        }
        let Some((mov, shift)) = crate::edit::classify_caret_key(ev, cfg!(target_os = "macos"), false) else {
            return false;
        };
        let texts = self.caret_browsing_texts(host);
        if texts.is_empty() {
            return false;
        }

        // Resolve current anchor and focus inside `host`, initializing a collapsed caret at
        // `texts[0]` byte 0 if focus arrived without one (or the previous text left the tree).
        let (anchor_end, focus_pos, focus_node, focus_byte, has_selection) = match self
            .selection_holder_end()
            .and_then(|h| Some((h, self.focus_holder_end(h)?, self.edit.selectable.phase.selection()?)))
        {
            Some((h, EndRef::Mounted(fi), sel)) if self.caret_browsing_contains(host, fi) => {
                let pos = texts.iter().position(|&t| t == fi).unwrap_or(0);
                let content = self.content_of(fi);
                let fb = content.floor_char_boundary(sel.focus.min(content.len()));
                let has_sel = h != EndRef::Mounted(fi) || sel.anchor != sel.focus;
                (h, pos, fi, fb, has_sel)
            }
            _ => {
                let first = texts[0];
                let path = self.tree.nodes[first].path.clone();
                let content = self.content_of(first).to_owned();
                self.select_step(phase::Event::Select(phase::Selection::new(path, content, (0, 0), (0, 0))));
                (EndRef::Mounted(first), 0, first, 0, false)
            }
        };
        let affinity = self.edit.selectable.affinity;

        let (dest_node, dest_byte, dest_aff, next_goal_x) = match mov {
            CaretMove::Left | CaretMove::Right if !shift && has_selection => {
                let right = mov == CaretMove::Right;
                let (dn, db) = self.collapse_caret_selection(host, anchor_end, focus_node, focus_byte, right);
                (dn, db, parley::Affinity::Downstream, None)
            }
            CaretMove::Left | CaretMove::Right | CaretMove::WordLeft | CaretMove::WordRight => {
                let right = matches!(mov, CaretMove::Right | CaretMove::WordRight);
                let word = matches!(mov, CaretMove::WordLeft | CaretMove::WordRight);
                let (dn, db, da) = self.step_horizontal(&texts, focus_pos, focus_byte, affinity, right, word);
                (dn, db, da, None)
            }
            CaretMove::LineStart | CaretMove::LineEnd => {
                let end = mov == CaretMove::LineEnd;
                let (db, da) = self.step_line_edge(focus_node, focus_byte, affinity, end);
                (focus_node, db, da, None)
            }
            CaretMove::DocStart => (texts[0], 0, parley::Affinity::Downstream, None),
            CaretMove::DocEnd => {
                let last = texts.last().copied().unwrap_or(texts[0]);
                (last, self.content_of(last).len(), parley::Affinity::Downstream, None)
            }
            CaretMove::Up | CaretMove::Down => {
                let down = mov == CaretMove::Down;
                let gx = self
                    .edit
                    .selectable
                    .goal_x
                    .or_else(|| self.caret_unscrolled_pos(focus_node, focus_byte, affinity).map(|p| p[0]))
                    .unwrap_or_else(|| self.tree.nodes[focus_node].target[0]);
                let (dn, db, da) = self.step_vertical(&texts, focus_pos, focus_byte, affinity, down, gx);
                (dn, db, da, Some(gx))
            }
            CaretMove::PageUp | CaretMove::PageDown => {
                let down = mov == CaretMove::PageDown;
                let gx = self
                    .edit
                    .selectable
                    .goal_x
                    .or_else(|| self.caret_unscrolled_pos(focus_node, focus_byte, affinity).map(|p| p[0]))
                    .unwrap_or_else(|| self.tree.nodes[focus_node].target[0]);
                let (dn, db, da) = self.step_page(&texts, focus_pos, focus_byte, affinity, down, gx);
                (dn, db, da, Some(gx))
            }
        };

        let dest_path = self.tree.nodes[dest_node].path.clone();
        let dest_content = self.content_of(dest_node).to_owned();
        let dest_byte = dest_content.floor_char_boundary(dest_byte.min(dest_content.len()));
        if shift {
            let far = (EndRef::Mounted(dest_node) != anchor_end).then(|| phase::Far::new(dest_path, dest_content));
            self.select_step(phase::Event::SetFocus { focus: dest_byte, far });
        } else {
            self.select_step(phase::Event::Select(phase::Selection::new(
                dest_path,
                dest_content,
                (dest_byte, dest_byte),
                (dest_byte, dest_byte),
            )));
        }
        self.edit.selectable.goal_x = next_goal_x;
        self.edit.selectable.affinity = dest_aff;
        self.edit.caret_follow = true;
        one.live_only();
        true
    }

    /// Plain Left/Right collapse of a non-empty selection to its start or end edge.
    fn collapse_caret_selection(
        &self,
        host: usize,
        anchor_end: EndRef,
        focus_node: usize,
        focus_byte: usize,
        right: bool,
    ) -> (usize, usize) {
        let Some(sel) = self.edit.selectable.phase.selection() else {
            return (focus_node, focus_byte);
        };
        if anchor_end == EndRef::Mounted(focus_node) {
            let content = self.content_of(focus_node);
            let a = content.floor_char_boundary(sel.anchor.min(focus_byte).min(content.len()));
            let b = content.floor_char_boundary(sel.anchor.max(focus_byte).min(content.len()));
            let n = &self.tree.nodes[focus_node];
            let flow = n.text_layout.as_ref().map(TextLayout::view);
            let to = match (flow, n.elided) {
                (Some(l), Some(e)) => {
                    let da = e.range(a, a).0;
                    let db = e.range(b, b).0;
                    let df = e.range(focus_byte, focus_byte).0;
                    e.content(crate::edit::collapse_edge(Some(l), da, db, df, right)).min(content.len())
                }
                (l, None) => crate::edit::collapse_edge(l, a, b, focus_byte, right),
                (None, Some(_)) => if right { b } else { a },
            };
            return (focus_node, to);
        }
        let anchor_first = self.tree_order(anchor_end) < self.tree_order(EndRef::Mounted(focus_node));
        let forward = right == (self.tree.nodes[host].flow == crate::el::Flow::Ltr);
        let pick_focus = forward == anchor_first;
        if pick_focus {
            (focus_node, focus_byte)
        } else if let EndRef::Mounted(ai) = anchor_end {
            let content = self.content_of(ai);
            (ai, content.floor_char_boundary(sel.anchor.min(content.len())))
        } else {
            // Offscreen anchor (virtual rows): stay at focus node edge for Step 1.
            (focus_node, if forward { self.content_of(focus_node).len() } else { 0 })
        }
    }

    /// One horizontal character or word step, crossing mounted sibling texts at boundaries.
    fn step_horizontal(
        &self,
        texts: &[usize],
        focus_pos: usize,
        focus_byte: usize,
        affinity: parley::Affinity,
        right: bool,
        word: bool,
    ) -> (usize, usize, parley::Affinity) {
        let i = texts[focus_pos];
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_ref() else {
            return (i, focus_byte, affinity);
        };
        let content = spec.content.as_str();
        let pills = pill_ranges(spec);

        // Bidi/RTL visual stepping when the paragraph contains RTL runs, sharing `rune-edit`'s
        // `visual_in` + `space_ahead` rules identically.
        if let Some(laid) = n.text_layout.as_ref() {
            let flow = laid.view();
            let arrow = match (right, word) {
                (false, false) => CaretArrow::Left,
                (true, false) => CaretArrow::Right,
                (false, true) => CaretArrow::WordLeft,
                (true, true) => CaretArrow::WordRight,
            };
            let drawn = n.elided.map_or(focus_byte, |e| e.range(focus_byte, focus_byte).0);
            if let Some((landing, dir)) = crate::edit::visual_in(flow, drawn, affinity, arrow, false) {
                match landing {
                    CaretLanding::At(mut j, mut side) => {
                        if word && !cfg!(target_os = "macos") {
                            let step = if right { CaretArrow::Right } else { CaretArrow::Left };
                            while crate::edit::space_ahead(flow, j, side, right)
                                && let Some((CaretLanding::At(k, s), _)) = crate::edit::visual_in(flow, j, side, step, false)
                            {
                                (j, side) = (k, s);
                            }
                        }
                        let byte = n.elided.map_or(j, |e| e.content(j)).min(content.len());
                        let forward = byte > focus_byte;
                        let byte = snap_out_of_pill(&pills, byte, forward);
                        return (i, byte, side);
                    }
                    CaretLanding::Edge => {
                        let forward = right == (dir == crate::el::Flow::Ltr);
                        return self.step_logical(texts, focus_pos, focus_byte, forward, word, &pills);
                    }
                }
            }
        }

        self.step_logical(texts, focus_pos, focus_byte, right, word, &pills)
    }

    fn step_logical(
        &self,
        texts: &[usize],
        focus_pos: usize,
        focus_byte: usize,
        forward: bool,
        word: bool,
        pills: &[(usize, usize)],
    ) -> (usize, usize, parley::Affinity) {
        let i = texts[focus_pos];
        let content = self.content_of(i);
        if forward {
            if focus_byte < content.len() {
                let next = if word {
                    crate::edit::next_word_in(content, focus_byte, pills)
                } else {
                    crate::edit::next_char(content, focus_byte)
                };
                let next = snap_out_of_pill(pills, next, true);
                return (i, next, parley::Affinity::Downstream);
            }
            if focus_pos + 1 < texts.len() {
                let next_node = texts[focus_pos + 1];
                let next_byte = if word && cfg!(target_os = "macos")
                    && let Some(next_spec) = self.tree.nodes[next_node].text.as_ref()
                {
                    let next_pills = pill_ranges(next_spec);
                    crate::edit::next_word_in(&next_spec.content, 0, &next_pills)
                } else {
                    0
                };
                return (next_node, next_byte, parley::Affinity::Downstream);
            }
            return (i, content.len(), parley::Affinity::Downstream);
        }
        if focus_byte > 0 {
            let prev = if word {
                crate::edit::prev_word_in(content, focus_byte, pills)
            } else {
                crate::edit::prev_char(content, focus_byte)
            };
            let prev = snap_out_of_pill(pills, prev, false);
            return (i, prev, parley::Affinity::Downstream);
        }
        if focus_pos > 0 {
            let prev_node = texts[focus_pos - 1];
            let Some(prev_spec) = self.tree.nodes[prev_node].text.as_ref() else {
                return (prev_node, 0, parley::Affinity::Downstream);
            };
            let prev_len = prev_spec.content.len();
            let prev_byte = if word {
                let prev_pills = pill_ranges(prev_spec);
                crate::edit::prev_word_in(&prev_spec.content, prev_len, &prev_pills)
            } else {
                prev_len
            };
            return (prev_node, prev_byte, parley::Affinity::Downstream);
        }
        (i, 0, parley::Affinity::Downstream)
    }

    /// Visual line start or end inside `i`, preserving soft-wrapped end affinity.
    fn step_line_edge(&self, i: usize, byte: usize, affinity: parley::Affinity, end: bool) -> (usize, parley::Affinity) {
        let n = &self.tree.nodes[i];
        let content = self.content_of(i);
        if let Some(laid) = n.text_layout.as_ref() {
            let drawn = n.elided.map_or(byte, |e| e.range(byte, byte).0);
            let (edge, aff) = laid.view().line_edge_with_affinity(drawn, affinity, end);
            let mapped = n.elided.map_or(edge, |e| e.content(edge)).min(content.len());
            return (content.floor_char_boundary(mapped), aff);
        }
        let edge = if end {
            content[byte..].find('\n').map_or(content.len(), |k| byte + k)
        } else {
            content[..byte].rfind('\n').map_or(0, |k| k + 1)
        };
        (edge, parley::Affinity::Downstream)
    }

    /// Groups `texts` into vertical bands (so side-by-side table cells share a band) and finds the
    /// nearest text in the band strictly below (`down`) or above (`!down`) `cur_node`.
    fn adjacent_band_text(&self, texts: &[usize], cur_pos: usize, down: bool, goal_x: f32) -> Option<usize> {
        let cur = texts[cur_pos];
        let cur_r = self.tree.nodes[cur].target;
        let mut candidates: Vec<(usize, [f32; 4])> = texts
            .iter()
            .copied()
            .filter(|&j| j != cur)
            .filter_map(|j| {
                let r = self.tree.nodes[j].target;
                let ok = if down { r[1] >= cur_r[3] - 1.0 } else { r[3] <= cur_r[1] + 1.0 };
                ok.then_some((j, r))
            })
            .collect();
        if candidates.is_empty() {
            // Fall back to tree order if boxes overlap unexpectedly.
            return if down {
                texts.get(cur_pos + 1).copied()
            } else {
                cur_pos.checked_sub(1).and_then(|p| texts.get(p).copied())
            };
        }
        let band_edge = if down {
            candidates.iter().map(|(_, r)| r[1]).fold(f32::INFINITY, f32::min)
        } else {
            candidates.iter().map(|(_, r)| r[3]).fold(f32::NEG_INFINITY, f32::max)
        };
        candidates.retain(|(_, r)| if down { r[1] < band_edge + 2.0 } else { r[3] > band_edge - 2.0 });
        candidates
            .into_iter()
            .min_by(|a, b| horiz_dist(a.1[0], a.1[2], goal_x).total_cmp(&horiz_dist(b.1[0], b.1[2], goal_x)))
            .map(|(j, _)| j)
    }

    /// One visual line Up or Down with sticky `goal_x`, crossing mounted sibling texts.
    fn step_vertical(
        &self,
        texts: &[usize],
        focus_pos: usize,
        focus_byte: usize,
        affinity: parley::Affinity,
        down: bool,
        goal_x: f32,
    ) -> (usize, usize, parley::Affinity) {
        let i = texts[focus_pos];
        let n = &self.tree.nodes[i];
        let content = self.content_of(i);
        if let (Some(laid), Some([ox, _])) = (n.text_layout.as_ref(), self.text_unscrolled_origin(i)) {
            let s = self.env.scale;
            let drawn = n.elided.map_or(focus_byte, |e| e.range(focus_byte, focus_byte).0);
            if let Some((next_drawn, next_aff, _)) = laid.view().move_line_strict(drawn, affinity, down, Some((goal_x - ox) * s)) {
                let mapped = n.elided.map_or(next_drawn, |e| e.content(next_drawn)).min(content.len());
                let byte = content.floor_char_boundary(mapped);
                let pills = n.text.as_deref().map(pill_ranges).unwrap_or_default();
                return (i, snap_out_of_pill(&pills, byte, down), next_aff);
            }
        }
        if let Some(adj) = self.adjacent_band_text(texts, focus_pos, down, goal_x) {
            let (byte, aff) = self.edge_hit_unscrolled(adj, goal_x, down);
            let pills = self.tree.nodes[adj].text.as_deref().map(pill_ranges).unwrap_or_default();
            return (adj, snap_out_of_pill(&pills, byte, down), aff);
        }
        // Boundary of mounted document: clamp to start on Up, end on Down.
        (i, if down { content.len() } else { 0 }, parley::Affinity::Downstream)
    }

    /// Viewport-height PageUp or PageDown step with sticky `goal_x`.
    fn step_page(
        &self,
        texts: &[usize],
        focus_pos: usize,
        focus_byte: usize,
        affinity: parley::Affinity,
        down: bool,
        goal_x: f32,
    ) -> (usize, usize, parley::Affinity) {
        let i = texts[focus_pos];
        let page_h = std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent)
            .find(|&p| self.tree.nodes[p].scroll.is_some() && self.tree.nodes[p].scroll_axes.has(false))
            .map(|v| (self.tree.nodes[v].target[3] - self.tree.nodes[v].target[1]).max(1.0));
        let last_text = texts.last().copied().unwrap_or(texts[0]);
        let Some(page_h) = page_h else {
            return if down {
                (last_text, self.content_of(last_text).len(), parley::Affinity::Downstream)
            } else {
                (texts[0], 0, parley::Affinity::Downstream)
            };
        };
        let cur_y = self
            .caret_unscrolled_pos(i, focus_byte, affinity)
            .map_or_else(|| (self.tree.nodes[i].target[1] + self.tree.nodes[i].target[3]) * 0.5, |p| p[1]);
        let target_y = cur_y + if down { page_h } else { -page_h };

        // Pick the mounted text spanning `target_y`, else the nearest text at or past `target_y`.
        let covering: Vec<usize> = texts
            .iter()
            .copied()
            .filter(|&j| {
                let r = self.tree.nodes[j].target;
                target_y >= r[1] && target_y <= r[3]
            })
            .collect();
        let nearest_x = |iter: Vec<usize>, fallback: usize| {
            iter.into_iter()
                .min_by(|&a, &b| {
                    let (ra, rb) = (self.tree.nodes[a].target, self.tree.nodes[b].target);
                    horiz_dist(ra[0], ra[2], goal_x).total_cmp(&horiz_dist(rb[0], rb[2], goal_x))
                })
                .unwrap_or(fallback)
        };
        let chosen = if !covering.is_empty() {
            nearest_x(covering, i)
        } else if down {
            let below: Vec<usize> = texts.iter().copied().filter(|&j| self.tree.nodes[j].target[1] > target_y).collect();
            if let Some(&first_below) = below.first() {
                let top = self.tree.nodes[first_below].target[1];
                let band: Vec<usize> = below.into_iter().filter(|&j| self.tree.nodes[j].target[1] < top + 2.0).collect();
                nearest_x(band, first_below)
            } else {
                last_text
            }
        } else {
            let above: Vec<usize> = texts.iter().copied().filter(|&j| self.tree.nodes[j].target[3] < target_y).collect();
            if let Some(&last_above) = above.last() {
                let bottom = self.tree.nodes[last_above].target[3];
                let band: Vec<usize> = above.into_iter().filter(|&j| self.tree.nodes[j].target[3] > bottom - 2.0).collect();
                nearest_x(band, last_above)
            } else {
                texts[0]
            }
        };

        let r = self.tree.nodes[chosen].target;
        let (byte, aff) = if target_y < r[1] {
            self.edge_hit_unscrolled(chosen, goal_x, true)
        } else if target_y > r[3] {
            self.edge_hit_unscrolled(chosen, goal_x, false)
        } else {
            self.hit_unscrolled(chosen, goal_x, target_y)
        };
        let pills = self.tree.nodes[chosen].text.as_deref().map(pill_ranges).unwrap_or_default();
        let byte = snap_out_of_pill(&pills, byte, down);
        if chosen == i && byte == focus_byte {
            // Ensure PageUp/PageDown always makes at least one visual line of progress unless
            // already at the very start/end of the mounted document.
            return self.step_vertical(texts, focus_pos, focus_byte, affinity, down, goal_x);
        }
        (chosen, byte, aff)
    }
}

#[cfg(test)]
#[path = "caret_browse_tests.rs"]
mod tests;
