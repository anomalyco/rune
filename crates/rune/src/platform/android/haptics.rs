//! Android haptic feedback via `View.performHapticFeedback(int)` and `HapticFeedbackConstants`.
//!
//! Pure mapping and flush helpers live here so every [`Haptic`] variant, API-level fallback, and
//! drain-once host contract are verified on non-Android test targets as well.

#![cfg_attr(target_os = "android", allow(unsafe_code))]

use crate::{Haptic, HapticOutcome, ImpactStrength};

#[cfg(test)]
#[path = "haptics_tests.rs"]
mod tests;

/// `android.view.HapticFeedbackConstants.LONG_PRESS` (API 3).
pub(crate) const LONG_PRESS: i32 = 0;
/// `android.view.HapticFeedbackConstants.VIRTUAL_KEY` (API 5).
pub(crate) const VIRTUAL_KEY: i32 = 1;
/// `android.view.HapticFeedbackConstants.KEYBOARD_TAP` (API 8).
pub(crate) const KEYBOARD_TAP: i32 = 3;
/// `android.view.HapticFeedbackConstants.CLOCK_TICK` (API 21).
pub(crate) const CLOCK_TICK: i32 = 4;
/// `android.view.HapticFeedbackConstants.CONTEXT_CLICK` (API 23).
pub(crate) const CONTEXT_CLICK: i32 = 6;
/// `android.view.HapticFeedbackConstants.CONFIRM` (API 30).
pub(crate) const CONFIRM: i32 = 16;
/// `android.view.HapticFeedbackConstants.REJECT` (API 30).
pub(crate) const REJECT: i32 = 17;
/// `android.view.HapticFeedbackConstants.SEGMENT_TICK` (API 34).
pub(crate) const SEGMENT_TICK: i32 = 26;
/// `android.view.HapticFeedbackConstants.SEGMENT_FREQUENT_TICK` (API 34).
pub(crate) const SEGMENT_FREQUENT_TICK: i32 = 27;

/// Maps a semantic Rune [`Haptic`] to the `android.view.HapticFeedbackConstants` integer code
/// supported by `api_level` (`android.os.Build.VERSION.SDK_INT`), matching the platform table in
/// [`crate::haptics`].
///
/// The returned code is passed to `View.performHapticFeedback(int)` with no override flags so
/// Android requires no `VIBRATE` permission and honours the system's Touch Feedback setting.
pub(crate) const fn feedback_constant(haptic: Haptic, api_level: i32) -> i32 {
    match haptic {
        Haptic::Selection => {
            if api_level >= 34 {
                SEGMENT_TICK
            } else {
                CLOCK_TICK
            }
        }
        Haptic::Impact(ImpactStrength::Light) => VIRTUAL_KEY,
        Haptic::Impact(ImpactStrength::Medium) => KEYBOARD_TAP,
        Haptic::Impact(ImpactStrength::Heavy) => {
            if api_level >= 23 {
                CONTEXT_CLICK
            } else {
                LONG_PRESS
            }
        }
        Haptic::Impact(ImpactStrength::Soft) => {
            if api_level >= 34 {
                SEGMENT_FREQUENT_TICK
            } else {
                CLOCK_TICK
            }
        }
        Haptic::Impact(ImpactStrength::Rigid) => {
            if api_level >= 23 {
                CONTEXT_CLICK
            } else {
                KEYBOARD_TAP
            }
        }
        Haptic::Notify(HapticOutcome::Success) => {
            if api_level >= 30 {
                CONFIRM
            } else {
                VIRTUAL_KEY
            }
        }
        Haptic::Notify(HapticOutcome::Warning) => KEYBOARD_TAP,
        Haptic::Notify(HapticOutcome::Error) => {
            if api_level >= 30 {
                REJECT
            } else {
                LONG_PRESS
            }
        }
    }
}

/// Dispatches drained haptics for an Android frame.
///
/// When `play` is true (a live, visible frame presented on screen), maps each queued [`Haptic`]
/// via [`feedback_constant`] for `api_level` and passes the `HapticFeedbackConstants` code to
/// `perform` in FIFO order. When `play` is false (missed/detached/paused), discards the drained
/// haptics without invoking `perform`.
pub(crate) fn flush_with(
    haptics: impl IntoIterator<Item = Haptic>,
    api_level: i32,
    play: bool,
    mut perform: impl FnMut(i32),
) {
    if !play {
        return;
    }
    for haptic in haptics {
        perform(feedback_constant(haptic, api_level));
    }
}

/// Reads `android.os.Build.VERSION.SDK_INT` once from JNI, returning `0` if unavailable.
#[cfg(target_os = "android")]
pub(super) unsafe fn read_sdk_int(env: *mut jni_sys::JNIEnv) -> i32 {
    // SAFETY: `env` is a valid UI-thread `JNIEnv*`.
    unsafe {
        let Some(find_class) = (**env).FindClass else {
            return 0;
        };
        let Some(get_static_field_id) = (**env).GetStaticFieldID else {
            return 0;
        };
        let Some(get_static_int_field) = (**env).GetStaticIntField else {
            return 0;
        };
        let cls = find_class(env, c"android/os/Build$VERSION".as_ptr());
        if cls.is_null() {
            clear_exception(env);
            return 0;
        }
        let field = get_static_field_id(env, cls, c"SDK_INT".as_ptr(), c"I".as_ptr());
        let sdk_int = if field.is_null() {
            clear_exception(env);
            0
        } else {
            let value = get_static_int_field(env, cls, field);
            clear_exception(env);
            value
        };
        if let Some(del) = (**env).DeleteLocalRef {
            del(env, cls);
        }
        sdk_int
    }
}

/// Flushes drained haptics into `RuneView.performHaptic(Int)` over JNI.
#[cfg(target_os = "android")]
pub(super) unsafe fn flush_jni(
    env: *mut jni_sys::JNIEnv,
    view: jni_sys::jobject,
    method: jni_sys::jmethodID,
    api_level: i32,
    play: bool,
    haptics: Vec<Haptic>,
) {
    flush_with(haptics, api_level, play, |code| {
        let arg = jni_sys::jvalue { i: code };
        // SAFETY: `env`, `view`, and `method` are valid JNI handles for `performHaptic(I)V`.
        unsafe {
            if let Some(call) = (**env).CallVoidMethodA {
                call(env, view, method, &arg);
                clear_exception(env);
            }
        }
    });
}

#[cfg(target_os = "android")]
unsafe fn clear_exception(env: *mut jni_sys::JNIEnv) {
    // SAFETY: `env` is a valid `JNIEnv*`.
    unsafe {
        if let Some(check) = (**env).ExceptionCheck
            && check(env) != jni_sys::JNI_FALSE
            && let Some(clear) = (**env).ExceptionClear
        {
            clear(env);
        }
    }
}
