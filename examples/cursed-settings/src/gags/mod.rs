//! The punchlines. Each frame turns the settings (and where Rune drew their targets) into
//! rune-fx layers and rune-light lights. Every gag runs on an envelope that springs from where
//! it is toward on or off, so turning one off mid-flight cools it instead of cutting it.

mod lighting;

use std::f32::consts::{PI, TAU};

use rune_fx::{
    BlurMode, DistortParams, ElectricParams, ElectricStyle, FireParams, FireStyle, Fx, FxLayer, GlowParams,
    LensParams, MotionBlurParams, MotionSamples, OneShots, Pose, PulseParams, PulseStyle, Rect, Rgb, Shape, rgb,
};
use rune_light::{Light, Settings};
use rune_motion::{Animated, Spring, Vec2};

use crate::ui::node;
use crate::{Cursed, Page};

/// How long the cache takes to go down the drain.
pub const FLUSH_SECONDS: f32 = 1.3;
/// The emptied card fading back in afterwards.
const FLUSH_REVEAL: f32 = 0.35;

pub const SRC_NODE: u32 = 0;
const STRIKE_WINDUP: f32 = 0.32;

// ---------------------------------------------------------------- envelopes

/// Step response of a spring with Rune's (duration, bounce) parameters.
fn step(u: f32, dur: f32, bounce: f32) -> f32 {
    if u <= 0.0 {
        return 0.0;
    }
    let w = TAU / dur.max(1e-3);
    if bounce <= 1e-3 {
        1.0 - (1.0 + w * u) * (-w * u).exp()
    } else {
        let z = 1.0 - bounce.min(0.95);
        let wd = w * (1.0 - z * z).sqrt();
        1.0 - (-z * w * u).exp() * ((wd * u).cos() + z * w / wd * (wd * u).sin())
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A switched envelope: 0 off, 1 on, sprung from wherever it was when the switch flipped.
#[derive(Clone, Copy, Debug)]
pub struct Env {
    on: Option<bool>,
    t0: f32,
    from: f32,
    up: (f32, f32),
    down: f32,
    delay: f32,
    reduced: bool,
}

impl Env {
    fn new(up: f32, bounce: f32, down: f32) -> Env {
        Env { on: None, t0: -1e3, from: 0.0, up: (up, bounce), down, delay: 0.0, reduced: false }
    }
    fn delay(mut self, d: f32) -> Env {
        self.delay = d;
        self
    }
    /// The first sight is the resting state, not a transition.
    fn set(&mut self, on: bool, t: f32, reduced: bool) -> bool {
        match self.on {
            None => {
                self.on = Some(on);
                self.from = on as u8 as f32;
                false
            }
            Some(o) if o != on => {
                self.from = self.at(t);
                self.on = Some(on);
                self.t0 = t;
                self.reduced = reduced;
                true
            }
            _ => false,
        }
    }
    fn is_on(&self) -> bool {
        self.on == Some(true)
    }
    fn since(&self, t: f32) -> f32 {
        t - self.t0
    }
    fn shape(&self) -> (f32, f32, f32) {
        if self.reduced {
            return (0.18, 0.0, 0.0);
        }
        if self.is_on() { (self.up.0, self.up.1, self.delay) } else { (self.down, 0.0, 0.0) }
    }
    fn at(&self, t: f32) -> f32 {
        let target = if self.is_on() { 1.0 } else { 0.0 };
        let (dur, bounce, delay) = self.shape();
        self.from + (target - self.from) * step(t - self.t0 - delay, dur, bounce)
    }
    fn moving(&self, t: f32) -> bool {
        let (dur, bounce, delay) = self.shape();
        self.since(t) < delay + dur * (1.3 + 2.5 * bounce)
    }
}

// ---------------------------------------------------------------- frame inputs and outputs

#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    Under(u16),
    Over(u16),
    /// Above everything but the pointer.
    Top,
}

pub struct Lay {
    pub layer: FxLayer,
    pub anchor: Anchor,
}

/// Where Rune drew each target this frame (points) and its opacity.
#[derive(Default)]
pub struct Targets {
    pub fast: Option<(Rect, f32)>,
    pub search: Option<(Rect, f32)>,
    pub firewall: Option<(Rect, f32)>,
    pub allowed: Option<(Rect, f32)>,
    pub ip: Option<(Rect, f32)>,
    pub fence: Option<(Rect, f32)>,
    pub perimeter: Option<(Rect, f32)>,
    pub reload: Option<(Rect, f32)>,
    pub cache: Option<(Rect, f32)>,
    pub sel: Option<(Rect, f32)>,
}

pub struct In<'a> {
    pub s: &'a Cursed,
    pub t: f32,
    pub now: f64,
    /// Device px per pt.
    pub scale: f32,
    /// The window in points.
    pub vp: Rect,
    pub pointer: Option<[f32; 2]>,
    pub dark_theme: bool,
    /// The selection colour under the glass (linear).
    pub glass_tint: [f32; 3],
    pub targets: Targets,
}

pub struct Out {
    pub lays: Vec<Lay>,
    pub lights: Vec<Light>,
    pub settings: Settings,
    /// How much of the lit frame shows over the plain one (0 skips lighting).
    pub lit: f32,
    /// The cache card's pose track while it drains, with its rest rect.
    pub flush: Option<(MotionBlurParams, MotionSamples, Rect)>,
    pub anim: bool,
}

pub struct Gags {
    fire: Env,
    fire_x: f32,
    charge: Env,
    fence: Env,
    dark: Env,
    contrast: Env,
    spot: Env,
    spot_pos: Option<Animated<Vec2>>,
    frost: Env,
    hot: Env,
    fast: Option<bool>,
    strike: Option<f32>,
    drop: Option<Animated<Vec2>>,
    torch: [f32; 2],
    taps: Option<u32>,
    shocks: OneShots<[f32; 2]>,
}

impl Default for Gags {
    fn default() -> Self {
        Gags {
            fire: Env::new(0.9, 0.12, 0.55).delay(0.24),
            fire_x: 0.0,
            charge: Env::new(0.35, 0.0, 0.5),
            fence: Env::new(0.45, 0.0, 0.4).delay(0.2),
            dark: Env::new(0.22, 0.0, 0.5),
            contrast: Env::new(0.6, 0.0, 0.5),
            spot: Env::new(0.5, 0.0, 0.55),
            spot_pos: None,
            frost: Env::new(0.5, 0.0, 0.45),
            hot: Env::new(1.6, 0.0, 2.4).delay(0.1),
            fast: None,
            strike: None,
            drop: None,
            torch: [480.0, 330.0],
            taps: None,
            shocks: OneShots::new(4),
        }
    }
}

fn center(r: Rect) -> [f32; 2] {
    [r.x + r.w * 0.5, r.y + r.h * 0.5]
}

fn rounded(r: Rect, radius: f32) -> Shape {
    Shape::rounded(r.x, r.y, r.w, r.h, radius)
}

fn lin3(c: Rgb) -> [f32; 3] {
    c.0
}

/// Black-body-ish ramp for heating metal: dull red, orange, yellow-white.
fn glow_of(h: f32) -> Rgb {
    let a = lin3(rgb(0x7a1204));
    let b = lin3(rgb(0xff4a12));
    let c = lin3(rgb(0xffb347));
    let k = h.clamp(0.0, 1.0);
    let (p, q, u) = if k < 0.6 { (a, b, k / 0.6) } else { (b, c, (k - 0.6) / 0.4) };
    Rgb([mix(p[0], q[0], u), mix(p[1], q[1], u), mix(p[2], q[2], u)])
}

/// Signed distance to a rounded rect and the nearest point on its border.
fn border_point(r: Rect, radius: f32, p: [f32; 2]) -> (f32, [f32; 2]) {
    let c = center(r);
    let half = [r.w * 0.5 - radius, r.h * 0.5 - radius];
    let q = [p[0] - c[0], p[1] - c[1]];
    let cl = [q[0].clamp(-half[0], half[0]), q[1].clamp(-half[1], half[1])];
    let d = [q[0] - cl[0], q[1] - cl[1]];
    let len = d[0].hypot(d[1]);
    if len > 1e-3 {
        let n = [d[0] / len, d[1] / len];
        (len - radius, [c[0] + cl[0] + n[0] * radius, c[1] + cl[1] + n[1] * radius])
    } else {
        // Inside the core: push out to the nearest side.
        let dx = half[0] + radius - q[0].abs();
        let dy = half[1] + radius - q[1].abs();
        if dx < dy {
            (-dx, [c[0] + q[0].signum() * (half[0] + radius), p[1]])
        } else {
            (-dy, [p[0], c[1] + q[1].signum() * (half[1] + radius)])
        }
    }
}

fn arc_shape(a: [f32; 2], b: [f32; 2]) -> Shape {
    let x = a[0].min(b[0]);
    let y = a[1].min(b[1]);
    Shape::rounded(x, y, (a[0] - b[0]).abs().max(2.0), (a[1] - b[1]).abs().max(2.0), 0.0)
}

impl Gags {
    pub fn frame(&mut self, i: &In) -> Out {
        let s = i.s;
        let p = &s.p;
        let t = i.t;
        let rm = p.reduce_motion;
        let tg = &i.targets;
        let light_ui = !i.dark_theme;
        // Emissive light vanishes on a white window: there it reads as ink, deep and opaque.
        let (volt, volt_core, volt_occ, volt_k) =
            if light_ui { (rgb(0x1f5cff), rgb(0x7fa6ff), 0.95, 1.5) } else { (rgb(0x5a9bff), rgb(0xeaf3ff), 0.12, 1.0) };
        let mut lays: Vec<Lay> = Vec::new();
        let mut anim = false;
        let mut push = |layer: FxLayer, anchor: Anchor| lays.push(Lay { layer: layer.at(t).reduced(rm), anchor });
        let light_theme = !i.dark_theme;

        // ---- Firewall: a wall of fire along the top of "Allowed apps"; off leaves embers.
        if self.fire.set(p.firewall, t, rm) && p.firewall {
            self.fire_x = tg.firewall.map_or(0.0, |(r, _)| center(r)[0]);
        }
        anim |= self.fire.moving(t);
        if let Some((a, alpha)) = tg.allowed {
            let g = self.fire.at(t).max(0.0);
            let since = self.fire.since(t);
            if self.fire.is_on() && since < 0.8 && !rm
                && let Some((sw, _)) = tg.firewall
            {
                // The spark: winds up, shoots from the switch to the card's edge, blooms there.
                let from = center(sw);
                let to = [self.fire_x.clamp(a.x + 12.0, a.x + a.w - 12.0), a.y];
                let d = [to[0] - from[0], to[1] - from[1]];
                let len = d[0].hypot(d[1]).max(1.0);
                push(
                    Fx::pulse(PulseParams {
                        style: PulseStyle::Ping { from, dir: [d[0] / len, d[1] / len], travel: len },
                        color: rgb(0xff7a1a),
                        core: rgb(0xfff1d6),
                        t0: self.fire.t0,
                        duration: 0.26,
                        head_radius: 2.4,
                        intensity: alpha,
                        ..Default::default()
                    })
                    .on(Shape::circle(from[0], from[1], 6.0)),
                    Anchor::Top,
                );
                anim = true;
            }
            if g > 0.003 {
                let ix = self.fire_x.clamp(a.x, a.x + a.w);
                let span = if self.fire.is_on() && !rm { step(since - 0.24, 0.85, 0.0) } else { 1.0 };
                let x0 = mix(ix, a.x, span);
                let x1 = mix(ix, a.x + a.w, span);
                let shape = Shape::rounded(x0, a.y, (x1 - x0).max(14.0), a.h, 8.0);
                push(
                    Fx::fire(FireParams {
                        height: 46.0 * g,
                        scale: 17.0,
                        speed: 1.5,
                        intensity: g.min(1.0) * alpha * if light_theme { 1.5 } else { 1.0 },
                        embers: 18,
                        ember_rise: 62.0,
                        occlusion: if light_theme { 0.9 } else { 0.3 },
                        base_glow: 0.55,
                        ..Default::default()
                    })
                    .on(shape)
                    .seed(5),
                    Anchor::Under(node::ALLOWED),
                );
                push(
                    Fx::distort(DistortParams::HeatHaze { height: 70.0 * g, scale: 10.0, speed: 2.0, amount: 1.3 * g * alpha, whole_shape: false })
                        .on(Shape::rounded(x0, a.y - 30.0 * g, (x1 - x0).max(14.0), a.h, 8.0)),
                    Anchor::Over(node::ALLOWED),
                );
            }
            if !self.fire.is_on() && self.fire.since(t) < 2.4 && !rm {
                let k = 1.0 - smooth(0.2, 2.4, self.fire.since(t));
                push(
                    Fx::fire(FireParams {
                        style: FireStyle::Embers,
                        embers: 26,
                        ember_rise: 44.0,
                        ember_life: 1.9,
                        ember_size: 0.9,
                        intensity: k * alpha,
                        ..Default::default()
                    })
                    .on(rounded(a, 8.0))
                    .seed(6),
                    Anchor::Over(node::ALLOWED),
                );
                anim = true;
            }
        }

        // ---- Static IP: the address field crackles with static.
        self.charge.set(p.static_ip, t, rm);
        anim |= self.charge.moving(t);
        if let Some((f, alpha)) = tg.ip {
            let c = self.charge.at(t);
            if c > 0.003 {
                let burst = if self.charge.is_on() { (-self.charge.since(t) / 0.4).exp() } else { 0.0 };
                let base = ElectricParams::charged();
                push(
                    Fx::electric(ElectricParams {
                        color: volt,
                        core: volt_core,
                        intensity: 0.85 * volt_k * c * alpha,
                        forks: 0.3 + 0.6 * burst,
                        amplitude: base.amplitude + 2.6 * burst,
                        glow: (base.glow * 0.6 + 2.0 * burst).max(1.0),
                        flicker: 0.25 * burst,
                        occlusion: volt_occ,
                        ..base
                    })
                    .on(rounded(f, 6.0))
                    .seed(9),
                    Anchor::Over(node::IP),
                );
            }
        }

        // ---- Electric fence: a charge runs from the switch to the perimeter, which goes live.
        self.fence.set(p.fence, t, rm);
        anim |= self.fence.moving(t);
        if let Some((pr, alpha)) = tg.perimeter {
            let v = self.fence.at(t);
            let since = self.fence.since(t);
            let blue = volt;
            if self.fence.is_on() && since < 0.45
                && let Some((sw, _)) = tg.fence
            {
                let a = center(sw);
                let b = [pr.x + pr.w - 24.0, pr.y + 2.0];
                let e = if since < 0.2 { 1.0 } else { (-(since - 0.2) / 0.08).exp() };
                push(
                    Fx::electric(ElectricParams {
                        style: ElectricStyle::Arc { a, b },
                        color: blue,
                        amplitude: 7.0,
                        frequency: 2.0,
                        strands: 2,
                        forks: 0.6,
                        core: volt_core,
                        glow: 3.5,
                        intensity: e * volt_k * alpha,
                        occlusion: volt_occ,
                        ..Default::default()
                    })
                    .on(arc_shape(a, b))
                    .seed(2 + (since * 20.0) as u32),
                    Anchor::Top,
                );
                anim = true;
            }
            if v > 0.003 {
                push(
                    Fx::electric(ElectricParams {
                        style: ElectricStyle::Border,
                        color: blue,
                        amplitude: 3.6,
                        frequency: 2.6,
                        speed: 3.4,
                        core: volt_core,
                        glow: 2.8,
                        forks: 0.5,
                        strands: 2,
                        intensity: v * volt_k * alpha,
                        occlusion: volt_occ,
                        ..Default::default()
                    })
                    .on(rounded(pr, 8.0))
                    .seed(7),
                    Anchor::Over(node::PERIMETER),
                );
                // Come close and it bites.
                if let Some(q) = i.pointer {
                    let (d, n) = border_point(pr, 8.0, q);
                    let reach = 60.0;
                    if v > 0.5 && d.abs() < reach {
                        let k = (1.0 - d.abs() / reach).powf(0.8) * v * alpha;
                        push(
                            Fx::electric(ElectricParams {
                                style: ElectricStyle::Arc { a: n, b: q },
                                color: blue,
                                amplitude: 3.0 + d.abs() * 0.08,
                                frequency: 2.4,
                                strands: 2,
                                forks: 0.7,
                                fork_len: 14.0,
                                core: volt_core,
                                glow: 3.0,
                                flicker: 0.6,
                                intensity: k * volt_k,
                                occlusion: volt_occ,
                                ..Default::default()
                            })
                            .on(arc_shape(n, q))
                            .seed(31 + (t * 11.0) as u32),
                            Anchor::Top,
                        );
                        push(
                            Fx::glow(GlowParams { color: blue, core: volt_core, radius: 6.0, intensity: k, hot: 0.8, occlusion: if light_ui { 0.6 } else { 0.0 }, ..Default::default() })
                                .on(Shape::circle(q[0], q[1], 2.5))
                                .over(),
                            Anchor::Top,
                        );
                        anim = true;
                    }
                }
            }
        }

        // ---- Lightning-fast startup: lightning strikes the switch.
        if let Some(prev) = self.fast
            && !prev
            && p.fast_startup
            && s.page == Page::General
        {
            self.strike = Some(t);
        }
        self.fast = Some(p.fast_startup);
        let mut flash = 0.0f32;
        let mut flash_at = [0.0f32; 2];
        let mut storm = 0.0f32;
        if let (Some(t0), Some((sw, alpha))) = (self.strike, tg.fast) {
            // Anticipation: the sky darkens first, then the bolt lands and the room recovers.
            let windup = if rm { 0.0 } else { STRIKE_WINDUP };
            let w = t - t0;
            storm = if rm { 0.0 } else { smooth(0.0, windup, w) * (1.0 - smooth(windup + 0.15, windup + 0.9, w)) };
            let u = w - windup;
            if u < 0.0 {
                anim = true;
            } else if u < 1.4 {
                anim = true;
                let e = if rm {
                    1.0 - smooth(0.0, 0.3, u)
                } else if u < 0.05 {
                    1.0
                } else if u < 0.1 {
                    0.12
                } else if u < 0.17 {
                    1.0
                } else {
                    (-(u - 0.17) / 0.09).exp()
                };
                let b = center(sw);
                let a = [b[0] + 46.0, i.vp.y - 12.0];
                if e > 0.01 {
                    push(
                        Fx::electric(ElectricParams {
                            style: ElectricStyle::Arc { a, b },
                            color: if light_ui { rgb(0x2a5cff) } else { rgb(0x9cc4ff) },
                            core: if light_ui { rgb(0x9bb8ff) } else { rgb(0xffffff) },
                            amplitude: 15.0,
                            frequency: 1.5,
                            speed: 6.0,
                            core_width: 1.3,
                            strands: 3,
                            forks: 0.95,
                            fork_len: 30.0,
                            glow: 4.5,
                            flicker: 0.0,
                            intensity: e * alpha * volt_k,
                            occlusion: if light_ui { 0.95 } else { 0.15 },
                            ..Default::default()
                        })
                        .on(arc_shape(a, b))
                        .seed(if u < 0.1 { 3 } else { 4 }),
                        Anchor::Top,
                    );
                }
                // The bolt lands: the window rings like a struck bell.
                if !rm {
                    let full = Shape::rounded(i.vp.x, i.vp.y, i.vp.w, i.vp.h, 0.0);
                    let t1 = t0 + windup;
                    push(
                        Fx::distort(DistortParams::Shockwave { at: b, reach: 420.0, width: 26.0, amount: 12.0, chroma: 0.2, t0: t1, duration: 1.1 }).on(full),
                        Anchor::Top,
                    );
                }
                // The switch stays hot for a moment.
                let ember = (-u / 0.5).exp() * alpha;
                push(
                    Fx::glow(GlowParams { color: rgb(0x8fb8ff), core: rgb(0xffffff), radius: 4.0, intensity: 0.6 * ember, hot: 0.7, occlusion: if light_ui { 0.6 } else { 0.15 }, ..Default::default() })
                        .on(Shape::rounded(sw.x, sw.y, sw.w, sw.h, 4.0)),
                    Anchor::Top,
                );
                flash = e;
                flash_at = b;
            } else {
                self.strike = None;
            }
        }

        // ---- Reduce transparency: the sidebar's glass selection frosts over.
        self.frost.set(p.reduce_transparency, t, rm);
        anim |= self.frost.moving(t);
        if let Some((r, alpha)) = tg.sel {
            let c = Vec2 { x: r.x + r.w * 0.5, y: r.y + r.h * 0.5 };
            let d = self.drop.get_or_insert_with(|| Animated::new(c));
            if rm {
                d.set(c);
            } else {
                d.animate_to(c, Spring::new(0.55, 0.0), i.now);
            }
            anim |= !d.is_settled(i.now);
            let dc = d.sample(i.now);
            let f = self.frost.at(t);
            push(
                Fx::lens(LensParams {
                    bevel: 7.0,
                    depth: 6.0,
                    dispersion: 0.12,
                    magnification: 1.03,
                    presence: alpha,
                    frost: f,
                    shadow: 0.18,
                    rim: 1.3,
                    tint: Rgb(i.glass_tint),
                    tint_amount: 0.92 * f,
                    second: Some(Shape::rounded(dc.x - r.w * 0.5, dc.y - r.h * 0.5, r.w, r.h, 6.0)),
                    union_k: 18.0,
                    ..Default::default()
                })
                .on(rounded(r, 6.0)),
                Anchor::Over(node::NAV),
            );
        }

        // ---- Hot reload: the reload button glows red-hot and the air above it shimmers.
        self.hot.set(p.hot_reload, t, rm);
        anim |= self.hot.moving(t);
        if let Some((b, alpha)) = tg.reload {
            let h = self.hot.at(t).clamp(0.0, 1.2);
            if h > 0.004 {
                anim = true;
                push(
                    Fx::glow(GlowParams {
                        color: glow_of(h),
                        core: rgb(0xffe6c0),
                        radius: 5.0 + 7.0 * h,
                        intensity: 0.9 * h * alpha,
                        hot: 0.35 * h,
                        breathe: 0.18,
                        breathe_period: 1.9,
                        occlusion: if light_theme { 0.7 } else { 0.1 },
                        ..Default::default()
                    })
                    .on(rounded(b, 6.0)),
                    Anchor::Under(node::RELOAD),
                );
                let ht = FireParams::heat();
                push(
                    Fx::fire(FireParams { intensity: ht.intensity * 1.5 * h * alpha, height: 13.0 * h, occlusion: if light_theme { 1.0 } else { 0.1 }, ..ht })
                        .on(rounded(b, 6.0))
                        .seed(2),
                    Anchor::Under(node::RELOAD),
                );
                push(
                    Fx::distort(DistortParams::HeatHaze { height: 52.0, scale: 9.0, speed: 2.2, amount: 1.1 * h * alpha, whole_shape: false }).on(rounded(b, 6.0)),
                    Anchor::Over(node::RELOAD),
                );
            }
        }

        // ---- Haptic feedback: Strong means every click shakes the window.
        if let Some(prev) = self.taps
            && s.taps != prev
            && p.haptics == 2
            && let Some(q) = i.pointer
        {
            self.shocks.trigger(t, q);
        }
        self.taps = Some(s.taps);
        self.shocks.prune(t, 1.4);
        for (sh, k) in self.shocks.live(t, 1.4) {
            anim = true;
            let at = sh.data;
            let full = Shape::rounded(i.vp.x, i.vp.y, i.vp.w, i.vp.h, 0.0);
            push(
                Fx::distort(DistortParams::Shockwave { at, reach: 320.0, width: 22.0, amount: 13.0, chroma: 0.5, t0: sh.t0, duration: 1.15 })
                    .on(full)
                    .opacity(k),
                Anchor::Top,
            );
            push(
                Fx::pulse(PulseParams {
                    style: PulseStyle::Ripple { at },
                    color: if light_ui { rgb(0x4a6cff) } else { rgb(0x8fb0ff) },
                    core: if light_ui { rgb(0x8ea4ff) } else { rgb(0xffffff) },
                    reach: 300.0,
                    width: 2.0,
                    t0: sh.t0,
                    duration: 1.0,
                    intensity: if light_ui { 0.6 } else { 0.35 },
                    occlusion: if light_ui { 0.7 } else { 0.0 },
                    ..Default::default()
                })
                .on(full)
                .over()
                .opacity(k),
                Anchor::Top,
            );
        }

        // ---- Flush cache: the list spirals down the drain, with real shutter blur.
        let mut flush = None;
        if let (Some(t0), Some((c, _))) = (s.flush_t0, tg.cache) {
            let u = t - t0;
            if u <= FLUSH_SECONDS + FLUSH_REVEAL {
                anim = true;
                let rest = c;
                let cc = center(c);
                let drain = [cc[0], c.y + c.h + 36.0];
                let pose_at = move |tt: f32| -> Pose { flush_pose(tt - t0, rest, cc, drain, rm) };
                let mp = MotionBlurParams {
                    mode: BlurMode::Shutter,
                    shutter_angle: if rm { 0.0 } else { 200.0 },
                    max_samples: 48,
                    ..Default::default()
                };
                let samples = MotionSamples::from_track(&mp, rest, t, i.scale, pose_at);
                flush = Some((mp, samples, rest));
            }
        }

        let (lights, settings, lit) = self.lighting(i, storm, flash, flash_at, &mut anim);
        Out { lays, lights, settings, lit, flush, anim }
    }
}

/// The drain: a wind-up, then an accelerating spiral that shrinks into the plughole; then the
/// emptied card fades back in where it was.
fn flush_pose(u: f32, rest: Rect, cc: [f32; 2], drain: [f32; 2], rm: bool) -> Pose {
    let _ = rest;
    if u <= 0.0 {
        return Pose::default();
    }
    if u >= FLUSH_SECONDS {
        let k = smooth(0.0, FLUSH_REVEAL, u - FLUSH_SECONDS);
        return Pose { opacity: k, ..Default::default() };
    }
    if rm {
        return Pose { opacity: 1.0 - smooth(0.0, FLUSH_SECONDS * 0.6, u), ..Default::default() };
    }
    let wind = 0.2;
    if u < wind {
        let w = (PI * u / wind).sin();
        return Pose { rotation: -0.035 * w, scale: 1.0 + 0.02 * w, y: -3.0 * w, ..Default::default() };
    }
    let v = ((u - wind) / (FLUSH_SECONDS - wind)).clamp(0.0, 1.0);
    let k = v.powf(2.2);
    let r0 = (cc[1] - drain[1]).abs();
    let ang = -PI * 0.5 + k * TAU * 1.1;
    let r = r0 * (1.0 - k);
    let x = drain[0] + r * ang.cos();
    let y = drain[1] + r * ang.sin();
    Pose {
        x: x - cc[0],
        y: y - cc[1],
        rotation: k * TAU * 1.35,
        scale: mix(1.0, 0.03, k.powf(0.75)),
        opacity: 1.0 - smooth(0.8, 1.0, v),
    }
}
