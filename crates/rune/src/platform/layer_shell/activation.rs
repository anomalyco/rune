//! Non-blocking XDG activation tokens for programs an overlay launches. A compositor that prevents
//! focus stealing raises a launched app's window only with a token from the surface the user was
//! typing into, so the overlay commits the token request while the surface is still live (ahead of
//! any hide or `release_keys` on the same turn) and returns immediately. The
//! `xdg_activation_token_v1` proxy lives on its own private `EventQueue` so it outlives a
//! destroyed layer surface and wakes winit's event loop through `wake_on_wayland_read` when `done`
//! arrives.

use std::sync::Arc;

use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
#[cfg(test)]
use wayland_client::Proxy;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{self, XdgActivationTokenV1};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

use super::super::layer_shell_machine::activation::{StartFlush, TokenPoll, Wire, begin_request};
use super::super::layer_shell_machine::connection;
use super::dead::classify_turn_error;
use super::globals::SharedGlobals;

#[derive(Default)]
struct Answer(Option<String>);

impl Dispatch<XdgActivationTokenV1, ()> for Answer {
    fn event(answer: &mut Self, _: &XdgActivationTokenV1, event: xdg_activation_token_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            answer.0 = Some(token);
        }
    }
}

struct StartWire<'a> {
    globals: &'a SharedGlobals,
    queue: EventQueue<Answer>,
    activation: Option<XdgActivationV1>,
    token: Option<XdgActivationTokenV1>,
    surface: &'a WlSurface,
    seat: Option<&'a WlSeat>,
    state: Answer,
}

impl Wire for StartWire<'_> {
    fn get_activation_token(&mut self) {
        if let Some(activation) = &self.activation {
            self.token = Some(activation.get_activation_token(&self.queue.handle(), ()));
        }
    }

    fn set_serial(&mut self, serial: u32) {
        if let (Some(token), Some(seat)) = (&self.token, self.seat) {
            token.set_serial(serial, seat);
        }
    }

    fn set_surface(&mut self) {
        if let Some(token) = &self.token {
            token.set_surface(self.surface);
        }
    }

    fn commit(&mut self) {
        if let Some(token) = &self.token {
            token.commit();
        }
    }

    fn flush_start(&mut self) -> StartFlush {
        let flush_err = self.queue.flush().err();
        let (kind, _) = classify_turn_error(None, flush_err, self.globals.conn().backend().last_error());
        if kind.is_some_and(connection::is_fatal) {
            StartFlush::Fatal
        } else {
            StartFlush::Sent
        }
    }

    fn destroy_token(&mut self) {
        if let Some(token) = self.token.take() {
            token.destroy();
        }
    }

    fn destroy_activation(&mut self) {
        // `XdgActivationV1` is owned by `SharedGlobals` and destroyed once the last holder drops.
        self.activation = None;
    }

    fn drain_pending(&mut self) {
        let _ = self.globals.dispatch_inner_queue();
        while matches!(self.queue.dispatch_pending(&mut self.state), Ok(n) if n > 0) {}
    }

    fn flush(&mut self) {
        self.queue.flush().ok();
    }
}

/// One in-flight `xdg_activation_token_v1` request on its own private `EventQueue`.
/// Independent of `LayerSurface` so hiding the overlay or releasing keyboard focus immediately
/// after committing the request does not block the event thread or cancel the compositor's reply.
pub(crate) struct PendingActivationToken {
    queue: EventQueue<Answer>,
    state: Answer,
    token: Option<XdgActivationTokenV1>,
    seen_drain_gen: u64,
    done: bool,
    globals: Option<Arc<SharedGlobals>>,
}

impl PendingActivationToken {
    /// Commits one `xdg_activation_v1` token request and returns immediately without a blocking
    /// roundtrip. Returns `None` when the surface is closed, the compositor lacks
    /// `xdg_activation_v1`, or the display connection has failed.
    pub(super) fn start(
        globals: &Arc<SharedGlobals>,
        surface: &WlSurface,
        seat: Option<(&WlSeat, u32)>,
        closed: bool,
    ) -> Option<Self> {
        if closed { return None; }
        let (kind, _) = classify_turn_error(None, None, globals.conn().backend().last_error());
        if kind.is_some_and(connection::is_fatal) {
            return None;
        }
        let activation = globals.activation()?;
        let queue = globals.conn().new_event_queue::<Answer>();
        let (seat, serial) = match seat {
            Some((seat, serial)) => (Some(seat), Some(serial)),
            None => (None, None),
        };
        let mut wire = StartWire {
            globals,
            queue,
            activation: Some(activation),
            token: None,
            surface,
            seat,
            state: Answer::default(),
        };
        if !begin_request(&mut wire, serial, false) {
            return None;
        }
        Some(Self {
            queue: wire.queue,
            state: wire.state,
            token: wire.token,
            seen_drain_gen: globals.inner_drain_gen(),
            done: false,
            globals: Some(globals.clone()),
        })
    }

    /// True while still waiting for `xdg_activation_token_v1.done` on a healthy display.
    pub(crate) fn is_live(&self) -> bool {
        !self.done
    }

    /// True when this token's private queue already holds unread events in libwayland memory
    /// (syscall-free check via `prepare_read`) or a sibling queue drained the shared inner queue.
    pub(crate) fn has_pending_events(&self) -> bool {
        self.is_live()
            && self.globals.as_ref().is_some_and(|g| {
                g.has_pending_events(self.seen_drain_gen, self.queue.prepare_read().is_none())
            })
    }

    /// Dispatches any queued events without reading the socket (winit reads the socket) and
    /// reports whether the token arrived, is still pending, or failed on a dead display.
    pub(crate) fn dispatch(&mut self) -> TokenPoll {
        if self.done {
            return TokenPoll::Done(None);
        }
        let Some(globals) = self.globals.clone() else {
            self.done = true;
            return TokenPoll::Done(None);
        };
        let inner_err = globals.dispatch_inner_queue().err().map(wayland_client::DispatchError::Backend);
        let dispatch_err = self.queue.dispatch_pending(&mut self.state).err().or(inner_err);
        self.seen_drain_gen = globals.inner_drain_gen();
        if let Some(raw) = self.state.0.take() {
            self.teardown();
            self.done = true;
            return TokenPoll::Done((!raw.is_empty()).then_some(raw));
        }
        let flush_err = self.queue.flush().err();
        let (kind, _) = classify_turn_error(dispatch_err, flush_err, globals.conn().backend().last_error());
        if kind.is_some_and(connection::is_fatal) {
            self.teardown();
            self.done = true;
            return TokenPoll::Done(None);
        }
        TokenPoll::Pending
    }

    #[cfg(test)]
    pub(crate) fn token_id(&self) -> Option<u32> {
        self.token.as_ref().map(|token| token.id().protocol_id())
    }

    fn teardown(&mut self) {
        // Drain before and after destroying every proxy so `EventQueueInner` retains no queued
        // events (each holds a `QueueHandle` cycle) when this queue drops, then flush best-effort.
        if let Some(globals) = &self.globals {
            let _ = globals.dispatch_inner_queue();
        }
        while matches!(self.queue.dispatch_pending(&mut self.state), Ok(n) if n > 0) {}
        if let Some(token) = self.token.take() {
            token.destroy();
        }
        if let Some(globals) = &self.globals {
            let _ = globals.dispatch_inner_queue();
        }
        while matches!(self.queue.dispatch_pending(&mut self.state), Ok(n) if n > 0) {}
        self.queue.flush().ok();
        self.globals = None;
    }
}

impl Drop for PendingActivationToken {
    fn drop(&mut self) {
        self.teardown();
    }
}

#[cfg(test)]
#[path = "activation_tests.rs"]
mod tests;
