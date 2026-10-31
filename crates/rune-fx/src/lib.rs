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
mod params;
mod renderer;
mod sim;
#[cfg(test)]
mod tests;

pub use catalog::Choice;
pub use catalog::Param;
pub use catalog::ParamKind;
pub use catalog::Preset;
pub use catalog::StyleInfo;
pub use catalog::Unit;
pub use catalog::Value;
pub use catalog::fill_style;
pub use catalog::fill_styles;
pub use fill::DitherParams;
pub use fill::DitherPattern;
pub use fill::FillAnchor;
pub use fill::FillInput;
pub use fill::FillParams;
pub use fill::FillStyle;
pub use fill::FlutedGlassParams;
pub use fill::GradientKind;
pub use fill::GrainGradientParams;
pub use fill::HalftoneParams;
pub use fill::LiquidMetalParams;
pub use fill::MAX_STOPS;
pub use fill::MeshGradientParams;
pub use fill::MetaballParams;
pub use fill::PlasticParams;
pub use fill::Stops;
pub use fill::SwirlParams;
pub use fill::VoronoiParams;
pub use fill::WaveParams;
pub use params::AuroraParams;
pub use params::BlurMode;
pub use params::DistortParams;
pub use params::ElectricParams;
pub use params::ElectricStyle;
pub use params::FireParams;
pub use params::FireStyle;
pub use params::GlowParams;
pub use params::GlowStyle;
pub use params::GrainParams;
pub use params::Knob;
pub use params::LensParams;
pub use params::MAX_MOTION_SAMPLES;
pub use params::MotionBlurParams;
pub use params::MotionSamples;
pub use params::Pose;
pub use params::PulseParams;
pub use params::PulseStyle;
pub use params::ShimmerParams;
pub use params::SparkleParams;
pub use renderer::FxFrame;
pub use renderer::FxOptions;
pub use renderer::FxRenderer;
pub use renderer::FxStats;
pub use renderer::MaskPyramid;
pub use rune_motion::Rect;

pub(crate) use pack::*;
pub(crate) use params::corners;

// ---------------------------------------------------------------- geometry / colour

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

