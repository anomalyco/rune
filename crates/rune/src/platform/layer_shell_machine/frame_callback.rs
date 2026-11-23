//! Tracks a Wayland layer surface's `wl_surface.frame` callback across presentations so the
//! overlay never presents a second frame before the compositor answers the previous commit's
//! callback. Kept cross-platform so its transitions and tests compile and run on every host.

use crate::machine::Effects;

/// Whether the layer surface can present now or is waiting for the compositor's `done` event
/// for its last committed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Ready to acquire and present. `next` is the id of the next callback request.
    Ready { next: u64 },
    /// Waiting for `wl_callback.done` for callback `id`.
    Pending { id: u64 },
}

impl Default for State {
    fn default() -> Self {
        State::Ready { next: 1 }
    }
}

impl State {
    /// True when no frame callback is outstanding, so presenting will not block in Mesa's
    /// FIFO wait.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn ready(self) -> bool {
        matches!(self, State::Ready { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// A frame is about to present: issue `wl_surface.frame` before `queue.present` so the
    /// callback commits with the buffer.
    Request,
    /// The compositor answered callback `id` (`wl_callback.done`).
    Done(u64),
    /// The surface closed or its GPU device was lost before a commit could land.
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Call `wl_surface.frame` with this callback id as its `Dispatch<WlCallback, u64>` data.
    Request(u64),
}

/// Advances the frame-callback state machine and returns any `wl_surface.frame` request to
/// issue on the surface's private queue.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Ready { next }, Event::Request) => {
            *state = State::Pending { id: next };
            Effects::one(Effect::Request(next))
        }
        (State::Pending { id }, Event::Request) => {
            let next = id.wrapping_add(1).max(1);
            *state = State::Pending { id: next };
            Effects::one(Effect::Request(next))
        }
        (State::Pending { id }, Event::Done(done)) if done == id => {
            *state = State::Ready { next: id.wrapping_add(1).max(1) };
            Effects::none()
        }
        (State::Ready { .. } | State::Pending { .. }, Event::Done(_)) => Effects::none(),
        (State::Ready { next }, Event::Reset) => {
            *state = State::Ready { next };
            Effects::none()
        }
        (State::Pending { id }, Event::Reset) => {
            *state = State::Ready { next: id.wrapping_add(1).max(1) };
            Effects::none()
        }
    }
}

#[cfg(test)]
#[path = "frame_callback_tests.rs"]
mod tests;
