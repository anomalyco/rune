//! Zooming and panning any content (a tree, a diagram, a chart) in a pane: [`ZoomPan`] holds
//! the view and [`Canvas`] draws the content in it with the gestures wired.
//!
//! The content lays out at its own size and draws scaled with [`El::scale`](rune::El::scale)
//! about its top-left. Until a hand touches it the view fits the content to the pane, and fits
//! again when the pane resizes; once the user pans or zooms, a resize keeps their view. A drag on
//! the background pans, Ctrl or ⌘ + wheel (or a pinch) zooms about the pointer, a plain wheel
//! pans content that overflows, a double-click fits, and with the pane focused `+`/`=` and `-`
//! step by √2 about the middle while `0` fits (bare or with ⌘/Ctrl).
//!
//! Hit testing inside the content follows the drawn rects, so a button in a zoomed tree takes
//! its press as it is drawn. Pointer coordinates the app gets in the pane's points (from a
//! handler on the pane, or from [`Drag`](rune::Drag) on it) map into the content's own layout
//! points with [`ZoomPan::to_content`], and back with [`ZoomPan::to_viewport`].
//!
//! Every change lands at once (`Tx::instant`).

mod canvas;
mod gesture;

use std::cell::Cell;

use rune::{KeyEvent, Tx};

use crate::image_zoom::math::{STEP, clamp_axis};
use gesture::{Effect, Event, Gesture};

pub use canvas::Canvas;

/// What the pane holds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum View {
    /// Untouched: the content fits the pane, whatever its size.
    #[default]
    Fit,
    /// Placed by a hand: `pan` offsets the content's middle from the pane's middle, in points.
    Held { scale: f32, pan: [f32; 2] },
}

/// The content, the pane and the limits the last build measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geom {
    pub content: [f32; 2],
    pub viewport: [f32; 2],
    pub padding: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    /// The fit may draw the content larger than its own size.
    pub grows: bool,
}

impl Default for Geom {
    fn default() -> Geom {
        Geom { content: [1.0; 2], viewport: [0.0; 2], padding: 0.0, min_scale: 0.25, max_scale: 8.0, grows: false }
    }
}

impl Geom {
    /// The pane less its padding, where a fitted content stays.
    fn room(&self) -> [f32; 2] {
        [(self.viewport[0] - 2.0 * self.padding).max(1.0), (self.viewport[1] - 2.0 * self.padding).max(1.0)]
    }

    fn fit_scale(&self) -> f32 {
        let room = self.room();
        let fit = (room[0] / self.content[0].max(1.0)).min(room[1] / self.content[1].max(1.0));
        let fit = if self.grows { fit } else { fit.min(1.0) };
        fit.clamp(self.min_scale, self.max_scale.max(self.min_scale))
    }

    fn clamp_scale(&self, scale: f32) -> f32 {
        scale.clamp(self.min_scale, self.max_scale.max(self.min_scale))
    }

    /// Keeps the scaled content covering the room on each axis it overflows, and centred on each
    /// axis it fits.
    fn clamp_pan(&self, scale: f32, pan: [f32; 2]) -> [f32; 2] {
        let room = self.room();
        [clamp_axis(pan[0], self.content[0] * scale, room[0]), clamp_axis(pan[1], self.content[1] * scale, room[1])]
    }
}

/// One pane's zoom and pan. The app owns it and hands [`Canvas`] a lens to it.
#[derive(Debug, Default)]
pub struct ZoomPan {
    view: View,
    gesture: Gesture,
    /// What the last build measured.
    geom: Cell<Geom>,
}

impl ZoomPan {
    pub fn new() -> ZoomPan { ZoomPan::default() }

    /// The scale the content draws at: 1 is its own size. Read it in the view to drop detail
    /// that would be unreadable (12 pt labels are 3 pt at 0.25). [`Canvas::scaled`] passes the
    /// scale of the build it is in, so a resize of a fitted pane is seen in the same frame.
    pub fn scale(&self) -> f32 { self.resolved().0 }

    /// True until the user pans or zooms (and again after a fit): the view refits on resize.
    pub fn fitted(&self) -> bool { self.view == View::Fit }

    /// True while a drag pans the content.
    pub fn panning(&self) -> bool { self.gesture.panning() }

    /// Where the content draws in the pane, `[x, y, w, h]` in the pane's points.
    pub fn content_rect(&self) -> [f32; 4] {
        let g = self.geom.get();
        let (scale, pan) = self.resolved();
        let (w, h) = (g.content[0] * scale, g.content[1] * scale);
        [g.viewport[0] / 2.0 + pan[0] - w / 2.0, g.viewport[1] / 2.0 + pan[1] - h / 2.0, w, h]
    }

    /// A point in the pane's points (from its top-left) in the content's own layout points.
    pub fn to_content(&self, at: [f32; 2]) -> [f32; 2] {
        let [x, y, _, _] = self.content_rect();
        let scale = self.scale();
        [(at[0] - x) / scale, (at[1] - y) / scale]
    }

    /// A point in the content's layout points in the pane's points.
    pub fn to_viewport(&self, at: [f32; 2]) -> [f32; 2] {
        let [x, y, _, _] = self.content_rect();
        let scale = self.scale();
        [x + at[0] * scale, y + at[1] * scale]
    }

    /// Fit the content to the pane, and keep fitting it on resize until a hand touches it again.
    pub fn fit(&mut self, tx: &mut Tx) {
        self.view = View::Fit;
        tx.instant();
    }

    /// Zoom to `scale` (clamped to the limits) keeping the content point under `at` (pane points
    /// from its top-left) where it is.
    pub fn zoom_at(&mut self, scale: f32, at: [f32; 2], tx: &mut Tx) {
        let g = self.geom.get();
        let (from, pan) = self.resolved();
        let next = g.clamp_scale(scale);
        let ratio = next / from;
        let a = [at[0] - g.viewport[0] / 2.0, at[1] - g.viewport[1] / 2.0];
        self.hold(next, [a[0] - (a[0] - pan[0]) * ratio, a[1] - (a[1] - pan[1]) * ratio], tx);
    }

    /// Zoom by `factor` about the pane's middle.
    pub fn zoom_by(&mut self, factor: f32, tx: &mut Tx) {
        let v = self.geom.get().viewport;
        self.zoom_at(self.scale() * factor, [v[0] / 2.0, v[1] / 2.0], tx);
    }

    /// Move the content by `delta` points, as far as it overflows the pane. True when it moved.
    pub fn pan_by(&mut self, delta: [f32; 2], tx: &mut Tx) -> bool {
        let (scale, pan) = self.resolved();
        self.hold(scale, [pan[0] + delta[0], pan[1] + delta[1]], tx)
    }

    /// The view's scale and pan with the last build's limits applied.
    fn resolved(&self) -> (f32, [f32; 2]) {
        let g = self.geom.get();
        match self.view {
            View::Fit => (g.fit_scale(), [0.0, 0.0]),
            View::Held { scale, pan } => {
                let scale = g.clamp_scale(scale);
                (scale, g.clamp_pan(scale, pan))
            }
        }
    }

    /// Holds the view a hand chose. True when it changed what the pane shows.
    fn hold(&mut self, scale: f32, pan: [f32; 2], tx: &mut Tx) -> bool {
        let g = self.geom.get();
        let next = (scale, g.clamp_pan(scale, pan));
        if next == self.resolved() { return false; }
        self.view = View::Held { scale: next.0, pan: next.1 };
        tx.instant();
        true
    }

    pub(crate) fn measure(&self, geom: Geom) { self.geom.set(geom); }

    /// True when the content overflows the pane on either axis, so a drag can pan it.
    fn overflows(&self) -> bool {
        let g = self.geom.get();
        let (scale, _) = self.resolved();
        let room = g.room();
        g.content[0] * scale > room[0] + 0.5 || g.content[1] * scale > room[1] + 0.5
    }

    fn run(&mut self, event: Event, tx: &mut Tx) {
        for effect in gesture::step(&mut self.gesture, event) {
            match effect {
                Effect::Pan(delta) => { self.pan_by(delta, tx); }
                Effect::Fit => self.fit(tx),
                Effect::Restore(view) => {
                    self.view = view;
                    tx.instant();
                }
            }
        }
    }

    fn drag(&mut self, d: &rune::Drag, tx: &mut Tx) {
        match d.phase {
            rune::Phase::Down => self.run(Event::Down { view: self.view }, tx),
            rune::Phase::Move => self.run(Event::Moved { travel: [d.dx, d.dy] }, tx),
            rune::Phase::Up if d.cancelled() => self.run(Event::Cancel, tx),
            rune::Phase::Up => self.run(Event::Up { clicks: d.clicks }, tx),
        }
    }

    /// Ctrl or ⌘ + wheel zooms about the pointer; a plain wheel pans content that overflows.
    /// False leaves the wheel to a scroll view around the pane.
    fn wheel(&mut self, e: rune::WheelEvent, tx: &mut Tx) -> bool {
        if e.mods.ctrl || e.mods.cmd {
            // A browser's wheel runs the other way.
            self.zoom_at(self.scale() * crate::image_zoom::math::wheel_zoom_factor(-e.dy), [e.x, e.y], tx);
            return true;
        }
        self.pan_by([e.dx, e.dy], tx)
    }

    fn magnify(&mut self, e: rune::MagnifyEvent, tx: &mut Tx) -> bool {
        self.zoom_at(self.scale() * (1.0 + e.delta).max(0.01), [e.x, e.y], tx);
        true
    }

    /// `+`/`=` and `-`/`_` step by √2 about the middle, `0` fits; bare or with ⌘/Ctrl.
    fn key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let m = ev.mods;
        if m.alt { return false; }
        let c = match &ev.key {
            rune::Key::Text(t) if !m.cmd && !m.ctrl && !m.logo => t.chars().next().filter(|_| t.chars().count() == 1),
            rune::Key::Command(c) => Some(*c),
            _ => None,
        };
        match c {
            Some('=' | '+') => self.zoom_by(STEP, tx),
            Some('-' | '_') => self.zoom_by(1.0 / STEP, tx),
            Some('0') => self.fit(tx),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests;
