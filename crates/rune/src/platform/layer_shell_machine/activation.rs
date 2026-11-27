//! Wayland `xdg_activation_v1` serial tracking, non-blocking request sequencing, and async token
//! lifecycle state machine for layer-shell overlays, kept cross-platform so its transitions and
//! protocol order tests run on every host.

use std::time::Duration;

use crate::machine::Effects;
use crate::time::Instant;

/// Fallback timeout for an in-flight `xdg_activation_token_v1` request when a compositor
/// advertises `xdg_activation_v1` but never sends `xdg_activation_token_v1.done`.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) const TIMEOUT: Duration = Duration::from_secs(2);

/// An input or focus event on a layer surface that may supply the
/// `xdg_activation_token_v1.set_serial` serial. Per `xdg-activation-v1`, compositors validate
/// tokens against the seat and serial of the triggering pointer button, key press, touch down, or
/// keyboard focus enter on the requesting surface.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputEvent {
    /// `wl_pointer.button` (`inside`: pointer is over our surface; `pressed`: press, not release).
    PointerButton { serial: u32, inside: bool, pressed: bool },
    /// `wl_keyboard.key` (`focused`: our surface holds keyboard focus; `pressed`: press, not release).
    Key { serial: u32, focused: bool, pressed: bool },
    /// `wl_keyboard.enter` (`ours`: keyboard focus entered our surface).
    KeyboardEnter { serial: u32, ours: bool },
    /// `wl_touch.down` (`ours`: the touch landed on our surface).
    TouchDown { serial: u32, ours: bool },
    /// The surface closed or tore down: discard any earlier serial.
    Reset,
}

/// The latest valid activating input serial received on a layer surface.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InputSerial {
    #[default]
    None,
    Recorded(u32),
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
impl InputSerial {
    pub(crate) fn step(&mut self, event: InputEvent) {
        match event {
            InputEvent::PointerButton { serial, inside: true, pressed: true }
            | InputEvent::Key { serial, focused: true, pressed: true }
            | InputEvent::KeyboardEnter { serial, ours: true }
            | InputEvent::TouchDown { serial, ours: true } => *self = Self::Recorded(serial),
            InputEvent::Reset => *self = Self::None,
            InputEvent::PointerButton { .. }
            | InputEvent::Key { .. }
            | InputEvent::KeyboardEnter { .. }
            | InputEvent::TouchDown { .. } => {}
        }
    }

    pub(crate) fn get(self) -> Option<u32> {
        match self {
            Self::None => None,
            Self::Recorded(serial) => Some(serial),
        }
    }
}

/// Result of flushing a newly committed `xdg_activation_token_v1` request without blocking.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartFlush {
    /// Flushed to the socket (or temporarily `WouldBlock`, which flushes on a later turn).
    Sent,
    /// Fatal protocol or socket error on the Wayland display connection.
    Fatal,
}

/// Wire seam for one non-blocking `xdg_activation_v1` token exchange so the exact request order
/// (`get_activation_token`, optional `set_serial`, `set_surface`, `commit`, non-blocking
/// `flush_start`, and teardown `drain_pending`, `destroy_token`, `destroy_activation`,
/// `drain_pending`, `flush`) is verified on every host without blocking roundtrips.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) trait Wire {
    fn get_activation_token(&mut self);
    fn set_serial(&mut self, serial: u32);
    fn set_surface(&mut self);
    fn commit(&mut self);
    fn flush_start(&mut self) -> StartFlush;
    fn destroy_token(&mut self);
    fn destroy_activation(&mut self);
    fn drain_pending(&mut self);
    fn flush(&mut self);
}

/// Issues one `xdg_activation_v1` token request in protocol order and returns immediately without
/// blocking in a compositor roundtrip. Returns `true` when the request is in flight awaiting
/// `done`, or `false` (cleaning up proxies on fatal flush error) when `closed` is true or the
/// display connection failed.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn begin_request(wire: &mut impl Wire, serial: Option<u32>, closed: bool) -> bool {
    if closed { return false; }
    wire.get_activation_token();
    if let Some(serial) = serial {
        wire.set_serial(serial);
    }
    wire.set_surface();
    wire.commit();
    match wire.flush_start() {
        StartFlush::Sent => true,
        StartFlush::Fatal => {
            finish_request(wire);
            false
        }
    }
}

/// Tears down an `xdg_activation_v1` request after `done`, fatal error, timeout, or drop: drains
/// pending events before and after destroying proxies so no `QueueHandle` cycle remains, then
/// flushes the destroys best-effort.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn finish_request(wire: &mut impl Wire) {
    wire.drain_pending();
    wire.destroy_token();
    wire.destroy_activation();
    wire.drain_pending();
    wire.flush();
}

/// Result of one non-blocking poll of an in-flight `xdg_activation_token_v1`.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokenPoll {
    /// Still waiting for the compositor's `xdg_activation_token_v1.done`.
    Pending,
    /// The request finished with a non-empty token (`Some`) or none (`None` for an empty token
    /// or fatal display error).
    Done(Option<String>),
}

/// Lifecycle state of one committed, in-flight `xdg_activation_token_v1` request.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Committed on the Wayland wire; waiting for `done` until `deadline`.
    Pending { deadline: Instant },
    /// Answered, failed, timed out, or cancelled; proxies torn down and reply delivered once.
    Finished,
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
impl State {
    pub(crate) fn is_pending(self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    pub(crate) fn deadline(self) -> Option<Instant> {
        match self {
            Self::Pending { deadline } => Some(deadline),
            Self::Finished => None,
        }
    }
}

/// Inputs to [`step`] for one in-flight activation token request.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Compositor delivered `xdg_activation_token_v1.done { token }`.
    Done(String),
    /// Fatal display error or empty compositor answer.
    Failed,
    /// Event-loop turn reached `now`.
    Tick { now: Instant },
    /// Overlay is terminating, dropping, or entered terminal GPU loss.
    Cancel,
}

/// Effect emitted when an in-flight activation token finishes.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Tear down the Wayland proxies and invoke the caller's callback once with this answer.
    Complete(Option<String>),
}

/// Advances one in-flight activation token's state machine. Guarantees at most one
/// [`Effect::Complete`].
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Finished, _) => Effects::none(),
        (State::Pending { .. }, Event::Done(token)) => {
            *state = State::Finished;
            Effects::one(Effect::Complete((!token.is_empty()).then_some(token)))
        }
        (State::Pending { .. }, Event::Failed | Event::Cancel) => {
            *state = State::Finished;
            Effects::one(Effect::Complete(None))
        }
        (State::Pending { deadline }, Event::Tick { now }) if now >= deadline => {
            *state = State::Finished;
            Effects::one(Effect::Complete(None))
        }
        (State::Pending { .. }, Event::Tick { .. }) => Effects::none(),
    }
}

#[cfg(test)]
#[path = "activation_tests.rs"]
mod tests;
