//! Dragging a window between displays with different scaling. Windows sends `WM_DPICHANGED`
//! with a rect it worked out for the new scale, and expects the window to take it. winit sizes
//! the window itself, keeps the pointer's place along the title bar, then nudges the window
//! until most of it is on the new display. Mid-drag that nudge moves the window out from
//! under the pointer; the move loop puts it back under the pointer, mostly on the old display,
//! which changes the scale back, and the window grows and shrinks at the boundary instead of
//! crossing it (hex#1355). While the user moves or sizes the window, it takes Windows' rect
//! after winit has handled the message, so winit still learns the new scale.

use std::cell::Cell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY,
};

const SUBCLASS: usize = 0x5255_4e44;

/// Makes `window` take the rect Windows suggests when its scale changes during a move or size.
pub(crate) fn follow_suggested_rect(window: &winit::window::Window) {
    let Some(hwnd) = super::hwnd(window) else { return };
    let moving = Box::into_raw(Box::new(Cell::new(false)));
    // SAFETY: `moving` lives until WM_NCDESTROY, where the subclass that reads it is removed.
    if unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, moving as usize) } == 0 {
        // SAFETY: never handed to Windows.
        drop(unsafe { Box::from_raw(moving) });
    }
}

unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, data: usize) -> LRESULT {
    let moving = data as *mut Cell<bool>;
    match message {
        // SAFETY: `data` is the live `Cell` until WM_NCDESTROY.
        WM_ENTERSIZEMOVE => unsafe { (*moving).set(true) },
        WM_EXITSIZEMOVE => unsafe { (*moving).set(false) },
        // SAFETY: as above; WM_DPICHANGED's lparam points to the suggested RECT.
        WM_DPICHANGED if unsafe { (*moving).get() } => unsafe {
            let suggested = *(lparam as *const RECT);
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            let RECT { left, top, right, bottom } = suggested;
            SetWindowPos(hwnd, std::ptr::null_mut(), left, top, right - left, bottom - top, SWP_NOZORDER | SWP_NOACTIVATE);
            return result;
        },
        WM_NCDESTROY => unsafe {
            RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS);
            drop(Box::from_raw(moving));
        },
        _ => {}
    }
    // SAFETY: forwards to the next window procedure.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

