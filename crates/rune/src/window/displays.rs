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

fn read(source: &impl Monitors) -> Vec<Display> {
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
