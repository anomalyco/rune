//! Host-neutral pointer input events for injecting external pointer streams (for example from a
//! low-level mouse hook or another thread) through Rune's native hit-testing, hover, press, drag,
//! and wheel pipeline.

use crate::{GesturePhase, Mods, PointerButton};

const NO_MODS: Mods = Mods { shift: false, cmd: false, alt: false, ctrl: false, logo: false };

/// Scroll delta for [`PointerInput::Wheel`].
///
/// ```
/// use rune::WheelDelta;
///
/// let wheel = WheelDelta::Lines { dx: 0.0, dy: -1.0 };
/// let trackpad = WheelDelta::Points { dx: 0.0, dy: -24.0 };
/// assert_ne!(wheel, trackpad);
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WheelDelta {
    /// Discrete or fractional scroll lines (mouse wheel notches). Rune scales lines into window
    /// points using the host platform's line step and turns vertical scroll horizontal under Shift.
    Lines { dx: f32, dy: f32 },
    /// Smooth scroll delta already in logical window points (precision touchpad or pixel scroll).
    Points { dx: f32, dy: f32 },
}

impl WheelDelta {
    #[cfg(not(target_arch = "wasm32"))]
    #[inline]
    pub(crate) fn is_finite(self) -> bool {
        match self {
            Self::Lines { dx, dy } | Self::Points { dx, dy } => dx.is_finite() && dy.is_finite(),
        }
    }
}

/// One host-neutral pointer event to inject into an overlay via
/// [`OverlayHandle::send_pointer`](crate::OverlayHandle::send_pointer).
///
/// All coordinates `(x, y)` are in overlay-local logical points from the overlay's top-left
/// corner (matching the values returned by
/// [`OverlayHandle::screen_to_local`](crate::OverlayHandle::screen_to_local)). Non-finite
/// coordinates or wheel deltas are ignored at entry.
///
/// ```
/// use rune::{PointerButton, PointerInput};
///
/// let move_ev = PointerInput::move_to(24.0, 12.0);
/// let down_ev = PointerInput::down(24.0, 12.0, PointerButton::Primary);
/// let up_ev = PointerInput::up(24.0, 12.0, PointerButton::Primary);
/// let wheel_ev = PointerInput::wheel_lines(24.0, 12.0, 0.0, -1.0);
/// assert_ne!(move_ev, down_ev);
/// assert_ne!(up_ev, wheel_ev);
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerInput {
    /// Move the pointer to overlay-local logical points `(x, y)`.
    Move { x: f32, y: f32 },
    /// Press `button` at overlay-local logical points `(x, y)` with `mods` held.
    Down { x: f32, y: f32, button: PointerButton, mods: Mods },
    /// Release `button` at overlay-local logical points `(x, y)` with `mods` held.
    Up { x: f32, y: f32, button: PointerButton, mods: Mods },
    /// Scroll at overlay-local logical points `(x, y)`.
    Wheel { x: f32, y: f32, delta: WheelDelta, mods: Mods, phase: GesturePhase },
    /// The pointer left the overlay.
    Leave,
    /// Cancel any active pointer capture or drag without delivering a click or drop.
    Cancel,
}

impl PointerInput {
    /// Moves the pointer to overlay-local logical points `(x, y)`.
    #[inline]
    pub const fn move_to(x: f32, y: f32) -> Self {
        Self::Move { x, y }
    }

    /// Presses `button` at overlay-local logical points `(x, y)` with no modifiers held.
    #[inline]
    pub const fn down(x: f32, y: f32, button: PointerButton) -> Self {
        Self::Down { x, y, button, mods: NO_MODS }
    }

    /// Releases `button` at overlay-local logical points `(x, y)` with no modifiers held.
    #[inline]
    pub const fn up(x: f32, y: f32, button: PointerButton) -> Self {
        Self::Up { x, y, button, mods: NO_MODS }
    }

    /// Scrolls by `(dx, dy)` lines at overlay-local logical points `(x, y)` with no modifiers.
    #[inline]
    pub const fn wheel_lines(x: f32, y: f32, dx: f32, dy: f32) -> Self {
        Self::Wheel {
            x,
            y,
            delta: WheelDelta::Lines { dx, dy },
            mods: NO_MODS,
            phase: GesturePhase::Moved,
        }
    }

    /// Scrolls by `(dx, dy)` logical points at overlay-local logical points `(x, y)` with no
    /// modifiers.
    #[inline]
    pub const fn wheel_points(x: f32, y: f32, dx: f32, dy: f32) -> Self {
        Self::Wheel {
            x,
            y,
            delta: WheelDelta::Points { dx, dy },
            mods: NO_MODS,
            phase: GesturePhase::Moved,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[inline]
    pub(crate) fn is_finite(self) -> bool {
        match self {
            Self::Move { x, y } | Self::Down { x, y, .. } | Self::Up { x, y, .. } => {
                x.is_finite() && y.is_finite()
            }
            Self::Wheel { x, y, delta, .. } => x.is_finite() && y.is_finite() && delta.is_finite(),
            Self::Leave | Self::Cancel => true,
        }
    }
}
