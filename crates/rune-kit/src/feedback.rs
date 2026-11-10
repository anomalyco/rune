//! Spinner, Loader, Progress and ProgressCircle (`@opencode/ui` components/spinner,
//! feedback/loader, components/progress, feedback/progress-circle).
//!
//! Renderer-owned loops cool through presence on removal; the spinner's run on the wall clock,
//! so every spinner on screen keeps one phase.

use rune::{Color, El, Icon, col, el, icon, row, spacer, text};

use crate::Kit;

mod radar;

/// Soft Radar, the activity spinner (`Spinner`, `spinner-radar.ts`): a 5×5 grid of 2-unit dots
/// on a 3-unit pitch in a 16-unit box, whose lit head sweeps clockwise around each square ring
/// every 1.8 s, leaving a fading trail, with the centre dot steady at 0.64. Unlit dots rest at
/// 0.2. Every spinner shares the wall clock's phase, so all of them on screen sweep together, and
/// under reduced motion each dot holds the pose a fifth of the way into its loop, as Desktop's
/// do. 18 points by default; Desktop's `ActivityRing` is 14 and its session progress indicator 16,
/// both in `icon_muted`.
pub struct Spinner {
    size: f32,
    color: Option<Color>,
}

impl Spinner {
    pub fn new() -> Spinner {
        Spinner { size: 18.0, color: None }
    }
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    /// The dots' ink (CSS `currentColor`); `std_text` by default.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn build(self, k: &Kit) -> El {
        let ink = self.color.unwrap_or(k.c.std_text);
        let u = self.size / 16.0;
        // Every dot draws antialiased at its exact sub-pixel place, the box's included, as
        // Desktop's dots do: each is an SVG on a composited layer, which the browser draws where
        // layout puts it without rounding to the device grid. Snapped, a 1.75 px dot on a
        // 2.625 px pitch (14 points at 1x) rounds to bars and gaps of 1 and 2 px.
        let mut e = el().role("progressbar").label("Loading").size(self.size, self.size).pixel_snap(false);
        for (i, d) in radar::dots().iter().enumerate() {
            let side = d.size * u;
            let dot = el().key(format!("d{i}")).absolute(d.x * u, d.y * u).size(side, side).radius(side * 0.24).fill(ink).a11y_hidden();
            e = e.child(match d.cycle {
                Some(c) => dot.cycle(c),
                None => dot.opacity(d.still),
            });
        }
        e
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

static RING: Icon = Icon {
    name: "kit-loader-ring",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2" fill="none"/>"#,
};
/// A third of the ring, from 12 o'clock clockwise, round caps.
static ARC: Icon = Icon {
    name: "kit-loader-arc",
    view: 16.0,
    body: r#"<path d="M8 2A6 6 0 0 1 13.258 10.89" stroke="currentColor" stroke-width="2" stroke-linecap="round" fill="none"/>"#,
};

/// The v2 loader, for loading that isn't AI activity (`Loader`): a 2-unit ring at 0.2 with a
/// third of it, round-capped from 12 o'clock, turning once every 900 ms; it stops at 12 o'clock
/// under reduced motion. 16 points by default, in `icon_muted`.
pub struct Loader {
    size: f32,
    color: Option<Color>,
}

impl Loader {
    pub fn new() -> Loader {
        Loader { size: 16.0, color: None }
    }
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    /// The ring's ink (CSS `currentColor`); `icon_muted` by default.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn build(self, k: &Kit) -> El {
        let ink = self.color.unwrap_or(k.c.icon_muted);
        el().role("progressbar")
            .label("Loading")
            .size(self.size, self.size)
            .child(icon(&RING, self.size).absolute(0.0, 0.0).color(ink).opacity(0.2))
            .child(icon(&ARC, self.size).absolute(0.0, 0.0).color(ink).looping(rune_motion::LoopSpec::rotation(0.9)))
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

/// A progress indicator's value for assistive technology: the fraction done, read as a percentage.
fn report(e: El, value: Option<f32>) -> El {
    match value {
        Some(v) => {
            let v = v.clamp(0.0, 1.0);
            e.range_value((f64::from(v) * 1e4).round() / 1e4, 0.0, 1.0).a11y_value(format!("{}%", (v * 100.0).round()))
        }
        None => e,
    }
}

/// A linear progress bar with an optional label and value label (`Progress`).
pub struct Progress {
    label: Option<String>,
    value: Option<f32>,
    show_value: bool,
}

impl Progress {
    /// `value` in 0..=1; `None` is indeterminate.
    pub fn new(value: Option<f32>) -> Progress {
        Progress { label: None, value, show_value: false }
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn show_value(mut self) -> Self {
        self.show_value = true;
        self
    }
    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let mut root = col().role("progressbar").w_full().gap(4.0);
        if self.label.is_some() || self.show_value {
            let mut header = row().key("header").gap(8.0).center_y();
            if let Some(l) = &self.label {
                root = root.label(l.clone());
                header = header.child(text(l.clone()).key("label").font(k.ty.text_small).color(c.std_text));
            }
            header = header.child(spacer());
            if let (true, Some(v)) = (self.show_value, self.value) {
                header = header.child(
                    text(format!("{}%", (v * 100.0).round())).key("value").font(k.ty.text_small).color(c.text_weak),
                );
            }
            root = root.child(header.a11y_hidden());
        }
        root = report(root, self.value);
        let mut track = row()
            .key("track")
            .w_full()
            .h(8.0)
            .radius(999.0)
            .fill(c.surface_base)
            .border(1.0, c.border_weak)
            .clip_soft(0.0);
        let fill = el().key("fill").h_full().radius(999.0).fill(c.border_active);
        track = track.child(match self.value {
            Some(v) => fill.w_frac(v.clamp(0.0, 1.0)),
            // A quarter-width segment crosses the whole track, then wraps from the left.
            None => fill.w_frac(0.25).looping(rune_motion::LoopSpec::slide(1.4, [-1.0, 0.0], [4.0, 0.0])),
        });
        root.child(track.key("track").a11y_hidden())
    }
}

/// A small ring that fills clockwise (`ProgressCircle`): `value` in 0..=1.
pub struct ProgressCircle {
    value: f32,
    size: f32,
    label: Option<String>,
}

impl ProgressCircle {
    pub fn new(value: f32) -> ProgressCircle {
        ProgressCircle { value, size: 16.0, label: None }
    }
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    /// The accessible name: what is progressing ("Upload").
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn build(self, k: &Kit) -> El {
        let ink = k.c.icon_muted;
        let e = report(el().role("progressbar"), Some(self.value));
        let e = match self.label {
            Some(l) => e.label(l),
            None => e,
        };
        e.size(self.size, self.size)
            .child(icon(&RING, self.size).absolute(0.0, 0.0).color(ink).opacity(0.2))
            .child(icon(arc_icon(self.value), self.size).absolute(0.0, 0.0).color(ink))
    }
}

/// Arcs for progress values, quantised to 2% (icons are static; one leak per step at most).
fn arc_icon(v: f32) -> &'static Icon {
    use std::sync::Mutex;
    static CACHE: Mutex<Vec<Option<&'static Icon>>> = Mutex::new(Vec::new());
    let step = (v.clamp(0.0, 1.0) * 50.0).round() as usize;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() < 51 {
        cache.resize(51, None);
    }
    if let Some(i) = cache[step] {
        return i;
    }
    let a = step as f32 / 50.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    let (x, y) = (8.0 + 6.0 * a.cos(), 8.0 + 6.0 * a.sin());
    let large = if step > 25 { 1 } else { 0 };
    let body: &'static str = if step == 0 {
        ""
    } else if step == 50 {
        r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2" fill="none"/>"#
    } else {
        Box::leak(
            format!(
                r#"<path d="M8 2A6 6 0 {large} 1 {x:.3} {y:.3}" stroke="currentColor" stroke-width="2" fill="none"/>"#
            )
            .into_boxed_str(),
        )
    };
    let i: &'static Icon = Box::leak(Box::new(Icon { name: "kit-progress-arc", view: 16.0, body }));
    cache[step] = Some(i);
    i
}
