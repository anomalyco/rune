//! The springs demo: a puck on a track driven by one of Rune's springs. Click to send it
//! somewhere, click again before it lands, or grab it and throw it. The trace under it plots
//! its position over the last 2.4 s: with velocity kept, every retarget bends the curve
//! smoothly. It draws frames only while something moves and only while it's on screen.

use std::collections::VecDeque;

use rune::{AnimExt, Color, Cx, Drag, El, LineCap, Phase, Shape, Spring, TextStyle, Tx, Velocity, canvas, col, el, oklcha, row, text};
use rune_motion::Animated;

use crate::State;
use crate::tokens::*;
use crate::ui::shape_dot;

pub const SPRINGS: [(&str, &str, Spring); 3] = [
    ("Snappy", "0.25s", Spring::new(0.25, 0.0)),
    ("Smooth", "0.4s", Spring::new(0.4, 0.0)),
    ("Bouncy", "0.5s · 0.3", Spring::new(0.5, 0.3)),
];
const STOPS: usize = 5;
/// Seconds of history in the trace.
const WINDOW: f64 = 2.4;
const STAGE_H: f32 = 72.0;
const TRACE_H: f32 = 96.0;
const PUCK: f32 = 32.0;
/// How close to the puck's centre a press catches it, in points.
const GRAB: f32 = 22.0;

fn snap(x: f32) -> f32 {
    let n = (STOPS - 1) as f32;
    (x.clamp(0.0, 1.0) * n).round() / n
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    t: f64,
    x: f32,
    target: f32,
    kick: bool,
}

#[derive(Debug)]
struct Grab {
    offset: f32,
    /// Recent (time, position) pairs, for the throw's speed.
    pts: VecDeque<(f64, f32)>,
    /// Holding the puck. A press on the track sends it there instead, until the pointer moves.
    live: bool,
}

pub struct Demo {
    pub which: usize,
    pub keep: bool,
    anim: Animated<f32>,
    samples: VecDeque<Sample>,
    kicked: bool,
    last_motion: f64,
    grab: Option<Grab>,
    visible: bool,
    running: bool,
    /// Where the last frame drew the puck (0 to 1) and when.
    pub x: f32,
    t: f64,
}

impl Default for Demo {
    fn default() -> Self {
        Demo {
            which: 2,
            keep: true,
            anim: Animated::new(0.0),
            samples: VecDeque::new(),
            kicked: false,
            last_motion: f64::NEG_INFINITY,
            grab: None,
            visible: false,
            running: false,
            x: 0.0,
            t: 0.0,
        }
    }
}

impl Demo {
    fn spring(&self) -> Spring {
        SPRINGS[self.which].2
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn target(&self) -> f32 {
        self.anim.target()
    }

    /// Sends the puck to `target` from where it is, keeping its speed unless "Keep velocity"
    /// is off. Reduced motion puts it there.
    fn retarget(&mut self, target: f32, now: f64, reduced: bool) {
        if reduced {
            self.anim.set(target);
        } else if self.keep {
            self.anim.animate_to(target, self.spring(), now);
        } else {
            self.anim.animate_to(target, self.spring().velocity(Velocity::Zero), now);
        }
        self.kick(now);
    }

    fn kick(&mut self, now: f64) {
        self.kicked = true;
        self.last_motion = now;
        if self.visible {
            self.running = true;
        }
    }

    pub fn step(&mut self, dir: i32, now: f64, reduced: bool) {
        let to = snap(self.anim.target() + dir as f32 / (STOPS - 1) as f32);
        self.retarget(to, now, reduced);
    }

    pub fn press(&mut self, at: f32, travel: f32, now: f64, reduced: bool) {
        let x = self.anim.sample(now);
        let on_puck = ((at - x) * travel).abs() < GRAB;
        self.grab = Some(Grab { offset: at - x, pts: VecDeque::from([(now, at)]), live: on_puck });
        if on_puck {
            // Caught: the finger holds it where it is, with no animation while it's held.
            self.anim.set(x);
            self.kick(now);
        } else {
            self.retarget(at, now, reduced);
        }
    }

    pub fn drag(&mut self, at: f32, travel: f32, now: f64) {
        let Some(g) = self.grab.as_mut() else { return };
        g.pts.push_back((now, at));
        if g.pts.len() > 6 {
            g.pts.pop_front();
        }
        if !g.live {
            // A press on the track sent the puck there; moving 4 points takes it over from
            // wherever it is now.
            if (at - g.pts[0].1).abs() * travel < 4.0 {
                return;
            }
            g.live = true;
            g.offset = at - self.anim.sample(now);
        }
        self.anim.set((at - g.offset).clamp(0.0, 1.0));
        self.kick(now);
    }

    pub fn release(&mut self, now: f64, reduced: bool) {
        let Some(g) = self.grab.take() else { return };
        if !g.live {
            return;
        }
        let pts: Vec<_> = g.pts.iter().filter(|(t, _)| now - t < 0.1).collect();
        let v = match (pts.first(), pts.last()) {
            (Some(a), Some(b)) if pts.len() > 1 => (b.1 - a.1) / ((b.0 - a.0) as f32).max(0.008),
            _ => 0.0,
        };
        let x = self.anim.sample(now);
        let target = snap(x + v * 0.12);
        if reduced {
            self.anim.set(target);
        } else {
            self.anim.release(if self.keep { v } else { 0.0 }, target, self.spring(), now);
        }
        self.kick(now);
    }

    pub fn set_visible(&mut self, on: bool, now: f64) {
        self.visible = on;
        self.running = on && now - self.last_motion < WINDOW;
    }

    /// Advances to `now`: samples the puck into the trace. True while it draws frames.
    pub fn tick(&mut self, now: f64) -> bool {
        if !self.running {
            return false;
        }
        if self.grab.is_some() || !self.anim.is_settled(now) {
            self.last_motion = now;
        }
        self.x = self.anim.sample(now);
        self.t = now;
        self.samples.push_back(Sample { t: now, x: self.x, target: self.anim.target(), kick: self.kicked });
        self.kicked = false;
        while self.samples.len() > 2 && self.samples[1].t < now - WINDOW {
            self.samples.pop_front();
        }
        if now - self.last_motion > WINDOW {
            self.running = false;
        }
        true
    }
}

/// The puck's position from a press `x` points into a stage `w` wide, 0 to 1.
fn frac(x: f32, w: f32) -> f32 {
    ((x - 32.0) / travel(w)).clamp(0.0, 1.0)
}

fn travel(w: f32) -> f32 {
    (w - 64.0).max(1.0)
}

/// `points` dashed `on` then `off` points along their length.
fn dashed(mut shape: Shape, points: &[[f32; 2]], on: f32, off: f32, width: f32, color: Color) -> Shape {
    let mut phase = 0.0f32;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let mut d = 0.0;
        while d < len {
            let period = on + off;
            let into = phase % period;
            let left = if into < on { on - into } else { period - into };
            let step = left.min(len - d);
            if into < on {
                let p = |s: f32| [a[0] + (b[0] - a[0]) * s / len, a[1] + (b[1] - a[1]) * s / len];
                shape = shape.line(p(d), p(d + step), width, LineCap::Butt, color);
            }
            d += step;
            phase += step;
        }
    }
    shape
}

fn trace(d: &Demo, w: f32) -> Shape {
    let h = TRACE_H;
    let pad = 8.0;
    let x_at = |t: f64| w - ((d.t - t) / WINDOW) as f32 * w;
    // Bouncy springs overshoot: leave room above and below the track's ends.
    let y_at = |v: f32| h - pad - (v + 0.15) / 1.3 * (h - 2.0 * pad);
    let mut shape = Shape::new();
    if d.samples.len() < 2 {
        return shape;
    }
    let target: Vec<[f32; 2]> = d.samples.iter().map(|s| [x_at(s.t), y_at(s.target)]).collect();
    shape = dashed(shape, &target, 2.0, 4.0, 1.0, Color::WHITE.alpha(0.12));
    let path: Vec<[f32; 2]> = d.samples.iter().map(|s| [x_at(s.t), y_at(s.x)]).collect();
    shape = shape.stroke(path, 1.5, ink());
    for s in d.samples.iter().filter(|s| s.kick) {
        shape = shape_dot([x_at(s.t), y_at(s.x)], 2.5, glint(), shape);
    }
    shape
}

fn segmented(d: &Demo, stacked: bool) -> El {
    let mut segs = row().key("springs").role("radiogroup").label("Spring").padding(3.0).radius(18.0).fill(surface2());
    for (i, (name, note, _)) in SPRINGS.into_iter().enumerate() {
        let on = i == d.which;
        let seg = if stacked { col().py(4.0) } else { row().h(30.0).gap(8.0).center_y() };
        segs = segs.child(seg.key(name).role("radio").label(name).checked(on).focusable(true).px(12.0).radius(15.0)
            .fill(if on { surface3() } else { Color::TRANSPARENT }).hover_group()
            .on_press(move |s: &mut State| s.spring.which = i)
            .child(text(name).font(TextStyle::new(13.0, 500.0).lh(20.0)).color(if on { fg() } else { fg2() }).group_color(fg()))
            .child(text(note).font(MONO_12).color(fg3()).a11y_hidden()));
    }
    segs
}

fn checkbox(on: bool) -> El {
    let mut b = el().key("box").size(16.0, 16.0).radius(4.0).center();
    b = if on {
        b.fill(ink()).child(canvas(Shape::new().view_box(16.0, 16.0)
            .stroke(vec![[4.0, 8.5], [7.0, 11.5], [12.0, 5.0]], 2.0, Color::BLACK)).key("tick").size(16.0, 16.0))
    } else {
        b.border(1.5, line2())
    };
    row().key("keep").role("checkbox").label("Keep velocity").checked(on).focusable(true).gap(8.0).center_y()
        .on_press(|s: &mut State| s.spring.keep = !s.spring.keep)
        .child(b.a11y_hidden())
        .child(text("Keep velocity").font(TEXT_14).color(fg2()).a11y_hidden())
}

pub fn demo(s: &State, cx: &Cx<State>, bp: Bp) -> El {
    let d = &s.spring;
    let reduced = cx.is_reduced_motion();
    let w = cx.layout_size("spring-stage").map_or(600.0, |[w, _]| w);
    let travel_w = travel(w);
    let mut rail = el().key("rail").h(2.0).radius(1.0).fill(surface3()).pin(Some(32.0), Some(35.0), Some(32.0), None).a11y_hidden();
    for i in 0..STOPS {
        let x = travel_w * i as f32 / (STOPS - 1) as f32;
        rail = rail.child(el().key(format!("stop-{i}")).size(4.0, 4.0).radius(2.0).fill(line2()).absolute(x - 2.0, -1.0));
    }
    let puck = el().key("puck").size(PUCK, PUCK).radius(PUCK / 2.0).fill(fg())
        .absolute(16.0 + d.x * travel_w, 20.0)
        .shadow(2.0, 8.0, oklcha(0.0, 0.0, 0.0, 0.5))
        .box_shadow(rune::Shadow { x: 0.0, y: 0.0, blur: 0.0, spread: 4.0, color: ink().alpha(0.25), inset: false })
        .a11y_hidden();
    let stage = el().key("spring-stage").w_full().h(STAGE_H).radius(R2).fill(surface1()).border(1.0, line())
        .role("slider").label("Puck position").focusable(true)
        .range_value(f64::from(d.target()), 0.0, 1.0).range_step(1.0 / (STOPS - 1) as f64)
        .on_step(move |s: &mut State, steps, _| {
            let now = s.clock_now();
            s.spring.step(steps.signum(), now, reduced);
        })
        .on_drag(move |s: &mut State, g: &mut Drag, _tx: &mut Tx| {
            let now = s.clock_now();
            let at = frac(g.x, g.w);
            match g.phase {
                Phase::Down => s.spring.press(at, travel(g.w), now, reduced),
                Phase::Move => s.spring.drag(at, travel(g.w), now),
                Phase::Up => s.spring.release(now, reduced),
            }
        })
        .on_visible(|s: &mut State, on, _tx: &mut Tx| {
            let now = s.clock_now();
            s.spring.set_visible(on, now);
        })
        .child(rail)
        .child(puck);
    let trace_w = cx.layout_size("trace").map_or(w, |[w, _]| w);
    let trace = canvas(trace(d, trace_w)).key("trace").w_full().h(TRACE_H).mt(8.0)
        .clip_fade_linear(trace_w * 0.25, 0.0, 0.0, 0.0).a11y_hidden();
    let controls = row().key("controls").w_full().justify_between().flex_wrap().gaps(12.0, 24.0).center_y().mt(16.0)
        .child(segmented(d, bp.narrow()))
        .child(checkbox(d.keep));
    col().key("spring-demo").role("figure").w_full().mt(32.0)
        .child(stage)
        .child(trace)
        .child(controls)
        .child(text("Click the track, then click somewhere else before the puck lands. Or grab it and throw it. Turn off \u{201c}Keep velocity\u{201d} to see a spring that starts from rest every time.")
            .key("caption").font(TEXT_13).color(fg3()).wrap().max_w(MEASURE).mt(12.0))
}
