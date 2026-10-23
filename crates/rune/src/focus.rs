//! Focus is keyed retained state. Platform AX can read the same flags as the inspector.
use super::*;
use crate::edit::Key;

/// Groups whose items stack vertically: only Up and Down move between them.
const VERTICAL_ROLES: [&str; 4] = ["menu", "listbox", "list", "tree"];
/// Groups that move to the item whose label the user types.
const TYPE_SELECT_ROLES: [&str; 5] = ["menu", "listbox", "list", "tree", "tablist"];
/// Without Full Keyboard Access, Tab still visits lists inside these groups.
const LIST_ROLES: [&str; 4] = ["listbox", "list", "tree", "grid"];
/// AppKit's type-select resets after this long without a key.
const TYPE_SELECT_PAUSE: f64 = 1.0;

fn sync_input_focus<S: 'static>(nodes: &mut [Node], state: &mut S, path: Option<&str>) -> bool {
    let mut changed = false;
    for n in nodes {
        if let Some(input) = &mut n.input {
            let focused = path == Some(n.path.as_str());
            if let Some(access) = &input.access {
                let edit = access(handler_state(state));
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
    /// The `focus_highlight` row that `i` (a pointer target) belongs to.
    pub(super) fn highlight_row(&self, mut i: usize) -> Option<usize> {
        loop {
            let n = &self.nodes[i];
            if n.focus.enabled { return (n.focus.highlight.is_some() && self.can_focus(i)).then_some(i); }
            i = n.parent?;
        }
    }
    pub(super) fn highlight_visible(&self, i: usize) -> bool {
        (self.keyboard_focus || self.hover_focus) && self.focused.as_deref() == Some(&self.nodes[i].path)
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
        if self.focused != path { self.ime_area = None; self.ime_reset = true; self.blur_links(); self.hover_focus = false; }
        let lost = self.focused != path && self.physical_focus_lost();
        let changed = sync_input_focus(&mut self.nodes, &mut self.state, path.as_deref()) || lost;
        let mut changed = changed;
        for (p, input) in &mut self.virtual_inputs {
            if let Some(access) = &input.access {
                let edit = access(handler_state(&mut self.state));
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
    /// Type-select: the typed prefix picks the first item whose label starts with it. A
    /// repeated letter cycles the items starting with it. The prefix resets after a pause.
    fn type_select(&mut self, items: &[usize], pos: usize, typed: &str) -> Option<usize> {
        let now = self.now_clock();
        let (buffer, at) = &mut self.typeahead;
        if now - *at > TYPE_SELECT_PAUSE { buffer.clear(); }
        *at = now;
        buffer.push_str(&typed.to_lowercase());
        let label = |j: usize| self.nodes[items[j]].label.as_deref().unwrap_or("").to_lowercase();
        let first = buffer.chars().next()?;
        let cycle = buffer.chars().all(|c| c == first);
        let order = (0..items.len()).map(|k| if cycle { (pos + 1 + k) % items.len() } else { k });
        let prefix = if cycle { first.to_string() } else { buffer.clone() };
        order.into_iter().find(|&j| label(j).starts_with(&prefix))
    }
    /// `shown`: the focus highlight was visible before this key arrived.
    pub(super) fn focus_key(&mut self, ev: &KeyEvent, shown: bool, tx: &mut Tx) -> bool {
        if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo { return false; }
        self.keyboard_focus = true;
        if ev.key == Key::Escape && let Some(i) = self.active_modal() {
            let f = self.nodes[i].modal.as_ref().unwrap().dismiss.clone();
            self.set_focus(None, true);
            f(handler_state(&mut self.state), tx);
            return true;
        }
        if ev.key == Key::Tab {
            if self.link_tab(ev.mods.shift) { return true; }
            let mut order = self.tab_order();
            if !self.full_keyboard_access {
                order.retain(|&i| self.nodes[i].input.is_some()
                    || self.group(i).is_some_and(|g| LIST_ROLES.contains(&self.nodes[g].role)));
            }
            if order.is_empty() { return self.active_modal().is_some(); }
            let current = order.iter().position(|&i| self.focused.as_deref() == Some(&self.nodes[i].path));
            let next = match current {
                Some(i) if ev.mods.shift => (i + order.len() - 1) % order.len(),
                Some(i) => (i + 1) % order.len(),
                None if ev.mods.shift => order.len() - 1,
                None => 0,
            };
            let path = self.nodes[order[next]].path.clone();
            self.set_focus(Some(path.clone()), true);
            tx.scroll_to.push(path);
            self.enter_links(order[next], ev.mods.shift);
            return true;
        }
        let Some(i) = self.nodes.iter().position(|n| self.focused.as_deref() == Some(&n.path)) else { return false; };
        if !self.can_focus(i) { return false; }
        if let Some(g) = self.group(i) {
            let vertical = VERTICAL_ROLES.contains(&self.nodes[g].role);
            let backward = matches!(ev.key, Key::Up) || (!vertical && ev.key == Key::Left);
            let forward = matches!(ev.key, Key::Down) || (!vertical && ev.key == Key::Right);
            let items: Vec<_> = (0..self.nodes.len()).filter(|&j| self.can_focus(j) && self.group(j) == Some(g)).collect();
            let pos = items.iter().position(|&j| j == i).unwrap();
            let next = if backward || forward || matches!(ev.key, Key::Home | Key::End) {
                self.typeahead.0.clear();
                Some(match ev.key {
                    Key::Home => 0, Key::End => items.len() - 1,
                    // A highlight the user cannot see yet appears where it is (a popup's checked
                    // item, else the first) before arrows move it; Up from nothing is the last.
                    _ if self.nodes[i].focus.highlight.is_some() && !shown =>
                        if backward && !self.nodes[i].focus.selected { items.len() - 1 } else { pos },
                    _ if backward => (pos + items.len() - 1) % items.len(),
                    _ => (pos + 1) % items.len(),
                })
            } else if let Key::Text(t) = &ev.key && TYPE_SELECT_ROLES.contains(&self.nodes[g].role) && !t.trim().is_empty() {
                self.type_select(&items, pos, t)
            } else { None };
            if let Some(next) = next {
                let n = &self.nodes[items[next]];
                let follows = n.role == "radio" || n.role == "tab" || self.nodes[g].focus.follows;
                let action = follows.then(|| n.on_press.clone()).flatten();
                let path = n.path.clone();
                self.set_focus(Some(path.clone()), true);
                tx.scroll_to.push(path);
                if let Some(f) = action { f(handler_state(&mut self.state), tx); }
                return true;
            }
        } else if self.nodes[i].role == "combobox" && self.nodes[i].input.is_none() && matches!(ev.key, Key::Up | Key::Down) {
            // A popup button opens on its arrows as well as Space and Return (NSPopUpButton).
            if let Some(f) = self.nodes[i].on_press.clone() { f(handler_state(&mut self.state), tx); }
            return true;
        }
        if self.nodes[i].input.is_none() && (ev.key == Key::Enter || ev.key == Key::Text(" ".into())) {
            // A row the user cannot see highlighted (the pointer left it) chooses nothing.
            if self.nodes[i].focus.highlight.is_some() && !shown { self.keyboard_focus = false; return true; }
            if self.link_key(i, tx) { return true; }
            if let Some(f) = self.nodes[i].on_press.clone() { f(handler_state(&mut self.state), tx); }
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
    fn state_focus_routes_the_first_same_frame_key_to_a_new_field() {
        use crate::{text_input, TextEdit, TextStyle};
        struct Form { first: TextEdit, next: TextEdit, open: bool, hidden: bool }
        fn view(s: &Form, _: &mut Cx<Form>) -> El {
            col().size(200.0, 100.0)
                .child(text_input(&s.first, TextStyle::new(14.0, 400.0)).key("first").size(200.0, 40.0)
                    .on_edit(|s: &mut Form| &mut s.first))
                .children(s.open.then(|| text_input(&s.next, TextStyle::new(14.0, 400.0)).key("next").size(200.0, 40.0)
                    .opacity(if s.hidden { 0.0 } else { 1.0 })
                    .on_edit(|s: &mut Form| &mut s.next)))
        }
        let mut first = TextEdit::new(""); first.focused = true;
        let mut rt = Runtime::new(Form { first, next: TextEdit::new(""), open: false, hidden: false }, view,
            (200.0, 100.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.update(|s, _| { s.open = true; s.next.focused = true; });
        rt.key(KeyEvent { key: Key::Text("x".into()), mods: Mods::default() });
        rt.frame(0.0);
        assert_eq!(focused(&rt), "next");
        assert_eq!(rt.state.next.text, "x", "the first key must not use the previous frame's focus");
        assert_eq!(rt.state.first.text, "");
        assert!(!rt.state.first.focused && rt.state.next.focused);
        assert!(rt.ime_area.is_some());
        rt.update(|s, _| s.hidden = true); rt.frame(0.0);
        assert!(rt.ime_area.is_none(), "authored hidden inputs must not enable IME");
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

    #[derive(Default)]
    struct Menu { open: bool, checked: Option<usize>, picks: Vec<usize>, follows: bool, search: bool, field: crate::TextEdit }
    const ITEMS: [&str; 4] = ["Alpha", "Beta", "Bravo", "Charlie"];
    fn menu_view(s: &Menu, _: &mut Cx<Menu>) -> El {
        let mut root = col().key("root").size(300.0, 300.0)
            .child(button("popup").role("combobox").on_press(|s: &mut Menu| s.open = true))
            .child(button("plain"));
        if s.open {
            let mut list = col().key("menu").role("menu").focus_group();
            if s.follows { list = list.focus_follows(); }
            root = root.child(col().key("surface").absolute(100.0, 100.0).focus_return("popup")
                .modal(true, |s: &mut Menu| s.open = false)
                .children(s.search.then(|| crate::text_input(&s.field, crate::TextStyle::new(14.0, 400.0))
                    .key("field").size(80.0, 20.0).on_edit(|s: &mut Menu| &mut s.field)))
                .child(list.children(ITEMS.iter().enumerate().map(|(i, label)| {
                    el().key(*label).label(*label).size(80.0, 20.0).focusable(true)
                        .focus_selected(s.checked == Some(i)).focus_highlight(crate::Color::WHITE)
                        .on_press(move |s: &mut Menu| { s.picks.push(i); s.open = s.follows; })
                }))));
        }
        root
    }
    fn menu_runtime(state: Menu) -> Runtime<Menu> {
        let mut rt = Runtime::new(state, menu_view, (300.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0); rt
    }
    fn press<S: 'static>(rt: &mut Runtime<S>, key: Key) {
        rt.key(KeyEvent { key, mods: Mods::default() }); rt.frame(0.0);
    }
    fn highlighted(rt: &Runtime<Menu>) -> Option<String> {
        (0..rt.nodes.len()).find(|&i| rt.nodes[i].focus.highlight.is_some() && rt.highlight_visible(i))
            .and_then(|i| rt.nodes[i].key.clone())
    }

    #[test]
    fn menu_arrows_wrap_vertically_and_return_focus_on_escape() {
        let mut rt = menu_runtime(Menu::default());
        let (x, y) = rt.find("popup").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "Alpha");
        assert_eq!(highlighted(&rt), None, "a pointer-opened menu shows no highlight yet");
        press(&mut rt, Key::Enter);
        assert!(rt.state.open && rt.state.picks.is_empty(), "Return chooses no hidden highlight");
        press(&mut rt, Key::Down);
        assert_eq!(highlighted(&rt).as_deref(), Some("Alpha"), "the first arrow reveals the highlight");
        press(&mut rt, Key::Down); assert_eq!(focused(&rt), "Beta");
        press(&mut rt, Key::Right); assert_eq!(focused(&rt), "Beta", "Left and Right do not move in a menu");
        press(&mut rt, Key::End); assert_eq!(focused(&rt), "Charlie");
        press(&mut rt, Key::Down); assert_eq!(focused(&rt), "Alpha");
        press(&mut rt, Key::Up); assert_eq!(focused(&rt), "Charlie");
        press(&mut rt, Key::Home); assert_eq!(focused(&rt), "Alpha");
        assert!(rt.state.picks.is_empty(), "moving does not choose");
        press(&mut rt, Key::Escape);
        assert!(!rt.state.open);
        assert_eq!(focused(&rt), "popup");
    }

    #[test]
    fn a_hidden_highlight_appears_on_the_checked_item_or_from_the_end() {
        let mut rt = menu_runtime(Menu { open: true, checked: Some(2), ..Default::default() });
        assert_eq!(focused(&rt), "Bravo", "a popup opens on its checked item");
        press(&mut rt, Key::Up);
        assert_eq!(focused(&rt), "Bravo");
        let mut rt = menu_runtime(Menu { open: true, ..Default::default() });
        press(&mut rt, Key::Up);
        assert_eq!(focused(&rt), "Charlie", "Up into an unchecked menu starts from the bottom");
    }

    #[test]
    fn type_select_matches_prefixes_and_cycles_a_repeated_letter() {
        let mut rt = menu_runtime(Menu { open: true, ..Default::default() });
        press(&mut rt, Key::Text("b".into())); assert_eq!(focused(&rt), "Beta");
        press(&mut rt, Key::Text("r".into())); assert_eq!(focused(&rt), "Bravo");
        rt.typeahead.1 -= TYPE_SELECT_PAUSE + 0.1;
        press(&mut rt, Key::Text("c".into())); assert_eq!(focused(&rt), "Charlie", "a pause starts a new prefix");
        rt.typeahead.1 -= TYPE_SELECT_PAUSE + 0.1;
        press(&mut rt, Key::Text("b".into())); assert_eq!(focused(&rt), "Beta");
        press(&mut rt, Key::Text("b".into())); assert_eq!(focused(&rt), "Bravo");
        press(&mut rt, Key::Text("b".into())); assert_eq!(focused(&rt), "Beta");
        press(&mut rt, Key::Enter);
        assert_eq!(rt.state.picks, [1]);
        assert!(!rt.state.open);
    }

    #[test]
    fn pointer_and_keys_share_one_highlight_but_fields_keep_focus() {
        let mut rt = menu_runtime(Menu { open: true, ..Default::default() });
        let (x, y) = rt.find("Bravo").unwrap();
        rt.pointer_move(x, y); rt.frame(0.0);
        assert_eq!(highlighted(&rt).as_deref(), Some("Bravo"));
        assert!(!rt.elements().iter().any(|e| e.focus_visible), "a pointer highlight is not a focus ring");
        press(&mut rt, Key::Down); assert_eq!(highlighted(&rt).as_deref(), Some("Charlie"));
        rt.pointer_move(5.0, 290.0); rt.frame(0.0);
        assert_eq!(highlighted(&rt).as_deref(), Some("Charlie"), "leaving rows keeps a keyboard highlight");

        let mut rt = menu_runtime(Menu { open: true, search: true, ..Default::default() });
        assert_eq!(focused(&rt), "field");
        let (x, y) = rt.find("Beta").unwrap();
        rt.pointer_move(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "field", "hovering results never takes focus from their search field");
    }

    #[test]
    fn follows_presses_as_it_moves_and_popups_open_on_arrows() {
        let mut rt = menu_runtime(Menu { follows: true, ..Default::default() });
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "popup");
        press(&mut rt, Key::Down);
        assert!(rt.state.open, "Down opens a popup button");
        assert_eq!(focused(&rt), "Alpha");
        press(&mut rt, Key::Down); press(&mut rt, Key::Down);
        assert_eq!(rt.state.picks, [1, 2], "each move presses the item it reaches");
    }

    #[test]
    fn without_full_keyboard_access_tab_visits_only_fields_and_lists() {
        use crate::{text_input, TextEdit, TextStyle};
        fn view(s: &TextEdit, _: &mut Cx<TextEdit>) -> El {
            col().size(200.0, 200.0)
                .child(text_input(s, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 30.0).on_edit(|s: &mut TextEdit| s))
                .child(button("button"))
                .child(col().role("list").focus_group().child(button("row0").focus_selected(true)).child(button("row1")))
        }
        let mut rt = Runtime::new(TextEdit::new(""), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.set_full_keyboard_access(false);
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "field");
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "row0", "buttons are skipped, lists are not");
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "field");
        rt.set_full_keyboard_access(true);
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "button");
    }

    #[test]
    fn keyboard_focus_scrolls_into_view() {
        fn view(_: &State, _: &mut Cx<State>) -> El {
            col().key("root").size(200.0, 200.0).child(button("before"))
                .child(col().key("viewport").size(200.0, 100.0).scroll(ScrollSpec { stick_bottom: false }).clip_soft(0.0)
                    .child(col().role("list").focus_group().children((0..10).map(|i| button(&format!("row{i}"))))))
                .child(button("after"))
        }
        let mut rt = Runtime::new(State::default(), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let visible = |rt: &Runtime<State>, key: &str| {
            let (_, y) = rt.find(key).unwrap();
            let (_, top) = rt.find("viewport").unwrap();
            (top - 50.0..=top + 50.0).contains(&y)
        };
        assert!(!visible(&rt, "row9"));
        key(&mut rt, Key::Tab, false); key(&mut rt, Key::Tab, false);
        key(&mut rt, Key::End, false);
        assert_eq!(focused(&rt), "row9");
        assert!(visible(&rt, "row9"), "arrows reveal the row they reach");
        key(&mut rt, Key::Tab, false); key(&mut rt, Key::Tab, true);
        key(&mut rt, Key::Home, false);
        assert!(visible(&rt, "row0"));
        rt.wheel(100.0, 100.0, -2000.0); rt.frame(0.0);
        key(&mut rt, Key::Tab, true); key(&mut rt, Key::Tab, false);
        assert_eq!(focused(&rt), "row0");
        assert!(visible(&rt, "row0"), "Tab reveals the control it reaches");
    }
}
