//! Keeps a window off screen until its first frame lands. Windows shows a window before
//! anything presents into it, and DWM fills that empty client area white for a frame or more
//! (Hex #850). Cloaked, DWM composes the window and it takes paints and presents, but nothing
//! of it reaches the screen. As OpenCode Desktop's windows stay hidden until their content is
//! ready, or a timeout shows them anyway (`packages/desktop/src/main/windows/reveal-machine.ts`).
//!
//! A window that should take the focus doesn't take it as it shows: activating it starts the
//! thread's text services (TSF), about 7,500 µs on the UI thread, and that would come before the
//! frame. It shows without activating and activates as the cloak lifts, once its frame is up.

use std::cell::Cell;
use std::time::Instant;

use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};

/// Whether DWM keeps the window off screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CloakState {
    /// On screen (or hidden the ordinary way).
    #[default]
    Lifted,
    /// Cloaked until its frame lands, or `until` at the latest; `activate` when it takes the
    /// focus as it comes on screen.
    Held { until: Instant, activate: bool },
}

pub(crate) enum CloakEvent {
    /// Shown before its frame: cloak until `until` at the latest, then activate it if asked.
    Hold { until: Instant, activate: bool },
    /// Making the GPU took part of the wait: give up at `until` instead.
    Extend { until: Instant },
    /// Its frame landed.
    Lift,
    /// It hid: off without taking the focus.
    Hidden,
    /// The loop turned at `now`.
    Tick { now: Instant },
}

/// What a cloak transition asks DWM for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloakEffect {
    None,
    Cloak,
    /// Uncloak, and activate the window when the cloak was asked to.
    Uncloak { activate: bool },
}

impl CloakState {
    pub(crate) fn step(self, event: CloakEvent) -> (CloakState, CloakEffect) {
        match (self, event) {
            (CloakState::Lifted, CloakEvent::Hold { until, activate }) => (CloakState::Held { until, activate }, CloakEffect::Cloak),
            (CloakState::Held { activate: was, .. }, CloakEvent::Hold { until, activate }) => (CloakState::Held { until, activate: was || activate }, CloakEffect::None),
            (CloakState::Held { activate, .. }, CloakEvent::Extend { until }) => (CloakState::Held { until, activate }, CloakEffect::None),
            (CloakState::Held { activate, .. }, CloakEvent::Lift) => (CloakState::Lifted, CloakEffect::Uncloak { activate }),
            (CloakState::Held { .. }, CloakEvent::Hidden) => (CloakState::Lifted, CloakEffect::Uncloak { activate: false }),
            (CloakState::Held { until, activate }, CloakEvent::Tick { now }) if until <= now => (CloakState::Lifted, CloakEffect::Uncloak { activate }),
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
    /// Cloaks the window until `lift`, or `until` at the latest; with `activate` it takes
    /// the focus as the cloak lifts.
    pub(crate) fn hold(&self, window: &winit::window::Window, until: Instant, activate: bool) { self.apply(window, CloakEvent::Hold { until, activate }); }

    pub(crate) fn held(&self) -> bool { self.state.get() != CloakState::Lifted }

    /// Moves a held cloak's deadline, as when making the GPU took part of it.
    pub(crate) fn extend(&self, until: Instant) {
        self.state.set(self.state.get().step(CloakEvent::Extend { until }).0);
    }

    /// When the cloak gives up on a frame.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match self.state.get() {
            CloakState::Held { until, .. } => Some(until),
            CloakState::Lifted => None,
        }
    }

    /// Uncloaks: its frame landed.
    pub(crate) fn lift(&self, window: &winit::window::Window) { self.apply(window, CloakEvent::Lift); }

    /// Uncloaks a window that hid, without activating it.
    pub(crate) fn hidden(&self, window: &winit::window::Window) { self.apply(window, CloakEvent::Hidden); }

    /// Uncloaks once `now` is past the deadline.
    pub(crate) fn lift_if_due(&self, window: &winit::window::Window, now: Instant) { self.apply(window, CloakEvent::Tick { now }); }

    fn apply(&self, window: &winit::window::Window, event: CloakEvent) {
        let (next, effect) = self.state.get().step(event);
        match effect {
            CloakEffect::None => self.state.set(next),
            // A window DWM won't cloak shows as it is: nothing is held, and it activates now.
            CloakEffect::Cloak => {
                if set(window, true) { self.state.set(next) } else {
                    self.state.set(CloakState::Lifted);
                    if matches!(next, CloakState::Held { activate: true, .. }) { activate(window); }
                }
            }
            CloakEffect::Uncloak { activate: then } => {
                self.state.set(next);
                set(window, false);
                crate::marks::mark("window.uncloaked");
                if then { activate(window); }
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

/// Brings the window to the foreground and gives it the focus, as a show that activates would,
/// when this process may take the foreground (a launch by the user). Otherwise it stays where a
/// show without the foreground puts it, below the foreground window: a `SetForegroundWindow` that
/// Windows refuses also holds the window under the foreground window from then on, so another
/// process's `HWND_TOPMOST` (PowerToys' Always On Top) succeeds and changes nothing.
/// `AllowSetForegroundWindow` for this process fails exactly when this process can't take it.
fn activate(window: &winit::window::Window) {
    let Some(hwnd) = super::hwnd(window) else { return };
    // SAFETY: plain calls on this process's id and a window this thread owns.
    unsafe {
        if windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows_sys::Win32::System::Threading::GetCurrentProcessId()) == 0 { return; }
        windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
    }
    crate::marks::mark("window.activated");
}
