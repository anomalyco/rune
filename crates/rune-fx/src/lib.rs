//! rune-fx: composable GPU effect layers for Rune (glow, pulses, electricity, fire, motion blur,
//! distortion, shimmer, sparkles, aurora, grain) and fills (liquid metal, mesh gradient, dither,
//! grain gradient, fluted glass and more; see `fill_styles()`).
//!
//! Contract (see README): a layer = effect params + node shape (rounded rect / capsule / circle,
//! in points) + time (Rune's clock) + seed (+ optional source texture, backdrop, motion samples).
//! Output is a premultiplied, linear-light colour layer drawn over or under the node.
//! Invariants for the next agent:
//! - Every effect is a pure function of (pixel, params, time, seed). No hidden state, no RNG.
//! - Intensity 0 or bounds off the viewport => the layer is culled in `prepare` (no draw, no cost).
//! - Speeds never multiply raw time inside a shader without going through `Phase` on the caller side
//!   when the speed animates; otherwise the phase jumps (no phase jump on speed change).

mod binds;
mod catalog;
mod cpu;
mod fill;
mod pack;
mod renderer;
mod sim;
pub use catalog::*;
pub use fill::*;
pub(crate) use pack::*;
pub use renderer::*;

// ---------------------------------------------------------------- geometry / colour

pub use rune_motion::Rect;

/// The node's shape, in points. Capsules and circles are rounded rects with radius = min(w, h) / 2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub rect: Rect,
    pub radius: f32,
}

impl Shape {
    pub fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Shape {
        Shape { rect: Rect::new(x, y, w, h), radius }
    }
    pub fn capsule(x: f32, y: f32, w: f32, h: f32) -> Shape {
        Shape { rect: Rect::new(x, y, w, h), radius: w.min(h) * 0.5 }
    }
    pub fn circle(cx: f32, cy: f32, r: f32) -> Shape {
        Shape { rect: Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), radius: r }
    }
}

/// Linear-light RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub [f32; 3]);

/// sRGB hex (0xRRGGBB) to linear.
pub fn rgb(hex: u32) -> Rgb {
    let f = |s: u32| rune_motion::srgb_decode(((hex >> s) & 0xff) as f32 / 255.0);
    Rgb([f(16), f(8), f(0)])
}

impl Rgb {
    fn v4(self) -> [f32; 4] {
        [self.0[0], self.0[1], self.0[2], 1.0]
    }
}

// ---------------------------------------------------------------- time helpers

/// Continuous phase clock for effects whose speed animates: `at(t)` never jumps when `set_speed`
/// is called (no phase jump when a speed changes).
#[derive(Clone, Copy, Debug)]
pub struct Phase {
    pub speed: f32,
    pub offset: f32,
}

impl Phase {
    pub fn new(speed: f32) -> Phase {
        Phase { speed, offset: 0.0 }
    }
    /// Effect time to pass as the layer's time (the shaders multiply by their own nominal rates).
    pub fn at(&self, t: f32) -> f32 {
        t * self.speed + self.offset
    }
    pub fn set_speed(&mut self, t: f32, speed: f32) {
        self.offset += t * (self.speed - speed);
        self.speed = speed;
    }
}

/// Heat of a light that is switched on/off: rises over `attack` s, cools exponentially with
/// time constant `cool` s once off. Feed into `GlowParams::heat`: lights cool, never cut.
#[derive(Clone, Copy, Debug)]
pub struct Heat {
    pub attack: f32,
    pub cool: f32,
    /// (time switched, heat at that time, on?)
    last: (f32, f32, bool),
}

impl Heat {
    pub fn new(attack: f32, cool: f32) -> Heat {
        Heat { attack, cool, last: (0.0, 0.0, false) }
    }
    pub fn set(&mut self, t: f32, on: bool) {
        if on != self.last.2 {
            self.last = (t, self.at(t), on);
        }
    }
    pub fn at(&self, t: f32) -> f32 {
        let (t0, h0, on) = self.last;
        let dt = (t - t0).max(0.0);
        if on {
            let k = (dt / self.attack.max(1e-3)).min(1.0);
            let e = 1.0 - (1.0 - k).powi(3);
            h0 + (1.0 - h0) * e
        } else {
            h0 * (-dt / self.cool.max(1e-3)).exp()
        }
    }
    /// Lit enough to keep ticking frames.
    pub fn is_active(&self, t: f32) -> bool {
        self.last.2 || self.at(t) > 1.0 / 512.0
    }
}

/// Overlapping one-shot effects: taps, pings, shockwaves, zaps, sparkle bursts.
///
/// Every `trigger` starts its own instance, which lives out its whole life beside the ones already
/// running, so a quick double tap shows two ripples instead of restarting one. The pool is bounded:
/// when `cap` instances are live, the oldest one fades out over `FADE` seconds instead of being cut.
/// Feed each live instance's opacity to `FxLayer::opacity`.
#[derive(Clone, Debug)]
pub struct OneShots<T> {
    cap: usize,
    shots: Vec<OneShot<T>>,
    count: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct OneShot<T> {
    /// Start time on the layer's clock.
    pub t0: f32,
    pub data: T,
    /// Distinct per instance, so overlapping instances don't look identical.
    pub seed: u32,
    retired: Option<f32>,
}

impl<T: Copy> OneShots<T> {
    /// How long a retired instance takes to fade out.
    pub const FADE: f32 = 0.16;

    pub fn new(cap: usize) -> Self {
        OneShots { cap: cap.max(1), shots: Vec::new(), count: 0 }
    }

    pub fn trigger(&mut self, t: f32, data: T) {
        self.shots.retain(|s| s.retired.is_none_or(|r| t - r < Self::FADE));
        let active = self.shots.iter().filter(|s| s.retired.is_none()).count();
        if active >= self.cap
            && let Some(old) = self.shots.iter_mut().filter(|s| s.retired.is_none()).min_by(|a, b| a.t0.total_cmp(&b.t0))
        {
            old.retired = Some(t);
        }
        // Retired instances are fading; under a storm of triggers drop the most faded first.
        while self.shots.len() >= self.cap * 2 {
            let i = (0..self.shots.len()).min_by(|&a, &b| self.shots[a].retired.unwrap_or(f32::MAX).total_cmp(&self.shots[b].retired.unwrap_or(f32::MAX))).unwrap();
            self.shots.remove(i);
        }
        self.count = self.count.wrapping_add(1);
        self.shots.push(OneShot { t0: t, data, seed: cpu::hu(self.count, 0x51ed), retired: None });
    }

    /// Instances alive at `t` (started less than `life` seconds ago) with their opacity.
    pub fn live(&self, t: f32, life: f32) -> impl Iterator<Item = (&OneShot<T>, f32)> {
        self.shots.iter().filter_map(move |s| {
            let k = match s.retired {
                Some(r) => {
                    let x = ((t - r) / Self::FADE).clamp(0.0, 1.0);
                    1.0 - x * x * (3.0 - 2.0 * x)
                }
                None => 1.0,
            };
            (t - s.t0 < life && k > 0.0).then_some((s, k))
        })
    }

    /// Drop instances that finished (older than `life`) or faded out.
    pub fn prune(&mut self, t: f32, life: f32) {
        self.shots.retain(|s| t - s.t0 < life && s.retired.is_none_or(|r| t - r < Self::FADE));
    }

    pub fn is_animating(&self, t: f32, life: f32) -> bool {
        self.live(t, life).next().is_some()
    }

    pub fn clear(&mut self) {
        self.shots.clear();
    }
}

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
    fn affine(&self, rest: Rect) -> [f32; 6] {
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

fn invert(m: [f32; 6]) -> [f32; 6] {
    let [a, b, c, d, tx, ty] = m;
    let det = a * d - b * c;
    let id = 1.0 / if det.abs() < 1e-9 { 1e-9 } else { det };
    let ia = d * id;
    let ib = -b * id;
    let ic = -c * id;
    let idd = a * id;
    [ia, ib, ic, idd, -(ia * tx + ic * ty), -(ib * tx + idd * ty)]
}

fn corners(r: Rect) -> [[f32; 2]; 4] {
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

// ---------------------------------------------------------------- Fx / layers

#[derive(Clone, Copy, Debug)]
pub enum Fx {
    Glow(GlowParams),
    Pulse(PulseParams),
    Electric(ElectricParams),
    Fire(FireParams),
    Distort(DistortParams),
    MotionBlur(MotionBlurParams, MotionSamples),
    Shimmer(ShimmerParams),
    Sparkles(SparkleParams),
    Aurora(AuroraParams),
    Grain(GrainParams),
    /// A fill style (`FillParams`): paints the shape or a border band inside its edge.
    Fill(FillParams),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Under,
    Over,
}

/// Which texture a layer samples (`FxFrame::sources[slot]`) and the rect (pt) it covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub slot: u32,
    pub rect: Rect,
}

/// Default unclipped rectangle for [`FxSlot::clip`], matching `rune::render::NO_CLIP`.
pub const NO_CLIP: [f32; 4] = [-1e6, -1e6, 1e6, 1e6];

#[derive(Clone, Copy, Debug)]
pub struct FxLayer {
    pub fx: Fx,
    pub shape: Shape,
    pub place: Place,
    pub time: f32,
    pub seed: u32,
    pub reduced_motion: bool,
    pub source: Option<Source>,
    /// 0..1, scales the layer's whole output (a retiring one-shot fades out with it).
    pub opacity: f32,
}

/// How an effect layer composites onto the target beneath it (`FxSlot::blend`).
///
/// `Normal` uses premultiplied source-over (allowing additive light where `rgb > a`).
/// `Multiply`, `Screen`, `Overlay`, and `SoftLight` evaluate the W3C Compositing and Blending
/// separable formulas in the frame's working colour space: linear light by default, or sRGB
/// when `FxRenderer::set_srgb_blend(true)` matches `rune::Tx::set_srgb_blend`.
///
/// `Screen` uses fixed-function `wgpu::BlendState` with zero backdrop reads.
/// `Multiply`, `Overlay` and `SoftLight` sample the backdrop (`FxFrame::backdrop` or `EffectSite::backdrop`
/// via `set_layer_backdrop` / `blur_backdrop`) and fall back to `Normal` when no backdrop is
/// available (for example, on a target without `COPY_SRC`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FxBlend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
}

impl FxBlend {
    /// True when the blend mode samples the backdrop texture in the fragment shader.
    pub const fn needs_backdrop(self) -> bool {
        matches!(self, Self::Multiply | Self::Overlay | Self::SoftLight)
    }

    /// Additional per-pixel cost of this blend mode on top of the effect's own `Fx::cost`.
    pub const fn cost(self) -> FxCost {
        match self {
            Self::Normal | Self::Screen => {
                FxCost { passes: 0, samples: 0, alu: 0, backdrop: false, source: false }
            }
            Self::Multiply | Self::Overlay | Self::SoftLight => {
                FxCost { passes: 0, samples: 1, alu: 1, backdrop: true, source: false }
            }
        }
    }

    pub(crate) const fn code(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Multiply => 1,
            Self::Screen => 2,
            Self::Overlay => 3,
            Self::SoftLight => 4,
        }
    }
}

/// Per-layer placement in a Rune effect slot. Passed separately to
/// [`FxRenderer::prepare_with_slots`] so existing `FxLayer` and `FxFrame` literals stay valid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxSlot {
    pub(crate) scale: f32,
    pub(crate) clip: [f32; 4],
    pub(crate) fade: [u32; 4],
    pub(crate) backdrop_srgb: Option<bool>,
    pub(crate) blend: FxBlend,
}

impl Default for FxSlot {
    fn default() -> Self {
        Self { scale: 0.0, clip: NO_CLIP, fade: [0; 4], backdrop_srgb: None, blend: FxBlend::Normal }
    }
}

impl FxSlot {
    /// Whether backdrop samples need software sRGB decoding. Default: `FxFrame::backdrop`
    /// samples are linear; per-slot copies use the frame's blend encoding. Set false for an
    /// `*Srgb` view (which hardware decodes) or a linear `Rgba16Float` input, even in sRGB mode.
    pub fn backdrop_srgb(mut self, on: bool) -> Self { self.backdrop_srgb = Some(on); self }
    /// Device pixels per point (`EffectSite::scale`); zero uses `FxFrame::scale`.
    pub fn scale(mut self, scale: f32) -> Self { self.scale = scale; self }
    /// Window device-pixel clip and packed soft-edge / rounded-clip words.
    pub fn clip(mut self, clip: [f32; 4], fade: [u32; 4]) -> Self {
        self.clip = clip;
        self.fade = fade;
        self
    }
    /// Blend mode for compositing this layer onto the target beneath it.
    pub fn blend(mut self, blend: FxBlend) -> Self {
        self.blend = blend;
        self
    }
}

impl Fx {
    pub fn glow(p: GlowParams) -> Fx { Fx::Glow(p) }
    pub fn pulse(p: PulseParams) -> Fx { Fx::Pulse(p) }
    pub fn electric(p: ElectricParams) -> Fx { Fx::Electric(p) }
    pub fn fire(p: FireParams) -> Fx { Fx::Fire(p) }
    pub fn distort(p: DistortParams) -> Fx { Fx::Distort(p) }
    pub fn lens(p: LensParams) -> Fx { Fx::Distort(DistortParams::Lens(p)) }
    pub fn motion_blur(p: MotionBlurParams, s: MotionSamples) -> Fx { Fx::MotionBlur(p, s) }
    pub fn shimmer(p: ShimmerParams) -> Fx { Fx::Shimmer(p) }
    pub fn sparkles(p: SparkleParams) -> Fx { Fx::Sparkles(p) }
    pub fn aurora(p: AuroraParams) -> Fx { Fx::Aurora(p) }
    pub fn grain(p: GrainParams) -> Fx { Fx::Grain(p) }
    /// `Fx::fill(LiquidMetalParams::gold())`, `Fx::fill(DitherParams::bayer().border(3.0))`.
    pub fn fill(p: impl Into<FillParams>) -> Fx { Fx::Fill(p.into()) }

    /// Attach to a node shape. Defaults: under the node for light that spills out (glow, pulse,
    /// fire), over it for things that replace or overlay pixels (distortion, motion blur, extras).
    /// Fills go under the node, as its background, unless they read the backdrop (glass, or a
    /// dither or halftone of what is behind), which goes over.
    pub fn on(self, shape: Shape) -> FxLayer {
        let place = match self {
            Fx::Glow(_) | Fx::Pulse(_) | Fx::Fire(_) | Fx::Aurora(_) => Place::Under,
            Fx::Fill(p) if !p.needs_backdrop() => Place::Under,
            Fx::Electric(_) => Place::Over,
            _ => Place::Over,
        };
        FxLayer { fx: self, shape, place, time: 0.0, seed: 1, reduced_motion: false, source: None, opacity: 1.0 }
    }

    pub fn needs_backdrop(&self) -> bool {
        match self {
            Fx::Distort(_) => true,
            Fx::Fill(p) => p.needs_backdrop(),
            _ => false,
        }
    }

    /// Static cost declaration per covered pixel: `samples` = texture taps + CPU-table reads,
    /// `alu` = gradient-noise evaluations.
    pub fn cost(&self) -> FxCost {
        let c = |samples, alu| FxCost { passes: 1, samples, alu, backdrop: false, source: false };
        match self {
            Fx::Glow(p) => match p.style {
                GlowStyle::Mask => FxCost { samples: 13, source: true, ..c(13, 0) },
                GlowStyle::Aura => c(0, 1),
                _ => c(0, 0),
            },
            Fx::Pulse(_) => c(0, 0),
            Fx::Electric(p) => {
                // Paths/forks are CPU tables: per pixel = table reads, no noise.
                let s = p.strands.clamp(1, 3);
                match p.style {
                    ElectricStyle::Arc { .. } => c(1 + s * 4 + 4 * 6, 0),
                    ElectricStyle::Border => c(1 + s * 6 + 5 * 6, 0),
                    ElectricStyle::Charged => c(1 + 6 + 5 * 6, 0),
                }
            }
            Fx::Fire(p) => c(p.embers.min(40) * 2, match p.style { FireStyle::Embers => 0, FireStyle::Border => 3, _ => 8 }),
            Fx::Distort(d) => {
                let (s, a) = match d {
                    DistortParams::HeatHaze { .. } => (1, 6),
                    DistortParams::Shockwave { .. } => (3, 0),
                    DistortParams::Lens(l) => (if l.frost > 0.0 { 12 } else { 4 + 8 }, 0),
                    DistortParams::Crt { .. } => (7, 0),
                    DistortParams::ChromaSplit { .. } => (3, 0),
                };
                FxCost { passes: 1, samples: s, alu: a, backdrop: true, source: false }
            }
            Fx::MotionBlur(_, s) => FxCost { passes: 1, samples: s.count, alu: 0, backdrop: false, source: true },
            Fx::Shimmer(p) => FxCost { source: p.use_mask, ..c(p.use_mask as u32, 0) },
            Fx::Sparkles(p) => c(p.count.min(64) * 2, 0),
            Fx::Aurora(_) => c(12, 3),
            Fx::Grain(_) => c(0, 0),
            Fx::Fill(p) => p.cost(),
        }
    }
}

/// What a layer costs per covered pixel (plus the quad area reported by `FxStats`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FxCost {
    pub passes: u32,
    pub samples: u32,
    pub alu: u32,
    pub backdrop: bool,
    pub source: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkle_cap_and_tables_reach_64_without_regrowing() {
        let p = SparkleParams { count: 100, burst: Some(0.0), life: 10.0, ..Default::default() };
        let layer = Fx::sparkles(p).on(card()).at(1.0);
        let (_, packed, _) = layer.pack().unwrap();
        assert_eq!(packed.p0[0], 64.0);
        assert_eq!(layer.fx.cost().samples, 128);
        let mut table = Vec::with_capacity(128);
        let capacity = table.capacity();
        let (_, count) = cpu::sparkles(&p, card().rect, 1.0, 1, false, &mut table);
        assert!(count > 32.0 && count <= 64.0);
        assert_eq!(table.len(), count as usize * 2);
        assert_eq!(table.capacity(), capacity);
    }

    fn card() -> Shape {
        Shape::rounded(100.0, 100.0, 200.0, 120.0, 16.0)
    }

    #[test]
    fn idle_layers_cost_nothing() {
        let l = Fx::glow(GlowParams { intensity: 0.0, ..Default::default() }).on(card());
        assert!(l.pack().is_none());
        let l = Fx::glow(GlowParams { heat: 0.0, ..Default::default() }).on(card());
        assert!(l.pack().is_none(), "a fully cooled light is culled");
        let ripple = PulseParams { style: PulseStyle::Ripple { at: [150.0, 150.0] }, t0: 1.0, duration: 0.5, ..Default::default() };
        assert!(Fx::pulse(ripple).on(card()).at(0.5).pack().is_none(), "before the tap");
        assert!(Fx::pulse(ripple).on(card()).at(1.2).pack().is_some());
        assert!(Fx::pulse(ripple).on(card()).at(1.6).pack().is_none(), "after the ripple");
        assert!(!Fx::pulse(ripple).on(card()).at(1.6).is_animating());
        let shock = DistortParams::shockwave([0.0, 0.0], 0.0);
        assert!(Fx::distort(shock).on(card()).at(2.0).pack().is_none());
    }

    #[test]
    fn layers_that_draw_nothing_never_keep_the_loop_awake() {
        let glow = GlowParams { breathe: 0.3, ..Default::default() };
        let pulse = PulseParams::default();
        let electric = ElectricParams::default();
        let fire = FireParams::default();
        let haze = |amount| DistortParams::HeatHaze { height: 40.0, scale: 14.0, speed: 2.2, amount, whole_shape: false };
        let chroma = |glitch| DistortParams::ChromaSplit { offset: [0.0, 0.0], glitch, feather: 0.0 };
        let shimmer = ShimmerParams::default();
        let sparkles = SparkleParams::default();
        let aurora = AuroraParams::default();
        let grain = GrainParams { fps: 24.0, ..Default::default() };
        let pairs = [
            (Fx::glow(glow), Fx::glow(GlowParams { intensity: 0.0, ..glow })),
            (Fx::glow(glow), Fx::glow(GlowParams { heat: 0.0, ..glow })),
            (Fx::pulse(pulse), Fx::pulse(PulseParams { intensity: 0.0, ..pulse })),
            (Fx::electric(electric), Fx::electric(ElectricParams { intensity: 0.0, ..electric })),
            (Fx::fire(fire), Fx::fire(FireParams { intensity: 0.0, ..fire })),
            (Fx::distort(haze(1.6)), Fx::distort(haze(0.0))),
            (Fx::distort(chroma(0.5)), Fx::distort(chroma(0.0))),
            (Fx::shimmer(shimmer), Fx::shimmer(ShimmerParams { intensity: 0.0, base_alpha: 0.0, ..shimmer })),
            (Fx::sparkles(sparkles), Fx::sparkles(SparkleParams { intensity: 0.0, ..sparkles })),
            (Fx::sparkles(sparkles), Fx::sparkles(SparkleParams { count: 0, ..sparkles })),
            (Fx::aurora(aurora), Fx::aurora(AuroraParams { intensity: 0.0, ..aurora })),
            (Fx::grain(grain), Fx::grain(GrainParams { amount: 0.0, ..grain })),
        ];
        for (live, silent) in pairs {
            let live = live.on(card()).at(1.0);
            assert!(live.is_animating() && live.pack().is_some(), "{live:?}");
            assert!(!live.opacity(0.0).is_animating(), "zero opacity: {live:?}");
            let silent = silent.on(card()).at(1.0);
            assert!(!silent.is_animating() && silent.pack().is_none(), "{silent:?}");
        }
    }

    #[test]
    fn a_ping_ticks_until_its_shot_has_cooled() {
        let ping = PulseParams { style: PulseStyle::Ping { from: [0.0, 0.0], dir: [1.0, 0.0], travel: 200.0 }, t0: 1.0, duration: 0.5, ..Default::default() };
        let at = |t: f32| Fx::pulse(ping).on(card()).at(t);
        let cooling = 1.0 + 0.5 * 2.1;
        assert!(at(cooling).pack().is_some(), "still drawn while it cools");
        assert!(at(cooling).is_animating(), "so the loop keeps ticking");
        let done = 1.0 + 0.5 * 2.3;
        assert!(at(done).pack().is_none() && !at(done).is_animating());
    }

    #[test]
    fn one_shots_overlap_and_retire_by_fading() {
        let mut p = OneShots::new(3);
        for k in 0..3 {
            p.trigger(k as f32 * 0.1, k);
        }
        let live: Vec<_> = p.live(0.25, 1.0).map(|(s, o)| (s.data, o)).collect();
        assert_eq!(live, vec![(0, 1.0), (1, 1.0), (2, 1.0)], "a retrigger never restarts or cuts the running ones");
        // Full: the oldest fades out over FADE instead of vanishing.
        p.trigger(0.3, 3);
        let o0 = |t: f32| p.live(t, 1.0).find(|(s, _)| s.data == 0).map_or(0.0, |(_, o)| o);
        assert_eq!(o0(0.3), 1.0);
        let mut prev = 1.0;
        for i in 1..=20 {
            let v = o0(0.3 + i as f32 * OneShots::<i32>::FADE / 20.0);
            assert!(v <= prev && prev - v < 0.2, "fades smoothly: {prev} -> {v}");
            prev = v;
        }
        assert_eq!(prev, 0.0);
        assert_eq!(p.live(0.31, 1.0).filter(|(_, o)| *o == 1.0).count(), 3);
        // A storm of triggers stays bounded.
        for k in 0..100 {
            p.trigger(0.4 + k as f32 * 0.001, k);
        }
        assert!(p.live(0.5, 1.0).count() <= 6);
        let seeds: std::collections::HashSet<u32> = p.live(0.5, 1.0).map(|(s, _)| s.seed).collect();
        assert!(seeds.len() > 1, "instances get distinct seeds");
    }

    #[test]
    fn opacity_zero_is_culled() {
        assert!(Fx::glow(GlowParams::default()).on(card()).opacity(0.0).pack().is_none());
        let (_, i, _) = Fx::glow(GlowParams::default()).on(card()).opacity(0.4).pack().unwrap();
        assert_eq!(i.shape[2], 0.4);
    }

    #[test]
    fn zaps_and_bursts_live_out_their_window_then_cull() {
        let zap = Fx::electric(ElectricParams { style: ElectricStyle::Arc { a: [0.0, 0.0], b: [200.0, 40.0] }, zap: Some(1.0), ..Default::default() }).on(card());
        assert!(zap.at(0.9).pack().is_none());
        assert!(zap.at(1.2).pack().is_some() && zap.at(1.2).is_animating());
        assert!(zap.at(1.0 + ElectricParams::ZAP_LIFE + 0.01).pack().is_none());
        let p = SparkleParams { burst: Some(2.0), ..Default::default() };
        let burst = Fx::sparkles(p).on(card());
        assert!(burst.at(2.3).pack().is_some());
        assert!(!burst.at(2.0 + p.burst_life() + 0.01).is_animating());
        let mut data = Vec::new();
        let (_, n) = cpu::sparkles(&p, card().rect, 2.0 + p.burst_life() + 0.01, 1, false, &mut data);
        assert_eq!(n, 0.0, "every burst star is gone by burst_life()");

        let dense = SparkleParams { count: 64, burst: Some(0.0), life: 2.0, ..Default::default() };
        let (_, inst, _) = Fx::sparkles(dense).on(card()).at(0.3).pack().unwrap();
        assert_eq!(inst.p0[0], 64.0);
        assert_eq!(Fx::sparkles(SparkleParams { count: 100, ..dense }).cost().samples, 128);
        data.clear();
        let (_, n64) = cpu::sparkles(&dense, card().rect, 0.3, 1, false, &mut data);
        assert_eq!(n64, 64.0, "all 64 burst stars are emitted within the cap");
        assert_eq!(data.len(), 128, "two vec4f entries per star");
    }

    #[test]
    fn motion_blur_is_one_sample_at_rest_and_scales_with_speed() {
        let rest = Rect::new(0.0, 0.0, 100.0, 60.0);
        let p = MotionBlurParams::default();
        let still = MotionSamples::from_track(&p, rest, 1.0, 2.0, |_| Pose::default());
        assert_eq!(still.count, 1);
        assert!(!still.is_moving());
        let slow = MotionSamples::from_track(&p, rest, 1.0, 2.0, |t| Pose { x: t * 300.0, ..Default::default() });
        let fast = MotionSamples::from_track(&p, rest, 1.0, 2.0, |t| Pose { x: t * 1500.0, ..Default::default() });
        assert!(slow.count > 1 && fast.count > slow.count, "{} {}", slow.count, fast.count);
        let w: f32 = fast.xforms[..fast.count as usize].iter().map(|x| x[6]).sum();
        assert!((w - 1.0).abs() < 1e-4, "shutter weights sum to opacity: {w}");
        // Bounds cover the swept rect.
        assert!(fast.bounds.w > rest.w + 10.0);
    }

    #[test]
    fn pose_inverse_round_trips() {
        let rest = Rect::new(10.0, 20.0, 100.0, 60.0);
        let pose = Pose { x: 33.0, y: -7.0, rotation: 0.4, scale: 1.3, opacity: 1.0 };
        let inv = invert(pose.affine(rest));
        let p = pose.apply(rest, [25.0, 40.0]);
        let back = [inv[0] * p[0] + inv[2] * p[1] + inv[4], inv[1] * p[0] + inv[3] * p[1] + inv[5]];
        assert!((back[0] - 25.0).abs() < 1e-3 && (back[1] - 40.0).abs() < 1e-3, "{back:?}");
    }

    #[test]
    fn phase_never_jumps_when_speed_changes() {
        let mut ph = Phase::new(1.0);
        let before = ph.at(2.0);
        ph.set_speed(2.0, 3.0);
        assert!((ph.at(2.0) - before).abs() < 1e-6);
        assert!((ph.at(2.1) - before - 0.3).abs() < 1e-5);
    }

    #[test]
    fn heat_cools_continuously() {
        let mut h = Heat::new(0.1, 0.3);
        h.set(0.0, true);
        let lit = h.at(0.5);
        assert!((lit - 1.0).abs() < 1e-6);
        h.set(0.5, false);
        assert!((h.at(0.5) - lit).abs() < 1e-6, "no cut at switch-off");
        let mut prev = h.at(0.5);
        for i in 1..100 {
            let v = h.at(0.5 + i as f32 * 0.01);
            assert!(v < prev && prev - v < 0.05, "monotonic, no step");
            prev = v;
        }
        // Switching back on mid-cool resumes from the current heat.
        let mid = h.at(0.7);
        h.set(0.7, true);
        assert!((h.at(0.7) - mid).abs() < 1e-6);
    }

    #[test]
    fn cpu_noise_is_bounded_periodic_and_deterministic() {
        for i in 0..2000 {
            let x = i as f32 * 0.173;
            let v = cpu::gnoise(x, x * 0.37, 0, 9);
            assert!(v.abs() <= 1.01, "{v}");
            assert_eq!(v, cpu::gnoise(x, x * 0.37, 0, 9));
            let a = cpu::gnoise(x, 0.4, 7, 3);
            let b = cpu::gnoise(x + 7.0, 0.4, 7, 3);
            assert!((a - b).abs() < 1e-4, "periodic in x");
        }
    }

    #[test]
    fn electric_tables_have_the_declared_layout() {
        let mut data = Vec::new();
        let p = ElectricParams { style: ElectricStyle::Arc { a: [0.0, 0.0], b: [300.0, 0.0] }, strands: 2, forks: 1.0, ..Default::default() };
        let xf = cpu::electric(&p, card(), 1.7, 3, false, &mut data);
        let (n, forks, strands) = (xf[1] as usize, xf[2] as usize, xf[3] as usize);
        assert_eq!(data.len(), 2 + 2 * strands * n / 4 + forks * 6);
        // Endpoints are pinned.
        assert!(data[2][0].abs() < 1e-3);
        let reduced = cpu::electric(&p, card(), 1.7, 3, true, &mut Vec::new());
        assert_eq!(reduced[2], 0.0, "reduced motion: no forks");
    }

    #[test]
    fn zero_amplitude_electricity_is_a_straight_finite_line() {
        for style in [ElectricStyle::Arc { a: [0.0, 0.0], b: [300.0, 0.0] }, ElectricStyle::Border] {
            let p = ElectricParams { style, amplitude: 0.0, forks: 1.0, ..Default::default() };
            for t in [0.0, 0.4, 1.7] {
                let mut data = Vec::new();
                cpu::electric(&p, card(), t, 3, false, &mut data);
                assert!(data.iter().flatten().all(|v| v.is_finite()), "{style:?} at {t}");
            }
        }
    }
}
