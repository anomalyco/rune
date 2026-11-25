//! Thin desktop window seam for declarative pointer lock (`El::pointer_lock`): cursor grab
//! mode negotiation, cursor hiding while locked and focused, and per-platform raw motion scaling.
//!
//! Platform grab modes (matching winit 0.30):
//! - **Windows**: `CursorGrabMode::Locked` first (1×1 cursor clip), falling back to `Confined`;
//!   `DeviceEvent::MouseMotion` carries raw device counts divided by `scale_factor()`.
//! - **macOS**: `CursorGrabMode::Locked` (`CGAssociateMouseAndMouseCursorPosition(false)`;
//!   `Confined` is unsupported); `DeviceEvent::MouseMotion` already reports logical points.
//! - **Wayland**: `CursorGrabMode::Locked` via `zwp_pointer_constraints_v1`, falling back to
//!   `Confined`; `DeviceEvent::MouseMotion` (`zwp_relative_pointer_v1`) reports logical units.
//! - **X11**: `CursorGrabMode::Locked` returns `NotSupported`, so it falls back to
//!   `CursorGrabMode::Confined` (`xcb_grab_pointer`); XInput2 `RawMotion` device counts are
//!   divided by `scale_factor()`.
//! - **Web**: handled asynchronously in `platform/web` via the browser Pointer Lock API
//!   (`requestPointerLock`, `pointerlockchange`, `pointerlockerror`, `movementX`/`movementY`).
//! - **iOS & native overlays**: pointer lock is disabled (`set_pointer_lock_supported(false)`).

use winit::window::CursorGrabMode;

use crate::el::Cursor;
use crate::runtime::{LockEffect, RuntimeCore};
use crate::window::surface::Win;

/// Outcome of applying a [`LockEffect`] through a window cursor-grab seam.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrabOutcome {
    /// Applied `CursorGrabMode::Locked`.
    Locked,
    /// `Locked` was refused; fell back to `CursorGrabMode::Confined`.
    Confined,
    /// Released the grab (`CursorGrabMode::None`).
    Released,
}

/// Applies `effect` through `set_grab`:
/// - `LockEffect::Grab`: tries `CursorGrabMode::Locked` first, falling back to
///   `CursorGrabMode::Confined`; returns `Err((locked_err, confined_err))` when both fail.
/// - `LockEffect::Ungrab`: calls `CursorGrabMode::None`, ignoring errors.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn apply_grab_with<E>(
    effect: LockEffect,
    mut set_grab: impl FnMut(CursorGrabMode) -> Result<(), E>,
) -> Result<GrabOutcome, (E, E)> {
    match effect {
        LockEffect::Grab => match set_grab(CursorGrabMode::Locked) {
            Ok(()) => Ok(GrabOutcome::Locked),
            Err(locked_err) => match set_grab(CursorGrabMode::Confined) {
                Ok(()) => Ok(GrabOutcome::Confined),
                Err(confined_err) => Err((locked_err, confined_err)),
            },
        },
        LockEffect::Ungrab => {
            // Releasing a grab the OS already dropped (for example on focus loss) is benign.
            set_grab(CursorGrabMode::None).ok();
            Ok(GrabOutcome::Released)
        }
    }
}

/// Platform unit convention for `winit::event::DeviceEvent::MouseMotion`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionPlatform {
    /// macOS (`NSEvent` delta) and Wayland (`zwp_relative_pointer_v1`): already logical points.
    LogicalPoints,
    /// Windows (`WM_INPUT`) and X11 (XInput2 `RawMotion`): device counts divided by `scale_factor`.
    DeviceCounts,
}

impl MotionPlatform {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    #[inline]
    pub(crate) const fn for_host(wayland: bool) -> Self {
        if cfg!(target_os = "macos") || wayland {
            Self::LogicalPoints
        } else {
            Self::DeviceCounts
        }
    }
}

/// Scales raw `DeviceEvent::MouseMotion` `(dx, dy)` into Rune points (`[f32; 2]`).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[inline]
pub(crate) fn scale_motion(
    dx: f64,
    dy: f64,
    scale_factor: f64,
    platform: MotionPlatform,
) -> [f32; 2] {
    let div = match platform {
        MotionPlatform::LogicalPoints => 1.0,
        MotionPlatform::DeviceCounts => scale_factor.max(f64::EPSILON),
    };
    [(dx / div) as f32, (dy / div) as f32]
}

/// Resolves the window cursor while accounting for pointer lock and window focus:
/// pointer lock forces `Cursor::Hidden` whenever the window has focus; otherwise `Cursor::Hidden`
/// falls back to `Cursor::Default` when unfocused.
#[inline]
pub(crate) fn resolve_cursor(cursor: Cursor, pointer_locked: bool, focused: bool) -> Cursor {
    if pointer_locked && focused {
        return Cursor::Hidden;
    }
    match cursor {
        Cursor::Hidden if !focused => Cursor::Default,
        other => other,
    }
}

impl Win {
    /// Applies any pending pointer-lock grab or release owed by `rt` to the native window.
    /// Logs once to stderr if both `Locked` and `Confined` fail; raw device motion still arrives.
    pub(in crate::window) fn apply_pointer_lock_core(&self, rt: &mut RuntimeCore) {
        let Some(effect) = rt.take_pointer_lock_effect() else { return };
        if let Err((locked_err, confined_err)) =
            apply_grab_with(effect, |mode| self.window.set_cursor_grab(mode))
        {
            eprintln!(
                "rune: pointer lock grab failed (Locked: {locked_err}; Confined: {confined_err})"
            );
        }
    }
}

#[cfg(test)]
#[path = "pointer_lock_tests.rs"]
mod tests;
