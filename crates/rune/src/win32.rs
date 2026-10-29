//! Win32 pieces winit 0.30 doesn't expose, for overlays.

use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use std::{cell::{Cell, RefCell}, collections::VecDeque, rc::Rc};
use crate::{OverlayInput, OverlayLevel, OverlayOptions, OverlaySupport};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SPI_GETWORKAREA, SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT,
};

#[cfg(test)]
#[path = "win32_tests.rs"]
mod tests;

fn hwnd(window: &winit::window::Window) -> Option<HWND> {
    let RawWindowHandle::Win32(h) = window.window_handle().ok()?.as_raw() else { return None };
    Some(h.hwnd.get() as HWND)
}

pub(crate) fn pointer_position() -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

struct Input {
    hwnd: HWND,
    regions: RefCell<Vec<[f32; 4]>>,
    scale: Cell<f32>,
    clicks: RefCell<VecDeque<(bool, (f32, f32))>>,
    captured: Cell<bool>,
    visible: Cell<bool>,
    input: Cell<OverlayInput>,
    follow: Cell<bool>,
    level: OverlayLevel,
}
impl Input {
    fn track(&self) -> bool { self.input.get() == OverlayInput::Regions || self.follow.get() }
    fn local(&self, mut point: POINT) -> (f32, f32) {
        unsafe { windows_sys::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut point); }
        (point.x as f32 / self.scale.get(), point.y as f32 / self.scale.get())
    }
    fn hit(&self, point: (f32, f32)) -> bool {
        self.captured.get() || match self.input.get() {
            OverlayInput::All => true,
            OverlayInput::Regions => self.regions.borrow().iter().any(|r| crate::overlay_options::contains(*r, point.0, point.1)),
            _ => false,
        }
    }
    fn observe(&self, point: POINT) {
        if !self.visible.get() { return; }
        let local = self.local(point);
        {
            unsafe {
                let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
                let next = if self.hit(local) { style & !WS_EX_TRANSPARENT } else { style | WS_EX_TRANSPARENT };
                if next != style { SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, next as isize); }
            }
        }
    }
}

thread_local! {
    static INPUTS: RefCell<Vec<Rc<Input>>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, data: usize) -> LRESULT {
    // The owner removes this callback before releasing Input or the winit window.
    let input = unsafe { &*(data as *const Input) };
    match message {
        WM_NCHITTEST => {
            let p = POINT { x: lparam as i16 as i32, y: (lparam >> 16) as i16 as i32 };
            return if input.hit(input.local(p)) { HTCLIENT as LRESULT } else { HTTRANSPARENT as LRESULT };
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP => {
            let down = message != WM_LBUTTONUP;
            input.captured.set(down);
            let point = (lparam as i16 as f32 / input.scale.get(), (lparam >> 16) as i16 as f32 / input.scale.get());
            input.clicks.borrow_mut().push_back((down, point));
        }
        WM_CAPTURECHANGED => input.captured.set(false),
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Per-window input owner; the shared low-level hook observes but never consumes events.
pub(crate) struct OverlayWindow {
    watch: Option<crate::win32_pointer::MouseWatch>,
    wake: Option<crate::Waker>,
    input: Rc<Input>,
    observing: bool,
    exclusion: OverlaySupport,
}
impl OverlayWindow {
    pub fn new(window: &winit::window::Window, interactive: bool, options: &OverlayOptions, wake: Option<crate::Waker>) -> Option<Self> {
        let hwnd = hwnd(window)?;
        let mode = options.input.resolved(interactive);
        prepare_overlay(window, mode == OverlayInput::All);
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style | WS_EX_LAYERED) as isize);
        }
        let exclusion = if options.exclude_from_capture {
            if unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } != 0 { OverlaySupport::Requested } else { OverlaySupport::Failed }
        } else { OverlaySupport::Unknown };
        let input = Rc::new(Input { hwnd, regions: RefCell::default(), scale: Cell::new(window.scale_factor() as f32), clicks: RefCell::default(), captured: Cell::new(false), visible: Cell::new(false), input: Cell::new(mode), follow: Cell::new(options.follow_pointer), level: options.level });
        if unsafe { SetWindowSubclass(hwnd, Some(subclass), 0x52554e45, Rc::as_ptr(&input) as usize) } == 0 { return None; }
        Some(Self { input, observing: false, exclusion, watch: None, wake })
    }
    pub fn capture_exclusion(&self) -> OverlaySupport { self.exclusion }
    pub fn set_capture_exclusion(&mut self, exclude: bool) -> OverlaySupport {
        let affinity = if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
        let applied = unsafe { SetWindowDisplayAffinity(self.input.hwnd, affinity) } != 0;
        self.exclusion = match (exclude, applied) { (true, true) => OverlaySupport::Requested, (true, false) => OverlaySupport::Failed, (false, _) => OverlaySupport::Unknown };
        self.exclusion
    }
    pub fn global_pointer(&self) -> OverlaySupport {
        if self.input.track() && self.watch.as_ref().is_none_or(|w| !w.supported()) { OverlaySupport::Failed }
        else { OverlaySupport::Supported }
    }
    pub fn take_pointer(&self) -> Option<(f32, f32)> {
        let (x, y) = self.watch.as_ref()?.take()?;
        self.input.observe(POINT { x, y });
        Some(self.input.local(POINT { x, y }))
    }
    pub fn set_input(&mut self, input: OverlayInput) {
        self.input.input.set(input);
        self.refresh_watch();
        if let Some((x, y)) = pointer_position() { self.input.observe(POINT { x, y }); }
    }
    pub fn set_follow_pointer(&mut self, follow: bool) { self.input.follow.set(follow); self.refresh_watch(); }

    fn refresh_watch(&mut self) {
        if self.input.visible.get() && self.input.track() {
            if self.watch.is_none() { self.watch = Some(crate::win32_pointer::MouseWatch::new(self.wake.clone())); }
            if let Some(watch) = &self.watch { watch.geometry(self.input.hwnd, self.input.follow.get()); }
        } else { self.watch.take(); }
    }
    pub fn take_click(&self, down: bool) -> Option<(f32, f32)> {
        self.input.clicks.borrow_mut().pop_front().filter(|(pressed, _)| *pressed == down).map(|(_, point)| point)
    }
    pub fn set_regions(&self, regions: &[[f32; 4]], scale: f32) {
        self.input.scale.set(scale);
        if self.input.regions.borrow().as_slice() != regions { *self.input.regions.borrow_mut() = regions.to_vec(); }
        if let Some(watch) = &self.watch { watch.geometry(self.input.hwnd, self.input.follow.get()); }
        if let Some((x, y)) = pointer_position() {
            self.input.observe(POINT { x, y });
            if let Some(watch) = &self.watch { watch.observe((x, y)); }
        }
    }
    pub fn show(&mut self) {
        self.input.visible.set(true);
        if self.observing { return; }
        INPUTS.with(|inputs| inputs.borrow_mut().push(self.input.clone()));
        self.refresh_watch();
        self.observing = true;
        if let Some((x, y)) = pointer_position() { self.input.observe(POINT { x, y }); }
    }
    pub fn hide(&mut self) {
        self.input.visible.set(false);
        self.input.captured.set(false);
        self.watch.take();
        self.input.clicks.borrow_mut().clear();
        if !self.observing { return; }
        INPUTS.with(|inputs| inputs.borrow_mut().retain(|i| !Rc::ptr_eq(i, &self.input)));
        self.observing = false;
    }
}
impl Drop for OverlayWindow {
    fn drop(&mut self) {
        self.hide();
        unsafe { RemoveWindowSubclass(self.input.hwnd, Some(subclass), 0x52554e45); }
    }
}

/// Topmost tool window that never activates; unless `interactive`, every click goes through.
pub(crate) fn prepare_overlay(window: &winit::window::Window, interactive: bool) {
    let Some(hwnd) = hwnd(window) else { return };
    let through = if interactive { 0 } else { WS_EX_TRANSPARENT | WS_EX_LAYERED };
    // SAFETY: style changes on a live window owned by this thread.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let style = style | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | through;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize);
    }
}

/// Shows the overlay without activating it, above other topmost windows.
pub(crate) fn show_overlay(window: &winit::window::Window) {
    let Some(hwnd) = hwnd(window) else { return };
    // SAFETY: Win32 calls on a live window owned by this thread.
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
    let inputs = INPUTS.with(|inputs| inputs.borrow().clone());
    for level in [OverlayLevel::Status, OverlayLevel::AboveStatus, OverlayLevel::AboveOverlays] {
        for input in &inputs {
            if input.visible.get() && input.level == level {
                unsafe { SetWindowPos(input.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
            }
        }
    }
}

/// Hide the window that `show_overlay` exposed directly through Win32.
pub(crate) fn hide_overlay(window: &winit::window::Window) {
    let Some(hwnd) = hwnd(window) else { return };
    // winit did not perform the show, so its cached visibility may still be false.
    unsafe { ShowWindow(hwnd, SW_HIDE); }
}

/// The bottom of the primary display's work area (above the taskbar), in physical pixels.
pub(crate) fn work_area_bottom() -> Option<i32> {
    let mut area = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: SPI_GETWORKAREA writes one RECT.
    (unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut area).cast(), 0) } != 0).then_some(area.bottom)
}
