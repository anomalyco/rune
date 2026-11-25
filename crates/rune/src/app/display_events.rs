//! `App::on_display_events`: a turn on the UI thread for an app's own event queue on winit's
//! display connection (a second Wayland `wl_event_queue` bound through
//! `WindowHandle::with_raw_display_handle`).
//!
//! On Wayland, any read of the display socket (winit's own, Mesa's WSI, a roundtrip) can move
//! the app queue's events into memory without dispatching any of winit's, and winit then skips
//! the turn. While a window carries this hook, winit wakes on every display read (the same
//! wake Rune's layer overlays use), and each turn's end asks the hook whether events wait; if
//! they do, the dispatch runs in the window's next turn like an `AppHandle::update_if`. A
//! window that hasn't started or has finished (closed under `keep_running`) drains no updates,
//! so it neither checks the queue nor keeps the wake on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::el::Tx;

use super::App;

/// `App::on_display_events`'s hooks.
pub(crate) struct DisplayEvents<S> {
    pub(crate) pending: fn(&S) -> bool,
    pub(crate) dispatch: fn(&mut S, &mut Tx) -> bool,
}

impl<S> Clone for DisplayEvents<S> {
    fn clone(&self) -> Self { *self }
}

impl<S> Copy for DisplayEvents<S> {}

/// A window's `App::on_display_events` hooks and whether a dispatch they queued still waits in
/// the window's updates, so a turn that ends before it drains doesn't queue another.
pub(crate) struct DisplayWatch<S> {
    events: DisplayEvents<S>,
    queued: Arc<AtomicBool>,
}

impl<S: 'static> DisplayWatch<S> {
    pub(crate) fn new(events: DisplayEvents<S>) -> Self {
        Self { events, queued: Arc::default() }
    }

    /// Queues `dispatch` as an update on `shared` (waking the loop) when `pending` says events
    /// wait and no earlier dispatch is still queued. Returns whether it queued one.
    pub(crate) fn queue_if_pending(&self, state: &S, shared: &super::Shared<S>) -> bool {
        if self.queued.load(Ordering::Relaxed) || !(self.events.pending)(state) { return false; }
        self.queued.store(true, Ordering::Relaxed);
        let (dispatch, queued) = (self.events.dispatch, self.queued.clone());
        shared.push_update(Box::new(move |s, tx| {
            queued.store(false, Ordering::Relaxed);
            dispatch(s, tx)
        }));
        true
    }
}

impl<S: 'static> App<S> {
    /// Gives an event queue the app keeps on winit's display connection a turn on the UI
    /// thread whenever events wait in it, with a [`Tx`] that can change state and ask for a
    /// frame. At the end of every turn of the app's window Rune calls `pending`; when it returns
    /// true, `dispatch` runs in the next turn (as [`AppHandle::update_if`](crate::AppHandle::update_if) does) and returns
    /// whether it changed state the view reads.
    ///
    /// On Wayland winit also wakes on every read of its display while the hook is set, so
    /// a read that only filled the app's queue (by winit, Mesa's WSI or a roundtrip) still
    /// gets a turn instead of waiting for an unrelated wake. `pending` should be cheap and
    /// make no syscall: with wayland-client, `queue.prepare_read().is_none()` (dropping the
    /// guard otherwise); `dispatch` then calls `queue.dispatch_pending`. Elsewhere only the
    /// end-of-turn check runs. Pair it with
    /// [`WindowHandle::with_raw_display_handle`](crate::WindowHandle::with_raw_display_handle)
    /// to bind the queue and [`App::on_display_close`] to drop it. Native windows only.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// #[derive(Default)]
    /// struct State { events: usize }
    ///
    /// App::new(AppSpec::new("Display", (320.0, 200.0), State::default, |_, _| el()))
    ///     .on_display_events(
    ///         |_s| false, // e.g. `s.queue.prepare_read().is_none()`
    ///         |s, _tx| { s.events += 1; true }, // e.g. `s.queue.dispatch_pending(..)`
    ///     )
    ///     .run();
    /// ```
    pub fn on_display_events(mut self, pending: fn(&S) -> bool, dispatch: fn(&mut S, &mut Tx) -> bool) -> Self {
        self.display_events = Some(DisplayEvents { pending, dispatch });
        self
    }
}

#[cfg(test)]
#[path = "display_events_tests.rs"]
mod tests;
