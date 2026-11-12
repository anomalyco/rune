//! The system's fonts: monospace and symbol families, CJK and script fallbacks, and the font
//! directories they're found in.
//!
//! None is touched at launch: a system font loads the first time a text has a character the
//! app's own fonts lack (`TextSystem::load_fallbacks`). The platform's own fallback names the
//! font and its files, as browsers ask it: DirectWrite on Windows (`direct_write`), CoreText on
//! Apple platforms (`core_text`), fontconfig in process elsewhere (`fontconfig`). No font
//! folder is walked and no process is started.

use super::*;

#[cfg(target_vendor = "apple")]
mod core_text;
#[cfg(windows)]
mod direct_write;
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
mod fontconfig;
#[cfg(target_vendor = "apple")]
use core_text as platform;
#[cfg(windows)]
use direct_write as platform;
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
use fontconfig as platform;
/// Android and the web: no system fonts to ask.
#[cfg(any(target_os = "android", target_arch = "wasm32"))]
mod platform {
    pub(super) fn family_files(_: &[&str]) -> Vec<std::path::PathBuf> { Vec::new() }
    pub(super) fn fallback_family_files(_: &[char], _: Option<&str>) -> Vec<std::path::PathBuf> { Vec::new() }
}

// Common-script symbols can inherit the script of adjacent letters. Put these families at
// the end of the requested family list, rather than registering only a Zyyy fallback.
// On macOS the system font comes first: its keyboard symbols (⌘⇧⏎⌫⎋⌥⌃) sit on the baseline at
// cap height and follow the requested weight through its `wght` axis. Apple Symbols draws
// them light, small and (⇧) below the baseline next to app fonts' capitals.
// The colour emoji family comes last, so symbols with a text form keep it.
#[cfg(target_vendor = "apple")]
pub(super) const SYMBOL_FAMILIES: &[&str] = &["System Font", ".SF NS", "Apple Symbols", ".SF NS Symbols", "Menlo", "Apple Color Emoji"];
#[cfg(target_vendor = "apple")]
// SF italic rides along so `Family::Named("System Font").italic()` gets the real face.
pub(super) const SYMBOL_FONT_FILES: &[&str] = &["sfnsttf", "sfnsitalic", "applesymbols", "sfnssymbols", "menlo", "applecoloremoji"];
// Windows loads its symbol fonts as its own fallback names them (`direct_write::fallback_files`),
// and this colour emoji family for a `U+FE0F` sequence.
#[cfg(windows)]
pub(super) const SYMBOL_FAMILIES: &[&str] = &["Segoe UI Emoji"];
// DejaVu Sans lacks ⎋ (U+238B), which DejaVu Sans Mono has; `dejavusans` loads both files.
#[cfg(not(any(target_vendor = "apple", windows)))]
pub(super) const SYMBOL_FAMILIES: &[&str] =
    &["DejaVu Sans", "Noto Sans Symbols2", "Noto Sans Symbols", "DejaVu Sans Mono", "Noto Color Emoji"];
#[cfg(not(any(target_vendor = "apple", windows)))]
pub(super) const SYMBOL_FONT_FILES: &[&str] = &["dejavusans", "notosanssymbols", "notocoloremoji"];

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
pub(super) fn load_system_mono(collection: &mut Collection, loaded: &mut LoadedFiles) -> Vec<String> {
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
pub(super) fn names_system_mono(list: &str) -> bool {
    FontFamilyName::parse_css_list(list).flatten().any(|f| f == FontFamilyName::Generic(parley::GenericFamily::UiMonospace))
}

/// `list` parsed, with `ui-monospace` replaced by the installed `Family::SystemMono` families.
/// When that leaves no named family (no monospace font installed, or the web), `sans` follows,
/// so text still finds a font.
pub(super) fn family_list<'a>(list: &'a str, system_mono: &'a [String], sans: &'a str) -> Vec<FontFamilyName<'a>> {
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
pub(super) fn load_families(collection: &mut Collection, files: &[&str], names: &[&'static str]) -> Vec<&'static str> {
    collection.load_fonts_from_paths(installed_files(names, files));
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
pub(super) const CJK_FONT_FILES: [&str; 30] = [
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

/// Makes text in scripts the app's fonts don't cover render instead of drawing tofu: the
/// families the app registered beyond its sans and mono (`AppSpec::fonts`), already in memory.
/// The system's CJK fonts follow them the first time a text has CJK (`register_system_cjk`).
pub(super) fn register_cjk_fallbacks(collection: &mut Collection, registered: &[FamilyId], families: (&str, &str)) -> Vec<FamilyId> {
    let primary = [collection.family_id(families.0), collection.family_id(families.1)];
    let mut fallbacks: Vec<FamilyId> = Vec::new();
    for id in registered {
        if !primary.contains(&Some(*id)) && !fallbacks.contains(id) {
            fallbacks.push(*id);
        }
    }
    if !fallbacks.is_empty() {
        super::language::register(collection, &fallbacks);
    }
    fallbacks
}

/// The CJK fonts in `files` as fallbacks after the app's, memory-mapped, so a 20 MB CJK
/// collection costs a mapping, not a read. Every CJK fallback so far (`all`) is ordered again
/// for each language, so a Japanese font loaded after a Chinese one still comes first for
/// Japanese text.
fn register_system_cjk(collection: &mut Collection, loaded: &mut LoadedFiles, files: Vec<std::path::PathBuf>, families: (&str, &str), all: &mut Vec<FamilyId>) {
    let primary = [collection.family_id(families.0), collection.family_id(families.1)];
    let before = all.len();
    // Only files new to the collection: one already loaded is a fallback already.
    let new: Vec<std::path::PathBuf> = files.into_iter().filter(|f| !loaded.contains_key(f)).take(16).collect();
    for id in load_files(collection, loaded, new) {
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
pub(super) struct Fallbacks {
    /// The app's own font files, whose characters need no system font.
    own: Vec<&'static [u8]>,
    /// Characters already looked up (Windows: each asked of DirectWrite once).
    asked: std::collections::HashSet<char>,
    /// The locales the system's CJK fonts were looked up for (Windows), or whether they were
    /// loaded at all (elsewhere: `Some(None)`).
    cjk: Vec<Option<String>>,
    /// The symbol families were loaded (elsewhere than Windows, where each comes on its own).
    symbols: bool,
    /// Every CJK fallback family, the app's then the system's in the order they loaded.
    cjk_families: Vec<FamilyId>,
    /// Segoe UI Symbol is loaded, for symbols with a text form (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    text_symbols: bool,
}

impl Fallbacks {
    pub(super) fn new(own: &[&'static [u8]], cjk_families: Vec<FamilyId>) -> Self {
        Fallbacks { own: own.to_vec(), cjk_families, ..Default::default() }
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
    /// Loads the system fonts `text` needs and the app's fonts lack, each the first time:
    /// a script's fallback (`fallback::SCRIPT_FONTS`), CJK fonts, math fonts, the colour emoji
    /// family, and (Windows) the font Windows falls back to for any other character; elsewhere
    /// the symbol families. Latin text the app's fonts cover loads nothing.
    pub(super) fn load_fallbacks(&mut self, text: &str, language: Option<parley::fontique::Language>) {
        if text.is_ascii() {
            return;
        }
        let locale = language.as_ref().map(|l| l.to_string());
        let own: Vec<skrifa::charmap::Charmap<'static>> = self.fallbacks.own.iter().filter_map(|data| skrifa::FontRef::from_index(data, 0).ok()).map(|f| skrifa::MetadataProvider::charmap(&f)).collect();
        let covered = |ch: char| own.iter().any(|map| map.map(ch).is_some());
        let (mut math, mut emoji, mut scripts) = (false, false, Vec::new());
        let (mut cjk, mut cjk_more, mut missing): (Vec<char>, Vec<char>, Vec<char>) = (Vec::new(), Vec::new(), Vec::new());
        let cjk_done = self.fallbacks.cjk.contains(&locale);
        for ch in text.chars() {
            if let Some(i) = fallback::SCRIPT_FONTS.iter().position(|fonts| fonts.covers(ch)) {
                if self.scripts & (1 << i) == 0 {
                    self.scripts |= 1 << i;
                    scripts.push((i, ch));
                }
                continue;
            }
            if is_cjk(ch) {
                if covered(ch) { continue; }
                // The language's own CJK font, the first time a text in it has CJK.
                if !cjk_done && !cjk.contains(&ch) && cjk.len() < 64 {
                    cjk.push(ch);
                } else if !self.fallbacks.asked.contains(&ch) && !cjk_more.contains(&ch) {
                    // A character the CJK fonts loaded so far may lack (Simplified Chinese in a
                    // Japanese font): the platform names a font that has it, once per character.
                    cjk_more.push(ch);
                }
                continue;
            }
            // Windows' fallback covers symbol blocks; only math alphanumerics want one math font.
            math |= self.math.is_none() && if cfg!(windows) { fallback::is_math_alphanumeric(ch) } else { fallback::needs_symbols(ch) };
            // An emoji by default (✅ ⚡) wants the colour emoji family as `U+FE0F` does.
            emoji |= (ch == '\u{FE0F}' || fallback::is_emoji(ch.encode_utf8(&mut [0; 4]))) && self.emoji.is_none() && !self.fallbacks.asked.contains(&'\u{FE0F}');
            if !needs_no_font(ch) && !self.fallbacks.asked.contains(&ch) && !missing.contains(&ch) && !covered(ch) {
                missing.push(ch);
            }
        }
        if scripts.is_empty() && !math && cjk.is_empty() && cjk_more.is_empty() && !emoji && missing.is_empty() {
            return;
        }
        // The first system font a launch loads, for its trace (`marks`).
        crate::marks::mark("text.fallback.begin");
        for (i, ch) in scripts {
            register_script_fallbacks(&mut self.font_cx.collection, &mut self.font_files, &fallback::SCRIPT_FONTS[i], ch, locale.as_deref());
            if self.mono_scripts.is_none() && matches!(fallback::SCRIPT_FONTS[i].script, b"Arab" | b"Hebr") {
                self.mono_scripts = Some(load_families(&mut self.font_cx.collection, fallback::MONO_SCRIPT_FILES, fallback::MONO_SCRIPT_FAMILIES));
            }
        }
        if math {
            self.math = Some(load_families(&mut self.font_cx.collection, fallback::MATH_FONT_FILES, fallback::MATH_FAMILIES));
        }
        if !cjk.is_empty() || !cjk_more.is_empty() {
            self.fallbacks.asked.extend(cjk.iter().chain(&cjk_more).copied());
            cjk.extend(cjk_more);
            let files = platform::fallback_family_files(&cjk, locale.as_deref());
            if !cjk_done { self.fallbacks.cjk.push(locale.clone()); }
            let families = (self.families.0.clone(), self.families.1.clone());
            register_system_cjk(&mut self.font_cx.collection, &mut self.font_files, files, (&families.0, &families.1), &mut self.fallbacks.cjk_families);
        }
        if emoji {
            self.fallbacks.asked.insert('\u{FE0F}');
        }
        #[cfg(windows)]
        {
            if emoji {
                self.add_symbol_files(direct_write::family_files(SYMBOL_FAMILIES));
            }
            if !missing.is_empty() {
                // A symbol that isn't an emoji by default (☺ ✓ ♥) draws in Segoe UI Symbol's
                // text form, as Chromium draws it, not in the colour emoji Windows maps it to.
                if !self.fallbacks.text_symbols && missing.iter().any(|&c| fallback::needs_symbols(c) && !fallback::is_emoji(c.encode_utf8(&mut [0; 4]))) {
                    self.fallbacks.text_symbols = true;
                    self.add_symbol_files(direct_write::family_files(&["Segoe UI Symbol"]));
                }
                self.fallbacks.asked.extend(missing.iter().copied());
                self.add_symbol_files(direct_write::fallback_family_files(&missing, locale.as_deref()));
            }
        }
        #[cfg(not(windows))]
        {
            if (emoji || !missing.is_empty()) && !self.fallbacks.symbols {
                self.load_symbol_families();
            }
            // What the symbol families lack (Ethiopic, Khmer, Tibetan...): the platform's own
            // fallback, once per character.
            if !missing.is_empty() {
                self.fallbacks.asked.extend(missing.iter().copied());
                self.add_symbol_files(platform::fallback_family_files(&missing, locale.as_deref()));
            }
        }
        crate::marks::mark("text.fallback.loaded");
    }

    #[cfg(not(windows))]
    fn load_symbol_families(&mut self) {
        self.fallbacks.symbols = true;
        self.symbol_families = load_families(&mut self.font_cx.collection, SYMBOL_FONT_FILES, SYMBOL_FAMILIES);
        self.emoji = self.symbol_families.iter().copied().find(|f| f.contains("Emoji"));
    }

    /// When the sans or mono slot names no family the collection has, loads the system font
    /// its text falls back to, once, so a missing family draws in the system's font, not
    /// nothing (`load_fallbacks` loads nothing for Latin text). Run where the slots are set.
    pub(super) fn load_slot_fallbacks(&mut self) {
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
        #[cfg(not(windows))]
        if !self.fallbacks.symbols {
            self.load_symbol_families();
        }
    }

    /// The families in `files` at the end of every family stack, colour emoji last, so a symbol
    /// with a text form keeps it.
    fn add_symbol_files(&mut self, files: Vec<std::path::PathBuf>) {
        for id in load_files(&mut self.font_cx.collection, &mut self.font_files, files) {
            let Some(name) = self.font_cx.collection.family_name(id).map(intern) else { continue };
            if self.symbol_families.contains(&name) {
                continue;
            }
            if name.contains("Emoji") {
                self.symbol_families.push(name);
                self.emoji.get_or_insert(name);
            } else {
                let at = self.symbol_families.iter().position(|f| f.contains("Emoji")).unwrap_or(self.symbol_families.len());
                self.symbol_families.insert(at, name);
            }
        }
    }

    /// Loads the symbol families now, as a text needing them would (tests check what's installed).
    #[cfg(test)]
    pub(super) fn load_symbol_fonts(&mut self) {
        self.load_fallbacks("\u{2318}\u{2713}\u{FE0F}\u{1F600}", None);
    }
}

/// The files of the installed family `name` (any of its names, case aside), found through the
/// platform's own list of fonts: DirectWrite's system collection on Windows, CoreText on macOS
/// and fontconfig on Linux. The lookup runs in a collection of its own, so the app's collection
/// gains only the files of the families text asks for, and its own families still come first.
pub(super) fn installed_family_files(name: &str) -> Vec<std::path::PathBuf> {
    platform::family_files(&[name])
}

/// Loads the installed families the CSS family list `list` names that the collection lacks,
/// each looked up once (`tried`).
pub(super) fn load_installed(collection: &mut Collection, loaded: &mut LoadedFiles, tried: &mut std::collections::HashSet<String>, list: &str) -> bool {
    let mut added = false;
    for family in named_list(list) {
        let FontFamilyName::Named(name) = family else { continue };
        if collection.family_id(&name).is_some() || !tried.insert(name.to_lowercase()) {
            continue;
        }
        added |= !load_files(collection, loaded, installed_family_files(&name)).is_empty();
    }
    added
}

/// Whether the slot `list` names a family the collection has, or `ui-monospace`, whose
/// families load with the text that asks for them (`family_list` then falls back to sans).
fn slot_resolves(collection: &mut Collection, list: &str) -> bool {
    names_system_mono(list)
        || named_list(list).iter().any(|f| matches!(f, FontFamilyName::Named(name) if collection.family_id(name).is_some()))
}

/// `Family::Named`'s families: one literal name, or a CSS list when it has a comma.
pub(super) fn named_list(list: &str) -> Vec<FontFamilyName<'_>> {
    if list.contains(',') {
        FontFamilyName::parse_css_list(list).flatten().collect()
    } else {
        vec![FontFamilyName::named(list)]
    }
}

/// The families each system font file added to the collection, so a file is loaded once.
pub(super) type LoadedFiles = HashMap<std::path::PathBuf, Vec<FamilyId>>;

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
pub(super) fn system_font_files(_: &[&str]) -> Vec<std::path::PathBuf> { Vec::new() }

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn system_font_files(fragments: &[&str]) -> Vec<std::path::PathBuf> {
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
        let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
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

/// `path`'s file name, lowercase without spaces and dots, when it's a font file Rune loads.
#[cfg(not(target_arch = "wasm32"))]
fn font_file_name(path: &std::path::Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    [".ttf", ".otf", ".ttc", ".otc"].iter().any(|ext| name.ends_with(ext)).then(|| name.replace([' ', '.'], ""))
}
