//! The app quitting, as one signal whatever starts it: `Tx::quit` or `AppHandle::quit` from any
//! window, the last window closing, macOS Quit (⌘Q, the Dock, logout and shutdown, which AppKit
//! asks the app delegate about with `applicationShouldTerminate:`), and a Windows logoff or
//! shutdown (`WM_QUERYENDSESSION`, then `WM_ENDSESSION`). Every open window's `App::on_quit`
//! runs once when it starts, before any window closes for it, and [`quitting`] holds from then
//! on, so a window's `on_close` can tell the app ending from the user closing that one window.
//!
//! The windows' hooks run in the event loop (`Windows::quit`). Windows sends its session
//! messages to a window and waits for the answer, and may end the process once every
//! `WM_ENDSESSION` has returned, so the session window (`win32`) wakes the loop and runs it
//! until the hooks have run before it answers.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "windows")]
mod win32;

static QUITTING: AtomicBool = AtomicBool::new(false);
/// Counts the quits that started, so a quit after a cancelled one runs the hooks again.
static STARTED: AtomicU64 = AtomicU64::new(0);

/// Whether the app is quitting: a quit started and the system didn't cancel it. A window's
/// `on_close` that runs while this holds closes because the app ends, so it keeps what it would
/// forget when the user closes it alone (an app reopening its windows next launch keeps that
/// window's place in the list). False while the app runs, including when its last window
/// closes, whose `on_close` runs before the quit starts.
pub fn quitting() -> bool {
    QUITTING.load(Ordering::Acquire)
}

/// A quit starts (or goes on). Returns the quit's number, which `Windows` compares with the
/// last one it ran the hooks for.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn start() -> u64 {
    if !QUITTING.swap(true, Ordering::AcqRel) {
        STARTED.fetch_add(1, Ordering::AcqRel);
    }
    STARTED.load(Ordering::Acquire)
}

/// The system took the quit back (another app refused a Windows logoff): the app runs on.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn cancel() {
    QUITTING.store(false, Ordering::Release);
}

/// Hooks the system's quit requests into the loop: `send` delivers a quit (`true` when the
/// session ends now and the windows must close) to `Windows`. Once per process, on the UI thread.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn install(send: impl Fn(bool) -> bool + 'static) {
    #[cfg(target_os = "windows")]
    win32::install(send);
    #[cfg(target_os = "macos")]
    { let _ = send; mac::install(); }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = send;
}

/// `Windows` ran the hooks for a quit the system sent: its session window may answer.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn handled() {
    #[cfg(target_os = "windows")]
    win32::handled();
}
