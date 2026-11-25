//! `App::on_window_geometry`: the native window's applied content size, scale and whether it
//! is on an output, reported when the window is created and again at idle when they change.
//!
//! Resize and scale events don't cover every change: a Wayland window entering its first
//! output at scale 1 gets neither, yet it went from no output to one. Sampling at idle catches
//! that. An app that doesn't set the hook samples nothing.

#[cfg(not(target_arch = "wasm32"))]
use winit::event_loop::ActiveEventLoop;
#[cfg(not(target_arch = "wasm32"))]
use winit::window::Window;

#[cfg(not(target_arch = "wasm32"))]
use super::host::Host;
use crate::el::Tx;

/// What the native window reports about its content area, as `App::on_window_geometry`
/// delivers it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WindowGeometry {
    /// The content area's size in physical pixels, as the window system applied it (zero while
    /// it has none, such as when minimised on Windows).
    pub physical_size: (u32, u32),
    /// The native scale factor: physical pixels per logical point.
    pub scale_factor: f64,
    /// Whether the window is on an output (monitor). A Wayland window has none until the
    /// compositor maps it onto one.
    pub has_output: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl WindowGeometry {
    /// Reads the native window now. On X11 the size is a blocking round trip to the server.
    pub(in crate::window) fn sample(window: &Window) -> Self {
        let size = window.inner_size();
        WindowGeometry {
            physical_size: (size.width, size.height),
            scale_factor: window.scale_factor(),
            has_output: window.current_monitor().is_some(),
        }
    }
}

/// `App::on_window_geometry`'s hook.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type OnWindowGeometry<S> = fn(&mut S, WindowGeometry, &mut Tx) -> bool;

impl<S: 'static> crate::App<S> {
    /// Hear the native window's applied content size, scale and output presence. Runs when
    /// each native window is created (again after a Wayland close or a GPU release), before
    /// its first focus and frame, then at idle whenever one of them changed, including a
    /// window entering an output at scale 1 without a resize. A sample equal to the last runs
    /// nothing and asks for no frame. Return whether state the view reads changed.
    ///
    /// The window's own resize and scale changes still reach the view first; this reports
    /// what the window system applied, not what was asked for (`WindowHandle::resize`), and
    /// says nothing about whether a frame reached the screen (`on_frame_presented`). Window
    /// requests the creation sample makes apply once creation finishes. A closing window
    /// sends no last sample.
    ///
    /// Setting it queries the size, scale and output on every idle turn of the loop,
    /// animating ones included; on X11 the size query waits on the server. Native windows
    /// only: headless tooling and the web don't call it.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Canvas", (640.0, 480.0), || None, |_, _| el()))
    ///     .on_window_geometry(|last, geometry, _| {
    ///         *last = Some(geometry);
    ///         false
    ///     })
    ///     .run();
    /// ```
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    pub fn on_window_geometry(mut self, f: fn(&mut S, WindowGeometry, &mut Tx) -> bool) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        { self.on_window_geometry = Some(f); }
        #[cfg(target_arch = "wasm32")]
        let _ = f;
        self
    }
}

/// The last sample `App::on_window_geometry` heard, so an unchanged one runs nothing.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(in crate::window) struct GeometryWatch {
    last: Option<WindowGeometry>,
}

#[cfg(not(target_arch = "wasm32"))]
impl GeometryWatch {
    /// The sample to report: always at creation (`initial`), otherwise only when it differs
    /// from the last one reported.
    pub(in crate::window) fn observe(&mut self, sample: WindowGeometry, initial: bool) -> Option<WindowGeometry> {
        if !initial && self.last == Some(sample) { return None; }
        self.last = Some(sample);
        Some(sample)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Host {
    /// At native creation: runs the hook now, before the window's first focus and frame.
    pub(in crate::window) fn creation_geometry(&mut self) {
        if !self.app.watches_geometry() { return; }
        let Some(w) = &self.win else { return };
        let Some(sample) = self.geometry_watch.observe(WindowGeometry::sample(&w.window), true) else { return };
        self.app.run_on_geometry(sample);
    }

    /// At idle: runs the hook as an update when the sample changed, so its frame and window
    /// requests go as an update's do. True when that finished the window (a close it asked for).
    pub(in crate::window) fn idle_geometry(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.started || self.life.finished() || !self.app.watches_geometry() { return false; }
        let Some(w) = &self.win else { return false };
        let Some(sample) = self.geometry_watch.observe(WindowGeometry::sample(&w.window), false) else { return false };
        self.app.queue_on_geometry(sample);
        self.drain(event_loop, false);
        self.publish();
        self.life.finished()
    }
}

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;
