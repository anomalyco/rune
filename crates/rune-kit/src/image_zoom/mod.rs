//! Zooming and panning an image (`@opencode/ui` `image-preview`): the full-window preview with
//! its zoom bar (−, the level, +, close), and [`Stage`], a pane's image with `PreviewControls`.
//!
//! In the preview a pinch or Ctrl + wheel zooms about the pointer, a wheel pans, a drag pans a
//! zoomed image, a double-click zooms to 100% (twice the fit for an image not much larger than
//! its fit) and back, and ⌘ + / ⌘ − step by √2 about the middle while ⌘ 0 and 0 fit it again.
//! Steps, the double-click and fit glide on a 0.3 s spring; the hand moves it at once. Zoomed
//! in, the first Escape fits it. A click on the backdrop closes it.
//!
//! The app owns an [`ImageZoom`] and hands [`Preview`] a lens to it. While a glide runs it asks
//! for frames: call [`ImageZoom::poll`] from the app's poll.
//!
//! ```ignore
//! Preview::new(|s: &mut App| &mut s.zoom, image.size(), el().pixels(image.pixels()))
//!     .on_close(|s: &mut App, tx| s.viewing = None)
//!     .build(&s.zoom, &kit, window)
//! ```

pub mod gesture;
mod glide;
pub mod math;
mod stage;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use rune::{Cursor, Drag, El, Icon, Key, KeyEvent, Phase, Tx, el, row, text};

use crate::{Button, IconButton, IconButtonVariant, Kit};
use gesture::{Effect, Event, Gesture, State};
use glide::Glide;
use math::{FIT_VIEW, STEP, ZoomBounds, ZoomView};

pub use stage::{Stage, StageZoom};

/// The preview's close button, which takes focus when it opens.
pub const CLOSE_KEY: &str = "image-preview-close";
/// The preview, which takes focus back when a press lands outside its controls.
pub const KEY: &str = "image-preview";

/// Points between the window's top and bottom edges and the body the image zooms in.
const BAND: f32 = 44.0;
/// The body's padding: a fitted image stays inside it, a zoomed one fades out across it.
const PAD: f32 = 12.0;

/// One preview's zoom: its gestures and the springs that hold the view.
#[derive(Default)]
pub struct ImageZoom {
    state: State,
    glide: Glide,
    /// What the last view measured: the fitted image and its viewport, and how far the fit
    /// shrinks the image.
    bounds: Cell<ZoomBounds>,
    fit_ratio: Cell<f32>,
    reduced_motion: Cell<bool>,
}

impl ImageZoom {
    pub fn new() -> ImageZoom { ImageZoom::default() }

    /// The view as it shows now.
    pub fn view(&self) -> ZoomView { self.glide.shown() }

    /// The zoom as a percentage of the image's natural size, as the zoom bar shows it.
    pub fn level(&self) -> u32 { (self.view().scale * self.fit_ratio.get().max(f32::EPSILON) * 100.0).round() as u32 }

    /// Whether the image shows above fit.
    pub fn zoomed(&self) -> bool { self.view().scale > 1.0 }

    /// Zoomed in (or on its way there), the first Escape zooms back out instead of closing.
    pub fn owns_escape(&self) -> bool { self.glide.goal().scale > 1.0 }

    /// Whether a pan holds the pointer.
    pub fn dragging(&self) -> bool { gesture::dragging(&self.state) }

    /// Whether a glide runs: `poll` does nothing otherwise, so a caller that polls every frame
    /// can skip it (and the write it takes).
    pub fn gliding(&self) -> bool { matches!(self.state, State::Gliding { .. }) }

    /// Steps a glide to the frame clock `now`. True while one runs (the view must run again);
    /// it asks `tx` for the next frame until it lands. The image lands each frame on its own
    /// (`El::instant_changes`), so `tx` stays animated: an app that closes the preview in the
    /// same frame still flies or fades it out.
    pub fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        let State::Gliding { gesture, last } = self.state else { return false };
        let moving = self.glide.step(now - last);
        self.state = State::Gliding { gesture, last: now };
        if moving {
            tx.wake_after(Duration::ZERO);
        } else {
            let zoomed = self.zoomed();
            self.run(Event::GlideSettled { zoomed }, tx);
        }
        true
    }

    /// Back to fit at once, as a new image opens.
    pub fn reset(&mut self) { *self = ImageZoom::default(); }

    /// ⌘ + and the + button: √2 times the destination, about the middle.
    pub fn zoom_in(&mut self, tx: &mut Tx) { self.step_by(STEP, tx) }

    /// ⌘ − and the − button.
    pub fn zoom_out(&mut self, tx: &mut Tx) { self.step_by(1.0 / STEP, tx) }

    /// ⌘ 0, 0, the level button and a double-click on a zoomed image.
    pub fn fit(&mut self, tx: &mut Tx) { self.ease(FIT_VIEW, tx) }

    fn step_by(&mut self, factor: f32, tx: &mut Tx) {
        let goal = self.glide.goal();
        self.ease(math::zoom_at(goal, goal.scale * factor, [0.0, 0.0], &self.bounds.get()), tx);
    }

    /// A double-click at `at` (viewport-centre points): 100% about the pointer, or back to fit.
    fn detail(&mut self, at: [f32; 2], tx: &mut Tx) {
        if self.glide.goal().scale > 1.0 { return self.fit(tx); }
        let to = math::zoom_at(self.glide.goal(), math::detail_scale(self.fit_ratio.get()), at, &self.bounds.get());
        self.ease(to, tx);
    }

    /// Pinch, wheel and drag follow the hand directly and take over from an eased zoom.
    fn follow(&mut self, to: ZoomView, tx: &mut Tx) {
        self.run(Event::Follow { zoomed: to.scale > 1.0 }, tx);
        self.glide = Glide::snap(to);
        tx.instant();
    }

    fn ease(&mut self, to: ZoomView, tx: &mut Tx) {
        if self.reduced_motion.get() { return self.follow(to, tx); }
        self.glide.set(to);
        self.run(Event::GlideStart { now: tx.clock() }, tx);
    }

    /// Steps the machine and carries out its effects; true when it asks to close.
    fn run(&mut self, event: Event, tx: &mut Tx) -> bool {
        let mut close = false;
        for effect in gesture::step(&mut self.state, event) {
            match effect {
                Effect::Pan([dx, dy]) => {
                    let (v, b) = (self.view(), self.bounds.get());
                    let to = ZoomView {
                        scale: v.scale,
                        x: math::clamp_axis(v.x + dx, b.width * v.scale, b.viewport_width),
                        y: math::clamp_axis(v.y + dy, b.height * v.scale, b.viewport_height),
                    };
                    self.follow(to, tx);
                }
                Effect::Frames => tx.wake_after(Duration::ZERO),
                Effect::Close => close = true,
            }
        }
        close
    }

    /// Measures the preview for a `window` of points showing an image of `natural` pixels
    /// (a pixel a point, shrunk to fit and never grown) and holds the view inside the new
    /// bounds. Returns the body's rect and the fitted size.
    fn measure(&self, window: [f32; 2], natural: [f32; 2]) -> ([f32; 4], [f32; 2]) {
        let side = (window[0] * 0.03).clamp(8.0, 36.0);
        let body = [side, BAND, (window[0] - 2.0 * side).max(1.0), (window[1] - 2.0 * BAND).max(1.0)];
        let [nw, nh] = natural.map(|v| v.max(1.0));
        let k = ((body[2] - 2.0 * PAD).max(1.0) / nw).min((body[3] - 2.0 * PAD).max(1.0) / nh).min(1.0);
        let fitted = [nw * k, nh * k];
        self.fit_ratio.set(k);
        self.bounds.set(ZoomBounds { width: fitted[0], height: fitted[1], viewport_width: body[2], viewport_height: body[3], max_scale: math::max_scale(k) });
        (body, fitted)
    }

    /// Where the image shows in the body, `[x, y, w, h]` from the body's top-left.
    fn image_rect(&self, body: [f32; 4], fitted: [f32; 2]) -> [f32; 4] {
        let v = math::clamp_view(self.view(), &self.bounds.get());
        let (w, h) = (fitted[0] * v.scale, fitted[1] * v.scale);
        [body[2] / 2.0 + v.x - w / 2.0, body[3] / 2.0 + v.y - h / 2.0, w, h]
    }

    fn hits_image(&self, at: [f32; 2]) -> bool {
        let b = self.bounds.get();
        let v = self.view();
        let [x, y] = [at[0] - b.viewport_width / 2.0 - v.x, at[1] - b.viewport_height / 2.0 - v.y];
        x.abs() <= b.width * v.scale / 2.0 && y.abs() <= b.height * v.scale / 2.0
    }

    fn centred(&self, at: [f32; 2]) -> [f32; 2] {
        let b = self.bounds.get();
        [at[0] - b.viewport_width / 2.0, at[1] - b.viewport_height / 2.0]
    }

    /// The preview's keys: ⌘ + (or =), ⌘ − (or _), ⌘ 0 and 0, and Escape, which fits a zoomed
    /// image first. True when Escape asks to close.
    fn key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> Option<bool> {
        let m = ev.mods;
        let bare = !m.cmd && !m.ctrl && !m.alt && !m.logo;
        match &ev.key {
            Key::Escape if self.owns_escape() => { self.fit(tx); Some(false) }
            Key::Escape => Some(true),
            Key::Text(t) if bare && t == "0" => { self.fit(tx); Some(false) }
            Key::Command(c) if !m.alt => match c {
                '=' | '+' => { self.zoom_in(tx); Some(false) }
                '-' | '_' => { self.zoom_out(tx); Some(false) }
                '0' => { self.fit(tx); Some(false) }
                _ => None,
            },
            _ => None,
        }
    }
}

type Lens<S> = fn(&mut S) -> &mut ImageZoom;
type OnClose<S> = Rc<dyn Fn(&mut S, &mut Tx)>;

/// The full-window preview over the app's scrim: the image fitted between the bands and the
/// zoom bar under it.
pub struct Preview<S> {
    lens: Lens<S>,
    natural: [f32; 2],
    image: El,
    close: OnClose<S>,
    icons: Option<[&'static Icon; 2]>,
    labels: [String; 4],
    format: fn(u32) -> String,
    controls: Option<rune::Transition>,
}

impl<S: 'static> Preview<S> {
    /// `image` (the app's element drawing the picture, with its key and shared-element id) is
    /// placed and sized here; `natural` is its size in pixels.
    pub fn new(lens: Lens<S>, natural: [f32; 2], image: El) -> Preview<S> {
        Preview {
            lens,
            natural,
            image,
            close: Rc::new(|_, _| {}),
            icons: None,
            labels: ["Zoom out".into(), "Zoom to fit".into(), "Zoom in".into(), "Close".into()],
            format: |n| format!("{n}%"),
            controls: None,
        }
    }

    /// How the zoom bar comes and goes (as the image flies, or with the scrim).
    pub fn controls_transition(mut self, t: rune::Transition) -> Self {
        self.controls = Some(t);
        self
    }

    pub fn on_close(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.close = Rc::new(f);
        self
    }

    /// The − and + glyphs (the close button takes the kit's).
    pub fn icons(mut self, minus: &'static Icon, plus: &'static Icon) -> Self {
        self.icons = Some([minus, plus]);
        self
    }

    /// Accessible names: zoom out, zoom to fit (also the level's tooltip), zoom in and close.
    pub fn labels(mut self, zoom_out: impl Into<String>, fit: impl Into<String>, zoom_in: impl Into<String>, close: impl Into<String>) -> Self {
        self.labels = [zoom_out.into(), fit.into(), zoom_in.into(), close.into()];
        self
    }

    /// How the level reads, from a whole percentage (`141%` unless set).
    pub fn format_level(mut self, f: fn(u32) -> String) -> Self {
        self.format = f;
        self
    }

    /// The preview over a `window` of points. `reduced_motion` makes steps land at once.
    pub fn build(self, zoom: &ImageZoom, k: &Kit, window: [f32; 2], reduced_motion: bool) -> El {
        zoom.reduced_motion.set(reduced_motion);
        let (body, fitted) = zoom.measure(window, self.natural);
        let [x, y, w, h] = zoom.image_rect(body, fitted);
        let lens = self.lens;
        let bar = self.bar(zoom, k);
        let scale = zoom.view().scale;
        // A glide's frames, a pinch and a pan move it to exactly where the view says.
        let image = self.image.absolute(x, y).size(w, h).radius(k.radius.md * scale).instant_changes();
        let close = self.close.clone();
        let mut body_el = el()
            .key("image-preview-body")
            .absolute(body[0], body[1])
            .size(body[2], body[3])
            .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| press(s, lens, d, true, &close, tx))
            .on_wheel(move |s: &mut S, e: rune::WheelEvent, tx: &mut Tx| {
                let z = lens(s);
                let v = z.view();
                let b = z.bounds.get();
                let to = if e.mods.ctrl || e.mods.cmd {
                    // Off macOS a pinch is a Ctrl + wheel; a browser's wheel runs the other way.
                    math::zoom_at(v, v.scale * math::wheel_zoom_factor(-e.dy), z.centred([e.x, e.y]), &b)
                } else {
                    math::pan_by(v, [e.dx, e.dy], &b)
                };
                z.follow(to, tx);
                true
            })
            .child(image);
        if cfg!(target_os = "macos") {
            body_el = body_el.on_magnify(move |s: &mut S, e: rune::MagnifyEvent, tx: &mut Tx| {
                let z = lens(s);
                let v = z.view();
                let factor = math::wheel_zoom_factor(-100.0 * (1.0 + e.delta).max(0.01).ln());
                let to = math::zoom_at(v, v.scale * factor, z.centred([e.x, e.y]), &z.bounds.get());
                z.follow(to, tx);
                true
            });
        }
        if zoom.zoomed() {
            // A zoomed image fades out across the padding instead of being cut at the edge; a
            // fitted one never reaches it.
            body_el = body_el
                .clip_fade_linear(PAD, PAD, PAD, PAD)
                .cursor(if zoom.dragging() { Cursor::Grabbing } else { Cursor::Grab });
        }
        let close = self.close.clone();
        let dismiss = self.close.clone();
        // The preview holds focus, so its keys reach it before the app's (which zoom the window
        // on the same shortcuts) wherever the pointer pressed.
        el().key(KEY)
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .focusable(true)
            .no_focus_ring()
            .modal_tx(false, move |s: &mut S, tx: &mut Tx| {
                if lens(s).owns_escape() { lens(s).fit(tx) } else { dismiss(s, tx) }
            })
            .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| press(s, lens, d, false, &close, tx))
            .on_wheel(|_: &mut S, _, _: &mut Tx| true)
            .on_key({
                let close = self.close.clone();
                move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| match lens(s).key(ev, tx) {
                    Some(true) => { close(s, tx); true }
                    Some(false) => true,
                    None => false,
                }
            })
            .child(body_el)
            .child(row().pin(Some(0.0), None, Some(0.0), Some(PAD)).justify_center().child(bar))
    }

    /// The zoom bar (`image-preview-controls`): −, the level (which fits it), +, a divider and
    /// close, on the first layer with the floating shadow.
    fn bar(&self, zoom: &ImageZoom, k: &Kit) -> El {
        let lens = self.lens;
        let [out, fit, into, close_label] = self.labels.clone();
        let level = (self.format)(zoom.level());
        let mut bar = row().key("image-preview-controls").gap(2.0).padding(2.0).center_y().radius(8.0).fill(k.c.layer_01);
        if let Some(t) = self.controls.clone() { bar = bar.transition(t); }
        bar = crate::shadows(k, bar, &k.elevation.floating);
        let glyphs = self.icons.unwrap_or([k.glyph.dash, k.glyph.dash]);
        let close = self.close.clone();
        let tnum = k.ty.ui.features("'tnum' 1");
        #[allow(deprecated)]
        let level = Button::new(fit.clone())
            .variant(crate::ButtonVariant::GhostMuted)
            .size(crate::ButtonSize::Small)
            .help(fit)
            .label_with(move |_, ink| text(level).font(tnum).color(ink))
            .on_press_tx(move |s: &mut S, tx: &mut Tx| lens(s).fit(tx))
            .build(k)
            .key("image-preview-zoom-level")
            // Three tabular digits, so the bar holds still as the level passes 100%.
            .min_w(58.0);
        bar.child(IconButton::new(glyphs[0]).variant(IconButtonVariant::Ghost).label(out)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| lens(s).zoom_out(tx)).build(k).key("image-preview-zoom-out"))
            .child(level)
            .child(IconButton::new(glyphs[1]).variant(IconButtonVariant::Ghost).label(into)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| lens(s).zoom_in(tx)).build(k).key("image-preview-zoom-in"))
            .child(el().key("image-preview-divider").size(1.0, 16.0).mx(2.0).fill(k.c.border_base))
            .child(IconButton::new(k.glyph.close).variant(IconButtonVariant::Ghost).label(close_label)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| close(s, tx)).build(k).key(CLOSE_KEY))
    }
}

/// A press on the body (`on_body`, where a zoomed image pans) or the band around it.
fn press<S>(s: &mut S, lens: Lens<S>, d: &mut Drag, on_body: bool, close: &OnClose<S>, tx: &mut Tx) {
    let z = lens(s);
    let at = [d.x, d.y];
    match d.phase {
        Phase::Down => {
            tx.focus(KEY, false);
            let on_backdrop = !on_body || !z.hits_image(at);
            z.run(Event::PointerDown { on_body, on_backdrop, zoomed: z.zoomed(), at }, tx);
        }
        Phase::Move => { z.run(Event::PointerMoved { at }, tx); }
        Phase::Up if d.cancelled() => { z.run(Event::PointerCancel, tx); }
        Phase::Up => {
            z.run(Event::PointerUp, tx);
            let on_backdrop = !on_body || !z.hits_image(at);
            if z.run(Event::Click { on_backdrop }, tx) { return close(s, tx); }
            if on_body && d.clicks == 2 { z.detail(z.centred(at), tx); }
        }
    }
    if matches!(z.state, State::Still(Gesture::Panning { .. })) { tx.instant(); }
}
