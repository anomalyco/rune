//! Parley layout (wrapping, rich spans, registered families) over embedded fonts, swash rasterising
//! glyphs and SVG icons into one greyscale atlas.
//!
//! Invariants: layouts are in device pixels; every layout is cached by a 64-bit hash of
//! (text, spans, style, scale, width) and evicted after two commits unused; glyph keys include
//! the run's variation coordinates, so one variable font serves every weight.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, FamilyId, Script, SourceCache};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontWeight, Layout, LayoutContext,
    LineHeight, PositionedLayoutItem, StyleProperty,
};
use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Cap, Fill, Format, Join, Mask, Stroke, Transform, Vector};

use crate::color::{Color, srgb_encode};
use crate::tok::{Family, OpticalSize, TextStyle};

mod svg_path;

const DEFAULT_FONTS: [&[u8]; 3] = [
    include_bytes!("../assets/Inter-Regular.ttf"),
    include_bytes!("../assets/Inter-Medium.ttf"),
    include_bytes!("../assets/Inter-SemiBold.ttf"),
];

/// Scripts the app's own families usually lack: Han, kana, Hangul and Bopomofo.
const CJK_SCRIPTS: [&[u8; 4]; 5] = [b"Hani", b"Hira", b"Kana", b"Hang", b"Bopo"];

// Common-script symbols can inherit the script of adjacent letters. Put these families at
// the end of the requested family list, rather than registering only a Zyyy fallback.
// On macOS the system font comes first: its keyboard symbols (⌘⇧⏎⌫⎋⌥⌃) sit on the baseline at
// cap height and follow the requested weight through its `wght` axis. Apple Symbols draws
// them light, small and (⇧) below the baseline next to app fonts' capitals.
#[cfg(target_os = "macos")]
const SYMBOL_FAMILIES: &[&str] = &["System Font", ".SF NS", "Apple Symbols", ".SF NS Symbols", "Menlo"];
#[cfg(target_os = "macos")]
const SYMBOL_FONT_FILES: &[&str] = &["sfnsttf", "applesymbols", "sfnssymbols", "menlo"];
#[cfg(windows)]
const SYMBOL_FAMILIES: &[&str] = &["Segoe UI Symbol"];
#[cfg(windows)]
const SYMBOL_FONT_FILES: &[&str] = &["seguisym"];
#[cfg(not(any(target_os = "macos", windows)))]
const SYMBOL_FAMILIES: &[&str] = &["DejaVu Sans", "Noto Sans Symbols2", "Noto Sans Symbols"];
#[cfg(not(any(target_os = "macos", windows)))]
const SYMBOL_FONT_FILES: &[&str] = &["dejavusans", "notosanssymbols"];

fn register_symbol_fallbacks(collection: &mut Collection) -> Vec<&'static str> {
    let mut files = system_font_files(SYMBOL_FONT_FILES);
    files.sort();
    collection.load_fonts_from_paths(files);
    SYMBOL_FAMILIES.iter().copied().filter(|name| collection.family_id(name).is_some()).collect()
}

/// File-name fragments (lowercase) of the CJK fonts desktops ship, most preferred first. Only
/// these files are loaded, and only as fallbacks, so Latin text stays on the app's own fonts.
const CJK_FONT_FILES: [&str; 24] = [
    "notosanscjk", "notosanssc", "notosansjp", "notosanstc", "notosanshk", "notosanskr", "sourcehansans",
    "notoserifcjk", "sourcehanserif", "pingfang", "hiragino", "msyh", "yugoth", "meiryo", "malgun", "msjh",
    "wqy", "wenquanyi", "droidsansfallback", "sarasa", "lxgw", "ukai", "uming", "simsun",
];

/// Makes text in scripts the app's fonts don't cover render instead of drawing tofu. Families
/// the app registered beyond its sans and mono (`AppSpec::fonts`) come first, then the CJK fonts
/// installed on the system (found by file name in the platform's font directories, including
/// `$XDG_DATA_HOME/fonts` and `$XDG_DATA_DIRS/*/fonts`).
fn register_cjk_fallbacks(collection: &mut Collection, registered: &[FamilyId], families: (&str, &str)) {
    let primary = [collection.family_id(families.0), collection.family_id(families.1)];
    let mut fallbacks: Vec<FamilyId> = Vec::new();
    for id in registered {
        if !primary.contains(&Some(*id)) && !fallbacks.contains(id) {
            fallbacks.push(*id);
        }
    }
    let mut files = system_font_files(&CJK_FONT_FILES);
    files.sort_by_key(|path| {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        CJK_FONT_FILES.iter().position(|fragment| name.contains(fragment)).unwrap_or(usize::MAX)
    });
    // Memory-mapped, so a 20 MB CJK collection costs a mapping, not a read.
    for path in files.into_iter().take(8) {
        let before: Vec<FamilyId> = collection.family_ids().collect();
        collection.load_fonts_from_paths([&path]);
        let added: Vec<FamilyId> = collection.family_ids().filter(|id| !before.contains(id)).collect();
        for id in added {
            if !primary.contains(&Some(id)) && !fallbacks.contains(&id) {
                fallbacks.push(id);
            }
        }
    }
    if fallbacks.is_empty() {
        return;
    }
    for script in CJK_SCRIPTS {
        collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*script), None), fallbacks.iter().copied());
    }
}

fn system_font_files(fragments: &[&str]) -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(Into::into));
    } else if cfg!(windows) {
        let root = std::env::var_os("WINDIR").map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from);
        dirs.push(root.join("Fonts"));
    } else {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        dirs.extend(data_home.map(|d| d.join("fonts")));
        dirs.extend(home.map(|h| h.join(".fonts")));
        let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        dirs.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| std::path::Path::new(d).join("fonts")));
    }
    let mut found = Vec::new();
    let mut stack = dirs;
    let mut visited = 0;
    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > 4096 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let font = [".ttf", ".otf", ".ttc", ".otc"].iter().any(|ext| name.ends_with(ext));
            let compact = name.replace([' ', '.'], "");
            if font && fragments.iter().any(|fragment| compact.contains(fragment)) && !found.contains(&path) {
                found.push(path);
            }
        }
    }
    found
}

pub const ATLAS_SIZE: u32 = 2048;
const SUBPIXEL_STEPS: f32 = 4.0;

/// Brush carried through Parley: 0 is the element's own colour, n is `spans[n - 1]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ink(pub u16);

/// A styled byte range of a text element (markdown inline code, bold, links).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub weight: Option<f32>,
    pub mono: bool,
    pub size: Option<f32>,
    pub color: Option<Color>,
    /// Rounded background behind the span's runs (inline code), padded by `bg_pad` points.
    pub bg: Option<Color>,
    pub bg_pad: f32,
    pub underline: Option<Color>,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start: start as u32, end: end as u32, ..Default::default() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u32,
    size: u32,
    coords: u64,
    subpx: u8,
    dilation: u32,
}

#[derive(Clone, Copy)]
struct AtlasGlyph {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left: i32,
    top: i32,
}

/// One glyph to draw: device-pixel rect, atlas origin, brush and source byte (for reveals).
#[derive(Clone, Copy, Debug)]
pub struct GlyphQuad {
    pub rect: [f32; 4],
    pub uv: [f32; 2],
    pub ink: u16,
    pub byte: u32,
}

/// A rect drawn under or over text: span backgrounds and underlines (device px).
#[derive(Clone, Copy, Debug)]
pub struct Deco {
    pub rect: [f32; 4],
    pub color: Color,
    pub radius: f32,
}

pub struct Atlas {
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    map: HashMap<GlyphKey, Option<AtlasGlyph>>,
    icons: HashMap<(usize, u32), Option<AtlasGlyph>>,
}

impl Atlas {
    /// Marks every row for upload (a new texture, e.g. after the window's format changed).
    pub fn invalidate(&mut self) {
        self.dirty = Some((0, ATLAS_SIZE));
    }

    fn new() -> Self {
        Atlas {
            data: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            dirty: None,
            shelf_x: 1,
            shelf_y: 1,
            shelf_h: 0,
            map: HashMap::new(),
            icons: HashMap::new(),
        }
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.shelf_x + w + 1 > ATLAS_SIZE {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_x = 1;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + 1 > ATLAS_SIZE {
            return None;
        }
        let pos = (self.shelf_x, self.shelf_y);
        self.shelf_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        Some(pos)
    }

    fn put(&mut self, w: u32, h: u32, src: &[u8], left: i32, top: i32) -> Option<AtlasGlyph> {
        let (ax, ay) = self.alloc(w, h)?;
        for row in 0..h {
            let s = &src[(row * w) as usize..((row + 1) * w) as usize];
            let dst = ((ay + row) * ATLAS_SIZE + ax) as usize;
            self.data[dst..dst + w as usize].copy_from_slice(s);
        }
        let d = self.dirty.get_or_insert((ay, ay + h));
        *d = (d.0.min(ay), d.1.max(ay + h));
        Some(AtlasGlyph { x: ax, y: ay, w, h, left, top })
    }
}

fn rasterize(scaler: &mut swash::scale::Scaler<'_>, atlas: &mut Atlas, glyph: u16, subpx: u8, dilation: f32) -> Option<AtlasGlyph> {
    let image = Render::new(&[Source::Outline])
        .format(Format::Alpha)
        // Swash expands toward +x/+y; offset back to preserve the outline's centre.
        .embolden(dilation)
        .offset(Vector::new(subpx as f32 / SUBPIXEL_STEPS - dilation, -dilation))
        .render(scaler, glyph)?;
    let (w, h) = (image.placement.width, image.placement.height);
    if w == 0 || h == 0 || !image.data.iter().any(|&coverage| coverage != 0) {
        return None;
    }
    atlas.put(w, h, &image.data, image.placement.left, image.placement.top)
}

/// CoreText-calibrated coverage for a linear-light target. Keep colour adjustment out of
/// atlas keys: theme animation must not rasterize five copies or jump between buckets.
/// See research/23-text-quality.md for the held-out glyph and motion measurements.
pub fn glyph_coverage_exponent(color: Color, scale: f32) -> f32 {
    // Stronger bright-ink contrast at Retina density limits phase-dependent blooming.
    0.35 + (1.15 + 0.2 * (scale - 1.0).clamp(0.0, 1.0)) * srgb_encode(color.luminance())
}

// ---------------------------------------------------------------------------------------------
// Icons: a subset of SVG (path, rect, circle, ellipse, line; fill/stroke, caps, joins, evenodd).

/// An SVG icon body as found in `@opencode/ui`'s catalog: elements inside a square viewBox.
#[derive(Debug)]
pub struct Icon {
    pub name: &'static str,
    pub view: f32,
    pub body: &'static str,
}

fn attr<'a>(el: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = el;
    loop {
        let i = rest.find(name)?;
        let before = rest[..i].chars().last();
        let after = &rest[i + name.len()..];
        if before.is_some_and(|c| c.is_whitespace()) && after.starts_with("=\"") {
            let v = &after[2..];
            return Some(&v[..v.find('"')?]);
        }
        rest = &rest[i + name.len()..];
    }
}

fn num(el: &str, name: &str) -> f32 {
    attr(el, name).and_then(|v| v.trim_end_matches("px").parse().ok()).unwrap_or(0.0)
}

/// SVG path data for one icon element, in viewBox units.
fn element_path(el: &str) -> Option<String> {
    let tag = el.trim_start_matches('<').split_whitespace().next()?;
    Some(match tag {
        "path" => attr(el, "d")?.to_owned(),
        "rect" => {
            let (x, y, w, h) = (num(el, "x"), num(el, "y"), num(el, "width"), num(el, "height"));
            let r = num(el, "rx").min(w / 2.0).min(h / 2.0);
            if r <= 0.0 {
                format!("M{x} {y}h{w}v{h}h{}Z", -w)
            } else {
                format!(
                    "M{} {y}h{}a{r} {r} 0 0 1 {r} {r}v{}a{r} {r} 0 0 1 {} {r}h{}a{r} {r} 0 0 1 {} {}v{}a{r} {r} 0 0 1 {r} {}Z",
                    x + r,
                    w - 2.0 * r,
                    h - 2.0 * r,
                    -r,
                    -(w - 2.0 * r),
                    -r,
                    -r,
                    -(h - 2.0 * r),
                    -r
                )
            }
        }
        "circle" | "ellipse" => {
            let (cx, cy) = (num(el, "cx"), num(el, "cy"));
            let (rx, ry) = if tag == "circle" { (num(el, "r"), num(el, "r")) } else { (num(el, "rx"), num(el, "ry")) };
            format!(
                "M{} {cy}a{rx} {ry} 0 1 0 {} 0a{rx} {ry} 0 1 0 {} 0Z",
                cx - rx,
                2.0 * rx,
                -2.0 * rx
            )
        }
        "line" => format!("M{} {}L{} {}", num(el, "x1"), num(el, "y1"), num(el, "x2"), num(el, "y2")),
        _ => return None,
    })
}

fn rasterize_icon(icon: &Icon, px: u32, atlas: &mut Atlas) -> Option<AtlasGlyph> {
    atlas.put(px, px, &icon_coverage(icon, px), 0, 0)
}

/// `icon`'s coverage at `px` × `px` device pixels, row-major, as the glyph atlas stores it.
pub fn icon_coverage(icon: &Icon, px: u32) -> Vec<u8> {
    let s = px as f32 / icon.view;
    let mut acc = vec![0u8; (px * px) as usize];
    let mut buf = vec![0u8; (px * px) as usize];
    for raw in icon.body.split('>') {
        let el = raw.trim();
        if el.is_empty() || el.starts_with("</") {
            continue;
        }
        let Some(d) = element_path(el) else { continue };
        let d = svg_path::parse(&d);
        let stroke = attr(el, "stroke").is_some_and(|v| v != "none");
        let fill = attr(el, "fill").map_or(!stroke, |v| v != "none");
        let mut paint = |style: swash::zeno::Style<'_>| {
            buf.iter_mut().for_each(|b| *b = 0);
            Mask::new(d.as_slice())
                .style(style)
                .transform(Some(Transform::scale(s, s)))
                .size(px, px)
                .render_into(&mut buf, None);
            for (a, &b) in acc.iter_mut().zip(buf.iter()) {
                let (fa, fb) = (*a as u32, b as u32);
                *a = (fa + fb - fa * fb / 255).min(255) as u8;
            }
        };
        if fill {
            let rule = if attr(el, "fill-rule") == Some("evenodd") { Fill::EvenOdd } else { Fill::NonZero };
            paint(rule.into());
        }
        if stroke {
            let w = attr(el, "stroke-width").and_then(|v| v.parse().ok()).unwrap_or(1.0f32);
            // The transform scales the width with the geometry.
            let mut st = Stroke::new(w);
            st.cap(match attr(el, "stroke-linecap") {
                Some("round") => Cap::Round,
                Some("square") => Cap::Square,
                _ => Cap::Butt,
            });
            st.join(match attr(el, "stroke-linejoin") {
                Some("round") => Join::Round,
                Some("bevel") => Join::Bevel,
                _ => Join::Miter,
            });
            paint(st.into());
        }
    }
    acc
}

// ---------------------------------------------------------------------------------------------

pub struct TextSystem {
    font_cx: FontContext,
    layout_cx: LayoutContext<Ink>,
    scale_cx: ScaleContext,
    layouts: HashMap<u64, (Rc<Layout<Ink>>, u32)>,
    generation: u32,
    pub sans: String,
    pub mono: String,
    symbol_families: Vec<&'static str>,
    pub atlas: Atlas,
    /// Layouts built since the last commit (shaping work counter for perf gates).
    pub shaped: u32,
    /// Reused across lines and frames when mapping reveal glyphs back to source bytes.
    cluster_scratch: Vec<(f32, u32)>,
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new(&[], ("Inter", "Inter"))
    }
}

pub fn text_hash(text: &str, spans: &[Span], style: TextStyle, scale: f32, width: Option<f32>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    for s in spans {
        (s.start, s.end, s.mono, s.weight.map(f32::to_bits), s.size.map(f32::to_bits)).hash(&mut h);
    }
    (style.size.to_bits(), style.weight.to_bits(), style.family, style.line_height.to_bits(), style.tracking.to_bits())
        .hash(&mut h);
    (scale.to_bits(), width.map(f32::to_bits)).hash(&mut h);
    let optical = match style.optical_size {
        OpticalSize::Auto => (0, 0), OpticalSize::FontDefault => (1, 0), OpticalSize::Fixed(size) => (2, size.to_bits()),
    };
    (style.variations, style.features, optical).hash(&mut h);
    h.finish()
}

fn variations(style: TextStyle, size: f32) -> Vec<parley::FontVariation> {
    let mut settings: Vec<_> = parley::FontVariation::parse_css_list(style.variations).flatten().collect();
    let opsz = parley::setting::Tag::new(b"opsz");
    let value = match style.optical_size {
        OpticalSize::Auto if !settings.iter().any(|v| v.tag == opsz) => Some(size),
        OpticalSize::Fixed(value) => { settings.retain(|v| v.tag != opsz); Some(value) }
        _ => None,
    };
    if let Some(value) = value.filter(|v| v.is_finite() && *v > 0.0) {
        settings.push(parley::FontVariation::new(opsz, value));
    }
    settings
}

impl TextSystem {
    /// `fonts` replaces the built-in Inter faces when non-empty; `families` names the sans and
    /// mono slots. Other registered families are selectable with `Family::Named`.
    pub fn new(fonts: &[&'static [u8]], families: (&str, &str)) -> Self {
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        let list: &[&'static [u8]] = if fonts.is_empty() { &DEFAULT_FONTS } else { fonts };
        let mut registered = Vec::new();
        for data in list {
            registered.extend(collection.register_fonts(Blob::new(Arc::new(*data)), None).into_iter().map(|(id, _)| id));
        }
        register_cjk_fallbacks(&mut collection, &registered, families);
        let symbol_families = register_symbol_fallbacks(&mut collection);
        TextSystem {
            font_cx: FontContext { collection, source_cache: SourceCache::default() },
            layout_cx: LayoutContext::new(),
            scale_cx: ScaleContext::new(),
            layouts: HashMap::new(),
            generation: 0,
            sans: families.0.to_owned(),
            mono: families.1.to_owned(),
            symbol_families,
            atlas: Atlas::new(),
            shaped: 0,
            cluster_scratch: Vec::new(),
        }
    }

    /// Drops layouts unused for two commits.
    pub fn end_commit(&mut self) {
        let g = self.generation;
        self.layouts.retain(|_, (_, used)| g.wrapping_sub(*used) < 2);
        self.generation = g.wrapping_add(1);
    }

    /// A layout in device pixels, wrapped at `max_width` device px when given.
    pub fn layout_rich(
        &mut self,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
    ) -> Rc<Layout<Ink>> {
        self.layout_aligned(text, spans, style, scale, max_width, Alignment::Start)
    }

    /// [`layout_rich`](Self::layout_rich) with each line aligned within `max_width`, so wrapped
    /// centred or end-aligned text lines up per line rather than as one left-aligned block.
    pub fn layout_aligned(
        &mut self,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
    ) -> Rc<Layout<Ink>> {
        let mut key = text_hash(text, spans, style, scale, max_width);
        if align != Alignment::Start {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (key, align as u8).hash(&mut h);
            key = h.finish();
        }
        let g = self.generation;
        if let Some((l, used)) = self.layouts.get_mut(&key) {
            *used = g;
            return l.clone();
        }
        self.shaped += 1;
        let with_symbols = |primary| {
            FontFamilyName::parse_css_list(primary)
                .flatten()
                .chain(self.symbol_families.iter().map(|name| FontFamilyName::named(name)))
                .collect::<Vec<_>>()
        };
        let sans = match style.family {
            Family::Sans => with_symbols(self.sans.as_str()),
            Family::Mono => with_symbols(self.mono.as_str()),
            Family::Named(name) => {
                let mut families = with_symbols(self.sans.as_str());
                families.insert(0, FontFamilyName::named(name));
                families
            }
        };
        let mono = with_symbols(self.mono.as_str());
        let mut b = self.layout_cx.ranged_builder(&mut self.font_cx, text, scale, true);
        b.push_default(FontFamily::from(sans.as_slice()));
        b.push_default(StyleProperty::FontSize(style.size));
        b.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight)));
        if !style.features.is_empty() {
            b.push_default(StyleProperty::FontFeatures(style.features.into()));
        }
        let settings = variations(style, style.size);
        if !settings.is_empty() {
            b.push_default(parley::FontVariations::List(std::borrow::Cow::Owned(settings)));
        }
        b.push_default(LineHeight::Absolute(style.line_px()));
        if style.tracking != 0.0 {
            b.push_default(StyleProperty::LetterSpacing(style.tracking));
        }
        for (i, s) in spans.iter().enumerate() {
            let r = (s.start as usize).min(text.len())..(s.end as usize).min(text.len());
            if r.is_empty() {
                continue;
            }
            b.push(StyleProperty::Brush(Ink(i as u16 + 1)), r.clone());
            if s.mono {
                b.push(FontFamily::from(mono.as_slice()), r.clone());
            }
            if let Some(w) = s.weight {
                b.push(StyleProperty::FontWeight(FontWeight::new(w)), r.clone());
            }
            if let Some(sz) = s.size {
                b.push(StyleProperty::FontSize(sz), r.clone());
                if style.optical_size == OpticalSize::Auto {
                    b.push(parley::FontVariations::List(std::borrow::Cow::Owned(variations(style, sz))), r.clone());
                }
            }
        }
        let mut layout: Layout<Ink> = b.build(text);
        layout.break_all_lines(max_width);
        layout.align(align, AlignmentOptions::default());
        let l = Rc::new(layout);
        self.layouts.insert(key, (l.clone(), g));
        l
    }

    /// Size in points, rounded up to whole device pixels so layout stays on the pixel grid.
    pub fn measure(&mut self, text: &str, style: TextStyle, scale: f32) -> (f32, f32) {
        let l = self.layout_rich(text, &[], style, scale, None);
        (l.full_width().ceil() / scale, layout_height(&l, style, scale) / scale)
    }

    /// A live run owns its latest layout; don't retain every timer tick in the commit cache.
    pub(crate) fn layout_live(&mut self, text: &str, style: TextStyle, scale: f32) -> Rc<Layout<Ink>> {
        let key = text_hash(text, &[], style, scale, None);
        let cached = self.layouts.contains_key(&key);
        let layout = self.layout_rich(text, &[], style, scale, None);
        if !cached { self.layouts.remove(&key); }
        layout
    }

    /// Glyph quads for single-line `text` with its layout box's top-left at `origin` (device px).
    pub fn glyphs(&mut self, text: &str, style: TextStyle, scale: f32, origin: [f32; 2], out: &mut Vec<GlyphQuad>) {
        let l = self.layout_rich(text, &[], style, scale, None);
        self.glyphs_for_layout(&l, scale, origin, false, out);
    }

    /// Glyph quads for a cached [`Layout`] with its top-left at `origin` (device px). When
    /// `bytes` is set, each quad carries the byte offset of its cluster.
    pub fn glyphs_for_layout(
        &mut self,
        layout: &Layout<Ink>,
        scale: f32,
        origin: [f32; 2],
        bytes: bool,
        out: &mut Vec<GlyphQuad>,
    ) {
        let dilation = 0.1 * scale;
        let table = &mut self.cluster_scratch;
        for line in layout.lines() {
            if bytes {
                table.clear();
                let mut x = line.metrics().offset;
                for run in line.runs() {
                    for cl in run.visual_clusters() {
                        table.push((x, cl.text_range().start as u32));
                        x += cl.advance();
                    }
                }
            }
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
                let run = glyph_run.run();
                let font = run.font();
                let font_size = run.font_size();
                let coords = run.normalized_coords();
                let coords_key = coords.iter().fold(0u64, |h, c| h.wrapping_mul(31).wrapping_add(c.to_bits() as u16 as u64));
                let ink = glyph_run.style().brush.0;
                let baseline = (origin[1] + glyph_run.baseline()).round();
                let mut rel = glyph_run.offset();
                for g in glyph_run.glyphs() {
                    let gx = origin[0] + rel + g.x;
                    let gy = baseline + g.y;
                    let byte = if bytes {
                        let i = table.partition_point(|&(x0, _)| x0 <= rel + 0.01);
                        table.get(i.saturating_sub(1)).map_or(0, |e| e.1)
                    } else {
                        0
                    };
                    rel += g.advance;
                    let fx = gx.floor();
                    let subpx = ((gx - fx) * SUBPIXEL_STEPS).round() as u8;
                    let (fx, subpx) = if subpx as f32 >= SUBPIXEL_STEPS { (fx + 1.0, 0) } else { (fx, subpx) };
                    let key = GlyphKey {
                        font: font.data.id(),
                        index: font.index,
                        glyph: g.id,
                        size: font_size.to_bits(),
                        coords: coords_key,
                        subpx,
                        dilation: dilation.to_bits(),
                    };
                    let entry = match self.atlas.map.get(&key) {
                        Some(&e) => e,
                        None => {
                            let Some(font_ref) = FontRef::from_index(font.data.as_ref(), font.index as usize) else {
                                continue;
                            };
                            let mut scaler = self
                                .scale_cx
                                .builder(font_ref)
                                .size(font_size)
                                .hint(false)
                                .normalized_coords(coords.iter().map(|c| c.to_bits()))
                                .build();
                            let e = rasterize(&mut scaler, &mut self.atlas, g.id as u16, subpx, dilation);
                            self.atlas.map.insert(key, e);
                            e
                        }
                    };
                    if let Some(a) = entry {
                        out.push(GlyphQuad {
                            rect: [fx + a.left as f32, gy.round() - a.top as f32, a.w as f32, a.h as f32],
                            uv: [a.x as f32, a.y as f32],
                            ink,
                            byte,
                        });
                    }
                }
            }
        }
    }

    /// Backgrounds and underlines for `spans` (device px, layout top-left at `origin`). Touching
    /// runs with the same background merge into one rounded rect.
    pub fn decorations(&self, layout: &Layout<Ink>, spans: &[Span], scale: f32, origin: [f32; 2], out: &mut Vec<Deco>) {
        if spans.iter().all(|s| s.bg.is_none() && s.underline.is_none()) {
            return;
        }
        for line in layout.lines() {
            let m = line.metrics();
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
                let ink = gr.style().brush.0;
                if ink == 0 {
                    continue;
                }
                let Some(s) = spans.get(ink as usize - 1) else { continue };
                let x0 = origin[0] + gr.offset();
                let w = gr.advance();
                if let Some(bg) = s.bg {
                    let pad = s.bg_pad * scale;
                    let fm = gr.run().font_metrics();
                    let (asc, desc) = (fm.ascent, fm.descent);
                    let base = origin[1] + m.baseline;
                    let top = (base - asc - 1.0 * scale).round();
                    let bot = (base + desc.abs() + 1.0 * scale).round();
                    let rect = [x0 - pad, top, w + 2.0 * pad, bot - top];
                    // Adjacent runs with the same background (a span split by style) share one rect.
                    match out.last_mut() {
                        Some(d) if d.color == bg && d.rect[1] == top && (d.rect[0] + d.rect[2] - rect[0]).abs() < 1.0 => {
                            d.rect[2] = rect[0] + rect[2] - d.rect[0];
                        }
                        _ => out.push(Deco { rect, color: bg, radius: 4.0 * scale }),
                    }
                }
                if let Some(uc) = s.underline {
                    let y = (origin[1] + m.baseline + 2.0 * scale).round();
                    out.push(Deco { rect: [x0, y, w, scale.max(1.0)], color: uc, radius: 0.0 });
                }
            }
        }
    }

    /// The atlas quad for `icon` at `px` device pixels square, top-left at `origin`.
    pub fn icon_quad(&mut self, icon: &'static Icon, px: u32, origin: [f32; 2]) -> Option<GlyphQuad> {
        let key = (icon as *const Icon as usize, px);
        let entry = match self.atlas.icons.get(&key) {
            Some(&e) => e,
            None => {
                let e = rasterize_icon(icon, px, &mut self.atlas);
                self.atlas.icons.insert(key, e);
                e
            }
        };
        entry.map(|a| GlyphQuad {
            rect: [origin[0].round(), origin[1].round(), a.w as f32, a.h as f32],
            uv: [a.x as f32, a.y as f32],
            ink: 0,
            byte: 0,
        })
    }
}

/// Layout height in device px: whole lines at the style's line height (an empty text still
/// takes one line, like a browser).
pub fn layout_height(l: &Layout<Ink>, style: TextStyle, scale: f32) -> f32 {
    l.height().max(style.line_px() * scale).ceil()
}

/// Fraction of the final line occupied by ink advance, excluding hanging whitespace.
/// Width and the layout use device pixels. No shaping or allocation occurs here.
pub fn last_line_fill(l: &Layout<Ink>, width: f32) -> f32 {
    let Some(line) = l.lines().next_back() else { return 0.0 };
    let m = line.metrics();
    ((m.advance - m.hanging_advance) / width.max(1.0)).clamp(0.0, 1.0)
}

/// Height held back while the last line fills (research/18 §7), in points.
pub fn stream_height_lag(l: &Layout<Ink>, width: f32, scale: f32) -> f32 {
    let Some(line) = l.lines().next_back() else { return 0.0 };
    let cap = if l.len() == 1 { 0.52 } else { 0.82 };
    let lag = line.metrics().line_height / scale * cap * (1.0 - last_line_fill(l, width));
    (lag * 64.0).round() / 64.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph_ids(text: &str) -> Vec<u32> {
        let mut ts = TextSystem::default();
        let layout = ts.layout_rich(text, &[], TextStyle::new(13.0, 400.0), 2.0, None);
        let mut ids = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(run) = item {
                    ids.extend(run.glyphs().map(|g| g.id));
                }
            }
        }
        ids
    }

    /// Han text shapes with real glyphs (not .notdef) whenever a CJK font is installed; Latin
    /// text keeps the app's own family.
    #[test]
    fn cjk_falls_back_to_an_installed_font() {
        if system_font_files(&CJK_FONT_FILES).is_empty() {
            eprintln!("skipping: no CJK font installed (set XDG_DATA_DIRS to a dir with fonts/)");
            return;
        }
        let han = glyph_ids("你好世界");
        assert_eq!(han.len(), 4);
        assert!(han.iter().all(|id| *id != 0), "tofu: {han:?}");
        assert_eq!(glyph_ids("Hello"), glyph_ids("Hello"));
    }

    #[test]
    fn plex_keyboard_symbols_use_real_glyphs() {
        const PLEX: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
        let mut ts = TextSystem::new(&[PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
        if ts.symbol_families.is_empty() {
            eprintln!("skipping: no symbol font installed");
            return;
        }
        let plex = FontRef::from_index(PLEX, 0).unwrap();
        assert_eq!(plex.charmap().map('⌘'), 0, "fixture must need fallback");
        // Exercise standalone Common script, surrounding Latin and a mono rich-text span.
        for text in ["⌘⇧⏎⌥⌃⎋⌫←↑→↓↵", "Copy ⌘K ⇧⏎ ⌥⌃⎋⌫←↑→↓↵"] {
            for style in [TextStyle::new(13.0, 400.0), TextStyle::new(13.0, 400.0).mono()] {
                let spans = [Span { mono: true, ..Span::new(0, text.len()) }];
                for spans in [&[][..], &spans[..]] {
                    let layout = ts.layout_rich(text, spans, style, 2.0, None);
                    let mut count = 0;
                    for line in layout.lines() {
                        for item in line.items() {
                            if let PositionedLayoutItem::GlyphRun(run) = item {
                                for glyph in run.glyphs() {
                                    assert_ne!(glyph.id, 0, "missing symbol in {text:?}");
                                    count += 1;
                                }
                            }
                        }
                    }
                    assert_eq!(count, text.chars().count());
                }
            }
        }
        let layout = ts.layout_rich("Hello", &[], TextStyle::new(13.0, 400.0), 2.0, None);
        let ids: Vec<_> = layout
            .lines()
            .flat_map(|line| line.items())
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(run) => Some(run.glyphs().map(|glyph| glyph.id).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(ids, "Hello".chars().map(|ch| u32::from(plex.charmap().map(ch))).collect::<Vec<_>>());
    }

    /// Each glyph's ink top and bottom in px above the baseline, with its run's weight axis.
    fn ink_extents(ts: &mut TextSystem, text: &str, style: TextStyle) -> Vec<(f32, f32)> {
        let layout = ts.layout_rich(text, &[], style, 2.0, None);
        let mut out = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
                let run = glyph_run.run();
                let font = run.font();
                let font_ref = FontRef::from_index(font.data.as_ref(), font.index as usize).unwrap();
                let mut scaler = ts
                    .scale_cx
                    .builder(font_ref)
                    .size(run.font_size())
                    .normalized_coords(run.normalized_coords().iter().map(|c| c.to_bits()))
                    .build();
                for g in glyph_run.glyphs() {
                    let b = scaler.scale_outline(g.id as u16).unwrap().bounds();
                    out.push((b.max.y, b.min.y));
                }
            }
        }
        out
    }

    /// Keyboard symbols a primary font lacks fall back to glyphs that stand on the baseline at
    /// about cap height, so `⇧` and `⏎` don't shrink and sink next to the font's capitals.
    #[test]
    fn keyboard_symbol_fallbacks_match_cap_height() {
        const PLEX: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
        let mut ts = TextSystem::new(&[PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
        if !cfg!(target_os = "macos") || system_font_files(&["sfnsttf"]).is_empty() {
            eprintln!("skipping: no macOS system font");
            return;
        }
        for size in [13.0, 15.0] {
            let glyphs = ink_extents(&mut ts, "H⌘⇧⏎⌫⎋⌥", TextStyle::new(size, 400.0).mono());
            let (cap, _) = glyphs[0];
            for (ch, (top, bottom)) in "H⌘⇧⏎⌫⎋⌥".chars().zip(&glyphs).skip(1) {
                assert!((top / cap - 1.0).abs() < 0.12, "{ch} at {size}pt: top {top:.1} vs cap {cap:.1}");
                assert!(bottom.abs() < cap * 0.06, "{ch} at {size}pt: bottom {bottom:.1} off the baseline");
            }
        }
    }
}

#[cfg(test)]
mod quality_tests {
    use super::*;

    #[test]
    fn centred_wrapped_lines_centre_each_line_in_the_wrap_width() {
        let mut text = TextSystem::default();
        let style = TextStyle::new(13.0, 400.0);
        let content = "a much longer first line of words that wraps\nshort";
        let width = 400.0;
        let start = text.layout_rich(content, &[], style, 2.0, Some(width));
        let centred = text.layout_aligned(content, &[], style, 2.0, Some(width), Alignment::Center);
        assert!(!Rc::ptr_eq(&start, &centred), "alignment is part of the cache key");
        let offsets = |l: &Layout<Ink>| {
            l.lines().map(|line| (line.metrics().offset, line.metrics().advance - line.metrics().hanging_advance)).collect::<Vec<_>>()
        };
        assert!(offsets(&start).iter().all(|(offset, _)| *offset == 0.0));
        let lines = offsets(&centred);
        assert!(lines.len() >= 2);
        for (offset, advance) in lines {
            assert!((offset - (width - advance) / 2.0).abs() < 1.0, "offset {offset} advance {advance}");
        }
    }

    #[test]
    fn live_layouts_do_not_accumulate_between_commits() {
        let mut text = TextSystem::default();
        let style = TextStyle::new(13.0, 400.0);
        let stable = text.layout_rich("stable label", &[], style, 2.0, None);
        for tick in 0..256 { text.layout_live(&tick.to_string(), style, 2.0); }
        assert_eq!(text.layouts.len(), 1);
        assert!(Rc::ptr_eq(&stable, &text.layout_rich("stable label", &[], style, 2.0, None)));
    }

    #[test]
    fn optical_size_default_keeps_static_font_glyphs_byte_identical() {
        let mut a = TextSystem::default(); let mut b = TextSystem::default();
        for scale in [1.0, 2.0] {
            for size in [11.0, 13.0, 24.0] {
                let style = TextStyle::new(size, 400.0);
                let mut qa = Vec::new(); let mut qb = Vec::new();
                a.glyphs("Hello ffi 0123456789", style, scale, [0.0, 0.0], &mut qa);
                b.glyphs("Hello ffi 0123456789", style.optical_sizing(OpticalSize::FontDefault), scale, [0.0, 0.0], &mut qb);
                assert_eq!(qa.iter().map(|q| (q.rect, q.uv)).collect::<Vec<_>>(), qb.iter().map(|q| (q.rect, q.uv)).collect::<Vec<_>>());
                assert_eq!(a.atlas.data, b.atlas.data);
            }
        }
    }

    #[test]
    fn moving_glyphs_reuse_four_phases_and_land_without_atlas_work() {
        let mut text = TextSystem::default();
        let layout = text.layout_rich("lHEx ", &[], TextStyle::new(13.0, 400.0), 2.0, None);
        let mut quads = Vec::new();
        // Negative origins exercise phase carry as well as both sides of zero.
        for step in 0..256 {
            quads.clear();
            text.glyphs_for_layout(&layout, 2.0, [-2.0 + step as f32 / 64.0, 12.37], false, &mut quads);
            assert_eq!(quads.len(), 4, "dilation must not turn whitespace into empty draw instances");
            assert!(quads.iter().all(|g| g.rect[0].fract() == 0.0 && g.rect[1].fract() == 0.0));
        }
        let entries = text.atlas.map.len();
        assert!(entries <= 5 * 4, "four glyphs plus cached whitespace have at most four x phases");
        let mut track = rune_motion::Animated::new(-2.0f32).with_class(rune_motion::PropClass::Spatial);
        track.animate_to(8.0, rune_motion::Spring::new(0.3, 0.0), 0.0);
        let end = track.settle_time();
        assert!(end.is_finite());
        text.atlas.dirty = None;
        quads.clear();
        text.glyphs_for_layout(&layout, 2.0, [8.0, 12.37], false, &mut quads);
        let landed: Vec<_> = quads.iter().map(|q| (q.rect, q.uv)).collect();
        for frame in 0..120 {
            let x = track.sample(end + frame as f64 / 120.0);
            assert_eq!(x, 8.0, "the analytic tail lands on its target exactly");
            quads.clear();
            text.glyphs_for_layout(&layout, 2.0, [x, 12.37], false, &mut quads);
            assert!(quads.iter().map(|q| (q.rect, q.uv)).eq(landed.iter().copied()));
        }
        assert_eq!(text.atlas.map.len(), entries);
        assert!(text.atlas.dirty.is_none());
    }

    #[test]
    fn nominal_size_dilation_is_part_of_the_cache_key() {
        let mut text = TextSystem::default();
        let small = text.layout_rich("H", &[], TextStyle::new(12.0, 400.0), 2.0, None);
        let large = text.layout_rich("H", &[], TextStyle::new(24.0, 400.0), 1.0, None);
        let mut a = Vec::new(); let mut b = Vec::new();
        text.glyphs_for_layout(&small, 2.0, [0.0, 0.0], false, &mut a);
        text.glyphs_for_layout(&large, 1.0, [0.0, 0.0], false, &mut b);
        assert_ne!(a[0].uv, b[0].uv, "same pixel size, different nominal-size dilation");
    }
}
