//! Where a maximized or minimized window goes back to (`WindowPlacement::restore`), as Windows
//! keeps it. Windows moves that frame with the window: Win+Shift+Arrow takes a maximized window
//! to another display and its restored frame with it, which reading the window's frame while it
//! is restored can never see.

use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, GetWindowPlacement, WINDOWPLACEMENT, WS_EX_TOOLWINDOW};

/// The frame restoring `window` returns to: its top-left in desktop pixels and its content
/// size, the space `WindowOptions::position` and `physical_size` take.
pub(crate) fn restore_frame(window: &winit::window::Window) -> Option<((i32, i32), (u32, u32))> {
    let hwnd = super::hwnd(window)?;
    // SAFETY: plain queries on a live window this thread owns; each struct carries its size.
    unsafe {
        let mut p: WINDOWPLACEMENT = std::mem::zeroed();
        p.length = size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut p) == 0 { return None; }
        let mut r = p.rcNormalPosition;
        // Workspace coordinates, whose origin is the top-left of the work area of the display
        // the frame is on (a taskbar on the left or top moves it); a tool window's are the
        // screen's. Chromium converts them the same way (`HWNDMessageHandler::GetWindowPlacement`).
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW == 0 {
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST), &mut info) != 0 {
                let (dx, dy) = (info.rcWork.left - info.rcMonitor.left, info.rcWork.top - info.rcMonitor.top);
                (r.left, r.top, r.right, r.bottom) = (r.left + dx, r.top + dy, r.right + dx, r.bottom + dy);
            }
        }
        let b = super::frame::restored_borders(hwnd, GetDpiForWindow(hwnd));
        let width = (r.right - r.left - b.left - b.right).max(1) as u32;
        let height = (r.bottom - r.top - b.top - b.bottom).max(1) as u32;
        Some(((r.left, r.top), (width, height)))
    }
}
