//! Whether a window can be seen, asked when it's about to draw a frame (`window::look`), never
//! listened for. Nothing in the process hears other apps' windows: a window that draws nothing
//! costs nothing whatever the rest of the desktop does, and one that draws pays one walk of the
//! z-order a quarter second at most.
//!
//! The walk is Chromium's `WindowOcclusionCalculator`'s: from the top of the z-order down, what's
//! left of the screens shrinks by each opaque window, and the window shows if any of what's left
//! is in its rect when the walk reaches it. Minimised, on another virtual desktop (DWM cloaks it)
//! or behind the lock screen, it's covered too.
//!
//! Only on the desktop the user sees. A window on another one (a capture harness's hidden
//! desktop, which several apps share) is never covered, so it keeps drawing for whoever reads
//! its pixels.

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Dwm::{DWM_CLOAKED_SHELL, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    COMPLEXREGION, CombineRgn, CreateRectRgn, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW, GetWindowRgn, HDC, HMONITOR, HRGN,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, RGN_DIFF, RGN_OR, RectInRegion,
};
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, DESKTOP_READOBJECTS, GetThreadDesktop, GetUserObjectInformationW, HDESK, OpenInputDesktop, UOI_NAME};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetLayeredWindowAttributes, GetWindowLongW, GetWindowPlacement, IsIconic, IsWindowVisible,
    LWA_ALPHA, LWA_COLORKEY, SW_MAXIMIZE, WINDOWPLACEMENT, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};

/// What a look found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    Seen,
    Covered,
}

/// Whether `window` can be seen now.
pub(crate) fn look(window: &winit::window::Window) -> Found {
    let Some(hwnd) = super::hwnd(window) else { return Found::Seen };
    match desktop() {
        Desktop::Other => return Found::Seen,
        Desktop::Locked => return Found::Covered,
        Desktop::Shown => {}
    }
    // SAFETY: plain queries on a live top-level window.
    unsafe {
        if IsWindowVisible(hwnd) == 0 { return Found::Seen; }
        if IsIconic(hwnd) != 0 || cloaked(hwnd).is_some_and(|c| c & DWM_CLOAKED_SHELL != 0) { return Found::Covered; }
    }
    walk(hwnd)
}

/// Where this thread's desktop is, against the one the user sees.
enum Desktop {
    /// It's the one the user sees.
    Shown,
    /// The user's, behind the lock screen or a secure desktop (the input desktop can't be read).
    Locked,
    /// Another one (a harness's hidden desktop): nothing covers it.
    Other,
}

fn desktop() -> Desktop {
    // SAFETY: desktop handles this function opens it closes; the thread's own isn't closed.
    unsafe {
        let ours = desktop_name(GetThreadDesktop(GetCurrentThreadId()));
        let input = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if input.is_null() {
            return if ours.as_deref() == Some(DEFAULT) { Desktop::Locked } else { Desktop::Other };
        }
        let same = desktop_name(input).is_some_and(|name| ours == Some(name));
        CloseDesktop(input);
        if same { Desktop::Shown } else if ours.as_deref() == Some(DEFAULT) { Desktop::Locked } else { Desktop::Other }
    }
}

/// The interactive desktop's name ("Default").
const DEFAULT: &[u16] = &[0x44, 0x65, 0x66, 0x61, 0x75, 0x6C, 0x74];

fn desktop_name(desktop: HDESK) -> Option<Vec<u16>> {
    if desktop.is_null() { return None; }
    let mut name = [0u16; 256];
    let mut needed = 0u32;
    // SAFETY: `name` holds 512 bytes, as passed.
    let ok = unsafe { GetUserObjectInformationW(desktop, UOI_NAME, name.as_mut_ptr().cast(), std::mem::size_of_val(&name) as u32, &mut needed) };
    if ok == 0 { return None; }
    Some(name.iter().copied().take_while(|&c| c != 0).collect())
}

/// One walk down the z-order to `target`.
fn walk(target: HWND) -> Found {
    struct Walk {
        target: HWND,
        left: HRGN,
        found: Option<Found>,
    }
    unsafe extern "system" fn monitor(_m: HMONITOR, _dc: HDC, rect: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the walk's region; `rect` the monitor's, from the system.
        unsafe {
            let left = data as HRGN;
            let r = &*rect;
            let screen = CreateRectRgn(r.left, r.top, r.right, r.bottom);
            CombineRgn(left, left, screen, RGN_OR);
            DeleteObject(screen);
        }
        TRUE
    }
    unsafe extern "system" fn window(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Walk` `walk` passes, alive for the enumeration.
        let walk = unsafe { &mut *(data as *mut Walk) };
        if hwnd == walk.target {
            // SAFETY: a live top-level window and the walk's region.
            let shows = bounds(hwnd).is_none_or(|r| unsafe { RectInRegion(walk.left, &r) } != 0);
            walk.found = Some(if shows { Found::Seen } else { Found::Covered });
            return 0;
        }
        if let Some(r) = opaque_rect(hwnd) {
            // SAFETY: regions this walk owns.
            unsafe {
                let rgn = CreateRectRgn(r.left, r.top, r.right, r.bottom);
                CombineRgn(walk.left, walk.left, rgn, RGN_DIFF);
                DeleteObject(rgn);
            }
        }
        TRUE
    }
    // SAFETY: the region is this walk's, deleted below; the callbacks get valid pointers.
    unsafe {
        let left = CreateRectRgn(0, 0, 0, 0);
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(monitor), left as LPARAM);
        let mut walk = Walk { target, left, found: None };
        EnumWindows(Some(window), (&raw mut walk) as LPARAM);
        DeleteObject(left);
        // Not reached (destroyed meanwhile): nothing to hold back.
        walk.found.unwrap_or(Found::Seen)
    }
}

/// The rect an opaque window covers, or None for one that may let what's under it show, as
/// Chromium's `IsWindowVisibleAndFullyOpaque` decides.
fn opaque_rect(hwnd: HWND) -> Option<RECT> {
    // SAFETY: plain queries on a window `EnumWindows` handed over.
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 { return None; }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        // Clicks fall through it.
        if ex & WS_EX_TRANSPARENT != 0 { return None; }
        // Floating windows can be larger than what they draw; the taskbar isn't.
        if (ex & WS_EX_TOOLWINDOW != 0 || GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_POPUP != 0) && !taskbar(hwnd) { return None; }
        if ex & WS_EX_LAYERED != 0 {
            let (mut key, mut alpha, mut flags) = (0u32, 0u8, 0u32);
            // Fails for a window drawn with UpdateLayeredWindow: per-pixel alpha.
            if GetLayeredWindowAttributes(hwnd, &mut key, &mut alpha, &mut flags) == 0 { return None; }
            if flags & LWA_COLORKEY != 0 || (flags & LWA_ALPHA != 0 && alpha < 255) { return None; }
        }
        let rgn = CreateRectRgn(0, 0, 0, 0);
        let shape = GetWindowRgn(hwnd, rgn);
        DeleteObject(rgn);
        if shape == COMPLEXREGION { return None; }
        // Shown to no one: on another virtual desktop, or a shell window kept for later.
        if cloaked(hwnd).is_some_and(|c| c != 0) { return None; }
        let mut r = bounds(hwnd)?;
        if r.right <= r.left || r.bottom <= r.top { return None; }
        let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut placement) != 0 && placement.showCmd == SW_MAXIMIZE as u32 {
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) != 0 {
                let w = info.rcWork;
                r = RECT { left: r.left.max(w.left), top: r.top.max(w.top), right: r.right.min(w.right), bottom: r.bottom.min(w.bottom) };
            }
        }
        Some(r)
    }
}

/// The rect DWM draws a window in, without its invisible resize borders.
fn bounds(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: `r` is a RECT, as the attribute's size says.
    let hr = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&raw mut r).cast(), std::mem::size_of::<RECT>() as u32) };
    (hr >= 0).then_some(r)
}

/// Why DWM keeps a window off screen, if it does: `DWM_CLOAKED_SHELL` for another virtual
/// desktop, `DWM_CLOAKED_APP` for its app's own cloak.
fn cloaked(hwnd: HWND) -> Option<u32> {
    let mut why = 0u32;
    // SAFETY: `why` is a u32, as the attribute's size says.
    let hr = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, (&raw mut why).cast(), 4) };
    (hr >= 0).then_some(why)
}

fn taskbar(hwnd: HWND) -> bool {
    let mut class = [0u16; 32];
    // SAFETY: `class` holds 32 characters, as passed.
    let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    n > 0 && String::from_utf16_lossy(&class[..n as usize]) == "Shell_TrayWnd"
}
