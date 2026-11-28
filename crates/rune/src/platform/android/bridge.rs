//! Process-wide JNI helpers and bridges for Android links (`RuneView.openUrl`) and clipboard
//! (`dev.rune.RuneClipboard`).
//!
//! Kept outside `host.rs`'s UI-thread `RefCell<Option<Host>>` so `open_url` invoked during
//! `LayerHost::frame` never re-borrows `HOST`, and so the background clipboard worker thread
//! can invoke `RuneClipboard` without locking the UI host.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use jni_sys::{
    JNI_EDETACHED, JNI_FALSE, JNI_OK, JNI_TRUE, JNI_VERSION_1_6, JNIEnv, JNINativeMethod, JavaVM,
    jbyte, jbyteArray, jclass, jint, jlong, jmethodID, jobject, jobjectArray, jsize, jstring,
    jvalue,
};

static JAVA_VM: AtomicPtr<JavaVM> = AtomicPtr::new(ptr::null_mut());
static LINK_VIEW: Mutex<Option<LinkTarget>> = Mutex::new(None);
static CLIPBOARD: OnceLock<ClipboardBridge> = OnceLock::new();
static CLIPBOARD_REVISION: AtomicU64 = AtomicU64::new(0);

struct LinkTarget {
    obj: jobject,
    open_url: jmethodID,
}

// SAFETY: JNI global references and `jmethodID`s are process-wide and safe to share across threads
// when accessed through a thread-attached `JNIEnv*`.
unsafe impl Send for LinkTarget {}
// SAFETY: access is synchronized by `LINK_VIEW`.
unsafe impl Sync for LinkTarget {}

struct ClipboardBridge {
    obj: jobject,
    read_text: jmethodID,
    read_content: jmethodID,
    write_text: jmethodID,
    write_html: jmethodID,
    write_image: jmethodID,
    clear: jmethodID,
}

// SAFETY: JNI global references and `jmethodID`s are process-wide and valid for the lifetime of
// the ART process; `RuneClipboard` is thread-safe.
unsafe impl Send for ClipboardBridge {}
unsafe impl Sync for ClipboardBridge {}

struct DetachGuard(*mut JavaVM);

impl Drop for DetachGuard {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the process `JavaVM*` that we attached on this thread.
        unsafe {
            if let Some(detach) = (**self.0).DetachCurrentThread {
                let _ = detach(self.0);
            }
        }
    }
}

pub(super) fn set_java_vm(vm: *mut JavaVM) {
    JAVA_VM.store(vm, Ordering::Release);
}

/// Runs `f` with a `JNIEnv*` attached to the current thread, detaching automatically on exit
/// when this call attached the thread.
pub(super) fn with_jni_env<R>(f: impl FnOnce(*mut JNIEnv) -> R) -> Option<R> {
    let vm = JAVA_VM.load(Ordering::Acquire);
    if vm.is_null() {
        return None;
    }
    let mut env_ptr: *mut c_void = ptr::null_mut();
    // SAFETY: `vm` is valid for the lifetime of the ART process; `GetEnv` writes into `env_ptr`.
    let rc = unsafe {
        let get_env = (**vm).GetEnv?;
        get_env(vm, &mut env_ptr, JNI_VERSION_1_6)
    };
    let _detach = if rc == JNI_EDETACHED {
        // SAFETY: attaching the current Rust thread to `vm` writes a valid `JNIEnv*` to `env_ptr`.
        let attach_rc = unsafe {
            let attach = (**vm).AttachCurrentThread?;
            attach(vm, &mut env_ptr, ptr::null_mut())
        };
        if attach_rc != JNI_OK || env_ptr.is_null() {
            return None;
        }
        Some(DetachGuard(vm))
    } else if rc == JNI_OK && !env_ptr.is_null() {
        None
    } else {
        return None;
    };
    Some(f(env_ptr.cast::<JNIEnv>()))
}

pub(super) unsafe fn jni_clear_exception(env: *mut JNIEnv) -> bool {
    // SAFETY: `env` is a valid `JNIEnv*`.
    unsafe {
        if let Some(check) = (**env).ExceptionCheck
            && check(env) != JNI_FALSE
        {
            if let Some(clear) = (**env).ExceptionClear {
                clear(env);
            }
            return true;
        }
        false
    }
}

pub(super) unsafe fn jni_delete_global_ref(env: *mut JNIEnv, obj: jobject) {
    if obj.is_null() || env.is_null() {
        return;
    }
    // SAFETY: `env` is a valid `JNIEnv*` and `obj` is a live global ref.
    unsafe {
        if let Some(del) = (**env).DeleteGlobalRef {
            del(env, obj);
        }
    }
}

pub(super) fn jni_method(
    name: &'static std::ffi::CStr,
    sig: &'static std::ffi::CStr,
    fn_ptr: *mut c_void,
) -> JNINativeMethod {
    JNINativeMethod {
        name: name.as_ptr().cast_mut(),
        signature: sig.as_ptr().cast_mut(),
        fnPtr: fn_ptr,
    }
}

pub(super) unsafe fn register_natives(
    env: *mut JNIEnv,
    class_name: &'static std::ffi::CStr,
    methods: &[JNINativeMethod],
) -> bool {
    // SAFETY: `env` is a valid JNIEnv*; caller guarantees descriptors match Java declarations.
    unsafe {
        let Some(find_class) = (**env).FindClass else { return false };
        let Some(register) = (**env).RegisterNatives else { return false };
        let cls: jclass = find_class(env, class_name.as_ptr());
        if cls.is_null() {
            jni_clear_exception(env);
            return false;
        }
        let ok = register(env, cls, methods.as_ptr(), methods.len() as jint) == JNI_OK;
        if !ok {
            jni_clear_exception(env);
        }
        jni_delete_local_ref(env, cls);
        ok
    }
}

pub(super) unsafe fn call_void_0(env: *mut JNIEnv, obj: jobject, method: jmethodID) {
    // SAFETY: `env`, `obj`, and `method` are valid JNI handles for a `()V` method.
    unsafe {
        if let Some(call) = (**env).CallVoidMethodA {
            call(env, obj, method, ptr::null());
            jni_clear_exception(env);
        }
    }
}

pub(super) unsafe fn call_void_long(env: *mut JNIEnv, obj: jobject, method: jmethodID, value: jlong) {
    let arg = jvalue { j: value };
    // SAFETY: `env`, `obj`, and `method` are valid JNI handles for a `(J)V` method.
    unsafe {
        if let Some(call) = (**env).CallVoidMethodA {
            call(env, obj, method, &arg);
            jni_clear_exception(env);
        }
    }
}

pub(super) unsafe fn call_void_bool_bool(
    env: *mut JNIEnv,
    obj: jobject,
    method: jmethodID,
    first: bool,
    second: bool,
) {
    let args = [
        jvalue { z: if first { JNI_TRUE } else { JNI_FALSE } },
        jvalue { z: if second { JNI_TRUE } else { JNI_FALSE } },
    ];
    // SAFETY: `env`, `obj`, and `method` are valid JNI handles for a `(ZZ)V` method.
    unsafe {
        if let Some(call) = (**env).CallVoidMethodA {
            call(env, obj, method, args.as_ptr());
            jni_clear_exception(env);
        }
    }
}

unsafe fn jni_delete_local_ref(env: *mut JNIEnv, obj: jobject) {
    if obj.is_null() || env.is_null() {
        return;
    }
    // SAFETY: `env` is a valid `JNIEnv*` and `obj` is a live local ref.
    unsafe {
        if let Some(del) = (**env).DeleteLocalRef {
            del(env, obj);
        }
    }
}

/// Encodes a Rust UTF-8 string into a Java `String` via UTF-16 (`NewString`), avoiding
/// Modified UTF-8 aborts on supplementary-plane emoji and embedded NULs.
unsafe fn to_jstring(env: *mut JNIEnv, s: &str) -> Option<jstring> {
    let utf16: Vec<u16> = s.encode_utf16().collect();
    let len = jsize::try_from(utf16.len()).ok()?;
    // SAFETY: `env` is valid; `NewString` reads `len` UTF-16 code units without dereferencing when 0.
    unsafe {
        let new_str = (**env).NewString?;
        let jstr = new_str(env, utf16.as_ptr(), len);
        if jni_clear_exception(env) || jstr.is_null() {
            return None;
        }
        Some(jstr)
    }
}

/// Decodes a Java `String` into a Rust `String` via UTF-16 (`GetStringChars`).
pub(super) unsafe fn from_jstring(env: *mut JNIEnv, jstr: jstring) -> Option<String> {
    if jstr.is_null() {
        return None;
    }
    // SAFETY: `env` and `jstr` are valid JNI handles; `GetStringChars` is paired with `ReleaseStringChars`.
    unsafe {
        let len = (**env).GetStringLength?(env, jstr);
        if len <= 0 {
            return Some(String::new());
        }
        let chars = (**env).GetStringChars?(env, jstr, ptr::null_mut());
        if chars.is_null() {
            jni_clear_exception(env);
            return None;
        }
        let slice = std::slice::from_raw_parts(chars, len as usize);
        let out = String::from_utf16_lossy(slice);
        if let Some(release) = (**env).ReleaseStringChars {
            release(env, jstr, chars);
        }
        Some(out)
    }
}

unsafe fn to_jbyte_array(env: *mut JNIEnv, bytes: &[u8]) -> Option<jbyteArray> {
    let len = jsize::try_from(bytes.len()).ok()?;
    // SAFETY: `env` is valid; `SetByteArrayRegion` copies `len` bytes when non-empty.
    unsafe {
        let arr = (**env).NewByteArray?(env, len);
        if jni_clear_exception(env) || arr.is_null() {
            return None;
        }
        if !bytes.is_empty() {
            let Some(set_region) = (**env).SetByteArrayRegion else {
                jni_delete_local_ref(env, arr);
                return None;
            };
            set_region(env, arr, 0, len, bytes.as_ptr().cast::<jbyte>());
            if jni_clear_exception(env) {
                jni_delete_local_ref(env, arr);
                return None;
            }
        }
        Some(arr)
    }
}

unsafe fn from_jbyte_array(env: *mut JNIEnv, arr: jbyteArray) -> Option<Vec<u8>> {
    if arr.is_null() {
        return None;
    }
    // SAFETY: `env` and `arr` are valid JNI handles; `GetByteArrayRegion` writes `len` bytes.
    unsafe {
        let len = (**env).GetArrayLength?(env, arr);
        if len <= 0 {
            return Some(Vec::new());
        }
        let len_usize = usize::try_from(len).ok()?;
        let mut buf = vec![0u8; len_usize];
        let get_region = (**env).GetByteArrayRegion?;
        get_region(env, arr, 0, len, buf.as_mut_ptr().cast::<jbyte>());
        if jni_clear_exception(env) {
            return None;
        }
        Some(buf)
    }
}

unsafe extern "system" fn native_on_revision_changed(
    _env: *mut JNIEnv,
    _this: jobject,
    revision: jlong,
) {
    if revision > 0 {
        CLIPBOARD_REVISION.store(revision as u64, Ordering::Release);
    }
}

/// Registers `dev.rune.RuneClipboard.nativeOnRevisionChanged(J)V`. Missing class is non-fatal
/// so custom hosts that only embed `RuneView` still load cleanly.
pub(super) unsafe fn register_clipboard_natives(env: *mut JNIEnv) {
    let methods = [JNINativeMethod {
        name: c"nativeOnRevisionChanged".as_ptr().cast_mut(),
        signature: c"(J)V".as_ptr().cast_mut(),
        fnPtr: native_on_revision_changed as *const () as *mut c_void,
    }];
    // SAFETY: `env` is a valid `JNIEnv*`; the signature matches `RuneClipboard.kt`.
    unsafe {
        let (Some(find_class), Some(register)) = ((**env).FindClass, (**env).RegisterNatives) else {
            return;
        };
        let cls: jclass = find_class(env, c"dev/rune/RuneClipboard".as_ptr());
        if cls.is_null() {
            jni_clear_exception(env);
            return;
        }
        if register(env, cls, methods.as_ptr(), methods.len() as jint) != JNI_OK {
            jni_clear_exception(env);
        }
        jni_delete_local_ref(env, cls);
    }
}

/// Binds `RuneView.openUrl` for link launches. Replaces any previous view's global ref.
pub(super) unsafe fn set_link_target(env: *mut JNIEnv, view: jobject, open_url: jmethodID) {
    if open_url.is_null() {
        return;
    }
    // SAFETY: `env` and `view` are valid JNI handles on the UI thread.
    let global = unsafe {
        let Some(new_global) = (**env).NewGlobalRef else {
            return;
        };
        let g = new_global(env, view);
        if jni_clear_exception(env) || g.is_null() {
            return;
        }
        g
    };
    let old = LINK_VIEW
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(LinkTarget { obj: global, open_url });
    if let Some(prev) = old {
        // SAFETY: `env` is valid and `prev.obj` was created by `NewGlobalRef`.
        unsafe { jni_delete_global_ref(env, prev.obj) };
    }
}

pub(super) unsafe fn clear_link_target(env: *mut JNIEnv) {
    let old = LINK_VIEW.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(prev) = old {
        // SAFETY: `env` is valid and `prev.obj` was created by `NewGlobalRef`.
        unsafe { jni_delete_global_ref(env, prev.obj) };
    }
}

/// Caches the process-wide `dev.rune.RuneClipboard` singleton from `RuneView.getClipboard()`
/// and seeds `CLIPBOARD_REVISION`.
pub(super) unsafe fn init_clipboard(env: *mut JNIEnv, view: jobject, get_clipboard: jmethodID) {
    if CLIPBOARD.get().is_some() || get_clipboard.is_null() {
        return;
    }
    // SAFETY: `env`, `view`, and `get_clipboard` are valid JNI handles on the UI thread.
    unsafe {
        let (Some(call_obj), Some(call_long), Some(get_class), Some(get_method), Some(new_global)) = (
            (**env).CallObjectMethodA,
            (**env).CallLongMethodA,
            (**env).GetObjectClass,
            (**env).GetMethodID,
            (**env).NewGlobalRef,
        ) else {
            return;
        };
        let cb_local = call_obj(env, view, get_clipboard, ptr::null());
        if jni_clear_exception(env) || cb_local.is_null() {
            return;
        }
        let cls = get_class(env, cb_local);
        if jni_clear_exception(env) || cls.is_null() {
            jni_delete_local_ref(env, cb_local);
            return;
        }
        let revision_m = get_method(env, cls, c"revision".as_ptr(), c"()J".as_ptr());
        let read_text = get_method(env, cls, c"readText".as_ptr(), c"()Ljava/lang/String;".as_ptr());
        let read_content = get_method(
            env,
            cls,
            c"readContent".as_ptr(),
            c"(Z)[Ljava/lang/Object;".as_ptr(),
        );
        let write_text = get_method(
            env,
            cls,
            c"writeText".as_ptr(),
            c"(Ljava/lang/String;Z)Ljava/lang/String;".as_ptr(),
        );
        let write_html = get_method(
            env,
            cls,
            c"writeHtml".as_ptr(),
            c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;".as_ptr(),
        );
        let write_image = get_method(
            env,
            cls,
            c"writeImage".as_ptr(),
            c"([B)Ljava/lang/String;".as_ptr(),
        );
        let clear = get_method(env, cls, c"clear".as_ptr(), c"()Ljava/lang/String;".as_ptr());
        jni_delete_local_ref(env, cls);

        if jni_clear_exception(env)
            || revision_m.is_null()
            || read_text.is_null()
            || read_content.is_null()
            || write_text.is_null()
            || write_html.is_null()
            || write_image.is_null()
            || clear.is_null()
        {
            jni_delete_local_ref(env, cb_local);
            return;
        }

        let initial_rev = call_long(env, cb_local, revision_m, ptr::null());
        if !jni_clear_exception(env) && initial_rev > 0 {
            CLIPBOARD_REVISION.store(initial_rev as u64, Ordering::Release);
        }

        let global = new_global(env, cb_local);
        jni_delete_local_ref(env, cb_local);
        if jni_clear_exception(env) || global.is_null() {
            return;
        }
        let bridge = ClipboardBridge {
            obj: global,
            read_text,
            read_content,
            write_text,
            write_html,
            write_image,
            clear,
        };
        if let Err(unused) = CLIPBOARD.set(bridge) {
            jni_delete_global_ref(env, unused.obj);
        }
    }
}

/// Opens a validated `http:`, `https:`, or `mailto:` link via `RuneView.openUrl`.
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    with_jni_env(|env| {
        // SAFETY: `env` is attached to the current thread. Promote the cached view global ref to a
        // local ref under the lock so we never hold `LINK_VIEW` across the Java `startActivity` call.
        unsafe {
            let (view_local, method) = {
                let guard = LINK_VIEW.lock().unwrap_or_else(|e| e.into_inner());
                let Some(target) = guard.as_ref() else {
                    return Err("no active Android view".into());
                };
                let Some(new_local) = (**env).NewLocalRef else {
                    return Err("JNI NewLocalRef unavailable".into());
                };
                let local = new_local(env, target.obj);
                if jni_clear_exception(env) || local.is_null() {
                    return Err("no active Android view".into());
                }
                (local, target.open_url)
            };
            let Some(url_jstr) = to_jstring(env, url) else {
                jni_delete_local_ref(env, view_local);
                return Err("failed to encode link for Android".into());
            };
            let arg = jvalue { l: url_jstr };
            let ret = (**env)
                .CallObjectMethodA
                .map(|call| call(env, view_local, method, &arg))
                .unwrap_or(ptr::null_mut());
            jni_delete_local_ref(env, url_jstr);
            jni_delete_local_ref(env, view_local);
            if jni_clear_exception(env) {
                return Err("Android link launch threw an exception".into());
            }
            if ret.is_null() {
                return Ok(());
            }
            let msg = from_jstring(env, ret).unwrap_or_else(|| "couldn't open link".into());
            jni_delete_local_ref(env, ret);
            Err(msg)
        }
    })
    .unwrap_or_else(|| Err("Android JNI unavailable".into()))
}

pub(crate) fn clipboard_available() -> bool {
    CLIPBOARD.get().is_some()
}

pub(crate) fn clipboard_revision() -> Option<u64> {
    let rev = CLIPBOARD_REVISION.load(Ordering::Acquire);
    (rev != 0).then_some(rev)
}

unsafe fn call_string_result(
    env: *mut JNIEnv,
    obj: jobject,
    method: jmethodID,
    args: *const jvalue,
) -> Result<(), String> {
    // SAFETY: caller provides a valid attached `env`, live `obj`, matching `method`, and `args`.
    unsafe {
        let Some(call) = (**env).CallObjectMethodA else {
            return Err("JNI CallObjectMethodA unavailable".into());
        };
        let ret = call(env, obj, method, args);
        if jni_clear_exception(env) {
            return Err("Android clipboard call threw an exception".into());
        }
        if ret.is_null() {
            return Ok(());
        }
        let msg = from_jstring(env, ret).unwrap_or_else(|| "Android clipboard failed".into());
        jni_delete_local_ref(env, ret);
        Err(msg)
    }
}

pub(crate) fn clipboard_write_text(text: &str, sensitive: bool) -> Result<(), String> {
    let Some(cb) = CLIPBOARD.get() else {
        return Err("Android clipboard unavailable".into());
    };
    with_jni_env(|env| unsafe {
        let Some(jtext) = to_jstring(env, text) else {
            return Err("failed to encode clipboard text".into());
        };
        let args = [
            jvalue { l: jtext },
            jvalue {
                z: if sensitive { JNI_TRUE } else { JNI_FALSE },
            },
        ];
        let res = call_string_result(env, cb.obj, cb.write_text, args.as_ptr());
        jni_delete_local_ref(env, jtext);
        res
    })
    .unwrap_or_else(|| Err("Android JNI unavailable".into()))
}

pub(crate) fn clipboard_write_html(text: &str, html: &str) -> Result<(), String> {
    let Some(cb) = CLIPBOARD.get() else {
        return Err("Android clipboard unavailable".into());
    };
    with_jni_env(|env| unsafe {
        let Some(jtext) = to_jstring(env, text) else {
            return Err("failed to encode clipboard text".into());
        };
        let Some(jhtml) = to_jstring(env, html) else {
            jni_delete_local_ref(env, jtext);
            return Err("failed to encode clipboard HTML".into());
        };
        let args = [jvalue { l: jtext }, jvalue { l: jhtml }];
        let res = call_string_result(env, cb.obj, cb.write_html, args.as_ptr());
        jni_delete_local_ref(env, jhtml);
        jni_delete_local_ref(env, jtext);
        res
    })
    .unwrap_or_else(|| Err("Android JNI unavailable".into()))
}

pub(crate) fn clipboard_write_image(png: &[u8]) -> Result<(), String> {
    let Some(cb) = CLIPBOARD.get() else {
        return Err("Android clipboard unavailable".into());
    };
    with_jni_env(|env| unsafe {
        let Some(jbytes) = to_jbyte_array(env, png) else {
            return Err("failed to allocate clipboard image buffer".into());
        };
        let arg = jvalue { l: jbytes };
        let res = call_string_result(env, cb.obj, cb.write_image, &arg);
        jni_delete_local_ref(env, jbytes);
        res
    })
    .unwrap_or_else(|| Err("Android JNI unavailable".into()))
}

pub(crate) fn clipboard_clear() -> Result<(), String> {
    let Some(cb) = CLIPBOARD.get() else {
        return Err("Android clipboard unavailable".into());
    };
    with_jni_env(|env| unsafe { call_string_result(env, cb.obj, cb.clear, ptr::null()) })
        .unwrap_or_else(|| Err("Android JNI unavailable".into()))
}

pub(crate) fn clipboard_read_text() -> Option<String> {
    let cb = CLIPBOARD.get()?;
    with_jni_env(|env| unsafe {
        let call = (**env).CallObjectMethodA?;
        let ret = call(env, cb.obj, cb.read_text, ptr::null());
        if jni_clear_exception(env) || ret.is_null() {
            return None;
        }
        let text = from_jstring(env, ret);
        jni_delete_local_ref(env, ret);
        text
    })
    .flatten()
}

pub(crate) fn clipboard_read_content(
    include_image: bool,
) -> Option<(Option<String>, Option<String>, Option<Vec<u8>>)> {
    let cb = CLIPBOARD.get()?;
    with_jni_env(|env| unsafe {
        let call = (**env).CallObjectMethodA?;
        let get_len = (**env).GetArrayLength?;
        let get_elem = (**env).GetObjectArrayElement?;
        let arg = jvalue {
            z: if include_image { JNI_TRUE } else { JNI_FALSE },
        };
        let arr: jobjectArray = call(env, cb.obj, cb.read_content, &arg);
        if jni_clear_exception(env) || arr.is_null() {
            return None;
        }
        let len = get_len(env, arr);
        if jni_clear_exception(env) || len < 3 {
            jni_delete_local_ref(env, arr);
            return None;
        }
        let text_obj = get_elem(env, arr, 0);
        let html_obj = get_elem(env, arr, 1);
        let png_obj = get_elem(env, arr, 2);
        let failed = jni_clear_exception(env);
        let text = if failed { None } else { from_jstring(env, text_obj) };
        let html = if failed { None } else { from_jstring(env, html_obj) };
        let png = if failed { None } else { from_jbyte_array(env, png_obj) };
        jni_delete_local_ref(env, text_obj);
        jni_delete_local_ref(env, html_obj);
        jni_delete_local_ref(env, png_obj);
        jni_delete_local_ref(env, arr);
        if failed {
            None
        } else {
            Some((text, html, png))
        }
    })
    .flatten()
}
