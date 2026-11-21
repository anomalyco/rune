//! The secondary gesture (right-click, macOS control-click, the Menu key and Shift+F10):
//! which element's menu it opens.
use super::*;

impl capture::PointerState {
    /// The secondary press went to the browser's own menu: its release opens none of Rune's.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline]
    pub(in crate::runtime) fn abandon_secondary(&mut self) {
        self.clear_secondary();
    }
}

impl RuntimeCore {
    /// The nearest element with `on_secondary_press` (or a link with `on_link_secondary`) at
    /// (x, y), else the selectable text there (its Copy menu): an app's context menu on a row
    /// wins over its text's.
    pub(in crate::runtime) fn secondary_target(&self, x: f32, y: f32) -> Option<&str> {
        let id = self.hit.top_hit(|id, r| self.hit_contains(id, r, x, y))?;
        let mut index = *self.tree.node_index.get(&id)?;
        let mut text = None;
        loop {
            let n = &self.tree.nodes[index];
            if self.subtree_exiting(index) || !self.allowed(index) { break; }
            if n.handlers.on_secondary_press.is_some() || self.secondary_link_at(index, x, y).is_some() { return Some(&n.path); }
            // A text field's own menu (`field_menu`) wins over its ancestors'.
            if n.input.as_ref().is_some_and(|i| i.access.is_some()) { return Some(&n.path); }
            if text.is_none() && self.is_selectable(index) { text = Some(n.path.as_str()); }
            let Some(p) = n.parent else { break };
            index = p;
        }
        text
    }

    /// Capture a secondary gesture without activating primary actions or text selection.
    pub fn secondary_down(&mut self, x: f32, y: f32) {
        self.pointer.pos = Some((x, y));
        if self.pointer.autoscroll_press(crate::PointerButton::Secondary) { return; }
        self.tint_down(x, y, crate::PointerButton::Secondary);
        self.release_hover_hold();
        self.kbd.focus.step(focus_machine::FocusEvent::Pointed);
        self.dismiss_help();
        let path = self.secondary_target(x, y).map(str::to_owned);
        let i = path.as_ref().and_then(|p| self.tree.nodes.iter().position(|n| n.path == *p));
        self.pointer.set_secondary(match path {
            Some(path) => capture::Secondary::Held { path, link: i.and_then(|i| self.secondary_link_at(i, x, y)), left: false },
            None => capture::Secondary::Up,
        });
        self.secondary_region_down(x, y, self.pointer.secondary_held());
        // Under `Cx::web_focus` a right-click focuses the control it's on, as Chromium's
        // mousedown does, so focus is back on it when its menu closes.
        if self.commit.defaults.web_focus
            && let Some(c) = i.and_then(|i| std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent).find(|&j| self.can_focus(j)))
            && self.tree.nodes[c].input.is_none()
        {
            self.frame.queue.push(Action::FocusControl(self.tree.nodes[c].path.clone()));
        }
    }

    /// A middle or secondary press at (x, y), with no primary press held: what a primary press
    /// there would take shows pressed until the button comes up (`capture::Tint`).
    pub(in crate::runtime) fn tint_down(&mut self, x: f32, y: f32, button: crate::PointerButton) {
        if self.pointer.press.held().is_some_and(|h| h.button == crate::PointerButton::Primary) { return; }
        let path = self.primary_target(x, y).filter(|&i| self.allowed(i) && !self.subtree_exiting(i)).map(|i| self.tree.nodes[i].path.clone());
        self.pointer.set_tint(path.map_or(capture::Tint::Up, |path| capture::Tint::Held { path, button }));
    }

    /// What the secondary press being held is on, for a browser that shows its own context
    /// menu: `"app"` when an element's menu or a link's takes it (Rune draws that), `"field"` on
    /// a text field, `"text"` on selectable text alone, `""` on nothing of Rune's.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn secondary_claim(&self) -> &'static str {
        let capture::Secondary::Held { path, link, .. } = self.pointer.secondary() else { return "" };
        let Some(n) = self.tree.nodes.iter().find(|n| n.path == *path) else { return "" };
        if link.is_some() || n.handlers.on_secondary_press.is_some() { return "app"; }
        // On a misspelled word or a pill with a secondary handler, Rune's own menu wins over the browser's.
        if let Some(input) = n.input.as_deref()
            && input.access.is_some()
        {
            let on_pill = input.on_pill_secondary.is_some()
                && self.pointer.pos.and_then(|(x, y)| self.live(path).and_then(|i| self.input_pill_at(i, x, y))).is_some();
            return if on_pill || self.on_misspelled_word(path) { "app" } else { "field" };
        }
        "text"
    }

    /// The pointer is over a word field `path`'s spell checker marks.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn on_misspelled_word(&self, path: &str) -> bool {
        let Some(i) = self.live(path) else { return false };
        let n = &self.tree.nodes[i];
        let (Some(input), Some((x, y)), Some(layout)) = (n.input.as_deref(), self.pointer.pos, n.text_layout.as_ref()) else { return false };
        if !input.spellcheck || input.edit.is_secure() { return false; }
        let Some([tx, ty]) = self.input_text_point(i, x, y) else { return false };
        let caret = super::input_click::hit(&input.edit, layout.view(), tx, ty, self.env.scale, input.single_line).caret;
        crate::spell::misspelled_at(&input.edit.text, caret).is_some()
    }

    /// The secondary press went to the browser's own menu: its release opens none of Rune's.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline]
    pub(crate) fn secondary_abandon(&mut self) {
        self.pointer.abandon_secondary();
    }

    pub(crate) fn secondary_up(&mut self, app: &dyn ViewDriver, x: f32, y: f32) {
        self.pointer.pos = Some((x, y));
        self.pointer.release_tint(crate::PointerButton::Secondary);
        if self.pointer.autoscroll_release(crate::PointerButton::Secondary) { return; }
        self.secondary_region_up(x, y);
        let (path, down_link, left) = match self.pointer.take_secondary() {
            capture::Secondary::Held { path, link, left } => (Some(path), link, left),
            capture::Secondary::Up => (None, None, false),
        };
        if let Some(path) = path
            && self.secondary_target(x, y) == Some(path.as_str())
        {
            let i = self.tree.nodes.iter().position(|n| n.path == path);
            let point = i.map_or(PointerEvent { x, y, window_x: x, window_y: y }, |i| self.pointer_event(i, x, y));
            let link = i.and_then(|i| self.secondary_link_at(i, x, y));
            // Like a primary link press, it must end on the link it began on; one that moved
            // onto, off or between links opens nothing.
            match (link, down_link) {
                (Some(k), Some(d)) if k == d => self.frame.queue.push(Action::LinkSecondary(path, k, point)),
                (None, None) if !self.selection_menu(app, &path, x, y) && !self.queue_field_menu(&path, x, y) => self.frame.queue.push(Action::SecondaryPress(path, point)),
                _ => {}
            }
        }
        self.leave_after_release(left);
    }

    /// A secondary press at (x, y) on text field `path` with no `on_secondary_press` of its own:
    /// its menu (`field_menu`), queued.
    fn queue_field_menu(&mut self, path: &str, x: f32, y: f32) -> bool {
        let Some(i) = self.live(path) else { return false };
        let n = &self.tree.nodes[i];
        if n.handlers.on_secondary_press.is_some() || n.input.as_ref().is_none_or(|i| i.access.is_none()) { return false; }
        self.frame.queue.push(Action::FieldMenu(path.to_owned(), (x, y), Some((x, y))));
        true
    }

    /// The Menu key, or Shift+F10, that nothing before it took: the focused element's context
    /// menu. That is its focused link's `on_link_secondary`, else its own or its nearest
    /// ancestor's `on_secondary_press`, at the bottom-left of the focused link or element, where
    /// a menu opens below it as AppKit's and Windows' keyboard menus do.
    pub(in crate::runtime) fn context_menu_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let m = ev.mods;
        let plain = !m.cmd && !m.ctrl && !m.alt && !m.logo;
        let menu_key = match ev.key {
            crate::Key::ContextMenu => plain,
            crate::Key::Function(10) => plain && m.shift,
            _ => false,
        };
        let Some(focused) = self.focused_index().filter(|&i| menu_key && self.can_focus(i)) else { return false };
        if self.focused_link_menu(app, focused, tx) { return true; }
        let [x, _, _, y] = self.drawn_box(focused);
        let n = &self.tree.nodes[focused];
        if n.handlers.on_secondary_press.is_none() && n.input.as_ref().is_some_and(|i| i.access.is_some()) {
            return self.field_menu(app, n.path.clone(), (x, y), None, tx);
        }
        let mut index = focused;
        loop {
            let n = &self.tree.nodes[index];
            if !self.allowed(index) { return false; }
            if let Some(f) = n.handlers.on_secondary_press.clone() {
                let path = n.path.clone();
                let point = self.pointer_event(index, x, y);
                log_event!(self, "secondary press {path}");
                f(app.states(), point, tx);
                return true;
            }
            let Some(p) = n.parent else { return false };
            index = p;
        }
    }
}
