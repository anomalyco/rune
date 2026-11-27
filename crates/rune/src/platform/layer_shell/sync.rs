//! A one-shot Wayland `wl_display.sync` roundtrip on winit's shared display.
//!
//! Sent after an overlay's Wayland surface is destroyed, its `wl_callback.done` answer arrives
//! once the compositor has processed the destroy and moved keyboard focus on. Keeping the
//! callback on a private event queue lets winit's `wake_on_wayland_read` wake the event loop
//! when the answer lands, even when no layer surface is shown.

use wayland_backend::client::Backend;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use winit::raw_window_handle::WaylandDisplayHandle;

use super::super::layer_shell_machine::connection;
use super::dead::classify_turn_error;

/// Result of polling a [`HideSync`] without reading the Wayland socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyncPoll {
    /// Still waiting for the compositor's `wl_callback.done`.
    Pending,
    /// The compositor answered `wl_callback.done`.
    Answered,
    /// The display connection failed; no answer will arrive.
    Failed,
}

#[derive(Default)]
struct SyncState {
    answered: bool,
}

impl Dispatch<WlCallback, ()> for SyncState {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.answered = true;
        }
    }
}

/// One in-flight `wl_display.sync` callback on a private `EventQueue`.
pub(crate) struct HideSync {
    conn: Connection,
    queue: EventQueue<SyncState>,
    state: SyncState,
    callback: Option<WlCallback>,
    dead: bool,
}

impl HideSync {
    /// Sends `wl_display.sync` on winit's `wl_display` and flushes it. Returns `None` if the
    /// display connection has already failed.
    #[allow(unsafe_code)]
    pub(crate) fn new(display: WaylandDisplayHandle) -> Option<Self> {
        // SAFETY: winit owns this `wl_display` for the life of the event loop, which outlives
        // every overlay; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        Self::from_connection(Connection::from_backend(backend))
    }

    pub(crate) fn from_connection(conn: Connection) -> Option<Self> {
        let (kind, _) = classify_turn_error(None, None, conn.backend().last_error());
        if kind.is_some_and(connection::is_fatal) {
            return None;
        }
        let mut queue = conn.new_event_queue::<SyncState>();
        let callback = conn.display().sync(&queue.handle(), ());
        let mut state = SyncState::default();
        let flush_err = queue.flush().err();
        let (kind, _) = classify_turn_error(None, flush_err, conn.backend().last_error());
        if kind.is_some_and(connection::is_fatal) {
            conn.backend().destroy_object(&callback.id()).ok();
            while matches!(queue.dispatch_pending(&mut state), Ok(n) if n > 0) {}
            return None;
        }
        Some(Self {
            conn,
            queue,
            state,
            callback: Some(callback),
            dead: false,
        })
    }

    /// True while the sync is still waiting for `wl_callback.done` on a healthy display.
    pub(crate) fn is_live(&self) -> bool {
        !self.state.answered && !self.dead
    }

    /// True when this sync's private queue already holds unread events in libwayland memory
    /// (syscall-free check via `prepare_read`).
    pub(crate) fn has_pending_events(&self) -> bool {
        self.is_live() && self.queue.prepare_read().is_none()
    }

    /// Dispatches any queued events without reading the socket (winit reads the socket) and
    /// reports whether the sync answered, is still pending, or failed on a dead display.
    pub(crate) fn dispatch(&mut self) -> SyncPoll {
        if self.state.answered {
            return SyncPoll::Answered;
        }
        if self.dead {
            return SyncPoll::Failed;
        }
        let dispatch_err = self.queue.dispatch_pending(&mut self.state).err();
        if self.state.answered {
            // `wl_callback.done` is a destructor event; libwayland already destroyed the proxy.
            self.callback = None;
            return SyncPoll::Answered;
        }
        let flush_err = self.queue.flush().err();
        let (kind, _) = classify_turn_error(dispatch_err, flush_err, self.conn.backend().last_error());
        if kind.is_some_and(connection::is_fatal) {
            self.dead = true;
            if let Some(cb) = self.callback.take() {
                self.conn.backend().destroy_object(&cb.id()).ok();
            }
            return SyncPoll::Failed;
        }
        SyncPoll::Pending
    }

    #[cfg(test)]
    pub(crate) fn callback_id(&self) -> Option<u32> {
        self.callback.as_ref().map(|cb| cb.id().protocol_id())
    }
}

impl Drop for HideSync {
    fn drop(&mut self) {
        while matches!(self.queue.dispatch_pending(&mut self.state), Ok(n) if n > 0) {}
        if !self.state.answered
            && let Some(cb) = self.callback.take()
        {
            self.conn.backend().destroy_object(&cb.id()).ok();
        }
        while matches!(self.queue.dispatch_pending(&mut self.state), Ok(n) if n > 0) {}
        self.queue.flush().ok();
    }
}
