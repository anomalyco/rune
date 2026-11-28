//! Android JNI calls bridging `dev.rune.RuneView` and [`super::selection`].

#![allow(unsafe_code)]

use std::ffi::{CStr, c_void};
use std::sync::{Arc, Mutex};

use jni_sys::{
    JNI_FALSE, JNI_TRUE, JNIEnv, JNINativeMethod, jboolean, jfloat, jint, jmethodID, jobject,
    jvalue,
};

use super::bridge::jni_clear_exception;
use super::host::{self, ViewBridge};
use super::selection::{HandleKind, HandlePose, Presentation};

/// JNI method signature for `RuneView.updateSelectionUi`.
pub(super) const UPDATE_SELECTION_UI_SIG: &CStr = c"(ZFFFZFFFZFFFZFFIFFFF)V";

fn jni_method(name: &'static CStr, sig: &'static CStr, fn_ptr: *mut c_void) -> JNINativeMethod {
    JNINativeMethod {
        name: name.as_ptr().cast_mut(),
        signature: sig.as_ptr().cast_mut(),
        fnPtr: fn_ptr,
    }
}

/// Returns the native JNI methods on `dev.rune.RuneView` for touch text-selection handles and
/// floating toolbar actions.
pub(super) fn native_methods() -> [JNINativeMethod; 5] {
    [
        jni_method(
            c"nativeSelectionHandleGrab",
            c"(IFF)V",
            native_selection_handle_grab as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeSelectionHandleMove",
            c"(FF)V",
            native_selection_handle_move as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeSelectionHandleRelease",
            c"()V",
            native_selection_handle_release as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeSelectionCaretTap",
            c"()V",
            native_selection_caret_tap as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeSelectionToolbarAction",
            c"(I)V",
            native_selection_toolbar_action as *const () as *mut c_void,
        ),
    ]
}

#[inline]
fn jbool(v: bool) -> jboolean {
    if v { JNI_TRUE } else { JNI_FALSE }
}

#[inline]
fn pose_px(pose: Option<HandlePose>, scale: f32) -> (jboolean, f32, f32, f32) {
    match pose {
        Some(p) => (JNI_TRUE, p.x * scale, p.top * scale, p.bottom * scale),
        None => (JNI_FALSE, 0.0, 0.0, 0.0),
    }
}

/// Pushes `presentation` (converted from logical dp to physical view pixels via `scale`) to
/// `RuneView.updateSelectionUi` on the Android UI thread, outside any `HOST` borrow.
pub(super) unsafe fn apply_presentation_bridge(
    env: *mut JNIEnv,
    bridge: &Arc<Mutex<Option<ViewBridge>>>,
    scale: f32,
    p: Presentation,
) {
    let (obj, method): (jobject, jmethodID) = {
        let guard = bridge.lock().unwrap_or_else(|e| e.into_inner());
        let Some(view) = guard.as_ref() else { return };
        if view.update_selection_ui.is_null() || view.obj.is_null() {
            return;
        }
        // SAFETY: `env` is the UI thread's `JNIEnv*`; promote global ref to local ref so the
        // mutex is not held across the Java callback.
        let local = unsafe {
            let Some(new_local) = (**env).NewLocalRef else { return };
            let l = new_local(env, view.obj);
            if jni_clear_exception(env) || l.is_null() {
                return;
            }
            l
        };
        (local, view.update_selection_ui)
    };
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (has_start, sx, st, sb) = pose_px(p.start, s);
    let (has_end, ex, et, eb) = pose_px(p.end, s);
    let (has_caret, cx, ct, cb) = pose_px(p.caret, s);
    let (has_mag, mx, my) = match p.magnifier {
        Some((x, y)) => (JNI_TRUE, x * s, y * s),
        None => (JNI_FALSE, 0.0, 0.0),
    };
    let args = [
        jvalue { z: has_start },
        jvalue { f: sx },
        jvalue { f: st },
        jvalue { f: sb },
        jvalue { z: has_end },
        jvalue { f: ex },
        jvalue { f: et },
        jvalue { f: eb },
        jvalue { z: has_caret },
        jvalue { f: cx },
        jvalue { f: ct },
        jvalue { f: cb },
        jvalue { z: jbool(has_mag != JNI_FALSE) },
        jvalue { f: mx },
        jvalue { f: my },
        jvalue { i: p.toolbar_actions.bits() },
        jvalue { f: p.toolbar_rect[0] * s },
        jvalue { f: p.toolbar_rect[1] * s },
        jvalue { f: p.toolbar_rect[2] * s },
        jvalue { f: p.toolbar_rect[3] * s },
    ];
    // SAFETY: `env`, `obj`, and `method` are valid UI-thread JNI handles matching
    // `UPDATE_SELECTION_UI_SIG`.
    unsafe {
        if let Some(call) = (**env).CallVoidMethodA {
            call(env, obj, method, args.as_ptr());
            jni_clear_exception(env);
        }
        if let Some(del) = (**env).DeleteLocalRef {
            del(env, obj);
        }
    }
}

/// Returns the earlier of two optional wake delays in seconds.
#[inline]
pub(super) fn merge_wake(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

unsafe extern "system" fn native_selection_handle_grab(
    env: *mut JNIEnv,
    _this: jobject,
    kind: jint,
    x_px: jfloat,
    y_px: jfloat,
) {
    let Some(handle) = HandleKind::from_jni(kind) else { return };
    host::handle_selection_grab(env, handle, x_px, y_px);
}

unsafe extern "system" fn native_selection_handle_move(
    env: *mut JNIEnv,
    _this: jobject,
    x_px: jfloat,
    y_px: jfloat,
) {
    host::handle_selection_move(env, x_px, y_px);
}

unsafe extern "system" fn native_selection_handle_release(env: *mut JNIEnv, _this: jobject) {
    host::handle_selection_release(env);
}

unsafe extern "system" fn native_selection_caret_tap(env: *mut JNIEnv, _this: jobject) {
    host::handle_selection_caret_tap(env);
}

unsafe extern "system" fn native_selection_toolbar_action(
    env: *mut JNIEnv,
    _this: jobject,
    action: jint,
) {
    host::handle_selection_toolbar_action(env, action);
}
