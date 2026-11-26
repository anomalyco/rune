//! Wayland `xdg_activation_v1` serial tracking and request sequencing for layer-shell
//! overlays, kept cross-platform so its transitions and protocol order tests run on every host.

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

/// Wire seam for one `xdg_activation_v1` token exchange so the exact request order
/// (`get_activation_token`, optional `set_serial`, `set_surface`, `commit`, `roundtrip`,
/// `destroy`, post-destroy queue drain, `flush`) is verified on every host.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) trait Wire {
    fn get_activation_token(&mut self);
    fn set_serial(&mut self, serial: u32);
    fn set_surface(&mut self);
    fn commit(&mut self);
    /// Runs one compositor roundtrip; returns `Ok(Some(token))` if `done` arrived, `Ok(None)`
    /// if no `done` arrived yet, or `Err(())` on connection failure.
    fn roundtrip(&mut self) -> Result<Option<String>, ()>;
    fn destroy_token(&mut self);
    fn destroy_activation(&mut self);
    fn drain_pending(&mut self);
    fn flush(&mut self);
}

/// Drives one `xdg_activation_v1` token request in protocol order. Returns `None` without
/// touching the wire when `closed` is true, or when the compositor sends no non-empty token
/// within two roundtrips.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn run_request(wire: &mut impl Wire, serial: Option<u32>, closed: bool) -> Option<String> {
    if closed { return None; }
    wire.get_activation_token();
    if let Some(serial) = serial {
        wire.set_serial(serial);
    }
    wire.set_surface();
    wire.commit();
    let mut answer = None;
    // `done` follows the commit; the second round covers a compositor that answers late.
    for _ in 0..2 {
        match wire.roundtrip() {
            Ok(Some(token)) => {
                if !token.is_empty() { answer = Some(token); }
                break;
            }
            Ok(None) => {}
            Err(()) => break,
        }
    }
    wire.destroy_token();
    wire.destroy_activation();
    wire.drain_pending();
    wire.flush();
    answer
}

#[cfg(test)]
#[path = "activation_tests.rs"]
mod tests;
