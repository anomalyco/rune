//! Fills: styles that paint a node's whole shape, or a band inside its edge as a stroke. They share
//! the shape mask (`FillParams::border`), colour lists (`Stops`, blended in OKLab) and one input
//! for the styles that re-tone something (`FillInput`: a ramp, noise, the backdrop or an image).
//!
//! Every style is a pure function of (pixel, params, time, seed). Under reduced motion each one
//! holds a single settled pose. `catalog` lists the styles, their params and presets.

use crate::catalog::*;
use crate::*;

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

/// Linear sRGB to OKLab.
fn oklab(c: Rgb) -> [f32; 3] {
    let [r, g, b] = c.0;
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.max(0.0).cbrt(), m.max(0.0).cbrt(), s.max(0.0).cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
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
    fn code(self) -> f32 {
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

// ---------------------------------------------------------------- styles

/// Polished metal reflecting a studio: a soft overhead key light, a strip light, a bright sky
/// that falls to a hard horizon over a dark floor. The face is gently domed so the horizon
/// crosses it, the rim rolls off over `bevel`, and the surface flows like liquid.
/// As a border (`.border(w)`) it is a round tube.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiquidMetalParams {
    /// Reflectance at normal incidence: near white for chrome, warm for gold.
    pub tint: Rgb,
    /// The studio lights' colour.
    pub light: Rgb,
    /// How far the surface ripples (0 = a still, polished slab).
    pub flow: f32,
    /// Ripple size, pt.
    pub scale: f32,
    pub speed: f32,
    /// Width of the rolled rim, pt.
    pub bevel: f32,
    /// How domed the face is: 0 flat (one reflection everywhere), 1 strongly curved.
    pub dome: f32,
    /// Tilt of the studio around the view axis, radians.
    pub angle: f32,
    /// Thin-film colour where the surface bends hardest.
    pub iridescence: f32,
    /// 1 = studio as lit; higher deepens the floor and sharpens the lights.
    pub contrast: f32,
    /// Where the ripples are pinned: `World` (default) flows one surface across neighbouring
    /// shapes; `Element` gives every shape of the same size its own identical copy, so a row of
    /// keys matches.
    pub anchor: FillAnchor,
}

impl Default for LiquidMetalParams {
    fn default() -> Self {
        LiquidMetalParams::chrome()
    }
}

impl LiquidMetalParams {
    pub fn chrome() -> Self {
        LiquidMetalParams {
            tint: rgb(0xe9ecf0),
            light: rgb(0xffffff),
            flow: 0.6,
            scale: 100.0,
            speed: 0.35,
            bevel: 14.0,
            dome: 0.55,
            angle: -0.12,
            iridescence: 0.08,
            contrast: 1.0,
            anchor: FillAnchor::World,
        }
    }
    /// `LiquidMetalParams::chrome().anchor(FillAnchor::Element)`.
    pub fn anchor(mut self, anchor: FillAnchor) -> Self {
        self.anchor = anchor;
        self
    }
    pub fn gold() -> Self {
        LiquidMetalParams { tint: rgb(0xf3c56b), light: rgb(0xfff4dc), iridescence: 0.0, ..Self::chrome() }
    }
    pub fn rose() -> Self {
        LiquidMetalParams { tint: rgb(0xf2b7a2), light: rgb(0xfff0ea), iridescence: 0.05, ..Self::chrome() }
    }
    pub fn gunmetal() -> Self {
        LiquidMetalParams { tint: rgb(0x8d949e), light: rgb(0xe8f0ff), contrast: 1.35, iridescence: 0.12, ..Self::chrome() }
    }
}

params!(LiquidMetalParams {
    tint: "Tint" => ParamKind::Color,
    light: "Light" => ParamKind::Color,
    flow: "Flow" => num(0.0, 1.5, 0.01, Unit::None),
    scale: "Scale" => num(16.0, 480.0, 1.0, Unit::Pt),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    bevel: "Bevel" => num(0.0, 48.0, 0.5, Unit::Pt),
    dome: "Dome" => num(0.0, 1.0, 0.01, Unit::Fraction),
    angle: "Angle" => num(-3.14159, 3.14159, 0.01, Unit::Radians),
    iridescence: "Iridescence" => num(0.0, 1.0, 0.01, Unit::Fraction),
    contrast: "Contrast" => num(0.5, 2.0, 0.01, Unit::None),
    anchor: "Anchor" => ParamKind::Choice(FillAnchor::CHOICES),
});

/// Moulded plastic under one soft light: a diffuse base in `color`, a specular highlight that is
/// small and bright when glossy and broad and faint when matte, the dome's curvature shading, a
/// rim rolled over `bevel` that catches the light, and a faint reflection at grazing angles. The
/// face's flat centre shows `color`. Uses liquid metal's dome and bevel; it never animates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlasticParams {
    pub color: Rgb,
    /// 0 polished, 1 matte: the highlight's size and softness.
    pub roughness: f32,
    /// How curved the face is: positive bulges, negative is dished like a keycap's top.
    pub dome: f32,
    /// Width of the rolled rim, pt.
    pub bevel: f32,
    /// Where the light comes from, radians around the view axis: 0 from above, negative from the left.
    pub angle: f32,
}

impl Default for PlasticParams {
    fn default() -> Self {
        PlasticParams::glossy()
    }
}

impl PlasticParams {
    pub fn glossy() -> Self {
        PlasticParams { color: rgb(0xece8de), roughness: 0.15, dome: 0.35, bevel: 6.0, angle: -0.5 }
    }
    pub fn matte() -> Self {
        PlasticParams { color: rgb(0x2e3036), roughness: 0.8, ..Self::glossy() }
    }
    /// A matte face dished like a keycap's top.
    pub fn dished() -> Self {
        PlasticParams { color: rgb(0xd8d2c4), roughness: 0.7, dome: -0.45, ..Self::glossy() }
    }
}

params!(PlasticParams {
    color: "Colour" => ParamKind::Color,
    roughness: "Roughness" => num(0.05, 1.0, 0.01, Unit::Fraction),
    dome: "Dome" => num(-1.0, 1.0, 0.01, Unit::None),
    bevel: "Bevel" => num(0.0, 48.0, 0.5, Unit::Pt),
    angle: "Light angle" => num(-3.14159, 3.14159, 0.01, Unit::Radians),
});

/// Colours pinned to points that drift slowly, blended smoothly in OKLab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshGradientParams {
    pub colors: Stops,
    /// Each colour's rest point, 0..1 across the shape's rect.
    pub points: [[f32; 2]; MAX_STOPS],
    /// How far the points wander from rest, as a fraction of the rect.
    pub drift: f32,
    pub speed: f32,
    /// 1 = wide soft blends; higher keeps each colour to its own region.
    pub sharpness: f32,
    /// Organic bend of the blend field.
    pub warp: f32,
}

const MESH_POINTS: [[f32; 2]; MAX_STOPS] = [[0.12, 0.18], [0.86, 0.12], [0.5, 0.55], [0.14, 0.9], [0.9, 0.86], [0.62, 0.3]];

impl Default for MeshGradientParams {
    fn default() -> Self {
        MeshGradientParams::dusk()
    }
}

impl MeshGradientParams {
    pub fn dusk() -> Self {
        MeshGradientParams { colors: Stops::hex(&[0x2b1b5a, 0xff6f91, 0xffc46b, 0x6a4cff, 0xff9a5a]), points: MESH_POINTS, drift: 0.12, speed: 0.25, sharpness: 1.6, warp: 0.35 }
    }
    pub fn lagoon() -> Self {
        MeshGradientParams { colors: Stops::hex(&[0x0b3d5c, 0x2ec4b6, 0xcbf3f0, 0x1b6ca8]), ..Self::dusk() }
    }
    pub fn orchid() -> Self {
        MeshGradientParams { colors: Stops::hex(&[0xf9d5e5, 0xc06c84, 0x6c5b7b, 0xf8b195, 0x355c7d, 0xeeac99]), sharpness: 2.0, ..Self::dusk() }
    }
    pub fn graphite() -> Self {
        MeshGradientParams { colors: Stops::hex(&[0x15171c, 0x3a3f4b, 0x8a93a3, 0x22262e]), drift: 0.08, warp: 0.2, ..Self::dusk() }
    }
}

params!(MeshGradientParams {
    colors: "Colours" => ParamKind::Colors { min: 2, max: MAX_STOPS },
    points: "Points" => ParamKind::Points { max: MAX_STOPS },
    drift: "Drift" => num(0.0, 0.4, 0.01, Unit::Fraction),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    sharpness: "Sharpness" => num(0.5, 4.0, 0.01, Unit::None),
    warp: "Warp" => num(0.0, 1.0, 0.01, Unit::None),
});

/// Ordered dithering of a tone into a palette, in square pixels anchored to the shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DitherParams {
    pub pattern: DitherPattern,
    pub input: FillInput,
    /// Dark to light. Two colours is one-bit; more is a palette.
    pub colors: Stops,
    /// Size of one dither pixel, pt.
    pub pixel: f32,
    /// Linear input: ramp direction, radians.
    pub angle: f32,
    /// Noise input: feature size, pt.
    pub scale: f32,
    /// Noise input: drift speed.
    pub speed: f32,
    /// Shifts the tone before quantising (-1..1).
    pub bias: f32,
    pub contrast: f32,
}

impl Default for DitherParams {
    fn default() -> Self {
        DitherParams::bayer()
    }
}

impl DitherParams {
    /// One-bit Bayer 4×4 over a diagonal ramp.
    pub fn bayer() -> Self {
        DitherParams { pattern: DitherPattern::Bayer4, input: FillInput::Linear, colors: Stops::hex(&[0x14161c, 0xe8e4d8]), pixel: 2.0, angle: 0.6, scale: 90.0, speed: 0.2, bias: 0.0, contrast: 1.0 }
    }
    /// Four greens, Bayer 8×8, over drifting noise.
    pub fn handheld() -> Self {
        DitherParams { pattern: DitherPattern::Bayer8, input: FillInput::Noise, colors: Stops::hex(&[0x0f380f, 0x306230, 0x8bac0f, 0x9bbc0f]), pixel: 3.0, ..Self::bayer() }
    }
    /// Blue-noise one-bit over a radial ramp.
    pub fn grain() -> Self {
        DitherParams { pattern: DitherPattern::BlueNoise, input: FillInput::Radial, colors: Stops::hex(&[0x1b1530, 0xffd7a8]), pixel: 1.0, ..Self::bayer() }
    }
    /// Re-tones whatever is behind the layer in two inks.
    pub fn backdrop() -> Self {
        DitherParams { pattern: DitherPattern::Bayer4, input: FillInput::Backdrop, colors: Stops::hex(&[0x101418, 0xf2efe6]), pixel: 2.0, contrast: 1.35, bias: -0.1, ..Self::bayer() }
    }
}

params!(DitherParams {
    pattern: "Pattern" => ParamKind::Choice(DitherPattern::CHOICES),
    input: "Input" => ParamKind::Choice(FillInput::CHOICES),
    colors: "Colours" => ParamKind::Colors { min: 2, max: MAX_STOPS },
    pixel: "Pixel" => num(0.5, 16.0, 0.5, Unit::Pt),
    angle: "Angle" => num(-3.14159, 3.14159, 0.01, Unit::Radians),
    scale: "Noise scale" => num(8.0, 400.0, 1.0, Unit::Pt),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    bias: "Bias" => num(-1.0, 1.0, 0.01, Unit::None),
    contrast: "Contrast" => num(0.2, 4.0, 0.01, Unit::None),
});

/// A linear, radial or conic gradient whose colours scatter into film grain. A conic gradient
/// loops: its last colour blends back into the first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrainGradientParams {
    pub kind: GradientKind,
    pub colors: Stops,
    /// Linear: direction; conic: start, radians.
    pub angle: f32,
    /// Radial and conic centre, 0..1 across the rect.
    pub center: [f32; 2],
    /// How far grain scatters the gradient and its brightness (0..1).
    pub amount: f32,
    /// Grain size, pt.
    pub size: f32,
    /// One grain value for all channels; off gives colour grain.
    pub mono: bool,
    /// Grain re-rolls per second (0 = static).
    pub fps: f32,
}

impl Default for GrainGradientParams {
    fn default() -> Self {
        GrainGradientParams::ember()
    }
}

impl GrainGradientParams {
    pub fn ember() -> Self {
        GrainGradientParams { kind: GradientKind::Linear, colors: Stops::hex(&[0x1a0b2e, 0xc2185b, 0xff8a3d, 0xffe1a8]), angle: 1.2, center: [0.5, 0.5], amount: 0.4, size: 0.5, mono: true, fps: 12.0 }
    }
    pub fn halo() -> Self {
        GrainGradientParams { kind: GradientKind::Radial, colors: Stops::hex(&[0xe9f1ff, 0x7aa2ff, 0x2a2f7a, 0x0b0d1f]), center: [0.5, 0.35], ..Self::ember() }
    }
    pub fn sweep() -> Self {
        GrainGradientParams { kind: GradientKind::Conic, colors: Stops::hex(&[0x00c2a8, 0x3a86ff, 0x8338ec, 0xff006e, 0xffbe0b]), angle: -1.57, mono: false, ..Self::ember() }
    }
}

params!(GrainGradientParams {
    kind: "Kind" => ParamKind::Choice(GradientKind::CHOICES),
    colors: "Colours" => ParamKind::Colors { min: 2, max: MAX_STOPS },
    angle: "Angle" => num(-3.14159, 3.14159, 0.01, Unit::Radians),
    center: "Centre" => ParamKind::Point,
    amount: "Grain" => num(0.0, 1.0, 0.01, Unit::Fraction),
    size: "Grain size" => num(0.5, 8.0, 0.25, Unit::Pt),
    mono: "Mono" => ParamKind::Toggle,
    fps: "Grain rate" => num(0.0, 30.0, 1.0, Unit::PerSecond),
});

/// Reeded glass over the backdrop: parallel convex ribs, each showing a squeezed, flipped strip of
/// what is behind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlutedGlassParams {
    /// Rib width, pt.
    pub rib: f32,
    /// Rib direction, radians (0 = vertical ribs).
    pub angle: f32,
    /// How far each rib bends the image sideways, pt.
    pub refraction: f32,
    /// 0..1: clear to frosted (reads the blurred backdrop like a frosted lens).
    pub frost: f32,
    /// Colour split toward the rib edges.
    pub dispersion: f32,
    /// Sheen along each rib's lit side.
    pub highlight: f32,
    pub tint: Rgb,
    pub tint_amount: f32,
}

impl Default for FlutedGlassParams {
    fn default() -> Self {
        FlutedGlassParams::reeded()
    }
}

impl FlutedGlassParams {
    pub fn reeded() -> Self {
        FlutedGlassParams { rib: 14.0, angle: 0.0, refraction: 9.0, frost: 0.0, dispersion: 0.15, highlight: 0.5, tint: rgb(0xffffff), tint_amount: 0.0 }
    }
    pub fn fine() -> Self {
        FlutedGlassParams { rib: 6.0, refraction: 5.0, ..Self::reeded() }
    }
    pub fn frosted() -> Self {
        FlutedGlassParams { frost: 0.6, ..Self::reeded() }
    }
    pub fn wide() -> Self {
        FlutedGlassParams { rib: 32.0, refraction: 18.0, dispersion: 0.25, ..Self::reeded() }
    }
}

params!(FlutedGlassParams {
    rib: "Rib width" => num(2.0, 64.0, 0.5, Unit::Pt),
    angle: "Angle" => num(-3.14159, 3.14159, 0.01, Unit::Radians),
    refraction: "Refraction" => num(0.0, 40.0, 0.5, Unit::Pt),
    frost: "Frost" => num(0.0, 1.0, 0.01, Unit::Fraction),
    dispersion: "Dispersion" => num(0.0, 1.0, 0.01, Unit::Fraction),
    highlight: "Highlight" => num(0.0, 1.0, 0.01, Unit::Fraction),
    tint: "Tint" => ParamKind::Color,
    tint_amount: "Tint amount" => num(0.0, 1.0, 0.01, Unit::Fraction),
});

/// Glossy blobs that drift and merge, over a base colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetaballParams {
    /// Blob colours (each blob takes one in turn).
    pub colors: Stops,
    pub base: Rgb,
    pub count: u32,
    /// Blob radius as a fraction of the rect's shorter side.
    pub size: f32,
    pub speed: f32,
    /// Sheen on the blobs.
    pub gloss: f32,
}

impl Default for MetaballParams {
    fn default() -> Self {
        MetaballParams::lava()
    }
}

impl MetaballParams {
    pub fn lava() -> Self {
        MetaballParams { colors: Stops::hex(&[0xff5e3a, 0xff9a3c, 0xff2d6f]), base: rgb(0x1a0f1f), count: 6, size: 0.2, speed: 0.4, gloss: 0.5 }
    }
    pub fn mint() -> Self {
        MetaballParams { colors: Stops::hex(&[0x7cf2c4, 0x37d0e8, 0xb8f36b]), base: rgb(0x0d1b1e), ..Self::lava() }
    }
}

params!(MetaballParams {
    colors: "Colours" => ParamKind::Colors { min: 1, max: MAX_STOPS },
    base: "Base" => ParamKind::Color,
    count: "Count" => ParamKind::Count { min: 1, max: 8 },
    size: "Size" => num(0.05, 0.5, 0.01, Unit::Fraction),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    gloss: "Gloss" => num(0.0, 1.0, 0.01, Unit::Fraction),
});

/// Colours stirred into a slow swirl (warped noise through a ramp).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwirlParams {
    pub colors: Stops,
    /// Feature size, pt.
    pub scale: f32,
    pub speed: f32,
    /// Twist around the centre, turns at the middle.
    pub twist: f32,
    /// How much the noise folds into itself.
    pub warp: f32,
}

impl Default for SwirlParams {
    fn default() -> Self {
        SwirlParams::ink()
    }
}

impl SwirlParams {
    pub fn ink() -> Self {
        SwirlParams { colors: Stops::hex(&[0x0b1026, 0x3b2c85, 0x2fb3c9, 0xf2f6ff]), scale: 140.0, speed: 0.2, twist: 0.6, warp: 1.0 }
    }
    pub fn marble() -> Self {
        SwirlParams { colors: Stops::hex(&[0xf4f1ea, 0xc9c2b4, 0x6f6a61, 0xf4f1ea]), scale: 90.0, twist: 0.3, warp: 1.6, ..Self::ink() }
    }
}

params!(SwirlParams {
    colors: "Colours" => ParamKind::Colors { min: 2, max: MAX_STOPS },
    scale: "Scale" => num(16.0, 480.0, 1.0, Unit::Pt),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    twist: "Twist" => num(-2.0, 2.0, 0.01, Unit::None),
    warp: "Warp" => num(0.0, 2.5, 0.01, Unit::None),
});

/// Cells around drifting seed points, with optional seams.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoronoiParams {
    pub colors: Stops,
    /// Seam colour.
    pub edge_color: Rgb,
    /// Cell size, pt.
    pub scale: f32,
    pub speed: f32,
    /// Seam width, pt (0 = none).
    pub edge: f32,
    /// Shading toward each cell's rim (0 = flat cells).
    pub depth: f32,
}

impl Default for VoronoiParams {
    fn default() -> Self {
        VoronoiParams::cells()
    }
}

impl VoronoiParams {
    pub fn cells() -> Self {
        VoronoiParams { colors: Stops::hex(&[0x1f2a44, 0x2c3e66, 0x3d5a8a, 0x5577b0]), edge_color: rgb(0x0b0f1a), scale: 36.0, speed: 0.25, edge: 1.5, depth: 0.4 }
    }
    pub fn stained() -> Self {
        VoronoiParams { colors: Stops::hex(&[0xd7263d, 0xf46036, 0x2e294e, 0x1b998b, 0xc5d86d]), edge_color: rgb(0x111111), scale: 20.0, edge: 2.0, depth: 0.25, ..Self::cells() }
    }
}

params!(VoronoiParams {
    colors: "Colours" => ParamKind::Colors { min: 1, max: MAX_STOPS },
    edge_color: "Seam colour" => ParamKind::Color,
    scale: "Cell size" => num(6.0, 200.0, 1.0, Unit::Pt),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    edge: "Seam" => num(0.0, 8.0, 0.25, Unit::Pt),
    depth: "Depth" => num(0.0, 1.0, 0.01, Unit::Fraction),
});

/// Layered bands of colour with rolling, swaying edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveParams {
    /// One band per colour, back to front.
    pub colors: Stops,
    /// Edge height, as a fraction of the rect.
    pub amplitude: f32,
    /// Crests across the rect.
    pub frequency: f32,
    pub speed: f32,
    /// Direction the bands stack along, radians (0 = top to bottom).
    pub angle: f32,
    /// Edge softness, as a fraction of the rect.
    pub softness: f32,
}

impl Default for WaveParams {
    fn default() -> Self {
        WaveParams::sea()
    }
}

impl WaveParams {
    pub fn sea() -> Self {
        WaveParams { colors: Stops::hex(&[0xbfe7ff, 0x5fb3e8, 0x2a7bbf, 0x154a85, 0x0a2649]), amplitude: 0.05, frequency: 1.6, speed: 0.4, angle: 0.0, softness: 0.006 }
    }
    pub fn dunes() -> Self {
        WaveParams { colors: Stops::hex(&[0xfde6c4, 0xf4b77c, 0xe08a4f, 0xb35a3a, 0x6e3226]), amplitude: 0.07, frequency: 1.1, speed: 0.15, ..Self::sea() }
    }
}

params!(WaveParams {
    colors: "Colours" => ParamKind::Colors { min: 2, max: MAX_STOPS },
    amplitude: "Amplitude" => num(0.0, 0.25, 0.005, Unit::Fraction),
    frequency: "Frequency" => num(0.2, 6.0, 0.05, Unit::None),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    angle: "Angle" => num(-3.14159, 3.14159, 0.01, Unit::Radians),
    softness: "Softness" => num(0.0, 0.1, 0.001, Unit::Fraction),
});

/// Print-style dots on a rotated grid, sized by a tone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HalftoneParams {
    pub input: FillInput,
    pub ink: Rgb,
    /// The colour between the dots.
    pub base: Rgb,
    /// Dot pitch, pt.
    pub cell: f32,
    /// Screen angle, radians.
    pub angle: f32,
    /// Linear input: ramp direction, radians.
    pub ramp_angle: f32,
    /// Noise input: feature size, pt.
    pub scale: f32,
    pub speed: f32,
    pub bias: f32,
    pub contrast: f32,
}

impl Default for HalftoneParams {
    fn default() -> Self {
        HalftoneParams::print()
    }
}

impl HalftoneParams {
    pub fn print() -> Self {
        HalftoneParams { input: FillInput::Radial, ink: rgb(0x1b1b1f), base: rgb(0xf1ece1), cell: 6.0, angle: 0.785, ramp_angle: 0.6, scale: 90.0, speed: 0.2, bias: 0.0, contrast: 1.0 }
    }
    /// Re-screens whatever is behind the layer.
    pub fn backdrop() -> Self {
        HalftoneParams { input: FillInput::Backdrop, cell: 4.0, ..Self::print() }
    }
}

params!(HalftoneParams {
    input: "Input" => ParamKind::Choice(FillInput::CHOICES),
    ink: "Ink" => ParamKind::Color,
    base: "Base" => ParamKind::Color,
    cell: "Dot pitch" => num(2.0, 32.0, 0.5, Unit::Pt),
    angle: "Screen angle" => num(-3.14159, 3.14159, 0.01, Unit::Radians),
    ramp_angle: "Ramp angle" => num(-3.14159, 3.14159, 0.01, Unit::Radians),
    scale: "Noise scale" => num(8.0, 400.0, 1.0, Unit::Pt),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    bias: "Bias" => num(-1.0, 1.0, 0.01, Unit::None),
    contrast: "Contrast" => num(0.2, 4.0, 0.01, Unit::None),
});

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

    /// Pack into `i` (kind is `KIND_FILL`); `None` when the fill draws nothing.
    pub(crate) fn pack(&self, i: &mut InstRaw, rm: bool, time: f32) -> Option<Rect> {
        let r = rect_of(i);
        if self.draws_nothing(r) {
            return None;
        }
        let fin = |x: f32, d: f32| if x.is_finite() { x } else { d };
        // Looping styles settle on one pose under reduced motion, and fold large times to keep f32 precision.
        i.t[0] = if rm { CALM_T } else { cpu::fold_t(time) };
        i.t[2] = self.style.index() as f32;
        i.aux = [self.border, self.intensity.min(1.0), 0.0, 0.0];
        match self.style {
            FillStyle::LiquidMetal(p) => {
                i.c0 = p.tint.v4();
                i.c1 = p.light.v4();
                i.p0 = [fin(p.flow, 0.0).max(0.0), fin(p.scale, 100.0).max(1.0), fin(p.speed, 0.0), fin(p.bevel, 0.0).max(0.0)];
                i.p1 = [fin(p.dome, 0.0), fin(p.angle, 0.0), fin(p.iridescence, 0.0).max(0.0), fin(p.contrast, 1.0).max(0.1)];
                i.p2 = [(p.anchor == FillAnchor::Element) as u32 as f32, 0.0, 0.0, 0.0];
            }
            FillStyle::MeshGradient(p) => {
                i.p0 = [p.drift, p.speed, p.sharpness.clamp(0.1, 8.0), p.warp];
            }
            FillStyle::Dither(p) => {
                let pat = match p.pattern {
                    DitherPattern::Bayer2 => 1.0,
                    DitherPattern::Bayer4 => 2.0,
                    DitherPattern::Bayer8 => 3.0,
                    DitherPattern::BlueNoise => 4.0,
                    DitherPattern::White => 5.0,
                };
                i.p0 = [pat, p.input.code(), p.pixel.max(0.25), p.angle];
                i.p1 = [p.scale.max(1.0), p.speed, p.bias, p.contrast];
            }
            FillStyle::GrainGradient(p) => {
                let kind = match p.kind {
                    GradientKind::Linear => 0.0,
                    GradientKind::Radial => 1.0,
                    GradientKind::Conic => 2.0,
                };
                i.p0 = [kind, p.angle, p.center[0], p.center[1]];
                i.p1 = [p.amount, p.size.max(0.25), p.mono as u32 as f32, if rm { 0.0 } else { p.fps }];
            }
            FillStyle::FlutedGlass(p) => {
                i.c0 = p.tint.v4();
                i.p0 = [p.rib.max(1.0), p.angle, p.refraction, p.frost];
                i.p1 = [p.dispersion, p.highlight, p.tint_amount, 0.0];
            }
            FillStyle::Metaballs(p) => {
                i.c0 = p.base.v4();
                i.p0 = [p.count.clamp(1, 8) as f32, p.size, p.speed, p.gloss];
            }
            FillStyle::Swirl(p) => {
                i.p0 = [p.scale.max(1.0), p.speed, p.twist, p.warp];
            }
            FillStyle::Voronoi(p) => {
                i.c0 = p.edge_color.v4();
                i.p0 = [p.scale.max(2.0), p.speed, p.edge, p.depth];
            }
            FillStyle::Waves(p) => {
                i.p0 = [p.amplitude, p.frequency, p.speed, p.angle];
                i.p1 = [p.softness, 0.0, 0.0, 0.0];
            }
            FillStyle::Halftone(p) => {
                i.c0 = p.ink.v4();
                i.c1 = p.base.v4();
                i.p0 = [0.0, p.input.code(), p.cell.max(1.0), p.ramp_angle];
                i.p1 = [p.scale.max(1.0), p.speed, p.bias, p.contrast];
                i.p2 = [p.angle, 0.0, 0.0, 0.0];
            }
            FillStyle::Plastic(p) => {
                i.c0 = p.color.v4();
                i.p0 = [fin(p.roughness, 0.15).clamp(0.0, 1.0), fin(p.dome, 0.0), fin(p.bevel, 0.0).max(0.0), fin(p.angle, 0.0)];
            }
        }
        Some(r.inflate(1.0))
    }

    /// Per-frame tables: colour stops (linear, then OKLab; mesh points ride in `w`), metaball
    /// paths and the blue-noise tile. `xf` = (stops offset, stop count, extra offset, extra count).
    pub(crate) fn tables(&self, i: &mut InstRaw, rect: Rect, t: f32, seed: u32, data: &mut Vec<[f32; 4]>) {
        let stops = |data: &mut Vec<[f32; 4]>, s: &Stops, pts: Option<&[[f32; 2]]>| -> [f32; 2] {
            let first = data.len() as f32;
            for (k, c) in s.as_slice().iter().enumerate() {
                let p = pts.map_or([0.0, 0.0], |v| v[k]);
                let l = oklab(*c);
                data.push([c.0[0], c.0[1], c.0[2], p[0]]);
                data.push([l[0], l[1], l[2], p[1]]);
            }
            [first, s.len() as f32]
        };
        let [f, n] = match &self.style {
            FillStyle::MeshGradient(p) => {
                let pts = cpu::mesh_points(p, t, seed);
                stops(data, &p.colors, Some(&pts))
            }
            FillStyle::Dither(p) => stops(data, &p.colors, None),
            FillStyle::GrainGradient(p) => stops(data, &p.colors, None),
            FillStyle::Metaballs(p) => stops(data, &p.colors, None),
            FillStyle::Swirl(p) => stops(data, &p.colors, None),
            FillStyle::Voronoi(p) => stops(data, &p.colors, None),
            FillStyle::Waves(p) => stops(data, &p.colors, None),
            _ => {
                // Glass leaves `xf` to `blur_backdrop`, which marks the frost pyramid there.
                return;
            }
        };
        i.xf = [f, n, 0.0, 0.0];
        match &self.style {
            FillStyle::Dither(p) if p.pattern == DitherPattern::BlueNoise => {
                i.xf[2] = data.len() as f32;
                i.xf[3] = cpu::BLUE_N as f32;
                data.extend_from_slice(cpu::blue_noise());
            }
            FillStyle::Metaballs(p) => {
                i.xf[2] = data.len() as f32;
                i.xf[3] = p.count.clamp(1, 8) as f32;
                cpu::metaballs(p, rect, t, seed, data);
            }
            _ => {}
        }
    }
}

fn rect_of(i: &InstRaw) -> Rect {
    Rect::new(i.rect[0], i.rect[1], i.rect[2], i.rect[3])
}

#[cfg(test)]
mod tests {
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
    fn stops_keep_order_and_cap() {
        let s = Stops::hex(&[0xff0000, 0x00ff00, 0x0000ff]);
        assert_eq!(s.len(), 3);
        assert_eq!(s.as_slice()[1].hex(), 0x00ff00);
        assert_eq!(Stops::hex(&[0; 9]).len(), MAX_STOPS);
        assert_eq!(Stops::new(&[]).len(), 1);
    }
}
