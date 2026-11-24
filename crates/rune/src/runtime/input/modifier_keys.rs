//! Modifier keys pressed and released on their own (`Key::Modifier`), for runtimes that opt in.
//!
//! Hosts already pass every physical key, modifiers included, to `physical_with_fallback`, so
//! the logical event is made here once for windows, overlays, layer surfaces and headless runs.
//! It is made in queue order, once the focused `on_physical_input` handler has declined the
//! physical event, so a shortcut recorder sees modifiers first, as it sees every key.
//! Presses come only from physical keys. A release also comes from a modifier that a
//! `PhysicalInput::Modifiers` turns off while its key is down (a key-up the platform never
//! sent), and every held key is released on `FocusLost`.

use super::*;
use crate::{Key, Modifier, Mods, PhysicalInput, PhysicalKeyCode as Code};

const ALL: [Modifier; 9] = [Modifier::ShiftLeft, Modifier::ShiftRight, Modifier::ControlLeft, Modifier::ControlRight,
    Modifier::AltLeft, Modifier::AltRight, Modifier::SuperLeft, Modifier::SuperRight, Modifier::Fn];

#[derive(Default)]
pub(in crate::runtime) struct ModifierKeys {
    pub(in crate::runtime) enabled: bool,
    /// How far Ctrl+Shift's direction chord has gone (`edit::direction_keys`), whatever the
    /// app asked for and whoever took the keys.
    chord: crate::edit::DirChord,
    /// Test override for whether an RTL keyboard layout is installed (`None` queries the OS).
    #[cfg(test)]
    pub(super) rtl_layout_override: Option<bool>,
    /// Bit `i` set while `ALL[i]` is down.
    held: u16,
    /// The last `PhysicalInput::Modifiers`, to see a flag turn off. A flag that was never on
    /// says nothing: AltGr (Right Alt on many layouts) is down without `alt` on X11 and Wayland.
    last: Mods,
}

fn modifier(code: Code) -> Option<Modifier> {
    Some(match code {
        Code::ShiftLeft => Modifier::ShiftLeft,
        Code::ShiftRight => Modifier::ShiftRight,
        Code::ControlLeft => Modifier::ControlLeft,
        Code::ControlRight => Modifier::ControlRight,
        Code::AltLeft => Modifier::AltLeft,
        Code::AltRight => Modifier::AltRight,
        Code::SuperLeft => Modifier::SuperLeft,
        Code::SuperRight => Modifier::SuperRight,
        Code::Fn => Modifier::Fn,
        _ => return None,
    })
}

fn bit(m: Modifier) -> u16 { 1 << ALL.iter().position(|&a| a == m).unwrap_or(0) }

/// Both sides of `m`'s modifier.
fn pair(m: Modifier) -> u16 {
    use Modifier::*;
    match m {
        ShiftLeft | ShiftRight => bit(ShiftLeft) | bit(ShiftRight),
        ControlLeft | ControlRight => bit(ControlLeft) | bit(ControlRight),
        AltLeft | AltRight => bit(AltLeft) | bit(AltRight),
        SuperLeft | SuperRight => bit(SuperLeft) | bit(SuperRight),
        _ => bit(m),
    }
}

/// Whether `mods` has `m`'s modifier on. Fn has no flag.
fn active(mods: Mods, m: Modifier) -> bool {
    use Modifier::*;
    match m {
        ShiftLeft | ShiftRight => mods.shift,
        ControlLeft | ControlRight => mods.ctrl,
        AltLeft | AltRight => mods.alt,
        SuperLeft | SuperRight => mods.logo,
        _ => false,
    }
}

/// `mods` with `m`'s modifier set to `on`, including `cmd` (Command on macOS, Ctrl elsewhere).
fn with(mut mods: Mods, m: Modifier, on: bool) -> Mods {
    use Modifier::*;
    let mac = cfg!(target_os = "macos");
    match m {
        ShiftLeft | ShiftRight => mods.shift = on,
        ControlLeft | ControlRight => { mods.ctrl = on; if !mac { mods.cmd = on; } }
        AltLeft | AltRight => mods.alt = on,
        SuperLeft | SuperRight => { mods.logo = on; if mac { mods.cmd = on; } }
        _ => {}
    }
    mods
}

/// Windows clears Control and Alt in the flags while AltGr (Right Alt) is down, even with
/// Left Control or Left Alt held.
const ALTGR_CLEARS_FLAGS: bool = cfg!(target_os = "windows");

impl ModifierKeys {
    fn event(&self, m: Modifier, pressed: bool, mods: Mods) -> KeyEvent {
        // Platforms differ on whether a modifier's own key event carries the flags from
        // before or after it; report them after, from the keys held.
        KeyEvent { key: Key::Modifier { modifier: m, pressed }, mods: with(mods, m, self.held & pair(m) != 0) }
    }

    /// The keys `event` presses and releases, without changing anything. A press while held,
    /// a release not held and a repeat change nothing. `Modifiers` releases held keys whose
    /// flag turns off (Fn has none, so only its key-up or focus loss ends it); `FocusLost`
    /// releases them all.
    fn changes(&self, event: &PhysicalInput, altgr_clears: bool) -> (u16, u16) {
        if !self.enabled { return (0, 0); }
        match *event {
            PhysicalInput::Key { key, pressed, repeat, .. } => match modifier(key) {
                Some(m) if !repeat && (self.held & bit(m) != 0) != pressed => if pressed { (bit(m), 0) } else { (0, bit(m)) },
                _ => (0, 0),
            },
            PhysicalInput::Modifiers(now) => {
                let altgr = altgr_clears && self.held & bit(Modifier::AltRight) != 0;
                let lifted = ALL.into_iter().filter(|&m| {
                    use Modifier::*;
                    let cleared = altgr && matches!(m, ControlLeft | ControlRight | AltLeft | AltRight);
                    self.held & bit(m) != 0 && active(self.last, m) && !active(now, m) && !cleared
                });
                (0, lifted.fold(0, |all, m| all | bit(m)))
            }
            PhysicalInput::FocusLost => (0, self.held),
        }
    }

    /// Whether `event` presses or releases a modifier key.
    fn pending(&self, event: &PhysicalInput) -> bool { self.changes(event, ALTGR_CLEARS_FLAGS) != (0, 0) }

    /// Takes `event` in, with the `Key::Modifier` events it makes. None when a physical handler
    /// `consumed` it: a press it took is never held, so its release is never sent either, and a
    /// release or focus loss it took ends held keys silently. That is the one way `on_key` sees
    /// a press without its release: a handler, such as a shortcut recorder that starts while a
    /// modifier is down, takes the key-up.
    fn apply(&mut self, event: &PhysicalInput, consumed: bool, altgr_clears: bool) -> Vec<KeyEvent> {
        let (pressed, released) = self.changes(event, altgr_clears);
        let mods = match *event {
            PhysicalInput::Key { mods, .. } => mods,
            PhysicalInput::Modifiers(now) => { self.last = now; now }
            PhysicalInput::FocusLost => { self.last = Mods::default(); Mods::default() }
        };
        let mut out = Vec::new();
        for m in ALL {
            if pressed & bit(m) != 0 && !consumed {
                self.held |= bit(m);
                out.push(self.event(m, true, mods));
            } else if released & bit(m) != 0 {
                self.held &= !bit(m);
                if !consumed { out.push(self.event(m, false, mods)); }
            }
        }
        out
    }
}

impl focus_machine::KeyboardState {
    /// Send `Key::Modifier` presses and releases to `on_key` and the focused input's key
    /// handler. Off by default: handlers written before it may act on any key.
    #[inline]
    pub(in crate::runtime) fn set_modifier_keys(&mut self, enabled: bool) {
        if self.modifier_keys.enabled != enabled { self.modifier_keys.held = 0; }
        self.modifier_keys.enabled = enabled;
    }

    #[inline]
    pub(in crate::runtime) fn modifier_keys(&self) -> bool { self.modifier_keys.enabled }

    /// Whether `event` presses or releases a modifier key, which needs a frame to deliver.
    #[inline]
    pub(super) fn modifier_pending(&self, event: &PhysicalInput) -> bool { self.modifier_keys.pending(event) }

    #[inline]
    pub(super) fn disarm_direction_chord(&mut self) {
        self.modifier_keys.chord = crate::edit::DirChord::Idle;
    }

    fn has_rtl_layout(&self) -> bool {
        #[cfg(test)]
        if let Some(has) = self.modifier_keys.rtl_layout_override {
            return has;
        }
        crate::platform::ime::has_rtl_keyboard_layout()
    }

    /// Steps Ctrl+Shift's direction chord; returns the flow direction when the release completes it.
    pub(super) fn step_direction_chord(&mut self, event: &PhysicalInput) -> Option<crate::el::Flow> {
        match *event {
            PhysicalInput::FocusLost => {
                self.disarm_direction_chord();
                None
            }
            PhysicalInput::Modifiers(m) => {
                if m.alt || m.logo {
                    self.disarm_direction_chord();
                }
                None
            }
            PhysicalInput::Key { key, mods, pressed, repeat } => {
                let Some(m) = modifier(key) else {
                    if pressed {
                        self.disarm_direction_chord();
                    }
                    return None;
                };
                if repeat {
                    return None;
                }
                let after = with(mods, m, pressed);
                if after.alt || after.logo || (pressed && m == Modifier::Fn) {
                    self.disarm_direction_chord();
                    return None;
                }
                let (next, set) = crate::edit::step_dir_chord(self.modifier_keys.chord, m, pressed, after.ctrl, after.shift);
                self.modifier_keys.chord = next;
                set
            }
        }
    }

    #[inline]
    pub(super) fn apply_modifier_keys(&mut self, event: &PhysicalInput, consumed: bool) -> Vec<KeyEvent> {
        self.modifier_keys.apply(event, consumed, ALTGR_CLEARS_FLAGS)
    }
}

impl RuntimeCore {
    /// Send `Key::Modifier` presses and releases to `on_key` and the focused input's key
    /// handler. Off by default: handlers written before it may act on any key.
    #[inline]
    pub(crate) fn set_modifier_keys(&mut self, enabled: bool) { self.kbd.set_modifier_keys(enabled); }

    #[inline]
    pub(crate) fn modifier_keys(&self) -> bool { self.kbd.modifier_keys() }

    /// Ctrl+Left Shift and Ctrl+Right Shift (Windows with an RTL keyboard layout installed,
    /// matching Chrome): every physical key steps the chord (`edit::direction_keys`), and its
    /// release sets the focused field's direction. Any other key, modifier or focus loss disarms
    /// it. True if it set one.
    pub(in crate::runtime) fn direction_chord(&mut self, app: &mut dyn AppDriver, event: &PhysicalInput, tx: &mut Tx) -> bool {
        let Some(flow) = self.kbd.step_direction_chord(event)
            .filter(|_| self.focused_input().is_some() && self.kbd.has_rtl_layout())
        else { return false };
        self.with_focused(app, |e| { e.force_dir(flow); crate::edit::EditOutcome::default() }, tx)
    }

    /// After the focused physical handler saw `event` (and `consumed` it or not): route the
    /// modifier keys it presses and releases. True if a handler took one.
    pub(in crate::runtime) fn modifier_dispatch(&mut self, app: &mut dyn AppDriver, event: &PhysicalInput, consumed: bool, tx: &mut Tx, live: &mut Option<bool>) -> bool {
        let mut ran = false;
        let code = match *event { PhysicalInput::Key { key, .. } => Some(key), _ => None };
        for ev in self.kbd.apply_modifier_keys(event, consumed) {
            let mut one = self.transaction(app);
            one.physical_key = code;
            let took = self.route_modifier(app, &ev, &mut one);
            fold_live(live, took, &one);
            ran |= took;
            merge(tx, one);
        }
        ran
    }

    /// A modifier key goes to the app and the focused input's key handler, in `route_key`'s
    /// order, and to nothing built in: no editing, selection, focus traversal or focus ring.
    pub(in crate::runtime) fn route_modifier(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
        if !self.kbd.modifier_keys() { return false; }
        one.key_repeat = false;
        let app_first = self.kbd.focus.path().is_none();
        if app_first && app.has_on_key() && Tx::took(app.on_key(ev, one), one) { return true; }
        if self.run_input_key(app, ev, one) {
            return true;
        }
        if !app_first && app.has_on_key() && Tx::took(app.on_key(ev, one), one) {
            log_event!(self, "key {:?}", ev.key);
            return true;
        }
        matches!(ev.key, Key::Modifier { pressed: false, .. }) && self.route_key_up(app, ev, one)
    }
}

#[cfg(test)]
#[path = "modifier_keys_tests.rs"]
mod tests;
