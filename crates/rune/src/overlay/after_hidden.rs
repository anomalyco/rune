//! One-shot `OverlayHandle::after_hidden` callbacks: waits for a hide to take effect before
//! running on the event thread.
//!
//! On Wayland, hiding destroys the overlay's surface and this state machine sends a
//! `wl_display.sync` roundtrip on winit's shared display. While the sync is out the overlay
//! counts as live for `wayland_wake` so the compositor's `wl_callback.done` turns the loop even
//! with no layer surface mapped. Showing the overlay again before the answer arrives cancels the
//! sync and keeps the callbacks waiting for the next hide. Elsewhere, callbacks run in the
//! event-loop turn after the hide. Closing the overlay, quitting the app, or losing the display
//! drops pending callbacks unrun.

use super::*;
use crate::machine::Effects;

/// Where pending `OverlayHandle::after_hidden` callbacks sit in the hide lifecycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Phase {
    /// No `after_hidden` callbacks are waiting.
    #[default]
    Idle,
    /// Callbacks are queued while the overlay is shown or fading out; waiting for it to hide.
    WaitingForHide,
    /// The overlay is hidden on Wayland and a `wl_display.sync` roundtrip is in flight.
    WaitingSync,
    /// The overlay hid on a non-Wayland host during the current turn; callbacks wait for the
    /// next event-loop turn.
    WaitingNextTurn,
    /// The next turn after a non-Wayland hide has begun; callbacks run at `about_to_wait`
    /// unless the overlay is shown first.
    ReadyToRun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// One or more callbacks arrived from `OverlayHandle::after_hidden`.
    Queued { hidden: bool, uses_sync: bool },
    /// The overlay became shown (`Presence::Shown`).
    Shown,
    /// The overlay finished hiding (`Presence::Hidden`, after native hide / surface destroy).
    /// `surface_destroyed` is true when a live Wayland surface was destroyed in this hide.
    Hidden { uses_sync: bool, surface_destroyed: bool },
    /// A new event-loop turn began (`AnyHost::new_events`).
    NewTurn,
    /// The in-flight Wayland `wl_display.sync` callback answered (`wl_callback.done`).
    SyncAnswered,
    /// Starting or polling the Wayland `wl_display.sync` failed (fatal display error).
    SyncFailed,
    /// The host reached `about_to_wait` while hidden (`Presence::Hidden`).
    AboutToWaitHidden,
    /// The overlay is closing, quitting, or entered terminal GPU loss: drop everything unrun.
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    /// Start (or replace) a Wayland `wl_display.sync` roundtrip on winit's display.
    StartSync,
    /// Drop any in-flight Wayland `wl_display.sync`.
    CancelSync,
    /// Wake the event loop so the next turn runs (`WaitingNextTurn`).
    Wake,
    /// Run all queued `after_hidden` callbacks once on the event thread.
    RunCallbacks,
    /// Drop all queued `after_hidden` callbacks unrun.
    DropCallbacks,
}

pub(super) fn step(phase: &mut Phase, event: Event) -> Effects<Effect> {
    match (*phase, event) {
        (Phase::Idle | Phase::WaitingForHide, Event::Queued { hidden: false, .. }) => {
            *phase = Phase::WaitingForHide;
            Effects::none()
        }
        (Phase::Idle | Phase::WaitingForHide, Event::Queued { hidden: true, uses_sync: true }) => {
            *phase = Phase::WaitingSync;
            Effects::one(Effect::StartSync)
        }
        (Phase::Idle | Phase::WaitingForHide, Event::Queued { hidden: true, uses_sync: false }) => {
            *phase = Phase::WaitingNextTurn;
            Effects::one(Effect::Wake)
        }
        (Phase::WaitingSync, Event::Queued { hidden: true, .. }) => Effects::none(),
        (Phase::WaitingSync, Event::Queued { hidden: false, .. }) => {
            *phase = Phase::WaitingForHide;
            Effects::one(Effect::CancelSync)
        }
        (Phase::WaitingNextTurn, Event::Queued { hidden: true, .. })
        | (Phase::ReadyToRun, Event::Queued { hidden: true, .. }) => Effects::none(),
        (Phase::WaitingNextTurn | Phase::ReadyToRun, Event::Queued { hidden: false, .. }) => {
            *phase = Phase::WaitingForHide;
            Effects::none()
        }

        (Phase::Idle, Event::Shown) => Effects::none(),
        (Phase::WaitingSync, Event::Shown) => {
            *phase = Phase::WaitingForHide;
            Effects::one(Effect::CancelSync)
        }
        (Phase::WaitingForHide | Phase::WaitingNextTurn | Phase::ReadyToRun, Event::Shown) => {
            *phase = Phase::WaitingForHide;
            Effects::none()
        }

        (Phase::Idle, Event::Hidden { .. }) => Effects::none(),
        (Phase::WaitingForHide, Event::Hidden { uses_sync: true, .. }) => {
            *phase = Phase::WaitingSync;
            Effects::one(Effect::StartSync)
        }
        (Phase::WaitingForHide, Event::Hidden { uses_sync: false, .. }) => {
            *phase = Phase::WaitingNextTurn;
            Effects::one(Effect::Wake)
        }
        (Phase::WaitingSync, Event::Hidden { uses_sync: true, surface_destroyed: true }) => {
            Effects::one(Effect::StartSync)
        }
        (Phase::WaitingSync, Event::Hidden { uses_sync: true, surface_destroyed: false }) => {
            Effects::none()
        }
        (Phase::WaitingSync, Event::Hidden { uses_sync: false, .. }) => {
            *phase = Phase::WaitingNextTurn;
            Effects::two(Effect::CancelSync, Effect::Wake)
        }
        (Phase::WaitingNextTurn, Event::Hidden { uses_sync: true, .. })
        | (Phase::ReadyToRun, Event::Hidden { uses_sync: true, .. }) => {
            *phase = Phase::WaitingSync;
            Effects::one(Effect::StartSync)
        }
        (Phase::WaitingNextTurn, Event::Hidden { uses_sync: false, .. }) => Effects::none(),
        (Phase::ReadyToRun, Event::Hidden { uses_sync: false, .. }) => {
            *phase = Phase::WaitingNextTurn;
            Effects::one(Effect::Wake)
        }

        (Phase::WaitingNextTurn, Event::NewTurn) => {
            *phase = Phase::ReadyToRun;
            Effects::none()
        }
        (_, Event::NewTurn) => Effects::none(),

        (Phase::WaitingSync, Event::SyncAnswered) => {
            *phase = Phase::Idle;
            Effects::one(Effect::RunCallbacks)
        }
        (_, Event::SyncAnswered) => Effects::none(),

        (Phase::WaitingSync, Event::SyncFailed) => {
            *phase = Phase::Idle;
            Effects::two(Effect::CancelSync, Effect::DropCallbacks)
        }
        (_, Event::SyncFailed) => Effects::none(),

        (Phase::ReadyToRun, Event::AboutToWaitHidden) => {
            *phase = Phase::Idle;
            Effects::one(Effect::RunCallbacks)
        }
        (_, Event::AboutToWaitHidden) => Effects::none(),

        (Phase::Idle, Event::Cancel) => Effects::none(),
        (Phase::WaitingSync, Event::Cancel) => {
            *phase = Phase::Idle;
            Effects::two(Effect::CancelSync, Effect::DropCallbacks)
        }
        (Phase::WaitingForHide | Phase::WaitingNextTurn | Phase::ReadyToRun, Event::Cancel) => {
            *phase = Phase::Idle;
            Effects::one(Effect::DropCallbacks)
        }
    }
}

/// Owned subsystem state for `OverlayHandle::after_hidden` inside [`OverlayCore`].
#[derive(Default)]
pub(in crate::overlay) struct AfterHiddenState {
    pub(super) phase: Phase,
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) display: Option<winit::raw_window_handle::WaylandDisplayHandle>,
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) sync: Option<crate::platform::layer_shell::HideSync>,
}

impl AfterHiddenState {
    fn uses_sync(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        return self.display.is_some();
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        false
    }
}

impl OverlayCore {
    pub(super) fn record_wayland_display(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if self.after_hidden.display.is_some() || !crate::window::is_wayland(event_loop) {
                return;
            }
            use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
            if let Ok(display) = event_loop.display_handle()
                && let RawDisplayHandle::Wayland(handle) = display.as_raw()
            {
                self.after_hidden.display = Some(handle);
            }
        }
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        let _ = event_loop;
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_live_wayland_hide_sync(&self) -> bool {
        self.after_hidden.sync.as_ref().is_some_and(crate::platform::layer_shell::HideSync::is_live)
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_pending_wayland_hide_sync_events(&self) -> bool {
        self.after_hidden
            .sync
            .as_ref()
            .is_some_and(crate::platform::layer_shell::HideSync::has_pending_events)
    }

    pub(super) fn after_hidden_queued(&mut self, app: &mut dyn OverlayApp) {
        let event = Event::Queued {
            hidden: !self.presence.drawing(),
            uses_sync: self.after_hidden.uses_sync(),
        };
        let effects = step(&mut self.after_hidden.phase, event);
        self.apply_after_hidden(app, effects);
    }

    pub(super) fn after_hidden_shown(&mut self, app: &mut dyn OverlayApp) {
        let effects = step(&mut self.after_hidden.phase, Event::Shown);
        self.apply_after_hidden(app, effects);
    }

    pub(super) fn after_hidden_did_hide(&mut self, app: &mut dyn OverlayApp, surface_destroyed: bool) {
        let event = Event::Hidden {
            uses_sync: self.after_hidden.uses_sync(),
            surface_destroyed,
        };
        let effects = step(&mut self.after_hidden.phase, event);
        self.apply_after_hidden(app, effects);
    }

    pub(super) fn after_hidden_new_turn(&mut self, app: &mut dyn OverlayApp) {
        let effects = step(&mut self.after_hidden.phase, Event::NewTurn);
        self.apply_after_hidden(app, effects);
    }

    pub(super) fn poll_after_hidden(&mut self, app: &mut dyn OverlayApp) {
        if self.presence.drawing() {
            return;
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(sync) = self.after_hidden.sync.as_mut() {
            match sync.dispatch() {
                crate::platform::layer_shell::SyncPoll::Pending => return,
                crate::platform::layer_shell::SyncPoll::Answered => {
                    self.after_hidden.sync = None;
                    let effects = step(&mut self.after_hidden.phase, Event::SyncAnswered);
                    self.apply_after_hidden(app, effects);
                    return;
                }
                crate::platform::layer_shell::SyncPoll::Failed => {
                    let effects = step(&mut self.after_hidden.phase, Event::SyncFailed);
                    self.apply_after_hidden(app, effects);
                    return;
                }
            }
        }
        let effects = step(&mut self.after_hidden.phase, Event::AboutToWaitHidden);
        self.apply_after_hidden(app, effects);
    }

    pub(super) fn cancel_after_hidden(&mut self, app: &mut dyn OverlayApp) {
        let effects = step(&mut self.after_hidden.phase, Event::Cancel);
        self.apply_after_hidden(app, effects);
        app.drop_after_hidden();
    }

    pub(super) fn close_after_hidden(&mut self, app: &mut dyn OverlayApp) {
        let effects = step(&mut self.after_hidden.phase, Event::Cancel);
        self.apply_after_hidden(app, effects);
        app.close_after_hidden();
    }

    fn apply_after_hidden(&mut self, app: &mut dyn OverlayApp, effects: Effects<Effect>) {
        for effect in effects {
            match effect {
                Effect::StartSync => {
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    {
                        self.after_hidden.sync = None;
                        let sync = self
                            .after_hidden
                            .display
                            .and_then(crate::platform::layer_shell::HideSync::new);
                        if let Some(sync) = sync {
                            self.after_hidden.sync = Some(sync);
                        } else {
                            let fail = step(&mut self.after_hidden.phase, Event::SyncFailed);
                            self.apply_after_hidden(app, fail);
                        }
                    }
                }
                Effect::CancelSync => {
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    {
                        self.after_hidden.sync = None;
                    }
                }
                Effect::Wake => {
                    self.shared.wake();
                }
                Effect::RunCallbacks => {
                    if app.run_after_hidden() {
                        self.clipboard.flush_core(app.rt_mut());
                        self.open_popup(app);
                        app.rt_mut().schedule_pending_wakes();
                        self.take_window_requests(app.rt_mut());
                        self.dirty = true;
                        if self.retained() && app.rt().needs_commit() {
                            self.hidden_pace.changed(crate::time::now());
                        }
                    }
                }
                Effect::DropCallbacks => {
                    app.drop_after_hidden();
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "after_hidden_tests.rs"]
mod tests;
