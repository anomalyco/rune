//! Per-scene lights, rune-light material primitives and rune-fx layers.

use rune::render::Instance;
use rune_fx::{
    AuroraParams, DistortParams, ElectricParams, ElectricStyle, FireParams, Fx, FxLayer, GlowParams, GlowStyle,
    GrainParams, Heat, LensParams, PulseParams, PulseStyle, Shape, ShimmerParams, SparkleParams, rgb,
};
use rune_light::{Gi, Light, Prim, Settings, TextPolicy};
use rune_motion::{Animated, Spring, Vec2};

use crate::ShotKind;
use crate::knobs::{LAMP_COLORS, Scene, lin};
use crate::stage::{self, mat, node};

use super::{Anchor, Cv, Lay, SRC_MASK};

/// A light that follows the pointer 1:1 while it's over the stage and cools when it leaves.
struct Follow {
    pos: [f32; 2],
    k: Animated<f32>,
    inside: Option<bool>,
}

impl Follow {
    fn new(rest: [f32; 2]) -> Follow {
        Follow { pos: rest, k: Animated::new(1.0), inside: None }
    }

    fn update(&mut self, p: Option<[f32; 2]>, now: f64) -> ([f32; 2], f32) {
        match p {
            Some(p) => {
                self.pos = p;
                if self.inside != Some(true) {
                    self.k.animate_to(1.0, Spring::new(0.3, 0.0), now);
                    self.inside = Some(true);
                }
            }
            None if self.inside == Some(true) => {
                self.k.animate_to(0.0, Spring::new(0.45, 0.0), now);
                self.inside = Some(false);
            }
            None => {}
        }
        (self.pos, self.k.sample(now).clamp(0.0, 1.0))
    }

    fn moving(&self, now: f64) -> bool {
        !self.k.is_settled(now)
    }
}

/// A light switched by state: it warms up fast and cools slowly.
struct Switch {
    on: Option<bool>,
    heat: Heat,
}

impl Switch {
    fn new(attack: f32, cool: f32) -> Switch {
        Switch { on: None, heat: Heat::new(attack, cool) }
    }

    fn at(&mut self, on: bool, t: f32) -> f32 {
        if self.on != Some(on) {
            // The first sight is the resting state, not a warm-up.
            self.heat.set(if self.on.is_none() { -10.0 } else { t }, on);
            self.on = Some(on);
        }
        self.heat.at(t)
    }

    fn moving(&self, t: f32) -> bool {
        self.heat.is_active(t) && self.on == Some(false) || self.on == Some(true) && self.heat.at(t) < 0.999
    }
}

pub(super) struct Live {
    torch: Follow,
    keylight: Follow,
    sun: Follow,
    neon: Switch,
    charged: Switch,
    rec: Switch,
    upload: Switch,
    droplet: Option<Animated<Vec2>>,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            torch: Follow::new([470.0, 250.0]),
            keylight: Follow::new([150.0, 230.0]),
            sun: Follow::new(stage::GODRAY_REST),
            neon: Switch::new(0.12, 0.32),
            charged: Switch::new(0.15, 0.5),
            rec: Switch::new(0.12, 0.6),
            upload: Switch::new(0.12, 0.6),
            droplet: None,
        }
    }
}

/// Per-scene material inputs for prims.
#[derive(Default)]
pub(super) struct PrimCx {
    keys: Vec<f32>,
    bevel: f32,
    gloss: f32,
    rec: f32,
    status: f32,
    progress: f32,
}

fn srgbf(r: f32, g: f32, b: f32) -> [f32; 3] {
    let f = rune::color::srgb_decode;
    [f(r), f(g), f(b)]
}

pub(super) fn prim(ins: &Instance, material: u8, nd: u8, pc: &PrimCx, s: f32) -> Option<Prim> {
    let a = ins.fill[3].max(1e-4);
    let alb = [ins.fill[0] / a, ins.fill[1] / a, ins.fill[2] / a];
    let p = Prim::from_rune(ins.rect, ins.radii).albedo(alb);
    let (bv, g) = (pc.bevel, pc.gloss);
    Some(match material {
        mat::CARD => p.raised(6.0 * s).bevel(6.0 * s, 2.0 * s).gloss(0.55, 0.42),
        mat::CHIP => p.on(6.0 * s).raised(2.5 * s).bevel(4.0 * s, 1.0 * s).gloss(0.4, 0.4),
        mat::BUTTON => p.on(10.0 * s).raised(4.0 * s).bevel(10.0 * s, 3.0 * s).gloss(0.8, 0.3),
        mat::COMPOSER => p.raised(10.0 * s).bevel(7.0 * s, 2.5 * s).gloss(0.6, 0.38),
        mat::ON_CARD => p.on(6.0 * s).gloss(0.4, 0.4),
        mat::SWITCH => p.on(6.0 * s).raised(5.0 * s).bevel(9.0 * s, 4.0 * s).gloss(0.7, 0.3),
        mat::PILL => p.on(6.0 * s).raised(3.0 * s).bevel(4.0 * s, 1.2 * s).gloss(0.5, 0.4),
        mat::BAR => p.on(6.0 * s).raised(2.0 * s),
        mat::DIAL => p.on(6.0 * s).bevel(16.0 * s, 4.0 * s).gloss(0.6, 0.35),
        mat::EMIT => {
            let k = match nd {
                node::REC => pc.rec,
                node::STATUS => pc.status,
                node::PROGRESS => pc.progress,
                _ => 0.0,
            };
            p.on(6.0 * s).emissive([alb[0] * k, alb[1] * k, alb[2] * k])
        }
        mat::FLAT => p,
        mat::KNOB => p.floating(10.0 * s, 22.0 * s).bevel(30.0 * bv * s, 10.0 * bv * s).gloss(0.9, 0.26),
        mat::KEY => {
            let d = nd.checked_sub(node::KEY0).and_then(|i| pc.keys.get(i as usize)).copied().unwrap_or(0.0).clamp(0.0, 1.2);
            p.floating((10.0 - 6.0 * d) * s, (18.0 - 11.0 * d) * s).bevel(12.0 * bv * s, 4.5 * bv * s).gloss(g, 0.28)
        }
        mat::BODY => p.raised(10.0 * s).bevel(22.0 * bv * s, 7.0 * bv * s).gloss(0.7, 0.32),
        mat::SCREEN => p.on(10.0 * s).gloss(1.0, 0.18),
        mat::TILE => p.raised(14.0 * s).bevel(8.0 * s, 2.5 * s).gloss(0.5, 0.4),
        _ => return None,
    })
}

impl Live {
    /// Lights for a lit canvas (appended, scaled by its presence) and its settings.
    pub(super) fn lights(&mut self, cv: &Cv, out: &mut Vec<Light>, pc: &mut PrimCx, anim: &mut bool) -> Settings {
        let s = cv.scale;
        let (t, now) = (cv.t, cv.now);
        match cv.sc {
            Scene::Lamp => {
                let p = cv.px(cv.s.lamp);
                let col = lin(LAMP_COLORS[(cv.k("color") as usize).min(LAMP_COLORS.len() - 1)]);
                out.push(
                    Light::point(p[0], p[1], cv.k("height") * s)
                        .color(col)
                        .intensity(cv.k("intensity"))
                        .radius(cv.k("radius") * s)
                        .range(1100.0 * s)
                        .bulb(0.75)
                        .specular(1.2)
                        .scaled(cv.a),
                );
                if cv.on("fill") {
                    let q = cv.px([-300.0, -200.0]);
                    out.push(Light::point(q[0], q[1], 900.0 * s).color(srgbf(0.55, 0.65, 1.0)).intensity(0.35).radius(220.0 * s).range(6000.0 * s).specular(0.4).scaled(cv.a));
                }
                Settings {
                    ambient: [0.62, 0.63, 0.7],
                    albedo_floor: 0.07,
                    gi: cv.on("bounce").then_some(Gi { strength: 1.0, air: 0.0, bounce: 0.6, absorption: 0.24, ..Default::default() }),
                    rim: 0.9,
                    contact_ao: 0.45,
                    ..Default::default()
                }
            }
            Scene::Flashlight => {
                let (p, k) = self.torch.update(cv.pointer, now);
                *anim |= self.torch.moving(now);
                let h = cv.k("height");
                let lp = [p[0] + 80.0, p[1] + 130.0];
                let (a, b) = (cv.px(p), cv.px(lp));
                let axis = [a[0] - b[0], a[1] - b[1], -h * s];
                out.push(
                    Light::point(b[0], b[1], h * s)
                        .color(srgbf(1.0, 0.95, 0.88))
                        .intensity(cv.k("intensity"))
                        .radius(cv.k("radius") * s)
                        .range(1800.0 * s)
                        .spot(axis, cv.k("beam"))
                        .specular(1.4)
                        .scaled(cv.a * k),
                );
                let am = cv.k("ambient");
                Settings {
                    ambient: [am, am * 1.07, am * 1.28],
                    albedo_floor: 0.04,
                    text: TextPolicy { floor: 0.32, ceiling: 1.3, lit: 1.0, specular: 0.1 },
                    rim: 1.0,
                    contact_ao: 0.3,
                    background_gloss: 0.5,
                    ..Default::default()
                }
            }
            Scene::Gi => {
                let b = if cv.reduced { 1.0 } else { 0.775 + 0.225 * (std::f32::consts::TAU * t / 2.2).sin() };
                *anim |= !cv.reduced;
                let e = cv.k("emission");
                let rec = self.rec.at(cv.s.recording, t);
                let up = self.upload.at(cv.s.uploading, t);
                *anim |= self.rec.moving(t) || self.upload.moving(t);
                pc.rec = 7.0 * b * rec * e;
                pc.status = 2.2 * (0.6 + 0.4 * b) * e;
                pc.progress = 1.9 * (0.85 + 0.15 * b) * up * e;
                let am = cv.k("ambient");
                Settings {
                    ambient: [am, am * 1.03, am * 1.17],
                    gi: cv.on("gi").then_some(Gi { strength: cv.k("strength"), air: cv.k("air"), bounce: cv.k("bounce"), absorption: 0.24, ..Default::default() }),
                    contact_ao: 0.4,
                    rim: 0.6,
                    ..Default::default()
                }
            }
            Scene::Keys => {
                let (p, k) = self.keylight.update(cv.pointer, now);
                *anim |= self.keylight.moving(now);
                pc.keys = cv.s.keys.iter().map(|a| a.sample(now)).collect();
                *anim |= cv.s.keys.iter().any(|a| !a.is_settled(now));
                pc.bevel = cv.k("bevel");
                pc.gloss = cv.k("gloss");
                let q = cv.px(p);
                out.push(
                    Light::point(q[0], q[1], cv.k("height") * s)
                        .color(srgbf(1.0, 0.93, 0.85))
                        .intensity(3.4)
                        .radius(cv.k("radius") * s)
                        .range(2200.0 * s)
                        .specular(2.4)
                        .scaled(cv.a * k),
                );
                if cv.on("fill") {
                    let q = cv.px([1100.0, 760.0]);
                    out.push(Light::point(q[0], q[1], 700.0 * s).color(srgbf(0.5, 0.62, 1.0)).intensity(0.4).radius(200.0 * s).range(5000.0 * s).specular(0.6).scaled(cv.a));
                }
                Settings { ambient: [0.3, 0.31, 0.34], rim: 1.1, contact_ao: 0.5, ..Default::default() }
            }
            Scene::GodRays => {
                let (p, k) = self.sun.update(cv.pointer, now);
                *anim |= self.sun.moving(now);
                let q = cv.px(p);
                out.push(
                    Light::point(q[0], q[1], cv.k("height") * s)
                        .color(srgbf(1.0, 0.8, 0.55))
                        .intensity(cv.k("intensity"))
                        .radius(12.0 * s)
                        .range(3000.0 * s)
                        .shafts(cv.k("shafts"))
                        .softness(cv.k("softness"))
                        .scaled(cv.a * k),
                );
                Settings { ambient: [0.5, 0.5, 0.58], shafts_over: 0.12, rim: 0.8, contact_ao: 0.35, ..Default::default() }
            }
            _ => Settings::default(),
        }
    }

    /// Effect layers for an effects canvas, anchored to the nodes they decorate.
    pub(super) fn layers(&mut self, cv: &Cv, out: &mut Vec<Lay>, anim: &mut bool) {
        let t = cv.t;
        let sc = cv.sc;
        let mut push = |layer: FxLayer, anchor: Anchor| out.push(Lay { layer: layer.at(t), scene: sc, anchor });
        match sc {
            Scene::Glow => {
                let (r, i, br, rim, hot) = (cv.k("radius") / 14.0, cv.k("intensity") * cv.a, cv.k("breathe"), cv.k("rim_width"), cv.k("hot"));
                let color = cv.swatch();
                push(
                    Fx::glow(GlowParams { radius: 16.0 * r, intensity: 0.85 * i, breathe: br.max(0.4), breathe_period: 2.6, hot: 0.4 * hot / 0.6, ..Default::default() })
                        .on(cv.capsule(stage::GLOW_BTN)),
                    Anchor::Under(node::GLOW_BUTTON),
                );
                let heat = self.neon.at(cv.s.neon_on, t);
                *anim |= self.neon.moving(t);
                push(
                    Fx::glow(GlowParams {
                        style: GlowStyle::Neon,
                        color: rgb(color),
                        core: rgb(0xeafcff),
                        radius: 10.0 * r,
                        intensity: i,
                        inner: 0.35,
                        heat,
                        hot: (0.9 * hot / 0.6).min(1.0),
                        rim_width: rim * 0.55 / 0.6,
                        breathe: br,
                        ..Default::default()
                    })
                    .on(cv.rounded(stage::NEON, 16.0))
                    .over(),
                    Anchor::Over(node::NEON),
                );
                push(
                    Fx::glow(GlowParams {
                        style: GlowStyle::Aura,
                        color: rgb(0xff5fa2),
                        core: rgb(0x8b5cf6),
                        third: rgb(0x38bdf8),
                        radius: 12.0 * r,
                        intensity: 0.9 * i,
                        inner: 0.55,
                        hot: 0.55,
                        aura_speed: 0.15,
                        rim_width: rim,
                        occlusion: 0.1,
                        ..Default::default()
                    })
                    .on(cv.capsule(stage::AURA))
                    .over(),
                    Anchor::Over(node::AURA),
                );
                push(
                    Fx::glow(GlowParams {
                        style: GlowStyle::Mask,
                        color: rgb(0xff8a2a),
                        core: rgb(0xfff1d6),
                        radius: 10.0 * r,
                        intensity: 1.1 * i,
                        hot: 0.0,
                        breathe: br.max(0.25),
                        breathe_period: 3.2,
                        ..Default::default()
                    })
                    .on(cv.rounded(stage::GLOW_TEXT, 0.0))
                    .source(SRC_MASK, cv.viewport),
                    Anchor::Under(node::GLOW_TEXT),
                );
                push(
                    Fx::glow(GlowParams { color: rgb(0xffa630), core: rgb(0xfff6e0), radius: 9.0 * r, intensity: 1.3 * i, hot: (hot / 0.6).min(1.0), breathe: br, ..Default::default() })
                        .on(cv.circle(stage::HOT_DOT)),
                    Anchor::Under(node::HOT_DOT),
                );
            }
            Scene::Pulses => {
                let (reach, width, period, count, i) = (cv.k("reach"), cv.k("width"), cv.k("period"), cv.k("count").round() as u32, cv.k("intensity") * cv.a);
                let color = rgb(cv.swatch());
                push(
                    Fx::pulse(PulseParams { color, core: rgb(0xffffff), reach: reach * 46.0 / 36.0, width: width * 1.3 / 1.6, period: period * 2.2 / 2.4, count, intensity: i, ..Default::default() })
                        .on(cv.circle(stage::RADAR)),
                    Anchor::Under(node::RADAR),
                );
                push(
                    Fx::pulse(PulseParams { style: PulseStyle::Breathe, color: rgb(0x34d399), core: rgb(0xd1fae5), reach: 12.0, period: 2.8, intensity: 0.8 * i, ..Default::default() })
                        .on(cv.circle(stage::AVATAR)),
                    Anchor::Under(node::AVATAR),
                );
                push(
                    Fx::pulse(PulseParams { style: PulseStyle::Heartbeat, color: rgb(0xff3b6b), core: rgb(0xffe4ec), reach: 26.0, width: 1.1, period: 1.1, intensity: i, ..Default::default() })
                        .on(cv.capsule(stage::LIVE)),
                    Anchor::Under(node::LIVE),
                );
                for (shot, k) in cv.s.shots.live(t, 3.0) {
                    let sh = &shot.data;
                    let mut push = |layer: FxLayer, anchor: Anchor| push(layer.opacity(k), anchor);
                    match sh.kind {
                        ShotKind::Ripple => push(
                            Fx::pulse(PulseParams {
                                style: PulseStyle::Ripple { at: cv.w(sh.at) },
                                color,
                                core: rgb(0xffffff),
                                reach: reach * 6.0,
                                width: 2.2 * width / 1.6,
                                t0: sh.t0,
                                duration: 0.9,
                                intensity: 0.7 * i,
                                ..Default::default()
                            })
                            .on(cv.everywhere())
                            .over(),
                            Anchor::Top,
                        ),
                        ShotKind::RippleCard => push(
                            Fx::pulse(PulseParams { style: PulseStyle::Ripple { at: cv.w(sh.at) }, color, core: rgb(0xffffff), reach: 0.0, width: 2.2, t0: sh.t0, duration: 0.9, intensity: 0.7 * i, ..Default::default() })
                                .on(cv.rounded(stage::INBOX, 16.0))
                                .over(),
                            Anchor::Over(node::INBOX),
                        ),
                        ShotKind::Ping => {
                            let from = [stage::SEND[0] + stage::SEND[2], stage::SEND[1]];
                            let to = [stage::INBOX.x + 6.0, stage::INBOX.y + stage::INBOX.h * 0.5];
                            let d = [to[0] - from[0], to[1] - from[1]];
                            let len = d[0].hypot(d[1]);
                            push(
                                Fx::pulse(PulseParams {
                                    style: PulseStyle::Ping { from: cv.w(from), dir: [d[0] / len, d[1] / len], travel: len },
                                    color,
                                    core: rgb(0xf0f7ff),
                                    t0: sh.t0,
                                    duration: stage::PING_DURATION,
                                    lag: 0.16,
                                    head_radius: 2.6,
                                    intensity: i,
                                    ..Default::default()
                                })
                                .on(cv.circle(stage::SEND)),
                                Anchor::Top,
                            );
                            push(
                                Fx::pulse(PulseParams {
                                    style: PulseStyle::Ripple { at: cv.w(to) },
                                    color,
                                    core: rgb(0xffffff),
                                    reach: 0.0,
                                    width: 2.2,
                                    t0: sh.t0 + stage::PING_DURATION,
                                    duration: 0.9,
                                    intensity: 0.7 * i,
                                    ..Default::default()
                                })
                                .on(cv.rounded(stage::INBOX, 16.0))
                                .over(),
                                Anchor::Over(node::INBOX),
                            );
                        }
                        _ => {}
                    }
                }
            }
            Scene::Electric => {
                let (amp, fq, sp, gl, fk) = (cv.k("amplitude"), cv.k("frequency"), cv.k("speed"), cv.k("glow"), cv.k("forks"));
                let color = rgb(cv.swatch());
                let (a, b) = (cv.w([stage::ARC_A[0] + 6.0, stage::ARC_A[1]]), cv.w([stage::ARC_B[0] - 6.0, stage::ARC_B[1]]));
                push(
                    Fx::electric(ElectricParams {
                        style: ElectricStyle::Arc { a, b },
                        color,
                        amplitude: amp * 9.0 / 5.0,
                        frequency: fq,
                        speed: sp,
                        glow: gl * 6.0 / 5.0,
                        forks: (fk * 0.6 / 0.55).min(1.0),
                        strands: 3,
                        intensity: cv.a,
                        ..Default::default()
                    })
                    .on(Shape::rounded(a[0], a[1] - 1.0, b[0] - a[0], 2.0, 0.0)),
                    Anchor::Under(node::TERMINALS),
                );
                for (shot, k) in cv.s.shots.live(t, ElectricParams::ZAP_LIFE).filter(|(sh, _)| sh.data.kind == ShotKind::Zap) {
                    let to = cv.w(shot.data.at);
                    let from = if (to[0] - a[0]).hypot(to[1] - a[1]) < (to[0] - b[0]).hypot(to[1] - b[1]) { a } else { b };
                    push(
                        Fx::electric(ElectricParams {
                            style: ElectricStyle::Arc { a: from, b: to },
                            color,
                            amplitude: amp * 9.0 / 5.0,
                            frequency: fq,
                            glow: gl * 6.0 / 5.0,
                            forks: (fk * 0.6 / 0.55).min(1.0),
                            intensity: cv.a,
                            zap: Some(shot.t0),
                            ..Default::default()
                        })
                        .on(Shape::rounded(from[0].min(to[0]), from[1].min(to[1]), (to[0] - from[0]).abs().max(1.0), (to[1] - from[1]).abs().max(1.0), 0.0))
                        .seed(shot.seed)
                        .opacity(k),
                        Anchor::Top,
                    );
                }
                push(
                    Fx::electric(ElectricParams { color, amplitude: amp * 0.8, frequency: fq, speed: sp, glow: gl, forks: fk, gap: 3.0, fork_len: 22.0, intensity: cv.a, ..Default::default() })
                        .on(cv.rounded(stage::OVERLOAD, 18.0))
                        .seed(7),
                    Anchor::Over(node::OVERLOAD),
                );
                let hovered = cv.pointer.is_some_and(|p| {
                    let r = stage::CHARGED;
                    p[0] >= r.x && p[0] <= r.x + r.w && p[1] >= r.y && p[1] <= r.y + r.h
                });
                let heat = self.charged.at(hovered, t);
                *anim |= self.charged.moving(t);
                if heat > 0.002 {
                    let base = ElectricParams::charged();
                    push(
                        Fx::electric(ElectricParams { color, intensity: base.intensity * heat * cv.a, glow: base.glow * (0.6 + 0.4 * heat), ..base })
                            .on(cv.capsule(stage::CHARGED))
                            .seed(3),
                        Anchor::Over(node::CHARGED),
                    );
                }
            }
            Scene::Fire => {
                let i = cv.k("intensity") * cv.a;
                push(
                    Fx::fire(FireParams {
                        height: cv.k("height"),
                        scale: cv.k("scale"),
                        speed: cv.k("speed"),
                        embers: cv.k("embers").round() as u32,
                        intensity: i,
                        swirl: cv.k("swirl"),
                        ..Default::default()
                    })
                    .on(cv.rounded(stage::FLAMES, 18.0))
                    .seed(5),
                    Anchor::Under(node::FLAMES),
                );
                let pl = FireParams::plasma();
                push(Fx::fire(FireParams { intensity: pl.intensity * i, speed: pl.speed * cv.k("speed") / 1.5, ..pl }).on(cv.capsule(stage::PLASMA)).seed(11), Anchor::Under(node::PLASMA));
                let ht = FireParams::heat();
                push(Fx::fire(FireParams { intensity: ht.intensity * i, ..ht }).on(cv.capsule(stage::DEPLOY)).seed(2), Anchor::Under(node::DEPLOY));
                push(
                    Fx::distort(DistortParams::HeatHaze { height: 26.0, scale: 9.0, speed: 2.0, amount: 1.1 * cv.a, whole_shape: false }).on(cv.capsule(stage::DEPLOY)),
                    Anchor::Over(node::DEPLOY),
                );
            }
            Scene::Lens => {
                let now = cv.now;
                let target = Vec2 { x: cv.s.lens[0] + 128.0, y: cv.s.lens[1] + 8.0 };
                let d = self.droplet.get_or_insert_with(|| Animated::new(target));
                d.animate_to(target, Spring::new(0.5, 0.0), now);
                *anim |= !d.is_settled(now);
                let dp = d.sample(now);
                for (shot, k) in cv.s.shots.live(t, 3.0).filter(|(sh, _)| sh.data.kind == ShotKind::Shockwave) {
                    let sh = &shot.data;
                    push(
                        Fx::distort(DistortParams::shockwave(cv.w(sh.at), sh.t0)).on(cv.everywhere()).opacity(k),
                        Anchor::Top,
                    );
                }
                if cv.on("crt") {
                    push(Fx::distort(DistortParams::crt()).on(cv.rounded(stage::CRT, 14.0)), Anchor::Over(node::CRT));
                }
                push(
                    Fx::distort(DistortParams::ChromaSplit { offset: [1.6, 0.0], glitch: 0.3, feather: 2.0 }).on(cv.rounded(stage::CHROMA, 6.0)),
                    Anchor::Over(node::CHROMA),
                );
                let q = cv.w([dp.x, dp.y]);
                push(
                    Fx::lens(LensParams {
                        depth: cv.k("depth"),
                        bevel: cv.k("bevel"),
                        dispersion: cv.k("dispersion"),
                        magnification: cv.k("magnification"),
                        frost: cv.k("frost"),
                        presence: cv.a,
                        second: Some(Shape::circle(q[0], q[1], 26.0)),
                        union_k: 26.0,
                        ..Default::default()
                    })
                    .on(cv.capsule(stage::lens_rect(cv.s.lens))),
                    Anchor::Top,
                );
            }
            Scene::Extras => {
                if cv.on("shimmer") {
                    push(
                        Fx::shimmer(ShimmerParams { intensity: 0.11 * cv.a, band: 42.0, use_mask: true, ..Default::default() })
                            .on(cv.rounded(stage::SKELETON, 16.0))
                            .source(SRC_MASK, cv.viewport),
                        Anchor::Over(node::SKELETON),
                    );
                    push(
                        Fx::shimmer(ShimmerParams { intensity: 0.85 * cv.a, band: 18.0, period: 1.6, angle: 0.25, use_mask: true, ..Default::default() })
                            .on(cv.rounded(stage::THINKING, 0.0))
                            .source(SRC_MASK, cv.viewport),
                        Anchor::Over(node::THINKING),
                    );
                }
                if cv.on("aurora") {
                    push(
                        Fx::aurora(AuroraParams { intensity: 0.75 * cv.a, scale: 110.0, feather: 18.0, speed: cv.k("speed"), ..Default::default() })
                            .on(cv.rounded(stage::AURORA, 18.0))
                            .over(),
                        Anchor::Over(node::AURORA),
                    );
                }
                if cv.on("grain") {
                    push(Fx::grain(GrainParams { amount: 0.016 * cv.a, ..Default::default() }).on(cv.rounded(stage::AURORA, 18.0)), Anchor::Over(node::AURORA));
                }
                if cv.on("sparkles") {
                    push(
                        Fx::sparkles(SparkleParams { margin: 20.0, size: 8.0, count: 6, intensity: cv.a, ..Default::default() }).on(cv.capsule(stage::PRO)).seed(4),
                        Anchor::Over(node::PRO),
                    );
                }
                let burst = SparkleParams { margin: 34.0, size: 9.0, count: 8, intensity: cv.a, ..Default::default() };
                for (shot, k) in cv.s.shots.live(t, burst.burst_life()).filter(|(sh, _)| sh.data.kind == ShotKind::Sparkles) {
                    push(
                        Fx::sparkles(SparkleParams { burst: Some(shot.t0), ..burst }).on(cv.capsule(stage::PRO)).seed(shot.seed).opacity(k),
                        Anchor::Over(node::PRO),
                    );
                }
            }
            _ => {}
        }
    }
}
