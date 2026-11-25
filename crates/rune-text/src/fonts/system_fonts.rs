//! The system's fonts: monospace and symbol families, CJK and script fallbacks, and the font
//! directories they're found in.
//!
//! None is touched at launch: a system font loads the first time a text has a character the
//! app's own fonts lack (`TextSystem::load_fallbacks`). The platform's own fallback names the
//! font and its files, as browsers ask it: DirectWrite on Windows (`direct_write`), CoreText on
//! Apple platforms (`core_text`), fontconfig in process elsewhere (`fontconfig`). No font
//! folder is walked and no process is started.

use crate::*;

#[cfg(target_vendor = "apple")]
mod core_text;
mod coverage;
#[cfg(windows)]
mod direct_write;
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
mod fontconfig;
mod installed;
#[cfg(target_vendor = "apple")]
use core_text as platform;
#[cfg(windows)]
use direct_write as platform;
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
use fontconfig as platform;
/// Android and the web: no system fonts to ask.
#[cfg(any(target_os = "android", target_arch = "wasm32"))]
mod platform {
    pub(crate) fn family_files(_: &[&str]) -> Vec<std::path::PathBuf> { Vec::new() }
    pub(crate) fn fallback_family_files(_: &[char], _: Option<&str>) -> Vec<std::path::PathBuf> { Vec::new() }
    pub(super) fn installed_families() -> Vec<super::installed::RawFamily> { Vec::new() }
}

pub use installed::{InstalledFamily, installed_families};

// Common-script symbols can inherit the script of adjacent letters. Put these families at
// the end of the requested family list, rather than registering only a Zyyy fallback.
// On macOS the system font comes first: its keyboard symbols (⌘⇧⏎⌫⎋⌥⌃) sit on the baseline at
// cap height and follow the requested weight through its `wght` axis. Apple Symbols draws
// them light, small and (⇧) below the baseline next to app fonts' capitals.
// Apple Color Emoji loads separately on demand and sits last, so symbols with a text form keep it.
#[cfg(target_vendor = "apple")]
const SLOT_FALLBACK_FAMILIES: &[&str] = &["System Font", ".SF NS"];
#[cfg(target_vendor = "apple")]
pub(crate) const SYMBOL_FAMILIES: &[&str] = &["System Font", ".SF NS", "Apple Symbols", ".SF NS Symbols", "Menlo"];
#[cfg(target_vendor = "apple")]
const EMOJI_FAMILIES: &[&str] = &["Apple Color Emoji"];
// Windows loads its symbol fonts as its own fallback names them (`direct_write::fallback_files`),
// and this colour emoji family for a `U+FE0F` sequence.
#[cfg(windows)]
pub(crate) const SYMBOL_FAMILIES: &[&str] = &["Segoe UI Emoji"];
// DejaVu Sans lacks ⎋ (U+238B), which DejaVu Sans Mono has; `dejavusans` loads both files.
// Noto Sans stands in where DejaVu isn't installed (Arch and its derivatives ship Noto alone):
// without one of them, a Latin combining mark Inter lacks (U+0335, U+0322) draws as tofu.
// Current Noto releases name Symbols 2 with a space ("Noto Sans Symbols 2"), while older packages
// used "Noto Sans Symbols2"; list both so either matches fontique's family lookup.
#[cfg_attr(any(target_vendor = "apple", windows), allow(dead_code))]
pub(crate) const LINUX_SYMBOL_FAMILIES: &[&str] = &[
    "DejaVu Sans",
    "Noto Sans",
    "Noto Sans Symbols 2",
    "Noto Sans Symbols2",
    "Noto Sans Symbols",
    "DejaVu Sans Mono",
    "Noto Color Emoji",
];
#[cfg(not(any(target_vendor = "apple", windows)))]
pub(crate) const SYMBOL_FAMILIES: &[&str] = LINUX_SYMBOL_FAMILIES;
// Only Noto Sans's upright regular file, static or variable, so other Noto Sans* script fonts
// stay out when matching by file name.
#[cfg_attr(any(target_vendor = "apple", windows), allow(dead_code))]
pub(crate) const LINUX_SYMBOL_FONT_FILES: &[&str] = &[
    "dejavusans",
    "notosans-regular",
    "notosans[",
    "notosanssymbols",
    "notocoloremoji",
];
#[cfg(not(any(target_vendor = "apple", windows)))]
pub(crate) const SYMBOL_FONT_FILES: &[&str] = LINUX_SYMBOL_FONT_FILES;

// `Family::SystemMono`: the platform's monospaced UI families, most preferred first, and the
// files they're in. SF Mono's family name is `.SF NS Mono`, as SF's is `.SF NS`.
#[cfg(target_vendor = "apple")]
const SYSTEM_MONO_FAMILIES: &[&str] = &[".SF NS Mono", "Menlo"];
#[cfg(target_vendor = "apple")]
const SYSTEM_MONO_FILES: &[&str] = &["sfnsmono", "menlo"];
#[cfg(windows)]
const SYSTEM_MONO_FAMILIES: &[&str] = &["Cascadia Mono", "Consolas", "Courier New"];
#[cfg(windows)]
const SYSTEM_MONO_FILES: &[&str] = &["cascadiamono", "consola", "cour"];
// After fontconfig's own answer (`fontconfig_monospace`).
#[cfg(not(any(target_vendor = "apple", windows)))]
const SYSTEM_MONO_FAMILIES: &[&str] = &["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono", "Ubuntu Mono"];
#[cfg(not(any(target_vendor = "apple", windows)))]
const SYSTEM_MONO_FILES: &[&str] = &["dejavusansmono", "liberationmono", "notosansmono", "ubuntumono"];

/// The installed `Family::SystemMono` families, most preferred first, loading their files.
pub(crate) fn load_system_mono(collection: &mut Collection, loaded: &mut LoadedFiles) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    // fontconfig's own `monospace`, asked in process.
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
    if let Some((family, files)) = fontconfig::monospace() {
        load_files(collection, loaded, files);
        names.push(family);
    }
    load_files(collection, loaded, installed_files(SYSTEM_MONO_FAMILIES, SYSTEM_MONO_FILES).into_iter().take(12));
    for name in SYSTEM_MONO_FAMILIES {
        if !names.iter().any(|n| n == name) { names.push((*name).to_owned()); }
    }
    names.retain(|name| collection.family_id(name).is_some());
    names
}

/// Whether the CSS family list `list` names `ui-monospace`, which stands for `Family::SystemMono`'s
/// families, so the mono slot can be SF Mono, whose family name is hidden.
pub(crate) fn names_system_mono(list: &str) -> bool {
    FontFamilyName::parse_css_list(list).flatten().any(|f| f == FontFamilyName::Generic(parley::GenericFamily::UiMonospace))
}

/// `list` parsed, with `ui-monospace` replaced by the installed `Family::SystemMono` families.
/// When that leaves no named family (no monospace font installed, or the web), `sans` follows,
/// so text still finds a font.
pub(crate) fn family_list<'a>(list: &'a str, system_mono: &'a [String], sans: &'a str) -> Vec<FontFamilyName<'a>> {
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
pub(crate) fn load_families(collection: &mut Collection, files: &[&str], names: &[&'static str]) -> Vec<&'static str> {
    collection.load_fonts_from_paths(installed_files(names, files));
    present_families(collection, names)
}

/// The families from `names` present in `collection`, in order.
pub(crate) fn present_families(collection: &mut Collection, names: &[&'static str]) -> Vec<&'static str> {
    names.iter().copied().filter(|name| collection.family_id(name).is_some()).collect()
}

/// The files of the installed families `names`, found by name through the platform's own list
/// of fonts. `fragments` (their files' names) are what the tests check for what's installed.
fn installed_files(names: &[&str], fragments: &[&str]) -> Vec<std::path::PathBuf> {
    let _ = fragments;
    platform::family_files(names)
}

/// File-name fragments (lowercase) of the CJK fonts desktops ship, most preferred first: what the
/// tests check for what's installed (the platform's fallback finds the files the app loads).
/// macOS names Hiragino Sans's files in Japanese (`ヒラギノ角ゴシック W3.ttc`), in either Unicode
/// form.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const CJK_FONT_FILES: [&str; 30] = [
    "notosanscjk", "notosanssc", "notosansjp", "notosanstc", "notosanshk", "notosanskr", "sourcehansans",
    "notoserifcjk", "sourcehanserif", "pingfang", "hiragino",
    "\u{30d2}\u{30e9}\u{30ae}\u{30ce}\u{89d2}\u{30b4}\u{30b7}\u{30c3}\u{30af}w3",
    "\u{30d2}\u{30e9}\u{30ad}\u{3099}\u{30ce}\u{89d2}\u{30b3}\u{3099}\u{30b7}\u{30c3}\u{30af}w3",
    "\u{30d2}\u{30e9}\u{30ae}\u{30ce}\u{89d2}\u{30b4}\u{30b7}\u{30c3}\u{30af}w6",
    "\u{30d2}\u{30e9}\u{30ad}\u{3099}\u{30ce}\u{89d2}\u{30b3}\u{3099}\u{30b7}\u{30c3}\u{30af}w6",
    "applesdgothicneo", "msyh", "yugoth", "meiryo",
    "malgun", "msjh", "wqy", "wenquanyi", "droidsansfallback", "sarasa", "lxgw", "ukai", "uming", "simsun",
    "applegothic",
];

/// Family IDs currently named by the sans and mono slots.
fn primary_family_ids(collection: &mut Collection, sans: &str, mono: &str) -> Vec<FamilyId> {
    css_or_named(sans)
        .into_iter()
        .chain(css_or_named(mono))
        .filter_map(|f| match f {
            FontFamilyName::Named(name) => collection.family_id(&name),
            FontFamilyName::Generic(_) => None,
        })
        .collect()
}

/// The CJK fonts in `files` as fallbacks after the app's, memory-mapped, so a 20 MB CJK
/// collection costs a mapping, not a read. Every CJK fallback so far (`all`) is ordered again
/// for each language, so a Japanese font loaded after a Chinese one still comes first for
/// Japanese text.
fn register_system_cjk(collection: &mut Collection, loaded: &mut LoadedFiles, files: Vec<std::path::PathBuf>, families: (&str, &str), all: &mut Vec<FamilyId>) {
    let primary = [collection.family_id(families.0), collection.family_id(families.1)];
    let before = all.len();
    for id in load_files(collection, loaded, files.into_iter().take(16)) {
        if !primary.contains(&Some(id)) && !all.contains(&id) {
            all.push(id);
        }
    }
    if all.len() > before {
        super::language::register(collection, all);
    }
}

/// What `TextSystem::load_fallbacks` has looked up, so each system font is looked for once.
#[derive(Default)]
pub(crate) struct Fallbacks {
    /// The families registered from the app's initial font slices, deduplicated in registration order.
    own_families: Vec<FamilyId>,
    /// Characters already looked up (Windows: each asked of DirectWrite once).
    asked: std::collections::HashSet<char>,
    /// The locales the system's CJK fonts were looked up for (Windows), or whether they were
    /// loaded at all (elsewhere: `Some(None)`).
    cjk: Vec<Option<String>>,
    /// The system UI text family was loaded for an unresolved font slot (Apple).
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    slot_fallback: bool,
    /// The symbol families were loaded (elsewhere than Windows, where each comes on its own).
    #[cfg_attr(windows, allow(dead_code))]
    symbols: bool,
    /// System CJK fallback families loaded on demand, in load order.
    system_cjk: Vec<FamilyId>,
    /// Every CJK fallback family currently registered, the app's non-primary then the system's.
    cjk_families: Vec<FamilyId>,
    /// Segoe UI Symbol is loaded, for symbols with a text form (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    text_symbols: bool,
    /// Cached colour-table check (`COLR`, `CBDT` or `sbix`) per registered `FamilyId`.
    color_families: HashMap<FamilyId, bool>,
}

impl Fallbacks {
    pub(crate) fn new(registered: &[FamilyId]) -> Self {
        let mut own_families = Vec::new();
        for &id in registered {
            if !own_families.contains(&id) {
                own_families.push(id);
            }
        }
        Fallbacks { own_families, ..Default::default() }
    }

    pub(crate) fn clear_color_family_cache(&mut self) {
        self.color_families.clear();
    }
}

/// Han, kana, hangul, bopomofo and their punctuation and full-width forms.
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x11FF | 0x2E80..=0x2FDF | 0x2FF0..=0x303F | 0x3040..=0x31FF | 0x3200..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

/// Characters no font is looked up for: controls, joiners, bidi marks, variation selectors,
/// tags and private-use icons (an app's icon font has them or nothing does).
fn needs_no_font(ch: char) -> bool {
    ch.is_control()
        || matches!(ch as u32,
            0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0xFE00..=0xFE0F | 0xFEFF | 0xE0000..=0xE0FFF
            | 0xE000..=0xF8FF | 0xF0000..=0x10FFFF)
}

/// Interned family names: the symbol stack holds `&'static str`s, and the families a process
/// can add are the system's few fallback fonts, each named once.
fn intern(name: &str) -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(n) = names.iter().find(|n| **n == name) {
        return n;
    }
    let n: &'static str = Box::leak(name.into());
    names.push(n);
    n
}

impl TextSystem {
    #[cfg(target_vendor = "apple")]
    fn load_named_symbol_families(&mut self, query_names: &[&str], register_names: &[&'static str], is_emoji: bool) {
        load_files(&mut self.font_cx.collection, &mut self.font_files, core_text::family_files(query_names));
        for name in present_families(&mut self.font_cx.collection, register_names) {
            self.push_symbol_family(name, is_emoji);
        }
    }

    #[cfg(target_vendor = "apple")]
    fn load_symbol_families(&mut self) {
        self.fallbacks.symbols = true;
        let to_load = if self.fallbacks.slot_fallback {
            &SYMBOL_FAMILIES[SLOT_FALLBACK_FAMILIES.len()..]
        } else {
            SYMBOL_FAMILIES
        };
        self.load_named_symbol_families(to_load, SYMBOL_FAMILIES, false);
    }

    #[cfg(target_vendor = "apple")]
    fn load_emoji_family(&mut self) {
        self.load_named_symbol_families(EMOJI_FAMILIES, EMOJI_FAMILIES, true);
    }

    #[cfg(not(any(windows, target_vendor = "apple")))]
    fn load_symbol_families(&mut self) {
        self.fallbacks.symbols = true;
        self.symbol_families = load_families(&mut self.font_cx.collection, SYMBOL_FONT_FILES, SYMBOL_FAMILIES);
        self.emoji = self.symbol_families.iter().copied().find(|f| f.contains("Emoji"));
        #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
        if self.emoji.is_none()
            && let Some((family, files)) = fontconfig::emoji()
        {
            self.add_fallback_emoji(&family, files);
        }
    }

    /// Loads `files` and registers `family` (from fontconfig's generic `emoji`) at the end of
    /// `symbol_families` as the colour emoji family when none is set yet.
    #[cfg_attr(not(all(unix, not(target_vendor = "apple"), not(target_os = "android"))), allow(dead_code))]
    pub(crate) fn add_fallback_emoji(&mut self, family: &str, files: Vec<std::path::PathBuf>) {
        if self.emoji.is_some() {
            return;
        }
        let added = load_files(&mut self.font_cx.collection, &mut self.font_files, files);
        let emoji_id = self
            .font_cx
            .collection
            .family_id(family)
            .or_else(|| added.iter().copied().find(|&id| family_has_color_table(&mut self.font_cx.collection, id)))
            .or_else(|| added.into_iter().next());
        if let Some(name) = emoji_id.and_then(|id| self.font_cx.collection.family_name(id)).map(intern) {
            self.push_symbol_family(name, true);
        }
    }

    /// Registers the app's non-primary families (followed by any loaded system CJK families) as
    /// fallbacks. When a slot still resolves to no family and no non-primary app family is
    /// registered, includes the app's registered families so a host with no system fonts falls
    /// back to a bundled font instead of measuring as zero. Run where the slots are set.
    pub(crate) fn sync_cjk_fallbacks(&mut self) {
        let collection = &mut self.font_cx.collection;
        let primary = primary_family_ids(collection, &self.families.0, &self.families.1);
        let mut next: Vec<FamilyId> = Vec::new();
        for &id in self.fallbacks.own_families.iter().chain(&self.fallbacks.system_cjk) {
            if !primary.contains(&id) && !next.contains(&id) {
                next.push(id);
            }
        }
        if (!slot_resolves(collection, &self.sans) || !slot_resolves(collection, &self.mono))
            && !self.fallbacks.own_families.iter().any(|id| next.contains(id))
        {
            for &id in &self.fallbacks.own_families {
                if !next.contains(&id) {
                    next.push(id);
                }
            }
        }
        if next != self.fallbacks.cjk_families {
            super::language::register(collection, &next);
            self.fallbacks.cjk_families = next;
        }
    }

    /// When the sans or mono slot names no family the collection has, loads the system font
    /// its text falls back to, once, so a missing family draws in the system's font, not
    /// nothing (`load_fallbacks` loads nothing for Latin text). Run where the slots are set.
    pub(crate) fn load_slot_fallbacks(&mut self) {
        let collection = &mut self.font_cx.collection;
        if slot_resolves(collection, &self.sans) && slot_resolves(collection, &self.mono) {
            return;
        }
        #[cfg(windows)]
        // Windows' fallback for Latin; `asked` is otherwise never given ASCII.
        if self.fallbacks.asked.insert('A') {
            let locale = self.language.as_ref().map(|l| l.to_string());
            self.add_symbol_files(direct_write::fallback_files(&['A'], locale.as_deref()));
        }
        #[cfg(target_vendor = "apple")]
        if !self.fallbacks.slot_fallback && !self.fallbacks.symbols {
            self.fallbacks.slot_fallback = true;
            self.load_named_symbol_families(SLOT_FALLBACK_FAMILIES, SLOT_FALLBACK_FAMILIES, false);
        }
        #[cfg(not(any(windows, target_vendor = "apple")))]
        if !self.fallbacks.symbols {
            self.load_symbol_families();
        }
    }

    /// Adds `name` to `symbol_families`, keeping colour emoji families at the end so a symbol
    /// with a text form keeps it.
    fn push_symbol_family(&mut self, name: &'static str, is_emoji: bool) {
        if is_emoji {
            self.emoji.get_or_insert(name);
        }
        if self.symbol_families.contains(&name) {
            return;
        }
        if is_emoji {
            self.symbol_families.push(name);
        } else {
            let at = self
                .symbol_families
                .iter()
                .position(|f| self.emoji == Some(*f) || f.contains("Emoji"))
                .unwrap_or(self.symbol_families.len());
            self.symbol_families.insert(at, name);
        }
    }

    /// The families in `files` at the end of every family stack, colour emoji last, so a symbol
    /// with a text form keeps it.
    pub(crate) fn add_symbol_files(&mut self, files: Vec<std::path::PathBuf>) {
        for id in load_files(&mut self.font_cx.collection, &mut self.font_files, files) {
            let Some(name) = self.font_cx.collection.family_name(id).map(intern) else { continue };
            // A family's tables are read only until the colour emoji family is known.
            let is_emoji = self.emoji == Some(name)
                || name.contains("Emoji")
                || (self.emoji.is_none() && family_has_color_table(&mut self.font_cx.collection, id));
            self.push_symbol_family(name, is_emoji);
        }
    }

    /// Loads the symbol families now, as a text needing them would (tests check what's installed).
    #[cfg(test)]
    pub(crate) fn load_symbol_fonts(&mut self) {
        let text = "\u{2318}\u{2713}\u{FE0F}\u{1F600}";
        let routes = fallback::routes(text);
        self.load_fallbacks(text, rune_core::tok::TextStyle::new(14.0, 400.0), &crate::layout::span_map::SpanMap::empty(), &routes, None);
    }
}

/// The files of the installed family `name` (any of its names, case aside), found through the
/// platform's own list of fonts: DirectWrite's system collection on Windows, CoreText on macOS
/// and fontconfig on Linux. The lookup runs in a collection of its own, so the app's collection
/// gains only the files of the families text asks for, and its own families still come first.
pub(crate) fn installed_family_files(name: &str) -> Vec<std::path::PathBuf> {
    platform::family_files(&[name])
}

/// Parses a CSS family list, falling back to one literal family name when CSS parsing yields
/// nothing (for hidden family names such as `.SF NS` that are not CSS identifiers).
fn css_or_named(list: &str) -> Vec<FontFamilyName<'_>> {
    let parsed: Vec<FontFamilyName<'_>> = FontFamilyName::parse_css_list(list).flatten().collect();
    if parsed.is_empty() && !list.trim().is_empty() && !list.contains(',') {
        vec![FontFamilyName::named(list)]
    } else {
        parsed
    }
}

/// Parses an explicit emoji family setting: a CSS family list when it contains a comma or outer
/// quotes, or a single literal family name otherwise (so names with spaces, digits, or leading
/// dots such as `"Twemoji 17"` or `".Apple Color Emoji UI"` work without CSS quoting).
pub(crate) fn emoji_family_list(list: &str) -> Vec<FontFamilyName<'_>> {
    let trimmed = list.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.contains(',')
        || (trimmed.len() >= 2
            && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
                || (trimmed.starts_with('\'') && trimmed.ends_with('\''))))
    {
        css_or_named(trimmed)
            .into_iter()
            .filter(|f| matches!(f, FontFamilyName::Named(_)))
            .collect()
    } else {
        vec![FontFamilyName::named(trimmed)]
    }
}

fn load_installed_names<'a>(
    collection: &mut Collection,
    loaded: &mut LoadedFiles,
    tried: &mut std::collections::HashSet<String>,
    families: impl IntoIterator<Item = FontFamilyName<'a>>,
) -> bool {
    let mut added = false;
    for family in families {
        let FontFamilyName::Named(name) = family else { continue };
        if collection.family_id(&name).is_some() || !tried.insert(name.to_lowercase()) {
            continue;
        }
        added |= !load_files(collection, loaded, installed_family_files(&name)).is_empty();
    }
    added
}

/// Loads the installed families the CSS family list `list` names that the collection lacks,
/// each looked up once (`tried`). Generic CSS keywords (`ui-monospace`, `monospace`, ...) are
/// skipped so they trigger no system font lookup at launch.
pub(crate) fn load_installed(collection: &mut Collection, loaded: &mut LoadedFiles, tried: &mut std::collections::HashSet<String>, list: &str) -> bool {
    load_installed_names(collection, loaded, tried, css_or_named(list))
}

/// Loads the installed families named by an explicit emoji family setting (`emoji_family_list`)
/// that the collection lacks, each looked up once (`tried`).
pub(crate) fn load_installed_emoji(collection: &mut Collection, loaded: &mut LoadedFiles, tried: &mut std::collections::HashSet<String>, list: &str) -> bool {
    load_installed_names(collection, loaded, tried, emoji_family_list(list))
}

/// Whether the slot `list` names a family the collection has, or `ui-monospace`, whose
/// families load with the text that asks for them (`family_list` then falls back to sans).
fn slot_resolves(collection: &mut Collection, list: &str) -> bool {
    names_system_mono(list)
        || css_or_named(list).iter().any(|f| matches!(f, FontFamilyName::Named(name) if collection.family_id(name).is_some()))
}

/// `Family::Named`'s families: one literal name, or a CSS list when it has a comma.
pub(crate) fn named_list(list: &str) -> Vec<FontFamilyName<'_>> {
    if list.contains(',') {
        FontFamilyName::parse_css_list(list).flatten().collect()
    } else {
        vec![FontFamilyName::named(list)]
    }
}

/// The families each system font file added to the collection, so a file is loaded once.
pub(crate) type LoadedFiles = HashMap<std::path::PathBuf, Vec<FamilyId>>;

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
/// The font is the one the platform falls back to for `sample` in `locale`, its family whole.
fn register_script_fallbacks(collection: &mut Collection, loaded: &mut LoadedFiles, fonts: &fallback::ScriptFonts, sample: char, locale: Option<&str>) {
    // The whole family, so a medium label takes its Semibold and a heading its Bold.
    let files = platform::fallback_family_files(&[sample], locale);
    let added = load_files(collection, loaded, files.into_iter().take(16));
    if !added.is_empty() {
        // The Arabic comma, semicolon and question mark (and Urdu's full stop) are Common
        // characters: after Latin text they join its run, which looks only at Latin fallbacks.
        if matches!(fonts.script, b"Arab" | b"Hebr") {
            collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*b"Latn"), None), added.iter().copied());
        }
        collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*fonts.script), None), added.into_iter());
    }
}

/// Font files whose lowercase name, without spaces and dots, contains one of `fragments`,
/// ordered by the first fragment they match, then by path. The platform's font directories are
/// walked once per process.
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub fn system_font_files(_: &[&str]) -> Vec<std::path::PathBuf> { Vec::new() }

#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub fn system_font_files(fragments: &[&str]) -> Vec<std::path::PathBuf> {
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
        // Fonts macOS installs as assets, PingFang among them since macOS 11 (Hex #820).
        if let Ok(assets) = std::fs::read_dir("/System/Library/AssetsV2") {
            dirs.extend(
                assets
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with("com_apple_MobileAsset_Font"))
                    .map(|entry| entry.path()),
            );
        }
    } else if cfg!(target_os = "ios") {
        dirs.push("/System/Library/Fonts".into());
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
        let data_dirs = rune_core::env::var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        dirs.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| std::path::Path::new(d).join("fonts")));
    }
    let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
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
            if let Some(name) = font_file_name(&path) && seen.insert(path.clone()) {
                found.push((name, path));
            }
        }
    }
    // Fontconfig knows fonts outside these directories, such as NixOS's store paths.
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
    if let Ok(out) = std::process::Command::new("fc-list").arg("--format=%{file}\n").output() {
        for path in String::from_utf8_lossy(&out.stdout).lines().map(std::path::PathBuf::from) {
            if let Some(name) = font_file_name(&path) && seen.insert(path.clone()) {
                found.push((name, path));
            }
        }
    }
    found
}

const COLOR_TABLES: [[u8; 4]; 3] = [*b"COLR", *b"CBDT", *b"sbix"];

/// Whether the SFNT table directory at the current position of `r` has a colour glyph table
/// (`COLR`, `CBDT` or `sbix`, the formats `glyph_emit` draws in colour).
fn sfnt_has_color_table(r: &mut impl std::io::Read) -> bool {
    let mut header = [0u8; 12];
    if r.read_exact(&mut header).is_err() {
        return false;
    }
    if !matches!(&header[..4], [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1") {
        return false;
    }
    let num_tables = u16::from_be_bytes([header[4], header[5]]) as usize;
    if num_tables == 0 || num_tables > 1024 {
        return false;
    }
    let mut records = vec![0u8; num_tables * 16];
    if r.read_exact(&mut records).is_err() {
        return false;
    }
    records.chunks_exact(16).any(|rec| COLOR_TABLES.iter().any(|tag| &rec[..4] == tag))
}

/// Whether face `index` (or any face when `None`) of an SFNT or TrueType collection stream has a
/// colour glyph table (`COLR`, `CBDT` or `sbix`). Reads only the table directory header(s).
pub(crate) fn reader_has_color_table(r: &mut (impl std::io::Read + std::io::Seek), index: Option<u32>) -> bool {
    use std::io::SeekFrom;
    let mut magic = [0u8; 4];
    if r.read_exact(&mut magic).is_err() {
        return false;
    }
    if &magic == b"ttcf" {
        let mut ttc = [0u8; 8];
        if r.read_exact(&mut ttc).is_err() {
            return false;
        }
        let num_fonts = u32::from_be_bytes([ttc[4], ttc[5], ttc[6], ttc[7]]);
        let range = match index {
            Some(i) if i < num_fonts => i..i + 1,
            Some(_) => return false,
            None => 0..num_fonts.min(64),
        };
        for i in range {
            let mut off = [0u8; 4];
            if r.seek(SeekFrom::Start(12 + u64::from(i) * 4)).is_err() || r.read_exact(&mut off).is_err() {
                return false;
            }
            if r.seek(SeekFrom::Start(u64::from(u32::from_be_bytes(off)))).is_ok() && sfnt_has_color_table(r) {
                return true;
            }
        }
        return false;
    }
    index.unwrap_or(0) == 0 && r.seek(SeekFrom::Start(0)).is_ok() && sfnt_has_color_table(r)
}

/// Whether family `id`'s first face has a colour glyph table (`COLR`, `CBDT` or `sbix`).
fn family_has_color_table(collection: &mut Collection, id: FamilyId) -> bool {
    let Some(family) = collection.family(id) else { return false };
    family.fonts().first().is_some_and(|font| match &font.source().kind {
        parley::fontique::SourceKind::Memory(blob) => {
            reader_has_color_table(&mut std::io::Cursor::new(blob.as_ref()), Some(font.index()))
        }
        #[cfg(not(target_arch = "wasm32"))]
        parley::fontique::SourceKind::Path(path) => {
            std::fs::File::open(path).ok().is_some_and(|mut f| reader_has_color_table(&mut f, Some(font.index())))
        }
        #[cfg(target_arch = "wasm32")]
        parley::fontique::SourceKind::Path(_) => false,
    })
}

/// First family in `collection`'s `GenericFamily::Emoji` list whose font data has a colour glyph
/// table (`COLR`, `CBDT` or `sbix`).
#[cfg_attr(not(all(unix, not(target_vendor = "apple"), not(target_os = "android"))), allow(dead_code))]
pub(crate) fn first_color_emoji_family(collection: &mut Collection) -> Option<(FamilyId, String)> {
    let ids: Vec<FamilyId> = collection.generic_families(parley::GenericFamily::Emoji).collect();
    for id in ids {
        if family_has_color_table(collection, id)
            && let Some(name) = collection.family_name(id)
        {
            return Some((id, name.to_owned()));
        }
    }
    None
}

/// `path`'s file name, lowercase without spaces and dots, when it's a font file Rune loads.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn font_file_name(path: &std::path::Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    [".ttf", ".otf", ".ttc", ".otc"].iter().any(|ext| name.ends_with(ext)).then(|| name.replace([' ', '.'], ""))
}

#[cfg(test)]
mod emoji_family_tests;
#[cfg(test)]
mod tests;
