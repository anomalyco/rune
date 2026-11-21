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
mod tests;
