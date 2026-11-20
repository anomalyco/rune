//! UI Automation for a window: AccessKit's adapter answers the window's `WM_GETOBJECT`.
//! AccessKit's own subclassing adapter must exist before the window first shows; Rune's windows
//! open shown, so this subclasses with `SetWindowSubclass` instead, which works at any time.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;

use accesskit_windows::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, WM_GETOBJECT, WM_KILLFOCUS, WM_NCDESTROY, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_THEMECHANGED};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{Actions, Activation};

const SUBCLASS_ID: usize = 0x5255_4e41; // "RUNA"

struct State {
    adapter: accesskit_windows::Adapter,
    activation: Activation,
}

pub(super) struct Adapter {
    hwnd: isize,
    state: Rc<RefCell<State>>,
}

unsafe extern "system" fn subclass(hwnd: windows_sys::Win32::Foundation::HWND, message: u32, wparam: usize, lparam: isize, _: usize, data: usize) -> isize {
    let ptr = data as *const RefCell<State>;
    if message == WM_NCDESTROY {
        // SAFETY: removes this subclass; if it was still installed, reclaims the ref `new` gave it.
        unsafe {
            if RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID) != 0 {
                drop(Rc::from_raw(ptr));
            }
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
    }
    // Hold a strong ref across UIA callbacks and `DefSubclassProc` so dropping `Adapter`
    // re-entrantly cannot free `State` while this frame is still on the stack.
    // SAFETY: `ptr` is the live `Rc<RefCell<State>>` owned by the subclass until removal.
    let state: Rc<RefCell<State>> = unsafe {
        Rc::increment_strong_count(ptr);
        Rc::from_raw(ptr)
    };
    match message {
        WM_GETOBJECT => {
            // The answer calls into UIA, which may send a nested WM_GETOBJECT: borrow only to
            // build it.
            let result = state.try_borrow_mut().ok().and_then(|mut s| {
                let s = &mut *s;
                s.adapter.handle_wm_getobject(WPARAM(wparam), LPARAM(lparam), &mut s.activation).map(|r| -> LRESULT { r.into() })
            });
            if let Some(result) = result { return result.0; }
        }
        // Reduced motion, a contrast theme or the system colours changed: the runtime reads them
        // again, without waiting for the window's next activation.
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            if let Ok(s) = state.try_borrow() { s.activation.0.push(|i| i.settings = true); }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let events = state.try_borrow_mut().ok().and_then(|mut s| s.adapter.update_window_focus_state(message == WM_SETFOCUS));
            if let Some(events) = events { events.raise(); }
        }
        _ => {}
    }
    // SAFETY: forwarding the message this subclass received.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

impl Adapter {
    pub(super) fn new(window: &winit::window::Window, activation: Activation, actions: Actions) -> Option<Self> {
        let RawWindowHandle::Win32(h) = window.window_handle().ok()?.as_raw() else { return None };
        let hwnd = h.hwnd.get();
        // SAFETY: reads which window has focus; `hwnd` belongs to this thread.
        let focused = unsafe { GetForegroundWindow() } as isize == hwnd;
        let adapter = accesskit_windows::Adapter::new(HWND(hwnd as *mut _), focused, actions);
        let state = Rc::new(RefCell::new(State { adapter, activation }));
        let data = Rc::into_raw(state.clone());
        // SAFETY: `data` is reclaimed exactly once, either by `Drop` or by `WM_NCDESTROY`,
        // whichever succeeds in removing the subclass.
        let ok = unsafe { SetWindowSubclass(hwnd as _, Some(subclass), SUBCLASS_ID, data as usize) };
        if ok == 0 {
            // SAFETY: `SetWindowSubclass` failed so the subclass never took ownership of `data`;
            // reclaim the `Rc` ref created by `Rc::into_raw` above.
            unsafe { drop(Rc::from_raw(data)); }
            return None;
        }
        Some(Adapter { hwnd, state })
    }

    /// Announces what UIA events from AccessKit don't: children added, removed or reordered
    /// (`EVENT_OBJECT_REORDER`) and boxes moved by a scroll (`EVENT_OBJECT_LOCATIONCHANGE`), on
    /// the window's client object, as Win32 controls do.
    pub(super) fn notify(&self, changes: super::tree::Changes) {
        use windows_sys::Win32::UI::Accessibility::NotifyWinEvent;
        use windows_sys::Win32::UI::WindowsAndMessaging::{CHILDID_SELF, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_REORDER, OBJID_CLIENT};
        // SAFETY: plain notifications about a live window this thread owns.
        unsafe {
            if changes.structure { NotifyWinEvent(EVENT_OBJECT_REORDER, self.hwnd as _, OBJID_CLIENT, CHILDID_SELF as i32); }
            if changes.moved { NotifyWinEvent(EVENT_OBJECT_LOCATIONCHANGE, self.hwnd as _, OBJID_CLIENT, CHILDID_SELF as i32); }
        }
    }

    pub(super) fn update(&mut self, update: accesskit::TreeUpdate) {
        // Raised after the borrow ends: raising may bring a nested WM_GETOBJECT.
        let events = self.state.try_borrow_mut().ok().and_then(|mut s| s.adapter.update_if_active(|| update));
        if let Some(events) = events { events.raise(); }
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        // SAFETY: removes the callback installed in `new`; if it was still installed (not already
        // removed by `WM_NCDESTROY`), reclaims the `Rc` ref owned by the subclass.
        unsafe {
            if RemoveWindowSubclass(self.hwnd as _, Some(subclass), SUBCLASS_ID) != 0 {
                drop(Rc::from_raw(Rc::as_ptr(&self.state)));
            }
        }
    }
}
