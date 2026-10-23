//! Focus is keyed retained state. Platform AX can read the same flags as the inspector.
use super::*;
use crate::edit::Key;

fn sync_input_focus<S: 'static>(nodes: &mut [Node], state: &mut S, path: Option<&str>) -> bool {
    let mut changed = false;
    for n in nodes {
        if let Some(input) = &mut n.input {
            let focused = path == Some(n.path.as_str());
            if let Some(access) = &input.access {
                let edit = access(state);
                if input.edit.is_secure() && !edit.is_secure() {
                    edit.set_secure(true);
                    changed = true;
                }
                if edit.focused != focused || (!focused && edit.preedit.is_some()) {
                    edit.focused = focused;
                    if !focused { edit.clear_preedit(); }
                    edit.break_history_group();
                    changed = true;
                }
                input.edit = edit.clone();
            } else {
                input.edit.focused = focused;
            }
        }
    }
    changed
}

impl<S: 'static> Runtime<S> {
    pub(super) fn active_modal(&self) -> Option<usize> {
        self.modal_index
    }
    pub(super) fn refresh_modal_index(&mut self) {
        self.modal_index = self.nodes.iter().enumerate().rev()
            .find(|(i, n)| n.modal.is_some() && !self.subtree_exiting(*i)).map(|(i, _)| i);
        self.modal_members = (0..self.nodes.len()).map(|i| self.modal_index.is_none_or(|m| i >= m)).collect();
        // Portals are root siblings. Follow their anchor ownership, including chained
        // popovers, rather than requiring their layout parent to be the modal.
        loop {
            let mut changed = false;
            for (i, n) in self.nodes.iter().enumerate() {
                if self.modal_members[i] || self.subtree_exiting(i) { continue; }
                let parent_inside = n.parent.is_some_and(|p| self.modal_members[p]);
                let anchor_inside = n.above.as_ref().is_some_and(|a| self.nodes.iter().enumerate()
                    .any(|(j, anchor)| anchor.key.as_deref() == Some(&a.key) && self.modal_members[j]));
                if parent_inside || anchor_inside { self.modal_members[i] = true; changed = true; }
            }
            if !changed { break; }
        }
    }
    pub(super) fn allowed(&self, i: usize) -> bool {
        self.active_modal().is_none() || self.modal_members.get(i).copied().unwrap_or(false)
    }
    pub(super) fn can_focus(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        let mut parent = n.parent;
        while let Some(p) = parent {
            if self.nodes[p].vis.opacity <= 0.0 { return false; }
            parent = self.nodes[p].parent;
        }
        (n.focus.enabled || n.input.as_ref().is_some_and(|s| s.access.is_some()))
            && self.allowed(i) && !self.subtree_exiting(i)
            && n.vis.opacity > 0.0 && n.local.w > 0.0 && n.local.h > 0.0
    }
    fn group(&self, i: usize) -> Option<usize> {
        let mut p = self.nodes[i].parent;
        while let Some(i) = p {
            if self.nodes[i].focus.group { return Some(i); }
            p = self.nodes[i].parent;
        }
        None
    }
    pub(super) fn focus_ring_visible(&self, mut i: usize) -> bool {
        if !self.keyboard_focus { return false; }
        loop {
            if self.focused.as_deref() == Some(&self.nodes[i].path) { return true; }
            // A nested focusable owns its ring, never inherit another control's focus.
            if self.nodes[i].focus.enabled { return false; }
            let Some(p) = self.nodes[i].parent else { return false };
            i = p;
        }
    }
    pub(super) fn set_focus(&mut self, path: Option<String>, keyboard: bool) -> bool {
        if self.focused != path { self.ime_area = None; self.ime_reset = true; }
        let changed = sync_input_focus(&mut self.nodes, &mut self.state, path.as_deref());
        let mut changed = changed;
        for (p, input) in &mut self.virtual_inputs {
            if let Some(access) = &input.access {
                let edit = access(&mut self.state);
                if input.edit.is_secure() && !edit.is_secure() {
                    edit.set_secure(true);
                    changed = true;
                }
                let focused = path.as_deref() == Some(p.as_str());
                if edit.focused != focused || (!focused && edit.preedit.is_some()) {
                    edit.focused = focused;
                    if !focused { edit.clear_preedit(); }
                    edit.break_history_group();
                    changed = true;
                }
                input.edit = edit.clone();
            }
        }
        self.focused = path;
        self.keyboard_focus = keyboard;
        changed
    }
    pub(super) fn sync_requested_focus(&mut self, nodes: &mut Vec<Node>) -> bool {
        let restore = self.nodes.iter().enumerate().rev().find_map(|(i, old)| {
            let key = old.focus.return_to.as_deref()?;
            if self.subtree_exiting(i) || nodes.iter().any(|n| n.path == old.path) { return None; }
            let anchor = nodes.iter().find(|n| n.key.as_deref() == Some(key))?;
            let prefix = format!("{}/", anchor.path);
            nodes.iter().find(|n| (n.path == anchor.path || n.path.starts_with(&prefix))
                && (n.focus.enabled || n.input.as_ref().is_some_and(|i| i.access.is_some()))
                && n.vis.opacity > 0.0 && n.local.w > 0.0 && n.local.h > 0.0)
                .map(|n| n.path.clone())
        });
        // A newly requested text field wins; a focused control otherwise retains focus.
        let requested = nodes.iter().filter(|n| n.input.as_ref().is_some_and(|i| i.edit.focused));
        let next = requested.clone().find(|n| self.focused.as_deref() != Some(&n.path)
            && !self.nodes.iter().any(|old| old.path == n.path && old.input.as_ref().is_some_and(|i| i.edit.focused)))
            .map(|n| n.path.clone())
            .or(restore)
            .or_else(|| self.focused.clone().filter(|p| nodes.iter().any(|n| &n.path == p
                && (n.focus.enabled || n.input.as_ref().is_some_and(|i| i.edit.focused)))
                || self.virtual_inputs.get(p).is_some_and(|i| i.edit.focused)))
            .or_else(|| requested.into_iter().next().map(|n| n.path.clone()));
        // State may have deleted a row. Only the new view's accessors are valid now.
        // Use the same focus path as input events, then restore the old presentation
        // tree so apply can retain its exit visuals.
        std::mem::swap(&mut self.nodes, nodes);
        let changed = self.set_focus(next, self.keyboard_focus);
        std::mem::swap(&mut self.nodes, nodes);
        changed
    }
    fn tab_order(&self) -> Vec<usize> {
        let all: Vec<_> = (0..self.nodes.len()).filter(|&i| self.can_focus(i) && self.nodes[i].focus.order >= 0).collect();
        let mut order: Vec<_> = all.iter().copied().filter(|&i| {
            let Some(g) = self.group(i) else { return true; };
            let chosen = all.iter().copied().filter(|&j| self.group(j) == Some(g))
                .find(|&j| self.focused.as_deref() == Some(&self.nodes[j].path))
                .or_else(|| all.iter().copied().find(|&j| self.group(j) == Some(g) && self.nodes[j].focus.selected))
                .or_else(|| all.iter().copied().find(|&j| self.group(j) == Some(g)));
            chosen == Some(i)
        }).collect();
        order.sort_by_key(|&i| { let n = self.nodes[i].focus.order; (if n > 0 { n } else { i32::MAX }, i) });
        order
    }
    pub(super) fn focus_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo { return false; }
        self.keyboard_focus = true;
        if ev.key == Key::Escape && let Some(i) = self.active_modal() {
            let f = self.nodes[i].modal.as_ref().unwrap().dismiss.clone();
            self.set_focus(None, true);
            f(&mut self.state, tx);
            return true;
        }
        if ev.key == Key::Tab {
            let order = self.tab_order();
            if order.is_empty() { return self.active_modal().is_some(); }
            let current = order.iter().position(|&i| self.focused.as_deref() == Some(&self.nodes[i].path));
            let next = match current {
                Some(i) if ev.mods.shift => (i + order.len() - 1) % order.len(),
                Some(i) => (i + 1) % order.len(),
                None if ev.mods.shift => order.len() - 1,
                None => 0,
            };
            self.set_focus(Some(self.nodes[order[next]].path.clone()), true);
            return true;
        }
        let Some(i) = self.nodes.iter().position(|n| self.focused.as_deref() == Some(&n.path)) else { return false; };
        if !self.can_focus(i) { return false; }
        if let Some(g) = self.group(i) {
            let backward = matches!(ev.key, Key::Left | Key::Up);
            if backward || matches!(ev.key, Key::Right | Key::Down | Key::Home | Key::End) {
                let items: Vec<_> = (0..self.nodes.len()).filter(|&j| self.can_focus(j) && self.group(j) == Some(g)).collect();
                let pos = items.iter().position(|&j| j == i).unwrap();
                let next = match ev.key {
                    Key::Home => 0, Key::End => items.len() - 1,
                    _ if backward => (pos + items.len() - 1) % items.len(),
                    _ => (pos + 1) % items.len(),
                };
                let n = &self.nodes[items[next]];
                let action = (n.role == "radio" || n.role == "tab").then(|| n.on_press.clone()).flatten();
                let path = n.path.clone();
                self.set_focus(Some(path), true);
                if let Some(f) = action { f(&mut self.state, tx); }
                return true;
            }
        }
        if self.nodes[i].input.is_none() && (ev.key == Key::Enter || ev.key == Key::Text(" ".into())) {
            self.keyboard_focus = true;
            if let Some(f) = self.nodes[i].on_press.clone() { f(&mut self.state, tx); }
            return true;
        }
        false
    }
    pub(super) fn sync_modals(&mut self) {
        // Removed modals restore in reverse nesting order. Exiting visuals are already inert.
        while self.modal_stack.last().is_some_and(|(p, _)| self.node(p).is_none()) {
            let (_, restore) = self.modal_stack.pop().unwrap();
            let restore = restore.filter(|p| self.node(p).is_some());
            self.set_focus(restore, self.keyboard_focus);
        }
        let Some(i) = self.active_modal() else { return };
        let path = self.nodes[i].path.clone();
        if !self.modal_stack.iter().any(|(p, _)| p == &path) {
            self.modal_stack.push((path.clone(), self.focused.clone()));
            // Cancel any gesture captured before the modal opened.
            self.capture = None;
            self.secondary_capture = None;
            self.hovered = None;
        }
        let inside = self.focused.as_deref().and_then(|p| self.nodes.iter().position(|n| n.path == p))
            .is_some_and(|j| self.allowed(j));
        if !inside {
            let target = self.tab_order().first().map(|&j| self.nodes[j].path.clone()).unwrap_or(path);
            self.set_focus(Some(target), self.keyboard_focus);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{col, el, Mods};

    #[derive(Default)]
    struct State { open: bool, nested: bool, picks: usize, selected: usize, secondary_picks: usize }
    fn button(key: &str) -> El { el().key(key).size(30.0, 30.0).focusable(true).role("button") }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .on_secondary_press(|s: &mut State, _, _| s.secondary_picks += 1)
            .child(button("opener").on_press(|s: &mut State| s.open = true))
            .child(button("early").tab_index(1))
            .child(button("skip").tab_index(-1))
            .child(button("disabled").focusable(false))
            .child(col().key("radios").focus_group()
                .children((0..3).map(|i| button(&format!("radio{i}")).role("radio").focus_selected(s.selected == i)
                    .focusable(i != 1).on_press(move |s: &mut State| { s.selected = i; s.picks += 1; }))));
        if s.open {
            root = root.child(col().key("modal").absolute(100.0, 0.0).size(100.0, 120.0)
                .modal(true, |s: &mut State| s.open = false)
                .child(button("one").on_press(|s: &mut State| s.nested = true))
                .child(button("two")));
        }
        if s.open && s.nested {
            root = root.child(col().key("nested").absolute(200.0, 0.0).size(90.0, 90.0)
                .modal(false, |s: &mut State| s.nested = false).child(button("inner")));
        }
        root
    }
    fn runtime() -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (300.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0); rt
    }
    fn key(rt: &mut Runtime<State>, key: Key, shift: bool) {
        rt.key(KeyEvent { key, mods: Mods { shift, ..Default::default() } }); rt.frame(0.0);
    }
    fn focused<S: 'static>(rt: &Runtime<S>) -> String {
        rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key).unwrap()
    }
    #[test]
    fn window_blur_cancels_preedit_without_losing_committed_text_or_focus() {
        fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
            crate::text_input(s, crate::TextStyle::new(14.0, 400.0)).key("field").size(200.0, 40.0)
                .on_edit(|s: &mut crate::TextEdit| s)
        }
        let mut edit = crate::TextEdit::new("committed"); edit.focused = true;
        let mut rt = Runtime::new(edit, view, (200.0, 40.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.ime(crate::Ime::Preedit("pending".into(), None)); rt.frame(0.0);
        rt.ime_reset = false;
        rt.cancel_composition(); rt.frame(0.0);
        assert!(rt.state.preedit.is_none() && rt.state.focused && rt.ime_reset);
        assert_eq!(rt.state.text, "committed");
    }

    #[test]
    fn unrealized_virtual_input_keeps_focus_and_does_not_steal_it_back() {
        fn view(s: &Vec<crate::TextEdit>, _: &mut Cx<Vec<crate::TextEdit>>) -> El {
            col().key("root").size(200.0, 240.0).child(button("other"))
                .child(col().key("viewport").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).clip_soft(0.0)
                    .child(col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(40.0)
                        .children(s.iter().enumerate().map(|(i, edit)| {
                            crate::text_input(edit, crate::TextStyle::new(14.0, 400.0))
                                .key(format!("row-{i}")).size(200.0, 40.0)
                                .on_edit(move |s: &mut Vec<crate::TextEdit>| &mut s[i])
                        }))))
        }
        let mut state: Vec<_> = (0..100).map(|_| crate::TextEdit::new("")).collect();
        state[0].focused = true;
        let mut rt = Runtime::new(state, view, (200.0, 240.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let path = rt.focused.clone();
        rt.wheel(100.0, 100.0, -2000.0); rt.frame(0.0);
        assert!(rt.node(path.as_deref().unwrap()).is_none());
        assert_eq!(rt.focused, path);
        rt.key(KeyEvent { key: Key::Text("offscreen".into()), mods: Mods::default() }); rt.frame(0.0);
        assert_eq!(rt.state[0].text, "offscreen");
        let (x, y) = rt.find("other").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert!(!rt.state[0].focused);
        rt.wheel(100.0, 100.0, 2000.0); rt.frame(0.0);
        assert_eq!(focused(&rt), "other");
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("row-0")));
    }

    #[test]
    fn input_handover_cancels_composition_and_keys_bubble_after_the_field() {
        use crate::{text_input, TextEdit, TextStyle};
        struct Form { first: TextEdit, search: TextEdit, open: bool, app_keys: usize }
        fn view(s: &Form, _: &mut Cx<Form>) -> El {
            let mut root = col().size(300.0, 300.0)
                .child(text_input(&s.first, TextStyle::new(14.0, 400.0)).key("first").size(200.0, 40.0)
                    .on_edit(|s: &mut Form| &mut s.first))
                .child(button("open").on_press(|s: &mut Form| { s.open = true; s.search.focused = true; }));
            if s.open {
                root = root.child(text_input(&s.search, TextStyle::new(14.0, 400.0)).key("search").size(200.0, 40.0)
                    .on_edit(|s: &mut Form| &mut s.search));
            }
            root
        }
        let mut first = TextEdit::new("hello"); first.focused = true;
        let mut rt = Runtime::new(Form { first, search: TextEdit::new(""), open: false, app_keys: 0 }, view,
            (300.0, 300.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, _, _| { s.app_keys += 1; true });
        rt.frame(0.0);
        rt.ime(crate::Ime::Preedit("pending".into(), None)); rt.frame(0.0);
        rt.state.open = true; rt.state.search.focused = true; rt.invalidate(); rt.frame(0.0);
        assert!(rt.state.search.focused);
        assert!(!rt.state.first.focused);
        assert!(rt.state.first.preedit.is_none());
        assert!(rt.ime_reset);
        assert!(!rt.keyboard_focus);
        rt.key(KeyEvent { key: Key::Text("query".into()), mods: Mods::default() }); rt.frame(0.0);
        assert_eq!(rt.state.search.text, "query");
        assert_eq!(rt.state.first.text, "hello");
        assert_eq!(rt.state.app_keys, 0);
        rt.key(KeyEvent { key: Key::Function(5), mods: Mods::default() }); rt.frame(0.0);
        assert_eq!(rt.state.app_keys, 1);
        rt.ime(crate::Ime::Preedit("pending".into(), None)); rt.frame(0.0);
        rt.pointer_down(290.0, 290.0); rt.pointer_up(290.0, 290.0); rt.frame(0.0);
        assert!(rt.focused.is_none());
        assert!(!rt.state.search.focused);
        assert!(rt.state.search.preedit.is_none());
    }

    #[test]
    fn focus_document_order_overrides_roving_and_activation() {
        let mut rt = runtime();
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "early");
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "opener");
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "radio0");
        key(&mut rt, Key::Right, false); assert_eq!(focused(&rt), "radio2");
        assert_eq!((rt.state.selected, rt.state.picks), (2, 1));
        key(&mut rt, Key::Text(" ".into()), false); assert_eq!(rt.state.picks, 2);
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "early");
        key(&mut rt, Key::Tab, true); assert_eq!(focused(&rt), "radio2");
        let (x, y) = rt.find("early").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "early");
        assert!(!rt.elements().iter().any(|e| e.focus_visible));
    }
    #[test]
    fn modal_traps_restores_nested_and_blocks_background() {
        let mut rt = runtime();
        key(&mut rt, Key::Tab, false); key(&mut rt, Key::Tab, false);
        key(&mut rt, Key::Enter, false); assert_eq!(focused(&rt), "one");
        let (x, y) = rt.find("one").unwrap();
        rt.secondary_down(x, y); rt.secondary_up(x, y); rt.frame(0.0);
        assert_eq!(rt.state.secondary_picks, 0, "secondary events must not bubble past a modal");
        key(&mut rt, Key::Tab, true); assert_eq!(focused(&rt), "two");
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "one");
        key(&mut rt, Key::Enter, false); assert_eq!(focused(&rt), "inner");
        let (x, y) = rt.find("opener").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert!(rt.state.nested); assert_eq!(focused(&rt), "inner");
        key(&mut rt, Key::Escape, false); assert_eq!(focused(&rt), "one");
        key(&mut rt, Key::Escape, false); assert_eq!(focused(&rt), "opener");
        assert!(!rt.state.open);
        key(&mut rt, Key::Enter, false);
        rt.state.nested = false; rt.invalidate(); rt.frame(0.0);
        rt.pointer_down(5.0, 280.0); rt.pointer_up(5.0, 280.0); rt.frame(0.0);
        assert!(!rt.state.open); assert_eq!(focused(&rt), "opener");
        assert!(!rt.elements().iter().any(|e| e.focus_visible));
    }
}
