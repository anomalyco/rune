//! Shared event translation used by desktop windows (`window::host`), window-backed overlays
//! (`overlay`) and the web host (`platform::web`), so input units, modifiers, gestures, IME and
//! keyboard routing stay identical across hosts. Host-neutral `*_core` functions operate on plain
//! Rune values so both winit and DOM-bridge callers share one translation and unit tests run
//! natively.

use std::ops::Range;

use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, Ime as WIme, MouseScrollDelta, TouchPhase};
use winit::keyboard::{ModifiersState, PhysicalKey};
use winit::window::Window;

use crate::edit::Ime;
use crate::runtime::{AppDriver, RuntimeCore, Unhandled};
use crate::window::{key_base, map_key_event, map_mods};

/// Host-neutral key input over plain Rune types.
pub(crate) struct KeyInput {
    pub physical: Option<crate::PhysicalKeyCode>,
    pub logical: Option<crate::KeyEvent>,
    pub base: Option<char>,
    pub mods: crate::Mods,
    pub pressed: bool,
    pub repeat: bool,
    pub unhandled: Option<Unhandled>,
}

/// Host-neutral wheel input in window points (before dividing by `rt.zoom()`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct WheelInput {
    pub dx: f32,
    pub dy: f32,
    pub mods: crate::Mods,
    pub phase: crate::GesturePhase,
    pub precise: bool,
    pub notch: bool,
    /// Whether Ctrl+wheel tries `pinch_wheel` before scrolling (true off macOS).
    pub pinch: bool,
}

/// How [`wheel_core`] handled a wheel input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WheelOutcome {
    /// Delivered to an `on_magnify` handler via Ctrl+wheel pinch.
    Pinched,
    /// Delivered as a `WheelEvent` (or pointer-less phase boundary notification).
    Scrolled,
    /// No pointer and no pointer-less phase boundary to report.
    Ignored,
}

/// Maps a winit touch/gesture phase to Rune's `GesturePhase`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn gesture_phase(phase: TouchPhase) -> crate::GesturePhase {
    match phase {
        TouchPhase::Started => crate::GesturePhase::Began,
        TouchPhase::Moved => crate::GesturePhase::Moved,
        TouchPhase::Ended | TouchPhase::Cancelled => crate::GesturePhase::Ended,
    }
}

/// Whether the mouse message being handled is one Windows synthesized from a touch or pen
/// (`GetMessageExtraInfo` carries `MI_WP_SIGNATURE`).
#[cfg(target_os = "windows")]
pub(crate) fn touch_promoted() -> bool {
    const MI_WP_SIGNATURE: u32 = 0xFF51_5700;
    // SAFETY: reads the calling thread's current message info; no pointers involved.
    let info = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo() } as u32;
    info & 0xFFFF_FF00 == MI_WP_SIGNATURE
}

#[cfg(not(target_os = "windows"))]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn touch_promoted() -> bool { false }

/// Notifies the runtime of the current modifier state; returns whether a redraw is needed.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn modifiers_core(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    mods: crate::Mods,
) -> bool {
    rt.physical_observation(app, crate::PhysicalInput::Modifiers(mods))
}

/// Updates tracked modifiers and notifies the runtime; returns whether a redraw is needed.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn modifiers(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    mods: &mut ModifiersState,
    next: ModifiersState,
) -> bool {
    *mods = next;
    modifiers_core(rt, app, map_mods(*mods))
}

/// Translates a host-neutral key input into runtime physical/logical key calls, including
/// clipboard paste read on non-physical fallback keys. Returns whether a redraw is needed.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn keyboard_core(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    clipboard: &crate::clipboard::Client,
    input: KeyInput,
) -> bool {
    if let Some(key) = input.physical {
        rt.physical_then(
            app,
            crate::PhysicalInput::Key { key, mods: input.mods, pressed: input.pressed, repeat: input.repeat },
            input.logical,
            input.base,
            input.unhandled,
        )
    } else if let Some(ev) = input.logical {
        if !input.pressed {
            rt.key_up(app, ev)
        } else {
            if ev.edit_command() == Some('v') && rt.accepts_paste() {
                clipboard.read_core(rt);
            } else if input.repeat {
                rt.repeat_key(ev);
            } else {
                rt.key(ev);
            }
            true
        }
    } else {
        false
    }
}

/// Translates a non-synthetic winit `KeyboardInput` into runtime physical/logical key calls,
/// including clipboard paste read on non-physical fallback keys. Returns whether a redraw is
/// needed.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn keyboard(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    clipboard: &crate::clipboard::Client,
    event: &winit::event::KeyEvent,
    mods: ModifiersState,
    unhandled: Option<Unhandled>,
) -> bool {
    let pressed = event.state == ElementState::Pressed;
    // A release's logical key only matters to `on_key_up` handlers.
    let logical = (pressed || rt.wants_key_up()).then(|| map_key_event(event, mods)).flatten();
    let physical = match event.physical_key {
        PhysicalKey::Code(key) => Some(key),
        PhysicalKey::Unidentified(_) => None,
    };
    keyboard_core(
        rt,
        app,
        clipboard,
        KeyInput {
            physical,
            logical,
            base: key_base(event),
            mods: map_mods(mods),
            pressed,
            repeat: event.repeat,
            unhandled,
        },
    )
}

/// Translates a host-neutral wheel input (window points) into runtime pinch-wheel, `WheelEvent`,
/// or pointer-less phase begin/end notifications, scaled into view points.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn wheel_core(rt: &mut RuntimeCore, input: WheelInput) -> WheelOutcome {
    match rt.pointer() {
        // Off macOS a trackpad pinch is a Ctrl + wheel.
        Some((x, y)) if input.pinch && input.mods.ctrl && rt.pinch_wheel(x, y, input.dy) => WheelOutcome::Pinched,
        Some((x, y)) => {
            let zoom = rt.zoom();
            rt.wheel_event(
                crate::WheelEvent::new(input.dx / zoom, input.dy / zoom)
                    .at(x, y)
                    .mods(input.mods)
                    .phase(input.phase)
                    .precise(input.precise)
                    .notch(input.notch),
            );
            WheelOutcome::Scrolled
        }
        None if input.phase == crate::GesturePhase::Began => {
            rt.wheel_began();
            WheelOutcome::Scrolled
        }
        None if input.phase == crate::GesturePhase::Ended => {
            rt.wheel_ended();
            WheelOutcome::Scrolled
        }
        None => WheelOutcome::Ignored,
    }
}

/// Translates a winit `MouseWheel` event into runtime pinch-wheel, `WheelEvent`, or
/// pointer-less phase begin/end notifications, scaled into view points.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn wheel(
    rt: &mut RuntimeCore,
    delta: MouseScrollDelta,
    phase: crate::GesturePhase,
    scale: f32,
    mods: ModifiersState,
) {
    let shift = crate::runtime::SHIFT_TURNS_WHEEL && mods.shift_key();
    // In window points: a wheel turns as far on screen at any zoom, as Chromium's does,
    // and a pinch is the same gesture whatever the zoom.
    #[cfg(target_os = "windows")]
    let points = rt.windows_wheel_points(delta, shift);
    #[cfg(not(target_os = "windows"))]
    let points = crate::runtime::LINE_POINTS;
    let (dx, dy) = crate::runtime::wheel_delta(delta, scale, shift, points);
    let precise = matches!(delta, MouseScrollDelta::PixelDelta(_));
    // Whole lines are a notched wheel; a free-spinning wheel or a precision touchpad
    // sends fractions (the run decides, `WheelLock::notched`). On Linux winit gives a
    // wheel's detents as whole lines too and a touchpad's as fractions or pixels.
    let notch = cfg!(any(target_os = "windows", target_os = "linux"))
        && matches!(delta, MouseScrollDelta::LineDelta(x, y) if x.fract() == 0.0 && y.fract() == 0.0);
    let _ = wheel_core(
        rt,
        WheelInput {
            dx,
            dy,
            mods: map_mods(mods),
            phase,
            precise,
            notch,
            pinch: !cfg!(target_os = "macos"),
        },
    );
}

/// Translates a winit `PinchGesture` at the current pointer; returns true when delivered.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn magnify(rt: &mut RuntimeCore, delta: f64, phase: crate::GesturePhase) -> bool {
    let Some((x, y)) = rt.pointer() else { return false };
    rt.magnify_event(crate::MagnifyEvent::new(delta as f32).at(x, y).phase(phase));
    true
}

/// Translates a winit `RotationGesture` at the current pointer; returns true when delivered.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn rotate(rt: &mut RuntimeCore, delta: f32, phase: crate::GesturePhase) -> bool {
    let Some((x, y)) = rt.pointer() else { return false };
    rt.rotate_event(crate::RotateEvent::new(delta).at(x, y).phase(phase));
    true
}

/// Translates a winit `DoubleTapGesture` at the current pointer; returns true when delivered.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn smart_magnify(rt: &mut RuntimeCore) -> bool {
    let Some((x, y)) = rt.pointer() else { return false };
    rt.smart_magnify(x, y);
    true
}

/// Translates a host-neutral IME event (with any reconversion selection range) into the runtime.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn ime_core(rt: &mut RuntimeCore, select: Option<Range<usize>>, ime: Ime) {
    // A reconversion the input method confirmed comes first: the composition it starts replaces
    // that range.
    if let Some(range) = select {
        rt.ime(Ime::Select(range));
    }
    rt.ime(ime);
}

/// Translates a winit `Ime` event (plus any native reconversion range and clauses) into the
/// runtime.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn ime(rt: &mut RuntimeCore, window: &Window, ime: WIme) {
    let select = crate::platform::ime::take_select(window);
    let event = match ime {
        WIme::Preedit(s, cur) => {
            let clauses = crate::platform::ime::take_clauses(window, &s);
            Ime::Preedit(s, cur, clauses)
        }
        WIme::Commit(s) => Ime::Commit(s),
        WIme::Enabled => Ime::Enabled,
        WIme::Disabled => Ime::Disabled,
    };
    ime_core(rt, select, event);
}

/// Takes any pending IME reset flag and returns the current IME cursor area in window points.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn ime_sync_state(rt: &mut RuntimeCore) -> (bool, Option<[f32; 4]>) {
    let reset = std::mem::take(&mut rt.ime_reset);
    let area = rt.ime_area.map(|a| rt.to_window(a));
    (reset, area)
}

/// Synchronizes a window's OS IME allowance, cursor area, and published text with the runtime.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn sync_ime(window: &Window, rt: &mut RuntimeCore, ime_state: &mut bool, focused: bool) {
    let (reset, area) = ime_sync_state(rt);
    let ime_allowed = area.is_some() && !rt.has_secure_focus() && focused;
    if reset { crate::platform::ime::cancel(window); }
    if reset || ime_allowed != *ime_state {
        window.set_ime_allowed(false);
        window.set_ime_allowed(ime_allowed);
        *ime_state = ime_allowed;
    }
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
        let want = if ime_allowed && rt.ime_option_as_alt { OptionAsAlt::Both } else { OptionAsAlt::None };
        if window.option_as_alt() != want {
            window.set_option_as_alt(want);
        }
    }
    crate::platform::ime::sync(window, if ime_allowed { rt.ime_text() } else { None });
    if let Some(a) = area {
        window.set_ime_cursor_area(
            LogicalPosition::new(a[0] as f64, a[1] as f64),
            LogicalSize::new(a[2] as f64, a[3] as f64),
        );
    }
}

/// Refreshes OS accessibility and appearance preferences sampled on window/overlay focus changes.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn refresh_focus_preferences(
    rt: &mut RuntimeCore,
    scroll_bars_override: Option<crate::platform::scroll_bars::ScrollBars>,
) {
    rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
    rt.set_system_contrast(crate::platform::preferences::contrast());
    rt.set_system_increased_contrast(crate::platform::preferences::increased_contrast());
    rt.set_full_keyboard_access(crate::platform::preferences::full_keyboard_access());
    rt.set_system_scroll_bars_state(crate::platform::scroll_bars::reread().with_override(scroll_bars_override));
}

#[cfg(test)]
#[path = "host_events_tests.rs"]
mod tests;
