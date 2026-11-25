//! Lazy Wayland `wl_data_device` binding state machine for one window.
//!
//! Tracks pre-first-frame deferral, commit/modal sync gating (so steady and animation frames do
//! zero work), a short grace timer before tearing down on unmount (preventing thread/round-trip
//! churn across brief modal or screen switches), and terminal unavailability on missing global or
//! lost display.

use std::time::Duration;

use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;

/// Grace period before tearing down a window's Wayland drop binding after its last drop handler
/// unmounts. Prevents destroying and re-spawning the reader thread + `registry_queue_init`
/// round-trip on brief modal/screen toggles while still tearing down promptly when drop handlers
/// stay unmounted.
pub(crate) const TEARDOWN_GRACE: Duration = Duration::from_millis(500);

/// Snapshot key gating `RuntimeCore::has_drop_targets()` / `has_drag_out()` scans: an unchanged
/// `(commit_generation, active_modal, active_drag)` means live drop/drag-out targets have not changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SyncKey {
    pub(crate) commit: u64,
    pub(crate) modal: Option<usize>,
    pub(crate) active_drag: bool,
}

/// Per-window lazy binding mode for Wayland `wl_data_device`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Window has not yet completed its first frame and has no pending drop handler.
    Unbound { last_sync: Option<SyncKey> },
    /// A drop handler mounted before the first frame; binds once `WindowReady` arrives.
    PendingBind { last_sync: Option<SyncKey> },
    /// Window completed its first frame and has no bound Wayland drop target.
    ReadyUnbound { last_sync: Option<SyncKey> },
    /// Window is registered with the shared per-display Wayland drop controller.
    Bound { last_sync: Option<SyncKey> },
    /// Last drop handler unmounted; waiting `TEARDOWN_GRACE` before unregistering and tearing
    /// down so brief modal/screen switches do not churn threads or Wayland globals.
    TearingDown {
        last_sync: Option<SyncKey>,
        attempt: Attempt,
        due: Instant,
    },
    /// Compositor lacks `wl_data_device_manager` or the display connection was lost; never retry.
    Unavailable,
}

impl Default for State {
    fn default() -> Self {
        Self::Unbound { last_sync: None }
    }
}

impl State {
    /// Returns true only when `has_drop_targets()` must be evaluated for `key`.
    /// Steady and animation frames with unchanged `SyncKey` return `false` (zero scan work).
    pub(crate) fn needs_sync(&self, key: SyncKey) -> bool {
        match *self {
            Self::Unavailable => false,
            Self::Unbound { last_sync }
            | Self::PendingBind { last_sync }
            | Self::ReadyUnbound { last_sync }
            | Self::Bound { last_sync }
            | Self::TearingDown { last_sync, .. } => last_sync != Some(key),
        }
    }

    /// Next teardown grace deadline for `Host::wake_deadline()`.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match *self {
            Self::TearingDown { due, .. } => Some(due),
            _ => None,
        }
    }

    /// Armed teardown grace attempt, if currently waiting in `TearingDown`.
    pub(crate) fn grace_attempt(&self) -> Option<Attempt> {
        match *self {
            Self::TearingDown { attempt, .. } => Some(attempt),
            _ => None,
        }
    }
}

/// Events stepping one window's lazy Wayland drop binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// The window completed its first frame (`run_on_ready`).
    WindowReady,
    /// Commit generation or active modal changed; `has_targets` is `RuntimeCore::has_drop_targets()`.
    TreeSynced { key: SyncKey, has_targets: bool, now: Instant },
    /// `wayland::drop::install` failed (compositor lacks `wl_data_device_manager`).
    BindFailed,
    /// The teardown grace timer fired at `now` for `attempt`.
    GraceDue { attempt: Attempt, now: Instant },
    /// The window closed or hid; tear down immediately without waiting for grace.
    WindowClosed,
    /// The Wayland display connection failed; tear down immediately and never retry.
    DisplayLost,
}

/// Actions requested by one binding transition, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Register this window's `wl_surface` with the shared per-display `wl_data_device` controller.
    Bind,
    /// Clear any active runtime file-drag hover (`RuntimeCore::drag_files_leave`).
    LeaveRuntimeDrag,
    /// Wake the event loop at `Instant` to run `Event::GraceDue`.
    WakeAt(Instant),
    /// Drop the window's `DropTarget` guard (unregisters surface; tears down controller + threads
    /// when the last window on the display unregisters).
    Unbind,
}

/// Advances `state` by `event` and returns the ordered effects to execute.
pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Unavailable, _) => Effects::none(),
        (State::Unbound { last_sync }, Event::WindowReady) => {
            *state = State::ReadyUnbound { last_sync };
            Effects::none()
        }
        (State::PendingBind { last_sync }, Event::WindowReady) => {
            *state = State::Bound { last_sync };
            Effects::one(Effect::Bind)
        }
        (_, Event::WindowReady) => Effects::none(),
        (State::Unbound { last_sync } | State::PendingBind { last_sync }, Event::TreeSynced { key, has_targets, .. }) => {
            if last_sync == Some(key) {
                return Effects::none();
            }
            *state = if has_targets {
                State::PendingBind { last_sync: Some(key) }
            } else {
                State::Unbound { last_sync: Some(key) }
            };
            Effects::none()
        }
        (State::ReadyUnbound { last_sync }, Event::TreeSynced { key, has_targets, .. }) => {
            if last_sync == Some(key) {
                return Effects::none();
            }
            if has_targets {
                *state = State::Bound { last_sync: Some(key) };
                Effects::one(Effect::Bind)
            } else {
                *state = State::ReadyUnbound { last_sync: Some(key) };
                Effects::none()
            }
        }
        (State::Bound { last_sync }, Event::TreeSynced { key, has_targets, now }) => {
            if last_sync == Some(key) {
                return Effects::none();
            }
            if has_targets {
                *state = State::Bound { last_sync: Some(key) };
                Effects::none()
            } else {
                let attempt = seq.next();
                let due = now + TEARDOWN_GRACE;
                *state = State::TearingDown { last_sync: Some(key), attempt, due };
                Effects::two(Effect::LeaveRuntimeDrag, Effect::WakeAt(due))
            }
        }
        (State::TearingDown { last_sync, attempt, due }, Event::TreeSynced { key, has_targets, .. }) => {
            if last_sync == Some(key) {
                return Effects::none();
            }
            if has_targets {
                *state = State::Bound { last_sync: Some(key) };
            } else {
                // Keep original (attempt, due) so unrelated commits cannot postpone teardown.
                *state = State::TearingDown { last_sync: Some(key), attempt, due };
            }
            Effects::none()
        }
        (State::Bound { .. }, Event::BindFailed) => {
            *state = State::Unavailable;
            Effects::none()
        }
        (_, Event::BindFailed) => Effects::none(),
        (State::TearingDown { last_sync, attempt: armed, due }, Event::GraceDue { attempt, now })
            if attempt == armed && now >= due =>
        {
            *state = State::ReadyUnbound { last_sync };
            Effects::one(Effect::Unbind)
        }
        (_, Event::GraceDue { .. }) => Effects::none(),
        (State::Bound { .. }, Event::WindowClosed) => {
            *state = State::Unbound { last_sync: None };
            Effects::two(Effect::LeaveRuntimeDrag, Effect::Unbind)
        }
        (State::TearingDown { .. }, Event::WindowClosed) => {
            *state = State::Unbound { last_sync: None };
            Effects::one(Effect::Unbind)
        }
        (_, Event::WindowClosed) => {
            *state = State::Unbound { last_sync: None };
            Effects::none()
        }
        (State::Bound { .. }, Event::DisplayLost) => {
            *state = State::Unavailable;
            Effects::two(Effect::LeaveRuntimeDrag, Effect::Unbind)
        }
        (State::TearingDown { .. }, Event::DisplayLost) => {
            *state = State::Unavailable;
            Effects::one(Effect::Unbind)
        }
        (_, Event::DisplayLost) => {
            *state = State::Unavailable;
            Effects::none()
        }
    }
}

#[cfg(test)]
#[path = "binding_tests.rs"]
mod tests;
