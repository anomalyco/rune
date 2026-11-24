//! A message-only window receives WM_HOTKEY inside winit's event-loop thread.

#![allow(unsafe_code)]

use super::{Backend, Shortcut, ShortcutError, ShortcutId, Sink, native_status, parse::*};
use std::{cell::RefCell, collections::HashMap, ptr};
use winit::platform::scancode::PhysicalKeyExtScancode;
use windows_sys::Win32::{Foundation::*, System::LibraryLoader::GetModuleHandleW, UI::{WindowsAndMessaging::*, Input::KeyboardAndMouse::*}};

struct Context { sink: Sink, ids: RefCell<HashMap<i32, ShortcutId>> }
pub(super) struct Native { window: HWND, context: Box<Context> }

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: Windows owns the message parameters. The context outlives the window and is
    // only accessed on its creating thread; it is cleared before destruction completes.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        if message == WM_HOTKEY {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context;
            if let Some(context) = p.as_ref() {
                let id = context.ids.borrow().get(&(w as i32)).copied();
                if let Some(id) = id {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (context.sink)(id, None))).ok();
                }
            }
            return 0;
        }
        if message == WM_NCDESTROY { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); }
        DefWindowProcW(hwnd, message, w, l)
    }
}

fn last_error() -> ShortcutError {
    // SAFETY: Reads this thread's last Win32 status immediately after a failed call.
    native_status(unsafe { GetLastError() }.into(), ERROR_HOTKEY_ALREADY_REGISTERED.into()).unwrap_err()
}

impl Native {
    pub(super) fn new(sink: Sink) -> Result<Self, ShortcutError> {
        let class: Vec<u16> = "RuneGlobalShortcuts\0".encode_utf16().collect();
        let mut context = Box::new(Context { sink, ids: RefCell::default() });
        // SAFETY: Class name is NUL-terminated; the box is stable until after DestroyWindow.
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let wc = WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
            if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS { return Err(last_error()); }
            let window = CreateWindowExW(0, class.as_ptr(), ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE,
                ptr::null_mut(), instance, (&mut *context as *mut Context).cast());
            if window.is_null() { return Err(last_error()); }
            Ok(Self { window, context })
        }
    }
}

fn virtual_key(shortcut: &Shortcut) -> Result<u32, ShortcutError> {
    if shortcut.physical_key() == super::PhysicalKeyCode::PrintScreen {
        return Ok(u32::from(VK_SNAPSHOT));
    }
    let scan = shortcut.physical_key().to_scancode().ok_or_else(|| ShortcutError::InvalidAccelerator(shortcut.to_string()))?;
    // RegisterHotKey takes a virtual key. Resolve the physical scan code in the
    // current layout rather than assuming US letters. Re-register on layout changes.
    // SAFETY: `MapVirtualKeyW` takes integer values only and reads the active keyboard layout.
    let key = unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX) };
    if key == 0 { return Err(ShortcutError::InvalidAccelerator(shortcut.to_string())); }
    Ok(key)
}

impl Backend for Native {
    fn register(&mut self, id: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        let local = (1..=0xbfff).find(|id| !self.context.ids.borrow().contains_key(id))
            .ok_or_else(|| ShortcutError::Backend("Win32 hotkey id space exhausted".into()))?;
        let mut mods = MOD_NOREPEAT;
        for (bit, native) in [(CTRL, MOD_CONTROL), (ALT, MOD_ALT), (SHIFT, MOD_SHIFT), (SUPER, MOD_WIN)] {
            if shortcut.mods & bit != 0 { mods |= native; }
        }
        let key = virtual_key(shortcut)?;
        // SAFETY: RegisterHotKey and its window run on the event-loop thread.
        if unsafe { RegisterHotKey(self.window, local, mods, key) } == 0 { return Err(last_error()); }
        self.context.ids.borrow_mut().insert(local, id);
        Ok(())
    }
    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> {
        let local = self.context.ids.borrow().iter().find(|(_, v)| **v == id).map(|(k, _)| *k).ok_or(ShortcutError::UnknownId)?;
        // SAFETY: The hotkey belongs to this window and thread.
        if unsafe { UnregisterHotKey(self.window, local) } == 0 { return Err(last_error()); }
        self.context.ids.borrow_mut().remove(&local);
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: Context remains alive throughout destruction. The host drops on this thread.
        unsafe {
            for (id, _) in self.context.ids.get_mut().drain() { UnregisterHotKey(self.window, id); }
            DestroyWindow(self.window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printscreen_resolves_to_vk_snapshot() {
        for accel in ["printscreen", "print", "prtsc", "ctrl+printscreen", "alt+printscreen", "shift+printscreen"] {
            let shortcut: Shortcut = accel.parse().unwrap();
            assert_eq!(virtual_key(&shortcut), Ok(u32::from(VK_SNAPSHOT)), "{accel}");
        }
        let space: Shortcut = "ctrl+space".parse().unwrap();
        assert_eq!(virtual_key(&space), Ok(u32::from(VK_SPACE)));
    }
}
