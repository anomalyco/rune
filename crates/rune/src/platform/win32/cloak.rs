//! Keeps a window off screen until its first frame lands. Windows shows a window before
//! anything presents into it, and DWM fills that empty client area white for a frame or more
//! (Hex #850). Cloaked, DWM composes the window and it takes paints and presents, but nothing
//! of it reaches the screen. As OpenCode Desktop's windows stay hidden until their content is
//! ready, or a timeout shows them anyway (`packages/desktop/src/main/windows/reveal-machine.ts`).

use std::cell::Cell;
use std::time::Instant;

use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};

/// Whether DWM keeps the window off screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CloakState {
    /// On screen (or hidden the ordinary way).
    #[default]
    Lifted,
    /// Cloaked until its frame lands, or `until` at the latest.
    Held { until: Instant },
}

pub(crate) enum CloakEvent {
    /// Shown before its frame: cloak until `until` at the latest.
    Hold { until: Instant },
    /// Making the GPU took part of the wait: give up at `until` instead.
    Extend { until: Instant },
    /// Its frame landed, or it hid.
    Lift,
    /// The loop turned at `now`.
    Tick { now: Instant },
}

/// What a cloak transition asks DWM for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloakEffect {
    None,
    /// Cloak (true) or uncloak (false).
    Set(bool),
}

impl CloakState {
    pub(crate) fn step(self, event: CloakEvent) -> (CloakState, CloakEffect) {
        match (self, event) {
            (state, CloakEvent::Hold { until }) => (CloakState::Held { until }, if state == CloakState::Lifted { CloakEffect::Set(true) } else { CloakEffect::None }),
            (CloakState::Held { .. }, CloakEvent::Extend { until }) => (CloakState::Held { until }, CloakEffect::None),
            (CloakState::Held { .. }, CloakEvent::Lift) => (CloakState::Lifted, CloakEffect::Set(false)),
            (CloakState::Held { until }, CloakEvent::Tick { now }) if until <= now => (CloakState::Lifted, CloakEffect::Set(false)),
            (state, _) => (state, CloakEffect::None),
        }
    }
}

/// A window's cloak.
#[derive(Default)]
pub(crate) struct Cloak {
    state: Cell<CloakState>,
}

impl Cloak {
    /// Cloaks the window until `lift`, or `until` at the latest.
    pub(crate) fn hold(&self, window: &winit::window::Window, until: Instant) { self.apply(window, CloakEvent::Hold { until }); }

    pub(crate) fn held(&self) -> bool { self.state.get() != CloakState::Lifted }

    /// Moves a held cloak's deadline, as when making the GPU took part of it.
    pub(crate) fn extend(&self, until: Instant) {
        self.state.set(self.state.get().step(CloakEvent::Extend { until }).0);
    }

    /// When the cloak gives up on a frame.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match self.state.get() {
            CloakState::Held { until } => Some(until),
            CloakState::Lifted => None,
        }
    }

    /// Uncloaks: its frame landed, or it hid.
    pub(crate) fn lift(&self, window: &winit::window::Window) { self.apply(window, CloakEvent::Lift); }

    /// Uncloaks once `now` is past the deadline.
    pub(crate) fn lift_if_due(&self, window: &winit::window::Window, now: Instant) { self.apply(window, CloakEvent::Tick { now }); }

    fn apply(&self, window: &winit::window::Window, event: CloakEvent) {
        let (next, effect) = self.state.get().step(event);
        match effect {
            CloakEffect::None => self.state.set(next),
            // A window DWM won't cloak shows as it is: nothing is held.
            CloakEffect::Set(true) => self.state.set(if set(window, true) { next } else { CloakState::Lifted }),
            CloakEffect::Set(false) => {
                self.state.set(next);
                set(window, false);
            }
        }
    }
}

fn set(window: &winit::window::Window, cloaked: bool) -> bool {
    let Some(hwnd) = super::hwnd(window) else { return false };
    let value = i32::from(cloaked);
    // SAFETY: DWMWA_CLOAK reads one BOOL from `value` for a window this thread owns.
    unsafe { DwmSetWindowAttribute(hwnd, DWMWA_CLOAK as u32, (&raw const value).cast(), 4) >= 0 }
}
