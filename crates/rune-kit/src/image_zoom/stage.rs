//! A pane's image with `PreviewControls` (the file viewer's `ImageStage` in `@opencode/plugin`
//! `artifact-view.tsx`): fitted with 24 points to spare, never grown past its pixels (an SVG
//! may grow); a click zooms to 100% about the pointer and another fits it; Ctrl + wheel (a
//! pinch) zooms about the pointer from a quarter of the fit (or less) to 8× (or 100%); a
//! zoomed image pans with a drag or the wheel; the controls step by 1.25. Every change lands at
//! once.

use std::cell::Cell;
use std::rc::Rc;

use rune::machine::Effects;
use rune::{Cursor, Drag, El, Icon, Phase, Tx, el, row};

use crate::{IconButton, IconButtonVariant, Kit};

/// How far a press moves before it pans rather than clicks.
const DRAG_THRESHOLD: f32 = 3.0;

/// The press on a zoomed image (`ImageStageGestureState`). `SuppressClick` follows a pan that
/// moved: the click that ends it neither fits nor zooms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum StageGesture {
    #[default]
    Idle,
    Panning {
        moved: bool,
        /// Where the pan's pointer pressed, then where it was on its last move.
        origin: [f32; 2],
    },
    SuppressClick,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StageEvent {
    PointerDown { zoomed: bool, at: [f32; 2] },
    PointerMoved { at: [f32; 2] },
    PointerUp,
    PointerCancel,
    /// Fit, a click, a pinch or a wheel zoom changed whether the image shows above fit.
    ZoomChanged { zoomed: bool },
    Click,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StageEffect {
    Pan([f32; 2]),
    /// The click that zooms to 100% or back to fit.
    Toggle,
}

pub fn stage_step(state: &mut StageGesture, event: StageEvent) -> Effects<StageEffect> {
    match (*state, event) {
        (_, StageEvent::PointerDown { zoomed, at }) => {
            *state = if zoomed { StageGesture::Panning { moved: false, origin: at } } else { StageGesture::Idle };
            Effects::none()
        }
        (StageGesture::Panning { moved, origin }, StageEvent::PointerMoved { at }) => {
            let delta = [at[0] - origin[0], at[1] - origin[1]];
            if delta == [0.0, 0.0] || (!moved && delta[0].hypot(delta[1]) < DRAG_THRESHOLD) { return Effects::none(); }
            *state = StageGesture::Panning { moved: true, origin: at };
            Effects::one(StageEffect::Pan(delta))
        }
        (StageGesture::Panning { moved, .. }, StageEvent::PointerUp) => {
            *state = if moved { StageGesture::SuppressClick } else { StageGesture::Idle };
            Effects::none()
        }
        (StageGesture::Panning { .. }, StageEvent::PointerCancel) => {
            *state = StageGesture::Idle;
            Effects::none()
        }
        (StageGesture::Panning { .. }, StageEvent::ZoomChanged { zoomed: false }) => {
            *state = StageGesture::Idle;
            Effects::none()
        }
        (StageGesture::SuppressClick, StageEvent::Click) => {
            *state = StageGesture::Idle;
            Effects::none()
        }
        (StageGesture::Idle, StageEvent::Click) => Effects::one(StageEffect::Toggle),
        _ => Effects::none(),
    }
}

/// One stage's zoom: 1 is the fit, `pan` points from the middle.
#[derive(Debug)]
pub struct StageZoom {
    zoom: f32,
    pan: [f32; 2],
    gesture: StageGesture,
    /// What the last view measured.
    geom: Cell<Geom>,
}

/// The fit, the stage's size and the image's natural size.
#[derive(Clone, Copy, Debug)]
struct Geom {
    fit: f32,
    size: [f32; 2],
    natural: [f32; 2],
}

impl Default for StageZoom {
    fn default() -> StageZoom {
        StageZoom { zoom: 1.0, pan: [0.0, 0.0], gesture: StageGesture::Idle, geom: Cell::new(Geom { fit: 1.0, size: [0.0; 2], natural: [1.0; 2] }) }
    }
}

impl StageZoom {
    pub fn zoomed(&self) -> bool { self.zoom > 1.0 }

    fn min_zoom(&self) -> f32 { 0.25f32.min(1.0 / self.geom.get().fit) }

    fn max_zoom(&self) -> f32 { 8.0f32.max(1.0 / self.geom.get().fit) }

    /// Keeps the fitted footprint covered, its letterboxing included.
    fn bound(&self, offset: f32, natural: f32) -> f32 {
        let limit = (natural * self.geom.get().fit * (self.zoom - 1.0) / 2.0).max(0.0);
        offset.clamp(-limit, limit)
    }

    fn set_pan(&mut self, pan: [f32; 2]) {
        let n = self.geom.get().natural;
        self.pan = [self.bound(pan[0], n[0]), self.bound(pan[1], n[1])];
    }

    fn set_zoom(&mut self, zoom: f32, pan: [f32; 2], tx: &mut Tx) {
        self.zoom = zoom;
        self.set_pan(pan);
        self.run(StageEvent::ZoomChanged { zoomed: zoom > 1.0 }, tx);
        tx.instant();
    }

    pub fn reset(&mut self, tx: &mut Tx) { self.set_zoom(1.0, [0.0, 0.0], tx) }

    /// Zoom to `zoom` about `at` (points from the stage's top-left).
    fn zoom_at(&mut self, zoom: f32, at: [f32; 2], tx: &mut Tx) {
        let next = zoom.clamp(self.min_zoom(), self.max_zoom());
        let ratio = next / self.zoom;
        let size = self.geom.get().size;
        let a = [at[0] - size[0] / 2.0, at[1] - size[1] / 2.0];
        self.set_zoom(next, [a[0] - (a[0] - self.pan[0]) * ratio, a[1] - (a[1] - self.pan[1]) * ratio], tx);
    }

    pub fn zoom_by(&mut self, factor: f32, tx: &mut Tx) {
        let size = self.geom.get().size;
        self.zoom_at(self.zoom * factor, [size[0] / 2.0, size[1] / 2.0], tx);
    }

    fn run(&mut self, event: StageEvent, tx: &mut Tx) -> Option<[f32; 2]> {
        let mut toggle = None;
        for effect in stage_step(&mut self.gesture, event) {
            match effect {
                StageEffect::Pan([dx, dy]) => {
                    self.set_pan([self.pan[0] + dx, self.pan[1] + dy]);
                    tx.instant();
                }
                StageEffect::Toggle => toggle = Some([0.0, 0.0]),
            }
        }
        toggle
    }

    /// The image's rect in the stage, `[x, y, w, h]`.
    fn rect(&self) -> [f32; 4] {
        let g = self.geom.get();
        let (w, h) = (g.natural[0] * g.fit * self.zoom, g.natural[1] * g.fit * self.zoom);
        let pan = [self.bound(self.pan[0], g.natural[0]), self.bound(self.pan[1], g.natural[1])];
        [g.size[0] / 2.0 + pan[0] - w / 2.0, g.size[1] / 2.0 + pan[1] - h / 2.0, w, h]
    }
}

type Lens<S> = fn(&mut S) -> &mut StageZoom;

/// The stage: `image` (the app's element drawing the picture) fitted in a pane of `size`
/// points, with the controls at its bottom right.
pub struct Stage<S> {
    lens: Lens<S>,
    natural: [f32; 2],
    image: El,
    grows: bool,
    icons: Option<[&'static Icon; 3]>,
    labels: [String; 4],
}

impl<S: 'static> Stage<S> {
    pub fn new(lens: Lens<S>, natural: [f32; 2], image: El) -> Stage<S> {
        Stage {
            lens,
            natural,
            image,
            grows: false,
            icons: None,
            labels: ["Zoom in".into(), "Zoom out".into(), "Fit".into(), "Preview controls".into()],
        }
    }

    /// An SVG fits by growing too.
    pub fn grows(mut self, on: bool) -> Self {
        self.grows = on;
        self
    }

    /// The zoom in, zoom out and fit glyphs.
    pub fn icons(mut self, zoom_in: &'static Icon, zoom_out: &'static Icon, fit: &'static Icon) -> Self {
        self.icons = Some([zoom_in, zoom_out, fit]);
        self
    }

    /// The controls' tooltips and names, and the group's name.
    pub fn labels(mut self, zoom_in: impl Into<String>, zoom_out: impl Into<String>, fit: impl Into<String>, group: impl Into<String>) -> Self {
        self.labels = [zoom_in.into(), zoom_out.into(), fit.into(), group.into()];
        self
    }

    pub fn build(self, zoom: &StageZoom, k: &Kit, size: [f32; 2]) -> El {
        let [nw, nh] = self.natural.map(|v| v.max(1.0));
        let fit = (if self.grows { f32::INFINITY } else { 1.0 })
            .min((size[0] - 48.0).max(1.0) / nw)
            .min((size[1] - 48.0).max(1.0) / nh);
        zoom.geom.set(Geom { fit, size, natural: [nw, nh] });
        let lens = self.lens;
        let controls = self.controls(zoom, k);
        let [x, y, w, h] = zoom.rect();
        let cursor = match (zoom.zoomed(), zoom.gesture) {
            (true, StageGesture::Panning { .. }) => Cursor::Grabbing,
            (true, _) => Cursor::Grab,
            _ => Cursor::Default,
        };
        let image = self.image
            .absolute(x, y)
            .size(w, h)
            .cursor(cursor)
            .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
                let z = lens(s);
                let [x, y, _, _] = z.rect();
                let at = [x + d.x, y + d.y];
                match d.phase {
                    Phase::Down => { z.run(StageEvent::PointerDown { zoomed: z.zoomed(), at }, tx); }
                    Phase::Move => { z.run(StageEvent::PointerMoved { at }, tx); }
                    Phase::Up if d.cancelled() => { z.run(StageEvent::PointerCancel, tx); }
                    Phase::Up => {
                        z.run(StageEvent::PointerUp, tx);
                        if z.run(StageEvent::Click, tx).is_some() {
                            if z.zoom != 1.0 { z.reset(tx) } else { z.zoom_at(1.0 / z.geom.get().fit, at, tx) }
                        }
                    }
                }
            });
        let mut stage = el()
            .key("artifact-stage")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(k.c.layer_01)
            .clip_soft(0.0)
            .on_wheel(move |s: &mut S, e: rune::WheelEvent, tx: &mut Tx| {
                let z = lens(s);
                let pinch = e.mods.ctrl || e.mods.cmd;
                if !pinch && z.zoom <= 1.0 { return false; }
                if pinch {
                    // A browser's delta runs the other way.
                    z.zoom_at(z.zoom * (e.dy / 100.0).exp(), [e.x, e.y], tx);
                } else {
                    z.set_pan([z.pan[0] + e.dx, z.pan[1] + e.dy]);
                    tx.instant();
                }
                true
            })
            .child(image);
        if cfg!(target_os = "macos") {
            stage = stage.on_magnify(move |s: &mut S, e: rune::MagnifyEvent, tx: &mut Tx| {
                let z = lens(s);
                z.zoom_at(z.zoom * (1.0 + e.delta).max(0.01), [e.x, e.y], tx);
                true
            });
        }
        stage.child(controls)
    }

    /// `PreviewControls`: zoom in, zoom out and fit, in a raised box at the bottom right.
    fn controls(&self, zoom: &StageZoom, k: &Kit) -> El {
        let lens = self.lens;
        let [zin, zout, fit, group] = self.labels.clone();
        let glyphs = self.icons.unwrap_or([k.glyph.dash; 3]);
        let divider = || el().size(0.5, 20.0).mx(4.0).fill(k.c.border_base).a11y_hidden();
        let button = |glyph: &'static Icon, label: String, disabled: bool, f: Rc<dyn Fn(&mut StageZoom, &mut Tx)>| {
            IconButton::new(glyph)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(crate::ControlSize::Large)
                .help(label)
                .disabled(disabled)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| f(lens(s), tx))
                .build(k)
        };
        let mut bar = row().key("preview-controls").role("group").label(group).padding(4.0).center_y().radius(8.0)
            .fill(k.c.bg_base).border(0.5, k.c.border_base);
        bar = crate::shadows(k, bar, &k.elevation.raised);
        bar.pin(None, None, Some(12.0), Some(12.0))
            .child(button(glyphs[0], zin, zoom.zoom >= zoom.max_zoom(), Rc::new(|z, tx| z.zoom_by(1.25, tx))))
            .child(divider())
            .child(button(glyphs[1], zout, zoom.zoom <= zoom.min_zoom(), Rc::new(|z, tx| z.zoom_by(1.0 / 1.25, tx))))
            .child(divider())
            .child(button(glyphs[2], fit, false, Rc::new(|z, tx| z.reset(tx))))
    }
}
