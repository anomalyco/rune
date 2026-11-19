//! Fills: styles that paint a node's whole shape, or a band inside its edge as a stroke. They share
//! the shape mask (`FillParams::border`), colour lists (`Stops`, blended in OKLab) and one input
//! for the styles that re-tone something (`FillInput`: a ramp, noise, the backdrop or an image).
//!
//! Every style is a pure function of (pixel, params, time, seed). Under reduced motion each one
//! holds a single settled pose. `catalog` lists the styles, their params and presets.

use crate::catalog::*;
use crate::*;

mod pack;
mod styles;

pub use styles::DitherParams;
pub use styles::FlutedGlassParams;
pub use styles::GrainGradientParams;
pub use styles::HalftoneParams;
pub use styles::LiquidMetalParams;
pub use styles::MeshGradientParams;
pub use styles::MetaballParams;
pub use styles::PlasticParams;
pub use styles::SwirlParams;
pub use styles::VoronoiParams;
pub use styles::WaveParams;

// ---------------------------------------------------------------- colours

/// Most colours a `Stops` list holds.
pub const MAX_STOPS: usize = 6;

/// An ordered list of 1 to `MAX_STOPS` colours (linear light).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stops {
    n: u8,
    c: [Rgb; MAX_STOPS],
}

impl Stops {
    /// Keeps the first `MAX_STOPS`; an empty list is one black stop.
    pub fn new(colors: &[Rgb]) -> Stops {
        let mut c = [Rgb([0.0; 3]); MAX_STOPS];
        let n = colors.len().clamp(1, MAX_STOPS);
        for (d, s) in c.iter_mut().zip(colors) {
            *d = *s;
        }
        Stops { n: n as u8, c }
    }
    /// From sRGB hex colours (0xRRGGBB).
    pub fn hex(hex: &[u32]) -> Stops {
        let v: Vec<Rgb> = hex.iter().map(|h| rgb(*h)).collect();
        Stops::new(&v)
    }
    pub fn as_slice(&self) -> &[Rgb] {
        &self.c[..self.n as usize]
    }
    pub fn len(&self) -> usize {
        self.n as usize
    }
    pub fn is_empty(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------- shared choices

/// What a re-toning style (dither, halftone) reads as its tone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillInput {
    /// A ramp across the shape along `angle`.
    Linear,
    /// Light at the centre, dark at the rim.
    Radial,
    /// Slowly drifting noise.
    Noise,
    /// Whatever is drawn behind the layer (needs a backdrop copy).
    Backdrop,
    /// The layer's source texture (`FxLayer::source`).
    Image,
}

choices!(FillInput { Linear = "linear" "Linear", Radial = "radial" "Radial", Noise = "noise" "Noise", Backdrop = "backdrop" "Backdrop", Image = "image" "Image" });

impl FillInput {
    pub(super) fn code(self) -> f32 {
        match self {
            FillInput::Linear => 0.0,
            FillInput::Radial => 1.0,
            FillInput::Noise => 2.0,
            FillInput::Backdrop => 3.0,
            FillInput::Image => 4.0,
        }
    }
    fn animates(self) -> bool {
        self == FillInput::Noise
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DitherPattern {
    Bayer2,
    Bayer4,
    Bayer8,
    /// A 32×32 void-and-cluster tile: no visible grid, even grain.
    BlueNoise,
    /// Independent random threshold per pixel.
    White,
}

choices!(DitherPattern { Bayer2 = "bayer2" "Bayer 2×2", Bayer4 = "bayer4" "Bayer 4×4", Bayer8 = "bayer8" "Bayer 8×8", BlueNoise = "blue_noise" "Blue noise", White = "white" "White noise" });

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradientKind {
    Linear,
    Radial,
    /// Sweeps around `center`, starting at `angle`.
    Conic,
}

choices!(GradientKind { Linear = "linear" "Linear", Radial = "radial" "Radial", Conic = "conic" "Conic" });

/// Where a fill's pattern is pinned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillAnchor {
    /// To the window: neighbouring shapes show neighbouring parts of one surface.
    World,
    /// To the shape: every shape of the same size, seed and time looks the same.
    Element,
}

choices!(FillAnchor { World = "world" "World", Element = "element" "Element" });

// ---------------------------------------------------------------- FillStyle / FillParams

macro_rules! styles {
    ($($v:ident($p:ident) = $idx:literal),* $(,)?) => {
        /// Which fill, with its params. The order is the catalog's (`fill_styles()`).
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[non_exhaustive]
        pub enum FillStyle {
            $($v($p)),*
        }
        impl FillStyle {
            /// Position in `fill_styles()`; also the shader variant.
            pub(crate) fn index(&self) -> usize {
                match self { $(FillStyle::$v(_) => $idx),* }
            }
            pub(crate) fn get_param(&self, id: &str) -> Option<Value> {
                match self { $(FillStyle::$v(p) => p.get_param(id)),* }
            }
            pub(crate) fn set_param(&mut self, id: &str, v: Value) -> bool {
                match self { $(FillStyle::$v(p) => p.set_param(id, v)),* }
            }
            pub(crate) fn rust_field(&self, id: &str) -> Option<String> {
                match self { $(FillStyle::$v(p) => p.rust_field(id)),* }
            }
        }
        $(
            impl From<$p> for FillStyle {
                fn from(p: $p) -> Self { FillStyle::$v(p) }
            }
            impl From<$p> for FillParams {
                fn from(p: $p) -> Self { FillParams::new(FillStyle::$v(p)) }
            }
            impl $p {
                /// Paint only a band this wide (pt) inside the shape's edge.
                pub fn border(self, width: f32) -> FillParams { FillParams::from(self).border(width) }
                /// Fill opacity, 0..1.
                pub fn intensity(self, k: f32) -> FillParams { FillParams::from(self).intensity(k) }
            }
        )*
    };
}

styles!(
    LiquidMetal(LiquidMetalParams) = 0,
    MeshGradient(MeshGradientParams) = 1,
    Dither(DitherParams) = 2,
    GrainGradient(GrainGradientParams) = 3,
    FlutedGlass(FlutedGlassParams) = 4,
    Metaballs(MetaballParams) = 5,
    Swirl(SwirlParams) = 6,
    Voronoi(VoronoiParams) = 7,
    Waves(WaveParams) = 8,
    Halftone(HalftoneParams) = 9,
    Plastic(PlasticParams) = 10,
);

/// A fill: the style plus what every style shares. Build it with `FillParams::new`, `From` a
/// style's params, or a preset; more shared fields may come, so it has no struct literal.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct FillParams {
    pub style: FillStyle,
    /// 0 fills the shape; above 0 paints only a band this wide (pt) inside the edge, as a stroke.
    pub border: f32,
    /// Opacity of the fill, 0..1.
    pub intensity: f32,
}

impl FillParams {
    pub fn new(style: FillStyle) -> FillParams {
        FillParams { style, border: 0.0, intensity: 1.0 }
    }
    pub fn border(mut self, width: f32) -> FillParams {
        self.border = width;
        self
    }
    pub fn intensity(mut self, k: f32) -> FillParams {
        self.intensity = k;
        self
    }

    /// Whether this fill samples the backdrop (glass, or a dither or halftone of it).
    pub fn needs_backdrop(&self) -> bool {
        match self.style {
            FillStyle::FlutedGlass(_) => true,
            FillStyle::Dither(p) => p.input == FillInput::Backdrop,
            FillStyle::Halftone(p) => p.input == FillInput::Backdrop,
            _ => false,
        }
    }

    fn needs_source(&self) -> bool {
        match self.style {
            FillStyle::Dither(p) => p.input == FillInput::Image,
            FillStyle::Halftone(p) => p.input == FillInput::Image,
            _ => false,
        }
    }

    /// Whether the fill moves on its own (so the frame loop keeps ticking).
    pub(crate) fn animates(&self) -> bool {
        match self.style {
            FillStyle::LiquidMetal(p) => p.speed > 0.0 && p.flow > 0.0,
            FillStyle::MeshGradient(p) => p.speed > 0.0 && (p.drift > 0.0 || p.warp > 0.0),
            FillStyle::Dither(p) => p.input.animates() && p.speed > 0.0,
            FillStyle::GrainGradient(p) => p.fps > 0.0 && p.amount > 0.0,
            FillStyle::FlutedGlass(_) => false,
            FillStyle::Metaballs(p) => p.speed > 0.0,
            FillStyle::Swirl(p) => p.speed > 0.0,
            FillStyle::Voronoi(p) => p.speed > 0.0,
            FillStyle::Waves(p) => p.speed > 0.0 && p.amplitude > 0.0,
            FillStyle::Halftone(p) => p.input.animates() && p.speed > 0.0,
            FillStyle::Plastic(_) => false,
        }
    }

    pub(crate) fn cost(&self) -> FxCost {
        // A ramp reads two stops; mesh and metaballs read every stop or ball.
        let ramp = 2;
        let input = |i: FillInput| match i {
            FillInput::Noise => (0, 3),
            FillInput::Backdrop | FillInput::Image => (1, 0),
            _ => (0, 0),
        };
        let (samples, alu) = match self.style {
            FillStyle::LiquidMetal(p) => (0, if p.flow > 0.0 { 8 } else { 0 }),
            FillStyle::MeshGradient(p) => (p.colors.len() as u32 * 2, if p.warp > 0.0 { 2 } else { 0 }),
            FillStyle::Dither(p) => {
                let (s, a) = input(p.input);
                (s + 1 + (p.pattern == DitherPattern::BlueNoise) as u32, a)
            }
            FillStyle::GrainGradient(_) => (ramp, 0),
            FillStyle::FlutedGlass(p) => (if p.frost > 0.0 { 6 } else { 3 }, 0),
            FillStyle::Metaballs(p) => (p.count.clamp(1, 8) * 2, 0),
            FillStyle::Swirl(_) => (ramp, 9),
            FillStyle::Voronoi(_) => (1, 0),
            FillStyle::Waves(p) => (p.colors.len() as u32, 0),
            // Four dots per pixel, each reading the tone.
            FillStyle::Halftone(p) => {
                let (s, a) = input(p.input);
                (s * 4, a * 4)
            }
            FillStyle::Plastic(_) => (0, 0),
        };
        FxCost { passes: 1, samples, alu, backdrop: self.needs_backdrop(), source: self.needs_source() }
    }

    /// Whether the fill draws nothing on a shape of this rect, at any time.
    pub(crate) fn draws_nothing(&self, r: Rect) -> bool {
        !(self.intensity > 0.0) || !(self.border.is_finite() && self.border >= 0.0) || !(r.w > 0.0) || !(r.h > 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::pack::oklab;
    use super::*;

    fn card() -> Shape {
        Shape::rounded(100.0, 100.0, 240.0, 140.0, 18.0)
    }

    fn every_preset() -> impl Iterator<Item = (&'static StyleInfo, &'static Preset)> {
        fill_styles().iter().flat_map(|s| s.presets.iter().map(move |p| (s, p)))
    }

    fn packed(l: &FxLayer) -> Option<(InstRaw, Vec<[f32; 4]>)> {
        let (_, mut i, _) = l.pack()?;
        let mut data = Vec::new();
        l.fill(&mut i, &mut data);
        Some((i, data))
    }

    fn bits(i: &InstRaw) -> Vec<u32> {
        bytemuck::bytes_of(i).chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    #[test]
    fn idle_fills_are_culled() {
        for (s, p) in every_preset() {
            assert!(Fx::fill(p.fill().intensity(0.0)).on(card()).pack().is_none(), "{} {}", s.id, p.id);
            assert!(Fx::fill(p.fill().intensity(f32::NAN)).on(card()).pack().is_none(), "{} {}", s.id, p.id);
            assert!(Fx::fill(p.fill().border(-1.0)).on(card()).pack().is_none());
            assert!(Fx::fill(p.fill().border(f32::NAN)).on(card()).pack().is_none());
            assert!(Fx::fill(p.fill().border(f32::INFINITY)).on(card()).pack().is_none());
            assert!(Fx::fill(p.fill()).on(Shape::rounded(10.0, 10.0, 0.0, 40.0, 0.0)).pack().is_none(), "{} {}: zero width", s.id, p.id);
            assert!(Fx::fill(p.fill()).on(Shape::rounded(10.0, 10.0, 40.0, 0.0, 0.0)).pack().is_none(), "{} {}: zero height", s.id, p.id);
            assert!(Fx::fill(p.fill()).on(card()).pack().is_some());
        }
    }

    #[test]
    fn reduced_motion_holds_one_pose() {
        for (s, p) in every_preset() {
            let l = Fx::fill(p.fill()).on(card()).seed(3).reduced(true);
            assert!(!l.is_animating(), "{} {}", s.id, p.id);
            let (a, da) = packed(&l.at(0.5)).unwrap();
            let (b, db) = packed(&l.at(7.25)).unwrap();
            assert_eq!(bits(&a), bits(&b), "{} {}: same instance at any time", s.id, p.id);
            assert_eq!(da, db, "{} {}: same tables at any time", s.id, p.id);
        }
    }

    #[test]
    fn invisible_fills_never_animate() {
        for (s, p) in every_preset() {
            let off = Fx::fill(FillParams { intensity: 0.0, ..p.fill() }).on(card()).seed(11);
            assert!(!off.is_animating() && off.pack().is_none(), "{} {}", s.id, p.id);
            assert!(!Fx::fill(p.fill()).on(card()).opacity(0.0).is_animating(), "{} {}", s.id, p.id);
        }
        assert!(every_preset().any(|(_, p)| p.fill().animates()));
    }

    #[test]
    fn moving_fills_animate_and_are_deterministic() {
        for (s, p) in every_preset() {
            let l = Fx::fill(p.fill()).on(card()).seed(11);
            assert_eq!(l.is_animating(), p.fill().animates(), "{} {}", s.id, p.id);
            let (a, da) = packed(&l.at(1.5)).unwrap();
            let (b, db) = packed(&l.at(1.5)).unwrap();
            assert_eq!(bits(&a), bits(&b));
            assert_eq!(da, db);
            assert!(da.iter().flatten().all(|v| v.is_finite()), "{} {}", s.id, p.id);
        }
        let moving = |s: FillStyle| FillParams::new(s).animates();
        assert!(moving(LiquidMetalParams::gold().into()));
        assert!(!moving(LiquidMetalParams { flow: 0.0, ..LiquidMetalParams::gold() }.into()));
        assert!(!moving(FlutedGlassParams::reeded().into()));
        assert!(!moving(DitherParams::bayer().into()), "a ramp holds still");
        assert!(!moving(WaveParams { amplitude: 0.0, ..WaveParams::sea() }.into()), "flat waves hold still");
    }

    #[test]
    fn large_times_fold_without_precision_loss_or_jump() {
        assert_eq!(cpu::fold_t(0.0), 0.0);
        assert_eq!(cpu::fold_t(1.2), 1.2);
        assert_eq!(cpu::fold_t(512.0), 512.0);
        for t in [1e4f32, 1e5, -1e5] {
            let a = cpu::fold_t(t);
            let b = cpu::fold_t(t + 1.0 / 60.0);
            assert!((0.0..=512.0).contains(&a) && (0.0..=512.0).contains(&b), "{t} -> {a}");
            assert!((b - a).abs() > 1e-3 && (b - a).abs() < 0.05, "smooth step at {t}: {a} -> {b}");
        }
        for (s, p) in every_preset() {
            let l = Fx::fill(p.fill()).on(card()).seed(7);
            let (i0, d0) = packed(&l.at(1e5)).unwrap();
            let (i1, d1) = packed(&l.at(1e5 + 0.25)).unwrap();
            assert!(i0.t[0] >= 0.0 && i0.t[0] <= 512.0, "{} {}", s.id, p.id);
            assert!(d0.iter().flatten().all(|v| v.is_finite()), "{} {}", s.id, p.id);
            if p.fill().animates() {
                assert!(i0.t[0] != i1.t[0] || d0 != d1, "{} {} still moves at t = 1e5", s.id, p.id);
            }
        }
    }

    #[test]
    fn backdrop_and_placement_follow_the_input() {
        for (s, p) in every_preset() {
            let f = p.fill();
            if f.needs_backdrop() {
                assert!(s.reads_backdrop, "{} {}", s.id, p.id);
            }
            let l = Fx::fill(f).on(card());
            assert_eq!(l.fx.needs_backdrop(), f.needs_backdrop());
            assert_eq!(l.place, if f.needs_backdrop() { Place::Over } else { Place::Under });
            assert_eq!(l.fx.cost().backdrop, f.needs_backdrop());
        }
        assert!(Fx::fill(DitherParams::backdrop()).needs_backdrop());
        assert!(!Fx::fill(DitherParams::bayer()).needs_backdrop());
    }

    #[test]
    fn element_anchor_only_moves_the_anchor_slot() {
        assert_eq!(LiquidMetalParams::default().anchor, FillAnchor::World);
        let world = packed(&Fx::fill(LiquidMetalParams::gold()).on(card()).at(3.0)).unwrap().0;
        let element = packed(&Fx::fill(LiquidMetalParams::gold().anchor(FillAnchor::Element)).on(card()).at(3.0)).unwrap().0;
        assert_eq!(world.p2, [0.0; 4]);
        assert_eq!(element.p2, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(bits(&InstRaw { p2: [0.0; 4], ..element }), bits(&world));
    }

    #[test]
    fn plastic_holds_still() {
        for p in [PlasticParams::glossy(), PlasticParams::matte(), PlasticParams::dished()] {
            assert!(!FillParams::from(p).animates());
            assert!(!Fx::fill(p).on(card()).at(2.0).is_animating());
            assert_eq!(Fx::fill(p).cost(), FxCost { passes: 1, samples: 0, alu: 0, backdrop: false, source: false });
        }
        let non_finite_plastic = PlasticParams { roughness: f32::NAN, dome: f32::INFINITY, bevel: f32::NEG_INFINITY, angle: f32::NAN, ..PlasticParams::glossy() };
        let non_finite_metal = LiquidMetalParams {
            flow: f32::NAN,
            scale: f32::NAN,
            speed: f32::INFINITY,
            bevel: f32::NAN,
            dome: f32::NEG_INFINITY,
            angle: f32::NAN,
            iridescence: f32::INFINITY,
            contrast: f32::NAN,
            ..LiquidMetalParams::chrome()
        };
        for fx in [Fx::fill(non_finite_plastic), Fx::fill(non_finite_metal)] {
            let (i, _) = packed(&fx.on(card()).at(1.0)).unwrap();
            assert!(i.p0.iter().chain(&i.p1).chain(&i.p2).all(|v| v.is_finite()), "{i:?}");
        }
    }

    #[test]
    fn mesh_points_wander_within_drift() {
        let p = MeshGradientParams::dusk();
        for k in 0..40 {
            let pts = cpu::mesh_points(&p, k as f32 * 0.37, 5);
            for (q, r) in pts.iter().zip(p.points) {
                assert!((q[0] - r[0]).abs() <= p.drift * 1.5 && (q[1] - r[1]).abs() <= p.drift * 1.5, "{q:?} {r:?}");
            }
        }
        let still = MeshGradientParams { drift: 0.0, ..p };
        assert_eq!(cpu::mesh_points(&still, 3.0, 5), p.points);
    }

    #[test]
    fn blue_noise_tile_ranks_every_pixel_once() {
        let t = cpu::blue_noise();
        assert_eq!(t.len() * 4, cpu::BLUE_N * cpu::BLUE_N);
        let mut v: Vec<f32> = t.iter().flatten().copied().collect();
        v.sort_by(f32::total_cmp);
        for (k, x) in v.iter().enumerate() {
            assert!((x - (k as f32 + 0.5) / v.len() as f32).abs() < 1e-6);
        }
        // Blue: 2x2 box means stay near 0.5 (white noise would spread them far wider).
        let n = cpu::BLUE_N;
        let at = |x: usize, y: usize| t[(y % n * n + x % n) / 4][(y % n * n + x % n) % 4];
        let worst = (0..n * n).map(|i| (i % n, i / n)).map(|(x, y)| ((at(x, y) + at(x + 1, y) + at(x, y + 1) + at(x + 1, y + 1)) / 4.0 - 0.5).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.36, "{worst}");
    }

    #[test]
    fn oklab_of_white_and_grey_is_neutral() {
        let w = oklab(Rgb([1.0; 3]));
        assert!((w[0] - 1.0).abs() < 1e-3 && w[1].abs() < 1e-3 && w[2].abs() < 1e-3, "{w:?}");
        let g = oklab(rgb(0x777777));
        assert!(g[1].abs() < 1e-3 && g[2].abs() < 1e-3);
    }

    #[test]
    fn oklab_matches_canonical_for_non_negative_rgb_and_clamps_negative_lms() {
        assert_eq!(0.412_221_46f32.to_bits(), 0.412_221_47f32.to_bits());
        for v in 0..=255u8 {
            let c = rune_motion::srgb_decode(v as f32 / 255.0);
            for rgb in [[c, c, c], [c, 0.0, 0.0], [0.0, c, 0.0], [0.0, 0.0, c], [c, 1.0 - c, 0.5 * c]] {
                let got = oklab(Rgb(rgb));
                let canon = rune_motion::linear_to_oklab(rgb[0], rgb[1], rgb[2]);
                assert_eq!(got.map(f32::to_bits), canon.map(f32::to_bits), "{rgb:?}");
            }
        }
        for rgb in [[0.0, 0.0, 0.0], [2.5, 1.2, 0.4], [0.001, 0.999, 0.123]] {
            let got = oklab(Rgb(rgb));
            let canon = rune_motion::linear_to_oklab(rgb[0], rgb[1], rgb[2]);
            assert_eq!(got.map(f32::to_bits), canon.map(f32::to_bits), "{rgb:?}");
        }
        // Negative inputs can make LMS negative before `cbrt`, where `pack::oklab` clamps LMS to 0.
        let neg = [-0.5f32, 0.2, 0.1];
        assert_ne!(oklab(Rgb(neg)), rune_motion::linear_to_oklab(neg[0], neg[1], neg[2]));
        assert_ne!(oklab(Rgb(neg)), rune_motion::linear_to_oklab(neg[0].max(0.0), neg[1].max(0.0), neg[2].max(0.0)));
    }

    #[test]
    fn stops_keep_order_and_cap() {
        let s = Stops::hex(&[0xff0000, 0x00ff00, 0x0000ff]);
        assert_eq!(s.len(), 3);
        assert_eq!(s.as_slice()[1].hex(), 0x00ff00);
        assert_eq!(Stops::hex(&[0; 9]).len(), MAX_STOPS);
        assert_eq!(Stops::new(&[]).len(), 1);
    }
}
