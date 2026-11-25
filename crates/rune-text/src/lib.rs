//! Text shaping, layout, font discovery, glyph rasterization, outline filling and icon rendering
//! for Rune: Parley layout (wrapping, rich spans, registered families) over embedded and system
//! fonts, with swash and platform pipelines rasterising glyphs and SVG icons into one greyscale
//! atlas. Apps normally reach this crate through `rune::text`.
//!
//! Invariants: layouts are in device pixels; every layout is cached by a 64-bit hash of
//! (text, spans, style, scale, width), kept for two commits unused and then while a budget of
//! stale layouts lasts (`STALE_BYTES`), least recently used out first; a width not cached re-breaks
//! a cached layout of the same text and style rather than shaping it again; glyph keys include
//! the run's variation coordinates, so one variable font serves every weight.

use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use parley::fontique::{Collection, CollectionOptions, FallbackKey, FamilyId, Script, SourceCache};
use parley::{Alignment, FontContext, FontFamilyName, FontStyle, FontWeight, Layout, LayoutContext, PositionedLayoutItem};
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};

use rune_core::color::Color;
use rune_core::{HashMap as FxHashMap, HashSet as FxHashSet};
use rune_core::tok::{Family, TextStyle};

mod atlas;
mod block;
mod direction;
#[doc(hidden)]
pub mod fonts;
#[doc(hidden)]
pub mod icon;
mod layout;
mod measure;
#[doc(hidden)]
pub mod outline;
mod paragraphs;
#[doc(hidden)]
pub mod profile;
mod raster;
#[doc(hidden)]
pub mod segmenter;

pub use atlas::ATLAS_SIZE;
pub use atlas::Atlas;
pub use atlas::AtlasPage;
pub use atlas::GlyphQuad;
#[doc(hidden)]
pub use atlas::fill::QueuedFill;
#[doc(hidden)]
pub use atlas::fill_bench;
#[doc(hidden)]
pub use atlas::glyph_emit::GLIDE_PHASES;
pub use atlas::mask_cache;
pub use block::Block;
pub use block::MAX_LIST_LEVEL;
pub use direction::Direction;
#[doc(hidden)]
pub use direction::Flow;
#[doc(hidden)]
pub use direction::TextDir;
#[doc(hidden)]
pub use direction::first_strong_char;
#[doc(hidden)]
pub use direction::paragraph_ranges;
pub use fonts::collate::compare_names;
#[doc(hidden)]
pub use fonts::face_weights;
pub use fonts::face_weights::FaceWeights;
pub use fonts::face_weights::set_face_weights;
pub use fonts::fallback::is_pictographic;
#[doc(hidden)]
pub use fonts::families::choose_emoji_family;
#[doc(hidden)]
pub use fonts::families::choose_families;
#[doc(hidden)]
pub use fonts::families::normalize_emoji_family;
#[doc(hidden)]
pub use fonts::families::resolved_family_of;
#[doc(hidden)]
pub use fonts::full_faces;
pub use fonts::full_faces::add_font;
pub use fonts::language::set_language;
pub use fonts::system_fonts::InstalledFamily;
pub use fonts::system_fonts::installed_families;
#[doc(hidden)]
pub use icon::Facing;
pub use icon::Icon;
#[doc(hidden)]
pub use icon::anim::{AnimState, AnimatedQuad};
#[doc(hidden)]
pub use icon::color::IconKind;
pub use icon::icon_coverage;
pub use icon::icon_coverage_placed;
pub use icon::set_icon_swap;
pub use icon::set_icon_swap_size;
#[doc(hidden)]
pub use layout::box_decos::span_box_decorations;
#[doc(hidden)]
pub use layout::cache::CachedElision;
#[doc(hidden)]
pub use layout::cache::Keep;
#[doc(hidden)]
pub use layout::case::Source as CaseSource;
pub use layout::case::TextCase;
#[doc(hidden)]
pub use layout::cursor::cursor_from_point;
#[doc(hidden)]
pub use layout::cursor::cursor_geometry;
#[doc(hidden)]
pub use layout::cursor::line_edge;
#[doc(hidden)]
pub use layout::cursor::move_line;
#[doc(hidden)]
pub use layout::cursor::range_boxes;
#[doc(hidden)]
pub use layout::cursor::space_ahead;
#[doc(hidden)]
pub use layout::cursor::visual_step;
pub use layout::decorations::Deco;
#[doc(hidden)]
pub use layout::decorations::PaintedDeco;
pub use layout::decorations::SPAN_RADIUS;
#[doc(hidden)]
pub use layout::hash::directed;
#[doc(hidden)]
pub use layout::hash::elide_key;
#[doc(hidden)]
pub use layout::hash::mix_span_boxes;
#[doc(hidden)]
pub use layout::hash::shape_key;
pub use layout::hash::text_hash;
#[doc(hidden)]
pub use layout::id::LayoutId;
pub use layout::line_metrics::last_line_fill;
pub use layout::line_metrics::layout_height;
#[doc(hidden)]
pub use layout::line_metrics::spec_height;
pub use layout::line_metrics::stream_height_lag;
#[doc(hidden)]
pub use layout::line_metrics::text_width;
pub use layout::ltr_units::ltr_units;
#[doc(hidden)]
pub use layout::pill_chips::bare_icons;
#[doc(hidden)]
pub use layout::pill_chips::pill_chips;
#[doc(hidden)]
pub use layout::pill_chips::pill_chips_with_images;
#[doc(hidden)]
pub use layout::pretty::pretty_wrap;
pub use layout::pretty::set_pretty_wrap;
pub use layout::rich::RichLayoutOptions;
#[doc(hidden)]
pub use layout::rich::TextInput;
pub use layout::run::Run;
pub use layout::run::RunLayout;
#[doc(hidden)]
pub use layout::span::BareIcon;
pub use layout::span::Ink;
pub use layout::span::Link;
pub use layout::span::LinkPaint;
#[doc(hidden)]
pub use layout::span::PillChip;
#[doc(hidden)]
pub use layout::span::PillMetrics;
#[doc(hidden)]
pub use layout::span::Replace;
pub use layout::span::Span;
pub use layout::span::SpanBox;
#[doc(hidden)]
pub use layout::span::SpanBoxRun;
pub use layout::span::TextHighlight;
#[doc(hidden)]
pub use layout::span::flatten_spans;
#[doc(hidden)]
pub use layout::underline::line_end;
#[doc(hidden)]
pub use layout::underline::underline_edges;
#[doc(hidden)]
pub use layout::underline::underline_edges_with_end;
pub use measure::FamiliesGuard;
pub use measure::FontsGuard;
pub use measure::TextInk;
pub use measure::advance;
#[doc(hidden)]
pub use measure::ascii_metrics;
pub use measure::content_widths;
pub use measure::font_epoch;
pub use measure::ink;
#[doc(hidden)]
pub use measure::lend;
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub use measure::line_starts;
pub use measure::measure;
pub use measure::measure_with;
#[doc(hidden)]
pub use measure::primary_extents;
pub use measure::with_families;
pub use measure::with_fonts;
pub use outline::Outline;
#[doc(hidden)]
pub use paragraphs::AsciiLine;
#[doc(hidden)]
pub use paragraphs::BlockLine;
#[doc(hidden)]
pub use paragraphs::Hit;
#[doc(hidden)]
pub use paragraphs::Paragraph;
#[doc(hidden)]
pub use paragraphs::ParagraphLayout;
#[doc(hidden)]
pub use paragraphs::Piece;
#[doc(hidden)]
pub use paragraphs::Pieces;
pub use paragraphs::TextFlow;
#[doc(hidden)]
pub use paragraphs::TextLayout;
pub use paragraphs::common_end;
pub use paragraphs::common_start;
pub use parley;
#[doc(hidden)]
pub use raster::coretext_match;
pub use raster::coretext_match::FontSmoothing;
pub use raster::macos::smoothing::ApplePipeline;
pub use raster::support::SmoothingSupport;
#[doc(hidden)]
pub use raster::support::note_dual_source;

use atlas::{AtlasGlyph, COLOR_GLYPH_BUDGET, ColorGlyph, GlyphKey, GlyphMap, Rasterized};
#[cfg(test)]
use atlas::{MAX_OVERFLOW_PAGES, MAX_STEADY_PAGES};
use atlas::{glyph_cache, glyph_emit};
use atlas::glyph_emit::Face;
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
use fonts::default_fonts;
use fonts::default_fonts::DEFAULT_FONTS;
use fonts::hidden_axes::hidden_axes;
#[cfg(target_arch = "wasm32")]
use fonts::platform_face;
#[cfg(test)]
use fonts::fallback::Route;
#[cfg(test)]
use fonts::system_fonts::{CJK_FONT_FILES, system_font_files};
use fonts::system_fonts::{LoadedFiles, family_list, load_installed, load_system_mono, named_list, names_system_mono};
use fonts::trak::{Trak, Traks, font_trak};
use fonts::{fallback, families, system_fonts, system_weight};
use icon::anim::AnimatedIcons;
use icon::color::ColorIcons;
use icon::rasterize_icon;
use layout::cache::{Cache, Entry};
#[cfg(test)]
use layout::cache::STALE_BYTES;
use layout::decorations::Decoration;
use layout::id::LayoutIds;
use layout::rich::mono_tab_stops;
use layout::span::{HIDDEN_SIZE, PILL_BOX, SPAN_BOX, is_hidden_size};
use layout::{cursor, hash, line_break, line_metrics, span};
use raster::{macos, support, windfoil, windows};

/// Lean of a synthetic oblique, in degrees, for faces with no italic.
const SYNTHETIC_OBLIQUE: u8 = 12;

/// The colour image of `glyph` (COLR layers or an embedded bitmap scaled to size), or None
/// when the font draws it as an outline.
fn rasterize_color(scaler: &mut swash::scale::Scaler<'_>, glyph: u16) -> Option<(rune_image::pixels::Pixels, swash::zeno::Placement)> {
    if !scaler.has_color_bitmaps() && !scaler.has_color_outlines() {
        return None;
    }
    let image = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit)]).render(scaler, glyph)?;
    let (w, h) = (image.placement.width, image.placement.height);
    if image.content != Content::Color || w == 0 || h == 0 || image.data.len() != (w * h * 4) as usize {
        return None;
    }
    let pixels = rune_image::pixels::Pixels::new();
    pixels.set(w, h, image.data);
    Some((pixels, image.placement))
}

// ---------------------------------------------------------------------------------------------

pub struct TextSystem {
    font_cx: FontContext,
    layout_cx: LayoutContext<Ink>,
    scale_cx: ScaleContext,
    layouts: Cache<Rc<Layout<Ink>>>,
    /// The `layouts` key of the latest layout of each shaping (see `layout_blocks_inner`), whose
    /// glyphs a new width re-breaks instead of shaping the text again.
    shapes: HashMap<u64, u64>,
    /// Shapings whose text was edited this commit (see [`Self::supersede`]).
    superseded: FxHashSet<u64>,
    elisions: Cache<CachedElision>,
    /// By-paragraph field layouts ([`ParagraphLayout`]), by their text's break key.
    paragraphs: Cache<Rc<ParagraphLayout>>,
    /// The last by-paragraph layout made this commit, which its text at another width breaks from.
    last_paragraphs: Option<Rc<ParagraphLayout>>,
    /// The bytes the last two texts compared this commit share at their start and end, and the
    /// two texts (address and length): measuring a field at its two widths compares it once.
    last_diff: Option<([usize; 4], usize, usize)>,
    /// A long paste's lines as one width's keying read them this layout pass, for the other's.
    line_scan: Option<paragraphs::LineScan>,
    /// Printable ASCII's advances and line height per style and scale (`paragraphs::AsciiLine`).
    ascii: HashMap<u64, Option<Rc<paragraphs::AsciiLine>>>,
    runs: Cache<Rc<RunLayout>>,
    generation: u32,
    /// Incremented whenever [`Self::drop_shaped`] clears shaped caches so stale
    /// [`ParagraphLayout`]s are not reused.
    shaped_epoch: u64,
    /// The id of the layout at each address one was built or re-broken at (`layout::id`).
    layout_ids: LayoutIds,
    /// The slots' families as named; `sans` and `mono` are the lists text asks for.
    families: (String, String),
    /// The `choose_families` change these slots follow.
    chosen: u64,
    /// The language text shapes in, and the `set_language` change it follows.
    language: Option<parley::fontique::Language>,
    language_epoch: u64,
    /// How many of the faces a web app adds after its first frame it registered (`full_faces`).
    faces_taken: usize,
    pub sans: String,
    pub mono: String,
    symbol_families: Vec<&'static str>,
    /// The colour emoji family among `symbol_families`.
    emoji: Option<&'static str>,
    /// Opt-in app-preferred emoji family (`Self::set_emoji_family`), tried ahead of selected
    /// active-stack colour fonts and the lazily loaded system `emoji` family on `Route::Emoji`.
    preferred_emoji: Option<String>,
    /// The installed math families, once a text has needed them.
    math: Option<Vec<&'static str>>,
    /// The installed monospace families with Arabic and Hebrew (`fallback::MONO_SCRIPT_FAMILIES`),
    /// once a text has needed them: code keeps its columns in those scripts too.
    mono_scripts: Option<Vec<&'static str>>,
    /// The installed `Family::SystemMono` families, once a text has needed them.
    system_mono: Option<Vec<String>>,
    /// Whether the sans (`[0]`) or mono (`[1]`) slot names `ui-monospace` (see `family_list`).
    slot_system_mono: [bool; 2],
    /// Bit `i` is set once `fallback::SCRIPT_FONTS[i]` has been looked up.
    scripts: u64,
    font_files: LoadedFiles,
    /// The system fonts looked up so far, and the app's own (`load_fallbacks`).
    fallbacks: system_fonts::Fallbacks,
    /// The loaded system faces' families, for the face DirectWrite picks (`system_weight`).
    system_faces: system_weight::Faces,
    /// Installed families looked up by name (`load_installed`), lowercase.
    installed: std::collections::HashSet<String>,
    pub atlas: Atlas,
    color_glyphs: GlyphMap<ColorGlyph>,
    color_pixels: Vec<rune_image::pixels::Pixels>,
    color_bytes: usize,
    /// Colour glyphs placed so far, painted or from the mask cache: with the atlas's count, a
    /// change the cache's next write must carry.
    color_placed: u64,
    color_icons: ColorIcons,
    animated_icons: AnimatedIcons,
    /// Layouts shaped, a running count (shaping work counter for perf gates).
    pub shaped: u32,
    /// Layouts re-broken at a new width from an earlier shaping, a running count.
    pub rebroken: u32,
    /// Reused across lines and frames when mapping glyphs back to cluster start bytes.
    cluster_scratch: Vec<u32>,
    /// Byte ranges `(start, end, ink)` of coloured spans set by [`Self::set_glyph_inks`].
    ink_spans: Vec<(u32, u32, u32)>,
    coords_scratch: Vec<i16>,
    traks: Traks,
    /// The mode glyphs draw in: `Self::requested`'s, or its grayscale twin while subpixel
    /// text can't draw (`FontSmoothing::drawn`).
    #[doc(hidden)]
    pub smoothing: FontSmoothing,
    /// The mode the app asked for ([`Self::set_font_smoothing`]).
    requested: FontSmoothing,
    /// The `trak` table of the first font of a family slot at a weight, upright or italic.
    primary_traks: FxHashMap<(Family, u32, bool), Option<Rc<Trak>>>,
    /// The hidden-axis coordinates (`hidden_axes`) of the same first font.
    primary_axes: FxHashMap<(Family, u32, bool), Rc<[parley::FontVariation]>>,
    /// CoreGraphics' glyph pipeline for `smoothing`, and its per-ink state.
    apple: macos::smoothing::Smoothing,
    /// DirectWrite for `FontSmoothing::DirectWrite`, and its per-ink state.
    windows: windows::Rasterizer,
    #[doc(hidden)]
    pub glyph_cache: glyph_cache::GlyphCache,
    /// Device px `[x, y]` the next glyphs glide from their resting pixel ([`Self::set_glide`]).
    glide: [f32; 2],
    /// How the glyphs placed next draw ([`Self::begin_text`], [`Self::set_emit`]).
    emit: outline::Emit,
    /// Each painted text's raster path (`outline::RasterPath`), by node.
    paths: outline::Paths,
    /// What each face (font, index) is, for drawing it from outlines.
    face_kinds: FxHashMap<(u64, u32), windfoil::FaceKind>,
    /// Masks kept across launches (`mask_cache`).
    masks: mask_cache::Masks,
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new(&[], ("Inter", "ui-monospace"))
    }
}

impl TextSystem {
    /// `fonts` replaces the built-in Inter faces when non-empty on native. Web keeps its
    /// bundled sans and mono fallbacks, since there are no system fonts to discover.
    /// `families` names the sans and mono slots; others are selectable with `Family::Named`.
    pub fn new(fonts: &[&'static [u8]], families: (&str, &str)) -> Self {
        measure::note_fonts(fonts, families);
        Self::new_local(fonts, families)
    }

    /// Builds a [`TextSystem`] without updating the process-wide measurement state, for
    /// [`measure::with_text`]'s thread-local measurer.
    pub(crate) fn new_local(fonts: &[&'static [u8]], families: (&str, &str)) -> Self {
        // Windows' own word dictionaries for the scripts that need one (loaded when first asked).
        #[cfg(all(target_os = "windows", not(feature = "segmenter-dictionaries")))]
        crate::segmenter::install();
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        let list: &[&'static [u8]] = if fonts.is_empty() { DEFAULT_FONTS } else { fonts };
        let mut registered = Vec::new();
        for data in list {
            let mut added = face_weights::register(&mut collection, data);
            added.sort_unstable();
            registered.extend(added);
        }
        // A web app's own fonts first: the embedded faces fill in only a family it brought none
        // of, so they never stand in for its own, and the full Inter faces are fetched only when
        // the embedded Inter is in use.
        #[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
        {
            let fill: Vec<bool> = default_fonts::DEFAULT_FAMILIES.iter().map(|f| fonts.is_empty() || collection.family_id(f).is_none()).collect();
            for (data, _) in DEFAULT_FONTS.iter().zip(&fill).filter(|(_, fill)| **fill && !fonts.is_empty()) {
                face_weights::register(&mut collection, data);
            }
            if fill[0] {
                full_faces::note_embedded_inter();
            }
        }
        // In a browser, the face the page draws what no other font has (`platform_face`).
        #[cfg(target_arch = "wasm32")]
        platform_face::register(&mut collection);
        // No system font loads here: each comes the first time a text needs it (`load_fallbacks`).
        #[cfg(target_arch = "wasm32")]
        let families = if fonts.is_empty() { ("Inter", "IBM Plex Mono") } else { families };
        let (sans, mono) = families::slots(families.0, families.1);
        let mut text = TextSystem {
            font_cx: FontContext { collection, source_cache: SourceCache::default() },
            families: (families.0.to_owned(), families.1.to_owned()),
            sans,
            mono,
            slot_system_mono: [names_system_mono(families.0), names_system_mono(families.1)],
            fallbacks: system_fonts::Fallbacks::new(&registered),
            atlas: Atlas::new(),
            ..Self::empty()
        };
        text.follow_full_faces();
        text.load_installed(families.0);
        text.load_installed(families.1);
        text.sync_cjk_fallbacks();
        text.load_slot_fallbacks();
        text
    }

    /// An unallocated placeholder swapped into a caller's slot while [`measure::lend`] holds the
    /// live [`TextSystem`] in thread-local storage.
    pub(crate) fn empty() -> Self {
        let collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        TextSystem {
            font_cx: FontContext { collection, source_cache: SourceCache::default() },
            layout_cx: LayoutContext::new(),
            scale_cx: ScaleContext::new(),
            layouts: Default::default(),
            shapes: Default::default(),
            superseded: Default::default(),
            elisions: Default::default(),
            paragraphs: Default::default(),
            last_paragraphs: None,
            last_diff: None,
            line_scan: None,
            ascii: HashMap::default(),
            runs: Default::default(),
            generation: 0,
            shaped_epoch: 0,
            layout_ids: LayoutIds::default(),
            families: (String::new(), String::new()),
            chosen: 0,
            language: None,
            language_epoch: 0,
            faces_taken: 0,
            sans: String::new(),
            mono: String::new(),
            symbol_families: Vec::new(),
            emoji: None,
            preferred_emoji: None,
            math: None,
            mono_scripts: None,
            system_mono: None,
            slot_system_mono: [false; 2],
            scripts: 0,
            font_files: LoadedFiles::new(),
            fallbacks: system_fonts::Fallbacks::default(),
            system_faces: Default::default(),
            installed: Default::default(),
            atlas: Atlas::empty(),
            color_glyphs: GlyphMap::default(),
            color_pixels: Vec::new(),
            color_bytes: 0,
            color_placed: 0,
            color_icons: ColorIcons::default(),
            animated_icons: AnimatedIcons::default(),
            shaped: 0,
            rebroken: 0,
            cluster_scratch: Vec::new(),
            ink_spans: Vec::new(),
            coords_scratch: Vec::new(),
            traks: Traks::new(),
            smoothing: FontSmoothing::default(),
            requested: FontSmoothing::default(),
            primary_traks: Default::default(),
            primary_axes: Default::default(),
            apple: Default::default(),
            windows: Default::default(),
            glyph_cache: Default::default(),
            glide: [0.0; 2],
            emit: outline::Emit::Atlas,
            paths: Default::default(),
            masks: Default::default(),
            face_kinds: Default::default(),
        }
    }

    /// The `trak` table of the font text in `family` at `weight` shapes with first.
    fn primary_trak(&mut self, family: Family, weight: f32, italic: bool) -> Option<Rc<Trak>> {
        let key = (family, weight.to_bits(), italic);
        if let Some(trak) = self.primary_traks.get(&key) {
            return trak.clone();
        }
        let font = self.primary_font(family, weight, italic);
        let trak = font.and_then(|f| font_trak(&mut self.traks, &f.blob, f.index));
        self.primary_traks.insert(key, trak.clone());
        trak
    }

    /// The hidden-axis coordinates (`hidden_axes`) of the font text in `family` at `weight`
    /// shapes with first.
    fn primary_axes(&mut self, family: Family, weight: f32, italic: bool) -> Rc<[parley::FontVariation]> {
        let key = (family, weight.to_bits(), italic);
        if let Some(axes) = self.primary_axes.get(&key) {
            return axes.clone();
        }
        let font = self.primary_font(family, weight, italic);
        let axes: Rc<[parley::FontVariation]> = font
            .and_then(|f| FontRef::from_index(f.blob.as_ref(), f.index as usize).map(|r| hidden_axes(r, weight)))
            .unwrap_or_default()
            .into();
        self.primary_axes.insert(key, axes.clone());
        axes
    }

    /// The ascent and descent per em of the font text in `family` at `weight` shapes with first.
    fn primary_extents(&mut self, family: Family, weight: f32, italic: bool) -> Option<(f32, f32)> {
        let font = self.primary_font(family, weight, italic)?;
        let m = FontRef::from_index(font.blob.as_ref(), font.index as usize)?.metrics(&[]);
        let em = f32::from(m.units_per_em.max(1));
        Some((m.ascent / em, m.descent / em))
    }

    /// The font text in `family` at `weight` shapes with first.
    fn primary_font(&mut self, family: Family, weight: f32, italic: bool) -> Option<parley::fontique::QueryFont> {
        if self.needs_system_mono(family, false) {
            self.system_mono();
        }
        if let Family::Named(list) = family {
            self.load_installed(list);
        }
        let system = self.system_mono.as_deref().unwrap_or_default();
        let named = |list: &str| -> Vec<String> {
            family_list(list, system, &self.sans)
                .into_iter()
                .filter_map(|f| match f { FontFamilyName::Named(name) => Some(name.into_owned()), _ => None })
                .collect()
        };
        let names = match family {
            Family::Sans => named(&self.sans),
            Family::Mono => named(&self.mono),
            Family::Named(list) => named_list(list).into_iter().filter_map(|f| match f { FontFamilyName::Named(name) => Some(name.into_owned()), _ => None }).chain(named(&self.sans)).collect(),
            Family::SystemMono => system.iter().cloned().chain(named(&self.mono)).collect(),
        };
        let mut query = self.font_cx.collection.query(&mut self.font_cx.source_cache);
        query.set_families(names.iter().map(|name| parley::fontique::QueryFamily::Named(name)));
        let style = if italic { FontStyle::Italic } else { FontStyle::Normal };
        query.set_attributes(parley::fontique::Attributes::new(parley::fontique::FontWidth::NORMAL, style, FontWeight::new(weight)));
        query.first_available_font()
    }

    /// Loads the installed families CSS family list `list` names that aren't registered, by
    /// name, so a slot or `Family::Named` can be any font on the system.
    #[doc(hidden)]
    pub fn load_installed(&mut self, list: &str) {
        load_installed(&mut self.font_cx.collection, &mut self.font_files, &mut self.installed, list);
    }

    /// Loads any installed system font families named by `list` (as configured by
    /// [`Self::set_emoji_family`]) that are not yet registered in this collection.
    pub(crate) fn load_installed_emoji(&mut self, list: &str) {
        system_fonts::load_installed_emoji(&mut self.font_cx.collection, &mut self.font_files, &mut self.installed, list);
    }

    /// The installed `Family::SystemMono` families, loading them the first time.
    fn system_mono(&mut self) -> &[String] {
        let (collection, loaded) = (&mut self.font_cx.collection, &mut self.font_files);
        self.system_mono.get_or_insert_with(|| load_system_mono(collection, loaded))
    }

    /// The RGBA image of a colour glyph quad (`GlyphQuad::color`, nonzero).
    pub fn color_glyph(&self, color: u32) -> &rune_image::pixels::Pixels {
        &self.color_pixels[color as usize - 1]
    }

    /// The colours the next glyphs draw in (`spans` by `Ink`). On macOS they choose each
    /// glyph's dilation and on Windows its gamma table, so glyphs in light and dark inks
    /// rasterize separately.
    pub fn set_glyph_inks(&mut self, element: Color, spans: &[Span]) {
        self.apple.set_inks(element, spans);
        self.windows.set_inks(element, spans);
        glyph_emit::set_ink_spans(&mut self.ink_spans, spans);
    }

    /// Whether glyphs rasterise with DirectWrite: `FontSmoothing::DirectWrite` where it exists.
    #[doc(hidden)]
    pub fn direct_write(&mut self) -> bool {
        matches!(self.smoothing, FontSmoothing::DirectWrite | FontSmoothing::ClearType) && self.windows.available()
    }

    /// How glyphs rasterise and place ([`FontSmoothing`]).
    pub fn font_smoothing(&self) -> FontSmoothing { self.requested }

    /// What the atlas quads of a still, unscaled text depend on besides its layout, origin and
    /// inks: the atlas's clock (a page evicted or added moves masks) and how glyphs rasterise.
    /// Quads emitted under one key can be drawn again while it holds (`El::grid` rows).
    #[doc(hidden)]
    pub fn emit_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = rune_motion::Fx::default();
        (self.atlas.clock, self.smoothing, self.apple.pipeline(), self.atlas.lcd).hash(&mut h);
        self.glide.map(f32::to_bits).hash(&mut h);
        h.finish()
    }

    /// Marks the atlas page `quad` samples as drawn this frame, as emitting it did, so a quad
    /// drawn again from a cache keeps its page from eviction.
    #[doc(hidden)]
    pub fn touch_quad(&mut self, page: u16, texels: u32) {
        self.atlas.touch(page, texels);
    }

    /// How far each gliding glyph mask's ink sits below the top of its quad, in device px, keyed
    /// as `blend::apron_key` keys its instance: a DirectWrite or Windfoil mask keeps one box across its y
    /// phases and moves its ink down that box by its phase, so the box moves a whole pixel only
    /// where the phase wraps. The motion gate's raster, which spreads a glyph over its quad,
    /// moves its coverage by this so a glide reads as the quarter-pixel steps it draws.
    #[doc(hidden)]
    pub fn glyph_shifts(&self) -> std::collections::HashMap<[u32; 3], f32> {
        self.atlas.map.iter()
            .filter(|(k, _)| k.subpy != 0 && k.apple & (glyph_emit::DIRECT_WRITE_KEY | glyph_emit::WINDFOIL_KEY) != 0)
            .filter_map(|(k, a)| a.mask().map(|a| ([(a.x as f32).to_bits(), (a.y as f32).to_bits(), (a.page as f32).to_bits()], k.subpy as f32 / glyph_emit::GLIDE_PHASES as f32)))
            .collect()
    }

    /// The exponent a glyph's coverage is raised to when drawn in premultiplied `fill` on this
    /// platform: 1 (coverage as is) on macOS and iOS, and a browser's text gamma for `Off`
    /// elsewhere.
    pub fn coverage_exponent(&self, fill: [f32; 4]) -> f32 {
        let apple = coretext_match::apple_coverage();
        coretext_match::coverage_exponent(self.smoothing, fill, apple, coretext_match::windows_gamma())
    }

    /// Rasterise and place glyphs in `mode` from the next placement on, with its CoreGraphics
    /// stages ([`FontSmoothing::pipeline`]). Glyphs already rasterised in another mode stay
    /// cached under their own keys.
    pub fn set_font_smoothing(&mut self, mode: FontSmoothing) {
        self.requested = mode;
        self.smoothing = mode.drawn(self.lcd_ok());
        self.apple.set_pipeline(self.smoothing.pipeline());
        self.atlas.lcd = self.smoothing.lcd();
        self.atlas.lcd_wanted = mode.lcd();
    }

    /// Whether subpixel text draws here now: the renderer draws outlines and blends by channel,
    /// and for ClearType, DirectWrite is there.
    fn lcd_ok(&mut self) -> bool {
        support::lcd_ok(self.atlas.outlines.support())
            && (self.requested != FontSmoothing::ClearType || self.windows.available())
    }

    /// The CoreGraphics stages glyphs rasterize with.
    pub fn apple_pipeline(&self) -> ApplePipeline { self.apple.pipeline() }
    /// Overrides the stages [`Self::set_font_smoothing`] chose, until the next call to it; for
    /// measuring the stages one at a time.
    pub fn set_apple_pipeline(&mut self, pipeline: ApplePipeline) { self.apple.set_pipeline(pipeline); }

    /// Begins a new batch of glyph placements, reclaiming unused atlas storage.
    /// Previously emitted quads must no longer be sampled after this call. Runtime
    /// calls this automatically; direct text/renderer users call it before shaping.
    pub fn begin_frame(&mut self) {
        let _ = self.begin_frame_pinned(0);
    }

    /// Forgets every text's raster path and cached glyph runs, so the next paint places every
    /// glyph as a text seen for the first time would (`RUNE_RASTER_CHECK`).
    #[doc(hidden)]
    pub fn forget_raster_state(&mut self) {
        self.paths = Default::default();
        self.glyph_cache = Default::default();
    }

    /// [`Self::begin_frame`] keeping the pages in `pinned_mask`. Returns when the window should
    /// wake for the glyph mask cache's first write (`mask_cache`).
    #[doc(hidden)]
    pub fn begin_frame_pinned(&mut self, pinned_mask: u32) -> Option<std::time::Duration> {
        // A subpixel mode draws grayscale until the renderer can blend by channel, and after it
        // says it can't.
        if self.requested.lcd() && self.requested.drawn(self.lcd_ok()) != self.smoothing {
            self.set_font_smoothing(self.requested);
        }
        self.atlas.begin_frame(pinned_mask);
        let wake = self.mask_cache_frame(true);
        self.paths.begin_frame();
        self.glyph_cache.begin_frame();
        self.ink_spans.clear();
        if self.color_bytes > COLOR_GLYPH_BUDGET {
            let before = self.color_glyphs.len();
            self.color_glyphs.retain(|_, c| c.used);
            if self.color_glyphs.len() != before {
                // Only retire scales unused in the last frame. Clearing active emoji
                // would rerasterize the entire visible working set every frame once
                // that working set alone exceeds the soft budget.
                let old = std::mem::take(&mut self.color_pixels);
                self.color_bytes = 0;
                for c in self.color_glyphs.values_mut() {
                    self.color_pixels.push(old[c.index as usize - 1].clone());
                    c.index = self.color_pixels.len() as u32;
                    self.color_bytes += c.w as usize * c.h as usize * 4;
                }
            }
        }
        for c in self.color_glyphs.values_mut() { c.used = false; }
        self.color_icons.begin_frame();
        self.animated_icons.begin_frame();
        wake
    }

    /// Size in points, rounded up to whole device pixels so layout stays on the pixel grid.
    pub fn measure(&mut self, text: &str, style: TextStyle, scale: f32) -> (f32, f32) {
        let l = self.layout_rich(text, &[], style, scale, None);
        (crate::text_width(l.full_width()) / scale, layout_height(&l, style, scale) / scale)
    }

    /// Whether any monochrome animated icon has been classified in this `TextSystem` yet.
    /// Single boolean guard keeping static-only apps free of any animated-icon check in `El::cache`.
    #[doc(hidden)]
    #[inline]
    pub fn has_animated_icons(&self) -> bool {
        self.animated_icons.has_any()
    }

    /// Whether `icon` is a finite-duration animated icon (so callers should track mount-relative
    /// time rather than sampling the global runtime clock).
    #[doc(hidden)]
    #[inline]
    pub fn icon_anim_is_finite(&mut self, icon: &'static Icon) -> bool {
        let icon = icon::swapped(icon);
        self.animated_icons.is_finite(icon as *const Icon as usize)
    }

    /// Whether animated `icon` is still live (moving or waiting for a future segment) at runtime
    /// time `t` (with `mount_t` used for finite-duration icons).
    #[doc(hidden)]
    pub fn icon_anim_live(&mut self, icon: &'static Icon, t: f64, mount_t: f64) -> bool {
        if self.icon_kind(icon) != IconKind::AnimatedMono {
            return false;
        }
        let icon = icon::swapped(icon);
        self.animated_icons.is_live_at(icon as *const Icon as usize, t, mount_t)
    }

    /// Whether both double-buffered atlas tiles for animated `icon` are already resident on
    /// non-evicted pages, so a loop-only step can update it in place without allocating or
    /// evicting a glyph page.
    #[doc(hidden)]
    pub fn icon_anim_slots_ready(
        &self,
        icon: &'static Icon,
        sized: (u32, f32),
        facing: Facing,
        scope_id: u64,
    ) -> bool {
        let icon = icon::swapped(icon);
        self.animated_icons.slots_ready(&self.atlas, icon as *const Icon as usize, sized, facing, scope_id)
    }

    /// Evaluates monochrome animated `icon` into a double-buffered glyph-atlas tile, flipping
    /// between its two slots only when quantized coverage bytes change.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn animated_icon_quad(
        &mut self,
        icon: &'static Icon,
        sized: (u32, f32),
        origin: [f32; 2],
        facing: Facing,
        t: f64,
        mount_t: f64,
        scope_id: u64,
    ) -> Option<AnimatedQuad> {
        let icon = icon::swapped(icon);
        self.animated_icons.frame_quad(&mut self.atlas, icon as *const Icon as usize, sized, origin, facing, t, mount_t, scope_id)
    }

    /// Loop-step variant of [`Self::animated_icon_quad`] that updates only pre-allocated slots in
    /// place via [`atlas::InPlaceAtlas`] and cannot allocate in or evict from the glyph atlas.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn animated_icon_quad_in_place(
        &mut self,
        icon: &'static Icon,
        sized: (u32, f32),
        origin: [f32; 2],
        facing: Facing,
        t: f64,
        mount_t: f64,
        scope_id: u64,
    ) -> Option<AnimatedQuad> {
        let icon = icon::swapped(icon);
        self.animated_icons.frame_quad_in_place(self.atlas.in_place(), icon as *const Icon as usize, sized, origin, facing, t, mount_t, scope_id)
    }

    /// The atlas quad for `icon` at `px` device pixels square, top-left at `origin`.
    pub fn icon_quad(&mut self, icon: &'static Icon, px: u32, origin: [f32; 2]) -> Option<GlyphQuad> {
        self.icon_quad_facing(icon, px, origin, Facing::Upright)
    }

    /// [`Self::icon_quad`], drawn facing `facing` (`El::mirror_in_rtl` mirrors it).
    #[doc(hidden)]
    pub fn icon_quad_facing(&mut self, icon: &'static Icon, px: u32, origin: [f32; 2], facing: Facing) -> Option<GlyphQuad> {
        self.icon_quad_sized(icon, (px, px as f32), origin, facing)
    }

    /// [`Self::icon_quad_facing`] for an icon `size` device pixels across on a `px` canvas, `px`
    /// the size rounded up: the drawing keeps its exact scale where the size isn't whole.
    #[doc(hidden)]
    pub fn icon_quad_sized(&mut self, icon: &'static Icon, (px, size): (u32, f32), origin: [f32; 2], facing: Facing) -> Option<GlyphQuad> {
        let icon = icon::swapped(icon);
        // A mirrored drawing is cached apart, under the size's top bit; what the size falls short
        // of the canvas, in 64ths of a pixel, under the seven bits below it.
        let short = ((px as f32 - size) * 64.0).round().clamp(0.0, 63.0) as u32;
        let key = (icon as *const Icon as usize, px | (short << 24) | (u32::from(facing == Facing::Mirrored) << 31));
        let size = px as f32 - short as f32 / 64.0;
        let entry = match self.atlas.icons.get(&key) {
            Some(&e) => {
                if let Some(a) = e {
                    self.atlas.touch(a.page, a.w * a.h);
                }
                e
            }
            None => match self.cached_icon(icon, px, key).unwrap_or_else(|| rasterize_icon(icon, (px, size), facing, &mut self.atlas)) {
                Rasterized::Placed(g) => {
                    self.atlas.icons.insert(key, Some(g));
                    Some(g)
                }
                Rasterized::Empty => {
                    self.atlas.icons.insert(key, None);
                    None
                }
                Rasterized::Unplaced => None,
            },
        };
        entry.map(|a| GlyphQuad {
            rect: [origin[0], origin[1], px as f32, px as f32],
            uv: [(a.x + 1) as f32, (a.page as u32 * ATLAS_SIZE + a.y + 1) as f32],
            ink: 0,
            byte: 0,
            color: 0,
            outline: None,
            lcd: false,
        })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod core_text_tests;

#[cfg(test)]
mod quality_tests;

#[cfg(test)]
mod rewrap_tests;

#[cfg(test)]
mod tests;
