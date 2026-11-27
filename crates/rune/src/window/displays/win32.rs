//! Hidden top-level window that receives Windows display-change broadcasts
//! (`WM_DISPLAYCHANGE`, `WM_DPICHANGED`, `WM_SETTINGCHANGE`) even when the app has no open
//! visible window.

#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
    RegisterClassW, SPI_SETWORKAREA, SetWindowLongPtrW, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_SETTINGCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
};

use crate::runtime::Waker;

struct WatcherState {
    queued: Arc<AtomicBool>,
    wake: Waker,
}

/// Top-level hidden tool window listening for display arrival, removal, geometry, and DPI
/// broadcasts on the UI thread.
pub(super) struct Win32DisplayWatch {
    hwnd: HWND,
    state: *mut WatcherState,
    queued: Arc<AtomicBool>,
}

impl Win32DisplayWatch {
    pub(super) fn new(wake: Waker) -> Option<Self> {
        let class: Vec<u16> = "RuneDisplayWatch\0".encode_utf16().collect();
        // SAFETY: `class` is NUL-terminated and outlives both calls; the window stays hidden
        // on the UI thread and is destroyed in `Drop` before `state` is freed.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return None;
        }
        let queued = Arc::new(AtomicBool::new(false));
        let state = Box::into_raw(Box::new(WatcherState {
            queued: queued.clone(),
            wake,
        }));
        // SAFETY: `hwnd` is a live window owned by this thread and `state` remains valid
        // until `Drop` clears `GWLP_USERDATA` and destroys `hwnd`.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        }
        Some(Self { hwnd, state, queued })
    }

    /// Clears the coalescing flag before reading the refreshed display list so any change
    /// arriving during refresh queues a follow-up wake.
    pub(super) fn clear_queued(&self) {
        self.queued.store(false, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for Win32DisplayWatch {
    fn drop(&mut self) {
        // SAFETY: clearing `GWLP_USERDATA` before `DestroyWindow` ensures `procedure` never
        // dereferences `self.state` during teardown; `self.state` came from `Box::into_raw`.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            DestroyWindow(self.hwnd);
            drop(Box::from_raw(self.state));
        }
    }
}

fn should_notify_setting_change(wparam: WPARAM, lparam: LPARAM) -> bool {
    wparam == SPI_SETWORKAREA as WPARAM || (wparam == 0 && lparam == 0)
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let notify = match message {
        WM_DISPLAYCHANGE | WM_DPICHANGED => true,
        WM_SETTINGCHANGE => should_notify_setting_change(wparam, lparam),
        _ => false,
    };
    if notify {
        // SAFETY: `GWLP_USERDATA` is either 0 or the live `*mut WatcherState` installed by
        // `Win32DisplayWatch::new` on this thread.
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WatcherState;
        if !ptr.is_null() {
            let state = unsafe { &*ptr };
            if !state.queued.swap(true, Ordering::AcqRel) {
                (state.wake)();
            }
        }
    }
    // SAFETY: parameters are forwarded unmodified to Windows' default window procedure.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
