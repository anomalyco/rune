//! Glyphs drawn from their outlines on the GPU, by Windfoil's closed-form box filter
//! (DesLauriers 2026) over quadratic pieces, with CoreText's outline dilation applied per pixel.
//! The store holds each glyph once for every size; a glyph quad names its record, pen, scale and
//! dilation radius, and the uber shader integrates its coverage.

mod build;
mod cover;
mod path;
mod store;

pub(crate) use path::{Emit, Frame, PathEffect, Paths};
pub(crate) use cover::coverage;
pub(crate) use store::{Found, Glyph, Key, OutlineStore, Support, WIDTH};

use swash::FontRef;
use swash::scale::ScaleContext;

/// What the uber shader needs to draw a glyph from its outline: a `KIND_GLYPH` instance with
/// `radii = [pen x, pen y, scale, zoom]` and `border = [record, hint, radius, coverage]`.
///
/// Coverage is computed in rest pixels, the glyph's own raster, and a device pixel spans
/// `1 / zoom` of them: a zoomed glyph is its resting outline magnified, so it never changes
/// shape when the zoom lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    /// Where the baseline origin sits, in device px.
    pub pen: [f32; 2],
    /// Rest px per font unit.
    pub scale: f32,
    /// Device px per rest px.
    pub zoom: f32,
    /// The glyph's record in the outline store.
    pub record: u32,
    /// Its CoreGraphics hint transform's record, 0 for none.
    pub hint: u32,
    /// The dilation radius, rest px.
    pub radius: f32,
    /// The coverage function (`coretext_match::Coverage`): 0 Windfoil's box, 1 Slug's rays.
    pub(crate) coverage: u32,
}

/// What fits a glyph's outline to the pixel grid before it's stored, at a ppem.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Hinting {
    None,
    /// The font's own TrueType hints.
    TrueType(f32),
    /// FreeType's light autohint (skrifa's): heights to the grid, widths kept.
    Auto(f32),
}

impl Hinting {
    /// The store key's part for it: 0, a ppem's bits, or those with the top bit for autohint.
    pub(crate) fn key(self) -> u32 {
        match self {
            Hinting::None => 0,
            Hinting::TrueType(ppem) => ppem.to_bits(),
            Hinting::Auto(ppem) => ppem.to_bits() | 1 << 31,
        }
    }
}

/// The text weight of `FontSmoothing::Windfoil`: CoreText's dilation radius times this. CoreText
/// flattens curves into chords before its coverage, which takes ink off every curve; Windfoil
/// integrates the curves exactly, so the same radius draws heavier: 0.93 puts its ink within 1% of
/// CoreText's for Inter at 1x, 1.5x and 2x, light and dark (`examples/text_gpu.rs`).
pub(crate) const TEXT_WEIGHT: f64 = 0.93;

/// Rest px the hint transform moves a point at most, which the shader looks past
/// (`OUTLINE_HINT_REACH` in shader.wgsl).
pub(crate) const HINT_REACH: f32 = 1.5;

impl Outline {
    /// The device px rect `[x, y, w, h]` that holds every pixel the glyph touches, from its ink
    /// box in font units.
    pub(crate) fn rect(&self, bbox: [f32; 4]) -> [f32; 4] {
        let (s, r, z) = (self.scale, self.radius, self.zoom);
        let reach = if self.hint != 0 { HINT_REACH } else { 0.0 };
        // Rest px, y up: sides grow by r, tops by 2r, and a pixel's reach either side.
        let x0 = bbox[0] * s - r - 1.0;
        let x1 = bbox[2] * s + r + 1.0;
        let y0 = bbox[1] * s - reach - 1.0;
        let y1 = bbox[3] * s + 2.0 * r + reach + 1.0;
        [self.pen[0] + x0 * z, self.pen[1] - y1 * z, (x1 - x0) * z, (y1 - y0) * z]
    }
}

/// `glyph`'s outline in font units leaning by `skew` degrees, built into a store record: the
/// design outline, or the one its TrueType hints fit to the pixel grid at `hinted` ppem.
pub(crate) fn extract(cx: &mut ScaleContext, font: FontRef<'_>, index: u32, coords: &[i16], glyph: u16, skew: u8, hinting: Hinting) -> Option<build::Built> {
    let upem = font.metrics(coords).units_per_em;
    let lean = if skew == 0 { 0.0 } else { (skew as f64).to_radians().tan() };
    let size = match hinting {
        Hinting::None => upem as f32,
        Hinting::TrueType(ppem) | Hinting::Auto(ppem) => ppem,
    };
    let mut built = if let Hinting::Auto(ppem) = hinting {
        let path = autohinted(font.data, index, coords, glyph, ppem)?;
        build::build(path.iter().copied(), upem as f64 / size as f64, lean)?
    } else {
        let mut scaler = cx.builder(font).size(size).hint(hinting != Hinting::None).normalized_coords(coords.iter().copied()).build();
        let outline = scaler.scale_outline(glyph)?;
        build::build(swash::zeno::PathData::commands(&outline.path()), upem as f64 / size as f64, lean)?
    };
    built.upem = upem as f32;
    Some(built)
}

/// `glyph`'s outline at `ppem` px, y up, after FreeType's light autohint (skrifa's), as Chromium
/// draws text on Linux with `hintslight`.
fn autohinted(data: &[u8], index: u32, coords: &[i16], glyph: u16, ppem: f32) -> Option<Vec<swash::zeno::Command>> {
    use skrifa::MetadataProvider;
    use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, SmoothMode, Target};
    let font = skrifa::FontRef::from_index(data, index).ok()?;
    let outlines = font.outline_glyphs();
    let coords: Vec<skrifa::instance::NormalizedCoord> = coords.iter().map(|&c| skrifa::instance::NormalizedCoord::from_bits(c)).collect();
    let options = HintingOptions {
        engine: Engine::Auto(None),
        target: Target::Smooth { mode: SmoothMode::Light, symmetric_rendering: true, preserve_linear_metrics: true },
    };
    let instance = HintingInstance::new(&outlines, skrifa::instance::Size::new(ppem), skrifa::instance::LocationRef::new(&coords), options).ok()?;
    let g = outlines.get(skrifa::GlyphId::new(glyph as u32))?;
    let mut pen = Pen(Vec::new());
    g.draw(DrawSettings::hinted(&instance, false), &mut pen).ok()?;
    Some(pen.0)
}

/// Collects skrifa's outline as zeno's commands.
struct Pen(Vec<swash::zeno::Command>);

impl skrifa::outline::OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) { self.0.push(swash::zeno::Command::MoveTo([x, y].into())); }
    fn line_to(&mut self, x: f32, y: f32) { self.0.push(swash::zeno::Command::LineTo([x, y].into())); }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) { self.0.push(swash::zeno::Command::QuadTo([cx0, cy0].into(), [x, y].into())); }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(swash::zeno::Command::CurveTo([cx0, cy0].into(), [cx1, cy1].into(), [x, y].into()));
    }
    fn close(&mut self) { self.0.push(swash::zeno::Command::Close); }
}
