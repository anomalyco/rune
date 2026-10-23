//! Spinner, Loader, Progress and ProgressCircle (`@opencode/ui` components/spinner,
//! feedback/loader, components/progress, feedback/progress-circle).
//!
//! Renderer-owned loops share Rune's deterministic clock and cool through presence on removal.

use rune::{Color, El, Icon, col, el, icon, row, spacer, text};

use crate::Kit;

/// The 4×4 pulse grid: corners hidden, outer squares dim (0.15), inner squares 0.4.
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
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn build(self, k: &Kit) -> El {
        let ink = self.color.unwrap_or(k.c.std_text);
        let u = self.size / 15.0;
        let mut e = el().role("progressbar").label("Loading").size(self.size, self.size);
        for i in 0..16 {
            let (x, y) = ((i % 4) as f32 * 4.0, (i / 4) as f32 * 4.0);
            let corner = matches!(i, 0 | 3 | 12 | 15);
            let outer = matches!(i, 1 | 2 | 4 | 7 | 8 | 11 | 13 | 14);
            if corner {
                continue;
            }
            let a = if outer { 0.15 } else { 0.4 };
            e = e.child(
                el().key(format!("sq{i}")).size(3.0 * u, 3.0 * u).absolute(x * u, y * u).radius(u).fill(ink)
                    // Web chooses random 1–2 s periods and 0–1.5 s delays at module load.
                    // A fixed per-cell seed preserves that timing range in replays/goldens.
                    .looping(rune_motion::LoopSpec::opacity(1.0 + ((i * 37) % 101) as f64 / 100.0,
                        ((i * 53) % 101) as f64 * 0.015, a, if outer { 0.35 } else { 1.0 })),
            );
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

/// The v2 loader: a faint ring with a third-arc (`Loader`), 16 points by default.
pub struct Loader {
    size: f32,
}

impl Loader {
    pub fn new() -> Loader {
        Loader { size: 16.0 }
    }
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn build(self, k: &Kit) -> El {
        let ink = k.c.icon_muted;
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
            root = root.child(header);
        }
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
        root.child(track.key("track"))
    }
}

/// A small ring that fills clockwise (`ProgressCircle`): `value` in 0..=1.
pub struct ProgressCircle {
    value: f32,
    size: f32,
}

impl ProgressCircle {
    pub fn new(value: f32) -> ProgressCircle {
        ProgressCircle { value, size: 16.0 }
    }
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn build(self, k: &Kit) -> El {
        let ink = k.c.icon_muted;
        el().role("progressbar")
            .size(self.size, self.size)
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
