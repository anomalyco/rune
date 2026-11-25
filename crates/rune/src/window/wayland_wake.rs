//! Scheduling for Wayland layer-shell overlays on winit's shared `wl_display`.
//!
//! Instead of spawning a background reader thread per layer surface, winit wakes from
//! `ControlFlow::Wait` on any Wayland display read while at least one layer surface is live,
//! and a syscall-free `prepare_read` check at the end of `about_to_wait` posts one
//! `WakeEvent::LayerPending` whenever a mid-turn read (Mesa WSI acquire/present or a roundtrip)
//! already queued events for a layer surface.

use crate::machine::Effects;
use crate::window::AnyHost;

/// Whether winit's `wake_on_wayland_read` flag is active and whether a pending-event wake is
/// already in flight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    /// No live Wayland layer surface; winit's `wake_on_wayland_read` flag is off.
    #[default]
    Inactive,
    /// At least one Wayland layer surface is live and idle; winit wakes on any Wayland read.
    Watching,
    /// At least one Wayland layer surface is live and a `WakeEvent::LayerPending` ping is in
    /// flight to drain events already queued in libwayland.
    WakeQueued,
}

/// Inputs to [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// End of an event-loop turn, after every host's `about_to_wait` and `turn_ends`.
    AboutToWait {
        wake_on_read: bool,
        has_pending_events: bool,
    },
    /// `WakeEvent::LayerPending` was delivered to `ApplicationHandler::user_event`.
    WakeReceived,
    /// The event loop is exiting and hosts are being torn down.
    Exiting,
}

/// Side effects the event loop performs after [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Toggle winit's `ActiveEventLoopExtWayland::set_wake_on_wayland_read`.
    SetWakeOnRead(bool),
    /// Send `WakeEvent::LayerPending` through the event-loop proxy so the next turn runs
    /// immediately without sleeping in `epoll_wait` or entering `ControlFlow::Poll`.
    SendWake,
}

/// Advances the Wayland queue wake scheduler for `event` and returns the effects to perform.
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Inactive, Event::AboutToWait { wake_on_read: false, has_pending_events: false }) => {
            Effects::none()
        }
        (State::Inactive, Event::AboutToWait { wake_on_read: false, has_pending_events: true }) => {
            Effects::one(Effect::SendWake)
        }
        (State::Inactive, Event::AboutToWait { wake_on_read: true, has_pending_events: false }) => {
            *state = State::Watching;
            Effects::one(Effect::SetWakeOnRead(true))
        }
        (State::Inactive, Event::AboutToWait { wake_on_read: true, has_pending_events: true }) => {
            *state = State::WakeQueued;
            Effects::two(Effect::SetWakeOnRead(true), Effect::SendWake)
        }
        (State::Watching, Event::AboutToWait { wake_on_read: true, has_pending_events: false }) => {
            Effects::none()
        }
        (State::Watching, Event::AboutToWait { wake_on_read: true, has_pending_events: true }) => {
            *state = State::WakeQueued;
            Effects::one(Effect::SendWake)
        }
        (State::Watching, Event::AboutToWait { wake_on_read: false, has_pending_events: false }) => {
            *state = State::Inactive;
            Effects::one(Effect::SetWakeOnRead(false))
        }
        (State::Watching, Event::AboutToWait { wake_on_read: false, has_pending_events: true }) => {
            *state = State::Inactive;
            Effects::two(Effect::SetWakeOnRead(false), Effect::SendWake)
        }
        (State::WakeQueued, Event::AboutToWait { wake_on_read: true, has_pending_events: true }) => {
            Effects::none()
        }
        (State::WakeQueued, Event::AboutToWait { wake_on_read: true, has_pending_events: false }) => {
            *state = State::Watching;
            Effects::none()
        }
        (State::WakeQueued, Event::AboutToWait { wake_on_read: false, has_pending_events: false }) => {
            *state = State::Inactive;
            Effects::one(Effect::SetWakeOnRead(false))
        }
        (State::WakeQueued, Event::AboutToWait { wake_on_read: false, has_pending_events: true }) => {
            *state = State::Inactive;
            Effects::two(Effect::SetWakeOnRead(false), Effect::SendWake)
        }
        (State::WakeQueued, Event::WakeReceived) => {
            *state = State::Watching;
            Effects::none()
        }
        (State::Inactive | State::Watching, Event::WakeReceived) => Effects::none(),
        (State::Inactive, Event::Exiting) => Effects::none(),
        (State::Watching | State::WakeQueued, Event::Exiting) => {
            *state = State::Inactive;
            Effects::one(Effect::SetWakeOnRead(false))
        }
    }
}

/// Evaluates all hosts and secondary display-queue watches at the end of `Windows::about_to_wait`
/// and steps `state`.
pub(crate) fn about_to_wait(
    state: &mut State,
    hosts: &[Option<Box<dyn AnyHost>>],
    has_display_watch: bool,
    has_pending_display: bool,
) -> Effects<Effect> {
    let wake_on_read = has_display_watch
        || hosts.iter().flatten().any(|h| h.has_live_wayland_layer());
    let has_pending_events = has_pending_display
        || hosts.iter().flatten().any(|h| h.has_pending_platform_events());
    step(state, Event::AboutToWait { wake_on_read, has_pending_events })
}

#[cfg(test)]
#[path = "wayland_wake_tests.rs"]
mod tests;
