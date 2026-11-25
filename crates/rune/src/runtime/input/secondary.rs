//! The secondary gesture (right-click, macOS control-click, the Menu key and Shift+F10):
//! which element's menu it opens.
use super::*;

/// Host platform family for outside-modal secondary-press policy (Google Chrome's OS split).
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SecondaryPlatform {
    Windows,
    MacOs,
    Linux,
    Web,
}

impl SecondaryPlatform {
    pub(super) const CURRENT: Self = {
        #[cfg(target_arch = "wasm32")]
        { Self::Web }
        #[cfg(all(not(target_arch = "wasm32"), target_os = "windows"))]
        { Self::Windows }
        #[cfg(all(not(target_arch = "wasm32"), target_os = "macos"))]
        { Self::MacOs }
        #[cfg(all(not(target_arch = "wasm32"), not(target_os = "windows"), not(target_os = "macos")))]
        { Self::Linux }
    };
}

/// How a secondary press outside an active modal is handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutsideSecondary {
    /// Leave the modal open and consume the press (`click_outside == false` or `modal_trap()`).
    Swallow,
    /// Dismiss the modal and consume the press (macOS, Linux and web menus; dialogs and selects
    /// everywhere).
    DismissSwallow,
    /// Dismiss the modal and repost the press to what lies under it (Windows menus).
    DismissRepost,
}

/// Google Chrome's per-OS policy for a secondary press outside an active modal: non-dismissible
/// modals swallow everywhere, non-menu modals (dialogs, selects) dismiss and swallow everywhere,
/// and menu modals dismiss and repost on Windows while dismissing and swallowing on macOS, Linux
/// and web.
pub(super) const fn outside_secondary_policy(
    platform: SecondaryPlatform,
    is_menu: bool,
    can_dismiss_outside: bool,
) -> OutsideSecondary {
    if !can_dismiss_outside {
        return OutsideSecondary::Swallow;
    }
    match (platform, is_menu) {
        (SecondaryPlatform::Windows, true) => OutsideSecondary::DismissRepost,
        _ => OutsideSecondary::DismissSwallow,
    }
}

impl capture::PointerState {
    /// The secondary press went to the browser's own menu: its release opens none of Rune's.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline]
    pub(in crate::runtime) fn abandon_secondary(&mut self) {
        self.clear_secondary();
        self.note_button_up(crate::PointerButton::Secondary);
    }
}

impl RuntimeCore {
    /// Whether node `i` or any of its ancestors is marked disabled (`El::disabled` or
    /// `El::aria_disabled`).
    pub(crate) fn node_disabled(&self, i: usize) -> bool {
        std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent)
            .any(|k| self.tree.nodes[k].a11y.disabled)
    }

    /// Whether node `i` is an enabled editable text field eligible for its own field menu.
    fn editable_field_menu(&self, i: usize) -> bool {
        self.tree.nodes[i].input.as_ref().is_some_and(|spec| spec.access.is_some())
            && !self.node_disabled(i)
    }

    /// The single reachable editor inside a `role("textbox")` container that handles secondary
    /// presses (its own `on_secondary_press`, or its editable `field_menu`).
    fn textbox_secondary_input(&self, i: usize) -> Option<usize> {
        if self.tree.nodes[i].role != "textbox" { return None; }
        let mut inputs = (0..self.tree.nodes.len()).filter(|&j| {
            let n = &self.tree.nodes[j];
            n.input.is_some()
                && (n.handlers.on_secondary_press.is_some()
                    || n.input.as_ref().is_some_and(|spec| spec.access.is_some()))
                && self.reachable(j)
                && !self.node_disabled(j)
                && std::iter::successors(n.parent, |&p| self.tree.nodes[p].parent).any(|p| p == i)
        });
        let first = inputs.next()?;
        inputs.next().is_none().then_some(first)
    }

    /// The nearest element with `on_secondary_press` (or a link with `on_link_secondary`) at
    /// (x, y), else the selectable text there (its Copy menu): an app's context menu on a row
    /// wins over its text's.
    pub(in crate::runtime) fn secondary_target(&self, x: f32, y: f32) -> Option<&str> {
        let id = self.hit.top_hit(|id, r| self.hit_contains(id, r, x, y))?;
        let mut index = *self.tree.node_index.get(&id)?;
        let mut text = None;
        let mut first = true;
        loop {
            let n = &self.tree.nodes[index];
            if self.subtree_exiting(index) || !self.allowed(index) { break; }
            let disabled_input = n.input.is_some() && self.node_disabled(index);
            if !disabled_input && (n.handlers.on_secondary_press.is_some() || self.secondary_link_at(index, x, y).is_some()) {
                return Some(&n.path);
            }
            // A text field's own menu (`field_menu`) wins over its ancestors'.
            if self.editable_field_menu(index) { return Some(&n.path); }
            // A `role("textbox")` wrapper's padding routes secondary presses to its single editor.
            if first && let Some(input_i) = self.textbox_secondary_input(index) {
                return Some(&self.tree.nodes[input_i].path);
            }
            first = false;
            if text.is_none() && (self.default_link_at(index, x, y).is_some() || self.is_selectable(index)) {
                text = Some(n.path.as_str());
            }
            let Some(p) = n.parent else { break };
            index = p;
        }
        text
    }

    /// Whether active modal `i` is a menu modal (`role == "menu"` on itself or a direct child),
    /// excluding dialogs (`"dialog"`) and select listboxes (`"listbox"`).
    fn is_menu_modal(&self, i: usize) -> bool {
        let menu = |j: usize| self.tree.nodes[j].role == "menu";
        menu(i) || (i + 1..self.tree.nodes.len())
            .take_while(|&j| self.tree.nodes[j].parent.is_some_and(|p| p >= i))
            .any(|j| self.tree.nodes[j].parent == Some(i) && menu(j))
    }

    /// The live modal that becomes active if `modal` closes, if any (for nested-modal repost checks).
    pub(super) fn outer_modal_path(&self, modal: usize) -> Option<String> {
        self.tree.nodes.iter().enumerate().rev()
            .find(|&(j, n)| j != modal && n.handlers.get().is_some_and(|h| h.modal.is_some()) && !self.subtree_exiting(j) && !self.subtree_unmounted(j))
            .map(|(_, n)| n.path.clone())
    }

    /// Reposting is allowed only after `modal` actually closed without a brand-new modal opening
    /// in its place; falling back to an already-open outer modal keeps that outer modal's
    /// `ModalScope` in effect.
    pub(super) fn repost_allowed(&self, modal: &str, outer: Option<&str>) -> bool {
        let current = self.active_modal().map(|i| self.tree.nodes[i].path.as_str());
        current != Some(modal) && (current.is_none() || current == outer)
    }

    /// Under `Cx::web_focus` a right-click focuses the control it's on, as Chromium's mousedown
    /// does, so focus is back on it when its menu closes.
    fn secondary_focus_control(&self, i: usize) -> Option<String> {
        if !self.commit.defaults.web_focus { return None; }
        let c = std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent)
            .find(|&j| self.can_focus(j) && !self.tree.nodes[j].focus.keyboard_only)?;
        self.tree.nodes[c].input.is_none().then(|| self.tree.nodes[c].path.clone())
    }

    /// Capture a secondary gesture without activating primary actions or text selection.
    pub fn secondary_down(&mut self, x: f32, y: f32) {
        self.secondary_down_for(x, y, SecondaryPlatform::CURRENT);
    }

    pub(super) fn secondary_down_for(&mut self, x: f32, y: f32, platform: SecondaryPlatform) {
        let (x, y) = self.locked_point(x, y);
        let starts_gesture = !self.pointer.any_button_down() && !self.pointer.captured();
        self.pointer.note_button_down(crate::PointerButton::Secondary);
        self.pointer.pos = Some((x, y));
        if self.pointer.autoscroll_press(crate::PointerButton::Secondary) { return; }
        if starts_gesture { self.queue_any_press(crate::PointerButton::Secondary, x, y); }
        self.release_hover_hold();
        self.kbd.focus.step(focus_machine::FocusEvent::Pointed);
        self.dismiss_help();
        if let Some(i) = self.active_modal()
            && !self.point_in_modal(i, x, y)
        {
            self.pointer.clear_tint();
            let can_dismiss_outside = self.tree.nodes[i].handlers.modal.as_ref()
                .is_some_and(|m| m.outside && m.dismiss.is_some());
            match outside_secondary_policy(platform, self.is_menu_modal(i), can_dismiss_outside) {
                OutsideSecondary::Swallow => {
                    self.pointer.set_secondary(capture::Secondary::Dismissed);
                }
                OutsideSecondary::DismissSwallow => {
                    self.frame.queue.push(Action::Dismiss(self.tree.nodes[i].path.clone()));
                    self.pointer.set_secondary(capture::Secondary::Dismissed);
                }
                OutsideSecondary::DismissRepost => {
                    let modal = self.tree.nodes[i].path.clone();
                    let outer = self.outer_modal_path(i);
                    self.frame.queue.push(Action::Dismiss(modal.clone()));
                    self.pointer.set_secondary(capture::Secondary::DismissRepost { modal, outer, down: (x, y), left: false });
                }
            }
            return;
        }
        self.tint_down(x, y, crate::PointerButton::Secondary);
        let secondary_drag = self.secondary_drag_down(x, y);
        let path = self.secondary_target(x, y).map(str::to_owned);
        let i = path.as_ref().and_then(|p| self.tree.nodes.iter().position(|n| n.path == *p));
        self.pointer.set_secondary(match path {
            Some(path) => capture::Secondary::Held { path, link: i.and_then(|i| self.secondary_link_kind_at(i, x, y)), left: false },
            None => capture::Secondary::Up,
        });
        self.secondary_region_down(x, y, self.pointer.secondary_held() || secondary_drag);
        if let Some(control) = i.and_then(|i| self.secondary_focus_control(i)) {
            self.frame.queue.push(Action::FocusControl(control));
        }
    }

    /// Starts a secondary-button drag when no press is already held and an allowed, non-exiting
    /// ancestor under (x, y) opted into `PointerButton::Secondary` via [`El::pointer_buttons`]
    /// and has `on_drag`.
    fn secondary_drag_down(&mut self, x: f32, y: f32) -> bool {
        if self.pointer.press.is_held() { return false; }
        let hit = self.hit.top_hit(|id, r| self.hit_contains(id, r, x, y))
            .and_then(|id| self.tree.node_index.get(&id).copied());
        let Some(di) = std::iter::successors(hit, |&i| self.tree.nodes[i].parent)
            .take_while(|&i| !self.subtree_exiting(i) && self.allowed(i))
            .find(|&i| self.drags_with(i, crate::PointerButton::Secondary))
        else {
            return false;
        };
        let n = &self.tree.nodes[di];
        let r = n.target;
        let [lx, ly] = self.to_local(di, x, y);
        let d = Drag {
            phase: Phase::Down,
            x: lx,
            y: ly,
            w: r[2] - r[0],
            h: r[3] - r[1],
            start_x: lx,
            start_y: ly,
            dx: 0.0,
            dy: 0.0,
            data: 0.0,
            clicks: 1,
            cancelled: false,
            rtl: n.flow == crate::el::Flow::Rtl,
            window: [x, y],
            screen: self.screen_at(x, y),
            edge: None,
        };
        let path = n.path.clone();
        let drag_handler = n.handlers.on_drag.clone();
        if drag_handler.is_some() {
            self.frame.queue.push(Action::Drag(path.clone(), d, crate::PointerButton::Secondary));
        }
        self.pointer.clear_press_clicks();
        let now = self.now_clock();
        self.pointer.press.hold(
            capture::Held::new(path, d, crate::PointerButton::Secondary, (x, y), now, false, None)
                .drag_handler(drag_handler),
        );
        true
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
        if self.pointer.press.held().is_some_and(|h| h.button == crate::PointerButton::Secondary) {
            return "app";
        }
        let (path, link) = match self.pointer.secondary() {
            capture::Secondary::Dismissed => return "app",
            capture::Secondary::Held { path, link, .. } => (path, link),
            capture::Secondary::Up | capture::Secondary::DismissRepost { .. } => return "",
        };
        let Some(i) = self.live(path) else { return "" };
        let n = &self.tree.nodes[i];
        if n.input.is_some() && self.node_disabled(i) { return ""; }
        if matches!(link, Some(capture::SecondaryLink::Explicit(_))) || n.handlers.on_secondary_press.is_some() {
            return "app";
        }
        if matches!(link, Some(capture::SecondaryLink::Default(_))) {
            return if self.is_selectable(i) { "text" } else { "" };
        }
        // On a misspelled word or a pill with a secondary handler, Rune's own menu wins over the browser's.
        if let Some(input) = n.input.as_deref()
            && input.access.is_some()
        {
            let on_pill = input.on_pill_secondary.is_some()
                && self.pointer.pos.and_then(|(x, y)| self.input_pill_at(i, x, y)).is_some();
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
        if !input.spellcheck || input.edit.is_secure() || self.is_node_read_only(i) { return false; }
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
        let (x, y) = self.locked_point(x, y);
        self.pointer.note_button_up(crate::PointerButton::Secondary);
        let mut drag_left = false;
        let dragged = if self.pointer.press.held().is_some_and(|h| h.button == crate::PointerButton::Secondary) {
            drag_left = self.pointer.press.held().is_some_and(|h| h.left);
            self.release(app, x, y)
        } else {
            false
        };
        self.pointer.pos = Some((x, y));
        self.pointer.release_tint(crate::PointerButton::Secondary);
        if self.pointer.autoscroll_release(crate::PointerButton::Secondary) { return; }
        self.secondary_region_up(x, y);
        let (path, down_link, left) = match self.pointer.take_secondary() {
            capture::Secondary::Held { path, link, left } => (Some(path), link, left),
            capture::Secondary::Up | capture::Secondary::Dismissed => (None, None, false),
            capture::Secondary::DismissRepost { modal, outer, down, left } => {
                let dismiss_pending = self.frame.queue.iter().any(|a| matches!(a, Action::Dismiss(p) if *p == modal));
                if dismiss_pending || !self.tree.built || self.commit.dirty {
                    self.frame.queue.push(Action::SecondaryRepost { modal, outer, down, up: (x, y) });
                } else if self.repost_allowed(&modal, outer.as_deref()) {
                    self.queue_secondary_repost(app, down, (x, y));
                }
                self.leave_after_release(left || drag_left);
                return;
            }
        };
        if !dragged
            && let Some(path) = path
            && self.secondary_target(x, y) == Some(path.as_str())
        {
            let i = self.tree.nodes.iter().position(|n| n.path == path);
            let point = i.map_or(
                PointerEvent { x, y, window_x: x, window_y: y, pressing: self.pointer.any_button_down() },
                |i| self.pointer_event(i, x, y),
            );
            let link = i.and_then(|i| self.secondary_link_kind_at(i, x, y));
            // Like a primary link press, it must end on the link it began on; one that moved
            // onto, off or between links opens nothing.
            match (link, down_link) {
                (Some(capture::SecondaryLink::Explicit(k)), Some(capture::SecondaryLink::Explicit(d))) if k == d => {
                    self.frame.queue.push(Action::LinkSecondary(path, k, point));
                }
                (Some(capture::SecondaryLink::Default(k)), Some(capture::SecondaryLink::Default(d))) if k == d => {
                    self.open_link_menu(app, &path, k, x, y, Some((x, y)));
                }
                (None, None) if !self.selection_menu(app, &path, x, y) && !self.queue_field_menu(&path, x, y) => {
                    self.frame.queue.push(Action::SecondaryPress(path, point));
                }
                _ => {}
            }
        }
        self.leave_after_release(left || drag_left);
    }

    /// Resolves a reposted secondary click (`down` -> `up`) on the live, post-dismiss tree.
    fn resolve_repost_target(&self, (dx, dy): (f32, f32), (ux, uy): (f32, f32)) -> Option<(usize, String, Option<capture::SecondaryLink>, PointerEvent)> {
        let path = self.secondary_target(dx, dy)?.to_owned();
        if self.secondary_target(ux, uy) != Some(path.as_str()) { return None; }
        let i = self.live(&path)?;
        let down_link = self.secondary_link_kind_at(i, dx, dy);
        let up_link = self.secondary_link_kind_at(i, ux, uy);
        let link = match (up_link, down_link) {
            (Some(k), Some(d)) if k == d => Some(k),
            (None, None) => None,
            _ => return None,
        };
        Some((i, path, link, self.pointer_event(i, ux, uy)))
    }

    /// Cross-frame repost: dismissal already committed and rebuilt hit-testing, so queue the
    /// target's secondary action directly on release.
    fn queue_secondary_repost(&mut self, app: &dyn ViewDriver, down: (f32, f32), up: (f32, f32)) {
        let Some((i, path, link, point)) = self.resolve_repost_target(down, up) else { return };
        if let Some(control) = self.secondary_focus_control(i) {
            self.frame.queue.push(Action::FocusControl(control));
        }
        match link {
            Some(capture::SecondaryLink::Explicit(k)) => self.frame.queue.push(Action::LinkSecondary(path, k, point)),
            Some(capture::SecondaryLink::Default(k)) => { self.open_link_menu(app, &path, k, up.0, up.1, Some(up)); }
            None if !self.selection_menu(app, &path, up.0, up.1) && !self.queue_field_menu(&path, up.0, up.1) => {
                self.frame.queue.push(Action::SecondaryPress(path, point));
            }
            None => {}
        }
    }

    /// Same-frame repost: runs after `Action::Dismiss` commits so `ModalScope` and
    /// `register_node_hits` are freshly rebuilt for the post-dismiss tree.
    pub(super) fn run_secondary_repost(
        &mut self,
        app: &mut dyn AppDriver,
        modal: &str,
        outer: Option<&str>,
        down: (f32, f32),
        up: (f32, f32),
        one: &mut Tx,
    ) -> bool {
        if !self.repost_allowed(modal, outer) { return false; }
        if !self.tree.built {
            self.scene.present();
            self.begin_exit_mask();
            self.build();
            self.end_exit_mask();
        }
        let Some((i, path, link, point)) = self.resolve_repost_target(down, up) else { return false };
        if let Some(control) = self.secondary_focus_control(i) {
            self.set_focus(app, Some(control), false);
        }
        match link {
            Some(capture::SecondaryLink::Explicit(k)) => self.secondary_press_link(app, &path, k, point, one),
            Some(capture::SecondaryLink::Default(k)) => self.open_link_menu(app, &path, k, up.0, up.1, Some(up)),
            None => {
                if self.selection_menu(app, &path, up.0, up.1) { return true; }
                if self.tree.nodes[i].handlers.on_secondary_press.is_none() && self.editable_field_menu(i) {
                    return self.field_menu(app, path, up, Some(up), one);
                }
                let Some(f) = self.tree.nodes[i].handlers.on_secondary_press.clone() else { return false };
                log_event!(self, "secondary press {path}");
                f(app.states(), point, one);
                true
            }
        }
    }

    /// A secondary press at (x, y) on text field `path` with no `on_secondary_press` of its own:
    /// its menu (`field_menu`), queued.
    fn queue_field_menu(&mut self, path: &str, x: f32, y: f32) -> bool {
        let Some(i) = self.live(path) else { return false };
        if self.tree.nodes[i].handlers.on_secondary_press.is_some() || !self.editable_field_menu(i) { return false; }
        self.frame.queue.push(Action::FieldMenu(path.to_owned(), (x, y), Some((x, y))));
        true
    }

    /// The Menu key, or Shift+F10, that nothing before it took: the focused element's context
    /// menu. That is its focused link's `on_link_secondary`, else its active descendant's
    /// `on_secondary_press` when mounted (a tree or list keeping focus on its container), else
    /// its own or its nearest ancestor's `on_secondary_press`, at the bottom-left of that
    /// element, where a menu opens below it as AppKit's and Windows' keyboard menus do.
    pub(in crate::runtime) fn context_menu_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let m = ev.mods;
        let plain = !m.cmd && !m.ctrl && !m.alt && !m.logo;
        let menu_key = match ev.key {
            crate::Key::ContextMenu => plain,
            crate::Key::Function(10) => plain && m.shift,
            _ => false,
        };
        let Some(focused) = self.focused_index().filter(|&i| menu_key && self.can_focus(i)) else { return false };
        if self.focused_link_menu(app, focused, tx) || self.focused_link_default_menu(app, focused) { return true; }
        let [fx0, fy0, fx1, fy1] = self.drawn_box(focused);
        if let Some(active) = self.a11y_active_descendant(focused) {
            let inside = std::iter::successors(self.tree.nodes[active].parent, |&p| self.tree.nodes[p].parent).any(|p| p == focused);
            let disabled_input = self.tree.nodes[focused].input.is_some() && self.node_disabled(focused);
            if !disabled_input
                && (inside || self.tree.nodes[focused].input.is_some())
                && self.allowed(active)
                && let Some(f) = self.tree.nodes[active].handlers.on_secondary_press.clone()
            {
                let drawn = self.paint.drawn.get(active).is_some_and(|d| d[2] > 0.0 || d[3] > 0.0);
                let [mut x, _, _, mut y] = self.drawn_box(if drawn { active } else { focused });
                if inside && drawn {
                    x = x.clamp(fx0, fx1);
                    y = y.clamp(fy0, fy1);
                }
                let path = self.tree.nodes[active].path.clone();
                let point = self.pointer_event(active, x, y);
                log_event!(self, "secondary press {path}");
                f(app.states(), point, tx);
                return true;
            }
        }
        let (x, y) = (fx0, fy1);
        if self.tree.nodes[focused].handlers.on_secondary_press.is_none() && self.editable_field_menu(focused) {
            let path = self.tree.nodes[focused].path.clone();
            return self.field_menu(app, path, (x, y), None, tx);
        }
        let mut index = focused;
        loop {
            let n = &self.tree.nodes[index];
            if !self.allowed(index) { return false; }
            let disabled_input = n.input.is_some() && self.node_disabled(index);
            if !disabled_input && let Some(f) = n.handlers.on_secondary_press.clone() {
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
