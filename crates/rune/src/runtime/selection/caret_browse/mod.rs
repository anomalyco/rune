//! Opt-in keyboard caret browsing on mounted `El::selectable(true)` text,
//! `El::selection_group(true)` containers, and `El::virtual_rows` / `El::virtualize` lists
//! (`El::caret_browsing(true)`).
//!
//! Follows Chrome's caret browsing behaviour and shares `rune-edit`'s grapheme, word, bidi,
//! selection-collapse and platform modifier rules so read-only text fields and caret-browsing
//! selectable documents move identically. Unmounted virtual-row transitions are driven by the
//! explicit [`pending::State`] state machine.

pub(crate) mod pending;
mod steps;
mod virtual_nav;

use super::*;
use crate::edit::CaretMove;
use crate::runtime::selectable::phase;
use crate::runtime::selectable::virtual_selection::EndRef;

/// Distance from `x` outside horizontal span `[x0, x1]`.
pub(super) fn horiz_dist(x0: f32, x1: f32, x: f32) -> f32 {
    (x0 - x).max(x - x1).max(0.0)
}

/// Non-bare pill byte ranges on `spec` (including the FSI/LRI/RLI..PDI isolate marks
/// `apply_isolates` wraps around each pill), matching `TextEdit`'s atomic pill chips.
pub(super) fn pill_ranges(spec: &crate::el::TextSpec) -> Vec<(usize, usize)> {
    let s = spec.content_str();
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
pub(super) fn snap_out_of_pill(pills: &[(usize, usize)], byte: usize, forward: bool) -> usize {
    for &(a, b) in pills {
        if a < byte && byte < b {
            return if forward { b } else { a };
        }
    }
    byte
}

pub(super) enum StepOut {
    Mounted(usize, usize, parley::Affinity),
    Seek(pending::Target),
}

impl RuntimeCore {
    /// Whether node `n` is an opt-in caret-browsing host: a non-input `El::caret_browsing(true)`
    /// element that has not opted out of focus and is either a `selection_group` or selectable,
    /// non-redacted text, outside an `El::selection_enabled(false)` subtree.
    #[inline]
    pub(crate) fn is_caret_browsing_node(&self, n: &Node) -> bool {
        !n.focus.opt_out
            && n.focus.caret_browsing
            && !n.selection_off
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

    /// Whether an `EndRef` (mounted or offscreen pinned virtual row) belongs to `host`.
    fn caret_browsing_end_in(&self, host: usize, end: EndRef) -> bool {
        match end {
            EndRef::Mounted(j) => self.caret_browsing_contains(host, j),
            EndRef::Offscreen(..) => {
                self.tree.nodes[host].selection_group
                    && self.end_group_path(end) == Some(self.tree.nodes[host].path.as_str())
            }
        }
    }

    /// Whether the current selection already lives inside caret-browsing host `host`.
    pub(in crate::runtime) fn caret_browsing_selection_in(&self, host: usize) -> bool {
        let Some(h) = self.selection_holder_end() else { return false };
        let Some(f) = self.focus_holder_end(h) else { return false };
        self.caret_browsing_end_in(host, h) && self.caret_browsing_end_in(host, f)
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
        let content = nodes[focus_i].text.as_ref()?.content_str();
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
    /// focused caret-browsing host, for AccessKit and the web accessibility mirror. When the
    /// selection's anchor is pinned in an offscreen virtual row, projects the anchor onto the
    /// first or last mounted selectable text edge in `host` so every AccessKit endpoint remains
    /// a live `Role::TextRun`.
    pub(crate) fn caret_browse_endpoints(&self) -> Option<(usize, usize, usize, usize, usize)> {
        let host = self.caret_browsing_focused_host()?;
        let sel = self.edit.selectable.phase.selection()?;
        let h = self.selection_holder_end()?;
        let EndRef::Mounted(fi) = self.focus_holder_end(h)? else { return None };
        if !self.caret_browsing_contains(host, fi) {
            return None;
        }
        let f_str = self.content_of(fi);
        let f = f_str.floor_char_boundary(sel.focus.min(f_str.len()));
        let (ai, a) = match h {
            EndRef::Mounted(ai) => {
                if !self.caret_browsing_contains(host, ai) {
                    return None;
                }
                let a_str = self.content_of(ai);
                (ai, a_str.floor_char_boundary(sel.anchor.min(a_str.len())))
            }
            EndRef::Offscreen(..) => {
                if !self.caret_browsing_end_in(host, h) {
                    return None;
                }
                let texts = self.caret_browsing_texts(host);
                let first = texts.first().copied()?;
                let last = texts.last().copied().unwrap_or(first);
                if self.tree_order(h) < self.tree_order(EndRef::Mounted(fi)) {
                    (first, 0)
                } else {
                    (last, self.content_of(last).len())
                }
            }
        };
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
        let spec = n.text.as_ref()?;
        let t = sticky::anchored_target(nodes, &self.scroll.scrolls, i, self.viewport);
        let y = t[1] + self.stack_row_dy(nodes, i);
        if let Some(pt) = spec.positioned() {
            let rect = [t[0], y, t[2], y + (t[3] - t[1])];
            let tf = super::super::positioned::PositionedTransform::from_window_box(n, pt, rect, 1.0);
            let c = pt.caret(byte)?;
            return Some([tf.ox + c[0] * tf.sx, tf.oy + c[1] * tf.sy, 1.5, (c[3] * tf.sy).max(1.0)]);
        }
        let laid = n.text_layout.as_ref()?;
        let flow = laid.view();
        let s = self.env.scale;
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
    pub(super) fn text_unscrolled_origin(&self, i: usize) -> Option<[f32; 2]> {
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
    pub(super) fn caret_unscrolled_pos(&self, i: usize, byte: usize, affinity: parley::Affinity) -> Option<[f32; 3]> {
        let n = &self.tree.nodes[i];
        let spec = n.text.as_ref()?;
        if let Some(pt) = spec.positioned() {
            let tf = super::super::positioned::PositionedTransform::from_window_box(n, pt, n.target, 1.0);
            let c = pt.caret(byte)?;
            let h = (c[3] * tf.sy).max(1.0);
            return Some([tf.ox + c[0] * tf.sx, tf.oy + (c[1] + c[3] * 0.5) * tf.sy, h]);
        }
        let laid = n.text_layout.as_ref()?;
        let [ox, oy] = self.text_unscrolled_origin(i)?;
        let s = self.env.scale;
        let drawn_byte = n.elided.map_or(byte, |e| e.range(byte, byte).0);
        let bb = laid.view().caret(drawn_byte, affinity, 0.0);
        let h = if bb.y1 > bb.y0 { (bb.y1 - bb.y0) as f32 / s } else { spec.style.line_px() };
        let mid_y = oy + ((bb.y0 + bb.y1) * 0.5) as f32 / s;
        Some([ox + bb.x0 as f32 / s, mid_y, h])
    }

    /// Hit-tests unscrolled point `(x, y)` against node `i`'s laid-out text.
    pub(super) fn hit_unscrolled(&self, i: usize, x: f32, y: f32) -> (usize, parley::Affinity) {
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_ref() else { return (0, parley::Affinity::Downstream) };
        if let Some(pt) = spec.positioned() {
            let tf = super::super::positioned::PositionedTransform::from_window_box(n, pt, n.target, 1.0);
            let Some([sx, sy]) = tf.to_source(x, y) else { return (0, parley::Affinity::Downstream) };
            return (pt.hit(sx, sy).0, parley::Affinity::Downstream);
        }
        let Some(laid) = n.text_layout.as_ref() else { return (0, parley::Affinity::Downstream) };
        let Some([ox, oy]) = self.text_unscrolled_origin(i) else { return (0, parley::Affinity::Downstream) };
        let s = self.env.scale;
        let hit = laid.view().hit((x - ox) * s, (y - oy) * s);
        let byte = n.elided.map_or(hit.index, |e| e.content(hit.index)).min(spec.content.len());
        (spec.content.floor_char_boundary(byte), hit.side)
    }

    /// Hit-tests unscrolled x on node `i`'s first (`first`) or last (`!first`) visual line.
    pub(super) fn edge_hit_unscrolled(&self, i: usize, x: f32, first: bool) -> (usize, parley::Affinity) {
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_ref() else { return (0, parley::Affinity::Downstream) };
        if let Some(pt) = spec.positioned() {
            let tf = super::super::positioned::PositionedTransform::from_window_box(n, pt, n.target, 1.0);
            let sx = if tf.sx > 1e-6 { (x - tf.ox) / tf.sx } else { 0.0 };
            return (pt.edge_line_hit(sx, first), parley::Affinity::Downstream);
        }
        let Some(laid) = n.text_layout.as_ref() else {
            return (if first { 0 } else { spec.content_str().len() }, parley::Affinity::Downstream);
        };
        let Some([ox, _]) = self.text_unscrolled_origin(i) else {
            return (if first { 0 } else { spec.content.len() }, parley::Affinity::Downstream);
        };
        let s = self.env.scale;
        let (idx, aff) = laid.view().edge_line_hit((x - ox) * s, first);
        let byte = n.elided.map_or(idx, |e| e.content(idx)).min(spec.content.len());
        (spec.content.floor_char_boundary(byte), aff)
    }

    /// Mounted focus `(node, byte, affinity)` of an active selectable text selection, for
    /// `follow_caret` to scroll the moving selection end into view without exposing a caret.
    pub(in crate::runtime) fn selectable_focus_mounted(&self) -> Option<(usize, usize, parley::Affinity)> {
        let sel = self.edit.selectable.phase.selection()?;
        let h = self.selection_holder_end()?;
        let EndRef::Mounted(fi) = self.focus_holder_end(h)? else { return None };
        let content = self.content_of(fi);
        let byte = content.floor_char_boundary(sel.focus.min(content.len()));
        Some((fi, byte, self.edit.selectable.affinity))
    }

    /// Handles a keyboard navigation key while a `caret_browsing(true)` host holds focus.
    /// Never intercepts when a text input is focused.
    pub(in crate::runtime) fn caret_browse_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
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
        self.move_selection_in_host(app, host, mov, shift, pending::Scope::FocusedCaretHost, one)
    }

    /// Extends or shrinks an active selection on plain `El::selectable(true)` text (or a
    /// `selection_group`) with `Shift` + navigation keys (arrows, word steps, Home/End,
    /// PageUp/PageDown), matching Chrome's page-selection behaviour without requiring
    /// `El::caret_browsing(true)`. Leaves keys untouched when no selection is active or when a
    /// text input holds focus.
    pub(in crate::runtime) fn selection_extend_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
        if self.focused_input().is_some() {
            return false;
        }
        let Some((mov, true)) = crate::edit::classify_caret_key(ev, cfg!(target_os = "macos"), false) else {
            return false;
        };
        if !self.has_nonempty_selection() && !self.edit.selectable.phase.keyboard_extended() {
            return false;
        }
        let Some(h) = self.selection_holder_end() else { return false };
        if self.focus_holder_end(h).is_none() {
            return false;
        }
        let host = match self.end_group_path(h) {
            Some(gp) => match self.live(gp) {
                Some(g) => g,
                None => return false,
            },
            None => match h {
                EndRef::Mounted(i) if self.is_selectable(i) && !self.subtree_exiting(i) && self.allowed(i) => i,
                _ => return false,
            },
        };
        self.move_selection_in_host(app, host, mov, true, pending::Scope::ActiveSelection, one)
    }

    fn move_selection_in_host(
        &mut self,
        app: &mut dyn AppDriver,
        host: usize,
        mov: CaretMove,
        shift: bool,
        scope: pending::Scope,
        one: &mut Tx,
    ) -> bool {
        self.cancel_pending_caret();

        let texts = self.caret_browsing_texts(host);
        if texts.is_empty() && self.host_virtual_lists(host).is_empty() {
            return false;
        }

        // If the selection's focus itself scrolled offscreen (e.g. via mouse wheel), drive a
        // virtual seek directly from its pinned row identity.
        if let Some((h, EndRef::Offscreen(vi, ti), sel)) = self
            .selection_holder_end()
            .and_then(|h| Some((h, self.focus_holder_end(h)?, self.edit.selectable.phase.selection()?)))
            && self.caret_browsing_end_in(host, h)
            && self.caret_browsing_end_in(host, EndRef::Offscreen(vi, ti))
        {
            let focus_byte = sel.focus;
            let anchor_byte = sel.anchor;
            let has_sel = h != EndRef::Offscreen(vi, ti) || anchor_byte != focus_byte;
            if !shift && has_sel && matches!(mov, CaretMove::Left | CaretMove::Right) {
                let right = mov == CaretMove::Right;
                let anchor_first = self.tree_order(h) < self.tree_order(EndRef::Offscreen(vi, ti));
                let forward = right == (self.tree.nodes[host].flow == crate::el::Flow::Ltr);
                let pick_focus = forward == anchor_first;
                let target_end = if pick_focus { EndRef::Offscreen(vi, ti) } else { h };
                let target_off = if pick_focus { focus_byte } else { anchor_byte };
                match target_end {
                    EndRef::Mounted(mi) => {
                        let p = self.tree.nodes[mi].path.clone();
                        let c = self.content_of(mi).to_owned();
                        let b = c.floor_char_boundary(target_off.min(c.len()));
                        self.select_step(phase::Event::Select(phase::Selection::new(p, c, (b, b), (b, b))));
                        self.edit.caret_follow = true;
                        one.live_only();
                        return true;
                    }
                    EndRef::Offscreen(tvi, tti) => {
                        if let Some(target) = self.virtual_offscreen_collapse_target(host, tvi, tti, target_off) {
                            self.request_and_drive_pending_caret(app, target, scope);
                            one.live_only();
                            return true;
                        }
                    }
                }
            }
            if let Some(target) = self.virtual_offscreen_focus_target(host, &texts, (vi, ti), focus_byte, mov, shift) {
                self.request_and_drive_pending_caret(app, target, scope);
                one.live_only();
                return true;
            }
            return false;
        }

        if texts.is_empty() {
            if let Some(target) = self.virtual_doc_edge_target(host, &texts, matches!(mov, CaretMove::DocEnd), shift) {
                self.request_and_drive_pending_caret(app, target, scope);
                one.live_only();
                return true;
            }
            return false;
        }

        // Resolve current anchor and focus inside `host`. Caret browsing initializes a collapsed
        // caret at `texts[0]` byte 0 if focus arrived without one; plain selection extension only
        // steps an existing selection.
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
            _ if scope == pending::Scope::FocusedCaretHost => {
                let first = texts[0];
                let path = self.tree.nodes[first].path.clone();
                let content = self.content_of(first).to_owned();
                self.select_step(phase::Event::Select(phase::Selection::new(path, content, (0, 0), (0, 0))));
                (EndRef::Mounted(first), 0, first, 0, false)
            }
            _ => return false,
        };
        let affinity = self.edit.selectable.affinity;

        let (dest_node, dest_byte, dest_aff, next_goal_x) = match mov {
            CaretMove::Left | CaretMove::Right if !shift && has_selection => {
                let right = mov == CaretMove::Right;
                match self.collapse_caret_selection(host, anchor_end, focus_node, focus_byte, right) {
                    StepOut::Mounted(dn, db, da) => (dn, db, da, None),
                    StepOut::Seek(target) => {
                        self.request_and_drive_pending_caret(app, target, scope);
                        one.live_only();
                        return true;
                    }
                }
            }
            CaretMove::Left | CaretMove::Right | CaretMove::WordLeft | CaretMove::WordRight => {
                let right = matches!(mov, CaretMove::Right | CaretMove::WordRight);
                let word = matches!(mov, CaretMove::WordLeft | CaretMove::WordRight);
                match self.step_horizontal(host, &texts, focus_pos, focus_byte, affinity, right, word, shift) {
                    StepOut::Mounted(dn, db, da) => (dn, db, da, None),
                    StepOut::Seek(target) => {
                        self.request_and_drive_pending_caret(app, target, scope);
                        one.live_only();
                        return true;
                    }
                }
            }
            CaretMove::LineStart | CaretMove::LineEnd => {
                let end = mov == CaretMove::LineEnd;
                let (db, da) = self.step_line_edge(focus_node, focus_byte, affinity, end);
                (focus_node, db, da, None)
            }
            CaretMove::DocStart => {
                if let Some(target) = self.virtual_doc_edge_target(host, &texts, false, shift) {
                    self.request_and_drive_pending_caret(app, target, scope);
                    one.live_only();
                    return true;
                }
                (texts[0], 0, parley::Affinity::Downstream, None)
            }
            CaretMove::DocEnd => {
                if let Some(target) = self.virtual_doc_edge_target(host, &texts, true, shift) {
                    self.request_and_drive_pending_caret(app, target, scope);
                    one.live_only();
                    return true;
                }
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
                match self.step_vertical(host, &texts, focus_pos, focus_byte, affinity, down, shift, gx) {
                    StepOut::Mounted(dn, db, da) => (dn, db, da, Some(gx)),
                    StepOut::Seek(target) => {
                        self.request_and_drive_pending_caret(app, target, scope);
                        one.live_only();
                        return true;
                    }
                }
            }
            CaretMove::PageUp | CaretMove::PageDown => {
                let down = mov == CaretMove::PageDown;
                let gx = self
                    .edit
                    .selectable
                    .goal_x
                    .or_else(|| self.caret_unscrolled_pos(focus_node, focus_byte, affinity).map(|p| p[0]))
                    .unwrap_or_else(|| self.tree.nodes[focus_node].target[0]);
                if let Some(target) = self.virtual_page_target(host, &texts, focus_node, focus_byte, affinity, down, shift, gx) {
                    self.request_and_drive_pending_caret(app, target, scope);
                    one.live_only();
                    return true;
                }
                match self.step_page(host, &texts, focus_pos, focus_byte, affinity, down, shift, gx) {
                    StepOut::Mounted(dn, db, da) => (dn, db, da, Some(gx)),
                    StepOut::Seek(target) => {
                        self.request_and_drive_pending_caret(app, target, scope);
                        one.live_only();
                        return true;
                    }
                }
            }
        };

        let dest_path = self.tree.nodes[dest_node].path.clone();
        let dest_content = self.content_of(dest_node).to_owned();
        let dest_byte = dest_content.floor_char_boundary(dest_byte.min(dest_content.len()));
        if shift {
            let far = (EndRef::Mounted(dest_node) != anchor_end).then(|| phase::Far::new(dest_path, dest_content));
            self.select_step(phase::Event::SetFocus { focus: dest_byte, far });
            #[cfg(any(target_os = "linux", test))]
            if self.has_nonempty_selection() {
                self.primary_selection_changed();
            }
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

}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod virtual_tests;