//! Win32 pop-up menus. `TrackPopupMenuEx` runs its own modal loop until the menu closes. Run
//! inside a winit handler, that loop would hold every Rune frame: winit buffers the events it
//! dispatches while its handler is busy. So the menu opens from a message posted to the window,
//! which winit's loop dispatches after the handler returns. While the menu tracks, its loop
//! hands the window's paints and wakes to winit as usual, so Rune keeps drawing.
//!
//! The menu drops down below an element anchor and opens with its top-left corner at a point,
//! as Windows menus do. A plain Win32 menu can't put an item under the pointer or take a
//! minimum width, so `Show::positioned` and `Show::min_width` don't apply. Keyboard navigation
//! (arrows, Enter, Esc, first letters, submenus), screen-edge flipping and dismissal by a click
//! outside are the system's.
use std::{rc::Rc, sync::{Arc, OnceLock, atomic::{AtomicUsize, Ordering}}};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, EndMenu, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, HMENU,
    IsWindowVisible, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, RegisterWindowMessageW,
    SetForegroundWindow, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN, TPM_VERTICAL, TPMPARAMS,
    TrackPopupMenuEx, WM_NCDESTROY, WS_EX_NOACTIVATE,
};
use winit::window::Window;

use super::{Entry, PopupItem, PopupMenu, Session, Show};
use crate::runtime::Waker;

/// The next menu's subclass id. `GetWindowSubclass` would find an earlier one by a fixed id,
/// but the comctl32 an app without a v6 manifest loads exports it by ordinal only.
static NEXT: AtomicUsize = AtomicUsize::new(1);

/// A menu handed over and waiting for its posted message.
struct Pending { show: Show, session: Rc<Session>, wake: Option<Waker> }

/// Posts `show` to `window`, to open once the current winit handler returns. The answer lands
/// in `session` and `wake` brings the host to read it. False when the window has no Win32
/// handle. A session cancelled before then never opens; one whose window was hidden by then
/// answers `None`.
pub(super) fn show(window: &Arc<Window>, show: Show, session: Rc<Session>, wake: Option<Waker>) -> bool {
    let Some(hwnd) = crate::platform::win32::hwnd(window) else { return false };
    // Each menu has its own subclass, named in its message: one handed over earlier whose
    // message hasn't come yet was cancelled, and goes when that message comes.
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: Win32 calls on a live window owned by this thread; a subclass's data is the
    // `Pending` boxed here, taken back exactly once, by whoever removes the subclass.
    unsafe {
        let pending = Box::into_raw(Box::new(Pending { show, session, wake }));
        if SetWindowSubclass(hwnd, Some(procedure), id, pending as usize) == 0 {
            drop(Box::from_raw(pending));
            return false;
        }
        if PostMessageW(hwnd, open_message(), id, 0) == 0 {
            RemoveWindowSubclass(hwnd, Some(procedure), id);
            drop(Box::from_raw(pending));
            return false;
        }
    }
    true
}

/// Stops the menu tracking on this thread. Its `TrackPopupMenuEx` returns once the handler that
/// asked returns.
pub(super) fn end() {
    // SAFETY: ends this thread's active menu, if any.
    unsafe { EndMenu() };
}

fn open_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: the name is NUL-terminated.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(wide("Rune::PopupMenu").as_ptr()) })
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM, id: usize, data: usize) -> LRESULT {
    if (message == open_message() && w == id) || message == WM_NCDESTROY {
        // SAFETY: `data` is the `Pending` `show` boxed for this subclass, taken back once, here,
        // as the subclass goes. A window destroyed first answers `None`.
        let pending = unsafe {
            RemoveWindowSubclass(hwnd, Some(procedure), id);
            Box::from_raw(data as *mut Pending)
        };
        let opens = message != WM_NCDESTROY;
        let picked = if opens { std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| track(hwnd, &pending))) } else { Ok(None) };
        let picked = picked.unwrap_or_else(|_| {
            eprintln!("rune: pop-up menu panicked");
            None
        });
        if pending.session.resolve(picked) && let Some(wake) = &pending.wake { wake(); }
        if opens { return 0; }
    }
    // SAFETY: passes every other message on to winit's window procedure.
    unsafe { DefSubclassProc(hwnd, message, w, l) }
}

/// Shows the menu over `hwnd` until it closes. The picked item's id, or `None` when it was
/// dismissed, cancelled or couldn't open.
fn track(hwnd: HWND, pending: &Pending) -> Option<String> {
    if !pending.session.begin() { return None; }
    // SAFETY: Win32 calls on this thread's live window. The menu is built, tracked and
    // destroyed here; destroying it destroys its submenus.
    unsafe {
        // A close or hide in the same turn as the request, after the host handed it over.
        if IsWindowVisible(hwnd) == 0 { return None; }
        let mut ids = Vec::new();
        let menu = build(&pending.show.menu, &mut ids)?;
        let (x, y, params) = placement(hwnd, &pending.show);
        // A menu whose owner isn't the foreground window doesn't close on a click elsewhere. A
        // window that never activates (an overlay) keeps its focus where it is.
        let activates = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE == 0;
        if activates && GetForegroundWindow() != hwnd { SetForegroundWindow(hwnd); }
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN
            | if params.is_some() { TPM_VERTICAL } else { 0 };
        let params_ptr = params.as_ref().map_or(std::ptr::null(), |p| p as *const TPMPARAMS);
        let command = TrackPopupMenuEx(menu, flags, x, y, hwnd, params_ptr);
        DestroyMenu(menu);
        usize::try_from(command).ok()?.checked_sub(1).and_then(|i| ids.get(i)).cloned()
    }
}

/// Where the menu opens, in screen pixels: below an element anchor, keeping clear of it when it
/// has to flip up, or with its top-left corner at a point.
fn placement(hwnd: HWND, show: &Show) -> (i32, i32, Option<TPMPARAMS>) {
    // SAFETY: plain queries on this thread's live window.
    let (scale, origin) = unsafe {
        let dpi = GetDpiForWindow(hwnd);
        let mut origin = POINT { x: 0, y: 0 };
        ClientToScreen(hwnd, &mut origin);
        (if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }, origin)
    };
    let px = |v: f32, o: i32| o + (v * scale).round() as i32;
    let Some([x, y, w, h]) = show.element else { return (px(show.at.0, origin.x), px(show.at.1, origin.y), None) };
    let element = RECT { left: px(x, origin.x), top: px(y, origin.y), right: px(x + w, origin.x), bottom: px(y + h, origin.y) };
    let params = TPMPARAMS { cbSize: size_of::<TPMPARAMS>() as u32, rcExclude: element };
    (element.left, element.bottom, Some(params))
}

/// The menu's items, numbered from 1 in order (`ids[n - 1]` is item `n`'s id). `None` when
/// Windows has no menu to give.
unsafe fn build(menu: &PopupMenu, ids: &mut Vec<String>) -> Option<HMENU> {
    // SAFETY: the caller's thread owns the new menu; every string is NUL-terminated and copied.
    unsafe {
        let hmenu = CreatePopupMenu();
        if hmenu.is_null() { return None; }
        for entry in &menu.entries {
            match entry {
                Entry::Item(item) => {
                    ids.push(item.id.clone());
                    let flags = MF_STRING | if item.enabled { 0 } else { MF_GRAYED } | if item.checked { MF_CHECKED } else { 0 };
                    AppendMenuW(hmenu, flags, ids.len(), wide(&label(item)).as_ptr());
                }
                Entry::Separator => { AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null()); }
                Entry::Submenu(title, menu) => {
                    let Some(submenu) = build(menu, ids) else { continue };
                    AppendMenuW(hmenu, MF_STRING | MF_POPUP, submenu as usize, wide(&escape(title)).as_ptr());
                }
            }
        }
        Some(hmenu)
    }
}

/// An item's title, with its shortcut hint right-aligned after a tab, as Windows draws it.
fn label(item: &PopupItem) -> String {
    let title = escape(&item.title);
    match &item.key_equivalent {
        Some(shortcut) => format!("{title}\t{}", shortcut.menu_keycaps().join("+")),
        None => title,
    }
}

/// A title as Windows shows it literally: `&` would underline the next letter, and a tab would
/// start the shortcut column.
fn escape(title: &str) -> String { title.replace('&', "&&").replace('\t', " ") }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
