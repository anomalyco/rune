//! The low-level mouse hook never runs on the rendering/event thread. Its message loop
//! records positions and wakes interested overlays; window styles are changed by the owner.

#![allow(unsafe_code)]

use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, Ordering}};
use windows_sys::Win32::{Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM}, UI::WindowsAndMessaging::*};
use crate::input::pointer_observer::Observer;

type Observers = Arc<Mutex<Vec<Arc<Observer>>>>;
#[derive(Default)]
struct Service {
    observers: Observers,
    running: Arc<AtomicBool>,
    thread: Option<(u32, std::thread::JoinHandle<()>)>,
}
fn service() -> &'static Mutex<Service> { static SERVICE: OnceLock<Mutex<Service>> = OnceLock::new(); SERVICE.get_or_init(Mutex::default) }
thread_local! { static OBSERVERS: std::cell::RefCell<Option<Observers>> = const { std::cell::RefCell::new(None) }; }

unsafe extern "system" fn hook(code: i32, kind: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 && kind as u32 == WM_MOUSEMOVE {
        // SAFETY: for `WH_MOUSE_LL` with `code >= 0`, Windows passes a valid `MSLLHOOKSTRUCT` pointer in `data`.
        let point = unsafe { (*(data as *const MSLLHOOKSTRUCT)).pt };
        let observers = OBSERVERS.with(|s| s.borrow().as_ref().map(|s| s.lock().unwrap().clone())).unwrap_or_default();
        for observer in observers { observer.observe((point.x, point.y)); }
    }
    // SAFETY: forwards the hook parameters unchanged to the next hook in the `WH_MOUSE_LL` chain.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, kind, data) }
}

pub(crate) struct MouseWatch { observer: Arc<Observer>, running: Arc<AtomicBool> }
impl MouseWatch {
    pub fn new(wake: Option<crate::Waker>) -> Self {
        let observer = Arc::new(Observer::new(wake));
        let mut s = service().lock().unwrap();
        s.observers.lock().unwrap().push(observer.clone());
        if s.thread.is_none() {
            let (observers, running) = (s.observers.clone(), s.running.clone());
            let (ready, started) = std::sync::mpsc::sync_channel(1);
            // SAFETY: initializes this thread's message queue, installs a `WH_MOUSE_LL` hook with
            // this module's handle, pumps messages into a valid local `MSG`, and unhooks before exit.
            let thread = std::thread::spawn(move || unsafe {
                OBSERVERS.with(|s| *s.borrow_mut() = Some(observers));
                let mut message = std::mem::zeroed();
                PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
                let id = windows_sys::Win32::System::Threading::GetCurrentThreadId();
                let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()), 0);
                running.store(!hook.is_null(), Ordering::Release);
                ready.send(id).ok();
                if !hook.is_null() {
                    while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    UnhookWindowsHookEx(hook);
                }
                running.store(false, Ordering::Release);
                OBSERVERS.with(|s| s.borrow_mut().take());
            });
            s.thread = Some((started.recv().unwrap_or(0), thread));
        }
        Self { observer, running: s.running.clone() }
    }

    pub fn supported(&self) -> bool { self.running.load(Ordering::Acquire) }
    pub fn take(&self) -> Option<(i32, i32)> { self.observer.take() }
    pub fn observe(&self, point: (i32, i32)) { self.observer.observe(point); }

    /// Main-thread snapshot. The hook reads no HWND, application state, or renderer locks.
    pub fn geometry(&self, hwnd: windows_sys::Win32::Foundation::HWND, follow: bool) {
        use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, EnumDisplayMonitors};
        // SAFETY: `RECT` is a plain Win32 C struct for which all-zeros is a valid initial value.
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: `hwnd` is a live window on the main thread; `rect` and `origin` are valid local out-parameters.
        if unsafe { GetClientRect(hwnd, &mut rect) } == 0 || unsafe { ClientToScreen(hwnd, &mut origin) } == 0 { return; }
        let canvas = [origin.x, origin.y, origin.x + rect.right, origin.y + rect.bottom];
        if !self.observer.needs_geometry(canvas, follow) { return; }
        let mut displays: Vec<[i32; 4]> = Vec::new();
        unsafe extern "system" fn monitor(_: windows_sys::Win32::Graphics::Gdi::HMONITOR, _: windows_sys::Win32::Graphics::Gdi::HDC, rect: *mut RECT, data: LPARAM) -> i32 {
            // SAFETY: `EnumDisplayMonitors` passes a valid `RECT` pointer and the `&mut Vec<[i32; 4]>`
            // passed as `data`, which stays alive for the synchronous enumeration.
            let (r, displays) = unsafe { (&*rect, &mut *(data as *mut Vec<[i32; 4]>)) };
            displays.push([r.left, r.top, r.right, r.bottom]);
            1
        }
        // SAFETY: `EnumDisplayMonitors` synchronously invokes `monitor` on this thread while `displays` is exclusively borrowed and alive.
        if follow { unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(monitor), (&mut displays as *mut Vec<[i32; 4]>) as LPARAM); } }
        self.observer.configure(canvas, displays, follow);
    }
}
impl Drop for MouseWatch {
    fn drop(&mut self) {
        let thread = {
            let mut s = service().lock().unwrap();
            s.observers.lock().unwrap().retain(|o| !Arc::ptr_eq(o, &self.observer));
            if s.observers.lock().unwrap().is_empty() { s.thread.take() } else { None }
        };
        if let Some((id, thread)) = thread {
            // SAFETY: `id` is the non-zero Win32 thread ID of the still-unjoined hook thread whose message queue was created before `id` was sent.
            if id != 0 { unsafe { PostThreadMessageW(id, WM_QUIT, 0, 0); } }
            thread.join().ok();
        }
    }
}
