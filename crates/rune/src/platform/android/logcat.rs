//! Redirects Rust `stdout` and `stderr` to Android logcat under tag `Rune`, and installs a
//! panic hook that logs panics at `ANDROID_LOG_ERROR` priority.

#![allow(unsafe_code)]

use std::ffi::CString;
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;
use std::sync::Once;

#[link(name = "log")]
unsafe extern "C" {}

fn write_log(priority: ndk_sys::android_LogPriority, text: &str) {
    let sanitized = if text.contains('\0') {
        text.replace('\0', " ")
    } else {
        text.to_owned()
    };
    if let Ok(msg) = CString::new(sanitized) {
        // SAFETY: both tag and message are valid NUL-terminated C strings.
        unsafe {
            ndk_sys::__android_log_write(priority.0 as std::os::raw::c_int, c"Rune".as_ptr(), msg.as_ptr());
        }
    }
}

/// Installs stdout/stderr logcat forwarding and the panic log hook once per process.
pub(super) fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            write_log(ndk_sys::android_LogPriority::ANDROID_LOG_ERROR, &format!("panic: {info}"));
            prev_hook(info);
        }));

        let mut fds: [libc::c_int; 2] = [0; 2];
        // SAFETY: `fds` is a valid 2-int buffer for `pipe`.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return;
        }
        let read_fd = fds[0];
        let write_fd = fds[1];
        // SAFETY: redirecting stdout (1) and stderr (2) to the pipe write end is atomic via `dup2`.
        unsafe {
            libc::dup2(write_fd, libc::STDOUT_FILENO);
            libc::dup2(write_fd, libc::STDERR_FILENO);
            libc::close(write_fd);
        }
        let _ = std::thread::Builder::new()
            .name("rune-logcat".into())
            .spawn(move || {
                // SAFETY: `read_fd` is an owned open file descriptor from `pipe`.
                let file = unsafe { std::fs::File::from_raw_fd(read_fd) };
                let reader = BufReader::new(file);
                for line in reader.lines() {
                    match line {
                        Ok(line) if !line.is_empty() => {
                            write_log(ndk_sys::android_LogPriority::ANDROID_LOG_INFO, &line);
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            });
    });
}
