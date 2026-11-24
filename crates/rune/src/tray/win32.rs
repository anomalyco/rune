//! `Shell_NotifyIcon` on a message-only window that owns its thread and message loop. A left
//! click activates; a right click opens a `TrackPopupMenu` menu. The icon comes back when
//! Explorer restarts (`TaskbarCreated`).

#![allow(unsafe_code)]

use super::{Handler, TrayError, TrayEvent, TrayIcon, TrayItem, TraySpec};
use std::{cell::Cell, ptr, sync::{Arc, Mutex, mpsc}, thread::JoinHandle};
use windows_sys::Win32::{Foundation::*, System::LibraryLoader::GetModuleHandleW, UI::{Shell::*, WindowsAndMessaging::*}};

const CALLBACK: u32 = WM_APP + 1;
const UPDATE: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;

struct Model { tooltip: String, menu: Vec<TrayItem>, pending_icon: Option<TrayIcon> }
struct Context { model: Arc<Mutex<Model>>, on_event: Handler, icon: Cell<HICON>, taskbar_created: u32 }
pub(super) struct Native { window: isize, model: Arc<Mutex<Model>>, worker: Option<JoinHandle<()>> }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
fn last_error(what: &str) -> TrayError {
    // SAFETY: Reads this thread's last Win32 status immediately after a failed call.
    TrayError::Backend(format!("{what} failed: Win32 error {}", unsafe { GetLastError() }))
}

/// A 32-bit BGRA icon; the AND mask is the inverted alpha, which Windows ignores beside alpha.
pub(super) fn create_icon(icon: &TrayIcon) -> HICON {
    let (Ok(w), Ok(h)) = (i32::try_from(icon.width), i32::try_from(icon.height)) else {
        return ptr::null_mut();
    };
    if w <= 0 || h <= 0 {
        return ptr::null_mut();
    }
    let mut bgra = icon.rgba.clone();
    let mut mask = Vec::with_capacity(bgra.len() / 4);
    for p in bgra.chunks_exact_mut(4) { p.swap(0, 2); mask.push(p[3].wrapping_sub(u8::MAX)); }
    // SAFETY: Both buffers hold width × height pixels and outlive the call, which copies them.
    unsafe { CreateIcon(ptr::null_mut(), w, h, 1, 32, mask.as_ptr(), bgra.as_ptr()) }
}

/// Replaces `slot` with `next` when non-null and returns the previous handle so the caller can
/// destroy it after `Shell_NotifyIconW(NIM_MODIFY, ...)` completes.
pub(super) fn swap_icon(slot: &Cell<HICON>, next: HICON) -> Option<HICON> {
    if next.is_null() {
        return None;
    }
    let old = slot.replace(next);
    (!old.is_null()).then_some(old)
}

pub(super) fn icon_identifier(hwnd: HWND) -> NOTIFYICONIDENTIFIER {
    // SAFETY: NOTIFYICONIDENTIFIER is plain data; zero is its documented empty value.
    let mut id: NOTIFYICONIDENTIFIER = unsafe { std::mem::zeroed() };
    id.cbSize = size_of::<NOTIFYICONIDENTIFIER>() as u32;
    id.hWnd = hwnd;
    id.uID = ICON_ID;
    id
}

pub(super) fn rect_to_xywh(r: RECT) -> Option<[i32; 4]> {
    let w = r.right.checked_sub(r.left)?;
    let h = r.bottom.checked_sub(r.top)?;
    (w > 0 && h > 0).then_some([r.left, r.top, w, h])
}

pub(super) fn menu_item_flags(enabled: bool, checked: Option<bool>) -> u32 {
    let mut flags = MF_STRING;
    if !enabled {
        flags |= MF_GRAYED;
    }
    if let Some(is_checked) = checked {
        flags |= if is_checked { MF_CHECKED } else { MF_UNCHECKED };
    }
    flags
}

pub(super) fn data(hwnd: HWND, flags: u32) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data; zero is its documented empty value.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = ICON_ID;
    data.uFlags = flags;
    data
}
pub(super) fn set_tip(data: &mut NOTIFYICONDATAW, tooltip: &str) {
    let tip = super::truncate_utf16(tooltip, data.szTip.len() - 1);
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data.szTip[tip.len()] = 0;
}
fn notify(hwnd: HWND, context: &Context, message: NOTIFY_ICON_MESSAGE) -> bool {
    let mut data = data(hwnd, NIF_MESSAGE | NIF_ICON | NIF_TIP);
    data.uCallbackMessage = CALLBACK;
    data.hIcon = context.icon.get();
    set_tip(&mut data, &context.model.lock().unwrap_or_else(|e| e.into_inner()).tooltip);
    // SAFETY: `data` is fully initialized for this window's icon.
    unsafe { Shell_NotifyIconW(message, &data) != 0 }
}

/// Shows the menu at the cursor and returns the picked item's id.
fn pick(hwnd: HWND, context: &Context) -> Option<String> {
    let menu = context.model.lock().unwrap_or_else(|e| e.into_inner()).menu.clone();
    // SAFETY: The menu is created, shown and destroyed on this window's thread.
    unsafe {
        let popup = CreatePopupMenu();
        if popup.is_null() { return None; }
        for (index, item) in menu.iter().enumerate() {
            match item {
                TrayItem::Action { label, enabled, .. } => {
                    let Ok(command) = u16::try_from(index + 1) else { break };
                    let flags = menu_item_flags(*enabled, None);
                    AppendMenuW(popup, flags, usize::from(command), wide(&super::win32_menu_label(label)).as_ptr());
                }
                TrayItem::Check { label, enabled, checked, .. } => {
                    let Ok(command) = u16::try_from(index + 1) else { break };
                    let flags = menu_item_flags(*enabled, Some(*checked));
                    AppendMenuW(popup, flags, usize::from(command), wide(&super::win32_menu_label(label)).as_ptr());
                }
                TrayItem::Separator => { AppendMenuW(popup, MF_SEPARATOR, 0, ptr::null()); }
            }
        }
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        // Without the foreground, the menu doesn't close when the user clicks elsewhere.
        SetForegroundWindow(hwnd);
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | crate::popup_menu::layout();
        let picked = TrackPopupMenu(popup, flags, at.x, at.y, 0, hwnd, ptr::null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(popup);
        match menu.get((picked as usize).checked_sub(1)?) {
            Some(TrayItem::Action { id, enabled: true, .. } | TrayItem::Check { id, enabled: true, .. }) => Some(id.clone()),
            _ => None,
        }
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: Windows owns the message parameters. The context outlives the window and is only
    // used on its thread; it is cleared before destruction completes.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let Some(context) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context).as_ref() else {
            return DefWindowProcW(hwnd, message, w, l);
        };
        match message {
            CALLBACK => {
                let event = match l as u32 {
                    WM_LBUTTONUP => Some(TrayEvent::Activate { token: None }),
                    WM_RBUTTONUP => pick(hwnd, context).map(TrayEvent::Menu),
                    _ => None,
                };
                if let Some(event) = event {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (context.on_event)(event))).ok();
                }
                0
            }
            UPDATE => {
                let pending = context.model.lock().unwrap_or_else(|e| e.into_inner()).pending_icon.take();
                let replaced = pending.and_then(|icon| swap_icon(&context.icon, create_icon(&icon)));
                notify(hwnd, context, NIM_MODIFY);
                if let Some(old) = replaced {
                    DestroyIcon(old);
                }
                0
            }
            WM_DESTROY => {
                Shell_NotifyIconW(NIM_DELETE, &data(hwnd, 0));
                PostQuitMessage(0);
                0
            }
            WM_NCDESTROY => { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); DefWindowProcW(hwnd, message, w, l) }
            m if m == context.taskbar_created => { notify(hwnd, context, NIM_ADD); 0 }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}

/// Creates the window and icon on this thread; the context lives as long as the window.
fn create(context: &mut Context) -> Result<HWND, TrayError> {
    let class = wide("RuneTray");
    // SAFETY: Class name is NUL-terminated; the context is stable until the loop ends.
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS { return Err(last_error("RegisterClassW")); }
        let hwnd = CreateWindowExW(0, class.as_ptr(), ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, ptr::null_mut(), instance,
            (context as *mut Context).cast());
        if hwnd.is_null() { return Err(last_error("CreateWindowExW")); }
        if !notify(hwnd, context, NIM_ADD) { DestroyWindow(hwnd); return Err(last_error("Shell_NotifyIconW")); }
        Ok(hwnd)
    }
}

impl Native {
    pub(super) fn new(spec: TraySpec, on_event: Handler) -> Result<Self, TrayError> {
        let model = Arc::new(Mutex::new(Model {
            tooltip: spec.tooltip.clone(),
            menu: spec.menu.clone(),
            pending_icon: None,
        }));
        let shared = model.clone();
        let (ready, result) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("rune-tray".into()).spawn(move || {
            let icon = create_icon(&spec.icon);
            // SAFETY: The name is NUL-terminated.
            let taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
            let mut context = Box::new(Context { model: shared, on_event, icon: Cell::new(icon), taskbar_created });
            match create(&mut context) {
                Ok(hwnd) => { ready.send(Ok(hwnd as isize)).ok(); }
                Err(e) => {
                    let current = context.icon.replace(ptr::null_mut());
                    // SAFETY: The icon was created above and nothing else holds it.
                    if !current.is_null() { unsafe { DestroyIcon(current) }; }
                    ready.send(Err(e)).ok();
                    return;
                }
            }
            // SAFETY: A plain message loop for this thread's window; it ends with WM_QUIT.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 { TranslateMessage(&msg); DispatchMessageW(&msg); }
                let current = context.icon.replace(ptr::null_mut());
                if !current.is_null() { DestroyIcon(current); }
            }
            drop(context);
        }).map_err(|e| TrayError::Backend(e.to_string()))?;
        match result.recv() {
            Ok(Ok(window)) => {
                LIVE.lock().unwrap_or_else(|e| e.into_inner()).push(window);
                Ok(Self { window, model, worker: Some(worker) })
            }
            Ok(Err(e)) => { worker.join().ok(); Err(e) }
            Err(_) => { worker.join().ok(); Err(TrayError::Backend("tray thread exited".into())) }
        }
    }
    fn update(&self) {
        // SAFETY: Posting to a window owned by the tray thread; a gone window drops the message.
        unsafe { PostMessageW(self.window as HWND, UPDATE, 0, 0) };
    }
    pub(super) fn set_menu(&self, menu: Vec<TrayItem>) {
        self.model.lock().unwrap_or_else(|e| e.into_inner()).menu = menu;
    }
    pub(super) fn set_tooltip(&self, tooltip: String) {
        self.model.lock().unwrap_or_else(|e| e.into_inner()).tooltip = tooltip;
        self.update();
    }
    pub(super) fn set_icon(&self, icon: TrayIcon) -> Result<(), TrayError> {
        if icon.width == 0 || icon.height == 0 {
            return Err(TrayError::Backend("tray icon dimensions must be non-zero".into()));
        }
        if i32::try_from(icon.width).is_err() || i32::try_from(icon.height).is_err() {
            return Err(TrayError::Backend("tray icon dimensions overflow".into()));
        }
        self.model.lock().unwrap_or_else(|e| e.into_inner()).pending_icon = Some(icon);
        self.update();
        Ok(())
    }
    pub(super) fn set_icon_name(&self, _name: String) {}
    pub(super) fn set_title(&self, _title: String) {}
    pub(super) fn rect(&self) -> Option<[i32; 4]> {
        if self.window == 0 {
            return None;
        }
        let id = icon_identifier(self.window as HWND);
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: `id` is initialized with `cbSize` and names this tray's window and icon id;
        // `rect` is a valid out-pointer.
        let hr = unsafe { Shell_NotifyIconGetRect(&id, &mut rect) };
        (hr >= 0).then(|| rect_to_xywh(rect)).flatten()
    }
}

/// The windows of the trays alive now, so a fast exit can take their icons off the
/// notification area (an icon a process leaves behind stays until the pointer passes over it).
static LIVE: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// Removes every tray icon at once, from whatever thread (`NIM_DELETE` names the icon by its
/// window and id); their threads and windows go with the process.
pub(super) fn remove_all() {
    for &window in LIVE.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        // SAFETY: `data` names an icon this process added; a removed one fails harmlessly.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &data(window as HWND, 0)) };
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|&w| w != self.window);
        // SAFETY: WM_CLOSE destroys the window on its own thread, which then quits its loop.
        unsafe { PostMessageW(self.window as HWND, WM_CLOSE, 0, 0) };
        if let Some(worker) = self.worker.take() { worker.join().ok(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_swap_and_destroy_icon_handles_valid_and_zero_dimensions() {
        // Zero dimensions return null without calling CreateIcon.
        assert!(create_icon(&TrayIcon::rgba(0, 0, Vec::new())).is_null());
        assert!(create_icon(&TrayIcon::rgba(0, 16, Vec::new())).is_null());
        assert!(create_icon(&TrayIcon::rgba(16, 0, Vec::new())).is_null());

        // Creating, swapping, and destroying offscreen HICONs opens no window or taskbar item.
        let first = create_icon(&TrayIcon::rgba(16, 16, vec![255; 16 * 16 * 4]));
        assert!(!first.is_null());
        let slot = Cell::new(first);

        // Null replacement keeps the existing handle.
        assert!(swap_icon(&slot, ptr::null_mut()).is_none());
        assert_eq!(slot.get(), first);

        let second = create_icon(&TrayIcon::rgba(16, 16, vec![255, 64, 64, 255].repeat(16 * 16)));
        assert!(!second.is_null());
        let old = swap_icon(&slot, second);
        assert_eq!(old, Some(first));
        assert_eq!(slot.get(), second);

        // SAFETY: Both HICONs were created by `create_icon` and are owned exclusively by this test.
        unsafe {
            assert_ne!(DestroyIcon(first), 0);
            assert_ne!(DestroyIcon(second), 0);
        }
    }

    #[test]
    fn icon_identifier_and_rect_conversion_match_win32_layout() {
        let fake_hwnd = 0x1234usize as HWND;
        let id = icon_identifier(fake_hwnd);
        assert_eq!(id.cbSize, size_of::<NOTIFYICONIDENTIFIER>() as u32);
        assert_eq!(id.hWnd, fake_hwnd);
        assert_eq!(id.uID, ICON_ID);

        assert_eq!(rect_to_xywh(RECT { left: 1720, top: 1040, right: 1752, bottom: 1080 }), Some([1720, 1040, 32, 40]));
        assert_eq!(rect_to_xywh(RECT { left: -200, top: -40, right: -168, bottom: 0 }), Some([-200, -40, 32, 40]));
        assert_eq!(rect_to_xywh(RECT { left: 10, top: 10, right: 10, bottom: 30 }), None);
        assert_eq!(rect_to_xywh(RECT { left: 30, top: 10, right: 10, bottom: 30 }), None);
        assert_eq!(rect_to_xywh(RECT { left: i32::MIN, top: 0, right: 10, bottom: 10 }), None);
    }

    #[test]
    fn menu_item_flags_set_checked_unchecked_and_grayed() {
        assert_eq!(menu_item_flags(true, None), MF_STRING);
        assert_eq!(menu_item_flags(false, None), MF_STRING | MF_GRAYED);
        assert_eq!(menu_item_flags(true, Some(true)), MF_STRING | MF_CHECKED);
        assert_eq!(menu_item_flags(true, Some(false)), MF_STRING | MF_UNCHECKED);
        assert_eq!(menu_item_flags(false, Some(true)), MF_STRING | MF_GRAYED | MF_CHECKED);
        assert_eq!(menu_item_flags(false, Some(false)), MF_STRING | MF_GRAYED | MF_UNCHECKED);
    }
}
