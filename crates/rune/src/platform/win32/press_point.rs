//! Where the mouse button message being handled says the button went down or up. winit drops
//! the point a button message carries and reports only that the button changed, so the press
//! lands wherever the last move left the pointer. Input that isn't the real mouse (window
//! messages posted by an automation driver) has no real cursor over the window: Windows' leave
//! tracking ends the pointer between the move and the press, and the system cursor is
//! elsewhere, or unreadable on a desktop no one sees. The message's own point is the truth.

use std::cell::Cell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_MOUSEWHEEL, WM_NCDESTROY, WM_XBUTTONDBLCLK};

const SUBCLASS: usize = 0x5255_4e50;

thread_local! {
    /// The window and client point of the button message this thread is handling.
    static CURRENT: Cell<Option<(usize, i32, i32)>> = const { Cell::new(None) };
}

/// Records the point of each button message `window` receives while its handlers run.
pub(crate) fn install(window: &winit::window::Window) {
    let Some(hwnd) = super::hwnd(window) else { return };
    // SAFETY: the subclass reads no data, and goes at WM_NCDESTROY.
    unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, 0); }
}

/// The client point, in physical pixels, of the button message `window` is handling now.
pub(crate) fn current(window: &winit::window::Window) -> Option<(i32, i32)> {
    let hwnd = super::hwnd(window)? as usize;
    CURRENT.get().filter(|c| c.0 == hwnd).map(|c| (c.1, c.2))
}

fn button_message(message: u32) -> bool {
    (WM_LBUTTONDOWN..=WM_XBUTTONDBLCLK).contains(&message) && message != WM_MOUSEWHEEL
}

unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
    if button_message(message) {
        let (x, y) = (i32::from(lparam as u16 as i16), i32::from((lparam >> 16) as u16 as i16));
        let outer = CURRENT.replace(Some((hwnd as usize, x, y)));
        // SAFETY: forwards to the next window procedure.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        CURRENT.set(outer);
        return result;
    }
    if message == WM_NCDESTROY {
        // SAFETY: removes this subclass from the window it's on.
        unsafe { RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS); }
    }
    // SAFETY: forwards to the next window procedure.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}