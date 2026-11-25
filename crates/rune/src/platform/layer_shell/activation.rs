//! XDG activation tokens for programs an overlay launches. A compositor that prevents focus
//! stealing raises a launched app's window only with a token from the surface the user was
//! typing into, so the overlay asks for one while it still has the keys.

use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{self, XdgActivationTokenV1};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

use super::super::layer_shell_machine::activation::{Wire, run_request};

#[derive(Default)]
struct Answer(Option<String>);

impl Dispatch<XdgActivationV1, ()> for Answer {
    fn event(_: &mut Self, _: &XdgActivationV1, _: <XdgActivationV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationTokenV1, ()> for Answer {
    fn event(answer: &mut Self, _: &XdgActivationTokenV1, event: xdg_activation_token_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_activation_token_v1::Event::Done { token } = event { answer.0 = Some(token); }
    }
}

struct WaylandWire<'a> {
    queue: EventQueue<Answer>,
    activation: XdgActivationV1,
    token: Option<XdgActivationTokenV1>,
    surface: &'a WlSurface,
    seat: Option<&'a WlSeat>,
    answer: Answer,
}

impl Wire for WaylandWire<'_> {
    fn get_activation_token(&mut self) {
        self.token = Some(self.activation.get_activation_token(&self.queue.handle(), ()));
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

    fn roundtrip(&mut self) -> Result<Option<String>, ()> {
        self.queue.roundtrip(&mut self.answer).map_err(|_| ())?;
        Ok(self.answer.0.take())
    }

    fn destroy_token(&mut self) {
        if let Some(token) = self.token.take() {
            token.destroy();
        }
    }

    fn destroy_activation(&mut self) {
        self.activation.destroy();
    }

    fn drain_pending(&mut self) {
        // Drain after destroying every proxy so `EventQueueInner` retains no queued events
        // (each holds a `QueueHandle` cycle) when this temporary queue drops.
        while matches!(self.queue.dispatch_pending(&mut self.answer), Ok(n) if n > 0) {}
    }

    fn flush(&mut self) {
        // Best-effort flush of the destroy requests; a broken or full socket still tears down.
        self.queue.flush().ok();
    }
}

/// None when the surface is closed, the compositor lacks `xdg_activation_v1`, or it sends no
/// non-empty token. A token it refuses to honour still comes back; using it then only marks
/// the launched window urgent.
pub(super) fn request(
    conn: &Connection,
    globals: &GlobalList,
    surface: &WlSurface,
    seat: Option<(&WlSeat, u32)>,
    closed: bool,
) -> Option<String> {
    if closed { return None; }
    // A queue of its own, so waiting for the answer dispatches none of the surface's input.
    let queue = conn.new_event_queue::<Answer>();
    let activation = globals.bind::<XdgActivationV1, _, _>(&queue.handle(), 1..=1, ()).ok()?;
    let (seat, serial) = match seat {
        Some((seat, serial)) => (Some(seat), Some(serial)),
        None => (None, None),
    };
    let mut wire = WaylandWire {
        queue,
        activation,
        token: None,
        surface,
        seat,
        answer: Answer::default(),
    };
    run_request(&mut wire, serial, false)
}
