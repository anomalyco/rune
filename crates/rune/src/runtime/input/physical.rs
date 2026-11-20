use super::*;
use crate::{PhysicalInput, PhysicalKeyCode};

pub(in crate::runtime) struct Fallback { key: KeyEvent, base: Option<char>, unhandled: Option<Unhandled> }

/// What a window does with a logical key no handler took: close or minimize as its title-bar
/// button would, or run a menu item. A standard ⌘W or ⌘M item, or an app's item marked
/// `MenuItem::keys_first`, asks for it, so an app's own binding wins. On Windows, Alt+Space
/// opens the window menu.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone)]
pub(crate) enum Unhandled { Close, Minimize, Menu(std::sync::Arc<dyn Fn() + Send + Sync>), WindowMenu }

impl std::fmt::Debug for Unhandled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Self::Close => "Close", Self::Minimize => "Minimize", Self::Menu(_) => "Menu", Self::WindowMenu => "WindowMenu" })
    }
}

/// Whether an activation key (Enter, Escape, Space) is held down: its physical code (native and
/// physical input) or logical key (`Runtime::key`), and the focus its first press landed on.
#[derive(Default)]
pub(in crate::runtime) enum Activation {
    #[default]
    Up,
    Held { physical: Option<PhysicalKeyCode>, key: Option<crate::Key>, focus: Option<String> },
}

impl focus_machine::KeyboardState {
    /// Remember an activation key's first press and the focus it landed on; true drops a
    /// repeat of that same key once focus has moved. Any other first press replaces it.
    pub(super) fn held_gate(&mut self, physical: Option<PhysicalKeyCode>, key: Option<&crate::Key>, repeat: bool) -> bool {
        use crate::{Key, PhysicalKeyCode as Code};
        if !repeat {
            // The OS keeps repeating a held key through modifier presses.
            if physical.is_some_and(|c| matches!(c, Code::ShiftLeft | Code::ShiftRight | Code::ControlLeft
                | Code::ControlRight | Code::AltLeft | Code::AltRight | Code::SuperLeft | Code::SuperRight
                | Code::CapsLock | Code::Fn)) || key.is_some_and(|k| matches!(k, Key::Modifier { .. })) { return false; }
            let activation = physical.is_some_and(|c| matches!(c, Code::Enter | Code::NumpadEnter | Code::Escape | Code::Space))
                || key.is_some_and(|k| matches!(k, Key::Enter | Key::Escape) || matches!(k, Key::Text(t) if t == " "));
            self.held = if activation { Activation::Held { physical, key: key.cloned(), focus: self.focus.owned() } } else { Activation::Up };
            return false;
        }
        let Activation::Held { physical: held, key: held_key, focus } = &self.held else { return false };
        let same = match physical { Some(code) => *held == Some(code), None => held_key.as_ref() == key };
        same && focus.as_deref() != self.focus.path()
    }

    #[inline]
    pub(super) fn clear_held(&mut self) {
        self.held = Activation::Up;
    }

    #[inline]
    pub(super) fn release_held(&mut self, key: PhysicalKeyCode) {
        if matches!(&self.held, Activation::Held { physical, .. } if *physical == Some(key)) {
            self.held = Activation::Up;
        }
    }
}

impl RuntimeCore {
    /// Queue physical input without a logical fallback (also useful for headless drivers).
    /// A modifier key still gets its `Key::Modifier` when modifier keys are on.
    pub(crate) fn physical_input(&mut self, event: PhysicalInput) {
        self.observe_mods(&event);
        if self.cancel_drag_key(&event) { return; }
        if matches!(event, PhysicalInput::Key { pressed: true, .. }) && self.pointer.autoscroll_key() { return; }
        self.frame.queue.push(Action::Physical(event, None));
    }

    fn cancel_drag_key(&mut self, event: &PhysicalInput) -> bool {
        matches!(event, PhysicalInput::Key { key: PhysicalKeyCode::Escape, pressed: true, .. })
            && self.cancel_drag()
    }
    /// Shift held now decides whether the next press extends a text selection, ahead of the
    /// queue the press itself skips. A key press hides a tooltip.
    fn observe_mods(&mut self, event: &PhysicalInput) {
        match event {
            PhysicalInput::Modifiers(m) => { self.edit.set_shift(m.shift); self.kbd.held_mods = *m; }
            PhysicalInput::FocusLost => { self.edit.set_shift(false); self.kbd.held_mods = Default::default(); self.dismiss_help(); }
            PhysicalInput::Key { pressed: true, mods, .. } => { self.kbd.held_mods = *mods; self.dismiss_help(); }
            PhysicalInput::Key { mods, .. } => self.kbd.held_mods = *mods,
        }
    }
    /// Queue a key press with its physical code and logical key, as a native host sends them
    /// (a script's `{"type": "key", "code": …}`), and its `Tx::key_base`.
    pub(crate) fn physical_key_input(&mut self, app: &mut dyn AppDriver, event: PhysicalInput, key: KeyEvent, base: Option<char>) {
        self.observe_mods(&event);
        self.physical_then(app, event, Some(key), base, None);
    }
    /// Return whether the native host needs to request a frame for this event.
    #[cfg_attr(any(target_os = "windows", target_os = "ios", target_os = "android"), allow(dead_code))]
    pub(crate) fn physical_with_fallback(&mut self, app: &mut dyn AppDriver, event: PhysicalInput, key: Option<KeyEvent>) -> bool {
        self.physical_then(app, event, key, None, None)
    }
    /// `physical_with_fallback`, with the key's unmodified character (`Tx::key_base`), and
    /// `unhandled` happens when no handler takes the logical key.
    pub(crate) fn physical_then(&mut self, app: &mut dyn AppDriver, event: PhysicalInput, key: Option<KeyEvent>, base: Option<char>, unhandled: Option<Unhandled>) -> bool {
        if self.cancel_drag_key(&event) { return true; }
        // A key ends a middle-click autoscroll, and does nothing else.
        if matches!(event, PhysicalInput::Key { pressed: true, .. }) && self.pointer.autoscroll_key() { return true; }
        if key.is_none() && matches!(event, PhysicalInput::Modifiers(_) | PhysicalInput::Key { pressed: false, .. }) {
            return self.physical_observation(app, event);
        }
        if !matches!(event, PhysicalInput::Key { pressed: false, .. }) { self.dismiss_help(); }
        self.frame.queue.push(Action::Physical(event, key.map(|key| Fallback { key, base, unhandled })));
        true
    }
    /// Modifier/release events have no logical action. When input is already queued,
    /// its requested frame will process this observation in order after clicks/keys.
    /// At rest, only a consuming handler invalidates the view and requests a frame.
    pub(crate) fn physical_observation(&mut self, app: &mut dyn AppDriver, event: PhysicalInput) -> bool {
        let busy = !self.frame.queue.is_empty();
        self.observe_mods(&event);
        // The release of a modifier key the app hears needs a frame, in order like a press.
        // So does one while state changed since the last frame (a covered window keeps it
        // changed until it uncovers): the focused handler is the new view's.
        if self.kbd.modifier_pending(&event) || busy || self.commit.dirty {
            self.frame.queue.push(Action::Physical(event, None));
            return !busy;
        }
        let mut tx = Tx::default();
        let consumed = self.dispatch_physical(app, event, None, &mut tx);
        let live_only = std::mem::take(&mut tx.live_only);
        self.latch_requests(app, &mut tx);
        merge(&mut self.frame.pending, tx);
        if consumed && live_only {
            self.frame.live_only_skips += 1;
            self.frame.live.set();
        } else {
            self.commit.dirty |= consumed;
        }
        consumed
    }
    pub(in crate::runtime) fn dispatch_physical(&mut self, app: &mut dyn AppDriver, event: PhysicalInput, fallback: Option<Fallback>, tx: &mut Tx) -> bool {
        tx.by_keys = true;
        use crate::PhysicalKeyCode as Code;
        let is_mod = |c: Code| matches!(c, Code::ShiftLeft | Code::ShiftRight | Code::ControlLeft
            | Code::ControlRight | Code::AltLeft | Code::AltRight | Code::SuperLeft | Code::SuperRight
            | Code::CapsLock | Code::Fn);
        match event {
            PhysicalInput::Key { key, pressed: true, repeat, .. } => {
                if self.kbd.held_gate(Some(key), fallback.as_ref().map(|f| &f.key.key), repeat) { return false; }
                if !is_mod(key) { self.note_key_down(app, repeat); }
            }
            PhysicalInput::Key { key, pressed: false, .. } => {
                self.kbd.release_held(key);
                if fallback.is_none() && !is_mod(key) && self.composing(app) {
                    self.kbd.composing_release();
                }
            }
            PhysicalInput::FocusLost => {
                self.kbd.key_releases_lost();
            }
            PhysicalInput::Modifiers(_) => {}
        }
        let repeat = matches!(event, PhysicalInput::Key { repeat: true, .. });
        let code = match event { PhysicalInput::Key { key, .. } => Some(key), _ => None };
        // Live-only when every handler that took the event said so (`Tx::live_only`).
        let mut live = None;
        // Ctrl+Shift's direction chord sees every key, whoever takes it.
        let chord = self.direction_chord(app, &event, tx);
        // It changes the field, so the view builds again whatever else is live-only.
        if chord { live = Some(false); }
        let consumed = self.offer_physical(app, &event, tx, &mut live);
        let modifier = self.modifier_dispatch(app, &event, consumed, tx, &mut live);
        let ran = consumed || chord || modifier | match fallback {
            // A release's logical key, for `on_key_up`.
            Some(Fallback { key, base, .. }) if matches!(event, PhysicalInput::Key { pressed: false, .. }) => {
                let mut one = Tx { physical_key: code, key_base: base, ..self.transaction(app) };
                let ran = self.route_key_up(app, &key, &mut one);
                fold_live(&mut live, ran, &one);
                merge(tx, one);
                ran
            }
            Some(Fallback { key, base, unhandled }) => {
                if key.edit_command() == Some('v') && self.accepts_paste() {
                    // Capture the target in event order, before another queued click can
                    // change focus. An empty/image clipboard still consumes the command.
                    self.edit.paste_request = Some(self.paste_target());
                    false
                } else {
                    let mut one = Tx { physical_key: code, key_base: base, ..self.transaction(app) };
                    let ran = self.route_key(app, &key, repeat, &mut one);
                    // A held key repeats a menu item, as AppKit's own equivalents do, but closes once.
                    let unhandled = unhandled.filter(|u| !ran && (!repeat || matches!(u, Unhandled::Menu(_))));
                    let fell = unhandled.is_some();
                    match unhandled {
                        Some(Unhandled::Close) => one.close_window(),
                        Some(Unhandled::Minimize) => one.minimize(),
                        Some(Unhandled::Menu(run)) => run(),
                        Some(Unhandled::WindowMenu) => self.request_window_menu(),
                        None => {}
                    }
                    fold_live(&mut live, ran || fell, &one);
                    merge(tx, one);
                    ran || fell
                }
            }
            None => false,
        };
        let ran = self.dispatch_mods(app, tx, &mut live) || ran;
        tx.live_only = ran && live == Some(true);
        ran
    }
    /// `App::on_modifiers`, when the held modifiers changed since it last heard them.
    fn dispatch_mods(&mut self, app: &mut dyn AppDriver, tx: &mut Tx, live: &mut Option<bool>) -> bool {
        if !app.has_on_mods() { return false; }
        let Some(mods) = self.kbd.take_mods_changed() else { return false };
        let mut one = self.transaction(app);
        if !app.on_mods(mods, &mut one) { return false; }
        fold_live(live, true, &one);
        merge(tx, one);
        true
    }
    /// The focused element's `on_physical_input` handler; true if it consumed `event`.
    fn offer_physical(&mut self, app: &mut dyn AppDriver, event: &PhysicalInput, tx: &mut Tx, live: &mut Option<bool>) -> bool {
        let Some(handler) = self.kbd.focus.path().and_then(|p| self.node(p))
            .and_then(|n| n.focus.physical.clone()) else { return false };
        let mut one = Tx { key_repeat: matches!(event, PhysicalInput::Key { repeat: true, .. }), ..self.transaction(app) };
        let consumed = Tx::took(handler(app.states(), event, &mut one), &mut one);
        fold_live(live, consumed, &one);
        merge(tx, one);
        consumed
    }
    /// Focus moving off an element inside the window: its handler hears `FocusLost`, and
    /// modifier keys stay held.
    pub(in crate::runtime) fn physical_focus_lost(&mut self, app: &mut dyn AppDriver) -> bool {
        let mut tx = Tx::default();
        let changed = self.offer_physical(app, &PhysicalInput::FocusLost, &mut tx, &mut None);
        self.latch_requests(app, &mut tx);
        merge(&mut self.frame.pending, tx);
        changed
    }
}

#[cfg(test)]
#[path = "physical_tests.rs"]
mod tests;
