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

    /// Maximum size accepted for a Wayland `wl_keyboard.keymap` payload (16 MiB).
    pub(crate) const MAX_KEYMAP_BYTES: u32 = 16 * 1024 * 1024;

    /// Reads a `wl_keyboard.keymap` payload of `size` bytes from offset `0` via `read_at`,
    /// truncates at the first NUL byte, and validates UTF-8. Rejects empty or oversized payloads
    /// before allocating.
    pub(crate) fn decode_keymap(
        size: u32,
        read_at: impl FnOnce(&mut [u8], u64) -> std::io::Result<()>,
    ) -> Option<String> {
        if size == 0 || size > MAX_KEYMAP_BYTES {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        read_at(&mut buf, 0).ok()?;
        if let Some(nul) = buf.iter().position(|&b| b == 0) {
            buf.truncate(nul);
        }
        if buf.is_empty() {
            return None;
        }
        String::from_utf8(buf).ok()
    }
}

/// Bounded wait for a Wayland layer surface's initial `configure` (or `closed`) event.
/// Kept here so the deadline, flush-retry, interrupt-retry and timeout logic runs tests on
/// every host without needing a live Wayland compositor.
pub(crate) mod configure_wait {
    use std::time::Duration;
    use crate::time::Instant;

    /// How long `LayerSurface::new` waits for the compositor's initial `configure` (or `closed`)
    /// event before giving up so a silent or broken compositor cannot freeze the app.
    pub(crate) const TIMEOUT: Duration = Duration::from_secs(2);

    /// Result of flushing outgoing Wayland requests.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Flush {
        Ok,
        /// The socket send buffer is full; poll for writability and retry before reading.
        WouldBlock,
        Error,
    }

    /// Result of polling the Wayland socket up to the remaining deadline.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Poll {
        /// The socket is readable or reported hangup/error; read events now.
        Readable,
        /// Only writability fired while retrying a blocked flush; cancel the read and flush again.
        WritableOnly,
        /// The poll timed out without socket activity.
        TimedOut,
        /// Interrupted by a signal (`EINTR`); retry with the remaining time.
        Interrupted,
        Error,
    }

    /// Result of reading events from the Wayland socket into the queue.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Read {
        Ok,
        /// Another reader drained the socket or the read would block; retry until the deadline.
        WouldBlock,
        Error,
    }

    /// Outcome of [`wait`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Outcome {
        /// The surface reached `configure` or `closed` before the deadline.
        Done,
        /// The deadline elapsed before `configure` or `closed` arrived.
        TimedOut,
        /// Dispatching, flushing, polling or reading failed fatally.
        Error,
    }

    /// Platform operations needed by [`wait`]: dispatching pending events, flushing outgoing
    /// requests, and preparing, polling and reading the Wayland socket.
    pub(crate) trait Driver {
        /// Guard representing a prepared Wayland socket read. Dropping it without calling
        /// [`Self::read`] cancels the prepared read.
        type Guard;
        /// Whether `configure` or `closed` has already been dispatched.
        fn done(&self) -> bool;
        /// Current monotonic time.
        fn now(&mut self) -> Instant;
        /// Dispatches already-queued events; `false` on a fatal dispatch error.
        fn dispatch_pending(&mut self) -> bool;
        /// Flushes outgoing requests to the compositor.
        fn flush(&mut self) -> Flush;
        /// Prepares a synchronized read, or returns `None` when events are already pending in
        /// the queue and must be dispatched first.
        fn prepare_read(&mut self) -> Option<Self::Guard>;
        /// Polls the socket up to `timeout_ms` milliseconds (also watching for writability when
        /// `want_write` is true).
        fn poll(&mut self, guard: &Self::Guard, timeout_ms: i32, want_write: bool) -> Poll;
        /// Reads socket events into the queue, consuming the prepared-read guard.
        fn read(&mut self, guard: Self::Guard) -> Read;
    }

    /// Converts a non-zero remaining duration into a positive millisecond timeout for `poll(2)`,
    /// rounding sub-millisecond remainders up to 1 ms and clamping to `i32::MAX`.
    pub(crate) fn timeout_ms(remaining: Duration) -> i32 {
        let millis = remaining.as_millis();
        let rounded = if remaining.subsec_nanos() % 1_000_000 != 0 {
            millis.saturating_add(1)
        } else {
            millis
        };
        rounded.clamp(1, i32::MAX as u128) as i32
    }

    /// Waits for [`Driver::done`] up to `timeout`, keeping a single overall deadline across
    /// unrelated Wayland events, `EINTR` interruptions, and `WouldBlock` retries.
    pub(crate) fn wait<D: Driver>(driver: &mut D, timeout: Duration) -> Outcome {
        let deadline = driver.now() + timeout;
        loop {
            if !driver.dispatch_pending() {
                return Outcome::Error;
            }
            if driver.done() {
                return Outcome::Done;
            }
            let remaining = deadline.saturating_duration_since(driver.now());
            if remaining.is_zero() {
                return Outcome::TimedOut;
            }
            let want_write = match driver.flush() {
                Flush::Ok => false,
                Flush::WouldBlock => true,
                Flush::Error => return Outcome::Error,
            };
            let Some(guard) = driver.prepare_read() else {
                continue;
            };
            match driver.poll(&guard, timeout_ms(remaining), want_write) {
                Poll::Readable => match driver.read(guard) {
                    Read::Ok | Read::WouldBlock => {}
                    Read::Error => return Outcome::Error,
                },
                Poll::WritableOnly | Poll::Interrupted => {
                    drop(guard);
                }
                Poll::TimedOut => {
                    drop(guard);
                    if !driver.dispatch_pending() {
                        return Outcome::Error;
                    }
                    if driver.done() {
                        return Outcome::Done;
                    }
                    if driver.now() >= deadline {
                        return Outcome::TimedOut;
                    }
                }
                Poll::Error => {
                    drop(guard);
                    return Outcome::Error;
                }
            }
        }
    }
}

/// Held-key auto-repeat for Wayland layer-shell surfaces. Wayland leaves key repeat to
/// clients: `wl_keyboard.repeat_info` gives the rate (keys/s, 0 disables repeat) and initial
/// delay (ms), and `xkb_keymap_key_repeats` marks which keys may repeat.
///
/// The machine holds the repeat policy and, while a repeating key is down, the keycode,
/// fencing [`Attempt`] and next repeat [`Instant`]. Entering or advancing a hold returns
/// [`Effect::WakeAt`] for the overlay's wake deadline; [`Event::Due`] fires only when its
/// attempt still matches and `now >= due`, advancing one repeat per wake so stalls never burst.
pub(crate) mod repeat {
    use std::time::Duration;
    use crate::machine::{Attempt, Effects, Seq};
    use crate::time::Instant;

    /// Repeat cadence from `wl_keyboard.repeat_info`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Policy {
        /// Keys repeat after `delay`, then every `interval`.
        Repeat { delay: Duration, interval: Duration },
        /// `rate <= 0`: repeating is disabled.
        Disabled,
    }

    impl Default for Policy {
        /// 25 keys/s after 200 ms, matching winit until `wl_keyboard.repeat_info` arrives.
        fn default() -> Self {
            Self::Repeat {
                delay: Duration::from_millis(200),
                interval: Duration::from_millis(40),
            }
        }
    }

    impl Policy {
        /// Converts Wayland `wl_keyboard.repeat_info` arguments (`rate` in keys/s, `delay_ms` in ms).
        pub(crate) fn from_wayland(rate: i32, delay_ms: i32) -> Self {
            if rate <= 0 {
                return Self::Disabled;
            }
            let interval = Duration::from_nanos((1_000_000_000 / rate as u64).max(1));
            let delay = Duration::from_millis(delay_ms.max(0) as u64);
            Self::Repeat { delay, interval }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum State {
        /// No key is repeating.
        Idle { policy: Policy },
        /// `key` (Wayland evdev scancode) is held and repeats next at `due` under `attempt`.
        Holding {
            delay: Duration,
            interval: Duration,
            key: u32,
            attempt: Attempt,
            due: Instant,
        },
    }

    impl Default for State {
        fn default() -> Self {
            Self::Idle { policy: Policy::default() }
        }
    }

    impl State {
        /// Next repeat deadline while a key is held, for the overlay's wake timer.
        pub(crate) fn deadline(&self) -> Option<Instant> {
            match *self {
                Self::Holding { due, .. } => Some(due),
                Self::Idle { .. } => None,
            }
        }

        /// Fencing token of the currently armed repeat timer, if any.
        pub(crate) fn attempt(&self) -> Option<Attempt> {
            match *self {
                Self::Holding { attempt, .. } => Some(attempt),
                Self::Idle { .. } => None,
            }
        }

        /// Currently repeating Wayland keycode, if any.
        #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
        pub(crate) fn key(&self) -> Option<u32> {
            match *self {
                Self::Holding { key, .. } => Some(key),
                Self::Idle { .. } => None,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Event {
        /// Compositor updated `wl_keyboard.repeat_info`.
        RepeatInfo { rate: i32, delay_ms: i32 },
        /// Key pressed while focused; `repeats` comes from `xkb_keymap_key_repeats`.
        Press { key: u32, repeats: bool, now: Instant },
        /// Key released.
        Release { key: u32 },
        /// Focus left/changed, keymap invalidated the held key, or the surface was drained while hidden.
        Stop,
        /// The seat lost its keyboard capability: stops any repeat and resets policy to default
        /// for the next `wl_keyboard` object.
        KeyboardGone,
        /// The wake timer fired at `now` for `attempt`.
        Due { attempt: Attempt, now: Instant },
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Effect {
        /// Emit a repeated press of Wayland keycode `key` using the current XKB state.
        Repeat { key: u32 },
        /// Wake the overlay at `Instant` for the next repeat.
        WakeAt(Instant),
    }

    pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
        match (*state, event) {
            (State::Idle { .. }, Event::RepeatInfo { rate, delay_ms }) => {
                *state = State::Idle { policy: Policy::from_wayland(rate, delay_ms) };
                Effects::none()
            }
            (State::Holding { key, attempt, due, .. }, Event::RepeatInfo { rate, delay_ms }) => {
                match Policy::from_wayland(rate, delay_ms) {
                    Policy::Disabled => {
                        *state = State::Idle { policy: Policy::Disabled };
                    }
                    Policy::Repeat { delay, interval } => {
                        *state = State::Holding { delay, interval, key, attempt, due };
                    }
                }
                Effects::none()
            }
            (_, Event::Press { repeats: false, .. }) => Effects::none(),
            (State::Idle { policy: Policy::Disabled }, Event::Press { .. }) => Effects::none(),
            (
                State::Idle { policy: Policy::Repeat { delay, interval } }
                | State::Holding { delay, interval, .. },
                Event::Press { key, repeats: true, now },
            ) => {
                let attempt = seq.next();
                let due = now + delay;
                *state = State::Holding { delay, interval, key, attempt, due };
                Effects::one(Effect::WakeAt(due))
            }
            (State::Holding { delay, interval, key: held, .. }, Event::Release { key }) if held == key => {
                *state = State::Idle { policy: Policy::Repeat { delay, interval } };
                Effects::none()
            }
            (State::Holding { delay, interval, .. }, Event::Stop) => {
                *state = State::Idle { policy: Policy::Repeat { delay, interval } };
                Effects::none()
            }
            (_, Event::KeyboardGone) => {
                *state = State::default();
                Effects::none()
            }
            (State::Holding { delay, interval, key, attempt: armed, due }, Event::Due { attempt, now })
                if armed == attempt && now >= due =>
            {
                let next_attempt = seq.next();
                let next_due = now + interval;
                *state = State::Holding { delay, interval, key, attempt: next_attempt, due: next_due };
                Effects::two(Effect::Repeat { key }, Effect::WakeAt(next_due))
            }
            _ => Effects::none(),
        }
    }
}

/// The `zwlr_layer_surface_v1.set_exclusive_zone` value for `placement`: `-1` lets a full-screen
/// overlay cover bars and other surfaces that reserve space; other placements keep `0` so they
/// neither reserve space nor ignore other surfaces' exclusive zones.
pub(crate) fn exclusive_zone(placement: Option<crate::OverlayPlacement>) -> i32 {
    match placement {
        Some(crate::OverlayPlacement::FullScreen) => -1,
        Some(crate::OverlayPlacement::Edge { .. } | crate::OverlayPlacement::Rect { .. }) | None => 0,
    }
}

#[cfg(test)]
#[path = "layer_shell_machine_tests.rs"]
mod tests;
