//! The view's UI zoom (`Cx::zoom`), applied as a browser applies page zoom: as part of the device
//! scale. The view lays out in the window's points over the zoom, at the window's scale times
//! the zoom, so every border, icon, glyph and pixel snap lands on the device grid as it does
//! unzoomed, and everything that moves (flights, morphs, presences, springs) moves in the one
//! zoomed space the layout is in. Chromium does the same (zoom for DSF): a 1px border at 125%
//! on a 1.5x display is one device pixel, not 1.875 of them.
//!
//! The host speaks window points: [`Runtime::set_viewport`] takes the window's size and scale,
//! and what the runtime hands back for the window (the IME area, menus at a point, a fitted
//! height, chrome insets) is in window points. Pointer, wheel and touch input arrive in view
//! points: the host converts device pixels with [`Runtime::input_scale`].

use super::*;

/// The zoom and the window it applies to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Zoom {
    /// The view's zoom, 1 unzoomed.
    pub(crate) k: f32,
    /// The window's viewport in its own points, and its scale (device pixels per point).
    pub(crate) window: (f32, f32),
    pub(crate) window_scale: f32,
}

impl Default for Zoom {
    fn default() -> Self {
        Self { k: 1.0, window: (0.0, 0.0), window_scale: 1.0 }
    }
}

impl Zoom {
    /// The view's viewport and scale for the window's.
    pub(crate) fn view(&self) -> ((f32, f32), f32) {
        ((self.window.0 / self.k, self.window.1 / self.k), self.window_scale * self.k)
    }
}

/// A zoom the view may ask for: finite, and within what a page zoom allows.
pub(crate) fn clamp(k: f32) -> f32 {
    if k.is_finite() { k.clamp(0.1, 10.0) } else { 1.0 }
}

impl<S: 'static> Runtime<S> {
    /// The view's UI zoom ([`Cx::zoom`]), 1 unzoomed.
    pub fn zoom(&self) -> f32 {
        self.zoom.k
    }

    /// The window's viewport in its own points, as the host last set it.
    pub fn window_viewport(&self) -> (f32, f32) {
        self.zoom.window
    }

    /// The window's scale (device pixels per window point), as the host last set it.
    pub fn window_scale(&self) -> f32 {
        self.zoom.window_scale
    }

    /// Device pixels per view point: what a host divides a pointer's or touch's pixels by to
    /// hand them to the runtime. The window's scale times the zoom.
    pub fn input_scale(&self) -> f32 {
        self.env.scale
    }

    /// A rect in view points as window points, for what the host places by the window: the IME
    /// area, a menu, a drag image.
    pub fn to_window(&self, r: [f32; 4]) -> [f32; 4] {
        r.map(|v| v * self.zoom.k)
    }

    /// The zoom the last view asked for: the viewport and scale follow it. False when it was
    /// already the zoom.
    pub(super) fn apply_zoom(&mut self, k: f32) -> bool {
        let k = clamp(k);
        if k == self.zoom.k { return false; }
        self.zoom.k = k;
        let (viewport, scale) = self.zoom.view();
        self.change_viewport(viewport, scale);
        // The window's controls and the caption buttons it draws are in window points.
        self.dirty = true;
        true
    }
}
