use crate::{Rect, Rgb, Shape, cpu, rgb};

// ---------------------------------------------------------------- knobs

/// A tunable parameter with its range, for knobs panels.
#[derive(Clone, Copy, Debug)]
pub struct Knob {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

const fn k(name: &'static str, min: f32, max: f32, default: f32) -> Knob {
    Knob { name, min, max, default }
}

// ---------------------------------------------------------------- params

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlowStyle {
    /// Soft bloom outside the shape.
    Soft,
    /// Neon tube: white-hot hairline core on the border, coloured bloom both sides.
    Neon,
    /// Spectrum flowing around the border (inner rim light + outer bloom).
    Aura,
    /// Bloom of a mask texture (text, icons): needs `FxLayer::source` pointing at a `MaskPyramid`.
    Mask,
}

#[derive(Clone, Copy, Debug)]
pub struct GlowParams {
    pub style: GlowStyle,
    pub color: Rgb,
    pub core: Rgb,
    /// Aura only: the third palette colour (`color`, `core`, `third` flow around the border).
    pub third: Rgb,
    pub radius: f32,
    pub intensity: f32,
    pub inner: f32,
    pub occlusion: f32,
    pub breathe: f32,
    pub breathe_period: f32,
    /// 1 = fully lit; animate toward 0 to cool (see `Heat`).
    pub heat: f32,
    pub hot: f32,
    pub aura_speed: f32,
    pub rim_width: f32,
    pub reach: f32,
}

impl Default for GlowParams {
    fn default() -> Self {
        GlowParams {
            style: GlowStyle::Soft,
            color: rgb(0x7c5cff),
            core: rgb(0xf4f0ff),
            third: rgb(0x22d3ee),
            radius: 14.0,
            intensity: 1.0,
            inner: 0.0,
            occlusion: 0.15,
            breathe: 0.0,
            breathe_period: 2.6,
            heat: 1.0,
            hot: 0.6,
            aura_speed: 0.12,
            rim_width: 0.6,
            reach: 4.5,
        }
    }
}

impl GlowParams {
    pub const KNOBS: &'static [Knob] = &[
        k("radius", 1.0, 64.0, 14.0),
        k("intensity", 0.0, 3.0, 1.0),
        k("inner", 0.0, 1.0, 0.0),
        k("occlusion", 0.0, 1.0, 0.15),
        k("breathe", 0.0, 1.0, 0.0),
        k("breathe_period", 0.4, 8.0, 2.6),
        k("heat", 0.0, 1.0, 1.0),
        k("hot", 0.0, 1.0, 0.6),
        k("aura_speed", 0.0, 1.0, 0.12),
        k("rim_width", 0.2, 3.0, 0.6),
        k("reach", 2.0, 8.0, 4.5),
    ];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PulseStyle {
    /// Rings expand outward from the shape edge.
    Radar,
    /// A halo that breathes.
    Breathe,
    /// Lub-dub glow with a short ring per beat.
    Heartbeat,
    /// A comet that launches from `from` along `dir`, travels `travel` pt with anticipation and a
    /// decelerating landing, then blooms and cools at its destination.
    Ping { from: [f32; 2], dir: [f32; 2], travel: f32 },
    /// Ripple from a tap point, confined to the shape.
    Ripple { at: [f32; 2] },
}

#[derive(Clone, Copy, Debug)]
pub struct PulseParams {
    pub style: PulseStyle,
    pub color: Rgb,
    pub core: Rgb,
    pub count: u32,
    pub period: f32,
    pub reach: f32,
    pub width: f32,
    pub intensity: f32,
    /// One-shots (ping, ripple): start time on the layer's clock.
    pub t0: f32,
    pub duration: f32,
    /// Ping: repeat interval (0 = once).
    pub repeat: f32,
    pub lag: f32,
    pub head_radius: f32,
    pub occlusion: f32,
}

impl Default for PulseParams {
    fn default() -> Self {
        PulseParams {
            style: PulseStyle::Radar,
            color: rgb(0x3b82f6),
            core: rgb(0xe8f1ff),
            count: 3,
            period: 2.4,
            reach: 36.0,
            width: 1.6,
            intensity: 1.0,
            t0: 0.0,
            duration: 0.75,
            repeat: 0.0,
            lag: 0.16,
            head_radius: 2.6,
            occlusion: 0.15,
        }
    }
}

impl PulseParams {
    pub const KNOBS: &'static [Knob] = &[
        k("count", 1.0, 6.0, 3.0),
        k("period", 0.3, 6.0, 2.4),
        k("reach", 4.0, 200.0, 36.0),
        k("width", 0.3, 8.0, 1.6),
        k("intensity", 0.0, 3.0, 1.0),
        k("duration", 0.1, 3.0, 0.75),
        k("repeat", 0.0, 6.0, 0.0),
        k("lag", 0.02, 0.5, 0.16),
        k("head_radius", 0.5, 12.0, 2.6),
        k("occlusion", 0.0, 1.0, 0.15),
    ];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ElectricStyle {
    Arc { a: [f32; 2], b: [f32; 2] },
    Border,
    /// Calm UI variant: low amplitude, slow crawl, travelling beads of charge, rare tiny sparks.
    Charged,
}

#[derive(Clone, Copy, Debug)]
pub struct ElectricParams {
    pub style: ElectricStyle,
    pub color: Rgb,
    pub core: Rgb,
    pub cool: Rgb,
    pub amplitude: f32,
    pub frequency: f32,
    pub speed: f32,
    pub core_width: f32,
    pub intensity: f32,
    pub glow: f32,
    pub forks: f32,
    pub flicker: f32,
    pub strands: u32,
    pub gap: f32,
    pub fork_len: f32,
    pub occlusion: f32,
    /// One strike at this time (layer clock) instead of a continuous arc: it flashes, may
    /// re-strike down the same channel, then its afterimage fades within `ZAP_LIFE`. Overlapping
    /// zaps: one layer per `OneShots` instance, seeded with the instance's seed.
    pub zap: Option<f32>,
}

impl Default for ElectricParams {
    fn default() -> Self {
        ElectricParams {
            style: ElectricStyle::Border,
            color: rgb(0x4f8dff),
            core: rgb(0xeaf3ff),
            cool: rgb(0x7a4dff),
            amplitude: 5.0,
            frequency: 2.4,
            speed: 3.2,
            core_width: 0.6,
            intensity: 1.0,
            glow: 5.0,
            forks: 0.55,
            flicker: 0.45,
            strands: 2,
            gap: 2.0,
            fork_len: 20.0,
            occlusion: 0.12,
            zap: None,
        }
    }
}

impl ElectricParams {
    /// How long a zap lasts (s).
    pub const ZAP_LIFE: f32 = 0.5;
    pub const KNOBS: &'static [Knob] = &[
        k("amplitude", 0.0, 24.0, 5.0),
        k("frequency", 0.5, 8.0, 2.4),
        k("speed", 0.0, 10.0, 3.2),
        k("core_width", 0.2, 3.0, 0.6),
        k("intensity", 0.0, 3.0, 1.0),
        k("glow", 1.0, 20.0, 5.0),
        k("forks", 0.0, 1.0, 0.55),
        k("flicker", 0.0, 1.0, 0.45),
        k("strands", 1.0, 3.0, 2.0),
        k("gap", -6.0, 12.0, 2.0),
        k("fork_len", 2.0, 60.0, 20.0),
        k("occlusion", 0.0, 1.0, 0.12),
    ];
    /// The calm border for UI: a charged hairline that crawls.
    pub fn charged() -> ElectricParams {
        ElectricParams {
            style: ElectricStyle::Charged,
            amplitude: 1.1,
            frequency: 3.0,
            speed: 2.0,
            core_width: 0.5,
            glow: 4.0,
            forks: 0.25,
            flicker: 0.0,
            strands: 1,
            gap: 0.5,
            fork_len: 14.0,
            intensity: 0.9,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FireStyle {
    Top,
    Border,
    /// Subtle warm tongues for UI (use `FireParams::heat()`).
    Heat,
    Embers,
}

#[derive(Clone, Copy, Debug)]
pub struct FireParams {
    pub style: FireStyle,
    pub low: Rgb,
    pub mid: Rgb,
    pub hot: Rgb,
    pub height: f32,
    pub scale: f32,
    pub speed: f32,
    pub intensity: f32,
    pub embers: u32,
    pub ember_rise: f32,
    pub ember_size: f32,
    pub ember_life: f32,
    pub warp: f32,
    pub sides: f32,
    pub base_glow: f32,
    pub occlusion: f32,
    pub swirl: f32,
    /// How much smoke the cooling flame leaves (simulated fire only; see `FxRenderer::set_fire_sim`).
    pub smoke: f32,
}

impl Default for FireParams {
    fn default() -> Self {
        FireParams {
            style: FireStyle::Top,
            low: rgb(0x8a1405),
            mid: rgb(0xff5a14),
            hot: rgb(0xffc65a),
            height: 54.0,
            scale: 20.0,
            speed: 1.5,
            intensity: 1.0,
            embers: 16,
            ember_rise: 58.0,
            ember_size: 0.8,
            ember_life: 1.5,
            warp: 1.0,
            sides: 0.32,
            base_glow: 0.5,
            occlusion: 0.25,
            swirl: 0.0,
            smoke: 0.5,
        }
    }
}

impl FireParams {
    pub const KNOBS: &'static [Knob] = &[
        k("height", 4.0, 160.0, 54.0),
        k("scale", 4.0, 80.0, 20.0),
        k("speed", 0.0, 5.0, 1.5),
        k("intensity", 0.0, 3.0, 1.0),
        k("embers", 0.0, 40.0, 16.0),
        k("ember_rise", 0.0, 200.0, 58.0),
        k("ember_size", 0.2, 3.0, 0.8),
        k("ember_life", 0.2, 4.0, 1.5),
        k("warp", 0.0, 3.0, 1.0),
        k("sides", 0.0, 1.0, 0.32),
        k("base_glow", 0.0, 2.0, 0.5),
        k("occlusion", 0.0, 1.0, 0.25),
        k("swirl", 0.0, 3.0, 0.0),
        k("smoke", 0.0, 2.0, 0.5),
    ];
    pub fn heat() -> FireParams {
        FireParams {
            style: FireStyle::Heat,
            low: rgb(0x7a1606),
            mid: rgb(0xf0601a),
            hot: rgb(0xffc062),
            height: 15.0,
            scale: 7.0,
            speed: 1.3,
            intensity: 0.8,
            embers: 4,
            ember_rise: 30.0,
            ember_size: 0.55,
            ember_life: 1.1,
            warp: 0.8,
            base_glow: 0.35,
            occlusion: 0.1,
            ..Default::default()
        }
    }
    /// Plasma: cool palette, swirling, around the border.
    pub fn plasma() -> FireParams {
        FireParams {
            style: FireStyle::Border,
            low: rgb(0x3a1690),
            mid: rgb(0x7b5cff),
            hot: rgb(0x9fe8ff),
            height: 30.0,
            scale: 16.0,
            speed: 1.1,
            intensity: 0.95,
            embers: 0,
            warp: 1.0,
            sides: 0.55,
            base_glow: 0.6,
            swirl: 0.8,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LensParams {
    pub bevel: f32,
    pub depth: f32,
    pub dispersion: f32,
    pub magnification: f32,
    /// 0..1: glass materialises by lensing (refraction, rim and shadow ramp together), not fading.
    pub presence: f32,
    pub frost: f32,
    pub shadow: f32,
    pub rim: f32,
    pub light_angle: f32,
    pub tint: Rgb,
    pub tint_amount: f32,
    /// A second glass shape smooth-unioned with the first (gooey merge), or `None`.
    pub second: Option<Shape>,
    pub union_k: f32,
    /// 0..1: morph the union toward `second` alone.
    pub morph: f32,
}

impl Default for LensParams {
    fn default() -> Self {
        LensParams {
            bevel: 14.0,
            depth: 16.0,
            dispersion: 0.14,
            magnification: 1.04,
            presence: 1.0,
            frost: 0.0,
            shadow: 0.35,
            rim: 1.6,
            light_angle: -2.09,
            tint: rgb(0xffffff),
            tint_amount: 0.0,
            second: None,
            union_k: 22.0,
            morph: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum DistortParams {
    HeatHaze { height: f32, scale: f32, speed: f32, amount: f32, whole_shape: bool },
    Shockwave { at: [f32; 2], reach: f32, width: f32, amount: f32, chroma: f32, t0: f32, duration: f32 },
    Lens(LensParams),
    Crt { bulge: f32, scanline: f32, scan_depth: f32, chroma: f32, vignette: f32, glow: f32, roll: f32, feather: f32 },
    ChromaSplit { offset: [f32; 2], glitch: f32, feather: f32 },
}

impl DistortParams {
    pub fn heat_haze() -> Self {
        DistortParams::HeatHaze { height: 40.0, scale: 14.0, speed: 2.2, amount: 1.6, whole_shape: false }
    }
    pub fn shockwave(at: [f32; 2], t0: f32) -> Self {
        DistortParams::Shockwave { at, reach: 160.0, width: 14.0, amount: 9.0, chroma: 0.35, t0, duration: 0.9 }
    }
    pub fn crt() -> Self {
        DistortParams::Crt { bulge: 0.08, scanline: 1.5, scan_depth: 0.28, chroma: 0.8, vignette: 0.6, glow: 0.35, roll: 1.0, feather: 2.0 }
    }
    pub fn chroma_split(offset: [f32; 2]) -> Self {
        DistortParams::ChromaSplit { offset, glitch: 0.0, feather: 2.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlurMode {
    /// True shutter blur: exact sub-frame poses from the analytic track.
    Shutter,
    /// Cheap fallback: taps along the instantaneous velocity from the current pose only (no track).
    Directional,
}

#[derive(Clone, Copy, Debug)]
pub struct MotionBlurParams {
    pub mode: BlurMode,
    /// Degrees of the frame interval the shutter is open (180 = film look).
    pub shutter_angle: f32,
    pub frame_dt: f32,
    pub max_samples: u32,
    /// Target spacing between sub-frame samples, in device pixels.
    pub px_per_sample: f32,
    /// Raised-cosine shutter (soft trail ends) instead of a box.
    pub smooth_shutter: bool,
}

impl Default for MotionBlurParams {
    fn default() -> Self {
        MotionBlurParams {
            mode: BlurMode::Shutter,
            shutter_angle: 180.0,
            frame_dt: 1.0 / 60.0,
            max_samples: 48,
            px_per_sample: 1.25,
            smooth_shutter: true,
        }
    }
}

impl MotionBlurParams {
    pub const KNOBS: &'static [Knob] = &[
        k("shutter_angle", 0.0, 360.0, 180.0),
        k("max_samples", 1.0, 64.0, 48.0),
        k("px_per_sample", 0.5, 6.0, 1.25),
    ];
}

/// A node's pose relative to its rest rect: translate, rotate and scale about the rect centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub scale: f32,
    pub opacity: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Pose { x: 0.0, y: 0.0, rotation: 0.0, scale: 1.0, opacity: 1.0 }
    }
}

impl Pose {
    /// Forward affine (a, b, c, d, tx, ty): screen = [a c; b d] * local + t.
    pub(crate) fn affine(&self, rest: Rect) -> [f32; 6] {
        let rune_motion::Vec2 { x: cx, y: cy } = rest.center();
        let (s, c) = self.rotation.sin_cos();
        let a = c * self.scale;
        let b = s * self.scale;
        let cc = -s * self.scale;
        let d = c * self.scale;
        let tx = cx + self.x - (a * cx + cc * cy);
        let ty = cy + self.y - (b * cx + d * cy);
        [a, b, cc, d, tx, ty]
    }
    pub fn apply(&self, rest: Rect, p: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d, tx, ty] = self.affine(rest);
        [a * p[0] + c * p[1] + tx, b * p[0] + d * p[1] + ty]
    }
}

pub const MAX_MOTION_SAMPLES: usize = 64;

/// Sub-frame poses for one frame, sampled from the node's analytic track.
#[derive(Clone, Copy, Debug)]
pub struct MotionSamples {
    pub count: u32,
    pub mode: BlurMode,
    /// Per sample: inverse affine (ia, ib, ic, id, itx, ity), weight * opacity, 0.
    pub xforms: [[f32; 8]; MAX_MOTION_SAMPLES],
    /// Union of all sampled node rects (pt).
    pub bounds: Rect,
    /// Directional mode: blur vector over the whole shutter (pt).
    pub blur: [f32; 2],
    /// Largest corner travel across the shutter, in device px.
    pub travel_px: f32,
}

pub(crate) fn invert(m: [f32; 6]) -> [f32; 6] {
    let [a, b, c, d, tx, ty] = m;
    let det = a * d - b * c;
    let id = 1.0 / if det.abs() < 1e-9 { 1e-9 } else { det };
    let ia = d * id;
    let ib = -b * id;
    let ic = -c * id;
    let idd = a * id;
    [ia, ib, ic, idd, -(ia * tx + ic * ty), -(ib * tx + idd * ty)]
}

pub(crate) fn corners(r: Rect) -> [[f32; 2]; 4] {
    [[r.x, r.y], [r.x + r.w, r.y], [r.x, r.y + r.h], [r.x + r.w, r.y + r.h]]
}

impl MotionSamples {
    /// Sample `track` (a pose for any time) inside the shutter interval centred on `t`.
    /// The sample count scales with on-screen travel: 1 at rest, so a resting node costs nothing
    /// extra (`is_moving()` is false and the caller draws the node normally).
    pub fn from_track(p: &MotionBlurParams, rest: Rect, t: f32, scale: f32, track: impl Fn(f32) -> Pose) -> MotionSamples {
        let open = p.frame_dt * p.shutter_angle / 360.0;
        let t0 = t - open * 0.5;
        let t1 = t + open * 0.5;
        let pa = track(t0);
        let pb = track(t1);
        let pm = track(t);
        // Travel: max corner displacement between open, middle and close (catches rotation arcs).
        let mut travel = 0.0f32;
        for c in corners(rest) {
            let a = pa.apply(rest, c);
            let m = pm.apply(rest, c);
            let b = pb.apply(rest, c);
            let d1 = ((m[0] - a[0]).powi(2) + (m[1] - a[1]).powi(2)).sqrt();
            let d2 = ((b[0] - m[0]).powi(2) + (b[1] - m[1]).powi(2)).sqrt();
            travel = travel.max(d1 + d2);
        }
        let travel_px = travel * scale;
        let mut s = MotionSamples {
            count: 1,
            mode: p.mode,
            xforms: [[0.0; 8]; MAX_MOTION_SAMPLES],
            bounds: rest,
            blur: [0.0, 0.0],
            travel_px,
        };
        let max = (p.max_samples as usize).clamp(1, MAX_MOTION_SAMPLES);
        let want = ((travel_px / p.px_per_sample.max(0.25)).ceil() as usize).clamp(1, max);
        let put = |s: &mut MotionSamples, i: usize, pose: &Pose, w: f32| {
            let inv = invert(pose.affine(rest));
            s.xforms[i] = [inv[0], inv[1], inv[2], inv[3], inv[4], inv[5], w * pose.opacity, 0.0];
            let mut b: Option<Rect> = None;
            for c in corners(rest) {
                let q = pose.apply(rest, c);
                let r = Rect::new(q[0], q[1], 0.0, 0.0);
                b = Some(b.map_or(r, |bb| bb.union(r)));
            }
            s.bounds = if i == 0 { b.unwrap() } else { s.bounds.union(b.unwrap()) };
        };
        if want <= 1 || open <= 0.0 {
            put(&mut s, 0, &pm, 1.0);
            return s;
        }
        match p.mode {
            BlurMode::Shutter => {
                let mut w = [0.0f32; MAX_MOTION_SAMPLES];
                let mut sum = 0.0;
                for (i, wi) in w.iter_mut().enumerate().take(want) {
                    let u = (i as f32 + 0.5) / want as f32;
                    *wi = if p.smooth_shutter { 0.5 - 0.5 * (std::f32::consts::TAU * u).cos() } else { 1.0 };
                    sum += *wi;
                }
                for i in 0..want {
                    let u = (i as f32 + 0.5) / want as f32;
                    let pose = track(t0 + (t1 - t0) * u);
                    put(&mut s, i, &pose, w[i] / sum);
                }
                s.count = want as u32;
            }
            BlurMode::Directional => {
                put(&mut s, 0, &pm, 1.0);
                s.blur = [pb.x - pa.x, pb.y - pa.y];
                let taps = want.min(32);
                s.count = taps as u32;
                // Bounds: current rect swept along the blur vector.
                let half = [s.blur[0] * 0.5, s.blur[1] * 0.5];
                let r = s.bounds;
                let a = Rect::new(r.x - half[0], r.y - half[1], r.w, r.h);
                let b = Rect::new(r.x + half[0], r.y + half[1], r.w, r.h);
                s.bounds = a.union(b);
            }
        }
        s
    }
    pub fn is_moving(&self) -> bool {
        self.count > 1
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ShimmerParams {
    pub color: Rgb,
    pub base: Rgb,
    pub base_alpha: f32,
    pub period: f32,
    pub band: f32,
    pub angle: f32,
    pub intensity: f32,
    /// Sweep a mask texture (text) instead of the shape: needs `FxLayer::source`.
    pub use_mask: bool,
    pub pause: f32,
    pub feather: f32,
}

impl Default for ShimmerParams {
    fn default() -> Self {
        ShimmerParams {
            color: rgb(0xffffff),
            base: rgb(0x1c1f26),
            base_alpha: 0.0,
            period: 1.8,
            band: 28.0,
            angle: 0.35,
            intensity: 0.16,
            use_mask: false,
            pause: 0.3,
            feather: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SparkleParams {
    pub color: Rgb,
    pub count: u32,
    pub life: f32,
    pub size: f32,
    pub intensity: f32,
    pub margin: f32,
    /// A one-shot burst starting at this time (layer clock): every star pops once, flies a little
    /// way out from the shape and twinkles out within `burst_life()`. `None` twinkles forever.
    pub burst: Option<f32>,
}

impl Default for SparkleParams {
    fn default() -> Self {
        SparkleParams { color: rgb(0xffe6a8), count: 6, life: 1.5, size: 7.0, intensity: 1.0, margin: 10.0, burst: None }
    }
}

impl SparkleParams {
    /// Seconds from the start of a burst until its last star is gone.
    pub fn burst_life(&self) -> f32 {
        cpu::SPARKLE_STAGGER + self.life * cpu::BURST_LIFE * 1.3
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AuroraParams {
    pub a: Rgb,
    pub b: Rgb,
    pub c: Rgb,
    pub scale: f32,
    pub speed: f32,
    pub intensity: f32,
    pub feather: f32,
}

impl Default for AuroraParams {
    fn default() -> Self {
        AuroraParams {
            a: rgb(0x16c79a),
            b: rgb(0x3b6cff),
            c: rgb(0xa23bff),
            scale: 120.0,
            speed: 0.25,
            intensity: 0.5,
            feather: 40.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GrainParams {
    pub amount: f32,
    /// 0 = static grain (no temporal shimmer); otherwise re-rolls at this rate.
    pub fps: f32,
    pub feather: f32,
}

impl Default for GrainParams {
    fn default() -> Self {
        GrainParams { amount: 0.03, fps: 0.0, feather: 0.0 }
    }
}

