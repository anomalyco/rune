//! The layer surface's own state, kept out of `platform/layer_shell.rs` (which only builds on
//! unix) so its tests run headless on every host: which finger the surface has, and how far the
//! compositor has got with saying how big it is.

/// One touch on the surface. It takes one finger at a time: a second finger's events are ignored
/// until the first lifts, so a tap is exactly one press and one release.
pub(crate) mod touch {
    use crate::machine::Effects;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) enum State {
        /// No finger down.
        #[default]
        Up,
        /// `id`'s finger is down.
        Down { id: i32 },
    }

    /// What the compositor said about a touch.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) enum Event {
        Down { id: i32, x: f32, y: f32 },
        Motion { id: i32, x: f32, y: f32 },
        Up { id: i32 },
        /// The compositor took the touch for a system gesture of its own.
        Cancel,
        /// The seat went away: nothing more arrives for this finger.
        Gone,
    }

    /// What a touch means for the surface's input, in the order it means it.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) enum Effect {
        Move(f32, f32),
        Press,
        Release,
        Leave,
    }

    /// The finger after `event`, and the input the caller queues.
    pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
        match (*state, event) {
            (State::Up, Event::Down { id, x, y }) => {
                *state = State::Down { id };
                Effects::two(Effect::Move(x, y), Effect::Press)
            }
            (State::Down { id }, Event::Motion { id: moving, x, y }) if id == moving => Effects::one(Effect::Move(x, y)),
            (State::Down { id }, Event::Up { id: lifted }) if id == lifted => {
                *state = State::Up;
                Effects::two(Effect::Release, Effect::Leave)
            }
            // Leave before releasing, so the gesture isn't a click where the finger landed.
            (State::Down { .. }, Event::Cancel) => {
                *state = State::Up;
                Effects::two(Effect::Leave, Effect::Release)
            }
            (_, Event::Gone) => {
                *state = State::Up;
                Effects::none()
            }
            _ => Effects::none(),
        }
    }
}

/// How far the compositor has got with the surface. Nothing can be drawn before its first
/// configure; the size it asks for and the buffer scale it prefers are each read once, by
/// `dispatch`, in whatever order they arrive; and once it closes the surface it is final.
pub(crate) mod readiness {
    /// What the compositor said about the surface.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Event {
        /// The compositor's size for the surface; 0 on a side means the size asked for.
        Configure { width: u32, height: u32 },
        /// The buffer scale it prefers.
        Scale(i32),
        /// It destroyed the surface.
        Closed,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum State {
        /// No configure yet, so there is no size to draw at. A preferred buffer scale may have
        /// arrived first; it is kept for the first read.
        Configuring { scale: Option<i32> },
        /// Configured. `size` and `scale` are what the compositor said that `dispatch` hasn't
        /// read yet.
        Ready { size: Option<(u32, u32)>, scale: Option<i32> },
        /// Destroyed by the compositor: it says nothing more that counts.
        Closed,
    }

    impl Default for State {
        fn default() -> Self { State::Configuring { scale: None } }
    }

    /// The surface after `event`. No effect: what it said is read with the accessors below.
    pub(crate) fn step(state: &mut State, event: Event) {
        match (&*state, event) {
            (State::Closed, _) => {}
            (_, Event::Closed) => *state = State::Closed,
            (State::Configuring { scale }, Event::Configure { width, height }) => *state = State::Ready { size: Some((width, height)), scale: *scale },
            (State::Ready { scale, .. }, Event::Configure { width, height }) => *state = State::Ready { size: Some((width, height)), scale: *scale },
            (State::Configuring { .. }, Event::Scale(scale)) => *state = State::Configuring { scale: Some(scale) },
            (State::Ready { size, .. }, Event::Scale(scale)) => *state = State::Ready { size: *size, scale: Some(scale) },
        }
    }

    impl State {
        /// The compositor configured a size, so the surface can be drawn on.
        pub(crate) fn configured(&self) -> bool { !matches!(self, State::Configuring { .. }) }

        /// The compositor destroyed the surface.
        pub(crate) fn closed(&self) -> bool { matches!(self, State::Closed) }

        /// The size the compositor asked for that `dispatch` hasn't read yet.
        pub(crate) fn size(&self) -> Option<(u32, u32)> {
            match self {
                State::Ready { size, .. } => *size,
                State::Configuring { .. } | State::Closed => None,
            }
        }

        /// Takes the newest size reported since the last read; readiness doesn't depend on it.
        pub(crate) fn take_size(&mut self) -> Option<(u32, u32)> {
            match self {
                State::Ready { size, .. } => size.take(),
                State::Configuring { .. } | State::Closed => None,
            }
        }

        /// Takes the newest buffer scale reported since the last read.
        pub(crate) fn take_scale(&mut self) -> Option<i32> {
            match self {
                State::Configuring { scale } | State::Ready { scale, .. } => scale.take(),
                State::Closed => None,
            }
        }
    }
}

/// Layer-shell keyboard focus transitions, persisted modifier masks, command-chord translation,
/// and `KeyPolicy` capability checks, kept cross-platform so they compile and run tests on every
/// host.
pub(crate) mod keyboard {
    use crate::machine::Effects;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) enum FocusState {
        #[default]
        Unfocused,
        Focused,
    }

    impl FocusState {
        #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
        pub(crate) fn focused(self) -> bool { matches!(self, FocusState::Focused) }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum FocusEvent {
        EnterOurs,
        EnterOther,
        LeaveOurs,
        KeyboardGone,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum FocusEffect {
        Focus(bool),
    }

    /// Transitions keyboard focus and emits `Focus(true)` / `Focus(false)` only on real edges.
    pub(crate) fn step(state: &mut FocusState, event: FocusEvent) -> Effects<FocusEffect> {
        match (*state, event) {
            (FocusState::Unfocused, FocusEvent::EnterOurs) => {
                *state = FocusState::Focused;
                Effects::one(FocusEffect::Focus(true))
            }
            (FocusState::Focused, FocusEvent::EnterOther | FocusEvent::LeaveOurs | FocusEvent::KeyboardGone) => {
                *state = FocusState::Unfocused;
                Effects::one(FocusEffect::Focus(false))
            }
            _ => Effects::none(),
        }
    }

    /// Last modifier mask sent by `wl_keyboard.modifiers`, reapplied whenever a new `keymap`
    /// replaces the XKB state (e.g. layout switch or `Modifiers` arriving before `Keymap`).
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct ModState {
        pub(crate) depressed: u32,
        pub(crate) latched: u32,
        pub(crate) locked: u32,
        pub(crate) group: u32,
    }

    /// Translates an XKB key press into Rune's logical `KeyEvent`, matching `window/keys.rs`:
    /// ignores NUL/control characters under `Ctrl` (such as pressing Shift while holding Ctrl),
    /// keeps `Ctrl+Space` as `Key::Text(" ")`, and maps non-ASCII layout chords (such as Russian
    /// `Ctrl+М`) through `us_char` to their US physical command key (`Command('v')`).
    pub(crate) fn key_event_from(
        named: Option<crate::Key>,
        utf32: u32,
        text: String,
        mods: crate::Mods,
        physical: Option<crate::PhysicalKeyCode>,
    ) -> Option<crate::KeyEvent> {
        let key = match named {
            Some(key) => key,
            None if mods.cmd => {
                let ch = char::from_u32(utf32).filter(|c| !c.is_control())?;
                if ch == ' ' {
                    crate::Key::Text(" ".to_owned())
                } else if !ch.is_ascii()
                    && let Some(us) = physical.and_then(crate::shortcuts::us_char)
                {
                    crate::Key::Command(us)
                } else {
                    crate::Key::Command(ch.to_ascii_lowercase())
                }
            }
            None => {
                if !text.is_empty() && !text.chars().any(char::is_control) {
                    crate::Key::Text(text)
                } else {
                    return None;
                }
            }
        };
        Some(crate::KeyEvent { key, mods })
    }

    /// Whether a layer-shell overlay with `policy` may request keyboard interactivity on
    /// `zwlr_layer_surface_v1` `layer_version`.
    pub(crate) fn can_request_keys(policy: crate::KeyPolicy, layer_version: u32) -> bool {
        policy == crate::KeyPolicy::OnDemand && layer_version >= 4
    }

    /// Whether a layer-shell overlay reports `nonactivating_keys` support.
    pub(crate) fn supports_keys(policy: crate::KeyPolicy, layer_version: u32, has_seat: bool) -> bool {
        can_request_keys(policy, layer_version) && has_seat
    }
}

#[cfg(test)]
#[path = "layer_shell_machine_tests.rs"]
mod tests;
