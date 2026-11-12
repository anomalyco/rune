//! Win32 pieces winit 0.30 doesn't expose, for overlays.

use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use std::{cell::{Cell, RefCell}, collections::VecDeque, rc::Rc};
use crate::{OverlayInput, OverlayLevel, OverlayOptions, OverlaySupport};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT,
};

mod cloak;
pub(crate) mod dark_mode;
pub(crate) mod dcomp_handover;
pub(crate) mod dib;
mod dpi_drag;
pub(crate) mod drop_target;
mod frame;
pub(crate) mod jump_list;
pub(crate) mod occlusion;
mod placement;
pub(crate) mod press_point;
mod pointer;
pub(crate) mod session_lock;
pub(crate) mod taskbar;

pub(crate) use cloak::Cloak;
pub(crate) use dpi_drag::follow_suggested_rect;
pub(crate) use dpi_drag::in_size_move;
pub(crate) use frame::CustomFrame;
pub(crate) use frame::has_window_menu;
pub(crate) use frame::request_inner_size;
pub(crate) use frame::window_menu;
pub(crate) use placement::restore_frame;

pub(crate) fn hwnd(window: &winit::window::Window) -> Option<HWND> {
    let RawWindowHandle::Win32(h) = window.window_handle().ok()?.as_raw() else { return None };
    Some(h.hwnd.get() as HWND)
}

/// When the display shows its next refresh after `now`, from DWM's timing (`now` plus a 60 Hz
/// frame when DWM won't say).
pub(crate) fn next_vblank(now: std::time::Instant) -> std::time::Instant {
    use windows_sys::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    // SAFETY: plain out-parameters.
    unsafe {
        let mut t: DWM_TIMING_INFO = std::mem::zeroed();
        t.cbSize = size_of::<DWM_TIMING_INFO>() as u32;
        let (mut qpc, mut freq) = (0i64, 0i64);
        if DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut t) < 0 || t.qpcRefreshPeriod == 0
            || QueryPerformanceCounter(&mut qpc) == 0 || QueryPerformanceFrequency(&mut freq) == 0 || freq <= 0 {
            return now + std::time::Duration::from_micros(16_667);
        }
        let (period, last) = (t.qpcRefreshPeriod as i64, t.qpcVBlank as i64);
        let next = last + ((qpc - last) / period + 1) * period;
        now + std::time::Duration::from_nanos(((next - qpc).max(0) as i128 * 1_000_000_000 / freq as i128) as u64)
    }
}

/// Whether a screen reader is running (`SPI_GETSCREENREADER`).
pub(crate) fn screen_reader() -> bool {
    let mut on = 0i32;
    // SAFETY: SPI_GETSCREENREADER writes one BOOL.
    unsafe { SystemParametersInfoW(SPI_GETSCREENREADER, 0, (&raw mut on).cast(), 0) != 0 && on != 0 }
}

/// Whether the system asks `window` to paint part of itself again (an update region): it threw
/// away what the window showed, as when a window covered whole comes back.
pub(crate) fn repaint_owed(window: &winit::window::Window) -> bool {
    use windows_sys::Win32::Graphics::Gdi::GetUpdateRect;
    let Some(hwnd) = hwnd(window) else { return false };
    // SAFETY: a live window this thread owns; no rectangle asked for.
    unsafe { GetUpdateRect(hwnd, std::ptr::null_mut(), 0) != 0 }
}

/// Raises D3D12's "D3D Background Thread"s, once, from idle priority to below normal, on a thread of
/// its own. D3D12 starts them at idle priority (base 1) for the driver's shader work, and a thread
/// has to run to end: on a busy machine they sat Ready, never scheduled, after the process was
/// ended (a kill, the uninstaller closing running copies), keeping it alive for seconds, once
/// 106 s. Called once a first GPU frame is done; a second pass 2 s later takes any D3D12 started
/// since. Returns how many it raised.
pub(crate) fn unidle_d3d_threads_soon() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| { let _ = std::thread::Builder::new().name("rune-unidle".into()).spawn(|| { unidle_d3d_threads(); std::thread::sleep(std::time::Duration::from_secs(2)); unidle_d3d_threads(); }); });
}

/// This process's threads by name and priority, counted (`name (priority) xN`, unnamed ones as
/// `?`), for saying what a quit waits on.
pub(crate) fn threads_summary() -> String {
    let mut seen: Vec<(String, i32, usize)> = Vec::new();
    for_each_own_thread(|thread| {
        let name = thread_name(thread).filter(|n| !n.is_empty()).unwrap_or_else(|| "?".into());
        // SAFETY: a thread handle `for_each_own_thread` opened for querying.
        let priority = unsafe { windows_sys::Win32::System::Threading::GetThreadPriority(thread) };
        match seen.iter_mut().find(|(n, p, _)| *n == name && *p == priority) {
            Some(s) => s.2 += 1,
            None => seen.push((name, priority, 1)),
        }
    });
    seen.iter().map(|(n, p, c)| if *c > 1 { format!("{n} ({p}) x{c}") } else { format!("{n} ({p})") }).collect::<Vec<_>>().join(", ")
}

/// Raises this process's threads whose name passes `named` and whose priority is under `floor` to
/// `floor` (a `THREAD_PRIORITY_*`); returns how many. Walks only this process's threads.
pub fn raise_threads(named: impl Fn(&str) -> bool, floor: i32) -> usize {
    use windows_sys::Win32::System::Threading::{GetThreadPriority, SetThreadPriority};
    let mut raised = 0;
    for_each_own_thread(|thread| {
        // SAFETY: a thread handle `for_each_own_thread` opened for querying and setting.
        unsafe {
            if GetThreadPriority(thread) < floor && thread_name(thread).is_some_and(|n| named(&n)) && SetThreadPriority(thread, floor) != 0 {
                raised += 1;
            }
        }
    });
    raised
}

fn unidle_d3d_threads() -> usize {
    raise_threads(|name| name.starts_with("D3D Background Thread"), windows_sys::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL)
}

/// Calls `f` with a handle to each of this process's threads, open for querying and setting its
/// priority, walking this process's own thread list (`NtGetNextThread`). A Toolhelp snapshot lists
/// every thread on the machine: 67-75 ms a walk on a busy one, twice per launch.
fn for_each_own_thread(mut f: impl FnMut(windows_sys::Win32::Foundation::HANDLE)) {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SET_INFORMATION};
    type NextThread = unsafe extern "system" fn(HANDLE, HANDLE, u32, u32, u32, *mut HANDLE) -> i32;
    static NEXT: std::sync::OnceLock<Option<NextThread>> = std::sync::OnceLock::new();
    // SAFETY: ntdll is loaded in every process; NtGetNextThread has this signature since Vista.
    let next = *NEXT.get_or_init(|| unsafe {
        let ntdll = GetModuleHandleW("ntdll.dll\0".encode_utf16().collect::<Vec<_>>().as_ptr());
        if ntdll.is_null() { return None; }
        GetProcAddress(ntdll, c"NtGetNextThread".as_ptr().cast()).map(|p| std::mem::transmute::<_, NextThread>(p))
    });
    let Some(next) = next else { return };
    let mut prev: HANDLE = std::ptr::null_mut();
    loop {
        let mut thread: HANDLE = std::ptr::null_mut();
        // SAFETY: this process's pseudo-handle and the last thread handle it returned (or none);
        // each handle it returns is closed once the next is taken.
        let status = unsafe { next(GetCurrentProcess(), prev, THREAD_QUERY_LIMITED_INFORMATION | THREAD_SET_INFORMATION, 0, 0, &mut thread) };
        if !prev.is_null() { unsafe { CloseHandle(prev) }; }
        if status < 0 || thread.is_null() { return; }
        f(thread);
        prev = thread;
    }
}

/// A thread's description (`SetThreadDescription`), if it has one.
fn thread_name(thread: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::System::Threading::GetThreadDescription;
    let mut desc = std::ptr::null_mut();
    // SAFETY: a live thread handle; the description is read to its NUL and freed with LocalFree.
    unsafe {
        if GetThreadDescription(thread, &mut desc) < 0 || desc.is_null() { return None; }
        let len = (0..).take_while(|&i| *desc.add(i) != 0).count();
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(desc, len));
        LocalFree(desc.cast());
        Some(name)
    }
}

/// Drops the paint the system still owes `window`, which the loop just painted itself.
pub(crate) fn drop_paint(window: &winit::window::Window) {
    use windows_sys::Win32::Graphics::Gdi::{RDW_NOINTERNALPAINT, RedrawWindow};
    let Some(hwnd) = hwnd(window) else { return };
    // SAFETY: a live window this thread owns; no region or rectangle.
    unsafe { RedrawWindow(hwnd, std::ptr::null(), std::ptr::null_mut(), RDW_NOINTERNALPAINT) };
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
    /// Fading out (`OverlayWindow::fade`): every click goes through.
    fading: Cell<bool>,
}
impl Input {
    fn track(&self) -> bool { self.input.get() == OverlayInput::Regions || self.follow.get() }
    fn local(&self, mut point: POINT) -> (f32, f32) {
        unsafe { windows_sys::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut point); }
        (point.x as f32 / self.scale.get(), point.y as f32 / self.scale.get())
    }
    fn hit(&self, point: (f32, f32)) -> bool {
        !self.fading.get() && (self.captured.get() || match self.input.get() {
            OverlayInput::All => true,
            OverlayInput::Regions => self.regions.borrow().iter().any(|r| crate::overlay::options::contains(*r, point.0, point.1)),
            _ => false,
        })
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
    watch: Option<crate::platform::win32::pointer::MouseWatch>,
    wake: Option<crate::Waker>,
    input: Rc<Input>,
    observing: bool,
    exclusion: OverlaySupport,
}
impl OverlayWindow {
    pub fn new(window: &winit::window::Window, interactive: bool, options: &OverlayOptions, wake: Option<crate::Waker>) -> Result<Self, String> {
        let hwnd = hwnd(window).ok_or("the overlay window has no Win32 handle")?;
        let mode = options.input.resolved(interactive);
        overlay_styles(hwnd, mode)?;
        let exclusion = if options.exclude_from_capture {
            if unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } != 0 { OverlaySupport::Requested } else { OverlaySupport::Failed }
        } else { OverlaySupport::Unknown };
        let input = Rc::new(Input { hwnd, regions: RefCell::default(), scale: Cell::new(window.scale_factor() as f32), clicks: RefCell::default(), captured: Cell::new(false), visible: Cell::new(false), input: Cell::new(mode), follow: Cell::new(options.follow_pointer), level: options.level, fading: Cell::new(false) });
        if unsafe { SetWindowSubclass(hwnd, Some(subclass), 0x52554e45, Rc::as_ptr(&input) as usize) } == 0 {
            return Err(format!("SetWindowSubclass failed: {}", last_error()));
        }
        Ok(Self { input, observing: false, exclusion, watch: None, wake })
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
            if self.watch.is_none() { self.watch = Some(crate::platform::win32::pointer::MouseWatch::new(self.wake.clone())); }
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
    /// Puts the window on screen without activating it, above other topmost windows. Rune shows
    /// and hides overlays with Win32 calls alone: winit never sees either, and a winit
    /// visibility change would also reset these styles. Errs when Windows leaves it hidden.
    pub fn show(&mut self) -> Result<(), String> {
        let hwnd = self.input.hwnd;
        // Opaque again, after any fade-out.
        self.input.fading.set(false);
        overlay_styles(hwnd, self.input.input.get())?;
        // SAFETY: Win32 calls on a live window owned by this thread.
        let visible = unsafe {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            IsWindowVisible(hwnd) != 0
        };
        if !visible { return Err("ShowWindow left the overlay window hidden".into()); }
        let mut cloaked = 0u32;
        // SAFETY: DWMWA_CLOAKED writes one u32 into `cloaked`.
        let read = unsafe {
            windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(hwnd, windows_sys::Win32::Graphics::Dwm::DWMWA_CLOAKED as u32, (&raw mut cloaked).cast(), 4)
        };
        if read >= 0 && cloaked != 0 { return Err(format!("Windows cloaked the overlay window (DWMWA_CLOAKED {cloaked:#x})")); }
        self.input.visible.set(true);
        raise_levels();
        if self.observing { return Ok(()); }
        INPUTS.with(|inputs| inputs.borrow_mut().push(self.input.clone()));
        self.refresh_watch();
        self.observing = true;
        if let Some((x, y)) = pointer_position() { self.input.observe(POINT { x, y }); }
        Ok(())
    }
    /// Keeps the shown window up at `alpha` (255 opaque) while it fades out, taking no clicks.
    pub fn fade(&self, alpha: u8) {
        if !self.input.fading.replace(true) && let Some((x, y)) = pointer_position() {
            self.input.observe(POINT { x, y });
        }
        // SAFETY: a Win32 call on a live layered window owned by this thread.
        unsafe { SetLayeredWindowAttributes(self.input.hwnd, 0, alpha, LWA_ALPHA); }
    }
    pub fn hide(&mut self) {
        // SAFETY: a Win32 call on a live window owned by this thread.
        unsafe { ShowWindow(self.input.hwnd, SW_HIDE); }
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

/// A layered, topmost tool window that never activates; unless `mode` takes every click, every
/// click goes through (`Input::observe` lifts that over input regions). Windows composes a
/// layered window, its DirectComposition content included, only once it has layered
/// attributes: without them it stays invisible however often it is shown (#510 in Hex).
/// Opaque attributes leave the content's own alpha as the only transparency.
fn overlay_styles(hwnd: HWND, mode: OverlayInput) -> Result<(), String> {
    let through = if mode == OverlayInput::All { 0 } else { WS_EX_TRANSPARENT };
    // SAFETY: style changes on a live window owned by this thread.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let next = style | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | through;
        if next != style { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next as isize); }
        if SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA) == 0 {
            return Err(format!("SetLayeredWindowAttributes failed: {}", last_error()));
        }
    }
    Ok(())
}

fn last_error() -> String {
    // SAFETY: reads this thread's last Win32 error.
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    format!("Win32 error {code}")
}

/// Keeps shown overlays above one another by level.
fn raise_levels() {
    let inputs = INPUTS.with(|inputs| inputs.borrow().clone());
    for level in [OverlayLevel::Status, OverlayLevel::AboveStatus, OverlayLevel::AboveOverlays] {
        for input in &inputs {
            if input.visible.get() && input.level == level {
                // SAFETY: a Win32 call on a live window owned by this thread.
                unsafe { SetWindowPos(input.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
            }
        }
    }
}

/// `monitor`'s work area (without its taskbar and app bars) as top and bottom, in physical
/// desktop pixels.
pub(crate) fn work_area(monitor: &winit::monitor::MonitorHandle) -> Option<(i32, i32)> {
    use winit::platform::windows::MonitorHandleExtWindows;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};
    // SAFETY: MONITORINFO is plain data; all zeros is valid before cbSize is set.
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // SAFETY: GetMonitorInfoW fills `info`, whose cbSize says which struct it is.
    (unsafe { GetMonitorInfoW(monitor.hmonitor() as _, &mut info) } != 0).then_some((info.rcWork.top, info.rcWork.bottom))
}

/// Destroys this thread's windows at once, for a fast exit: each gets the default window procedure
/// first, so neither winit's nor a subclass runs while the process is ending.
pub(crate) fn destroy_own_windows() {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DefWindowProcW, DestroyWindow, EnumThreadWindows, GWLP_WNDPROC, SetWindowLongPtrW};
    unsafe extern "system" fn add(hwnd: HWND, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the Vec `destroy_own_windows` passes, alive for the enumeration.
        unsafe { (*(list as *mut Vec<HWND>)).push(hwnd) };
        TRUE
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: this thread's own top-level windows; each is destroyed by the thread that made it.
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), Some(add), (&raw mut windows) as LPARAM);
        for hwnd in windows {
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, DefWindowProcW as usize as isize);
            DestroyWindow(hwnd);
        }
    }
}