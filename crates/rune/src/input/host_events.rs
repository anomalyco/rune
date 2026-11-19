//! Shared winit-to-runtime event translation used by both desktop windows (`window::host`) and
//! window-backed overlays (`overlay`), so input units, modifiers, gestures, IME and keyboard
//! routing stay identical across hosts.

use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, Ime as WIme, MouseScrollDelta, TouchPhase};
use winit::keyboard::{ModifiersState, PhysicalKey};
use winit::window::Window;

use crate::edit::Ime;
use crate::runtime::{AppDriver, RuntimeCore, Unhandled};
use crate::window::{key_base, map_key_event, map_mods};

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

/// Updates tracked modifiers and notifies the runtime; returns whether a redraw is needed.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn modifiers(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    mods: &mut ModifiersState,
    next: ModifiersState,
) -> bool {
    *mods = next;
    rt.physical_observation(app, crate::PhysicalInput::Modifiers(map_mods(*mods)))
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
    if let PhysicalKey::Code(key) = event.physical_key {
        rt.physical_then(
            app,
            crate::PhysicalInput::Key { key, mods: map_mods(mods), pressed, repeat: event.repeat },
            logical,
            key_base(event),
            unhandled,
        )
    } else if let Some(ev) = logical {
        if !pressed {
            rt.key_up(app, ev)
        } else {
            if ev.edit_command() == Some('v') && rt.accepts_paste() {
                clipboard.read_core(rt);
            } else if event.repeat {
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
    match rt.pointer() {
        // Off macOS a trackpad pinch is a Ctrl + wheel.
        Some((x, y)) if !cfg!(target_os = "macos") && mods.control_key() && rt.pinch_wheel(x, y, dy) => {}
        Some((x, y)) => rt.wheel_event(
            crate::WheelEvent::new(dx / rt.zoom(), dy / rt.zoom())
                .at(x, y)
                .mods(map_mods(mods))
                .phase(phase)
                .precise(precise)
                .notch(notch),
        ),
        None if phase == crate::GesturePhase::Began => rt.wheel_began(),
        None if phase == crate::GesturePhase::Ended => rt.wheel_ended(),
        None => {}
    }
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

/// Translates a winit `Ime` event (plus any native reconversion range and clauses) into the
/// runtime.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn ime(rt: &mut RuntimeCore, window: &Window, ime: WIme) {
    // A reconversion the input method confirmed comes first: the composition it starts replaces
    // that range.
    if let Some(range) = crate::platform::ime::take_select(window) {
        rt.ime(Ime::Select(range));
    }
    match ime {
        WIme::Preedit(s, cur) => {
            let clauses = crate::platform::ime::take_clauses(window, &s);
            rt.ime(Ime::Preedit(s, cur, clauses));
        }
        WIme::Commit(s) => rt.ime(Ime::Commit(s)),
        WIme::Enabled => rt.ime(Ime::Enabled),
        WIme::Disabled => rt.ime(Ime::Disabled),
    }
}

/// Synchronizes a window's OS IME allowance, cursor area, and published text with the runtime.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn sync_ime(window: &Window, rt: &mut RuntimeCore, ime_state: &mut bool, focused: bool) {
    let ime_allowed = rt.ime_area.is_some() && !rt.has_secure_focus() && focused;
    let reset = std::mem::take(&mut rt.ime_reset);
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
    if let Some(a) = rt.ime_area.map(|a| rt.to_window(a)) {
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
