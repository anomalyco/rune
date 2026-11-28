//! Android Storage Access Framework (`content://`) document bridge over JNI.
//!
//! [`Document`] implements [`FileHandleProvider`] by calling `RuneView.openDocumentFd(uri, mode)`
//! (which invokes `ContentResolver.openFileDescriptor(uri, mode).detachFd()`) and wrapping the
//! detached POSIX file descriptor directly in `std::fs::File` with zero copies.

#![allow(unsafe_code)]

use std::io::{self, Read, Write};
use std::os::fd::FromRawFd;
use std::ptr;
use std::sync::{Arc, Mutex};

use jni_sys::{JNI_FALSE, JNI_TRUE, JNIEnv, jlong, jobject, jobjectArray, jsize, jstring, jvalue};

use crate::file_dialog::FileHandleProvider;

use super::bridge::{self, jni_clear_exception};
use super::host::{self, ViewBridge};
use super::picker::{PickAction, PickAnswer, PickRequest, PickedDocument};

/// [`FileHandleProvider`] backed by an Android `content://` document URI.
pub(super) struct Document {
    bridge: Arc<Mutex<Option<ViewBridge>>>,
    uri: String,
}

impl Document {
    pub(super) fn new(bridge: Arc<Mutex<Option<ViewBridge>>>, uri: String) -> Self {
        Self { bridge, uri }
    }

    fn open_fd(&self, mode: &str) -> io::Result<std::fs::File> {
        bridge::with_jni_env(|env| self.open_fd_with_env(env, mode)).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "Android JVM is not initialized")
        })?
    }

    fn open_fd_with_env(&self, env: *mut JNIEnv, mode: &str) -> io::Result<std::fs::File> {

        // Acquire a local reference under the mutex and drop the lock before running ContentResolver IPC.
        let (local_view, method) = {
            let guard = self.bridge.lock().unwrap_or_else(|e| e.into_inner());
            let Some(view) = guard.as_ref() else {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Android view is no longer active",
                ));
            };
            // SAFETY: `env` is attached and `view.obj` is a live global ref while `guard` is held.
            let local = unsafe { new_local_ref(env, view.obj) }.ok_or_else(|| {
                io::Error::other("failed to acquire JNI local ref for RuneView")
            })?;
            (local, view.open_document_fd)
        };

        // SAFETY: `env` is attached and `local_view` is a valid local ref owned here.
        let result = unsafe {
            let Some(uri_jstr) = new_jstring(env, &self.uri) else {
                delete_local_ref(env, local_view);
                return Err(io::Error::other("failed to allocate JNI string for document URI"));
            };
            let Some(mode_jstr) = new_jstring(env, mode) else {
                delete_local_ref(env, uri_jstr);
                delete_local_ref(env, local_view);
                return Err(io::Error::other("failed to allocate JNI string for open mode"));
            };
            let args = [jvalue { l: uri_jstr }, jvalue { l: mode_jstr }];
            let fd = if let Some(call) = (**env).CallIntMethodA {
                call(env, local_view, method, args.as_ptr())
            } else {
                -1
            };
            let exc = take_jni_exception(env);
            delete_local_ref(env, mode_jstr);
            delete_local_ref(env, uri_jstr);
            delete_local_ref(env, local_view);
            if let Some((kind, msg)) = exc {
                Err(io::Error::new(kind, msg))
            } else if fd < 0 {
                Err(io::Error::other(format!(
                    "Android ContentResolver returned invalid fd for {} ({mode})",
                    self.uri
                )))
            } else {
                // SAFETY: `ParcelFileDescriptor.detachFd()` transferred exclusive ownership of
                // this open file descriptor to Rust; `File` closes it on drop.
                Ok(std::fs::File::from_raw_fd(fd))
            }
        };
        result
    }
}

impl FileHandleProvider for Document {
    fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(self.open_fd("r")?))
    }

    fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        Ok(Box::new(self.open_fd("wt")?))
    }
}

/// Launches a pending document pick on `dev.rune.RuneView` from the UI thread.
///
/// Must be called after releasing the thread-local `HOST` borrow so synchronous error callbacks
/// from `RuneView` (e.g. when no picker launcher is registered) can safely re-enter native code.
pub(super) fn start_pick(
    env: *mut JNIEnv,
    bridge: &Arc<Mutex<Option<ViewBridge>>>,
    req: &PickRequest,
) -> Result<(), String> {
    let (local_view, method) = {
        let guard = bridge.lock().unwrap_or_else(|e| e.into_inner());
        let Some(view) = guard.as_ref() else {
            return Err("Android view is no longer active".into());
        };
        // SAFETY: `env` is the UI thread's `JNIEnv*` and `view.obj` is alive under `guard`.
        let local = unsafe { new_local_ref(env, view.obj) }
            .ok_or_else(|| "failed to acquire JNI local ref for RuneView".to_owned())?;
        (local, view.start_document_pick)
    };

    let (is_save, multiple, file_name) = match &req.action {
        PickAction::Open { multiple } => (false, *multiple, ""),
        PickAction::Create { file_name } => (true, false, file_name.as_str()),
    };

    // SAFETY: `env` is valid and all local refs are cleaned up before returning.
    unsafe {
        let Some(name_jstr) = new_jstring(env, file_name) else {
            delete_local_ref(env, local_view);
            return Err("failed to allocate JNI string for file name".into());
        };
        let Some(mime_jstr) = new_jstring(env, &req.mime_type) else {
            delete_local_ref(env, name_jstr);
            delete_local_ref(env, local_view);
            return Err("failed to allocate JNI string for MIME type".into());
        };
        let Some(extra_arr) = new_jstring_array(env, &req.extra_mime_types) else {
            delete_local_ref(env, mime_jstr);
            delete_local_ref(env, name_jstr);
            delete_local_ref(env, local_view);
            return Err("failed to allocate JNI array for extra MIME types".into());
        };
        let args = [
            jvalue { j: req.id as jlong },
            jvalue { z: if is_save { JNI_TRUE } else { JNI_FALSE } },
            jvalue { z: if multiple { JNI_TRUE } else { JNI_FALSE } },
            jvalue { l: name_jstr },
            jvalue { l: mime_jstr },
            jvalue { l: extra_arr },
        ];
        if let Some(call) = (**env).CallVoidMethodA {
            call(env, local_view, method, args.as_ptr());
        }
        let exc = take_jni_exception(env);
        delete_local_ref(env, extra_arr);
        delete_local_ref(env, mime_jstr);
        delete_local_ref(env, name_jstr);
        delete_local_ref(env, local_view);
        if let Some((_, msg)) = exc {
            Err(msg)
        } else {
            Ok(())
        }
    }
}

pub(super) unsafe extern "system" fn native_file_pick_result(
    env: *mut JNIEnv,
    _this: jobject,
    request_id: jlong,
    uris: jobjectArray,
    names: jobjectArray,
) {
    let Ok(id) = u64::try_from(request_id) else {
        return;
    };
    if id == 0 {
        return;
    }
    // SAFETY: `env`, `uris`, and `names` are valid JNI handles passed from `RuneView`.
    let (uri_list, name_list) = unsafe {
        (read_jstring_array(env, uris), read_jstring_array(env, names))
    };
    let docs: Vec<PickedDocument> = uri_list
        .into_iter()
        .enumerate()
        .filter(|(_, uri)| !uri.is_empty())
        .map(|(i, uri)| PickedDocument {
            uri,
            name: name_list.get(i).cloned().unwrap_or_default(),
        })
        .collect();
    host::deliver_file_pick_from_jni(env, id, PickAnswer::Picked(docs));
}

pub(super) unsafe extern "system" fn native_file_pick_cancelled(
    env: *mut JNIEnv,
    _this: jobject,
    request_id: jlong,
) {
    let Ok(id) = u64::try_from(request_id) else {
        return;
    };
    if id == 0 {
        return;
    }
    host::deliver_file_pick_from_jni(env, id, PickAnswer::Cancelled);
}

pub(super) unsafe extern "system" fn native_file_pick_error(
    env: *mut JNIEnv,
    _this: jobject,
    request_id: jlong,
    message: jstring,
) {
    let Ok(id) = u64::try_from(request_id) else {
        return;
    };
    if id == 0 {
        return;
    }
    // SAFETY: `env` and `message` are valid JNI handles passed from `RuneView`.
    let msg = unsafe { jstring_to_string(env, message) };
    let msg = if msg.is_empty() {
        "Android document picker failed".to_owned()
    } else {
        msg
    };
    host::deliver_file_pick_from_jni(env, id, PickAnswer::Error(msg));
}

unsafe fn new_local_ref(env: *mut JNIEnv, obj: jobject) -> Option<jobject> {
    if obj.is_null() {
        return None;
    }
    // SAFETY: `env` and `obj` are valid per caller contract.
    unsafe {
        let f = (**env).NewLocalRef?;
        let local = f(env, obj);
        if local.is_null() {
            jni_clear_exception(env);
            None
        } else {
            Some(local)
        }
    }
}

unsafe fn delete_local_ref(env: *mut JNIEnv, obj: jobject) {
    if obj.is_null() {
        return;
    }
    // SAFETY: `env` and `obj` are valid per caller contract.
    unsafe {
        if let Some(del) = (**env).DeleteLocalRef {
            del(env, obj);
        }
    }
}

unsafe fn new_jstring(env: *mut JNIEnv, text: &str) -> Option<jstring> {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: `env` is valid and `utf16` is a valid UTF-16 buffer of length `utf16.len()`.
    unsafe {
        let new_str = (**env).NewString?;
        let jstr = new_str(env, utf16.as_ptr(), utf16.len() as jsize);
        if jstr.is_null() {
            jni_clear_exception(env);
            None
        } else {
            Some(jstr)
        }
    }
}

unsafe fn jstring_to_string(env: *mut JNIEnv, s: jstring) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: `env` and `s` are valid JNI handles.
    unsafe {
        let Some(get_len) = (**env).GetStringLength else {
            return String::new();
        };
        let Some(get_chars) = (**env).GetStringChars else {
            return String::new();
        };
        let Some(rel_chars) = (**env).ReleaseStringChars else {
            return String::new();
        };
        let len = get_len(env, s);
        if len <= 0 {
            return String::new();
        }
        let chars = get_chars(env, s, ptr::null_mut());
        if chars.is_null() {
            jni_clear_exception(env);
            return String::new();
        }
        let slice = std::slice::from_raw_parts(chars, len as usize);
        let out = String::from_utf16_lossy(slice);
        rel_chars(env, s, chars);
        out
    }
}

unsafe fn new_jstring_array(env: *mut JNIEnv, items: &[String]) -> Option<jobjectArray> {
    // SAFETY: `env` is valid.
    unsafe {
        let find_class = (**env).FindClass?;
        let new_array = (**env).NewObjectArray?;
        let set_elem = (**env).SetObjectArrayElement?;
        let cls = find_class(env, c"java/lang/String".as_ptr());
        if cls.is_null() {
            jni_clear_exception(env);
            return None;
        }
        let arr = new_array(env, items.len() as jsize, cls, ptr::null_mut());
        delete_local_ref(env, cls);
        if arr.is_null() {
            jni_clear_exception(env);
            return None;
        }
        for (i, item) in items.iter().enumerate() {
            let Some(jstr) = new_jstring(env, item) else {
                delete_local_ref(env, arr);
                return None;
            };
            set_elem(env, arr, i as jsize, jstr);
            delete_local_ref(env, jstr);
            if let Some(check) = (**env).ExceptionCheck && check(env) != JNI_FALSE {
                jni_clear_exception(env);
                delete_local_ref(env, arr);
                return None;
            }
        }
        Some(arr)
    }
}

unsafe fn read_jstring_array(env: *mut JNIEnv, arr: jobjectArray) -> Vec<String> {
    if arr.is_null() {
        return Vec::new();
    }
    // SAFETY: `env` and `arr` are valid JNI handles.
    unsafe {
        let Some(get_len) = (**env).GetArrayLength else {
            return Vec::new();
        };
        let Some(get_elem) = (**env).GetObjectArrayElement else {
            return Vec::new();
        };
        let len = get_len(env, arr);
        if len <= 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            let elem = get_elem(env, arr, i);
            if let Some(check) = (**env).ExceptionCheck && check(env) != JNI_FALSE {
                jni_clear_exception(env);
                delete_local_ref(env, elem);
                break;
            }
            out.push(jstring_to_string(env, elem));
            delete_local_ref(env, elem);
        }
        out
    }
}

unsafe fn take_jni_exception(env: *mut JNIEnv) -> Option<(io::ErrorKind, String)> {
    // SAFETY: `env` is a valid JNIEnv*.
    unsafe {
        let exc = ((**env).ExceptionOccurred?)(env);
        if exc.is_null() {
            return None;
        }
        if let Some(clear) = (**env).ExceptionClear {
            clear(env);
        }
        let mut msg = String::new();
        if let (Some(get_class), Some(get_method), Some(call_obj)) = (
            (**env).GetObjectClass,
            (**env).GetMethodID,
            (**env).CallObjectMethodA,
        ) {
            let cls = get_class(env, exc);
            if !cls.is_null() {
                let to_string = get_method(
                    env,
                    cls,
                    c"toString".as_ptr(),
                    c"()Ljava/lang/String;".as_ptr(),
                );
                if !to_string.is_null() {
                    let jstr = call_obj(env, exc, to_string, ptr::null());
                    jni_clear_exception(env);
                    if !jstr.is_null() {
                        msg = jstring_to_string(env, jstr);
                        delete_local_ref(env, jstr);
                    }
                } else {
                    jni_clear_exception(env);
                }
                delete_local_ref(env, cls);
            } else {
                jni_clear_exception(env);
            }
        }
        delete_local_ref(env, exc);
        if msg.is_empty() {
            msg = "Android JNI exception".into();
        }
        let kind = if msg.contains("FileNotFoundException") {
            io::ErrorKind::NotFound
        } else if msg.contains("SecurityException") {
            io::ErrorKind::PermissionDenied
        } else if msg.contains("UnsupportedOperationException") {
            io::ErrorKind::Unsupported
        } else {
            io::ErrorKind::Other
        };
        Some((kind, msg))
    }
}
