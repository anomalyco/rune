//! Per-style fill parameter structs, presets, and catalog parameter tables.

use std::f32::consts::PI;

use super::{DitherPattern, FillAnchor, FillInput, GradientKind, MAX_STOPS, Stops};
use crate::catalog::*;
use crate::{Rgb, rgb};

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
    angle: "Angle" => num(-PI, PI, 0.01, Unit::Radians),
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
    angle: "Light angle" => num(-PI, PI, 0.01, Unit::Radians),
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
    angle: "Angle" => num(-PI, PI, 0.01, Unit::Radians),
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
    angle: "Angle" => num(-PI, PI, 0.01, Unit::Radians),
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
    angle: "Angle" => num(-PI, PI, 0.01, Unit::Radians),
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
    angle: "Angle" => num(-PI, PI, 0.01, Unit::Radians),
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
    angle: "Screen angle" => num(-PI, PI, 0.01, Unit::Radians),
    ramp_angle: "Ramp angle" => num(-PI, PI, 0.01, Unit::Radians),
    scale: "Noise scale" => num(8.0, 400.0, 1.0, Unit::Pt),
    speed: "Speed" => num(0.0, 2.0, 0.01, Unit::PerSecond),
    bias: "Bias" => num(-1.0, 1.0, 0.01, Unit::None),
    contrast: "Contrast" => num(0.2, 4.0, 0.01, Unit::None),
});
