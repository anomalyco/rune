//! Whether a window can be seen, as Chromium's `NativeWindowOcclusionTrackerWin` works it out:
//! a thread of its own hears windows move, show, hide, minimise, change virtual desktop or come
//! to the front (`SetWinEventHook`) and, a frame later, walks every window from the top of the
//! z-order down, taking the opaque ones out of what's left of the screens. A tracked window none
//! of whose rect is left, that's minimised, or that's on another virtual desktop is covered: no
//! one can see it, so it stops drawing (`App::on_visibility` hears it) until it shows again.
//!
//! Only on the desktop the user sees. A window on another one (a capture harness's hidden
//! desktop, which several apps share) is never covered, so it keeps drawing for whoever reads
//! its pixels.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Dwm::{
    DWM_CLOAKED_SHELL, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    COMPLEXREGION, CombineRgn, CreateRectRgn, DeleteObject, EnumDisplayMonitors, GetWindowRgn, HDC, HMONITOR, HRGN,
    MONITORINFO, MonitorFromWindow, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, RGN_DIFF, RGN_OR, RectInRegion,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_READOBJECTS, GetThreadDesktop, GetUserObjectInformationW, HDESK, OpenInputDesktop, UOI_NAME,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_OBJECT_REORDER, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MOVESIZEEND, EnumWindows, GA_ROOT, GWL_EXSTYLE, GWL_STYLE, GetAncestor, GetClassNameW,
    GetLayeredWindowAttributes, GetMessageW, GetWindowLongW, GetWindowPlacement, IsIconic, IsWindowVisible, KillTimer,
    LWA_ALPHA, LWA_COLORKEY, MSG, OBJID_WINDOW, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SW_MAXIMIZE, SetTimer,
    WINDOWPLACEMENT, WINEVENT_OUTOFCONTEXT, WM_APP, WM_TIMER, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    WS_POPUP,
};

/// Called on the tracker's thread with whether the window is covered now.
pub(crate) type Report = Arc<dyn Fn(bool) + Send + Sync>;

/// A window the tracker watches until this drops.
pub(crate) struct Tracked {
    hwnd: usize,
}

impl Tracked {
    /// Watches `window`, telling `report` each time it becomes covered or uncovered. None when
    /// the tracker's thread couldn't start; the window then never counts as covered.
    pub(crate) fn new(window: &winit::window::Window, report: Report) -> Option<Self> {
        let hwnd = super::hwnd(window)? as usize;
        let tracker = tracker()?;
        tracker.windows.lock().unwrap().push(Watched { hwnd, report, sight: Sight::Unknown });
        tracker.wake();
        Some(Tracked { hwnd })
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let Some(tracker) = tracker() else { return };
        tracker.windows.lock().unwrap().retain(|w| w.hwnd != self.hwnd);
    }
}

/// What the tracker last told a window's owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sight {
    /// Nothing yet, or the window was off screen (hidden), where its owner starts it uncovered
    /// again when it shows.
    Unknown,
    Seen,
    Covered,
}

/// What a pass found for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Found {
    /// Hidden (not `IsWindowVisible`): its owner knows, and a show starts it uncovered.
    Hidden,
    Seen,
    Covered,
}

impl Sight {
    /// The report a pass's finding asks for, if any: a change, or the first word on a window
    /// that's on screen.
    fn step(self, found: Found) -> (Sight, Option<bool>) {
        match (self, found) {
            (_, Found::Hidden) => (Sight::Unknown, None),
            (Sight::Seen, Found::Seen) | (Sight::Covered, Found::Covered) => (self, None),
            (_, Found::Seen) => (Sight::Seen, Some(false)),
            (_, Found::Covered) => (Sight::Covered, Some(true)),
        }
    }
}

struct Watched {
    hwnd: usize,
    report: Report,
    sight: Sight,
}

struct Tracker {
    thread: u32,
    windows: Mutex<Vec<Watched>>,
}

/// Posted to the tracker's thread: the tracked windows changed, look now.
const WM_TRACKED: u32 = WM_APP + 1;
/// A frame: how long the tracker lets a burst of window events settle before it looks, as
/// Chromium's does.
const SETTLE_MS: u32 = 16;

impl Tracker {
    fn wake(&self) {
        // SAFETY: posts to a thread whose queue exists (`start` waits for it); no pointers.
        unsafe { PostThreadMessageW(self.thread, WM_TRACKED, 0, 0) };
    }
}

fn tracker() -> Option<&'static Tracker> {
    static TRACKER: OnceLock<Option<Tracker>> = OnceLock::new();
    TRACKER.get_or_init(start).as_ref()
}

/// Starts the tracker's thread, once its message queue exists so posts reach it.
fn start() -> Option<Tracker> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("rune-occlusion".into())
        .spawn(move || {
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            // SAFETY: makes this thread's message queue; `msg` is a valid out pointer.
            unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
            // SAFETY: this thread's id.
            let _ = tx.send(unsafe { GetCurrentThreadId() });
            run();
        })
        .ok()?;
    let thread = rx.recv().ok()?;
    Some(Tracker { thread, windows: Mutex::new(Vec::new()) })
}

/// When the next pass runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    Idle,
    /// A burst of window events is settling; the timer runs the pass.
    Settling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassEvent {
    /// A window event that may change what covers what.
    Moved,
    /// A tracked window came or went: look at once.
    Now,
    /// The settle timer fired.
    Timer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassEffect {
    None,
    StartTimer,
    /// Run the pass, stopping the timer if it runs.
    Run { stop_timer: bool },
}

impl Pass {
    fn step(self, event: PassEvent) -> (Pass, PassEffect) {
        match (self, event) {
            (Pass::Idle, PassEvent::Moved) => (Pass::Settling, PassEffect::StartTimer),
            (Pass::Settling, PassEvent::Moved) => (Pass::Settling, PassEffect::None),
            (Pass::Idle, PassEvent::Now) => (Pass::Idle, PassEffect::Run { stop_timer: false }),
            (Pass::Settling, PassEvent::Now | PassEvent::Timer) => (Pass::Idle, PassEffect::Run { stop_timer: true }),
            (Pass::Idle, PassEvent::Timer) => (Pass::Idle, PassEffect::None),
        }
    }
}

thread_local! {
    static PASS: std::cell::Cell<Pass> = const { std::cell::Cell::new(Pass::Idle) };
}

/// The settle timer's id on the tracker's thread (`SetTimer` with no window picks it).
static TIMER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn pass_event(event: PassEvent) {
    let (next, effect) = PASS.with(|p| p.get()).step(event);
    PASS.with(|p| p.set(next));
    match effect {
        PassEffect::None => {}
        PassEffect::StartTimer => {
            // SAFETY: a thread timer (no window); WM_TIMER comes to this thread's queue.
            let id = unsafe { SetTimer(std::ptr::null_mut(), 0, SETTLE_MS, None) };
            TIMER.store(id, std::sync::atomic::Ordering::Relaxed);
        }
        PassEffect::Run { stop_timer } => {
            if stop_timer {
                // SAFETY: the timer this thread set, or 0, which fails harmlessly.
                unsafe { KillTimer(std::ptr::null_mut(), TIMER.swap(0, std::sync::atomic::Ordering::Relaxed)) };
            }
            run_pass();
        }
    }
}

fn run() {
    let hook = |min: u32, max: u32| {
        // SAFETY: an out-of-context hook on every process's events, called on this thread while
        // it waits for messages; `on_event` is a plain function.
        unsafe { SetWinEventHook(min, max, std::ptr::null_mut(), Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT) }
    };
    let _hooks = [
        hook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        hook(EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MINIMIZEEND),
        hook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_REORDER),
        hook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
        hook(EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: `msg` is a valid out pointer; out-of-context hooks run inside GetMessageW.
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        match msg.message {
            WM_TRACKED => pass_event(PassEvent::Now),
            WM_TIMER => pass_event(PassEvent::Timer),
            _ => {}
        }
    }
}

/// A window event from any process on this desktop. Only whole top-level windows count. Even a
/// tracked window's own events settle first: a restore reports its end while the window can
/// still read as minimised.
unsafe extern "system" fn on_event(_hook: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, child: i32, _thread: u32, _time: u32) {
    if object != OBJID_WINDOW || child != CHILDID_SELF as i32 || hwnd.is_null() { return; }
    // A window going away can't say whether it was top-level.
    // SAFETY: GetAncestor tolerates any handle.
    if event != EVENT_OBJECT_DESTROY && unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd { return; }
    pass_event(PassEvent::Moved);
}

/// Looks at every tracked window and tells the owners of those that changed.
fn run_pass() {
    let Some(tracker) = tracker() else { return };
    let tracked: Vec<usize> = tracker.windows.lock().unwrap().iter().map(|w| w.hwnd).collect();
    if tracked.is_empty() { return; }
    let found = if on_input_desktop() { look(&tracked) } else { tracked.iter().map(|&h| (h, seen_or_hidden(h))).collect() };
    let reports: Vec<(Report, bool)> = {
        let mut windows = tracker.windows.lock().unwrap();
        windows.iter_mut().filter_map(|w| {
            let (_, f) = found.iter().find(|(h, _)| *h == w.hwnd)?;
            let (sight, report) = w.sight.step(*f);
            w.sight = sight;
            report.map(|covered| (w.report.clone(), covered))
        }).collect()
    };
    // Outside the lock: a report may track or untrack windows.
    for (report, covered) in reports { report(covered); }
}

fn seen_or_hidden(hwnd: usize) -> Found {
    // SAFETY: IsWindowVisible tolerates any handle.
    if unsafe { IsWindowVisible(hwnd as HWND) } == 0 { Found::Hidden } else { Found::Seen }
}

/// Whether this thread's desktop is the one the user sees. False when it can't tell.
fn on_input_desktop() -> bool {
    // SAFETY: desktop handles this function opens it closes; the thread's own isn't closed.
    unsafe {
        let input = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if input.is_null() { return false; }
        let same = desktop_name(input).is_some_and(|name| desktop_name(GetThreadDesktop(GetCurrentThreadId())) == Some(name));
        CloseDesktop(input);
        same
    }
}

fn desktop_name(desktop: HDESK) -> Option<Vec<u16>> {
    if desktop.is_null() { return None; }
    let mut name = [0u16; 256];
    let mut needed = 0u32;
    // SAFETY: `name` holds 512 bytes, as passed.
    let ok = unsafe { GetUserObjectInformationW(desktop, UOI_NAME, name.as_mut_ptr().cast(), std::mem::size_of_val(&name) as u32, &mut needed) };
    if ok == 0 { return None; }
    Some(name.iter().copied().take_while(|&c| c != 0).collect())
}

/// One pass down the z-order, as Chromium's `WindowOcclusionCalculator` does: what's left of
/// the screens shrinks by each opaque window, and a tracked window shows if any of what's left
/// is in its rect when the walk reaches it.
fn look(tracked: &[usize]) -> Vec<(usize, Found)> {
    struct Walk<'a> {
        tracked: &'a [usize],
        left: HRGN,
        found: Vec<(usize, Found)>,
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
        // SAFETY: `data` is the `Walk` `look` passes, alive for the enumeration.
        let walk = unsafe { &mut *(data as *mut Walk) };
        let tracked = walk.tracked.contains(&(hwnd as usize));
        if tracked {
            walk.found.push((hwnd as usize, tracked_window(hwnd, walk.left)));
        }
        if let Some(r) = opaque_rect(hwnd, tracked) {
            // SAFETY: regions this pass owns.
            unsafe {
                let rgn = CreateRectRgn(r.left, r.top, r.right, r.bottom);
                CombineRgn(walk.left, walk.left, rgn, RGN_DIFF);
                DeleteObject(rgn);
            }
        }
        // Done once every tracked window is found.
        if walk.found.len() == walk.tracked.len() { 0 } else { TRUE }
    }
    // SAFETY: the region is this pass's, deleted below; the callbacks get valid pointers.
    unsafe {
        let left = CreateRectRgn(0, 0, 0, 0);
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(monitor), left as LPARAM);
        let mut walk = Walk { tracked, left, found: Vec::new() };
        EnumWindows(Some(window), (&raw mut walk) as LPARAM);
        DeleteObject(left);
        // A tracked window the walk didn't reach (destroyed meanwhile) is off screen.
        tracked.iter().map(|&h| (h, walk.found.iter().find(|(f, _)| *f == h).map_or(Found::Hidden, |(_, f)| *f))).collect()
    }
}

/// What a tracked window is, with `left` what other windows leave of the screens above it.
fn tracked_window(hwnd: HWND, left: HRGN) -> Found {
    // SAFETY: plain queries on a live top-level window.
    unsafe {
        if IsWindowVisible(hwnd) == 0 { return Found::Hidden; }
        if IsIconic(hwnd) != 0 || cloaked(hwnd).is_some_and(|c| c & DWM_CLOAKED_SHELL != 0) { return Found::Covered; }
        // Rune's own first-frame cloak (DWM_CLOAKED_APP) waits for the frame this window draws.
        let Some(r) = bounds(hwnd) else { return Found::Seen };
        if RectInRegion(left, &r) != 0 { Found::Seen } else { Found::Covered }
    }
}

/// The rect an opaque window covers, or None for one that may let what's under it show, as
/// Chromium's `IsWindowVisibleAndFullyOpaque` decides. `tracked` windows are ours, and cover
/// what's under them whatever their styles.
fn opaque_rect(hwnd: HWND, tracked: bool) -> Option<RECT> {
    // SAFETY: plain queries on a window `EnumWindows` handed over.
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 { return None; }
        if !tracked {
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
        }
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

