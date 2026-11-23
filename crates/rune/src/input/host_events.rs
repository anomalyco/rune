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
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WheelInput {
    pub dx: f32,
    pub dy: f32,
    pub mods: crate::Mods,
    pub phase: crate::GesturePhase,
    pub precise: bool,
    pub notch: bool,
    /// Whether Ctrl+wheel tries `pinch_wheel` before scrolling.
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

/// Host-neutral mouse button input at view point `(x, y)`.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) struct MouseButtonInput {
    pub x: f32,
    pub y: f32,
    pub button: crate::PointerButton,
    pub pressed: bool,
    pub mods: crate::Mods,
    pub ctrl_click_is_secondary: bool,
    pub queue_mods_on_primary_down: bool,
}

/// Resolved mouse button action after macOS Ctrl+click mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) enum ButtonAction {
    PrimaryDown,
    PrimaryUp,
    SecondaryDown,
    SecondaryUp,
    OtherDown(crate::PointerButton),
    OtherUp(crate::PointerButton),
}

/// Converts physical surface pixels into view points (dividing by `rt.input_scale()` = scale × zoom).
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn physical_point(rt: &RuntimeCore, x: f64, y: f64) -> (f32, f32) {
    let scale = rt.input_scale();
    (x as f32 / scale, y as f32 / scale)
}

/// Converts logical window points (CSS pixels on web) into view points (dividing by `rt.zoom()`).
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn window_point(rt: &RuntimeCore, x: f32, y: f32) -> (f32, f32) {
    let zoom = rt.zoom();
    (x / zoom, y / zoom)
}

/// Classifies a mouse button press/release, turning primary Ctrl+click into a secondary click
/// when `ctrl_click_is_secondary` is true (macOS) and pairing its release via `control_click`.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn mouse_button_action(
    button: crate::PointerButton,
    pressed: bool,
    ctrl_held: bool,
    ctrl_click_is_secondary: bool,
    control_click: &mut bool,
) -> ButtonAction {
    match button {
        crate::PointerButton::Primary => {
            if pressed {
                *control_click = ctrl_click_is_secondary && ctrl_held;
            }
            let secondary = *control_click;
            if !pressed {
                *control_click = false;
            }
            match (pressed, secondary) {
                (true, true) => ButtonAction::SecondaryDown,
                (false, true) => ButtonAction::SecondaryUp,
                (true, false) => ButtonAction::PrimaryDown,
                (false, false) => ButtonAction::PrimaryUp,
            }
        }
        crate::PointerButton::Secondary => {
            if pressed { ButtonAction::SecondaryDown } else { ButtonAction::SecondaryUp }
        }
        other => {
            if pressed { ButtonAction::OtherDown(other) } else { ButtonAction::OtherUp(other) }
        }
    }
}

/// Dispatches a host-neutral mouse button event through the runtime, returning the resolved
/// [`ButtonAction`] so callers can run host-specific side effects.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn mouse_button_core(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    input: MouseButtonInput,
    control_click: &mut bool,
) -> ButtonAction {
    let action = mouse_button_action(
        input.button,
        input.pressed,
        input.mods.ctrl,
        input.ctrl_click_is_secondary,
        control_click,
    );
    match action {
        ButtonAction::PrimaryDown => {
            rt.pointer_down(app, input.x, input.y);
            if input.queue_mods_on_primary_down {
                rt.physical_input(crate::PhysicalInput::Modifiers(input.mods));
            }
        }
        ButtonAction::PrimaryUp => rt.pointer_up(app, input.x, input.y),
        ButtonAction::SecondaryDown => rt.secondary_down(input.x, input.y),
        ButtonAction::SecondaryUp => rt.secondary_up(app, input.x, input.y),
        ButtonAction::OtherDown(b) => rt.button_down(app, input.x, input.y, b),
        ButtonAction::OtherUp(b) => rt.button_up(app, input.x, input.y, b),
    }
    action
}

/// Maps a winit touch/gesture phase to Rune's `GesturePhase`.
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
#[allow(unsafe_code)]
pub(crate) fn touch_promoted() -> bool {
    const MI_WP_SIGNATURE: u32 = 0xFF51_5700;
    // SAFETY: reads the calling thread's current message info; no pointers involved.
    let info = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo() } as u32;
    info & 0xFFFF_FF00 == MI_WP_SIGNATURE
}

#[cfg(not(target_os = "windows"))]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn touch_promoted() -> bool { false }

/// Builds a `ModifiersState` from individual shift/ctrl/alt/meta flags.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn modifiers_from_flags(shift: bool, ctrl: bool, alt: bool, meta: bool) -> ModifiersState {
    let mut state = ModifiersState::empty();
    if shift { state |= ModifiersState::SHIFT; }
    if ctrl { state |= ModifiersState::CONTROL; }
    if alt { state |= ModifiersState::ALT; }
    if meta { state |= ModifiersState::SUPER; }
    state
}

/// Notifies the runtime of the current modifier state; returns whether a redraw is needed.
pub(crate) fn modifiers_core(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    mods: crate::Mods,
) -> bool {
    rt.physical_observation(app, crate::PhysicalInput::Modifiers(mods))
}

/// Updates tracked modifiers and notifies the runtime; returns whether a redraw is needed.
pub(crate) fn modifiers(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    mods: &mut ModifiersState,
    next: ModifiersState,
) -> bool {
    *mods = next;
    modifiers_core(rt, app, map_mods(*mods))
}

/// Updates tracked modifiers and notifies the runtime only when `next` differs from `*mods`.
/// Used where both DOM capture refresh and winit's `ModifiersChanged` report the same transition.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn sync_modifiers(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    mods: &mut ModifiersState,
    next: ModifiersState,
) -> bool {
    if *mods == next { return false; }
    modifiers(rt, app, mods, next)
}

/// Translates a host-neutral key input into runtime physical/logical key calls, including
/// clipboard paste read on non-physical fallback keys. Returns whether a redraw is needed.
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

/// Converts browser `WheelEvent.DOM_DELTA_PAGE` page counts (positive moving content right and
/// down) into window points, matching Chromium's 0.875 page fraction and Shift-axis turn.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn wheel_page_delta(rt: &RuntimeCore, dx_pages: f32, dy_pages: f32, shift: bool) -> (f32, f32) {
    const PAGE_FRACTION: f32 = 0.875;
    let (w, h) = rt.window_viewport();
    if shift && dx_pages == 0.0 {
        (dy_pages * w * PAGE_FRACTION, 0.0)
    } else {
        (dx_pages * w * PAGE_FRACTION, dy_pages * h * PAGE_FRACTION)
    }
}

/// Translates a browser wheel delta in window points through [`wheel_core`], applying browser
/// policies: Ctrl+wheel only pinches outside document mode (unclaimed Ctrl+wheel stays with
/// browser zoom), and `fit_height` canvases leave vertical wheel to the page.
/// Returns `(redraw, sync_wheel_used)`.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn web_wheel(
    rt: &mut RuntimeCore,
    dx: f32,
    dy: f32,
    mods: crate::Mods,
    phase: crate::GesturePhase,
    precise: bool,
    document: bool,
    fit_height: bool,
) -> (bool, Option<bool>) {
    let Some((x, y)) = rt.pointer() else { return (false, None) };
    // A page's pinch is the browser's zoom; elsewhere unclaimed Ctrl+wheel stays with browser
    // zoom instead of falling through to scroll.
    if mods.ctrl {
        let used = !document && rt.pinch_wheel(x, y, dy);
        return (used, Some(used));
    }
    // On a content-sized embedded canvas, vertical wheel belongs to the page unless a
    // horizontal strip is being scrolled.
    if fit_height && dx.abs() <= dy.abs() {
        return (false, None);
    }
    let outcome = wheel_core(
        rt,
        WheelInput { dx, dy, mods, phase, precise, notch: false, pinch: false },
    );
    (outcome != WheelOutcome::Ignored, None)
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
            // On macOS too: Chromium hands a page Ctrl + wheel and a trackpad pinch alike, as a
            // wheel with `ctrlKey`, so a zoom handler hears both (`MagnifyEvent::wheel` tells them
            // apart); a real pinch still arrives as its own gesture.
            pinch: true,
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

/// Host-side record of what was last sent to the native window's input method, so unchanged
/// frames don't re-submit the cursor area. On Wayland each `set_ime_cursor_area` commits
/// `zwp_text_input_v3`, which prompts a `done` reply and wakes the window for another frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum ImeSync {
    #[default]
    Disabled,
    Enabled { sent: Option<([f32; 4], f32)> },
}

/// Native window IME calls needed for one frame or focus sync.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) struct ImeStep {
    pub(crate) cancel: bool,
    pub(crate) set_allowed: Option<bool>,
    pub(crate) cursor_area: Option<[f32; 4]>,
}

impl ImeSync {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    /// Clears host IME state when the window disables IME directly (focus loss, hide, creation).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn clear(&mut self) {
        *self = Self::Disabled;
    }

    /// Drops the last-sent cursor area while keeping the current enabled state, so the next sync
    /// resends it (for example after `Ime::Enabled` when Wayland's `text_input` enters after
    /// `set_ime_allowed(true)` already ran).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn invalidate_area(&mut self) {
        if let Self::Enabled { sent } = self {
            *sent = None;
        }
    }

    /// Advances host IME state for one sync and returns the window calls to issue.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn step(
        &mut self,
        reset: bool,
        area: Option<[f32; 4]>,
        secure_focus: bool,
        focused: bool,
        scale: f32,
    ) -> ImeStep {
        let allowed = area.is_some() && !secure_focus && focused;
        let set_allowed = if reset || allowed != self.is_enabled() {
            *self = if allowed { Self::Enabled { sent: None } } else { Self::Disabled };
            Some(allowed)
        } else {
            None
        };
        let cursor_area = match (self, area) {
            (Self::Enabled { sent }, Some(a)) if *sent != Some((a, scale)) => {
                *sent = Some((a, scale));
                Some(a)
            }
            _ => None,
        };
        ImeStep { cancel: reset, set_allowed, cursor_area }
    }
}

/// Translates a winit `Ime` event (plus any native reconversion range and clauses) into the
/// runtime.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn ime(rt: &mut RuntimeCore, window: &Window, ime_state: &mut ImeSync, ime: WIme) {
    if matches!(ime, WIme::Enabled | WIme::Disabled) {
        ime_state.invalidate_area();
    }
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

/// Returns the current IME cursor area converted to window points.
pub(crate) fn ime_area_window(rt: &RuntimeCore) -> Option<[f32; 4]> {
    rt.ime_area.map(|a| rt.to_window(a))
}

/// Takes any pending IME reset flag and returns the current IME cursor area in window points.
pub(crate) fn ime_sync_state(rt: &mut RuntimeCore) -> (bool, Option<[f32; 4]>) {
    let reset = std::mem::take(&mut rt.edit.ime_reset);
    let area = ime_area_window(rt);
    (reset, area)
}

/// Releases held keys, cancels active pointer/drag capture, finishes open IME composition,
/// and clears host modifier/control-click tracking when a host hides without a blur.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn hide_reset(
    rt: &mut RuntimeCore,
    mods: &mut ModifiersState,
    control_click: &mut bool,
) {
    *mods = ModifiersState::empty();
    *control_click = false;
    rt.cancel_pointer();
    rt.finish_composition();
    rt.keys_lost();
}

/// Cancels active pointer/drag/secondary capture on pointer cancel or abnormal capture loss.
/// When `only_if_captured` is true (e.g. `lostpointercapture`, which also fires after a
/// normal `pointerup`), does nothing if no pointer capture is currently held, so hover is
/// preserved. Returns whether anything was cancelled and needs a frame.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn pointer_cancel(
    rt: &mut RuntimeCore,
    control_click: &mut bool,
    only_if_captured: bool,
) -> bool {
    if only_if_captured && !rt.pointer_captured() && !*control_click {
        return false;
    }
    *control_click = false;
    rt.cancel_pointer();
    true
}

/// Computes the IME sync step for `rt` and advances `ime_state`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn ime_sync_step(rt: &mut RuntimeCore, ime_state: &mut ImeSync, focused: bool) -> ImeStep {
    let (reset, area) = ime_sync_state(rt);
    ime_state.step(reset, area, rt.has_secure_focus(), focused, rt.window_scale())
}

/// Synchronizes a window's OS IME allowance, cursor area, and published text with the runtime.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn sync_ime(window: &Window, rt: &mut RuntimeCore, ime_state: &mut ImeSync, focused: bool) {
    let step = ime_sync_step(rt, ime_state, focused);
    let ime_allowed = ime_state.is_enabled();
    if step.cancel { crate::platform::ime::cancel(window); }
    if let Some(allowed) = step.set_allowed {
        window.set_ime_allowed(false);
        if allowed { window.set_ime_allowed(true); }
    }
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
        let want = if ime_allowed && rt.edit.ime_option_as_alt { OptionAsAlt::Both } else { OptionAsAlt::None };
        if window.option_as_alt() != want {
            window.set_option_as_alt(want);
        }
    }
    crate::platform::ime::sync(window, if ime_allowed { rt.ime_text() } else { None });
    if let Some(a) = step.cursor_area {
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
