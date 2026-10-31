//! Per-layer state over time and packing into GPU instances.

use crate::*;

impl FxLayer {
    pub fn at(mut self, t: f32) -> Self { self.time = t; self }
    pub fn seed(mut self, s: u32) -> Self { self.seed = s; self }
    pub fn over(mut self) -> Self { self.place = Place::Over; self }
    pub fn under(mut self) -> Self { self.place = Place::Under; self }
    pub fn reduced(mut self, on: bool) -> Self { self.reduced_motion = on; self }
    pub fn source(mut self, slot: u32, rect: Rect) -> Self { self.source = Some(Source { slot, rect }); self }
    pub fn opacity(mut self, k: f32) -> Self { self.opacity = k; self }

    /// Whether this layer still changes over time (the frame loop may stop ticking when false).
    /// A layer that draws nothing at any time (zero opacity or amplitude) is not animating.
    pub fn is_animating(&self) -> bool {
        if self.silent() {
            return false;
        }
        let t = self.time;
        let rm = self.reduced_motion;
        match &self.fx {
            Fx::Glow(p) => (p.breathe > 0.0 && !rm) || p.style == GlowStyle::Aura && p.aura_speed > 0.0 && !rm,
            Fx::Pulse(p) => match p.style {
                PulseStyle::Ping { .. } => p.repeat > 0.0 || t < p.t0 + p.duration * PING_LIFE,
                PulseStyle::Ripple { .. } => t < p.t0 + p.duration,
                _ => !rm,
            },
            Fx::Distort(DistortParams::Shockwave { t0, duration, .. }) => t < t0 + duration,
            Fx::Distort(DistortParams::Lens(_)) => false,
            Fx::Distort(DistortParams::ChromaSplit { glitch, .. }) => *glitch > 0.0 && !rm,
            Fx::MotionBlur(_, s) => s.is_moving(),
            Fx::Grain(g) => g.fps > 0.0 && !rm,
            Fx::Fill(p) => p.animates() && !rm,
            Fx::Electric(ElectricParams { zap: Some(t0), .. }) => t < t0 + ElectricParams::ZAP_LIFE,
            Fx::Sparkles(p @ SparkleParams { burst: Some(t0), .. }) => t < t0 + p.burst_life(),
            _ => !rm,
        }
    }

    /// Whether the layer draws nothing whatever the time: no opacity, or an amplitude of zero.
    fn silent(&self) -> bool {
        if !(self.opacity > 0.0) {
            return true;
        }
        match &self.fx {
            Fx::Glow(p) => p.intensity <= 0.0 || p.heat <= 0.0,
            Fx::Pulse(p) => p.intensity <= 0.0,
            Fx::Electric(p) => p.intensity <= 0.0,
            Fx::Fire(p) => p.intensity <= 0.0,
            Fx::Distort(DistortParams::HeatHaze { amount, .. } | DistortParams::Shockwave { amount, .. }) => *amount <= 0.0,
            Fx::Distort(DistortParams::ChromaSplit { offset, glitch, .. }) => *offset == [0.0, 0.0] && *glitch <= 0.0,
            Fx::Distort(DistortParams::Lens(_) | DistortParams::Crt { .. }) => false,
            Fx::MotionBlur(..) => self.source.is_none(),
            Fx::Shimmer(p) => p.intensity <= 0.0 && p.base_alpha <= 0.0,
            Fx::Sparkles(p) => p.intensity <= 0.0 || p.count == 0,
            Fx::Aurora(p) => p.intensity <= 0.0,
            Fx::Grain(p) => p.amount <= 0.0,
            Fx::Fill(p) => p.draws_nothing(self.shape.rect),
        }
    }
}

/// A ping's life in multiples of its duration: the head arrives at 1, then blooms and cools.
/// `pulse.wgsl` stops drawing a shot at the same age.
const PING_LIFE: f32 = 2.2;

// ---------------------------------------------------------------- packing

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct InstRaw {
    pub rect: [f32; 4],
    pub shape: [f32; 4],
    pub bounds: [f32; 4],
    pub c0: [f32; 4],
    pub c1: [f32; 4],
    pub c2: [f32; 4],
    pub p0: [f32; 4],
    pub p1: [f32; 4],
    pub p2: [f32; 4],
    pub p3: [f32; 4],
    pub t: [f32; 4],
    pub aux: [f32; 4],
    pub aux2: [f32; 4],
    pub xf: [f32; 4],
    pub clip: [f32; 4],
    pub fade: [u32; 4],
}

const KIND_GLOW: usize = 0;
const KIND_PULSE: usize = 1;
const KIND_ELECTRIC: usize = 2;
const KIND_FIRE: usize = 3;
const KIND_DISTORT: usize = 4;
const KIND_MOTION: usize = 5;
const KIND_EXTRAS: usize = 6;
pub(crate) const KIND_FILL: usize = 7;
pub(crate) const KINDS: usize = 8;

/// Blur sigma (pt) of a fully frosted lens (`LensParams::frost = 1`).
pub(crate) const LENS_FROST_SIGMA: f32 = 18.0;

/// Calm pose for looping noise effects under reduced motion: frozen at a fixed, pleasant time.
pub(crate) const CALM_T: f32 = 2.37;

impl FxLayer {
    /// Pack into an instance; `None` when the layer is idle (intensity 0, finished one-shot).
    /// Returns (kind, instance without bounds, bounds in pt).
    pub(crate) fn pack(&self) -> Option<(usize, InstRaw, Rect)> {
        if self.silent() {
            return None;
        }
        let s = self.shape;
        let r = s.rect;
        let mut i = InstRaw {
            rect: r.xywh(),
            shape: [s.radius.min(r.w * 0.5).min(r.h * 0.5).max(0.0), 1.0, self.opacity.min(1.0), 0.0],
            bounds: [0.0; 4],
            c0: [0.0; 4],
            c1: [0.0; 4],
            c2: [0.0; 4],
            p0: [0.0; 4],
            p1: [0.0; 4],
            p2: [0.0; 4],
            p3: [0.0; 4],
            t: [self.time, (self.seed % 65_536) as f32, 0.0, if self.reduced_motion { 1.0 } else { 0.0 }],
            aux: [0.0; 4],
            aux2: self.source.map(|s| s.rect.xywh()).unwrap_or([0.0, 0.0, 1.0, 1.0]),
            xf: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        };
        let rm = self.reduced_motion;
        let looped_t = if rm { CALM_T } else { self.time };
        let (kind, bounds) = match &self.fx {
            Fx::Glow(p) => {
                i.t[2] = match p.style { GlowStyle::Soft => 0.0, GlowStyle::Neon => 1.0, GlowStyle::Aura => 2.0, GlowStyle::Mask => 3.0 };
                i.c0 = p.color.v4();
                i.c1 = p.core.v4();
                i.c2 = p.third.v4();
                i.p0 = [p.radius, p.intensity, p.inner, p.occlusion];
                i.p1 = [p.breathe, p.breathe_period, p.heat.clamp(0.0, 1.0), p.hot];
                i.p2 = [p.aura_speed, p.rim_width, p.reach, 0.0];
                (KIND_GLOW, r.inflate(p.radius * p.reach * (1.0 + 0.22 * p.breathe)))
            }
            Fx::Pulse(p) => {
                i.c0 = p.color.v4();
                i.c1 = p.core.v4();
                i.p0 = [p.count as f32, p.period, p.reach, p.width];
                i.p1 = [p.intensity, p.t0, p.duration, p.repeat];
                i.p2 = [0.0, p.lag, p.head_radius, p.occlusion];
                let (v, b) = match p.style {
                    PulseStyle::Radar => (0.0, r.inflate(p.reach + p.width * 3.2 * 4.0)),
                    PulseStyle::Breathe => (1.0, r.inflate(p.reach * 5.0)),
                    PulseStyle::Heartbeat => (2.0, r.inflate(p.reach + p.width * 16.0 + 2.0)),
                    PulseStyle::Ping { from, dir, travel } => {
                        if p.repeat <= 0.0 && (self.time < p.t0 || self.time > p.t0 + p.duration * PING_LIFE) {
                            return None;
                        }
                        // A zero direction points right rather than nowhere (the shader would normalise it to NaN).
                        let l = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
                        let d = if l > 1e-6 { [dir[0] / l, dir[1] / l] } else { [1.0, 0.0] };
                        i.aux = [from[0], from[1], d[0], d[1]];
                        i.p2[0] = travel;
                        let to = [from[0] + d[0] * travel, from[1] + d[1] * travel];
                        let back = [from[0] - d[0] * travel * 0.06, from[1] - d[1] * travel * 0.06];
                        (3.0, Rect::from_points(back, to).inflate(p.head_radius * 10.0 + 30.0))
                    }
                    PulseStyle::Ripple { at } => {
                        if self.time < p.t0 || self.time > p.t0 + p.duration {
                            return None;
                        }
                        i.aux = [at[0], at[1], 0.0, 0.0];
                        if p.reach <= 0.0 {
                            let far = corners(r).iter().map(|c| ((c[0] - at[0]).powi(2) + (c[1] - at[1]).powi(2)).sqrt()).fold(0.0, f32::max);
                            i.p0[2] = far;
                        }
                        (4.0, r.inflate(1.0))
                    }
                };
                i.t[2] = v;
                (KIND_PULSE, b)
            }
            Fx::Electric(p) => {
                i.t[0] = looped_t;
                if let Some(t0) = p.zap {
                    if self.time < t0 || self.time > t0 + ElectricParams::ZAP_LIFE {
                        return None;
                    }
                    i.t[0] = self.time;
                }
                i.c0 = p.color.v4();
                i.c1 = p.core.v4();
                i.c2 = p.cool.v4();
                i.p0 = [p.amplitude, p.frequency, p.speed, p.core_width];
                i.p1 = [p.intensity, p.glow, p.forks, p.flicker];
                i.p2 = [p.strands.clamp(1, 3) as f32, p.gap, p.fork_len, p.occlusion];
                let m = p.amplitude * 1.7 + p.glow * 5.0;
                match p.style {
                    ElectricStyle::Arc { a, b } => {
                        i.t[2] = 0.0;
                        i.aux = [a[0], a[1], b[0], b[1]];
                        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt().max(1.0);
                        (KIND_ELECTRIC, Rect::from_points(a, b).inflate(p.glow * 5.0 + p.core_width * 3.0 + cpu::arc_reach(p, len)))
                    }
                    ElectricStyle::Border => {
                        i.t[2] = 1.0;
                        (KIND_ELECTRIC, r.inflate(m + p.gap.max(0.0) + p.fork_len * 1.3))
                    }
                    ElectricStyle::Charged => {
                        i.t[2] = 2.0;
                        (KIND_ELECTRIC, r.inflate(m + p.gap.max(0.0) + p.fork_len * 0.6))
                    }
                }
            }
            Fx::Fire(p) => {
                i.t[0] = looped_t;
                i.t[2] = match p.style { FireStyle::Top => 0.0, FireStyle::Border => 1.0, FireStyle::Heat => 0.0, FireStyle::Embers => 3.0 };
                i.c0 = p.low.v4();
                i.c1 = p.mid.v4();
                i.c2 = p.hot.v4();
                i.p0 = [p.height, p.scale, p.speed, p.intensity];
                i.p1 = [if rm { 0.0 } else { p.embers.min(40) as f32 }, p.ember_rise, p.ember_size, p.ember_life];
                i.p2 = [p.warp, p.sides, p.base_glow, p.occlusion];
                i.p3 = [p.swirl, 0.0, 0.0, 0.0];
                // Embers: spawn up to 0.55 H, fly (v0 - vt) / drag, then drift vt for the rest of life.
                let ember_up = if p.embers > 0 && !rm {
                    let life = p.ember_life.max(0.1) * 1.4;
                    p.height * 0.55 + p.ember_rise * (1.48 / cpu::EMBER_DRAG + 0.32 * life) + p.ember_size * 4.0
                } else {
                    0.0
                };
                let ember_side = if p.embers > 0 && !rm { 7.5 * p.ember_life.max(0.1) * 1.4 + 9.0 } else { 0.0 };
                let b = match p.style {
                    FireStyle::Border => r.inflate((p.height * p.sides * 0.3).max(2.5) * 4.6 + 2.0),
                    _ => {
                        // Room above the flames for the smoke they leave (simulated fire).
                        let up = (p.height * 2.6 + 8.0).max(ember_up) + 4.0;
                        Rect::new(r.x - ember_side, r.y - up, r.w + 2.0 * ember_side, up + 4.0)
                    }
                };
                (KIND_FIRE, b)
            }
            Fx::Distort(d) => {
                let b = match *d {
                    DistortParams::HeatHaze { height, scale, speed, amount, whole_shape } => {
                        i.t[0] = looped_t;
                        i.t[2] = 0.0;
                        i.p0 = [height, scale, speed, amount];
                        i.p1 = [whole_shape as u32 as f32, 0.0, 0.0, 0.0];
                        if whole_shape { r.inflate(amount + 2.0) } else { Rect::new(r.x - 4.0, r.y - height - 4.0, r.w + 8.0, height + 12.0) }
                    }
                    DistortParams::Shockwave { at, reach, width, amount, chroma, t0, duration } => {
                        if self.time < t0 || self.time > t0 + duration {
                            return None;
                        }
                        i.t[2] = 1.0;
                        i.aux = [at[0], at[1], 0.0, 0.0];
                        i.p0 = [reach, width, amount, chroma];
                        i.p1 = [t0, duration, 0.0, 0.0];
                        Rect::new(at[0], at[1], 0.0, 0.0).inflate(reach + width * 5.0 + amount)
                    }
                    DistortParams::Lens(l) => {
                        i.t[2] = 2.0;
                        i.c0 = l.tint.v4();
                        i.p0 = [l.bevel, l.depth, l.dispersion, l.magnification];
                        i.p1 = [l.presence.clamp(0.0, 1.0), l.frost, l.shadow, l.rim];
                        let mut b = r;
                        if let Some(s2) = l.second {
                            i.p2 = [l.light_angle, l.tint_amount, l.union_k, s2.radius.min(s2.rect.w * 0.5).min(s2.rect.h * 0.5)];
                            i.p3 = s2.rect.xywh();
                            i.shape[3] = l.morph;
                            b = b.union(s2.rect);
                        } else {
                            i.p2 = [l.light_angle, l.tint_amount, 0.0, 0.0];
                        }
                        b.inflate(28.0)
                    }
                    DistortParams::Crt { bulge, scanline, scan_depth, chroma, vignette, glow, roll, feather } => {
                        i.t[2] = 3.0;
                        i.p0 = [bulge, scanline, scan_depth, chroma];
                        i.p1 = [vignette, glow, roll, feather];
                        r.inflate(feather)
                    }
                    DistortParams::ChromaSplit { offset, glitch, feather } => {
                        i.t[2] = 4.0;
                        i.p0 = [offset[0], offset[1], glitch, feather];
                        r.inflate(feather)
                    }
                };
                (KIND_DISTORT, b)
            }
            Fx::MotionBlur(_, ms) => {
                i.xf = [0.0, ms.count as f32, if ms.mode == BlurMode::Shutter { 0.0 } else { 1.0 }, 0.0];
                i.aux = [ms.blur[0], ms.blur[1], 0.0, 0.0];
                (KIND_MOTION, ms.bounds.inflate(2.0))
            }
            Fx::Shimmer(p) => {
                i.t[2] = 0.0;
                i.c0 = p.color.v4();
                i.c1 = [p.base.0[0], p.base.0[1], p.base.0[2], p.base_alpha];
                i.p0 = [p.period, p.band, p.angle, p.intensity];
                i.p1 = [p.use_mask as u32 as f32, p.pause, p.feather, 0.0];
                (KIND_EXTRAS, r.inflate(1.0))
            }
            Fx::Sparkles(p) => {
                if let Some(t0) = p.burst
                    && (self.time < t0 || self.time > t0 + p.burst_life())
                {
                    return None;
                }
                i.t[2] = 1.0;
                i.c0 = p.color.v4();
                i.p0 = [p.count.min(64) as f32, p.life, p.size, p.intensity];
                i.p1 = [p.margin, 0.0, 0.0, 0.0];
                (KIND_EXTRAS, r.inflate(p.margin + p.size * cpu::SPARKLE_MAX_SCALE + 2.0))
            }
            Fx::Aurora(p) => {
                i.t[2] = 2.0;
                i.c0 = p.a.v4();
                i.c1 = p.b.v4();
                i.c2 = p.c.v4();
                i.p0 = [p.scale, p.speed, p.intensity, p.feather];
                (KIND_EXTRAS, r.inflate(1.0))
            }
            Fx::Grain(p) => {
                i.t[2] = 3.0;
                i.p0 = [p.amount, p.fps, p.feather, 0.0];
                (KIND_EXTRAS, r.inflate(1.0))
            }
            Fx::Fill(p) => (KIND_FILL, p.pack(&mut i, rm, self.time)?),
        };
        Some((kind, i, bounds))
    }

    /// Second stage, only for layers that survived culling: per-frame CPU tables into `data`.
    pub(crate) fn fill(&self, i: &mut InstRaw, data: &mut Vec<[f32; 4]>) {
        let t = i.t[0];
        let rm = self.reduced_motion;
        match &self.fx {
            Fx::MotionBlur(_, ms) => {
                i.xf[0] = data.len() as f32;
                for x in &ms.xforms[..ms.count as usize] {
                    data.push([x[0], x[1], x[2], x[3]]);
                    data.push([x[4], x[5], x[6], x[7]]);
                }
            }
            Fx::Electric(p) => i.xf = cpu::electric(p, self.shape, t, self.seed, rm, data),
            Fx::Fire(p) => {
                if !rm && p.embers > 0 {
                    let (f, n) = cpu::embers(p, self.shape.rect, i.shape[0], t, self.seed, data);
                    i.xf = [f, n, 0.0, 0.0];
                }
            }
            Fx::Sparkles(p) => {
                let (f, n) = cpu::sparkles(p, self.shape.rect, t, self.seed, rm, data);
                i.xf = [f, n, 0.0, 0.0];
            }
            Fx::Aurora(p) => {
                let tt = if rm { CALM_T } else { t };
                i.xf = [cpu::aurora(p, self.shape.rect, tt, self.seed, data), cpu::AURORA_SAMPLES as f32, 0.0, 0.0];
            }
            Fx::Fill(p) => p.tables(i, self.shape.rect, t, self.seed, data),
            _ => {}
        }
    }
}
