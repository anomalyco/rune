//! Locking the session (Win+L) wipes the secret Rune copied last, if the clipboard still holds it,
//! as `Tx::clear_secret_clipboard` does: a token copied and left there doesn't wait for its
//! countdown while the desk is empty. Each window asks for session notifications once, after its
//! first frame (`wtsapi32.dll` loads then, not with the process); the system posts
//! `WM_WTSSESSION_CHANGE` only when the session changes, so it costs nothing while idle.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_NCDESTROY, WM_WTSSESSION_CHANGE};

const SUBCLASS: usize = 0x5255_4e4c;
/// `WTS_SESSION_LOCK`.
const SESSION_LOCK: usize = 0x7;
/// `NOTIFY_FOR_THIS_SESSION`.
const THIS_SESSION: u32 = 0;

type Register = unsafe extern "system" fn(HWND, u32) -> i32;
type Unregister = unsafe extern "system" fn(HWND) -> i32;

/// Asks for `window`'s session notifications. Call it once, after the first frame.
pub(crate) fn install(window: &winit::window::Window) {
    let Some(hwnd) = super::hwnd(window) else { return };
    let Some(register) = proc::<Register>(c"WTSRegisterSessionNotification") else { return };
    // SAFETY: the subclass keeps no pointer and goes at WM_NCDESTROY; registering for a live
    // window this thread owns.
    unsafe {
        if SetWindowSubclass(hwnd, Some(procedure), SUBCLASS, 0) == 0 { return; }
        register(hwnd, THIS_SESSION);
    }
}

/// An export of `wtsapi32.dll`, loaded from System32 the first time.
fn proc<F: Copy>(name: &std::ffi::CStr) -> Option<F> {
    // SAFETY: a NUL-terminated name from System32; the function's type is the caller's, which
    // names its documented signature.
    unsafe {
        let module = LoadLibraryExW(windows_sys::core::w!("wtsapi32.dll"), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if module.is_null() { return None; }
        let f = GetProcAddress(module, name.as_ptr().cast())?;
        Some(std::mem::transmute_copy(&f))
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: usize, l: isize, _: usize, _: usize) -> isize {
    if msg == WM_WTSSESSION_CHANGE && w == SESSION_LOCK {
        crate::clipboard::clear_secret_now();
    }
    if msg == WM_NCDESTROY {
        if let Some(unregister) = proc::<Unregister>(c"WTSUnRegisterSessionNotification") {
            // SAFETY: the window is still alive during WM_NCDESTROY.
            unsafe { unregister(hwnd); }
        }
        // SAFETY: removes this subclass from the window it was set on.
        unsafe { RemoveWindowSubclass(hwnd, Some(procedure), SUBCLASS) };
    }
    // SAFETY: passes every message on.
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}
