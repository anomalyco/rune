//! Pure Android `MotionEvent` action-to-phase mapping and coordinate/timing conversion helpers.
//!
//! Compiled on every target so host unit tests verify Android touch translation without a device.

use crate::TouchPhase;

/// Bitmask extracting the action code from a raw `MotionEvent.getAction()` integer.
pub(crate) const ACTION_MASK: i32 = 0xff;
/// Primary pointer touched the screen (`MotionEvent.ACTION_DOWN`).
pub(crate) const ACTION_DOWN: i32 = 0;
/// Last pointer lifted from the screen (`MotionEvent.ACTION_UP`).
pub(crate) const ACTION_UP: i32 = 1;
/// Active pointer(s) moved (`MotionEvent.ACTION_MOVE`).
pub(crate) const ACTION_MOVE: i32 = 2;
/// Gesture cancelled by Android (`MotionEvent.ACTION_CANCEL`).
pub(crate) const ACTION_CANCEL: i32 = 3;
/// Movement outside the window bounds (`MotionEvent.ACTION_OUTSIDE`); ignored as touch.
#[cfg(test)]
pub(crate) const ACTION_OUTSIDE: i32 = 4;
/// Non-primary pointer touched the screen (`MotionEvent.ACTION_POINTER_DOWN`).
pub(crate) const ACTION_POINTER_DOWN: i32 = 5;
/// Non-primary pointer lifted from the screen (`MotionEvent.ACTION_POINTER_UP`).
pub(crate) const ACTION_POINTER_UP: i32 = 6;
/// Hover move without contact (`MotionEvent.ACTION_HOVER_MOVE`); ignored as touch.
#[cfg(test)]
pub(crate) const ACTION_HOVER_MOVE: i32 = 7;
/// Non-touch scroll (`MotionEvent.ACTION_SCROLL`); ignored as touch.
#[cfg(test)]
pub(crate) const ACTION_SCROLL: i32 = 8;
/// Hover enter (`MotionEvent.ACTION_HOVER_ENTER`); ignored as touch.
#[cfg(test)]
pub(crate) const ACTION_HOVER_ENTER: i32 = 9;
/// Hover exit (`MotionEvent.ACTION_HOVER_EXIT`); ignored as touch.
#[cfg(test)]
pub(crate) const ACTION_HOVER_EXIT: i32 = 10;

/// Android API 33+ `MotionEvent.FLAG_CANCELED` (`0x20`): indicates an `ACTION_UP` or
/// `ACTION_POINTER_UP` ends a gesture cancelled by the system (for example palm rejection or a
/// system gesture), so it must not fire a tap.
pub(crate) const FLAG_CANCELED: i32 = 0x20;

/// Maps an Android `MotionEvent` action (masked or unmasked with pointer index bits) and event
/// flags to a Rune [`TouchPhase`]. Returns `None` for non-touch actions.
#[inline]
pub(crate) fn action_to_phase(action: i32, flags: i32) -> Option<TouchPhase> {
    match action & ACTION_MASK {
        ACTION_DOWN | ACTION_POINTER_DOWN => Some(TouchPhase::Started),
        ACTION_MOVE => Some(TouchPhase::Moved),
        ACTION_UP | ACTION_POINTER_UP => {
            if (flags & FLAG_CANCELED) != 0 {
                Some(TouchPhase::Cancelled)
            } else {
                Some(TouchPhase::Ended)
            }
        }
        ACTION_CANCEL => Some(TouchPhase::Cancelled),
        _ => None,
    }
}

/// Converts an Android `MotionEvent.getPointerId()` integer into a collision-free `u64`.
#[inline]
pub(crate) fn pointer_id_to_u64(pointer_id: i32) -> u64 {
    u64::from(pointer_id as u32)
}

/// Converts Android's `MotionEvent` millisecond timestamp (`SystemClock.uptimeMillis`) to seconds.
#[inline]
pub(crate) fn event_time_ms_to_secs(time_ms: i64) -> f64 {
    (time_ms as f64) * 1e-3
}

/// Converts `ViewConfiguration.getLongPressTimeout()` milliseconds into seconds.
/// Non-positive values return `None` so Rune keeps its default delay.
#[inline]
pub(crate) fn long_press_ms_to_secs(timeout_ms: i32) -> Option<f64> {
    (timeout_ms > 0).then(|| f64::from(timeout_ms) * 1e-3)
}

/// Converts a physical pixel coordinate `(x_px, y_px)` into logical dp using `scale`.
/// Returns `None` if either coordinate is non-finite.
#[inline]
pub(crate) fn px_to_dp_point(x_px: f32, y_px: f32, scale: f32) -> Option<(f32, f32)> {
    if !x_px.is_finite() || !y_px.is_finite() {
        return None;
    }
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    Some((x_px / s, y_px / s))
}
