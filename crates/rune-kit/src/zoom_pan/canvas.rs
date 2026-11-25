//! [`Canvas`]: the pane that draws content in a [`ZoomPan`] view.

use rune::{Cursor, Drag, El, KeyEvent, Tx, el};

use super::{Geom, ZoomPan};

type Lens<S> = std::rc::Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut ZoomPan>;

enum Body {
    El(El),
    Scaled(Box<dyn FnOnce(f32) -> El>),
}

/// A pane of the size the app measured, clipping `content` (laid out at `content_size` points)
/// drawn at the [`ZoomPan`]'s view, with drag, wheel, pinch, double-click and keys wired.
///
/// ```
/// # use rune::{El, el};
/// # use rune_kit::{Canvas, ZoomPan};
/// # struct App { zoom: ZoomPan }
/// # fn tree(_detail: bool) -> El { el() }
/// # let s = App { zoom: ZoomPan::new() };
/// # let pane = [320.0, 480.0];
/// # let _ =
/// Canvas::scaled(|s: &mut App| &mut s.zoom, [784.0, 600.0], |scale| tree(scale >= 0.5))
///     .min_scale(0.35)
///     .padding(12.0)
///     .label("Chain of command")
///     .build(&s.zoom, pane)
/// # ;
/// ```
pub struct Canvas<S> {
    lens: Lens<S>,
    content: [f32; 2],
    body: Body,
    key: String,
    label: Option<String>,
    padding: f32,
    min_scale: f32,
    max_scale: f32,
    grows: bool,
}

impl<S: 'static> Canvas<S> {
    /// `content` laid out at `content_size` points.
    pub fn new(lens: impl for<'a> Fn(&'a mut S) -> &'a mut ZoomPan + 'static, content_size: [f32; 2], content: El) -> Canvas<S> {
        Canvas::with_body(std::rc::Rc::new(lens), content_size, Body::El(content))
    }

    /// Content built for the scale it draws at in this build, so it can drop detail (labels,
    /// edges' arrowheads) that would be unreadable when zoomed out.
    pub fn scaled(lens: impl for<'a> Fn(&'a mut S) -> &'a mut ZoomPan + 'static, content_size: [f32; 2], content: impl FnOnce(f32) -> El + 'static) -> Canvas<S> {
        Canvas::with_body(std::rc::Rc::new(lens), content_size, Body::Scaled(Box::new(content)))
    }

    fn with_body(lens: Lens<S>, content_size: [f32; 2], body: Body) -> Canvas<S> {
        let g = Geom::default();
        Canvas {
            lens,
            content: content_size,
            body,
            key: "zoom-pan-canvas".into(),
            label: None,
            padding: g.padding,
            min_scale: g.min_scale,
            max_scale: g.max_scale,
            grows: g.grows,
        }
    }

    /// The pane's key, which a press focuses so its keys work. Give each canvas in a window its own.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// The pane's accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Points a fitted content keeps from the pane's edges, and a panned one can show past its
    /// own. 0 by default.
    pub fn padding(mut self, points: f32) -> Self {
        self.padding = points.max(0.0);
        self
    }

    /// The smallest scale, fit included: below it the content no longer fits and pans instead.
    /// 0.25 by default.
    pub fn min_scale(mut self, scale: f32) -> Self {
        if scale.is_finite() && scale > 0.0 { self.min_scale = scale; }
        self
    }

    /// The largest scale. 8 by default.
    pub fn max_scale(mut self, scale: f32) -> Self {
        if scale.is_finite() && scale > 0.0 { self.max_scale = scale; }
        self
    }

    /// Whether a fit may draw content smaller than the pane larger than its own size. Off by
    /// default, so small content keeps its size.
    pub fn grows(mut self, on: bool) -> Self {
        self.grows = on;
        self
    }

    /// The pane, `size` points (as the app measured it, e.g. with `Cx::layout_size`). It fills
    /// its parent's width and grows in a column.
    pub fn build(self, zoom: &ZoomPan, size: [f32; 2]) -> El {
        zoom.measure(Geom {
            content: [self.content[0].max(1.0), self.content[1].max(1.0)],
            viewport: size,
            padding: self.padding,
            min_scale: self.min_scale,
            max_scale: self.max_scale,
            grows: self.grows,
        });
        let scale = zoom.scale();
        let [x, y, _, _] = zoom.content_rect();
        let content = match self.body {
            Body::El(e) => e,
            Body::Scaled(f) => f(scale),
        };
        let content = content.absolute(x, y).size(self.content[0], self.content[1]).origin(0.0, 0.0).scale(scale);
        let cursor = match (zoom.panning(), zoom.overflows()) {
            (true, _) => Cursor::Grabbing,
            (false, true) => Cursor::Grab,
            _ => Cursor::Default,
        };
        let mut pane = el()
            .key(self.key.clone())
            .role("group")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .clip_soft(0.0)
            .focusable(true)
            .cursor(cursor);
        if let Some(label) = self.label { pane = pane.label(label); }
        bind(pane, self.lens, self.key).child(content)
    }
}

fn bind<S: 'static>(pane: El, lens: Lens<S>, key: String) -> El {
    let (drag_lens, wheel_lens, key_lens, magnify_lens) = (lens.clone(), lens.clone(), lens.clone(), lens);
    let mut pane = pane
        .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            if d.phase == rune::Phase::Down { tx.focus(key.clone(), false); }
            drag_lens(s).drag(d, tx)
        })
        .on_wheel(move |s: &mut S, ev, tx| wheel_lens(s).wheel(ev, tx))
        .on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| key_lens(s).key(ev, tx));
    if cfg!(target_os = "macos") {
        pane = pane.on_magnify(move |s: &mut S, ev, tx| magnify_lens(s).magnify(ev, tx));
    }
    pane
}
