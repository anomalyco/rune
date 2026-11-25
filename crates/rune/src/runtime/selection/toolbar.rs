//! What a toolbar floating over a text selection needs from the selection: which way it was
//! made ([`TextSelection::backward`](crate::TextSelection::backward)), and controls a press on
//! which leaves the selection and the keyboard focus as they are
//! ([`El::keeps_text_selection`]). Where its line boxes are is
//! [`Cx::selection_rects`](crate::Cx::selection_rects).
use super::*;

/// What a press on an element and its descendants leaves as it is.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OnPress {
    /// The focused field ([`El::preserve_focus_on_press`]).
    pub(crate) focus: bool,
    /// The text selection and the focus ([`El::keeps_text_selection`]).
    pub(crate) selection: bool,
}

impl El {
    /// A press on this element or anything inside it leaves the
    /// [`El::selectable`] text selection and the keyboard focus as they are, as a web page's
    /// `preventDefault` on `mousedown` does: a Quote or Copy button floating over a selection
    /// can read it when pressed ([`Tx::text_selection`]), and a drag that starts on it doesn't
    /// start a new one. Focusable controls inside don't take focus either. The press still
    /// reaches `on_press` and the other handlers.
    pub fn keeps_text_selection(mut self) -> Self {
        self.cold_mut().focus.preserve_on_press.selection = true;
        self
    }
}

impl RuntimeCore {
    /// What a press on node `i` leaves as it is, from it and its ancestors.
    fn on_press_keeps(&self, i: usize) -> OnPress {
        std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent)
            .take_while(|&p| self.allowed(p))
            .map(|p| self.tree.nodes[p].focus.preserve_on_press)
            .fold(OnPress::default(), |a, b| OnPress { focus: a.focus || b.focus, selection: a.selection || b.selection })
    }

    /// A press on node `i` keeps the text selection (and so the focus too).
    pub(in crate::runtime) fn press_keeps_selection(&self, i: usize) -> bool {
        self.on_press_keeps(i).selection
    }

    /// A press on node `i` leaves the focused field focused.
    pub(in crate::runtime) fn press_keeps_focus(&self, i: usize) -> bool {
        let keeps = self.on_press_keeps(i);
        keeps.focus || keeps.selection
    }

    /// First selectable text of the list item whose non-selectable marker covers `(x, y)`,
    /// if no selectable text or overlying hit target sits above it. Works in both LTR and RTL
    /// lists and nested lists because it checks the marker node's own drawn box.
    pub(in crate::runtime) fn marker_anchor_at(&self, x: f32, y: f32) -> Option<usize> {
        let top_hit = self.top_hit(x, y);
        let nodes = &self.tree.nodes;
        for li in (0..nodes.len()).rev() {
            let n = &nodes[li];
            let is_item = n.role == "listitem"
                || (n.role == "checkbox"
                    && !n.has_primary_handler()
                    && n.parent.is_some_and(|p| nodes[p].role == "list"));
            if !is_item || !self.allowed(li) || self.subtree_exiting(li) {
                continue;
            }
            if let Some(th) = top_hit
                && !self.node_holds(th, li)
            {
                continue;
            }
            let marker = li + 1;
            if marker >= nodes.len()
                || nodes[marker].parent != Some(li)
                || !(nodes[marker].a11y.hidden || nodes[marker].role == "list-marker")
            {
                continue;
            }
            if !self.allowed(marker)
                || self.subtree_exiting(marker)
                || self.paint.drawn.get(marker).is_none_or(|d| d[2] <= 0.0 || d[3] <= 0.0)
                || !self.node_contains(marker, x, y)
            {
                continue;
            }
            let first = (li + 1..nodes.len())
                .take_while(|&j| self.node_holds(li, j))
                .find(|&j| !self.node_holds(marker, j) && self.is_selectable(j) && self.allowed(j) && !self.subtree_exiting(j));
            if first.is_some() {
                return first;
            }
        }
        None
    }

    /// A primary press on `target` (None: on nothing). On selectable text it starts a selection
    /// (one click places it — or arms waiting for drag slop on an inline link — two select a word,
    /// three a paragraph, and Shift extends the current one into another text of its group).
    /// Anywhere else it clears the selection.
    pub(in crate::runtime) fn selection_down(&mut self, target: Option<usize>, x: f32, y: f32) {
        use super::selectable::{Unit, phase, virtual_selection::EndRef, with_trailing_space};
        let Some(i) = target.filter(|&i| self.is_selectable(i)) else {
            self.clear_text_selection();
            self.edit.selectable.presses.clear();
            return;
        };
        let on_link = self.link_at(i, x, y).is_some();
        let Some(at) = self.text_index_at(i, x, y) else { return };
        let path = self.tree.nodes[i].path.clone();
        let content = self.content_of(i).to_owned();
        let now = self.now_clock();
        let clicks = self.edit.selectable.presses.press(&path, now, (x, y), self.double_click_interval).min(3);
        let i_end = EndRef::Mounted(i);
        let holder = self.selection_holder_end().filter(|&h| h == i_end || self.end_same_group(h, i_end));
        match holder {
            // Keep the anchor; the focus follows the pointer from here.
            Some(h) if clicks == 1 && self.edit.selectable.shift => {
                self.select_step(phase::Event::Extend { focus: at, far: (h != i_end).then(|| phase::Far::new(path, content)) });
            }
            // A first press on an inline link stays unarmed until pointer motion crosses drag slop,
            // so clicks and trembling clicks still open the link while drags start text selection.
            _ if clicks == 1 && on_link => {
                self.select_step(phase::Event::Arm { path, content, at, wait_slop: true });
            }
            _ => {
                let unit = match clicks { 1 => Unit::Char, 2 => Unit::Word, _ => Unit::Paragraph };
                let origin = self.unit_in(i, at, unit);
                let origin = if unit == Unit::Word { with_trailing_space(&content, origin) } else { origin };
                self.select_step(phase::Event::Press { path, content, unit, origin });
            }
        }
    }

    /// Primary press on a list item's non-selectable marker: single click arms a drag anchored at
    /// byte 0 of `i` without selecting or moving any caret; Shift+click extends to byte 0; double-
    /// and triple-click select the first word or paragraph of `i` (never the marker itself).
    fn selection_down_marker(&mut self, i: usize, x: f32, y: f32) {
        use super::selectable::{Unit, phase, virtual_selection::EndRef, with_trailing_space};
        let path = self.tree.nodes[i].path.clone();
        let content = self.content_of(i).to_owned();
        let now = self.now_clock();
        let clicks = self.edit.selectable.presses.press(&path, now, (x, y), self.double_click_interval).min(3);
        let i_end = EndRef::Mounted(i);
        let holder = self.selection_holder_end().filter(|&h| h == i_end || self.end_same_group(h, i_end));
        match holder {
            Some(h) if clicks == 1 && self.edit.selectable.shift => {
                self.select_step(phase::Event::Extend {
                    focus: 0,
                    far: (h != i_end).then(|| phase::Far::new(path, content)),
                });
            }
            _ if clicks == 1 => {
                self.select_step(phase::Event::Arm { path, content, at: 0, wait_slop: false });
            }
            _ => {
                let unit = if clicks == 2 { Unit::Word } else { Unit::Paragraph };
                let origin = self.unit_in(i, 0, unit);
                let origin = if unit == Unit::Word { with_trailing_space(&content, origin) } else { origin };
                self.select_step(phase::Event::Press { path, content, unit, origin });
            }
        }
    }

    /// Whether an armed marker press sits inside the currently focused caret-browsing host, so a
    /// bare click on its list marker neither blurs nor moves its caret.
    pub(in crate::runtime) fn armed_marker_in_focused_caret_host(&self) -> bool {
        let Some(host) = self.caret_browsing_focused_host() else { return false };
        let Some((sel, _, armed)) = self.edit.selectable.phase.drag_selection() else { return false };
        armed == Some(false) && self.live(&sel.path).is_some_and(|i| self.caret_browsing_host_of(i) == Some(host))
    }

    /// The selection side of a primary press on `target` at (x, y): it starts or clears a
    /// selection, unless the press is on an [`El::keeps_text_selection`] element.
    pub(in crate::runtime) fn press_selection(&mut self, target: Option<usize>, x: f32, y: f32) {
        if target.is_some_and(|t| self.press_keeps_selection(t)) { return; }
        if let Some(i) = self.selectable_at(x, y) {
            self.selection_down(Some(i), x, y);
        } else if let Some(i) = self.marker_anchor_at(x, y) {
            self.selection_down_marker(i, x, y);
        } else {
            self.selection_down(None, x, y);
        }
    }

    /// The selection's focus comes before its anchor.
    pub(in crate::runtime) fn selection_backward(&self) -> bool {
        let Some(h) = self.selection_holder_end() else { return false };
        match self.focus_holder_end(h) {
            Some(f) if f != h => self.tree_order(f) < self.tree_order(h),
            Some(_) => self.edit.selectable.phase.selection().is_some_and(|s| s.focus < s.anchor),
            None => false,
        }
    }
}
