//! Where a maximized or minimized window goes back to (`WindowPlacement::restore`), as Windows
//! keeps it, and atomic `SetWindowPos` frame moves/resizes honouring `ResizeAnchor`. Windows moves
//! the restored frame with the window: Win+Shift+Arrow takes a maximized window to another display
//! and its restored frame with it, which reading the window's frame while it is restored can never
//! see.

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, IsZoomed, SW_RESTORE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    ShowWindow, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

/// What one atomic Win32 `SetWindowPos` call will do for a window frame request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Win32WindowPosPlan {
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub flags: u32,
    pub moved: bool,
    pub resized: bool,
}

/// Outer window dimensions `(cx, cy)` in physical pixels for a logical content `size` at `scale`
/// and non-client `borders`.
pub(crate) fn outer_size_for_content(size: (f32, f32), scale: f64, borders: RECT) -> (i32, i32) {
    let cw = (f64::from(size.0) * scale).round().max(1.0) as i32;
    let ch = (f64::from(size.1) * scale).round().max(1.0) as i32;
    ((cw + borders.left + borders.right).max(1), (ch + borders.top + borders.bottom).max(1))
}

/// Computes the exact outer rect and `SetWindowPos` flags for a Win32 frame request in physical
/// pixels so moving and resizing (such as bottom-anchored `fit_content_height`) happen atomically
/// in a single `SetWindowPos` call with zero intermediate jump.
pub(crate) fn plan_window_pos(
    current: [i32; 4],
    outer: Option<(i32, i32)>,
    center: bool,
    visible: [i32; 4],
    anchor: crate::ResizeAnchor,
) -> Win32WindowPosPlan {
    let current_f = current.map(|v| v as f32);
    let outer_f = outer.map(|(w, h)| (w.max(1) as f32, h.max(1) as f32));
    let visible_f = visible.map(|v| v as f32);
    let rect = crate::window::frame::snap_anchored(
        crate::window::frame::target(current_f, outer_f, center, visible_f, anchor),
        1.0,
        anchor,
    );
    let x = rect[0].round() as i32;
    let y = rect[1].round() as i32;
    let cx = rect[2].round().max(1.0) as i32;
    let cy = rect[3].round().max(1.0) as i32;
    let moved = (x, y) != (current[0], current[1]);
    let resized = outer.is_some() && (cx, cy) != (current[2], current[3]);
    let mut flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    if !moved { flags |= SWP_NOMOVE; }
    if !resized { flags |= SWP_NOSIZE; }
    Win32WindowPosPlan { x, y, cx, cy, flags, moved, resized }
}

/// Atomically applies a window size and/or centring request on Windows in one `SetWindowPos`,
/// honouring `anchor` and clamping against the monitor's work area (`rcWork`) so bottom-anchored
/// or right-anchored resizes never jump between separate resize and move calls.
pub(crate) fn apply_window_frame(
    window: &winit::window::Window,
    size: Option<(f32, f32)>,
    center: bool,
    anchor: crate::ResizeAnchor,
) -> Option<Win32WindowPosPlan> {
    let hwnd = super::hwnd(window)?;
    let scale = window.scale_factor();
    // SAFETY: Win32 queries and `SetWindowPos` on a live window owned by this thread.
    unsafe {
        let was_zoomed = size.is_some() && IsZoomed(hwnd) != 0;
        if was_zoomed {
            ShowWindow(hwnd, SW_RESTORE);
        }
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(hwnd, &mut r) == 0 { return None; }
        let current = [r.left, r.top, (r.right - r.left).max(1), (r.bottom - r.top).max(1)];
        let outer = size.map(|size| {
            let dpi = match GetDpiForWindow(hwnd) {
                0 => (scale * 96.0).round().max(96.0) as u32,
                d => d,
            };
            let b = super::frame::restored_borders(hwnd, dpi);
            outer_size_for_content(size, scale, b)
        });
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let visible = if !monitor.is_null() && GetMonitorInfoW(monitor, &mut info) != 0 {
            let w = info.rcWork;
            [w.left, w.top, (w.right - w.left).max(1), (w.bottom - w.top).max(1)]
        } else {
            current
        };
        let mut plan = plan_window_pos(current, outer, center, visible, anchor);
        if plan.moved || plan.resized {
            SetWindowPos(hwnd, std::ptr::null_mut(), plan.x, plan.y, plan.cx, plan.cy, plan.flags);
        }
        plan.resized |= was_zoomed;
        Some(plan)
    }
}

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

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
