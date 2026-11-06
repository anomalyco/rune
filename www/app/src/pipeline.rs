//! One frame's path as a row of steps (a column on a phone). When it comes into view a light
//! runs along it once, warming each step as it passes; a click runs it again. Under reduced
//! motion the steps light together and fade.

use rune::{Color, El, col, el, row, text};

use crate::State;
use crate::tokens::*;

pub const STEPS: [(&str, &str); 7] = [
    ("Input", "pointer, keys, text"),
    ("State", "your struct, changed"),
    ("View", "your view, called again"),
    ("Diff", "keyed, against the last"),
    ("Layout", "flex and grid, by Taffy"),
    ("Springs", "position and velocity"),
    ("Draw", "one instanced draw"),
];

/// Seconds from the first step to the last.
const TRAVEL: f64 = 1.8;
const RISE: f64 = 0.06;
/// Cooling time constant: a lit step fades, it never cuts out.
const TAU: f64 = 0.32;
const COMET: f32 = 64.0;
const GAP_ROW: f32 = 12.0;
const GAP_COL: f32 = 8.0;

fn smootherstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// When the head passes step `i`, in seconds after the start.
fn passes(i: usize) -> f64 {
    let target = i as f64 / (STEPS.len() - 1) as f64;
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..18 {
        let m = (lo + hi) / 2.0;
        if smootherstep(m) < target { lo = m } else { hi = m }
    }
    lo * TRAVEL
}

#[derive(Default)]
pub struct Pipeline {
    /// When the light set off, while it's lit.
    start: Option<f64>,
    /// When the reduced-motion fade began, while it shows.
    fade: Option<f64>,
    played: bool,
    visible: bool,
    /// The time the view draws.
    t: f64,
}

/// What a frame of the light looks like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub heat: [f32; 7],
    /// Where the head is (0 to 1 along the steps) and how bright.
    pub head: f32,
    pub comet: f32,
}

impl Light {
    const DARK: Light = Light { heat: [0.0; 7], head: 0.0, comet: 0.0 };
}

impl Pipeline {
    pub fn active(&self) -> bool {
        self.start.is_some() || self.fade.is_some()
    }

    pub fn play(&mut self, now: f64, reduced: bool) {
        if reduced {
            self.fade = Some(now);
        } else {
            self.start = Some(now);
        }
    }

    pub fn set_visible(&mut self, on: bool, now: f64, reduced: bool) {
        self.visible = on;
        if !on {
            // Off screen: finish cooled, without drawing the rest.
            self.start = None;
            self.fade = None;
        } else if !self.played {
            self.played = true;
            self.play(now, reduced);
        }
    }

    pub fn light(&self) -> Light {
        if let Some(f) = self.fade {
            let s = self.t - f;
            let h = if s < 0.75 { smootherstep(s / 0.15) } else { 1.0 - smootherstep((s - 0.75) / 0.15) } as f32;
            return Light { heat: [h; 7], ..Light::DARK };
        }
        let Some(start) = self.start else { return Light::DARK };
        let t = self.t - start;
        let tail = t - TRAVEL;
        let comet = if tail < 0.0 { 1.0 } else { (-tail / TAU).exp() as f32 };
        let mut heat = [0.0; 7];
        for (i, h) in heat.iter_mut().enumerate() {
            let since = t - passes(i);
            let tau = if i == STEPS.len() - 1 { TAU * 2.5 } else { TAU };
            let v = if since < 0.0 { 0.0 } else if since < RISE { since / RISE } else { (-(since - RISE) / tau).exp() };
            *h = if v > 0.004 { v as f32 } else { 0.0 };
        }
        Light { heat, head: smootherstep(t / TRAVEL) as f32, comet: if comet > 0.004 { comet } else { 0.0 } }
    }

    /// Advances to `now`. True while the light is on.
    pub fn tick(&mut self, now: f64) -> bool {
        self.t = now;
        if let Some(f) = self.fade
            && now - f >= 0.9
        {
            self.fade = None;
        }
        if let Some(s) = self.start {
            let light = self.light();
            let lit = now - s < TRAVEL || light.comet > 0.0 || light.heat.iter().any(|&h| h > 0.0);
            if !lit {
                self.start = None;
            }
        }
        self.start.is_some() || self.fade.is_some()
    }
}

/// The centre of each step in a box `w` by `h`, laid out as a row or a column.
fn centers(w: f32, h: f32, stacked: bool) -> Vec<[f32; 2]> {
    let n = STEPS.len() as f32;
    (0..STEPS.len())
        .map(|i| {
            let i = i as f32;
            if stacked {
                let step = (h - GAP_COL * (n - 1.0)) / n;
                [w / 2.0, i * (step + GAP_COL) + step / 2.0]
            } else {
                let step = (w - GAP_ROW * (n - 1.0)) / n;
                [i * (step + GAP_ROW) + step / 2.0, h / 2.0]
            }
        })
        .collect()
}

fn along(c: &[[f32; 2]], p: f32) -> [f32; 2] {
    let span = (c.len() - 1) as f32 * p.clamp(0.0, 1.0);
    let i = (span.floor() as usize).min(c.len() - 2);
    let f = span - i as f32;
    let (a, b) = (c[i], c[i + 1]);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]
}

fn step(i: usize, heat: f32, stacked: bool) -> El {
    let (name, note) = STEPS[i];
    let body = if stacked { row().justify_between().gap(16.0) } else { col().gap(4.0).min_h(96.0).w(0.0).grow(1.0) };
    let glow = ink().alpha(0.35);
    body.key(format!("pl-{}", name.to_lowercase())).role("listitem").padding(12.0).radius(R2).fill(rune::oklch(0.15, 0.004, 265.0)).border(1.0, line())
        .child(el().key("heat").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(R2)
            .fill(ink().alpha(0.18)).border(1.0, ink().alpha(0.7)).shadow(0.0, 24.0, glow)
            .opacity(heat).a11y_hidden())
        .child(text(name).key("name").font(TEXT_14_MEDIUM).color(fg()))
        .child(text(note).key("note").font(MONO_12).color(fg3()).wrap())
}

pub fn pipeline(p: &Pipeline, size: Option<[f32; 2]>, bp: Bp, reduced: bool) -> El {
    let stacked = bp.stack_split();
    let light = p.light();
    let [w, h] = size.unwrap_or([1000.0, 96.0]);
    let c = centers(w, h, stacked);
    let (a, b) = (c[0], c[STEPS.len() - 1]);
    let wire = if stacked { el().size(1.0, b[1] - a[1]) } else { el().size(b[0] - a[0], 1.0) };
    let head = along(&c, light.head);
    let comet = el().key("comet").size(COMET, COMET).radius(COMET / 2.0)
        .radial_gradient([0.5, 0.5], COMET / 2.0, [(0.0, glint()), (0.2, ink().alpha(0.6)), (1.0, Color::TRANSPARENT)])
        .absolute(head[0] - COMET / 2.0, head[1] - COMET / 2.0)
        .opacity(light.comet).a11y_hidden();
    let body = if stacked { col().gap(GAP_COL) } else { row().gap(GAP_ROW) };
    let mut steps = body.key("pl").role("list").label("One frame, step by step").w_full();
    for i in 0..STEPS.len() {
        steps = steps.child(step(i, light.heat[i], stacked));
    }
    // The wire and the light run behind the steps, outside the list so it holds only steps.
    let diagram = col().key("pl-box").w_full()
        .on_press(move |s: &mut State| {
            let now = s.clock_now();
            s.pipeline.play(now, reduced);
        })
        .on_visible(move |s: &mut State, on, _tx: &mut rune::Tx| {
            let now = s.clock_now();
            s.pipeline.set_visible(on, now, reduced);
        })
        .child(wire.key("wire").fill(line2()).absolute(a[0], a[1]).a11y_hidden())
        .child(comet)
        .child(steps);
    col().key("pipeline").role("figure").w_full().mt(32.0)
        .child(diagram)
        .child(text("Click the diagram to run the frame again.").key("caption").font(TEXT_13).color(fg3()).wrap().max_w(MEASURE).mt(12.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_light_runs_once_and_cools() {
        let mut p = Pipeline::default();
        p.set_visible(true, 1.0, false);
        assert!(p.tick(1.0));
        assert!(p.tick(1.0 + TRAVEL / 2.0));
        let mid = p.light();
        assert!(mid.heat[0] > 0.0 && mid.heat[6] == 0.0, "{mid:?}");
        assert!(p.tick(1.0 + TRAVEL + 0.1));
        assert!(!p.tick(1.0 + TRAVEL + 6.0), "it stops asking for frames once cool");
        assert_eq!(p.light(), Light::DARK);
        p.set_visible(false, 9.0, false);
        p.set_visible(true, 10.0, false);
        assert!(!p.tick(10.0), "it plays on the first view only");
    }

    #[test]
    fn reduced_motion_fades_every_step_together() {
        let mut p = Pipeline::default();
        p.set_visible(true, 0.0, true);
        assert!(p.tick(0.4));
        assert_eq!(p.light().heat, [1.0; 7]);
        assert_eq!(p.light().comet, 0.0);
        assert!(!p.tick(1.0));
    }
}
