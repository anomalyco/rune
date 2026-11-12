//! The taskbar button's progress, overlay badge and flash (`crate::taskbar`), through the
//! shell's `ITaskbarList3`. The UI thread only compares and hands the state on: a thread of its
//! own (made on the first state that shows something, asleep on its channel in between) makes
//! the shell's object, which loads the shell's taskbar DLLs, draws the badges and makes the
//! calls, which go to Explorer.
//!
//! The badge is an icon drawn here with GDI at the taskbar's small-icon size for the window's
//! scale: a red disc with the count in white, anti-aliased, kept per count and size for the
//! run. A button Explorer makes again (it restarted) announces itself with
//! `TaskbarButtonCreated`; each window's subclass hears it and sends its state again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Shell::{ITaskbarList3, TBPF_ERROR, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_PAUSED, TBPFLAG, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::core::PCWSTR;

use crate::taskbar::{Taskbar, TaskbarProgress};

const SUBCLASS: usize = 0x5255_4e54;
/// The badge's disc: Windows 11's critical red.
const DISC: [u8; 3] = [0xC4, 0x2B, 0x1C];

/// One state for the worker to show: the window, what it showed, what it shows now.
struct Job {
    hwnd: isize,
    before: Taskbar,
    state: Taskbar,
}

thread_local! {
    /// What each window shows, to skip a call that changes nothing and to send again to a new button.
    static SHOWN: RefCell<HashMap<isize, Taskbar>> = RefCell::new(HashMap::new());
}

fn worker() -> &'static std::sync::Mutex<Sender<Job>> {
    static WORKER: OnceLock<std::sync::Mutex<Sender<Job>>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (to, from) = channel();
        if let Err(e) = std::thread::Builder::new().name("rune-taskbar".into()).spawn(move || run(from)) { eprintln!("rune: taskbar: {e}"); }
        std::sync::Mutex::new(to)
    })
}

/// Shows `state` on `window`'s taskbar button. Call it on the UI thread.
pub(crate) fn set(window: &winit::window::Window, state: Taskbar) {
    let Some(hwnd) = super::hwnd(window) else { return };
    let key = hwnd as isize;
    let before = SHOWN.with_borrow(|s| s.get(&key).cloned());
    if before.as_ref().unwrap_or(&Taskbar::default()) == &state { return; }
    if before.is_none() {
        // SAFETY: the subclass keeps no pointer and goes at WM_NCDESTROY.
        unsafe { windows_sys::Win32::UI::Shell::SetWindowSubclass(hwnd, Some(procedure), SUBCLASS, 0); }
    }
    SHOWN.with_borrow_mut(|s| s.insert(key, state.clone()));
    send(Job { hwnd: key, before: before.unwrap_or_default(), state });
}

fn send(job: Job) {
    let _ = worker().lock().unwrap_or_else(|e| e.into_inner()).send(job);
}

fn run(from: Receiver<Job>) {
    // SAFETY: this thread's own single-threaded apartment, for the shell's object.
    if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() { return; }
    let started = std::time::Instant::now();
    let Some(list) = list() else { return };
    eprintln!("rune: taskbar: the shell's object in {} µs off the UI thread", started.elapsed().as_micros());
    let mut icons = HashMap::new();
    while let Ok(job) = from.recv() {
        apply(&list, &mut icons, HWND(job.hwnd as *mut _), &job.before, &job.state);
    }
}
/// Flashes `window`'s taskbar button twice and leaves it lit until the window comes forward,
/// or stops it (`on` false). Nothing flashes while Focus Assist (Do not disturb) holds
/// notifications back, or for a window in front.
pub(crate) fn flash(window: &winit::window::Window, on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FLASHW_STOP, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GetForegroundWindow};
    let Some(hwnd) = super::hwnd(window) else { return };
    // SAFETY: plain calls on a live window this thread owns.
    unsafe {
        if on && (GetForegroundWindow() == hwnd || !accepts_notifications()) { return; }
        let info = FLASHWINFO { cbSize: size_of::<FLASHWINFO>() as u32, hwnd, dwFlags: if on { FLASHW_TRAY } else { FLASHW_STOP }, uCount: if on { 2 } else { 0 }, dwTimeout: 0 };
        FlashWindowEx(&info);
    }
}

/// Whether the person takes notifications now: not in Focus Assist, a full-screen app, a
/// presentation or the first hour after setup (`SHQueryUserNotificationState`).
fn accepts_notifications() -> bool {
    use windows_sys::Win32::UI::Shell::{QUNS_ACCEPTS_NOTIFICATIONS, SHQueryUserNotificationState};
    let mut state = 0;
    // SAFETY: fills `state`.
    unsafe { SHQueryUserNotificationState(&mut state) >= 0 && state == QUNS_ACCEPTS_NOTIFICATIONS }
}

fn list() -> Option<ITaskbarList3> {
    // SAFETY: the shell's taskbar object in this thread's apartment, initialised before use as
    // its contract asks.
    unsafe {
        let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()?;
        list.HrInit().ok()?;
        Some(list)
    }
}

fn apply(list: &ITaskbarList3, icons: &mut HashMap<(u32, i32), isize>, hwnd: HWND, before: &Taskbar, state: &Taskbar) {
    // SAFETY: shell calls about a window of this process (gone, they fail harmlessly).
    unsafe {
        if before.progress != state.progress {
            let flag: TBPFLAG = match state.progress {
                TaskbarProgress::None => TBPF_NOPROGRESS,
                TaskbarProgress::Busy => TBPF_INDETERMINATE,
                TaskbarProgress::Paused => TBPF_PAUSED,
                TaskbarProgress::Error => TBPF_ERROR,
            };
            // A paused or failed bar shows its value: full.
            if matches!(state.progress, TaskbarProgress::Paused | TaskbarProgress::Error) { let _ = list.SetProgressValue(hwnd, 1, 1); }
            let _ = list.SetProgressState(hwnd, flag);
        }
        if before.badge != state.badge {
            let (count, description) = &state.badge;
            let icon = if *count == 0 { HICON::default() } else { HICON(badge_icon(icons, hwnd, *count) as *mut _) };
            let description: Vec<u16> = description.encode_utf16().chain([0]).collect();
            let _ = list.SetOverlayIcon(hwnd, icon, PCWSTR(description.as_ptr()));
        }
    }
}

fn button_created() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: the name is NUL-terminated.
    *MESSAGE.get_or_init(|| unsafe { windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW("TaskbarButtonCreated\0".encode_utf16().collect::<Vec<u16>>().as_ptr()) })
}

unsafe extern "system" fn procedure(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32, w: usize, l: isize, _: usize, _: usize) -> isize {
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;
    if msg == WM_NCDESTROY {
        SHOWN.with(|s| s.try_borrow_mut().map(|mut s| s.remove(&(hwnd as isize))).ok());
        // SAFETY: removes this subclass from the window it was set on.
        unsafe { RemoveWindowSubclass(hwnd, Some(procedure), SUBCLASS) };
    } else if msg == button_created() {
        // A new button (Explorer restarted): it shows nothing until told again.
        if let Some(state) = SHOWN.with(|s| s.try_borrow().ok().and_then(|s| s.get(&(hwnd as isize)).cloned())) {
            send(Job { hwnd: hwnd as isize, before: Taskbar::default(), state });
        }
    }
    // SAFETY: passes every message on.
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}

/// The overlay icon for `count` at the taskbar's small-icon size for `hwnd`'s scale.
fn badge_icon(icons: &mut HashMap<(u32, i32), isize>, hwnd: HWND, count: u32) -> isize {
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON;
    // SAFETY: reads the window's scale and a system metric.
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(hwnd.0).max(96)) }.max(16);
    let count = count.min(10);
    *icons.entry((count, size)).or_insert_with(|| draw_badge(count, size))
}

/// A `size` square icon: the disc, and `count` (10 reads "9+") in white, its glyphs' coverage
/// from GDI's grayscale anti-aliasing.
fn draw_badge(count: u32, size: i32) -> isize {
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};
    let label: Vec<u16> = (if count >= 10 { "9+".to_owned() } else { count.to_string() }).encode_utf16().collect();
    // SAFETY: a plain C struct, all zeroes valid.
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: size, biHeight: -size, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, biSizeImage: 0, biXPelsPerMeter: 0, biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0 };
    let n = (size * size) as usize;
    // SAFETY: GDI objects made, used and deleted here; each DIB section holds `n` 32-bit pixels.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let mut text_bits = std::ptr::null_mut();
        let text = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut text_bits, std::ptr::null_mut(), 0);
        let old = SelectObject(dc, text);
        let height = -(f64::from(size) * if label.len() > 1 { 0.62 } else { 0.78 }).round() as i32;
        let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
        let font = CreateFontW(height, 0, 0, 0, FW_BOLD as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32, ANTIALIASED_QUALITY as u32, DEFAULT_PITCH as u32, face.as_ptr());
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
        let mut rect = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: size, bottom: size };
        let mut label = label;
        DrawTextW(dc, label.as_mut_ptr(), label.len() as i32, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        GdiFlush();
        let coverage: Vec<u8> = std::slice::from_raw_parts(text_bits.cast::<u8>(), n * 4).chunks_exact(4).map(|p| p[1]).collect();
        SelectObject(dc, old_font);
        DeleteObject(font);
        SelectObject(dc, old);
        DeleteObject(text);
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let out = std::slice::from_raw_parts_mut(bits.cast::<u8>(), n * 4);
        let r = f64::from(size) / 2.0;
        for (i, p) in out.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((i % size as usize) as f64 + 0.5 - r, (i / size as usize) as f64 + 0.5 - r);
            let a = (r - (x * x + y * y).sqrt()).clamp(0.0, 1.0);
            let t = f64::from(coverage[i]) / 255.0;
            let mix = |disc: u8| (f64::from(disc) * (1.0 - t) + 255.0 * t) * a;
            // Premultiplied BGRA.
            p.copy_from_slice(&[mix(DISC[2]).round() as u8, mix(DISC[1]).round() as u8, mix(DISC[0]).round() as u8, (a * 255.0).round() as u8]);
        }
        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
        let icon = CreateIconIndirect(&ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color });
        DeleteObject(mask);
        DeleteObject(color);
        DeleteDC(dc);
        icon as isize
    }
}
