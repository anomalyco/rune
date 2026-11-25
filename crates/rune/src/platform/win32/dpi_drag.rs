//! Dragging a window between displays with different scaling. Windows sends `WM_DPICHANGED`
//! with a rect it worked out for the new scale, and expects the window to take it. winit sizes
//! the window itself, keeps the pointer's place along the title bar, then nudges the window
//! until most of it is on the new display. Mid-drag that nudge moves the window out from
//! under the pointer; the move loop puts it back under the pointer, mostly on the old display,
//! which changes the scale back, and the window grows and shrinks at the boundary instead of
//! crossing it (hex#1355). While the user moves or sizes the window, it takes Windows' rect
//! after winit has handled the message, so winit still learns the new scale. A window with a
//! custom frame (`frame`) lands on Windows' rect in winit's own move, so it needs no second.
//!
//! The move or size loop is also when a resize draws its frame at once (`in_size_move`), and when
//! opt-in synchronized drag resize (`RUNE_RESIZE_SYNC=1`) suppresses `WM_ERASEBKGND`, returns
//! `WVR_VALIDRECTS` from `WM_NCCALCSIZE`, and presents its copy-model swap chain inside
//! `WM_PAINT` (`BeginPaint`/`EndPaint`).

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, UpdateWindow};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsWindow, NCCALCSIZE_PARAMS, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WM_DPICHANGED,
    WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_NCCALCSIZE, WM_NCDESTROY, WM_PAINT,
    WVR_VALIDRECTS,
};

const SUBCLASS: usize = 0x5255_4e44;

/// Pending modal size-move edge transitions captured by the Win32 subclass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResizeSyncSignal {
    pub entered: bool,
    pub exited: bool,
    pub destroyed: bool,
}

struct SyncEntry {
    hwnd: usize,
    wake: Rc<dyn Fn()>,
    active: bool,
    entered: bool,
    exited: bool,
    destroyed: bool,
}

struct ScopedPaint {
    hwnd: usize,
    /// Raw pointer to a stack-local `Option<&mut dyn FnMut()>` alive only for the synchronous
    /// `UpdateWindow` call in `run_in_paint`.
    run: *mut dyn FnMut(),
    ran: bool,
}

thread_local! {
    /// The windows on this thread inside a move or size loop.
    static MOVING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    /// Per-HWND state for windows opted into `RUNE_RESIZE_SYNC`. Empty when opt-in is off.
    static SYNC: RefCell<Vec<SyncEntry>> = const { RefCell::new(Vec::new()) };
    /// Active synchronous `WM_PAINT` callback for `run_in_paint`.
    static PAINT_ACTION: RefCell<Option<ScopedPaint>> = const { RefCell::new(None) };
}

/// Makes `window` take the rect Windows suggests when its scale changes during a move or size.
pub(crate) fn follow_suggested_rect(window: &winit::window::Window) {
    let Some(hwnd) = super::hwnd(window) else { return };
    // SAFETY: the subclass reads no data, and goes at WM_NCDESTROY.
    unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, 0); }
}

/// Registers `wake` to fire when `WM_EXITSIZEMOVE` arrives on `window` (only called when
/// `RUNE_RESIZE_SYNC` is enabled for an opaque app window).
pub(crate) fn register_resize_sync(window: &winit::window::Window, wake: impl Fn() + 'static) {
    let Some(hwnd) = super::hwnd(window) else { return };
    let key = hwnd as usize;
    let wake: Rc<dyn Fn()> = Rc::new(wake);
    SYNC.with(|list| {
        let mut list = list.borrow_mut();
        if let Some(entry) = list.iter_mut().find(|e| e.hwnd == key) {
            entry.wake = wake;
        } else {
            list.push(SyncEntry {
                hwnd: key,
                wake,
                active: false,
                entered: false,
                exited: false,
                destroyed: false,
            });
        }
    });
}

/// Toggles `WVR_VALIDRECTS` and `WM_ERASEBKGND` suppression for `hwnd` during an active drag.
pub(crate) fn set_resize_sync_active(hwnd: HWND, active: bool) {
    let key = hwnd as usize;
    SYNC.with(|list| {
        if let Some(entry) = list.borrow_mut().iter_mut().find(|e| e.hwnd == key) {
            entry.active = active;
        }
    });
}

/// Drains pending `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` / `WM_NCDESTROY` flags for `hwnd`.
pub(crate) fn take_resize_sync_signal(hwnd: HWND) -> ResizeSyncSignal {
    let key = hwnd as usize;
    SYNC.with(|list| {
        let mut list = list.borrow_mut();
        let Some(entry) = list.iter_mut().find(|e| e.hwnd == key) else {
            return ResizeSyncSignal::default();
        };
        ResizeSyncSignal {
            entered: std::mem::take(&mut entry.entered),
            exited: std::mem::take(&mut entry.exited),
            destroyed: std::mem::take(&mut entry.destroyed),
        }
    })
}

/// Whether `hwnd` is actively in synchronized drag resize and wants `WVR_VALIDRECTS` + erase
/// suppression. Always `false` when `RUNE_RESIZE_SYNC` is off.
pub(super) fn wants_valid_rects(hwnd: HWND) -> bool {
    let key = hwnd as usize;
    SYNC.with(|list| list.borrow().iter().any(|e| e.hwnd == key && e.active))
}

/// Writes `WVR_VALIDRECTS` destination (`rgrc[1]`) and source (`rgrc[2]`) rectangles from the
/// proposed new client rect (`params.rgrc[0]`) and previous client rect (`old_client`).
pub(super) fn apply_valid_rects(params: &mut NCCALCSIZE_PARAMS, old_client: RECT) {
    let new_c = params.rgrc[0];
    let (dest, src) = crate::window::win32_valid_rects(
        [new_c.left, new_c.top, new_c.right, new_c.bottom],
        [old_client.left, old_client.top, old_client.right, old_client.bottom],
    );
    params.rgrc[1] = RECT { left: dest[0], top: dest[1], right: dest[2], bottom: dest[3] };
    params.rgrc[2] = RECT { left: src[0], top: src[1], right: src[2], bottom: src[3] };
}

/// Runs `f` synchronously inside `WM_PAINT` (`BeginPaint`/`EndPaint`) for `hwnd` by invalidating
/// the client area and calling `UpdateWindow`. Returns `true` if `WM_PAINT` ran `f`.
pub(crate) fn run_in_paint(hwnd: HWND, mut f: impl FnMut()) -> bool {
    struct ResetGuard;
    impl Drop for ResetGuard {
        fn drop(&mut self) {
            PAINT_ACTION.with(|slot| { *slot.borrow_mut() = None; });
        }
    }
    let dyn_ref: &mut dyn FnMut() = &mut f;
    // SAFETY: erased lifetime is strictly bounded by `_guard` before `f` returns.
    let raw: *mut dyn FnMut() = unsafe { std::mem::transmute::<*mut dyn FnMut(), *mut dyn FnMut()>(dyn_ref) };
    PAINT_ACTION.with(|slot| {
        *slot.borrow_mut() = Some(ScopedPaint { hwnd: hwnd as usize, run: raw, ran: false });
    });
    let _guard = ResetGuard;
    // SAFETY: `hwnd` is a live window owned by this thread; `InvalidateRect` + `UpdateWindow`
    // dispatches `WM_PAINT` synchronously on this stack before `_guard` drops.
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
        UpdateWindow(hwnd);
    }
    PAINT_ACTION.with(|slot| slot.borrow().as_ref().is_some_and(|p| p.ran))
}

/// Whether the user is moving or sizing `window` by its frame (`WM_ENTERSIZEMOVE` until
/// `WM_EXITSIZEMOVE`), in the system's modal loop.
pub(crate) fn in_size_move(window: &winit::window::Window) -> bool {
    super::hwnd(window).is_some_and(moving)
}

fn moving(hwnd: HWND) -> bool {
    MOVING.with(|m| m.borrow().contains(&(hwnd as usize)))
}

unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
    let key = hwnd as usize;
    match message {
        WM_ENTERSIZEMOVE => {
            MOVING.with(|m| {
                let mut m = m.borrow_mut();
                if !m.contains(&key) { m.push(key); }
            });
            SYNC.with(|list| {
                if let Some(entry) = list.borrow_mut().iter_mut().find(|e| e.hwnd == key) {
                    entry.entered = true;
                    entry.exited = false;
                }
            });
        }
        WM_EXITSIZEMOVE => {
            MOVING.with(|m| m.borrow_mut().retain(|&h| h != key));
            let wake = SYNC.with(|list| {
                list.borrow_mut().iter_mut().find(|e| e.hwnd == key).map(|entry| {
                    entry.active = false;
                    entry.exited = true;
                    entry.wake.clone()
                })
            });
            if let Some(wake) = wake {
                wake();
            }
        }
        // Standard-frame windows (`!frame::installed`): return `WVR_VALIDRECTS` while drag-sync
        // is active. Custom-frame windows apply the same helper inside `frame::subclass`.
        WM_NCCALCSIZE if wparam != 0 && wants_valid_rects(hwnd) && !super::frame::installed(hwnd) => {
            // SAFETY: `lparam` points to `NCCALCSIZE_PARAMS` when `wparam != 0`.
            unsafe {
                let params = &mut *(lparam as *mut NCCALCSIZE_PARAMS);
                let old_client = params.rgrc[2];
                DefSubclassProc(hwnd, message, wparam, lparam);
                apply_valid_rects(params, old_client);
                return WVR_VALIDRECTS as LRESULT;
            }
        }
        // Suppress GDI background erase during active synchronized drag resize.
        WM_ERASEBKGND if wants_valid_rects(hwnd) => return 1,
        WM_PAINT => {
            let action = PAINT_ACTION.with(|slot| {
                let mut slot = slot.borrow_mut();
                if let Some(p) = slot.as_mut().filter(|p| p.hwnd == key && !p.ran) {
                    p.ran = true;
                    Some(p.run)
                } else {
                    None
                }
            });
            if let Some(run) = action {
                // SAFETY: `hwnd` is live on this thread; `run` is the live closure on the caller's
                // `run_in_paint` stack frame, invoked at most once before `EndPaint`.
                unsafe {
                    let mut ps: PAINTSTRUCT = std::mem::zeroed();
                    BeginPaint(hwnd, &mut ps);
                    (*run)();
                    EndPaint(hwnd, &ps);
                }
                return 0;
            }
        }
        // SAFETY: WM_DPICHANGED's lparam points to the suggested RECT.
        WM_DPICHANGED if moving(hwnd) && !super::frame::installed(hwnd) => unsafe {
            let suggested = *(lparam as *const RECT);
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            if IsWindow(hwnd) != 0 {
                let RECT { left, top, right, bottom } = suggested;
                SetWindowPos(hwnd, std::ptr::null_mut(), left, top, right - left, bottom - top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            return result;
        },
        WM_NCDESTROY => {
            MOVING.with(|m| m.borrow_mut().retain(|&h| h != key));
            SYNC.with(|list| list.borrow_mut().retain(|e| e.hwnd != key));
            PAINT_ACTION.with(|slot| {
                if slot.borrow().as_ref().is_some_and(|p| p.hwnd == key) {
                    *slot.borrow_mut() = None;
                }
            });
            // SAFETY: removes this subclass from the window it's on.
            unsafe { RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS); }
        }
        _ => {}
    }
    // SAFETY: forwards to the next window procedure.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
