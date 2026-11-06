//! A custom frame for a window whose app draws its own titlebar (`App::titlebar`).
//!
//! The window keeps its standard styles (caption, sizing border, system menu, minimize and
//! maximize boxes), so Windows still gives it resize edges, Aero Snap, the snap layouts
//! flyout, the window menu and its animations; only the title bar goes. `WM_NCCALCSIZE`
//! hands the caption's band to the client area and keeps the side and bottom borders, which
//! Windows 10 and 11 draw invisibly outside the window, as resize edges. The top resize edge
//! is then a band inside the client area that `WM_NCHITTEST` reports as `HTTOP`.
//!
//! Rune draws the caption buttons as elements (`titlebar::caption`), and this procedure answers
//! hit tests over them as `HTMINBUTTON`, `HTMAXBUTTON` and `HTCLOSE`. Windows 11 shows its snap
//! layouts flyout over a window's `HTMAXBUTTON`, so they have to be non-client hits; the
//! pointer over them then arrives as non-client messages, which this procedure turns into the
//! buttons' hover and press state and their actions (`WM_SYSCOMMAND`), never letting the
//! default procedure draw classic buttons over them.
//!
//! winit sizes the window from its client size with `AdjustWindowRectEx`, which counts the
//! title bar this frame removed: the minimum and maximum track sizes, the size for a new scale
//! (`WM_GETDPISCALEDSIZE`) and `request_inner_size` are corrected here to the frame's borders.

use std::cell::Cell;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, MonitorFromWindow, MONITOR_DEFAULTTONEAREST, GetMonitorInfoW, MONITORINFO, ScreenToClient};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::Shell::{ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETAUTOHIDEBAREX, APPBARDATA, DefSubclassProc, RemoveWindowSubclass, SHAppBarMessage, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::titlebar::{Caption, CaptionButton};

const SUBCLASS: usize = 0x5255_4e46;

/// A window's custom frame: installed for its life, removed as the window is destroyed.
pub(crate) struct CustomFrame {
    state: Rc<State>,
}

struct State {
    /// The titlebar's height in points, which the caption buttons take.
    height: f32,
    hover: Cell<Option<CaptionButton>>,
    pressed: Cell<Option<CaptionButton>>,
    /// A `TrackMouseEvent` for the non-client area is pending.
    tracking: Cell<bool>,
}

impl CustomFrame {
    /// Takes the title bar off `window`, keeping its client size.
    pub(crate) fn install(window: &winit::window::Window, height: f32) -> Option<Self> {
        let hwnd = super::hwnd(window)?;
        let state = Rc::new(State { height, hover: Cell::new(None), pressed: Cell::new(None), tracking: Cell::new(false) });
        let data = Rc::into_raw(state.clone());
        // SAFETY: `data` stays alive until WM_NCDESTROY, where the subclass that reads it goes.
        if unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, data as usize) } == 0 {
            // SAFETY: never handed to Windows.
            drop(unsafe { Rc::from_raw(data) });
            return None;
        }
        FRAMED.with(|f| f.borrow_mut().push(hwnd as usize));
        let before = client_size(hwnd);
        // SAFETY: Win32 calls on a live window this thread owns.
        unsafe {
            SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            let grown = client_size(hwnd).1 - before.1;
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if grown != 0 && IsZoomed(hwnd) == 0 && GetWindowRect(hwnd, &mut r) != 0 {
                SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, r.right - r.left, r.bottom - r.top - grown, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            }
        }
        Some(Self { state })
    }

    /// The caption buttons as they are now. A press shows only while the pointer is still
    /// over the button it began on.
    pub(crate) fn caption(&self, window: &winit::window::Window) -> Caption {
        let hover = self.state.hover.get();
        Caption {
            height: self.state.height,
            hover,
            pressed: self.state.pressed.get().filter(|p| hover == Some(*p)),
            maximized: window.is_maximized(),
        }
    }
}

/// Sizes `window`'s client area to `size` points, as `request_inner_size` would for a window
/// with this frame. False when it has none, for winit to do it.
pub(crate) fn request_inner_size(window: &winit::window::Window, size: (f32, f32)) -> bool {
    let Some(hwnd) = super::hwnd(window).filter(|&h| installed(h)) else { return false };
    let scale = window.scale_factor();
    let (w, h) = ((f64::from(size.0) * scale).round() as i32, (f64::from(size.1) * scale).round() as i32);
    // SAFETY: Win32 calls on a live window this thread owns.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let b = borders(hwnd, dpi);
    unsafe {
        if IsZoomed(hwnd) != 0 { ShowWindow(hwnd, SW_RESTORE); }
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, w + b.left + b.right, h + b.bottom, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
    }
    true
}

/// Opens the window menu at the pointer, as a right-click on a native title bar does, and
/// carries out what the user picks.
pub(crate) fn system_menu(window: &winit::window::Window) {
    let (Some(hwnd), Some((x, y))) = (super::hwnd(window), super::pointer_position()) else { return };
    // SAFETY: Win32 calls on a live window this thread owns and its own system menu.
    unsafe {
        let menu = GetSystemMenu(hwnd, 0);
        if menu.is_null() { return; }
        let zoomed = IsZoomed(hwnd) != 0;
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let enable = |item: u32, on: bool| { EnableMenuItem(menu, item, MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED }); };
        enable(SC_RESTORE, zoomed);
        enable(SC_MOVE, !zoomed);
        enable(SC_SIZE, !zoomed && style & WS_THICKFRAME != 0);
        enable(SC_MINIMIZE, style & WS_MINIMIZEBOX != 0);
        enable(SC_MAXIMIZE, !zoomed && style & WS_MAXIMIZEBOX != 0);
        enable(SC_CLOSE, true);
        SetMenuDefaultItem(menu, SC_CLOSE, 0);
        let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, 0, hwnd, std::ptr::null());
        if picked != 0 { PostMessageW(hwnd, WM_SYSCOMMAND, picked as WPARAM, 0); }
    }
}

thread_local! {
    /// The windows on this thread with a custom frame. (`GetWindowSubclass` would say, but the
    /// system's version 5 comctl32 exports it only by ordinal.)
    static FRAMED: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn installed(hwnd: HWND) -> bool {
    FRAMED.with(|f| f.borrow().contains(&(hwnd as usize)))
}

unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, data: usize) -> LRESULT {
    // SAFETY: `data` is the live `State` until WM_NCDESTROY.
    let state = unsafe { &*(data as *const State) };
    if message == WM_NCDESTROY {
        FRAMED.with(|f| f.borrow_mut().retain(|&h| h != hwnd as usize));
        // SAFETY: removes this subclass, then releases the reference `install` gave it.
        unsafe {
            RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS);
            drop(Rc::from_raw(data as *const State));
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
    }
    if !framed(hwnd) {
        // Full screen: no frame to adjust.
        // SAFETY: forwards to the next window procedure.
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }
    // SAFETY: each arm reads the message's own parameters as Windows documents them, on a
    // live window this thread owns.
    unsafe {
        match message {
            WM_NCCALCSIZE if wparam != 0 => {
                let params = &mut *(lparam as *mut NCCALCSIZE_PARAMS);
                let top = params.rgrc[0].top;
                DefSubclassProc(hwnd, message, wparam, lparam);
                params.rgrc[0].top = top;
                if IsZoomed(hwnd) != 0 { fit_maximized(hwnd, &mut params.rgrc[0]); }
                return 0;
            }
            WM_NCHITTEST => {
                let hit = DefSubclassProc(hwnd, message, wparam, lparam);
                if hit != HTCLIENT as LRESULT { return hit; }
                return hit_test(state, hwnd, POINT { x: lparam as i16 as i32, y: (lparam >> 16) as i16 as i32 }) as LRESULT;
            }
            WM_NCMOUSEMOVE => {
                let button = caption_button(wparam);
                state.set_hover(hwnd, button);
                if button.is_some() {
                    if !state.tracking.replace(true) {
                        let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                        TrackMouseEvent(&mut track);
                    }
                    return 0;
                }
            }
            WM_NCMOUSELEAVE => {
                state.tracking.set(false);
                state.pressed.set(None);
                state.set_hover(hwnd, None);
            }
            WM_MOUSEMOVE if state.hover.get().is_some() => {
                state.pressed.set(None);
                state.set_hover(hwnd, None);
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if caption_button(wparam).is_some() => {
                state.pressed.set(caption_button(wparam));
                state.set_hover(hwnd, caption_button(wparam));
                invalidate(hwnd);
                return 0;
            }
            WM_NCLBUTTONUP if caption_button(wparam).is_some() => {
                let button = caption_button(wparam);
                if state.pressed.replace(None) == button && let Some(button) = button {
                    let command = match button {
                        CaptionButton::Minimize => SC_MINIMIZE,
                        CaptionButton::Maximize if IsZoomed(hwnd) != 0 => SC_RESTORE,
                        CaptionButton::Maximize => SC_MAXIMIZE,
                        CaptionButton::Close => SC_CLOSE,
                    };
                    PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0);
                }
                invalidate(hwnd);
                return 0;
            }
            WM_NCRBUTTONDOWN | WM_NCRBUTTONUP | WM_NCRBUTTONDBLCLK if caption_button(wparam).is_some() => return 0,
            // Alt+Space: winit takes every character message for its key events, so the default
            // procedure never opens the window menu. The app still sees the key first.
            WM_SYSCHAR if wparam == 0x20 && lparam & (1 << 29) != 0 => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                DefWindowProcW(hwnd, message, wparam, lparam);
                return result;
            }
            // The default would repaint a title bar this window doesn't have.
            WM_NCACTIVATE => return DefSubclassProc(hwnd, message, wparam, -1),
            WM_GETMINMAXINFO => {
                let info = &mut *(lparam as *mut MINMAXINFO);
                let (min, max) = (info.ptMinTrackSize.y, info.ptMaxTrackSize.y);
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                // winit sized them for a title bar: take it back off what it set.
                let top = borders(hwnd, GetDpiForWindow(hwnd)).top;
                if info.ptMinTrackSize.y != min { info.ptMinTrackSize.y = (info.ptMinTrackSize.y - top).max(1); }
                if info.ptMaxTrackSize.y != max { info.ptMaxTrackSize.y = (info.ptMaxTrackSize.y - top).max(1); }
                return result;
            }
            WM_GETDPISCALEDSIZE if IsZoomed(hwnd) == 0 => {
                let dpi = wparam as u32;
                let (w, h) = client_size(hwnd);
                let ratio = f64::from(dpi) / f64::from(GetDpiForWindow(hwnd).max(1));
                let b = borders(hwnd, dpi);
                let out = &mut *(lparam as *mut SIZE);
                out.cx = (f64::from(w) * ratio).round() as i32 + b.left + b.right;
                out.cy = (f64::from(h) * ratio).round() as i32 + b.bottom;
                return 1;
            }
            WM_DPICHANGED => {
                // winit resizes for a title bar; Windows' rect (from WM_GETDPISCALEDSIZE) is right.
                let suggested = *(lparam as *const RECT);
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                let RECT { left, top, right, bottom } = suggested;
                SetWindowPos(hwnd, std::ptr::null_mut(), left, top, right - left, bottom - top, SWP_NOZORDER | SWP_NOACTIVATE);
                return result;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

impl State {
    fn set_hover(&self, hwnd: HWND, button: Option<CaptionButton>) {
        if self.hover.replace(button) != button { invalidate(hwnd); }
    }
}

/// The caption buttons draw with the next frame: winit turns the paint into a redraw.
fn invalidate(hwnd: HWND) {
    // SAFETY: a Win32 call on a live window this thread owns.
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0); }
}

/// A window with a title bar style, so not full screen.
fn framed(hwnd: HWND) -> bool {
    // SAFETY: reads the style of a live window.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    style & WS_CAPTION == WS_CAPTION
}

fn caption_button(hit: WPARAM) -> Option<CaptionButton> {
    match hit as u32 {
        HTMINBUTTON => Some(CaptionButton::Minimize),
        HTMAXBUTTON => Some(CaptionButton::Maximize),
        HTCLOSE => Some(CaptionButton::Close),
        _ => None,
    }
}

/// What a point (screen pixels) in the client area hits: the top resize edge, a caption
/// button, or the client.
fn hit_test(state: &State, hwnd: HWND, mut point: POINT) -> u32 {
    // SAFETY: Win32 calls on a live window this thread owns.
    let (dpi, zoomed, resizable) = unsafe {
        ScreenToClient(hwnd, &mut point);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        (GetDpiForWindow(hwnd), IsZoomed(hwnd) != 0, style & WS_THICKFRAME != 0)
    };
    let edge = frame_thickness(dpi);
    let client_width = client_size(hwnd).0;
    if !zoomed && resizable && point.y >= 0 && point.y < edge {
        return if point.x < edge { HTTOPLEFT } else if point.x >= client_width - edge { HTTOPRIGHT } else { HTTOP };
    }
    let scale = dpi as f32 / 96.0;
    let width = client_width as f32 / scale;
    let (x, y) = (point.x as f32 / scale, point.y as f32 / scale);
    let caption = Caption::new(state.height);
    let hit = caption.rects(width, scale).into_iter()
        .find(|(_, [rx, ry, rw, rh])| x >= *rx && x < rx + rw && y >= *ry && y < ry + rh);
    match hit.map(|(button, _)| button) {
        Some(CaptionButton::Minimize) => HTMINBUTTON,
        Some(CaptionButton::Maximize) => HTMAXBUTTON,
        Some(CaptionButton::Close) => HTCLOSE,
        None => HTCLIENT,
    }
}

/// The sizing border's thickness: the top resize edge, and how far a maximized window's top
/// hangs off the display.
fn frame_thickness(dpi: u32) -> i32 {
    // SAFETY: reads system metrics.
    unsafe { GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) }
}

/// What a standard frame adds around the client area at `dpi`, each edge as a positive width.
fn borders(hwnd: HWND, dpi: u32) -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: reads a live window's styles, and fills `r`.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        AdjustWindowRectExForDpi(&mut r, style, 0, ex, dpi);
    }
    RECT { left: -r.left, top: -r.top, right: r.right, bottom: r.bottom }
}

/// A maximized window hangs its frame off the display: the client area starts below the top
/// that hangs off, and leaves a pixel at an edge with an auto-hiding taskbar so the pointer
/// can still bring it up.
fn fit_maximized(hwnd: HWND, client: &mut RECT) {
    // SAFETY: Win32 calls on a live window, its monitor and the shell's app bars.
    unsafe {
        client.top += frame_thickness(GetDpiForWindow(hwnd));
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 { return; }
        for edge in [ABE_TOP, ABE_LEFT, ABE_BOTTOM, ABE_RIGHT] {
            let mut bar: APPBARDATA = std::mem::zeroed();
            bar.cbSize = size_of::<APPBARDATA>() as u32;
            bar.uEdge = edge;
            bar.rc = info.rcMonitor;
            if SHAppBarMessage(ABM_GETAUTOHIDEBAREX, &mut bar) == 0 { continue; }
            match edge {
                ABE_TOP => client.top += 1,
                ABE_LEFT => client.left += 1,
                ABE_BOTTOM => client.bottom -= 1,
                _ => client.right -= 1,
            }
        }
    }
}

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: fills `r` for a live window.
    unsafe { GetClientRect(hwnd, &mut r); }
    (r.right - r.left, r.bottom - r.top)
}
