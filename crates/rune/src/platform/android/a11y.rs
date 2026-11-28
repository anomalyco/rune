//! JNI entry points for Android TalkBack (`AccessibilityNodeProvider` + touch exploration hover).
//!
//! Every method is registered on `dev.rune.RuneView` whether or not `feature = "accessibility"`
//! is enabled so the Kotlin host never trips `UnsatisfiedLinkError`. When the feature is enabled,
//! all `accesskit_android::QueuedEvents::raise` calls happen strictly after releasing the
//! thread-local `HOST` borrow so synchronous framework callbacks cannot re-enter `RefCell`.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr;

use jni_sys::{JNI_FALSE, JNIEnv, JNINativeMethod, jboolean, jfloat, jint, jlong, jobject, jstring};

use super::bridge::jni_method;

/// Native `RuneView` accessibility methods bound during `JNI_OnLoad`.
pub(super) fn native_methods() -> [JNINativeMethod; 6] {
    [
        jni_method(
            c"nativeA11yCreateNodeInfo",
            c"(I)Landroid/view/accessibility/AccessibilityNodeInfo;",
            native_a11y_create_node_info as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeA11yFindFocus",
            c"(I)Landroid/view/accessibility/AccessibilityNodeInfo;",
            native_a11y_find_focus as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeA11yPerformAction",
            c"(IILandroid/os/Bundle;)Z",
            native_a11y_perform_action as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeA11yHover",
            c"(IFF)Z",
            native_a11y_hover as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeA11yWake",
            c"(J)V",
            native_a11y_wake as *const () as *mut c_void,
        ),
        jni_method(
            c"nativeA11yNodeFlags",
            c"(Ljava/lang/String;IIIIZLjava/lang/String;Ljava/lang/String;IZZ)I",
            native_a11y_node_flags as *const () as *mut c_void,
        ),
    ]
}

#[cfg(feature = "accessibility")]
pub(super) unsafe fn raise_events(
    env: *mut JNIEnv,
    host: jobject,
    events: accesskit_android::QueuedEvents,
) {
    if env.is_null() || host.is_null() {
        return;
    }
    // SAFETY: `env` is the UI thread's valid `JNIEnv*` and `host` is a live `RuneView` ref.
    let Ok(mut ak_env) = (unsafe { accesskit_android::jni::JNIEnv::from_raw(env.cast()) }) else {
        return;
    };
    // SAFETY: `JObject::from_raw` does not take ownership or delete `host` on drop.
    let host_obj = unsafe { accesskit_android::jni::objects::JObject::from_raw(host.cast()) };
    events.raise(&mut ak_env, &host_obj);
}

#[cfg(feature = "accessibility")]
unsafe fn jstring_to_option(
    env: &mut accesskit_android::jni::JNIEnv<'_>,
    raw: jstring,
) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` is a non-null Java `String` local reference valid for this JNI call.
    let js = unsafe { accesskit_android::jni::objects::JString::from_raw(raw.cast()) };
    env.get_string(&js).ok().map(Into::into)
}

unsafe extern "system" fn native_a11y_create_node_info(
    env: *mut JNIEnv,
    this: jobject,
    virtual_view_id: jint,
) -> jobject {
    #[cfg(not(feature = "accessibility"))]
    {
        let _ = (env, this, virtual_view_id);
        ptr::null_mut()
    }
    #[cfg(feature = "accessibility")]
    {
        if env.is_null() || this.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: `env` and `this` are valid JNI handles on the Android UI thread.
        let Ok(mut ak_env) = (unsafe { accesskit_android::jni::JNIEnv::from_raw(env.cast()) }) else {
            return ptr::null_mut();
        };
        // SAFETY: `this` is a valid `RuneView` reference for the duration of this call.
        let host_obj = unsafe { accesskit_android::jni::objects::JObject::from_raw(this.cast()) };
        super::host::with_host_mut(|host| {
            host.a11y
                .create_accessibility_node_info(&mut ak_env, &host_obj, virtual_view_id)
                .into_raw()
                .cast()
        })
        .unwrap_or(ptr::null_mut())
    }
}

unsafe extern "system" fn native_a11y_find_focus(
    env: *mut JNIEnv,
    this: jobject,
    focus_type: jint,
) -> jobject {
    #[cfg(not(feature = "accessibility"))]
    {
        let _ = (env, this, focus_type);
        ptr::null_mut()
    }
    #[cfg(feature = "accessibility")]
    {
        if env.is_null() || this.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: `env` and `this` are valid JNI handles on the Android UI thread.
        let Ok(mut ak_env) = (unsafe { accesskit_android::jni::JNIEnv::from_raw(env.cast()) }) else {
            return ptr::null_mut();
        };
        // SAFETY: `this` is a valid `RuneView` reference for the duration of this call.
        let host_obj = unsafe { accesskit_android::jni::objects::JObject::from_raw(this.cast()) };
        super::host::with_host_mut(|host| {
            host.a11y
                .find_focus(&mut ak_env, &host_obj, focus_type)
                .into_raw()
                .cast()
        })
        .unwrap_or(ptr::null_mut())
    }
}

unsafe extern "system" fn native_a11y_perform_action(
    env: *mut JNIEnv,
    this: jobject,
    virtual_view_id: jint,
    action: jint,
    arguments: jobject,
) -> jboolean {
    #[cfg(not(feature = "accessibility"))]
    {
        let _ = (env, this, virtual_view_id, action, arguments);
        JNI_FALSE
    }
    #[cfg(feature = "accessibility")]
    {
        if env.is_null() || this.is_null() {
            return JNI_FALSE;
        }
        // SAFETY: `env`, `this`, and `arguments` (nullable) are valid JNI handles on the UI thread.
        let Ok(mut ak_env) = (unsafe { accesskit_android::jni::JNIEnv::from_raw(env.cast()) }) else {
            return JNI_FALSE;
        };
        let args_obj = unsafe { accesskit_android::jni::objects::JObject::from_raw(arguments.cast()) };
        let Some(platform_action) =
            accesskit_android::PlatformAction::from_java(&mut ak_env, action, &args_obj)
        else {
            return JNI_FALSE;
        };
        let events = super::host::with_host_mut(|host| {
            host.a11y.perform_action(virtual_view_id, &platform_action)
        })
        .flatten();
        let Some(events) = events else {
            return JNI_FALSE;
        };
        // SAFETY: `HOST` borrow has ended; raise queued events on the UI thread.
        let host_obj = unsafe { accesskit_android::jni::objects::JObject::from_raw(this.cast()) };
        events.raise(&mut ak_env, &host_obj);
        jni_sys::JNI_TRUE
    }
}

unsafe extern "system" fn native_a11y_hover(
    env: *mut JNIEnv,
    this: jobject,
    action: jint,
    x: jfloat,
    y: jfloat,
) -> jboolean {
    #[cfg(not(feature = "accessibility"))]
    {
        let _ = (env, this, action, x, y);
        JNI_FALSE
    }
    #[cfg(feature = "accessibility")]
    {
        if env.is_null() || this.is_null() {
            return JNI_FALSE;
        }
        let events = super::host::with_host_mut(|host| host.a11y.on_hover_event(action, x, y))
            .flatten();
        let Some(events) = events else {
            return JNI_FALSE;
        };
        // SAFETY: `HOST` borrow has ended; raise queued hover events on the UI thread.
        unsafe { raise_events(env, this, events) };
        jni_sys::JNI_TRUE
    }
}

unsafe extern "system" fn native_a11y_wake(env: *mut JNIEnv, this: jobject, attempt: jlong) {
    #[cfg(not(feature = "accessibility"))]
    {
        let _ = (env, this, attempt);
    }
    #[cfg(feature = "accessibility")]
    {
        if env.is_null() || this.is_null() {
            return;
        }
        let attempt = crate::machine::Attempt(attempt as u32);
        let events = super::host::with_host_mut(|host| {
            let _ = host.stack.a11y_receive(&mut host.a11y);
            let sync = host.stack.a11y_wake(&mut host.a11y, attempt, crate::time::now())?;
            if let Some((next_attempt, delay)) = sync.wake_after {
                // SAFETY: `env` is the UI thread's valid `JNIEnv*`.
                unsafe { super::host::schedule_a11y_wake(env, host, next_attempt, delay) };
            }
            sync.events
        })
        .flatten();
        if let Some(events) = events {
            // SAFETY: `HOST` borrow has ended; raise queued tree-update events.
            unsafe { raise_events(env, this, events) };
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn native_a11y_node_flags(
    env: *mut JNIEnv,
    _this: jobject,
    class_name: jstring,
    left: jint,
    top: jint,
    right: jint,
    bottom: jint,
    has_bounds: jboolean,
    content_description: jstring,
    text: jstring,
    child_count: jint,
    focusable: jboolean,
    clickable: jboolean,
) -> jint {
    #[cfg(not(feature = "accessibility"))]
    {
        let _ = (
            env, _this, class_name, left, top, right, bottom, has_bounds, content_description,
            text, child_count, focusable, clickable,
        );
        0
    }
    #[cfg(feature = "accessibility")]
    {
        if env.is_null() || class_name.is_null() {
            return 0;
        }
        // SAFETY: `env` is the UI thread's valid `JNIEnv*`.
        let Ok(mut ak_env) = (unsafe { accesskit_android::jni::JNIEnv::from_raw(env.cast()) }) else {
            return 0;
        };
        // SAFETY: JNI string parameters are valid references (or null) for this call.
        let Some(class_name) = (unsafe { jstring_to_option(&mut ak_env, class_name) }) else {
            return 0;
        };
        if !crate::a11y::android::focusable::needs_flag_lookup(&class_name) {
            return 0;
        }
        let content_description = unsafe { jstring_to_option(&mut ak_env, content_description) };
        let text = unsafe { jstring_to_option(&mut ak_env, text) };
        let query = crate::a11y::android::focusable::NodeQuery {
            class_name: &class_name,
            bounds_px: (has_bounds != JNI_FALSE).then_some([left, top, right, bottom]),
            content_description: content_description.as_deref(),
            text: text.as_deref(),
            child_count: child_count.max(0) as usize,
            focusable: focusable != JNI_FALSE,
            clickable: clickable != JNI_FALSE,
        };
        super::host::with_host_mut(|host| host.a11y.node_flags(&query)).unwrap_or(0)
    }
}
