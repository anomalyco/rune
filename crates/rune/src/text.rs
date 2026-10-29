//! Parley layout (wrapping, rich spans, registered families) over embedded fonts, swash rasterising
//! glyphs and SVG icons into one greyscale atlas.
//!
//! Invariants: layouts are in device pixels; every layout is cached by a 64-bit hash of
//! (text, spans, style, scale, width), kept for two commits unused and then while a budget of
//! stale text lasts (`STALE_TEXT`), least recently used out first; a width not cached re-breaks
//! a cached layout of the same text and style rather than shaping it again; glyph keys include
//! the run's variation coordinates, so one variable font serves every weight.

use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, FamilyId, Script, SourceCache};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight, Layout, LayoutContext,
    LineHeight, PositionedLayoutItem, StyleProperty,
};
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};

use crate::color::Color;
use crate::tok::{Family, OpticalSize, TextStyle};

mod case;
mod default_fonts;
#[cfg(all(test, target_os = "macos"))]
mod core_text_tests;
pub(crate) mod coretext_match;
pub use coretext_match::FontSmoothing;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod pixel_compare;
mod fallback;
mod glyph_cache;
mod glyph_emit;
mod hash;
mod hidden_axes;
mod icon;
mod layout_cache;
mod macos_smoothing;
pub use macos_smoothing::ApplePipeline;
mod measure;
mod pill_chips;
mod rich_layout;
mod run;
pub(crate) use pill_chips::pill_chips;
mod svg_path;
mod trak;
mod underline;
mod line_metrics;
pub use line_metrics::{last_line_fill, layout_height, stream_height_lag};
pub(crate) use underline::underline_edges;
pub use case::TextCase;
pub use hash::text_hash;
pub(crate) use hash::shape_key;
pub use measure::{TextInk, advance, ink, measure};
pub use rich_layout::RichLayoutOptions;
pub use run::{Run, RunLayout};
pub(crate) use case::Source as CaseSource;

use default_fonts::DEFAULT_FONTS;
use fallback::Route;
use glyph_emit::Face;
use hidden_axes::hidden_axes;
use layout_cache::{Cache, evict};
#[cfg(test)]
use layout_cache::{STALE_ENTRY, STALE_TEXT};
pub use icon::{Icon, icon_coverage};
use icon::rasterize_icon;
use trak::{Trak, Traks, font_trak};

/// Scripts the app's own families usually lack: Han, kana, Hangul and Bopomofo.
const CJK_SCRIPTS: [&[u8; 4]; 5] = [b"Hani", b"Hira", b"Kana", b"Hang", b"Bopo"];

// Common-script symbols can inherit the script of adjacent letters. Put these families at
// the end of the requested family list, rather than registering only a Zyyy fallback.
// On macOS the system font comes first: its keyboard symbols (⌘⇧⏎⌫⎋⌥⌃) sit on the baseline at
// cap height and follow the requested weight through its `wght` axis. Apple Symbols draws
// them light, small and (⇧) below the baseline next to app fonts' capitals.
// The colour emoji family comes last, so symbols with a text form keep it.
#[cfg(target_os = "macos")]
const SYMBOL_FAMILIES: &[&str] = &["System Font", ".SF NS", "Apple Symbols", ".SF NS Symbols", "Menlo", "Apple Color Emoji"];
#[cfg(target_os = "macos")]
// SF italic rides along so `Family::Named("System Font").italic()` gets the real face.
const SYMBOL_FONT_FILES: &[&str] = &["sfnsttf", "sfnsitalic", "applesymbols", "sfnssymbols", "menlo", "applecoloremoji"];
#[cfg(windows)]
const SYMBOL_FAMILIES: &[&str] = &["Segoe UI Symbol", "Segoe UI Emoji"];
#[cfg(windows)]
const SYMBOL_FONT_FILES: &[&str] = &["seguisym", "seguiemj"];
#[cfg(not(any(target_os = "macos", windows)))]
const SYMBOL_FAMILIES: &[&str] = &["DejaVu Sans", "Noto Sans Symbols2", "Noto Sans Symbols", "Noto Color Emoji"];
#[cfg(not(any(target_os = "macos", windows)))]
const SYMBOL_FONT_FILES: &[&str] = &["dejavusans", "notosanssymbols", "notocoloremoji"];

// `Family::SystemMono`: the platform's monospaced UI families, most preferred first, and the
// files they're in. SF Mono's family name is `.SF NS Mono`, as SF's is `.SF NS`.
#[cfg(target_os = "macos")]
const SYSTEM_MONO_FAMILIES: &[&str] = &[".SF NS Mono", "Menlo"];
#[cfg(target_os = "macos")]
const SYSTEM_MONO_FILES: &[&str] = &["sfnsmono", "menlo"];
#[cfg(windows)]
const SYSTEM_MONO_FAMILIES: &[&str] = &["Cascadia Mono", "Consolas", "Courier New"];
#[cfg(windows)]
const SYSTEM_MONO_FILES: &[&str] = &["cascadiamono", "consola", "cour"];
// After fontconfig's own answer (`fontconfig_monospace`).
#[cfg(not(any(target_os = "macos", windows)))]
const SYSTEM_MONO_FAMILIES: &[&str] = &["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono", "Ubuntu Mono"];
#[cfg(not(any(target_os = "macos", windows)))]
const SYSTEM_MONO_FILES: &[&str] = &["dejavusansmono", "liberationmono", "notosansmono", "ubuntumono"];

/// The family and file fontconfig resolves `monospace` to, asked once per process.
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
fn fontconfig_monospace() -> Option<(String, std::path::PathBuf)> {
    static ANSWER: std::sync::OnceLock<Option<(String, std::path::PathBuf)>> = std::sync::OnceLock::new();
    ANSWER.get_or_init(|| {
        let out = std::process::Command::new("fc-match").args(["--format=%{family[0]}\n%{file}", "monospace"]).output().ok()?;
        let out = String::from_utf8(out.stdout).ok()?;
        let (family, file) = out.split_once('\n')?;
        (!family.is_empty() && !file.is_empty()).then(|| (family.to_owned(), file.trim().into()))
    }).clone()
}

/// The installed `Family::SystemMono` families, most preferred first, loading their files.
fn load_system_mono(collection: &mut Collection, loaded: &mut LoadedFiles) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
    if let Some((family, file)) = fontconfig_monospace() {
        load_files(collection, loaded, [file]);
        names.push(family);
    }
    load_files(collection, loaded, system_font_files(SYSTEM_MONO_FILES).into_iter().take(12));
    for name in SYSTEM_MONO_FAMILIES {
        if !names.iter().any(|n| n == name) { names.push((*name).to_owned()); }
    }
    names.retain(|name| collection.family_id(name).is_some());
    names
}

/// Whether the CSS family list `list` names `ui-monospace`, which stands for `Family::SystemMono`'s
/// families, so the mono slot can be SF Mono, whose family name is hidden.
fn names_system_mono(list: &str) -> bool {
    FontFamilyName::parse_css_list(list).flatten().any(|f| f == FontFamilyName::Generic(parley::GenericFamily::UiMonospace))
}

/// `list` parsed, with `ui-monospace` replaced by the installed `Family::SystemMono` families.
/// When that leaves no named family (no monospace font installed, or the web), `sans` follows,
/// so text still finds a font.
fn family_list<'a>(list: &'a str, system_mono: &'a [String], sans: &'a str) -> Vec<FontFamilyName<'a>> {
    let mut system = false;
    let mut out: Vec<FontFamilyName<'a>> = FontFamilyName::parse_css_list(list)
        .flatten()
        .flat_map(|f| {
            let ui_mono = f == FontFamilyName::Generic(parley::GenericFamily::UiMonospace);
            system |= ui_mono;
            let names = if ui_mono { system_mono } else { &[] };
            names.iter().map(|name| FontFamilyName::named(name)).chain((!ui_mono).then_some(f)).collect::<Vec<_>>()
        })
        .collect();
    if system && !out.iter().any(|f| matches!(f, FontFamilyName::Named(_))) {
        out.extend(FontFamilyName::parse_css_list(sans).flatten());
    }
    out
}

/// The installed families among `names`, loading the system font files matching `files`.
fn load_families(collection: &mut Collection, files: &[&str], names: &[&'static str]) -> Vec<&'static str> {
    collection.load_fonts_from_paths(system_font_files(files));
    names.iter().copied().filter(|name| collection.family_id(name).is_some()).collect()
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
fn register_cjk_fallbacks(collection: &mut Collection, loaded: &mut LoadedFiles, registered: &[FamilyId], families: (&str, &str)) {
    let primary = [collection.family_id(families.0), collection.family_id(families.1)];
    let mut fallbacks: Vec<FamilyId> = Vec::new();
    for id in registered {
        if !primary.contains(&Some(*id)) && !fallbacks.contains(id) {
            fallbacks.push(*id);
        }
    }
    // Memory-mapped, so a 20 MB CJK collection costs a mapping, not a read.
    for id in load_files(collection, loaded, system_font_files(&CJK_FONT_FILES).into_iter().take(8)) {
        if !primary.contains(&Some(id)) && !fallbacks.contains(&id) {
            fallbacks.push(id);
        }
    }
    if fallbacks.is_empty() {
        return;
    }
    for script in CJK_SCRIPTS {
        collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*script), None), fallbacks.iter().copied());
    }
}

/// The families each system font file added to the collection, so a file is loaded once.
type LoadedFiles = HashMap<std::path::PathBuf, Vec<FamilyId>>;

/// The families of `paths`, in order, loading each file the first time.
fn load_files(collection: &mut Collection, loaded: &mut LoadedFiles, paths: impl IntoIterator<Item = std::path::PathBuf>) -> Vec<FamilyId> {
    let mut families: Vec<FamilyId> = Vec::new();
    for path in paths {
        let ids = loaded.entry(path).or_insert_with_key(|path| {
            let before: Vec<FamilyId> = collection.family_ids().collect();
            collection.load_fonts_from_paths([path]);
            let mut added: Vec<FamilyId> = collection.family_ids().filter(|id| !before.contains(id)).collect();
            // `family_ids` iterates a hash map; `FamilyId`s increase in the collection file's face order.
            added.sort_unstable();
            added
        });
        for id in ids.iter() {
            if !families.contains(id) {
                families.push(*id);
            }
        }
    }
    families
}

/// Appends the system fonts for `fonts.script` as that script's fallbacks.
fn register_script_fallbacks(collection: &mut Collection, loaded: &mut LoadedFiles, fonts: &fallback::ScriptFonts) {
    let added = load_files(collection, loaded, system_font_files(fonts.files).into_iter().take(4));
    if !added.is_empty() {
        collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*fonts.script), None), added.into_iter());
    }
}

/// Font files whose lowercase name, without spaces and dots, contains one of `fragments`,
/// ordered by the first fragment they match, then by path. The platform's font directories are
/// walked once per process.
#[cfg(target_arch = "wasm32")]
fn system_font_files(_: &[&str]) -> Vec<std::path::PathBuf> { Vec::new() }

#[cfg(not(target_arch = "wasm32"))]
fn system_font_files(fragments: &[&str]) -> Vec<std::path::PathBuf> {
    static FILES: std::sync::OnceLock<Vec<(String, std::path::PathBuf)>> = std::sync::OnceLock::new();
    let mut files: Vec<_> = FILES
        .get_or_init(walk_font_dirs)
        .iter()
        .filter_map(|(name, path)| Some((fragments.iter().position(|fragment| name.contains(fragment))?, path)))
        .collect();
    files.sort();
    files.into_iter().map(|(_, path)| path.clone()).collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn walk_font_dirs() -> Vec<(String, std::path::PathBuf)> {
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
    let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut stack = dirs;
    let mut visited = std::collections::HashSet::new();
    while let Some(dir) = stack.pop() {
        if visited.len() > 4096 || !visited.insert(dir.clone()) {
            continue;
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
            if font {
                found.push((name.replace([' ', '.'], ""), path));
            }
        }
    }
    found
}

pub const ATLAS_SIZE: u32 = 2048;

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
    /// The family's italic face, or a synthetic oblique when it has none.
    pub italic: bool,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start: start as u32, end: end as u32, ..Default::default() }
    }

    pub fn weight(mut self, weight: f32) -> Span {
        self.weight = Some(weight);
        self
    }

    /// Weight 600.
    pub fn bold(mut self) -> Span {
        self.weight = Some(600.0);
        self
    }

    pub fn italic(mut self) -> Span {
        self.italic = true;
        self
    }

    pub fn mono(mut self) -> Span {
        self.mono = true;
        self
    }

    pub fn size(mut self, size: f32) -> Span {
        self.size = Some(size);
        self
    }

    pub fn color(mut self, color: Color) -> Span {
        self.color = Some(color);
        self
    }

    /// A rounded `color` background padded by `pad` points, as inline code draws.
    pub fn bg(mut self, color: Color, pad: f32) -> Span {
        self.bg = Some(color);
        self.bg_pad = pad;
        self
    }

    pub fn underline(mut self, color: Color) -> Span {
        self.underline = Some(color);
        self
    }
}

/// An atomic pill drawn as a chip: byte range `start..end` of the content, and its leading icon.
#[derive(Clone, Debug)]
pub(crate) struct PillChip {
    pub start: u32,
    pub end: u32,
    pub icon: Option<&'static Icon>,
    pub bg: Option<Color>,
    pub fg: Option<Color>,
}

/// A pill chip's horizontal geometry in points for text of `size` points: the room before its
/// label (outer margin, padding, then any icon and its gap) and after it (padding, margin).
pub(crate) struct PillMetrics {
    pub margin: f32,
    pub pad: f32,
    pub icon: f32,
    pub icon_gap: f32,
}

impl PillMetrics {
    pub fn new(size: f32) -> Self {
        Self { margin: (size * 0.12).max(1.5), pad: (size * 0.36).max(3.0), icon: (size * 0.95).round(), icon_gap: (size * 0.22).max(2.0) }
    }

    /// Room before the label, inside the chip and out.
    pub fn lead(&self, icon: bool) -> f32 {
        self.margin + self.pad + if icon { self.icon + self.icon_gap } else { 0.0 }
    }

    pub fn trail(&self) -> f32 {
        self.pad + self.margin
    }
}

/// Tags the inline boxes around pill labels: the rest of the id is the pill's index shifted
/// left once, with the low bit set on the box after the label.
const PILL_BOX: u64 = 1 << 62;

/// Flattens overlapping [`Span`]s into non-overlapping elementary intervals so that Parley's
/// single `Brush(Ink)` index per glyph run carries the combined colour, background, underline,
/// weight, italic, and monospace attributes of all overlapping spans.
pub(crate) fn flatten_spans(spans: &[Span]) -> Vec<Span> {
    if spans.len() <= 1
        || spans
            .windows(2)
            .all(|w| w[0].start < w[0].end && w[0].end <= w[1].start)
    {
        return spans.iter().filter(|s| s.start < s.end).cloned().collect();
    }
    let mut bounds = Vec::with_capacity(spans.len() * 2);
    let mut order: Vec<usize> = Vec::with_capacity(spans.len());
    for (i, s) in spans.iter().enumerate() {
        if s.start < s.end {
            bounds.push(s.start);
            bounds.push(s.end);
            order.push(i);
        }
    }
    bounds.sort_unstable();
    bounds.dedup();
    order.sort_by_key(|&i| spans[i].start);
    // Sweep the elementary intervals, keeping the spans that cover the current one in input
    // order so later spans still win, without rescanning every span per interval.
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut out: Vec<Span> = Vec::with_capacity(bounds.len());
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        active.retain(|&i| spans[i].end > a);
        while next < order.len() && spans[order[next]].start <= a {
            let i = order[next];
            let at = active.partition_point(|&j| j < i);
            active.insert(at, i);
            next += 1;
        }
        let mut merged: Option<Span> = None;
        for &i in &active {
            let s = &spans[i];
            let m = merged.get_or_insert_with(|| Span::new(a as usize, b as usize));
            if s.weight.is_some() {
                m.weight = s.weight;
            }
            m.mono |= s.mono;
            if s.size.is_some() {
                m.size = s.size;
            }
            if s.color.is_some() {
                m.color = s.color;
            }
            if s.bg.is_some() {
                m.bg = s.bg;
                m.bg_pad = s.bg_pad;
            }
            if s.underline.is_some() {
                m.underline = s.underline;
            }
            m.italic |= s.italic;
        }
        if let Some(m) = merged {
            if let Some(last) = out.last_mut()
                && last.end == m.start
                && last.weight == m.weight
                && last.mono == m.mono
                && last.size == m.size
                && last.color == m.color
                && last.bg == m.bg
                && last.bg_pad == m.bg_pad
                && last.underline == m.underline
                && last.italic == m.italic
            {
                last.end = m.end;
            } else {
                out.push(m);
            }
        }
    }
    out
}

/// A pressable byte range of a text element: a link, an @mention, a file chip. Attach with
/// [`El::link`](crate::El::link) and handle with [`El::on_link`](crate::El::on_link). Its resting
/// look is whatever [`Span`]s cover it; [`LinkPaint`]s add the hover, pressed and focus looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) id: String,
    pub(crate) role: &'static str,
    pub(crate) label: Option<String>,
    pub(crate) hover: LinkPaint,
    pub(crate) pressed: Option<LinkPaint>,
    pub(crate) focused: Option<LinkPaint>,
}

impl Link {
    /// Bytes `range` of the text, reported to `on_link` as `id`. Its role is "link".
    pub fn new(range: std::ops::Range<usize>, id: impl Into<String>) -> Link {
        Link {
            start: range.start as u32,
            end: range.end as u32,
            id: id.into(),
            role: "link",
            label: None,
            hover: LinkPaint::default(),
            pressed: None,
            focused: None,
        }
    }
    /// Accessibility role, such as "button" for a file chip.
    pub fn role(mut self, role: &'static str) -> Self {
        self.role = role;
        self
    }
    /// Accessible name; the range's text by default.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// The look while the pointer is over it.
    pub fn hover(mut self, paint: LinkPaint) -> Self {
        self.hover = paint;
        self
    }
    /// The look while it is held down; the hover look by default.
    pub fn pressed(mut self, paint: LinkPaint) -> Self {
        self.pressed = Some(paint);
        self
    }
    /// The look while it has keyboard focus; the hover look by default, or an underline in
    /// its text colour when that is empty.
    pub fn focused(mut self, paint: LinkPaint) -> Self {
        self.focused = Some(paint);
        self
    }
}

/// A link's look in one state, painted over its resting span style without reshaping. Unset
/// parts keep the span's own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LinkPaint {
    pub(crate) color: Option<Color>,
    pub(crate) bg: Option<Color>,
    pub(crate) ring: Option<(Color, f32)>,
    pub(crate) underline: Option<Color>,
}

impl LinkPaint {
    pub fn new() -> LinkPaint {
        LinkPaint::default()
    }
    /// Glyph colour.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// A rounded background behind each line of the range.
    pub fn bg(mut self, c: Color) -> Self {
        self.bg = Some(c);
        self
    }
    /// An inset `c` border ring `width` points wide inside the rounded background on each line.
    pub fn ring(mut self, c: Color, width: f32) -> Self {
        self.ring = (width > 0.0 && width.is_finite()).then_some((c, width));
        self
    }
    pub fn underline(mut self, c: Color) -> Self {
        self.underline = Some(c);
        self
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.color.is_none() && self.bg.is_none() && self.ring.is_none() && self.underline.is_none()
    }
}

/// A paint-only highlight over `start..end` UTF-8 bytes of a [`text`](crate::text) element.
///
/// Drawn from the shaped layout's glyph boxes without splitting runs, so adding, moving or
/// clearing highlights never reshapes text or shifts line breaks.
#[derive(Clone, Debug, PartialEq)]
pub struct TextHighlight {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) bg: Color,
    pub(crate) fg: Option<Color>,
    pub(crate) ring: Option<(Color, f32)>,
    pub(crate) pad: f32,
    pub(crate) key: Option<String>,
}

impl TextHighlight {
    /// Bytes `range` of the text, painted with a rounded `bg` fill behind each line's glyphs.
    pub fn new(range: std::ops::Range<usize>, bg: Color) -> TextHighlight {
        TextHighlight {
            start: range.start.min(u32::MAX as usize) as u32,
            end: range.end.min(u32::MAX as usize) as u32,
            bg,
            fg: None,
            ring: None,
            pad: 1.5,
            key: None,
        }
    }
    /// Glyph colour over the highlighted range.
    pub fn color(mut self, c: Color) -> Self {
        self.fg = Some(c);
        self
    }
    /// Inset `c` border ring `width` points wide inside the rounded background on each line.
    pub fn ring(mut self, c: Color, width: f32) -> Self {
        self.ring = (width > 0.0 && width.is_finite()).then_some((c, width));
        self
    }
    /// Horizontal padding around each line's background, in points (`1.5` by default).
    pub fn pad(mut self, pad: f32) -> Self {
        self.pad = pad;
        self
    }
    /// Optional key for [`Tx::scroll_into_view`](crate::Tx::scroll_into_view),
    /// [`Cx::layout`](crate::Cx::layout) and [`Runtime::find`](crate::Runtime::find).
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
    /// The UTF-8 byte range of this highlight.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
    /// Returns this highlight with its UTF-8 byte range replaced by `range`.
    pub fn with_range(mut self, range: std::ops::Range<usize>) -> Self {
        self.start = range.start.min(u32::MAX as usize) as u32;
        self.end = range.end.min(u32::MAX as usize) as u32;
        self
    }
}

/// Looked up for every glyph of every frame, so hashed with Fx rather than SipHash.
type GlyphMap<V> = HashMap<GlyphKey, V, std::hash::BuildHasherDefault<rune_motion::Fx>>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u32,
    size: u32,
    coords: u64,
    /// The x phase in sixtieths of a pixel (`coretext_match::PHASES`).
    subpx: u8,
    /// The dilation radius in device px, as f32 bits.
    dilation: u32,
    /// The CoreGraphics pipeline's stages (`macos_smoothing::Smoothing::key`).
    apple: u8,
    /// Synthetic oblique, in degrees (0 for none).
    skew: u8,
}

#[derive(Clone, Copy)]
struct AtlasGlyph {
    page: u16,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left: i32,
    top: i32,
}

/// Steady-state shelf pages (page 0 + one secondary page for ping-pong eviction).
const MAX_STEADY_PAGES: usize = 2;
/// Hard ceiling on total glyph atlas pages if a single frame or unframed loop overflows.
const MAX_OVERFLOW_PAGES: usize = 8;
/// CPU byte budget for cached RGBA colour glyphs (`color_pixels`) before frame-boundary eviction.
const COLOR_GLYPH_BUDGET: usize = 16 * 1024 * 1024;

pub struct AtlasPage {
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    shown: bool,
    touched: bool,
    full: bool,
    age: u64,
    live_texels: u32,
}

impl AtlasPage {
    fn new(age: u64) -> Self {
        AtlasPage {
            data: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            dirty: None,
            shelf_x: 1,
            shelf_y: 1,
            shelf_h: 0,
            shown: false,
            touched: false,
            full: false,
            age,
            live_texels: 0,
        }
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.shelf_x + w + 1 > ATLAS_SIZE {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_x = 1;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + 1 > ATLAS_SIZE {
            self.full = true;
            return None;
        }
        let pos = (self.shelf_x, self.shelf_y);
        self.shelf_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        Some(pos)
    }
}

enum Rasterized {
    Empty,
    Placed(AtlasGlyph),
    Unplaced,
}

/// One glyph to draw: device-pixel rect, atlas origin, brush and source byte (for reveals).
#[derive(Clone, Copy, Debug)]
pub struct GlyphQuad {
    pub rect: [f32; 4],
    /// Atlas origin with the page packed into y (`page * ATLAS_SIZE + y`).
    /// Use [`Self::atlas_page`] and [`Self::atlas_uv`] when sampling an atlas page directly.
    pub uv: [f32; 2],
    pub ink: u16,
    pub byte: u32,
    /// A colour glyph (emoji) drawn from [`TextSystem::color_glyph`] instead of the alpha
    /// atlas: 0 for none, else its index plus one.
    pub color: u32,
}

impl GlyphQuad {
    /// Alpha atlas page. Placements remain valid until the next frame begins.
    pub fn atlas_page(&self) -> usize { self.uv[1] as usize / ATLAS_SIZE as usize }

    /// Pixel origin within [`Self::atlas_page`], without the packed page index.
    pub fn atlas_uv(&self) -> [f32; 2] {
        [self.uv[0], self.uv[1] - (self.atlas_page() * ATLAS_SIZE as usize) as f32]
    }
}

/// A colour glyph's RGBA image (`GlyphQuad::color`) and its placement around the pen (device px).
#[derive(Clone, Copy)]
struct ColorGlyph {
    index: u32,
    used: bool,
    left: i32,
    top: i32,
    w: u32,
    h: u32,
}

impl ColorGlyph {
    fn quad(&self, x: f32, y: f32, ink: u16, byte: u32) -> GlyphQuad {
        let rect = [x.round() + self.left as f32, y.round() - self.top as f32, self.w as f32, self.h as f32];
        GlyphQuad { rect, uv: [0.0, 0.0], ink, byte, color: self.index }
    }
}

/// A rect drawn under or over text: span backgrounds and underlines (device px).
#[derive(Clone, Copy, Debug)]
pub struct Deco {
    pub rect: [f32; 4],
    pub color: Color,
    pub radius: f32,
}

/// Internal paint metadata: keep the public `Deco` literal unchanged.
pub(crate) struct PaintedDeco {
    pub deco: Deco,
    pub ring: Option<(Color, f32)>,
    pub byte: u32,
}

impl std::ops::Deref for PaintedDeco {
    type Target = Deco;
    fn deref(&self) -> &Deco { &self.deco }
}
impl std::ops::DerefMut for PaintedDeco {
    fn deref_mut(&mut self) -> &mut Deco { &mut self.deco }
}

trait Decoration {
    fn new(deco: Deco, ring: Option<(Color, f32)>, byte: u32) -> Self;
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>);
    fn extend_to(&mut self, _byte: u32) {}
}
impl Decoration for Deco {
    fn new(deco: Deco, _: Option<(Color, f32)>, _: u32) -> Self { deco }
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>) { (self, None) }
}
impl Decoration for PaintedDeco {
    fn new(deco: Deco, ring: Option<(Color, f32)>, byte: u32) -> Self { Self { deco, ring, byte } }
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>) { (&mut self.deco, self.ring) }
    fn extend_to(&mut self, byte: u32) { self.byte = self.byte.max(byte); }
}

pub struct Atlas {
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    /// `(node_id, signature)` of `El::cache` targets currently resident in the GPU renderer.
    pub(crate) resident_caches: HashMap<u64, u64>,
    /// Secondary and overflow shelf pages (`page >= 1` lives at `pages[page - 1]`).
    pub(crate) pages: Vec<AtlasPage>,
    pub(crate) resets: u64,
    pub(crate) rasterized: u64,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    shown: bool,
    touched: bool,
    full: bool,
    age: u64,
    live_texels: u32,
    clock: u64,
    map: GlyphMap<Option<AtlasGlyph>>,
    icons: HashMap<(usize, u32), Option<AtlasGlyph>>,
}

impl Atlas {
    /// Marks every row for upload (a new texture, e.g. after the window's format changed).
    pub fn invalidate(&mut self) {
        self.dirty = Some((0, ATLAS_SIZE));
        for page in &mut self.pages {
            page.dirty = Some((0, ATLAS_SIZE));
        }
        self.resident_caches.clear();
    }

    /// Number of allocated 2048×2048 shelf pages (at least 1).
    pub fn page_count(&self) -> usize {
        1 + self.pages.len()
    }

    /// Alpha texels for a page returned by [`GlyphQuad::atlas_page`].
    pub fn page_data(&self, page: usize) -> Option<&[u8]> {
        if page == 0 { Some(&self.data) } else { self.pages.get(page - 1).map(|p| p.data.as_slice()) }
    }

    /// Number of shelf page resets since the atlas was created.
    pub fn resets(&self) -> u64 {
        self.resets
    }

    /// Number of glyphs and icons placed into the atlas since creation.
    pub fn rasterized(&self) -> u64 {
        self.rasterized
    }

    /// Rasterized glyph entries, for tests that bound the atlas.
    #[cfg(test)]
    pub(crate) fn glyph_entries(&self) -> usize { self.map.len() }

    fn new() -> Self {
        Atlas {
            data: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            dirty: None,
            resident_caches: HashMap::new(),
            pages: Vec::new(),
            resets: 0,
            rasterized: 0,
            shelf_x: 1,
            shelf_y: 1,
            shelf_h: 0,
            shown: false,
            touched: false,
            full: false,
            age: 1,
            live_texels: 0,
            clock: 1,
            map: GlyphMap::default(),
            icons: HashMap::new(),
        }
    }

    fn alloc_page0(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.shelf_x + w + 1 > ATLAS_SIZE {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_x = 1;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + 1 > ATLAS_SIZE {
            self.full = true;
            return None;
        }
        let pos = (self.shelf_x, self.shelf_y);
        self.shelf_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        Some(pos)
    }

    fn touch(&mut self, page: u16, texels: u32) {
        if page == 0 {
            self.touched = true;
            self.live_texels = self.live_texels.saturating_add(texels);
        } else if let Some(p) = self.pages.get_mut(page as usize - 1) {
            p.touched = true;
            p.live_texels = p.live_texels.saturating_add(texels);
        }
    }

    fn page_age(&self, page: usize) -> u64 {
        if page == 0 { self.age } else { self.pages[page - 1].age }
    }

    fn page_full(&self, page: usize) -> bool {
        if page == 0 { self.full } else { self.pages[page - 1].full }
    }

    fn page_touched(&self, page: usize) -> bool {
        if page == 0 { self.touched } else { self.pages[page - 1].touched }
    }

    fn page_shown(&self, page: usize) -> bool {
        if page == 0 { self.shown } else { self.pages[page - 1].shown }
    }

    fn page_non_empty(&self, page: usize) -> bool {
        if page == 0 {
            self.shelf_x > 1 || self.shelf_y > 1 || self.shelf_h > 0
        } else {
            let p = &self.pages[page - 1];
            p.shelf_x > 1 || p.shelf_y > 1 || p.shelf_h > 0
        }
    }

    fn evict_page(&mut self, page: usize) {
        let p16 = page as u16;
        self.map.retain(|_, v| v.is_none_or(|g| g.page != p16));
        self.icons.retain(|_, v| v.is_none_or(|g| g.page != p16));
        self.clock += 1;
        self.resets += 1;
        if page == 0 {
            self.shelf_x = 1;
            self.shelf_y = 1;
            self.shelf_h = 0;
            self.dirty = None;
            self.shown = false;
            self.touched = false;
            self.full = false;
            self.age = self.clock;
            self.live_texels = 0;
        } else if let Some(p) = self.pages.get_mut(page - 1) {
            p.shelf_x = 1;
            p.shelf_y = 1;
            p.shelf_h = 0;
            p.dirty = None;
            p.shown = false;
            p.touched = false;
            p.full = false;
            p.age = self.clock;
            p.live_texels = 0;
        }
    }

    pub(crate) fn begin_frame(&mut self, pinned_mask: u32) {
        let total_live = self.live_texels as u64
            + self.pages.iter().map(|p| p.live_texels as u64).sum::<u64>();
        let half_page = (ATLAS_SIZE * ATLAS_SIZE / 2) as u64;
        let needed = ((total_live / half_page) as usize + MAX_STEADY_PAGES)
            .clamp(MAX_STEADY_PAGES, MAX_OVERFLOW_PAGES);
        let total = self.page_count();
        let full_count = (0..total).filter(|&p| self.page_full(p)).count();
        if full_count >= needed || total > needed {
            let keep = needed.min(total);
            if let Some(oldest) = (0..keep)
                .filter(|&p| self.page_full(p) && (pinned_mask & (1u32 << p.min(31))) == 0)
                .min_by_key(|&p| self.page_age(p))
            {
                self.evict_page(oldest);
            }
            for p in keep..total {
                if (pinned_mask & (1u32 << p.min(31))) == 0 && self.page_non_empty(p) {
                    self.evict_page(p);
                }
            }
            while self.page_count() > needed {
                let last = self.page_count() - 1;
                if (pinned_mask & (1u32 << last.min(31))) != 0 || self.page_non_empty(last) {
                    break;
                }
                self.pages.pop();
            }
        }
        self.shown = std::mem::take(&mut self.touched);
        self.live_texels = 0;
        for p in &mut self.pages {
            p.shown = std::mem::take(&mut p.touched);
            p.live_texels = 0;
        }
        for p in 0..self.page_count() {
            if (pinned_mask & (1u32 << p.min(31))) != 0 {
                self.touch(p as u16, 0);
            }
        }
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(usize, u32, u32)> {
        if w == 0 || h == 0 || w + 2 > ATLAS_SIZE || h + 2 > ATLAS_SIZE {
            return None;
        }
        if !self.full && let Some((x, y)) = self.alloc_page0(w, h) {
            return Some((0, x, y));
        }
        for (i, page) in self.pages.iter_mut().enumerate() {
            if !page.full && let Some((x, y)) = page.alloc(w, h) {
                return Some((i + 1, x, y));
            }
        }
        // Try shelf tails on full pages for small glyphs before opening or resetting a page.
        if let Some((x, y)) = self.alloc_page0(w, h) {
            return Some((0, x, y));
        }
        for (i, page) in self.pages.iter_mut().enumerate() {
            if let Some((x, y)) = page.alloc(w, h) {
                return Some((i + 1, x, y));
            }
        }
        let total = self.page_count();
        // Recycle an idle page (untouched in both the last frame and this frame so far).
        if let Some(p) = (0..total)
            .filter(|&p| !self.page_touched(p) && !self.page_shown(p) && self.page_non_empty(p))
            .min_by_key(|&p| self.page_age(p))
        {
            self.evict_page(p);
            let (x, y) = if p == 0 {
                self.alloc_page0(w, h).unwrap()
            } else {
                self.pages[p - 1].alloc(w, h).unwrap()
            };
            return Some((p, x, y));
        }
        // Once we have the steady-state page pair, recycle the oldest page not touched this frame.
        if total >= MAX_STEADY_PAGES
            && let Some(p) = (0..total)
                .filter(|&p| !self.page_touched(p) && self.page_non_empty(p))
                .min_by_key(|&p| self.page_age(p))
        {
            self.evict_page(p);
            let (x, y) = if p == 0 {
                self.alloc_page0(w, h).unwrap()
            } else {
                self.pages[p - 1].alloc(w, h).unwrap()
            };
            return Some((p, x, y));
        }
        if total < MAX_OVERFLOW_PAGES {
            self.clock += 1;
            let mut page = AtlasPage::new(self.clock);
            let (x, y) = page.alloc(w, h).unwrap();
            self.pages.push(page);
            return Some((self.pages.len(), x, y));
        }
        // Every page is referenced by this frame (or pinned by a text flight). Never
        // overwrite emitted quads to satisfy a later request. Failure is not cached,
        // so the request can succeed after a subsequent frame frees capacity.
        None
    }

    fn put(&mut self, w: u32, h: u32, src: &[u8], left: i32, top: i32) -> Option<AtlasGlyph> {
        let (page, ax, ay) = self.alloc(w, h)?;
        let (data, dirty) = if page == 0 {
            (&mut self.data, &mut self.dirty)
        } else {
            let p = &mut self.pages[page - 1];
            (&mut p.data, &mut p.dirty)
        };
        // Zero the 1px border around `[ax..ax+w, ay..ay+h]` so bilinear filtering never samples
        // stale texels from a previous generation without zeroing the whole 4 MB page on reset.
        let x0 = (ax - 1) as usize;
        let x1 = (ax + w + 1) as usize;
        let top_row = (ay - 1) as usize * ATLAS_SIZE as usize;
        data[top_row + x0..top_row + x1].fill(0);
        for row in 0..h {
            let s = &src[(row * w) as usize..((row + 1) * w) as usize];
            let line = (ay + row) as usize * ATLAS_SIZE as usize;
            data[line + x0] = 0;
            data[line + ax as usize..line + (ax + w) as usize].copy_from_slice(s);
            data[line + (ax + w) as usize] = 0;
        }
        let bot_row = (ay + h) as usize * ATLAS_SIZE as usize;
        data[bot_row + x0..bot_row + x1].fill(0);
        let (y0, y1) = (ay - 1, ay + h + 1);
        let d = dirty.get_or_insert((y0, y1));
        *d = (d.0.min(y0), d.1.max(y1));
        self.rasterized += 1;
        self.touch(page as u16, w * h);
        Some(AtlasGlyph { page: page as u16, x: ax, y: ay, w, h, left, top })
    }
}

/// Lean of a synthetic oblique, in degrees, for faces with no italic.
const SYNTHETIC_OBLIQUE: u8 = 12;

/// The colour image of `glyph` (COLR layers or an embedded bitmap scaled to size), or None
/// when the font draws it as an outline.
fn rasterize_color(scaler: &mut swash::scale::Scaler<'_>, glyph: u16) -> Option<(crate::pixels::Pixels, swash::zeno::Placement)> {
    if !scaler.has_color_bitmaps() && !scaler.has_color_outlines() {
        return None;
    }
    let image = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit)]).render(scaler, glyph)?;
    let (w, h) = (image.placement.width, image.placement.height);
    if image.content != Content::Color || w == 0 || h == 0 || image.data.len() != (w * h * 4) as usize {
        return None;
    }
    let pixels = crate::pixels::Pixels::new();
    pixels.set(w, h, image.data);
    Some((pixels, image.placement))
}

// ---------------------------------------------------------------------------------------------

pub struct TextSystem {
    font_cx: FontContext,
    layout_cx: LayoutContext<Ink>,
    scale_cx: ScaleContext,
    layouts: Cache<Layout<Ink>>,
    /// The `layouts` key of the latest layout of each shaping (see `layout_blocks_inner`), whose
    /// glyphs a new width re-breaks instead of shaping the text again.
    shapes: HashMap<u64, u64>,
    runs: Cache<RunLayout>,
    generation: u32,
    pub sans: String,
    pub mono: String,
    symbol_families: Vec<&'static str>,
    /// The colour emoji family among `symbol_families`.
    emoji: Option<&'static str>,
    /// The installed math families, once a text has needed them.
    math: Option<Vec<&'static str>>,
    /// The installed `Family::SystemMono` families, once a text has needed them.
    system_mono: Option<Vec<String>>,
    /// The sans or mono slot names `ui-monospace` (see `family_list`).
    slot_system_mono: bool,
    /// Bit `i` is set once `fallback::SCRIPT_FONTS[i]` has been looked up.
    scripts: u64,
    font_files: LoadedFiles,
    pub atlas: Atlas,
    color_glyphs: GlyphMap<ColorGlyph>,
    color_pixels: Vec<crate::pixels::Pixels>,
    color_bytes: usize,
    /// Layouts shaped, a running count (shaping work counter for perf gates).
    pub shaped: u32,
    /// Layouts re-broken at a new width from an earlier shaping, a running count.
    pub rebroken: u32,
    /// Reused across lines and frames when mapping reveal glyphs back to source bytes.
    cluster_scratch: Vec<(f32, u32)>,
    coords_scratch: Vec<i16>,
    traks: Traks,
    pub(crate) smoothing: FontSmoothing,
    /// The `trak` table of the first font of a family slot at a weight, upright or italic.
    primary_traks: HashMap<(Family, u32, bool), Option<Rc<Trak>>>,
    /// The hidden-axis coordinates (`hidden_axes`) of the same first font.
    primary_axes: HashMap<(Family, u32, bool), Rc<[parley::FontVariation]>>,
    /// CoreGraphics' glyph pipeline for `smoothing`, and its per-ink state.
    apple: macos_smoothing::Smoothing,
    pub(crate) glyph_cache: glyph_cache::GlyphCache,
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new(&[], ("Inter", "Inter"))
    }
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

/// The byte of each tab in monospace text (mono spans, or all of it in a mono style) with the
/// columns it fills to reach the next 4-column stop. Columns count from the start of the line
/// or of the monospace run, whichever is later.
fn mono_tab_stops(text: &str, spans: &[Span], mono_style: bool) -> Vec<(usize, u32)> {
    let mut regions: Vec<(usize, usize)> = if mono_style {
        vec![(0, text.len())]
    } else {
        let mut r: Vec<_> = spans.iter().filter(|s| s.mono && s.start < s.end).map(|s| (s.start as usize, (s.end as usize).min(text.len()))).collect();
        r.sort_unstable();
        r
    };
    regions.dedup_by(|next, prev| {
        let joined = next.0 <= prev.1;
        if joined {
            prev.1 = prev.1.max(next.1);
        }
        joined
    });
    let mut out = Vec::new();
    for (start, end) in regions {
        let Some(run) = text.get(start..end) else { continue };
        let mut column = 0u32;
        for (i, ch) in run.char_indices() {
            match ch {
                '\n' => column = 0,
                '\t' => {
                    let fill = 4 - column % 4;
                    out.push((start + i, fill));
                    column += fill;
                }
                _ => column += 1,
            }
        }
    }
    out
}

impl TextSystem {
    #[cfg(test)]
    pub(crate) fn cached_layout_count(&self) -> usize { self.layouts.len() }

    /// `fonts` replaces the built-in Inter faces when non-empty on native. Web keeps its
    /// bundled sans and mono fallbacks, since there are no system fonts to discover.
    /// `families` names the sans and mono slots; others are selectable with `Family::Named`.
    pub fn new(fonts: &[&'static [u8]], families: (&str, &str)) -> Self {
        measure::note_fonts(fonts, families);
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        let list: &[&'static [u8]] = if fonts.is_empty() { DEFAULT_FONTS } else { fonts };
        let mut registered = Vec::new();
        #[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
        if !fonts.is_empty() {
            for data in DEFAULT_FONTS {
                collection.register_fonts(Blob::new(Arc::new(*data)), None);
            }
        }
        for data in list {
            let mut added: Vec<FamilyId> = collection.register_fonts(Blob::new(Arc::new(*data)), None).into_iter().map(|(id, _)| id).collect();
            added.sort_unstable();
            registered.extend(added);
        }
        let mut font_files = LoadedFiles::new();
        register_cjk_fallbacks(&mut collection, &mut font_files, &registered, families);
        let symbol_families = load_families(&mut collection, SYMBOL_FONT_FILES, SYMBOL_FAMILIES);
        let emoji = symbol_families.iter().copied().find(|f| f.contains("Emoji"));
        #[cfg(target_arch = "wasm32")]
        let families = if fonts.is_empty() { ("Inter", "IBM Plex Mono") } else { families };
        TextSystem {
            font_cx: FontContext { collection, source_cache: SourceCache::default() },
            layout_cx: LayoutContext::new(),
            scale_cx: ScaleContext::new(),
            layouts: HashMap::new(),
            shapes: HashMap::new(),
            runs: HashMap::new(),
            generation: 0,
            sans: if cfg!(target_arch = "wasm32") { format!("{}, Inter", families.0) } else { families.0.to_owned() },
            mono: if cfg!(target_arch = "wasm32") { format!("{}, IBM Plex Mono", families.1) } else { families.1.to_owned() },
            symbol_families,
            emoji,
            math: None,
            system_mono: None,
            slot_system_mono: names_system_mono(families.0) || names_system_mono(families.1),
            scripts: 0,
            font_files,
            atlas: Atlas::new(),
            color_glyphs: GlyphMap::default(),
            color_pixels: Vec::new(),
            color_bytes: 0,
            shaped: 0,
            rebroken: 0,
            cluster_scratch: Vec::new(),
            coords_scratch: Vec::new(),
            traks: Traks::new(),
            smoothing: FontSmoothing::default(),
            primary_traks: HashMap::new(),
            primary_axes: HashMap::new(),
            apple: Default::default(),
            glyph_cache: Default::default(),
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

    /// The font text in `family` at `weight` shapes with first.
    fn primary_font(&mut self, family: Family, weight: f32, italic: bool) -> Option<parley::fontique::QueryFont> {
        if family == Family::SystemMono || self.slot_system_mono {
            self.system_mono();
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
            Family::Named(name) => std::iter::once(name.to_owned()).chain(named(&self.sans)).collect(),
            Family::SystemMono => system.iter().cloned().chain(named(&self.mono)).collect(),
        };
        let mut query = self.font_cx.collection.query(&mut self.font_cx.source_cache);
        query.set_families(names.iter().map(|name| parley::fontique::QueryFamily::Named(name)));
        let style = if italic { FontStyle::Italic } else { FontStyle::Normal };
        query.set_attributes(parley::fontique::Attributes::new(parley::fontique::FontWidth::NORMAL, style, FontWeight::new(weight)));
        query.first_available_font()
    }

    /// The installed `Family::SystemMono` families, loading them the first time.
    fn system_mono(&mut self) -> &[String] {
        let (collection, loaded) = (&mut self.font_cx.collection, &mut self.font_files);
        self.system_mono.get_or_insert_with(|| load_system_mono(collection, loaded))
    }

    /// The RGBA image of a colour glyph quad (`GlyphQuad::color`, nonzero).
    pub fn color_glyph(&self, color: u32) -> &crate::pixels::Pixels {
        &self.color_pixels[color as usize - 1]
    }

    /// The colours the next glyphs draw in (`spans` by `Ink`). On macOS they choose each
    /// glyph's dilation, so glyphs in light and dark inks rasterize separately.
    pub fn set_glyph_inks(&mut self, element: Color, spans: &[Span]) { self.apple.set_inks(element, spans); }

    /// How glyphs rasterise and place ([`FontSmoothing`]).
    pub fn font_smoothing(&self) -> FontSmoothing { self.smoothing }

    /// Rasterise and place glyphs in `mode` from the next placement on, with its CoreGraphics
    /// stages ([`FontSmoothing::pipeline`]). Glyphs already rasterised in another mode stay
    /// cached under their own keys.
    pub fn set_font_smoothing(&mut self, mode: FontSmoothing) {
        self.smoothing = mode;
        self.apple.set_pipeline(mode.pipeline());
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
        self.begin_frame_pinned(0);
    }

    pub(crate) fn begin_frame_pinned(&mut self, pinned_mask: u32) {
        self.atlas.begin_frame(pinned_mask);
        self.glyph_cache.begin_frame();
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
    }

    /// Keeps layouts used in the last two commits, and older ones, most recently used first,
    /// while they fit in [`layout_cache::STALE_TEXT`].
    pub fn end_commit(&mut self) {
        let g = self.generation;
        evict(&mut self.layouts, g);
        let layouts = &self.layouts;
        self.shapes.retain(|_, key| layouts.contains_key(key));
        evict(&mut self.runs, g);
        self.generation = g.wrapping_add(1);
    }

    /// `shape` is [`shape_key`] of the same text when the caller already has it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_keyed(
        &mut self,
        shape: Option<u64>,
        text: &str,
        spans: &[Span],
        blocks: &[crate::edit::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        wrap_anywhere: bool,
        cache: bool,
    ) -> Rc<Layout<Ink>> {
        let shared = (!cache).then(|| std::mem::replace(&mut self.layout_cx, LayoutContext::new()));
        let has_indented_blocks = blocks.iter().any(|&b| b != crate::edit::Block::Paragraph);
        // Each pill's label range, and the room before and after it in device px; boxes keep
        // the pill's index in their id, so ranges that fail the checks keep no chip.
        let pm = PillMetrics::new(style.size);
        let raw_pills = pills;
        let pills: Vec<(usize, std::ops::Range<usize>, u32, u32)> = pills
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let r = (p.start as usize).min(text.len())..(p.end as usize).min(text.len());
                (i, r, (pm.lead(p.icon.is_some()) * scale).round() as u32, (pm.trail() * scale).round() as u32)
            })
            .filter(|(_, r, ..)| {
                !r.is_empty() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end) && !text.as_bytes()[r.clone()].contains(&b'\n')
            })
            .collect();
        // Shaping depends on the text, spans, style, scale, overflow-wrap and pills; line
        // breaking adds the width, alignment and block indents. A layout cached at one width
        // is re-broken at another rather than shaped again.
        let shape_key = shape.unwrap_or_else(|| shape_key(text, spans, style, scale, wrap_anywhere, raw_pills));
        let key = hash::break_key(shape_key, max_width, align, has_indented_blocks.then_some(blocks));
        let g = self.generation;
        let break_lines = |layout: &mut Layout<Ink>| {
            if has_indented_blocks || !pills.is_empty() {
                break_lines_with_blocks(layout, blocks, style.size, scale, max_width);
            } else {
                layout.break_all_lines(max_width);
            }
        };
        if cache {
            if let Some((l, used, _)) = self.layouts.get_mut(&key) {
                *used = g;
                self.shapes.insert(shape_key, key);
                return l.clone();
            }
            if let Some(shaped) = self.shapes.get(&shape_key).and_then(|k| self.layouts.get(k)) {
                self.rebroken += 1;
                let _break_time = crate::frame_profile::Scope::new(crate::frame_profile::Phase::Text);
                let mut layout = Layout::clone(&shaped.0);
                break_lines(&mut layout);
                layout.align(align, AlignmentOptions::default());
                let l = Rc::new(layout);
                self.layouts.insert(key, (l.clone(), g, text.len() as u32));
                self.shapes.insert(shape_key, key);
                return l;
            }
        }
        self.shaped += 1;
        let _shape_time = crate::frame_profile::Scope::new(crate::frame_profile::Phase::Text);
        let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
        let tabs = if text.contains('\t') { mono_tab_stops(text, spans, mono_style) } else { Vec::new() };
        let widen = if tabs.is_empty() { Vec::new() } else { self.mono_tab_widening(&tabs, spans, style, scale) };
        // Parley has no tab stops and fonts draw a tab as a missing glyph (or nothing): shape it
        // as a space, one byte like the tab, so every byte range still holds. Monospace tabs
        // then widen to the next 4-column stop through word spacing.
        let spaced;
        let text = if text.contains('\t') {
            spaced = zeroize::Zeroizing::new(text.replace('\t', " "));
            spaced.as_str()
        } else {
            text
        };
        let routes = fallback::routes(text);
        if !text.is_ascii() {
            let (mut scripts, mut symbols) = (0u64, false);
            for ch in text.chars() {
                for (i, fonts) in fallback::SCRIPT_FONTS.iter().enumerate() {
                    if (self.scripts | scripts) & (1 << i) == 0 && fonts.covers(ch) {
                        scripts |= 1 << i;
                    }
                }
                symbols |= self.math.is_none() && fallback::needs_symbols(ch);
            }
            for (i, fonts) in fallback::SCRIPT_FONTS.iter().enumerate() {
                if scripts & (1 << i) != 0 {
                    self.scripts |= 1 << i;
                    register_script_fallbacks(&mut self.font_cx.collection, &mut self.font_files, fonts);
                }
            }
            if symbols {
                self.math = Some(load_families(&mut self.font_cx.collection, fallback::MATH_FONT_FILES, fallback::MATH_FAMILIES));
            }
        }
        // `trak` tracking at the point size (see `trak.rs`), on top of `style.tracking`. An
        // explicit optical size `o` picks the entry instead, as Core Text tracks `o`-point text
        // scaled to the set size; spans scale the lookup with their size. Each style range gets
        // its predicted first font's spacing; runs in another font are checked after shaping.
        let trak_tag = parley::setting::Tag::new(b"trak");
        let mut features: Vec<parley::FontFeature> = parley::FontFeature::parse_css_list(style.features).flatten().collect();
        let tracked = features.iter().rev().find(|f| f.tag == trak_tag).is_none_or(|f| f.value != 0);
        let listed = features.len();
        features.retain(|f| f.tag != trak_tag);
        let stripped = features.len() != listed;
        let optical = match style.optical_size {
            OpticalSize::Fixed(o) if o.is_finite() && o > 0.0 && style.size > 0.0 => Some(o),
            _ => None,
        };
        let spacing = |trak: Option<Rc<Trak>>, size: f32| {
            let px = size * scale;
            let points = px / scale;
            // `o` itself at the style's size (a run's size comes back through device px).
            let points = optical.map_or(points, |o| {
                if (points - style.size).abs() <= style.size * 1e-6 { o } else { o * points / style.size }
            });
            style.tracking + trak.map_or(0.0, |t| t.spacing(px, points, tracked) / scale)
        };
        let base_spacing = spacing(self.primary_trak(style.family, style.weight, style.italic), style.size);
        let span_spacing: Vec<Option<f32>> = spans
            .iter()
            .map(|s| {
                if !(s.mono || s.italic || s.weight.is_some() || s.size.is_some()) {
                    return None;
                }
                let family = if s.mono { Family::Mono } else { style.family };
                let trak = self.primary_trak(family, s.weight.unwrap_or(style.weight), s.italic || style.italic);
                Some(spacing(trak, s.size.unwrap_or(style.size))).filter(|&v| v != base_spacing)
            })
            .collect();
        // Each style range's predicted first font at its weight, drawn at its named instance.
        let base_axes = self.primary_axes(style.family, style.weight, style.italic);
        let span_axes: Vec<Rc<[parley::FontVariation]>> = spans
            .iter()
            .map(|s| {
                let family = if s.mono { Family::Mono } else { style.family };
                self.primary_axes(family, s.weight.unwrap_or(style.weight), s.italic || style.italic)
            })
            .collect();
        let with_axes = |mut settings: Vec<parley::FontVariation>, axes: &[parley::FontVariation]| {
            let extra: Vec<_> = axes.iter().filter(|a| !settings.iter().any(|v| v.tag == a.tag)).copied().collect();
            settings.extend(extra);
            settings
        };
        if style.family == Family::SystemMono || self.slot_system_mono {
            self.system_mono();
        }
        let math = self.math.as_deref().unwrap_or(&[]);
        let emoji = self.emoji.as_slice();
        let system = self.system_mono.as_deref().unwrap_or_default();
        let with_symbols = |primary| {
            family_list(primary, system, self.sans.as_str())
                .into_iter()
                .chain(self.symbol_families.iter().chain(math).map(|name| FontFamilyName::named(name)))
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
            Family::SystemMono => {
                let mut families = with_symbols(self.mono.as_str());
                families.splice(0..0, system.iter().map(|name| FontFamilyName::named(name)));
                families
            }
        };
        let mono = with_symbols(self.mono.as_str());
        // Routed clusters try their fallback first, then the run's own stack (mono spans included).
        // Each of the four {math, emoji} × {sans, mono} stacks is built at most once.
        let mut stacks: [Option<Vec<FontFamilyName<'_>>>; 4] = Default::default();
        let mut routed = Vec::new();
        for (range, route) in routes {
            let fonts = match route { Route::Math => math, Route::Emoji => emoji };
            if fonts.is_empty() {
                continue;
            }
            let mono_span = spans.iter().any(|s| s.mono && (s.start as usize) < range.end && range.start < s.end as usize);
            let stack = 2 * (route == Route::Emoji) as usize + mono_span as usize;
            stacks[stack].get_or_insert_with(|| {
                let base = if mono_span { &mono } else { &sans };
                fonts.iter().map(|name| FontFamilyName::named(name)).chain(base.iter().cloned()).collect()
            });
            routed.push((range, stack));
        }
        let span_range = |s: &Span| (s.start as usize).min(text.len())..(s.end as usize).min(text.len());
        // `exact` overrides the predicted spacing with each run's own, by byte range.
        // Code never takes ligatures (`->` stays two characters); later features override earlier.
        let off = [b"liga", b"clig", b"calt", b"dlig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 0));
        let unligated: Vec<parley::FontFeature> = features.iter().chain(&off).copied().collect();
        type Spacings = [(std::ops::Range<usize>, f32)];
        let build = |lcx: &mut LayoutContext<Ink>, fcx: &mut FontContext, exact: &Spacings, widen: &Spacings| {
            let mut b = lcx.ranged_builder(fcx, text, scale, true);
            b.push_default(FontFamily::from(sans.as_slice()));
            b.push_default(StyleProperty::FontSize(style.size));
            b.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight)));
            if style.italic {
                b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
            }
            // Parley turns optional ligatures off under letter spacing; Core Text's tracking
            // keeps them, so spacing that only corrects `trak` turns them back on.
            let corrected = base_spacing != 0.0 || span_spacing.iter().flatten().chain(exact.iter().map(|e| &e.1)).any(|&v| v != 0.0);
            if mono_style {
                b.push_default(StyleProperty::FontFeatures(unligated.as_slice().into()));
            } else if style.tracking == 0.0 && corrected {
                let ligatures = [b"liga", b"clig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 1));
                let list = ligatures.iter().chain(&features).copied().collect();
                b.push_default(StyleProperty::FontFeatures(parley::FontFeatures::List(std::borrow::Cow::Owned(list))));
            } else if stripped {
                b.push_default(StyleProperty::FontFeatures(features.as_slice().into()));
            } else if !style.features.is_empty() {
                b.push_default(StyleProperty::FontFeatures(style.features.into()));
            }
            let settings = with_axes(variations(style, style.size), &base_axes);
            if !settings.is_empty() {
                b.push_default(parley::FontVariations::List(std::borrow::Cow::Owned(settings)));
            }
            b.push_default(LineHeight::Absolute(style.line_px()));
            if wrap_anywhere {
                b.push_default(StyleProperty::OverflowWrap(parley::OverflowWrap::Anywhere));
            }
            if base_spacing != 0.0 {
                b.push_default(StyleProperty::LetterSpacing(base_spacing));
            }
            for (i, s) in spans.iter().enumerate() {
                let r = span_range(s);
                if r.is_empty() {
                    continue;
                }
                b.push(StyleProperty::Brush(Ink(i as u16 + 1)), r.clone());
                if s.mono {
                    b.push(FontFamily::from(mono.as_slice()), r.clone());
                    b.push(StyleProperty::FontFeatures(unligated.as_slice().into()), r.clone());
                }
                if let Some(w) = s.weight {
                    b.push(StyleProperty::FontWeight(FontWeight::new(w)), r.clone());
                }
                if s.italic {
                    b.push(StyleProperty::FontStyle(FontStyle::Italic), r.clone());
                }
                if let Some(sz) = s.size {
                    b.push(StyleProperty::FontSize(sz), r.clone());
                    // CSS's unitless line-height: a span of its own size (inline code, a smaller run) keeps the
                    // paragraph's leading ratio, so its box fits inside the line instead of growing it
                    b.push(LineHeight::FontSizeRelative(style.line_px() / style.size), r.clone());
                }
                let resized = s.size.is_some() && style.optical_size == OpticalSize::Auto;
                if resized || span_axes[i] != base_axes {
                    let settings = with_axes(variations(style, s.size.unwrap_or(style.size)), &span_axes[i]);
                    b.push(parley::FontVariations::List(std::borrow::Cow::Owned(settings)), r.clone());
                }
                if let Some(v) = span_spacing[i] {
                    b.push(StyleProperty::LetterSpacing(v), r.clone());
                }
            }
            for (range, stack) in &routed {
                b.push(FontFamily::from(stacks[*stack].as_deref().unwrap_or_default()), range.clone());
            }
            for (range, v) in exact {
                b.push(StyleProperty::LetterSpacing(*v), range.clone());
            }
            for (range, v) in widen {
                b.push(StyleProperty::WordSpacing(*v), range.clone());
            }
            // Boxes before and after a pill's label make room for its chip. The label may break
            // like any text; `break_lines_with_blocks` moves a split pill whole to the next line
            // and lets it wrap only when it is wider than a line, as a browser wraps a span.
            for (i, r, lead, trail) in &pills {
                for (side, index, width) in [(0, r.start, *lead), (1, r.end, *trail)] {
                    b.push_inline_box(parley::InlineBox {
                        id: PILL_BOX | (*i as u64) << 1 | side,
                        kind: parley::InlineBoxKind::InFlow,
                        index,
                        width: width as f32,
                        height: 0.0,
                        baseline: None,
                        vertical_align: parley::VerticalAlign::BASELINE,
                    });
                }
            }
            let mut layout: Layout<Ink> = b.build(text);
            break_lines(&mut layout);
            layout
        };
        let mut layout = build(&mut self.layout_cx, &mut self.font_cx, &[], &widen);
        // The spacing each run got, against the spacing its own font needs.
        let given = |at: usize| {
            spans.iter().zip(&span_spacing).rev().find_map(|(s, v)| v.filter(|_| span_range(s).contains(&at))).unwrap_or(base_spacing)
        };
        let mut exact: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
        let mut mismatch = false;
        for line in layout.lines() {
            for run in line.runs() {
                let font = run.font();
                let trak = font_trak(&mut self.traks, &font.data, font.index);
                let needed = spacing(trak, run.font_size() / scale);
                let range = run.text_range();
                if range.is_empty() {
                    continue;
                }
                mismatch |= (needed - given(range.start)).abs() > 1e-4;
                match exact.last_mut() {
                    Some((last, v)) if last.end == range.start && *v == needed => last.end = range.end,
                    _ => exact.push((range, needed)),
                }
            }
        }
        if mismatch {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &exact, &widen);
        }
        layout.align(align, AlignmentOptions::default());
        let l = Rc::new(layout);
        if cache {
            self.layouts.insert(key, (l.clone(), g, text.len() as u32));
            self.shapes.insert(shape_key, key);
        }
        if let Some(shared) = shared {
            self.layout_cx = shared;
        }
        l
    }

    fn mono_tab_widening(
        &mut self,
        tabs: &[(usize, u32)],
        spans: &[Span],
        style: TextStyle,
        scale: f32,
    ) -> Vec<(std::ops::Range<usize>, f32)> {
        let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
        let default_family = if mono_style { style.family } else { Family::Mono };
        let mut last_probe: Option<((u32, u32, bool), f32)> = None;
        let mut widen: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
        for &(at, columns) in tabs {
            if columns <= 1 {
                continue;
            }
            let (mut sz, mut wt, mut it) = (style.size, style.weight, style.italic);
            for s in spans {
                if (s.start as usize) <= at && at < s.end as usize {
                    if let Some(v) = s.size { sz = v; }
                    if let Some(v) = s.weight { wt = v; }
                    if s.italic { it = true; }
                }
            }
            let probe_key = (sz.to_bits(), wt.to_bits(), it);
            let col_pt = match last_probe {
                Some((k, v)) if k == probe_key => v,
                _ => {
                    let probe = TextStyle { family: default_family, size: sz, weight: wt, italic: it, ..style };
                    let shaped = self.shaped;
                    let l = self.layout_rich(" ", &[], probe, scale, None);
                    self.shaped = shaped;
                    let adv = parley::Cluster::from_byte_index(&l, 0).map_or(0.0, |c| c.advance() / scale);
                    last_probe = Some((probe_key, adv));
                    adv
                }
            };
            if col_pt <= 0.0 {
                continue;
            }
            let add = (columns - 1) as f32 * col_pt;
            match widen.last_mut() {
                Some((r, last_add)) if r.end == at && (*last_add - add).abs() < 1e-4 => r.end = at + 1,
                _ => widen.push((at..at + 1, add)),
            }
        }
        widen
    }

    /// Size in points, rounded up to whole device pixels so layout stays on the pixel grid.
    pub fn measure(&mut self, text: &str, style: TextStyle, scale: f32) -> (f32, f32) {
        let l = self.layout_rich(text, &[], style, scale, None);
        (l.full_width().ceil() / scale, layout_height(&l, style, scale) / scale)
    }

    /// `run` shaped as one line in `style`, cached as [`Self::end_commit`] keeps layouts.
    pub fn run_layout(&mut self, run: &Run, style: TextStyle, scale: f32) -> Rc<RunLayout> {
        let key = run.key(style, scale);
        let g = self.generation;
        if let Some((r, used, _)) = self.runs.get_mut(&key) {
            *used = g;
            return r.clone();
        }
        let r = Rc::new(RunLayout::new(self.layout_rich(run.as_str(), &[], style, scale, None), run.as_str()));
        self.runs.insert(key, (r.clone(), g, run.as_str().len() as u32));
        r
    }

    /// A live run owns its latest layout; don't retain every timer tick in the commit cache.
    pub(crate) fn layout_live(&mut self, text: &str, style: TextStyle, scale: f32) -> Rc<Layout<Ink>> {
        let key = shape_key(text, &[], style, scale, false, &[]);
        let cached = self.layouts.contains_key(&key);
        let prev_shape = self.shapes.get(&key).copied();
        let layout = self.layout_rich(text, &[], style, scale, None);
        if !cached {
            self.layouts.remove(&key);
            match prev_shape {
                Some(prev) => { self.shapes.insert(key, prev); }
                None => { self.shapes.remove(&key); }
            }
        }
        layout
    }

    /// Backgrounds and underlines for `spans` (device px, layout top-left at `origin`). Touching
    /// runs with the same background merge into one rounded rect.
    pub fn decorations(&self, layout: &Layout<Ink>, spans: &[Span], scale: f32, origin: [f32; 2], out: &mut Vec<Deco>) {
        self.decorations_into(layout, spans, &[], scale, origin, out);
    }

    /// [`Self::decorations`], plus inset border `rings` (`(start_byte, end_byte, color, width_pt)`).
    pub(crate) fn decorations_with_rings(
        &self,
        layout: &Layout<Ink>,
        spans: &[Span],
        rings: &[(u32, u32, Color, f32)],
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        self.decorations_into(layout, spans, rings, scale, origin, out);
    }

    fn decorations_into<D: Decoration>(
        &self, layout: &Layout<Ink>, spans: &[Span], rings: &[(u32, u32, Color, f32)],
        scale: f32, origin: [f32; 2], out: &mut Vec<D>,
    ) {
        if spans.iter().all(|s| s.bg.is_none() && s.underline.is_none()) {
            return;
        }
        let radius = 4.0 * scale;
        let mut last_bg = None;
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
                    let ring = rings
                        .iter()
                        .rev()
                        .find(|&&(ra, rb, _, rw)| rw > 0.0 && ra < s.end && s.start < rb)
                        .map(|&(_, _, c, rw)| (c, rw * scale));
                    let pad = s.bg_pad * scale;
                    // Balanced on the cap height (not a mono face's tall line box), on whole device pixels.
                    let cap = gr.run().font_metrics().cap_height.unwrap_or(gr.run().font_size() * 0.7);
                    let base = origin[1] + m.baseline;
                    let top = (base - cap * 1.35).round();
                    let bot = (base + cap * 0.55).round();
                    let (l, r) = ((x0 - pad).round(), (x0 + w + pad).round());
                    let rect = [l, top, r - l, bot - top];
                    // Touching runs with the same background and ring on one line (a span split by
                    // style, or inline code and its padding spaces in the body font) share one rect,
                    // as tall as its widest run, so a thin run in another font doesn't poke out as a sliver.
                    let byte = (gr.run().text_range().end as u32).min(s.end).saturating_sub(1);
                    let merge = last_bg.is_some_and(|i: usize| {
                        let (d, old_ring) = out[i].paint();
                        d.color == bg && old_ring == ring
                            && (d.radius - radius).abs() < 1e-3
                            && d.rect[1] < bot
                            && top < d.rect[1] + d.rect[3]
                            && (d.rect[0] + d.rect[2] - rect[0] - 2.0 * pad).abs() < 1.5
                    });
                    if merge {
                        let painted = &mut out[last_bg.unwrap()];
                        let (d, _) = painted.paint();
                        if rect[2] > d.rect[2] {
                            (d.rect[1], d.rect[3]) = (rect[1], rect[3]);
                        }
                        d.rect[2] = rect[0] + rect[2] - d.rect[0];
                        painted.extend_to(byte);
                    } else {
                        last_bg = Some(out.len());
                        out.push(D::new(Deco { rect, color: bg, radius }, ring, byte));
                    }
                }
                if let Some(uc) = s.underline.filter(|c| c.a > 0.0) {
                    let y = (origin[1] + m.baseline + 2.0 * scale).round();
                    let (l, r) = underline_edges(&line, origin[0], gr.offset(), gr.offset() + w);
                    out.push(D::new(Deco { rect: [l, y, r - l, scale.max(1.0)], color: uc, radius: 0.0 }, None, 0));
                }
            }
        }
    }

    /// Strikethrough lines for `strikes` (`(start_byte, end_byte, color)`) in device px.
    pub(crate) fn strike_decorations(
        &self,
        layout: &Layout<Ink>,
        spans: &[Span],
        strikes: &[(u32, u32, Option<Color>)],
        default_color: Color,
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        if strikes.is_empty() {
            return;
        }
        for &(s_start, s_end, sc) in strikes {
            let (a, b) = (s_start as usize, s_end as usize);
            if a >= b {
                continue;
            }
            let span_color = sc.or_else(|| {
                spans
                    .iter()
                    .find(|sp| (sp.start as usize) <= a && b <= (sp.end as usize) && sp.color.is_some())
                    .and_then(|sp| sp.color)
            });
            let color = span_color.unwrap_or(default_color);
            for (bb, line_idx) in range_boxes(layout, a, b, false) {
                let w = (bb.x1 - bb.x0) as f32;
                if w <= 0.0 {
                    continue;
                }
                let Some(line) = layout.get(line_idx) else { continue };
                let m = line.metrics();
                let mut strike_off = m.line_height * 0.25;
                let mut h = scale.round().max(1.0);
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(gr) = item {
                        let fm = gr.run().font_metrics();
                        strike_off = if fm.strikethrough_offset != 0.0 {
                            fm.strikethrough_offset
                        } else {
                            fm.ascent * 0.32
                        };
                        h = fm.strikethrough_size.max(scale).round().max(1.0);
                        break;
                    }
                }
                let x0 = origin[0] + bb.x0 as f32;
                let y = (origin[1] + m.baseline - strike_off).round();
                out.push(PaintedDeco::new(Deco { rect: [x0, y, w, h], color, radius: 0.0 }, None, 0));
            }
        }
    }

    /// Rounded background fills and rings for `highlights` in device px.
    pub(crate) fn highlight_decorations(
        &self,
        layout: &Layout<Ink>,
        highlights: &[TextHighlight],
        len: usize,
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        if highlights.is_empty() {
            return;
        }
        let radius = 4.0 * scale;
        for h in highlights {
            let (a, b) = ((h.start as usize).min(len), (h.end as usize).min(len));
            if a >= b || (h.bg.a <= 0.0 && h.ring.is_none()) {
                continue;
            }
            let pad = h.pad * scale;
            let ring = h.ring.map(|(c, rw)| (c, rw * scale));
            let mut last: Option<(usize, usize)> = None;
            for (bb, line_idx) in range_boxes(layout, a, b, false) {
                let w = (bb.x1 - bb.x0) as f32;
                if w <= 0.0 {
                    continue;
                }
                let x0 = origin[0] + bb.x0 as f32;
                let (mut top, mut bot) = ((origin[1] + bb.y0 as f32).round(), (origin[1] + bb.y1 as f32).round());
                if let Some(l) = layout.get(line_idx) {
                    let base = origin[1] + l.metrics().baseline;
                    let (mut asc, mut desc) = (0.0f32, 0.0f32);
                    for run in l.runs() {
                        let r = run.text_range();
                        if r.start < b && a < r.end {
                            let fm = run.font_metrics();
                            (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                        }
                    }
                    if asc > 0.0 || desc > 0.0 {
                        top = (base - asc - 1.0 * scale).round();
                        bot = (base + desc + 1.0 * scale).round();
                    }
                }
                let rect = [x0 - pad, top, w + 2.0 * pad, (bot - top).max(0.0)];
                let merge = last.filter(|&(li, _)| li == line_idx).is_some_and(|(_, k)| {
                    let (d, _) = out[k].paint();
                    (d.rect[0] + d.rect[2] - rect[0] - 2.0 * pad).abs() < 1.0
                });
                if merge {
                    let (d, _) = out[last.unwrap().1].paint();
                    if rect[3] > d.rect[3] {
                        (d.rect[1], d.rect[3]) = (rect[1], rect[3]);
                    }
                    d.rect[2] = rect[0] + rect[2] - d.rect[0];
                } else {
                    last = Some((line_idx, out.len()));
                    out.push(PaintedDeco::new(Deco { rect, color: h.bg, radius }, ring, (b as u32).saturating_sub(1)));
                }
            }
        }
    }

    /// The atlas quad for `icon` at `px` device pixels square, top-left at `origin`.
    pub fn icon_quad(&mut self, icon: &'static Icon, px: u32, origin: [f32; 2]) -> Option<GlyphQuad> {
        let key = (icon as *const Icon as usize, px);
        let entry = match self.atlas.icons.get(&key) {
            Some(&e) => {
                if let Some(a) = e {
                    self.atlas.touch(a.page, a.w * a.h);
                }
                e
            }
            None => match rasterize_icon(icon, px, &mut self.atlas) {
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
            rect: [origin[0].round(), origin[1].round(), a.w as f32, a.h as f32],
            uv: [a.x as f32, (a.page as u32 * ATLAS_SIZE + a.y) as f32],
            ink: 0,
            byte: 0,
            color: 0,
        })
    }
}

/// Breaks `layout` into lines, each paragraph indented by its block. A pill's chip that would
/// end a line apart from its label, or run past the line's end, moves to the next line.
fn break_lines_with_blocks(
    layout: &mut Layout<Ink>,
    blocks: &[crate::edit::Block],
    font_size: f32,
    scale: f32,
    max_width: Option<f32>,
) {
    let pills = layout.inline_boxes().filter(|b| b.id & PILL_BOX != 0).count() / 2;
    // `(para_idx, line_in_para)` and the room it gets instead: less to break before a chip, or
    // more to keep a chip with its label. Paragraphs break independently at `\n`, so one pass
    // can adjust the first misplaced pill in every paragraph at once.
    let mut caps: Vec<((usize, usize), f32)> = Vec::new();
    let mut lines_meta = Vec::new();
    for _ in 0..=2 * pills {
        break_lines_capped(layout, blocks, font_size, scale, max_width, &caps, &mut lines_meta);
        if max_width.is_none() || pills == 0 || !collect_misplaced_pills(layout, &lines_meta, &mut caps) {
            return;
        }
    }
}

/// [`break_lines_with_blocks`] once, with `caps` setting the room on given `(para_idx, line_in_para)`
/// lines (the last for a line wins). Fills `out` with `((para_idx, line_in_para), uncapped_room)`.
fn break_lines_capped(
    layout: &mut Layout<Ink>,
    blocks: &[crate::edit::Block],
    font_size: f32,
    scale: f32,
    max_width: Option<f32>,
    caps: &[((usize, usize), f32)],
    out: &mut Vec<((usize, usize), f32)>,
) {
    out.clear();
    let layout_max = max_width.unwrap_or(f32::MAX);
    let mut breaker = layout.break_lines();
    breaker.state_mut().set_layout_max_advance(layout_max);
    let (mut para_idx, mut line_in_para) = (0usize, 0usize);
    while !breaker.is_done() {
        let blk = blocks.get(para_idx).copied().unwrap_or(crate::edit::Block::Paragraph);
        let indent = (blk.indent_pt(font_size) * scale).round();
        let right_pad = if blk == crate::edit::Block::Code { indent } else { 0.0 };
        let line_max = max_width
            .map_or(f32::MAX, |w| (w - indent - right_pad).max(1.0))
            .min(layout_max);
        let key = (para_idx, line_in_para);
        out.push((key, line_max));
        let capped = caps.iter().rfind(|c| c.0 == key).map_or(line_max, |c| c.1);
        breaker.state_mut().set_line_x(indent);
        // Parley wants no line wider than the layout: a widened line widens it while it breaks.
        breaker.state_mut().set_layout_max_advance(layout_max.max(capped));
        breaker.state_mut().set_line_max_advance(capped);
        match breaker.break_next() {
            Some(parley::YieldData::LineBreak(data)) => {
                if data.reason == parley::BreakReason::Explicit {
                    para_idx += 1;
                    line_in_para = 0;
                } else {
                    line_in_para += 1;
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    breaker.state_mut().set_layout_max_advance(layout_max);
}

/// Appends the first misplaced pill per paragraph to `caps`, returning whether any was added.
fn collect_misplaced_pills(
    layout: &Layout<Ink>,
    lines_meta: &[((usize, usize), f32)],
    caps: &mut Vec<((usize, usize), f32)>,
) -> bool {
    let before = caps.len();
    let mut skip_para = None;
    let mut boxes: Vec<parley::PositionedInlineBox> = Vec::new();
    for (li, line) in layout.lines().enumerate() {
        let Some(&(key, room)) = lines_meta.get(li) else { break };
        if skip_para == Some(key.0) {
            continue;
        }
        boxes.clear();
        boxes.extend(line.items().filter_map(|it| match it {
            PositionedLayoutItem::InlineBox(b) if b.id & PILL_BOX != 0 => Some(b),
            _ => None,
        }));
        if boxes.is_empty() {
            continue;
        }
        let m = line.metrics();
        // Device px from the start of the line's content, as the breaker counts.
        let at = |x: f32| x - m.inline_min_coord - m.offset;
        for (k, lead) in boxes.iter().enumerate().filter(|(_, b)| b.id & 1 == 0) {
            let trail = boxes.iter().find(|b| b.id == lead.id | 1);
            if trail.is_some_and(|t| at(t.x) + t.width <= room + 0.5) {
                continue;
            }
            // A chip starting a line whose trail alone went on to the next: widen this line to
            // hold the whole pill. A label that itself went on is wider than a line: it wraps.
            let fix = if trail.is_none() && at(lead.x) < 0.5 {
                layout.get(li + 1).and_then(|next| {
                    let nm = next.metrics();
                    let t = next.items().find_map(|it| match it {
                        PositionedLayoutItem::InlineBox(b) if b.id == lead.id | 1 => Some(b),
                        _ => None,
                    })?;
                    let x = t.x - nm.inline_min_coord - nm.offset;
                    (x < 0.5).then(|| m.advance + x + t.width + 1.0)
                })
            } else {
                let mut first = k;
                while first >= 2
                    && boxes[first - 1].id & 1 == 1
                    && (boxes[first - 1].x + boxes[first - 1].width - boxes[first].x).abs() < 0.5
                    && boxes[first - 2].id == boxes[first - 1].id & !1
                {
                    first -= 2;
                }
                let x = at(boxes[first].x);
                (x >= 0.5).then(|| x + boxes[first].width * 0.5)
            };
            if let Some(w) = fix {
                let cap = (key, w);
                if !caps.contains(&cap) {
                    caps.push(cap);
                    skip_para = Some(key.0);
                    break;
                }
            }
        }
    }
    caps.len() > before
}

/// Parley 0.12's `Cursor::geometry` omits `line.metrics().inline_min_coord`; add it so carets on
/// indented paragraphs (`Block::Bullet`, `Block::Numbered`, `Block::Quote`, `Block::Code`) align
/// with their shaped glyph runs.
pub(crate) fn cursor_geometry(
    cursor: &parley::editing::Cursor,
    layout: &Layout<Ink>,
    size: f32,
) -> parley::BoundingBox {
    let mut bb = cursor.geometry(layout, size);
    let mut inline_min = match cursor.visual_clusters(layout) {
        [Some(left), _] => left.line().metrics().inline_min_coord,
        [_, Some(right)] => right.line().metrics().inline_min_coord,
        _ => layout
            .get(layout.len().saturating_sub(1))
            .map_or(0.0, |l| l.metrics().inline_min_coord),
    };
    // Parley's `Cluster::is_end_of_line` is false when a line ends with a pill's trailing box,
    // which leaves a Downstream cursor after a wrapped pill on the previous line.
    if let [Some(left), Some(right)] = cursor.visual_clusters(layout) {
        let (ll, rl) = (left.line(), right.line());
        let (lm, rm) = (ll.metrics(), rl.metrics());
        if lm.block_min_coord != rm.block_min_coord {
            let on_right = ll.break_reason() == parley::BreakReason::Explicit
                || (!left.is_rtl() && cursor.affinity() == parley::layout::Affinity::Downstream)
                || (left.is_rtl() && cursor.affinity() == parley::layout::Affinity::Upstream);
            if on_right {
                let x = right.visual_offset().unwrap_or_default();
                bb = parley::BoundingBox::new(
                    x as f64,
                    rm.block_min_coord.min(rm.content_block_min_coord) as f64,
                    (x + size) as f64,
                    rm.block_max_coord.max(rm.content_block_max_coord) as f64,
                );
                inline_min = rm.inline_min_coord;
            }
        }
    }
    if inline_min != 0.0 {
        bb.x0 += inline_min as f64;
        bb.x1 += inline_min as f64;
    }
    // Parley puts a caret at a pill's edge against its label, inside the chip: move it outside.
    let at = cursor.index();
    if let Some(x) = pill_edge(layout, at, ((bb.y0 + bb.y1) * 0.5) as f32) {
        let w = bb.x1 - bb.x0;
        bb.x0 = x as f64;
        bb.x1 = bb.x0 + w;
    }
    bb
}

/// The x just outside the pill chip whose label starts or ends at byte `at`, on the line at
/// height `y`: after the chip of a pill ending there, else before one starting there.
fn pill_edge(layout: &Layout<Ink>, at: usize, y: f32) -> Option<f32> {
    let ids: Vec<u64> = layout.inline_boxes().filter(|b| b.index == at && b.id & PILL_BOX != 0).map(|b| b.id).collect();
    if ids.is_empty() {
        return None;
    }
    let line = layout.lines().find(|l| {
        let m = l.metrics();
        m.block_min_coord <= y && y <= m.block_max_coord
    })?;
    let boxes: Vec<parley::PositionedInlineBox> = line
        .items()
        .filter_map(|it| match it {
            PositionedLayoutItem::InlineBox(b) if ids.contains(&b.id) => Some(b),
            _ => None,
        })
        .collect();
    if let Some(t) = boxes.iter().find(|b| b.id & 1 == 1) {
        return Some(t.x + t.width);
    }
    let lead = boxes.first()?;
    // If bidi reordering moved `lead` away from the pill's glyph run, step left from the run.
    let start_byte = layout.inline_boxes().find(|b| b.id == lead.id)?.index;
    let gr_x0 = line.items().find_map(|it| match it {
        PositionedLayoutItem::GlyphRun(gr) if gr.run().text_range().contains(&start_byte) => Some(gr.offset()),
        _ => None,
    });
    if let Some(gx) = gr_x0.filter(|&gx| (lead.x + lead.width - gx).abs() >= 1.0) {
        return Some(gx - lead.width);
    }
    Some(lead.x)
}

fn line_for_offset(layout: &Layout<Ink>, offset: f32) -> Option<(usize, parley::Line<'_, Ink>)> {
    if offset < 0.0 {
        return Some((0, layout.get(0)?));
    }
    for (i, line) in layout.lines().enumerate() {
        let m = line.metrics();
        if (m.block_min_coord..m.block_max_coord).contains(&offset) {
            return Some((i, line));
        }
    }
    None
}

/// Hit-tests `(x, y)` (layout device px) to a [`parley::editing::Cursor`], accounting for line
/// indentation and pill inline boxes (where Parley's `Cluster::from_point` falls through to the
/// end of the document when a line starts or ends with an inline box, or folds a trailing box's
/// width into the following cluster).
pub(crate) fn cursor_from_point(layout: &Layout<Ink>, x: f32, y: f32) -> parley::editing::Cursor {
    use parley::{BreakReason, editing::Cursor, layout::Affinity};
    if layout.inline_boxes().next().is_none() && layout.lines().all(|l| l.metrics().inline_min_coord == 0.0) {
        return Cursor::from_point(layout, x, y);
    }
    if layout.len() == 0 {
        return Cursor::from_byte_index(layout, 0, Affinity::Downstream);
    }
    let first = layout.get(0).unwrap();
    let fm = first.metrics();
    if y <= fm.block_min_coord && x <= fm.inline_min_coord + fm.offset {
        return Cursor::from_byte_index(layout, 0, Affinity::Downstream);
    }
    let last_idx = layout.len() - 1;
    let last = layout.get(last_idx).unwrap();
    let lm = last.metrics();
    if y >= lm.block_max_coord && x >= lm.inline_min_coord + lm.offset + lm.advance {
        return Cursor::from_byte_index(layout, last.text_range().end, Affinity::Upstream);
    }
    let line = if y <= fm.block_min_coord {
        first
    } else if y >= lm.block_max_coord {
        last
    } else {
        line_for_offset(layout, y).map_or(last, |(_, l)| l)
    };
    let m = line.metrics();
    let r = line.text_range();
    if x <= m.inline_min_coord + m.offset {
        return Cursor::from_byte_index(layout, r.start, Affinity::Downstream);
    }
    for item in line.items() {
        if let PositionedLayoutItem::InlineBox(b) = item
            && b.id & PILL_BOX != 0
            && x >= b.x
            && x <= b.x + b.width
            && let Some(ib) = layout.inline_boxes().find(|ib| ib.id == b.id)
        {
            let aff = if b.id & 1 == 0 { Affinity::Downstream } else { Affinity::Upstream };
            return Cursor::from_byte_index(layout, ib.index, aff);
        }
    }
    for run in line.runs() {
        let mut cx = m.inline_min_coord + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset);
        for cl in run.visual_clusters() {
            let adv = cl.advance();
            if x <= cx + adv {
                let leading = x - cx <= adv * 0.5;
                let (idx, aff) = if cl.is_rtl() {
                    if leading {
                        (cl.text_range().end, Affinity::Upstream)
                    } else {
                        (cl.text_range().start, Affinity::Downstream)
                    }
                } else if leading || cl.is_hard_line_break() {
                    (cl.text_range().start, Affinity::Downstream)
                } else {
                    (cl.text_range().end, Affinity::Upstream)
                };
                return Cursor::from_byte_index(layout, idx, aff);
            }
            cx += adv;
        }
    }
    if line.break_reason() == BreakReason::Explicit && r.end > r.start {
        if let Some(cl) = parley::Cluster::from_byte_index(layout, r.end - 1) {
            return Cursor::from_byte_index(layout, cl.text_range().start, Affinity::Downstream);
        }
    }
    Cursor::from_byte_index(layout, r.end, Affinity::Upstream)
}

/// Start (`!end`) or end (`end`) byte of the visual line containing `byte`.
pub(crate) fn line_edge(layout: &Layout<Ink>, byte: usize, end: bool) -> usize {
    use parley::{BreakReason, editing::Cursor, layout::Affinity};
    let cur = Cursor::from_byte_index(layout, byte, Affinity::Downstream);
    let bb = cursor_geometry(&cur, layout, 0.0);
    let y = ((bb.y0 + bb.y1) * 0.5) as f32;
    let Some((_, line)) = line_for_offset(layout, y).or_else(|| {
        let last = layout.len().checked_sub(1)?;
        Some((last, layout.get(last)?))
    }) else {
        return byte;
    };
    let r = line.text_range();
    if !end {
        return r.start;
    }
    if line.break_reason() == BreakReason::Explicit && r.end > r.start {
        parley::Cluster::from_byte_index(layout, r.end - 1).map_or(r.end - 1, |c| c.text_range().start)
    } else {
        r.end
    }
}

/// Byte index on the previous (`!down`) or next (`down`) visual line at `byte`'s x coordinate,
/// or `None` when already on the last line moving down.
pub(crate) fn move_line(layout: &Layout<Ink>, byte: usize, down: bool) -> Option<usize> {
    use parley::{editing::Cursor, layout::Affinity};
    if layout.len() == 0 {
        return None;
    }
    let cur = Cursor::from_byte_index(layout, byte, Affinity::Downstream);
    let bb = cursor_geometry(&cur, layout, 0.0);
    let y = ((bb.y0 + bb.y1) * 0.5) as f32;
    let limit = layout.len() - 1;
    let idx = line_for_offset(layout, y).map_or(limit, |(ix, _)| ix);
    if !down && idx == 0 {
        return Some(0);
    }
    if down && idx >= limit {
        return None;
    }
    let line = layout.get(if down { idx + 1 } else { idx - 1 })?;
    let m = line.metrics();
    Some(cursor_from_point(layout, bb.x0 as f32, (m.block_min_coord + m.block_max_coord) * 0.5).index())
}

/// Line boxes (`(BoundingBox, line_idx)`) covering bytes `a..b` of `layout`. When `pill_boxes`
/// is true, a selected pill's leading and trailing chip boxes are included in the highlight;
/// when false (underlines, strikethroughs, link hit boxes), inline boxes are excluded so they
/// never bleed into a neighbouring cluster's box.
pub(crate) fn range_boxes(
    layout: &Layout<Ink>,
    a: usize,
    b: usize,
    pill_boxes: bool,
) -> Vec<(parley::BoundingBox, usize)> {
    use parley::{BreakReason, editing::Cursor, editing::Selection, layout::Affinity};
    if a >= b {
        return Vec::new();
    }
    if layout.inline_boxes().next().is_none() {
        return Selection::new(
            Cursor::from_byte_index(layout, a, Affinity::Downstream),
            Cursor::from_byte_index(layout, b, Affinity::Upstream),
        )
        .geometry(layout);
    }
    let range = a..b;
    let mut rects = Vec::new();
    for (line_ix, line) in layout.lines().enumerate() {
        let lr = line.text_range();
        if lr.end <= a || lr.start >= b {
            continue;
        }
        let m = line.metrics();
        let line_min = m.block_min_coord.min(m.content_block_min_coord) as f64;
        let line_max = m.block_max_coord.max(m.content_block_max_coord) as f64;
        let newline_ws = if line.break_reason() == BreakReason::Explicit
            && lr.end > lr.start
            && range.contains(&(lr.end - 1))
        {
            parley::Cluster::from_byte_index(layout, lr.end - 1).map_or(0.0, |c| {
                let run = c.run();
                let fm = run.font_metrics();
                (fm.ascent as f64 + fm.descent as f64) * 0.25
            })
        } else {
            0.0
        };
        let mut intervals: Vec<(f64, f64)> = Vec::new();
        for run in line.runs() {
            let mut cx = (m.inline_min_coord + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset)) as f64;
            for cl in run.visual_clusters() {
                let adv = cl.advance() as f64;
                if range.contains(&cl.text_range().start) && adv > 0.0 {
                    intervals.push((cx, cx + adv));
                }
                cx += adv;
            }
        }
        if pill_boxes {
            for item in line.items() {
                if let PositionedLayoutItem::InlineBox(ib) = item
                    && ib.id & PILL_BOX != 0
                    && ib.width > 0.0
                    && layout.inline_boxes().find(|b| b.id == ib.id).is_some_and(|b| {
                        if ib.id & 1 == 0 {
                            range.contains(&b.index)
                        } else {
                            b.index > 0 && range.contains(&(b.index - 1))
                        }
                    })
                {
                    intervals.push((ib.x as f64, (ib.x + ib.width) as f64));
                }
            }
        }
        intervals.sort_by(|p, q| p.0.total_cmp(&q.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (x0, x1) in intervals {
            match merged.last_mut() {
                Some(last) if x0 <= last.1 + 0.5 => last.1 = last.1.max(x1),
                _ => merged.push((x0, x1)),
            }
        }
        if let Some(last) = merged.last_mut() {
            last.1 += newline_ws;
        } else if newline_ws > 0.0 {
            let x0 = (m.inline_min_coord + m.offset + m.advance) as f64;
            merged.push((x0, x0 + newline_ws));
        }
        for (x0, x1) in merged {
            if x1 > x0 {
                rects.push((parley::BoundingBox::new(x0, line_min, x1, line_max), line_ix));
            }
        }
    }
    rects
}

#[cfg(test)]
#[path = "text/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "text/quality_tests.rs"]
mod quality_tests;

#[cfg(test)]
#[path = "text/rewrap_tests.rs"]
mod rewrap_tests;
