//! The displays an app can read while it runs: `rune::displays()`.
//!
//! winit reads monitors from the event loop or from any of its windows. Each native window
//! Rune makes is tracked here (weakly), so `displays()` asks the first one still open. With no
//! window open (a tray-only app, a headless run) it gives the displays last read.
use std::cell::RefCell;
use std::sync::{Arc, Weak};

use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use winit::window::Window;

use crate::windows::Display;

#[path = "displays/watch.rs"]
mod watch;
pub use watch::DisplayEvent;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use watch::DisplayWatch;

thread_local! {
    /// The native windows made on this thread, to read monitors from.
    static WINDOWS: RefCell<Vec<Weak<Window>>> = const { RefCell::new(Vec::new()) };
    /// The displays last read, for calls while no window is open.
    static LAST: RefCell<Vec<Display>> = const { RefCell::new(Vec::new()) };
}

/// Forgets the previous run's windows and displays, at the start of a run.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn reset() {
    WINDOWS.with(|windows| windows.borrow_mut().clear());
    LAST.with(|last| last.borrow_mut().clear());
}

/// Lets `displays()` read monitors from `window` while it is open.
pub(crate) fn track(window: &Arc<Window>) {
    WINDOWS.with(|windows| {
        let mut windows = windows.borrow_mut();
        windows.retain(|w| w.strong_count() > 0);
        windows.push(Arc::downgrade(window));
    });
}

/// Keeps `displays` as the displays last read (the headless run's virtual desktop).
pub(crate) fn remember(displays: &[Display]) {
    LAST.with(|last| *last.borrow_mut() = displays.to_vec());
}

/// The displays connected now, with the fields a `run_windows` factory gets: native id, name,
/// position and size in desktop pixels, scale, and which is primary. Call it on the UI thread
/// (a handler, an update, `on_ready`): each call asks the system again through an open window,
/// so a monitor plugged in after launch can be found and given a window (`open_window` with
/// `WindowOptions::overlay`). It is a query, not a notification; don't call it every frame.
/// With no window open (a headless run, an app with only a tray item) it returns the displays
/// last read: the factory's list, or the headless run's virtual desktop. On another thread it
/// is empty.
pub fn displays() -> Vec<Display> {
    let window = WINDOWS.with(|windows| windows.borrow().iter().find_map(Weak::upgrade));
    match window {
        Some(window) => collect(&*window),
        None => LAST.with(|last| last.borrow().clone()),
    }
}

/// Scale factor (device pixels per point) of the primary display, if the platform can report it.
///
/// Callable before `run()` and inside `AppSpec::init`, so an app that prepares resolution-dependent
/// content ahead of its first window does not have to guess `2.0`. Once Rune has read displays (in
/// an open window or a headless run, including `--scale` and scripted desktops), this returns the
/// primary display's scale from [`displays`]. Before a window exists, it queries the OS directly on
/// macOS (`NSScreen`), Windows (`GetDpiForMonitor` / `GetDpiForSystem`), and the web
/// (`window.devicePixelRatio`), and returns `None` where no pre-window query is available (such as
/// Linux before a display connection is open, or off the main thread on macOS).
///
/// A window can still open on another monitor or move between screens after launch, so keep the
/// normal re-plan path when `Env::scale` or `Tx::scale` differs.
pub fn primary_display_scale() -> Option<f32> {
    let known = displays();
    if let Some(scale) = primary_scale_from(&known) {
        return Some(scale);
    }
    platform_primary_display_scale().filter(|s| s.is_finite() && *s > 0.0)
}

fn primary_scale_from(list: &[Display]) -> Option<f32> {
    list.iter()
        .find(|d| d.primary)
        .or_else(|| list.first())
        .map(|d| d.scale as f32)
        .filter(|s| s.is_finite() && *s > 0.0)
}

#[cfg(target_os = "macos")]
fn platform_primary_display_scale() -> Option<f32> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    let mtm = MainThreadMarker::new()?;
    let screen = NSScreen::mainScreen(mtm).or_else(|| NSScreen::screens(mtm).iter().next())?;
    Some(screen.backingScaleFactor() as f32)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn platform_primary_display_scale() -> Option<f32> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, GetDpiForSystem,
        MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext,
    };
    // Query under Per-Monitor-V2 so calls before winit sets process DPI awareness still read the
    // true primary monitor DPI without mutating process-wide awareness.
    // SAFETY: `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` is a predefined handle; `prev` is
    // restored immediately after the query if non-null.
    let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let dpi = unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut x = 0u32;
        let mut y = 0u32;
        if !monitor.is_null() && GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) == 0 && x > 0 {
            x
        } else {
            GetDpiForSystem()
        }
    };
    if !prev.is_null() {
        // SAFETY: `prev` was returned by `SetThreadDpiAwarenessContext` on this thread.
        unsafe { SetThreadDpiAwarenessContext(prev); }
    }
    (dpi > 0).then(|| dpi as f32 / 96.0)
}

#[cfg(target_arch = "wasm32")]
fn platform_primary_display_scale() -> Option<f32> {
    web_sys::window().map(|w| w.device_pixel_ratio() as f32)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_arch = "wasm32")))]
fn platform_primary_display_scale() -> Option<f32> {
    None
}

/// What monitors are read from: the event loop at launch, a window afterwards.
pub(crate) trait Monitors: HasDisplayHandle {
    fn monitors(&self) -> Vec<MonitorHandle>;
    fn primary(&self) -> Option<MonitorHandle>;
    fn wayland(&self) -> bool {
        matches!(self.display_handle().map(|h| h.as_raw()), Ok(RawDisplayHandle::Wayland(_)))
    }
}

impl Monitors for ActiveEventLoop {
    fn monitors(&self) -> Vec<MonitorHandle> { self.available_monitors().collect() }
    fn primary(&self) -> Option<MonitorHandle> { self.primary_monitor() }
    #[cfg(not(target_arch = "wasm32"))]
    fn wayland(&self) -> bool { super::is_wayland(self) }
}

impl Monitors for Window {
    fn monitors(&self) -> Vec<MonitorHandle> { self.available_monitors().collect() }
    fn primary(&self) -> Option<MonitorHandle> { self.primary_monitor() }
}

/// Reads the displays from `source` and keeps them as the last read.
pub(crate) fn collect(source: &impl Monitors) -> Vec<Display> {
    let displays = read(source);
    remember(&displays);
    displays
}

/// Reads the displays from `source` using `watch`'s live committed Wayland output metadata,
/// skipping any output that winit knows about before the watch has received its atomic `done`
/// boundary (or after the watch saw `global_remove`).
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(super) fn read_from_wayland_watch(
    source: &impl Monitors,
    watch: &crate::platform::wayland::output::OutputWatch,
) -> Vec<Display> {
    let primary = source.primary();
    source
        .monitors()
        .into_iter()
        .filter_map(|m| {
            let id = crate::DisplayId::from_monitor(m.clone(), true);
            let fallback = ((m.position().x, m.position().y), (m.size().width, m.size().height), m.scale_factor());
            let (position, size, scale) = match id.native_id() {
                Some(crate::NativeDisplayId::Wayland(output_id)) => {
                    let raw = watch.committed(output_id)?;
                    let resolved = crate::platform::wayland::output::resolve(
                        Some(raw),
                        crate::platform::wayland::output::ResolvedOutput {
                            position: fallback.0,
                            size: fallback.1,
                            scale: fallback.2,
                        },
                    );
                    (resolved.position, resolved.size, resolved.scale)
                }
                _ => fallback,
            };
            Some(Display {
                id: Some(id),
                name: m.name(),
                position,
                size,
                scale,
                primary: primary.as_ref() == Some(&m),
            })
        })
        .collect()
}

pub(super) fn read(source: &impl Monitors) -> Vec<Display> {
    let wayland = source.wayland();
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let wayland_outputs = wayland
        .then(|| source.display_handle().ok())
        .flatten()
        .map(|display| crate::platform::wayland::output::query(display.as_raw()))
        .unwrap_or_default();
    let primary = source.primary();
    source.monitors().into_iter().map(|m| {
        let id = crate::DisplayId::from_monitor(m.clone(), wayland);
        let fallback = ((m.position().x, m.position().y), (m.size().width, m.size().height), m.scale_factor());
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let (position, size, scale) = match id.native_id() {
            Some(crate::NativeDisplayId::Wayland(output_id)) => {
                let resolved = crate::platform::wayland::output::resolve(
                    wayland_outputs.get(&output_id),
                    crate::platform::wayland::output::ResolvedOutput {
                        position: fallback.0,
                        size: fallback.1,
                        scale: fallback.2,
                    },
                );
                (resolved.position, resolved.size, resolved.scale)
            }
            _ => fallback,
        };
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        let (position, size, scale) = fallback;
        Display {
            id: Some(id), name: m.name(), position, size, scale, primary: primary.as_ref() == Some(&m),
        }
    }).collect()
}

#[cfg(test)]
#[path = "displays_tests.rs"]
mod tests;
