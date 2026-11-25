//! XDG activation tokens for programs an overlay launches. A compositor that prevents focus
//! stealing raises a launched app's window only with a token from the surface the user was
//! typing into, so the overlay asks for one while it still has the keys.

use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{self, XdgActivationTokenV1};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

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

/// None when the compositor lacks `xdg_activation_v1` or doesn't answer. A token it refuses
/// to honour still comes back; using it then only marks the launched window urgent.
pub(super) fn request(conn: &Connection, globals: &GlobalList, surface: &WlSurface, seat: Option<(&WlSeat, u32)>) -> Option<String> {
    // A queue of its own, so waiting for the answer dispatches none of the surface's input.
    let mut queue = conn.new_event_queue::<Answer>();
    let qh = queue.handle();
    let activation = globals.bind::<XdgActivationV1, _, _>(&qh, 1..=1, ()).ok()?;
    let token = activation.get_activation_token(&qh, ());
    if let Some((seat, serial)) = seat { token.set_serial(serial, seat); }
    token.set_surface(surface);
    token.commit();
    let mut answer = Answer::default();
    // `done` follows the commit; the second round covers a compositor that answers late.
    for _ in 0..2 {
        if queue.roundtrip(&mut answer).is_err() || answer.0.is_some() { break; }
    }
    token.destroy();
    activation.destroy();
    let _ = queue.flush();
    answer.0
}
