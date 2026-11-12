//! A window's first frame painted on the CPU, for the moment before any GPU device exists.
//!
//! `paint` takes the same draw data the renderer consumes for a frame (the instance list after
//! layout, the glyph atlas pages and the image atlas pages) and fills a `Bgra8` buffer with the
//! bytes the GPU would write to a `Bgra8Unorm` swap chain for it: tiles of 64 × 32 pixels run
//! across threads, each instance blended into the 8-bit target in sRGB encoding as the GPU
//! blends (`Renderer::srgb_blend`), with the uber shader's coverage for each kind (`shade.rs`
//! mirrors shader.wgsl function for function).
//!
//! Text comes from the same masks the GPU samples. With [`attach_text`] the text system queues
//! Windfoil masks as it does for a renderer that fills them, and [`fill_masks`] fills them across
//! threads with the coverage function the GPU fill's shader runs (`text::outline::coverage`).
//! Those masks then belong to the CPU copy of the atlas: the GPU's first frame uploads and
//! samples the same texels, so text doesn't change at the hand-over.
//!
//! What it doesn't draw it doesn't guess at: [`Scene::needs_gpu`] names the first thing in a
//! frame that only the GPU can draw (a blurred or isolated layer, a backdrop blur, a compositor's
//! effect slot, a native surface, a glyph drawn from its outline), and `paint` refuses that frame
//! with the same answer. The supported set is the list in `NeedsGpu`'s negation, kept small on
//! purpose: a second renderer that drifts from the first shows as a flicker at the hand-over, so
//! [`drift`] counts how many pixels the GPU's frame changed when it took over.
//!
//! Nothing here runs or allocates until it's called: no statics to initialise, no tables.

use std::time::Instant;

use super::{Instance, KIND_BACKDROP_BLUR, KIND_EFFECT, KIND_GLYPH, KIND_LAYER, KIND_LAYER_END};
use crate::pixels::ImageAtlas;
use crate::text::{Atlas, FontSmoothing};

mod backdrop;
mod bin;
mod blend;
mod damage;
mod drift;
mod fill;
mod painter;
mod pool;
mod raster;
mod scroll;
#[doc(hidden)]
pub mod record;
mod shade;
pub(crate) mod tool;

pub use drift::{Drift, Pixels, drift, drift_wanted, last_drift};
pub use fill::{FillReport, attach_text, fill_masks};
pub use painter::{CpuPainter, Painted};

/// One frame's draw data, borrowed from the runtime that built it.
pub struct Scene<'a> {
    instances: &'a [Instance],
    glyph_pages: Vec<&'a [u8]>,
    /// The outline store, for glyphs drawn from their outlines (text whose scale moves).
    outlines: Option<&'a crate::text::outline::OutlineStore>,
    images: Vec<Image<'a>>,
    size: (u32, u32),
    clear: [f32; 4],
    /// The frame's text draws subpixel masks blended by channel (`Atlas::lcd`).
    lcd: bool,
    srgb_blend: bool,
    native_surfaces: bool,
    compositor: bool,
    smoothing: Option<FontSmoothing>,
    /// How many times the atlas has evicted a page (`Atlas::resets`): a glyph at the same place
    /// may be another mask after one.
    atlas_resets: u64,
    /// How the GPU that takes over rounds into half floats (layers, blur levels).
    half: HalfRounding,
}

/// How a GPU stores a float into an `Rgba16Float` target. Vendors differ, and a layer's pixels
/// come out a step apart when the CPU guesses wrong, so the painter follows the adapter the
/// window will use (`gpu::pick`, known before any device exists).
///
/// Measured on D3D12 (a mid-animation layer frame at 2880×1800 against each GPU's own frame of
/// the same draw data): Intel Iris Xe and NVIDIA RTX 3080 Ti both store toward zero (rounded to
/// nearest, 9% of that frame's pixels come out a step off; toward zero, 0.13–0.27%). D3D lets a
/// float target's conversion round either way, so a vendor not measured takes toward zero too;
/// if it rounds to nearest instead, layer pixels can come out a step off (two inside a nested
/// layer), and the drift count at the hand-over says so.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HalfRounding {
    #[default]
    TowardZero,
    /// To nearest, ties to even (IEEE's default).
    NearestEven,
}

/// PCI vendor IDs whose half-float rounding was measured.
const MEASURED: [(u32, HalfRounding); 2] = [(0x8086, HalfRounding::TowardZero), (0x10de, HalfRounding::TowardZero)];

impl HalfRounding {
    /// The rounding of the GPU with PCI vendor ID `vendor`: measured for Intel and NVIDIA,
    /// assumed (toward zero) for any other, AMD (0x1002) among them.
    pub fn of_vendor(vendor: u32) -> HalfRounding {
        MEASURED.iter().find(|m| m.0 == vendor).map_or(HalfRounding::TowardZero, |m| m.1)
    }
}

/// A scene's switches (`Scene::flags`).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Flags {
    srgb_blend: bool,
    native_surfaces: bool,
    compositor: bool,
    lcd: bool,
    smoothing: Option<FontSmoothing>,
    half: HalfRounding,
}

/// An image atlas page: RGBA8 texels, `width * 4` bytes a row.
#[derive(Clone, Copy)]
pub struct Image<'a> {
    width: u32,
    data: &'a [u8],
    /// Rows written since the renderer last uploaded the page (`ImagePage::dirty`).
    dirty: Option<(u32, u32)>,
}

impl<'a> Scene<'a> {
    /// The frame `instances` draws with `atlas`'s glyph masks and `images`' pages into a
    /// `size` (device pixels) target cleared to `clear` (`Runtime::clear_color`), blending in
    /// sRGB encoding as windows do by default.
    pub fn new(instances: &'a [Instance], atlas: &'a Atlas, images: &'a ImageAtlas, size: (u32, u32), clear: [f32; 4]) -> Self {
        let glyph_pages = (0..atlas.page_count()).filter_map(|p| atlas.page_data(p)).collect();
        let images = images.pages.iter().map(|p| Image { width: p.width, data: &p.data, dirty: p.dirty }).collect();
        Scene { instances, glyph_pages, outlines: Some(&atlas.outlines), images, size, clear, lcd: atlas.lcd, srgb_blend: true, native_surfaces: false, compositor: false, smoothing: None, atlas_resets: atlas.resets(), half: HalfRounding::default() }
    }

    /// Whether the frame blends in sRGB encoding (`Runtime::srgb_blend`). Linear blending needs
    /// the GPU's sRGB target.
    pub fn srgb_blend(mut self, on: bool) -> Self {
        self.srgb_blend = on;
        self
    }

    /// Whether the frame shows native surfaces (`Runtime::surfaces`: video, an embedded browser).
    pub fn native_surfaces(mut self, any: bool) -> Self {
        self.native_surfaces = any;
        self
    }

    /// Whether the window has a compositor (`App::compositor`), which draws into effect slots.
    /// Without one an effect slot draws nothing.
    pub fn compositor(mut self, has: bool) -> Self {
        self.compositor = has;
        self
    }

    /// Whether subpixel text blends by channel, as the GPU's subpixel pipeline does; it does when
    /// the atlas says so, unless the GPU couldn't (no dual-source blending).
    pub fn lcd(mut self, on: bool) -> Self {
        self.lcd = on;
        self
    }

    /// The scene-wide switches, which a frame that only recolours keeps.
    fn flags(&self) -> Flags {
        Flags { srgb_blend: self.srgb_blend, native_surfaces: self.native_surfaces, compositor: self.compositor, lcd: self.lcd, smoothing: self.smoothing, half: self.half }
    }

    /// How the GPU that will take over rounds half floats (`HalfRounding::of_vendor`).
    pub fn half_rounding(mut self, r: HalfRounding) -> Self {
        self.half = r;
        self
    }

    /// The text's font smoothing (`Runtime::font_smoothing`): Slug's coverage has no CPU form.
    pub fn font_smoothing(mut self, mode: FontSmoothing) -> Self {
        self.smoothing = Some(mode);
        self
    }

    /// The first thing in this frame only the GPU can draw, or None when `paint` draws all of it.
    pub fn needs_gpu(&self) -> Option<NeedsGpu> {
        if !self.srgb_blend { return Some(NeedsGpu::LinearBlend); }
        if self.native_surfaces { return Some(NeedsGpu::NativeSurface); }
        if self.smoothing == Some(FontSmoothing::Slug) { return Some(NeedsGpu::SlugText); }
        // How many layer scopes are open: a backdrop blur inside one reads that layer's target.
        let mut depth = 0u32;
        self.instances.iter().find_map(|i| match i.params[1] {
            k if k == KIND_LAYER && i.params[2] != 0.0 => Some(NeedsGpu::LayerBlur),
            k if k == KIND_LAYER && i.border[1] != 0.0 => Some(NeedsGpu::TurnedLayer),
            k if k == KIND_LAYER => { depth += 1; None }
            k if k == KIND_LAYER_END => { depth = depth.saturating_sub(1); None }
            k if k == KIND_BACKDROP_BLUR && depth > 0 => Some(NeedsGpu::BackdropBlur),
            k if k == KIND_EFFECT && self.compositor => Some(NeedsGpu::Effect),
            // Slug's rays (coverage 1) have no CPU form; Windfoil's box does (outline::coverage).
            k if k == KIND_GLYPH && i.radii[3] > 0.0 && (self.outlines.is_none() || i.border[3] == 1.0) => Some(NeedsGpu::OutlineGlyph),
            k if !(0.0..=13.0).contains(&k) || k.fract() != 0.0 => Some(NeedsGpu::UnknownKind),
            _ => None,
        })
    }
}

/// Why a frame needs the GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedsGpu {
    /// The frame blends in linear light (`App::srgb_blend(false)`).
    LinearBlend,
    /// It shows a native surface.
    NativeSurface,
    /// Its text uses Slug's coverage.
    SlugText,
    /// A layer blurred as a whole (`El::blur`).
    LayerBlur,
    /// A layer turned as a whole (`Instance::turned`).
    TurnedLayer,
    /// A backdrop blur inside a layer (it blurs that layer's target).
    BackdropBlur,
    /// An effect slot a compositor draws into.
    Effect,
    /// A glyph drawn from its outline with Slug's coverage, or with no outline store at hand.
    OutlineGlyph,
    /// An instance kind this painter doesn't know.
    UnknownKind,
    /// The target isn't the scene's size, or its buffer is too small.
    Target,
}

/// Where `paint` writes: `width × height` pixels, four bytes each in memory order B, G, R, A,
/// `stride` bytes from one row's start to the next, row 0 at the top.
pub struct Target<'a> {
    pixels: &'a mut [u8],
    width: u32,
    height: u32,
    stride: usize,
}

impl<'a> Target<'a> {
    /// Tightly packed rows (`width * 4` bytes), as a top-down 32-bit DIB section lays them out.
    pub fn new(pixels: &'a mut [u8], width: u32, height: u32) -> Self {
        Target { pixels, width, height, stride: width as usize * 4 }
    }

    /// Rows `stride` bytes apart.
    pub fn with_stride(pixels: &'a mut [u8], width: u32, height: u32, stride: usize) -> Self {
        Target { pixels, width, height, stride }
    }
}

/// How a `paint` went: each stage's time and what it drew.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub threads: usize,
    /// Binning instances into tiles, on the calling thread.
    pub bin_ns: u64,
    /// Painting every tile, all threads, wall clock.
    pub raster_ns: u64,
    pub total_ns: u64,
    pub tiles: usize,
    /// Instance-tile pairs painted, and those a later opaque instance covering the whole tile
    /// made unnecessary.
    pub binned: usize,
    pub culled: usize,
    /// Tiles cleared to the clear colour (the rest begin under an opaque instance).
    pub cleared: usize,
    /// Pixels written by each path: constant spans, per-pixel shading, glyph texels.
    pub span_px: u64,
    pub shaded_px: u64,
    pub glyph_px: u64,
    /// The longest one tile took: no thread count paints faster.
    pub slowest_tile_ns: u64,
    /// Taking backdrop blurs, on the calling thread between phases.
    pub blur_ns: u64,
}

/// Paints `scene` into `target` on `threads` threads (the calling one among them), or says why
/// only the GPU can.
pub fn paint(scene: &Scene<'_>, target: &mut Target<'_>, threads: usize) -> Result<Report, NeedsGpu> {
    if let Some(why) = scene.needs_gpu() { return Err(why); }
    check_target(scene, target)?;
    let start = Instant::now();
    let bins = bin::Bins::of(scene);
    let binned = Instant::now();
    let tiles: Vec<u32> = (0..bins.tile_count() as u32).collect();
    let mut report = raster::run(scene, &bins, target, &tiles, &raster::Spawn(threads.max(1)), &raster::Spawn(threads.max(1)));
    let done = Instant::now();
    report.bin_ns = ns(binned - start);
    report.raster_ns = ns(done - binned);
    report.total_ns = ns(done - start);
    report.tiles = bins.tile_count();
    report.binned = bins.len();
    report.culled = bins.culled;
    Ok(report)
}

/// Whether `target` is `scene`'s size, with rows at least a row of pixels apart.
fn check_target(scene: &Scene<'_>, target: &Target<'_>) -> Result<(), NeedsGpu> {
    let (w, h) = scene.size;
    let fits = target.width == w && target.height == h && target.stride >= w as usize * 4
        && target.pixels.len() >= target.stride * h.saturating_sub(1) as usize + w as usize * 4;
    if fits { Ok(()) } else { Err(NeedsGpu::Target) }
}

fn ns(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}
