//! Android: system font files found through `/system/etc/font_fallback.xml` (Android 15+) and
//! `/system/etc/fonts.xml` (Android 5+), parsed once lazily with a pure-`std` XML reader so no
//! font folder is walked and no extra XML dependency is required. Respects
//! `RUNE_SYSTEM_FONTS=off` / `set_system_fonts(false)`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::fonts::fallback;

mod xml;

use xml::parse_familyset;

use super::installed::{RawFamily, push_weight_range, reader_sfnt_meta};

const SYSTEM_CONFIG_PATHS: &[&str] = &["/system/etc/font_fallback.xml", "/system/etc/fonts.xml"];
const SYSTEM_FONT_DIRS: &[&str] = &["/system/fonts", "/product/fonts", "/system_ext/fonts"];
const MAX_XML_BYTES: u64 = 4 * 1024 * 1024;

static CONFIG: OnceLock<FontConfig> = OnceLock::new();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FamilyVariant {
    #[default]
    Default = 0,
    Compact = 1,
    Elegant = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct XmlFont {
    pub(super) path: PathBuf,
    pub(super) file_name: String,
    pub(super) base_stem: String,
    pub(super) weight: u16,
    pub(super) italic: bool,
    pub(super) index: u32,
    pub(super) fallback_for: Option<String>,
    pub(super) wght_axis_values: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct XmlFamily {
    pub(super) name: Option<String>,
    pub(super) langs: Vec<LangTag>,
    pub(super) variant: FamilyVariant,
    pub(super) fonts: Vec<XmlFont>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct XmlAlias {
    pub(super) name: String,
    pub(super) to: String,
    pub(super) weight: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LangTag {
    pub(super) lang: String,
    pub(super) script: Option<String>,
    pub(super) region: Option<String>,
}

impl LangTag {
    pub(super) fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let mut parts = trimmed.split(['-', '_']).filter(|p| !p.is_empty());
        let lang = parts.next()?.to_ascii_lowercase();
        let mut script = None;
        let mut region = None;
        for part in parts {
            let lower = part.to_ascii_lowercase();
            if script.is_none() && lower.len() == 4 && lower.bytes().all(|b| b.is_ascii_alphabetic()) {
                script = Some(lower);
            } else if region.is_none()
                && ((lower.len() == 2 && lower.bytes().all(|b| b.is_ascii_alphabetic()))
                    || (lower.len() == 3 && lower.bytes().all(|b| b.is_ascii_digit())))
            {
                region = Some(lower);
            }
        }
        Some(Self { lang, script, region })
    }

    fn effective_script(&self) -> Option<&str> {
        if let Some(script) = self.script.as_deref() {
            return Some(script);
        }
        default_script_for_lang(&self.lang)
    }
}

/// Parsed representation of Android's `font_fallback.xml` / `fonts.xml`.
#[derive(Debug, Default)]
pub(super) struct FontConfig {
    pub(super) families: Vec<XmlFamily>,
    pub(super) aliases: Vec<XmlAlias>,
    /// Named family lookup table: lowercase XML `name` -> indices into `families`.
    named_index: HashMap<String, Vec<usize>>,
    /// Alias lookup table: lowercase alias `name` -> index into `aliases`.
    alias_index: HashMap<String, usize>,
    /// Lazily populated SFNT family name -> paths map filled by `installed_families()`.
    sfnt_index: OnceLock<HashMap<String, Vec<PathBuf>>>,
}

impl FontConfig {
    /// Loads the first readable, non-empty config from `config_paths`, resolving relative font
    /// filenames against `font_dirs`.
    pub(super) fn load_from_paths(config_paths: &[&Path], font_dirs: &[&Path]) -> Self {
        for config_path in config_paths {
            let Ok(meta) = std::fs::metadata(config_path) else {
                continue;
            };
            if !meta.is_file() || meta.len() > MAX_XML_BYTES {
                continue;
            }
            let Ok(xml) = std::fs::read_to_string(config_path) else {
                continue;
            };
            let parsed = Self::parse(&xml, |file| resolve_font_in_dirs(file, font_dirs));
            if !parsed.families.is_empty() {
                return parsed;
            }
        }
        Self::default()
    }

    /// Parses `xml` as an Android `<familyset>` document, resolving each `<font>` filename via
    /// `resolve_file` and skipping entries whose file does not resolve.
    pub(super) fn parse(xml: &str, resolve_file: impl FnMut(&str) -> Option<PathBuf>) -> Self {
        let parsed = parse_familyset(xml, resolve_file);
        let mut named_index: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, fam) in parsed.families.iter().enumerate() {
            if let Some(name) = fam.name.as_deref() {
                named_index.entry(name.to_lowercase()).or_default().push(i);
            }
        }
        let mut alias_index: HashMap<String, usize> = HashMap::new();
        for (i, alias) in parsed.aliases.iter().enumerate() {
            alias_index.entry(alias.name.to_lowercase()).or_insert(i);
        }
        Self {
            families: parsed.families,
            aliases: parsed.aliases,
            named_index,
            alias_index,
            sfnt_index: OnceLock::new(),
        }
    }

    /// Resolves an alias chain `name -> ... -> target`, returning the final target lowercase name
    /// and the first weight filter along the chain (with a cycle guard).
    fn resolve_alias(&self, lower_name: &str) -> Option<(String, Option<u16>)> {
        let mut cur = lower_name.to_owned();
        let mut weight = None;
        let mut seen = HashSet::new();
        let mut resolved = false;
        for _ in 0..16 {
            if !seen.insert(cur.clone()) {
                return None;
            }
            let Some(&idx) = self.alias_index.get(&cur) else {
                break;
            };
            let alias = &self.aliases[idx];
            if weight.is_none() {
                weight = alias.weight;
            }
            cur = alias.to.to_lowercase();
            resolved = true;
        }
        resolved.then_some((cur, weight))
    }

    fn push_named_family(&self, lower_name: &str, weight_filter: Option<u16>, out: &mut Vec<PathBuf>) -> bool {
        let Some(indices) = self.named_index.get(lower_name) else {
            return false;
        };
        let mut added = false;
        for &idx in indices {
            let fam = &self.families[idx];
            if let Some(target_w) = weight_filter {
                let mut matched_weight = false;
                for font in &fam.fonts {
                    if font.weight == target_w || font.wght_axis_values.contains(&target_w) {
                        push_unique(out, &font.path);
                        matched_weight = true;
                        added = true;
                    }
                }
                if matched_weight {
                    continue;
                }
            }
            for font in &fam.fonts {
                push_unique(out, &font.path);
                added = true;
            }
        }
        added
    }

    /// Files for the requested family names (matching XML `<family name>`, `<alias>`, exact
    /// filename, normalized font base stem, or cached SFNT family names).
    pub(super) fn family_files(&self, names: &[&str]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for raw_name in names {
            let name = raw_name.trim();
            if name.is_empty() {
                continue;
            }
            let lower = name.to_lowercase();

            // 1. Direct `<family name="...">` match.
            if self.push_named_family(&lower, None, &mut out) {
                continue;
            }

            // 2. `<alias name="..." to="..." weight="...">` match.
            if let Some((target, weight)) = self.resolve_alias(&lower)
                && self.push_named_family(&target, weight, &mut out)
            {
                continue;
            }

            // 3. Exact file name (e.g. `Roboto-Regular.ttf`) or normalized base stem (e.g.
            //    `Noto Color Emoji` -> `notocoloremoji`, `Droid Sans Mono` -> `droidsansmono`).
            let norm = normalize_query(name);
            let mut matched = false;
            for fam in &self.families {
                for font in &fam.fonts {
                    if font.file_name.eq_ignore_ascii_case(name)
                        || (!norm.is_empty() && font.base_stem == norm)
                    {
                        push_unique(&mut out, &font.path);
                        matched = true;
                    }
                }
            }
            if matched {
                continue;
            }

            // 4. Cached SFNT family name map (populated when `installed_families()` runs).
            if let Some(sfnt) = self.sfnt_index.get()
                && let Some(paths) = sfnt.get(&lower)
            {
                for path in paths {
                    push_unique(&mut out, path);
                }
            }
        }
        out
    }

    /// Files for Android's configured `monospace` family.
    pub(super) fn monospace_files(&self) -> Vec<PathBuf> {
        self.family_files(&["monospace"])
    }

    /// Files for Android's colour emoji fallback (`lang="und-Zsye"` or emoji-named fallback fonts).
    pub(super) fn emoji_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for fam in &self.families {
            let is_zsye = fam
                .langs
                .iter()
                .any(|t| t.script.as_deref() == Some("zsye"));
            if is_zsye {
                push_sans_fallback_fonts(fam, &mut out);
            }
        }
        if out.is_empty() {
            for fam in &self.families {
                for font in &fam.fonts {
                    if font.base_stem.contains("emoji") {
                        push_unique(&mut out, &font.path);
                    }
                }
            }
        }
        out
    }

    /// Fallback font files for `chars` in `locale`, honouring script tags, CJK language order,
    /// `compact` vs `elegant` variants, and excluding `fallbackFor="serif"` faces from default
    /// sans fallbacks.
    pub(super) fn fallback_family_files(&self, chars: &[char], locale: Option<&str>) -> Vec<PathBuf> {
        if chars.is_empty() || self.families.is_empty() {
            return Vec::new();
        }
        let locale_tag = locale.and_then(LangTag::parse);
        let scripts = script_tags(chars);
        let mut needs_emoji = false;
        let mut needs_math = false;
        let mut needs_symbols = false;
        for &ch in chars {
            if fallback::is_emoji(ch.encode_utf8(&mut [0; 4])) || fallback::is_pictographic(ch) {
                needs_emoji = true;
            } else if fallback::is_math_alphanumeric(ch) {
                needs_math = true;
            } else if fallback::needs_symbols(ch)
                || (!ch.is_ascii() && char_script_is_common_or_inherited(ch))
            {
                needs_symbols = true;
            }
        }

        let mut out = Vec::new();
        for script in &scripts {
            match script {
                b"Hani" => self.push_han_fallbacks(locale_tag.as_ref(), &mut out),
                b"Hira" | b"Kana" => self.push_cjk_bucket(CjkBucket::Jpan, &mut out),
                b"Hang" => self.push_cjk_bucket(CjkBucket::Kore, &mut out),
                b"Bopo" => {
                    let before = out.len();
                    self.push_script_fallbacks(b"Bopo", locale_tag.as_ref(), &mut out);
                    if out.len() == before {
                        self.push_cjk_bucket(CjkBucket::Hant, &mut out);
                    }
                }
                _ => {
                    let before = out.len();
                    self.push_script_fallbacks(script, locale_tag.as_ref(), &mut out);
                    if out.len() == before && matches!(script, b"Latn" | b"Cyrl" | b"Grek") {
                        self.push_named_family("sans-serif", None, &mut out);
                    }
                }
            }
        }

        if needs_math {
            self.push_tagged_or_stem("zmth", &["notosansmath", "math"], &mut out);
        }
        if needs_symbols {
            self.push_tagged_or_stem("zsym", &["notosanssymbols", "notosanssymbols2"], &mut out);
        }
        if needs_emoji {
            for path in self.emoji_files() {
                push_unique(&mut out, &path);
            }
        }

        if out.is_empty() {
            // Last-resort unnamed, untagged fallback families (e.g. legacy `DroidSansFallback.ttf`).
            for fam in &self.families {
                if fam.name.is_none() && fam.langs.is_empty() {
                    for font in sans_fallback_fonts(fam) {
                        if !needs_emoji && font.base_stem.contains("emoji") {
                            continue;
                        }
                        push_unique(&mut out, &font.path);
                    }
                }
            }
        }
        out
    }

    fn push_han_fallbacks(&self, locale: Option<&LangTag>, out: &mut Vec<PathBuf>) {
        let order: &[CjkBucket] = match locale {
            Some(l) if l.lang == "ja" || l.script.as_deref() == Some("jpan") => {
                &[CjkBucket::Jpan, CjkBucket::Hans, CjkBucket::Hant, CjkBucket::Kore]
            }
            Some(l) if l.lang == "ko" || l.script.as_deref() == Some("kore") => {
                &[CjkBucket::Kore, CjkBucket::Hans, CjkBucket::Hant, CjkBucket::Jpan]
            }
            Some(l)
                if l.lang == "zh"
                    && matches!(l.region.as_deref(), Some("hk" | "mo")) =>
            {
                &[
                    CjkBucket::HantHk,
                    CjkBucket::Hant,
                    CjkBucket::Hans,
                    CjkBucket::Jpan,
                    CjkBucket::Kore,
                ]
            }
            Some(l)
                if l.lang == "zh"
                    && (l.script.as_deref() == Some("hant")
                        || l.region.as_deref() == Some("tw")) =>
            {
                &[CjkBucket::Hant, CjkBucket::Hans, CjkBucket::Jpan, CjkBucket::Kore]
            }
            _ => &[CjkBucket::Hans, CjkBucket::Hant, CjkBucket::Jpan, CjkBucket::Kore],
        };
        for &bucket in order {
            self.push_cjk_bucket(bucket, out);
        }
    }

    fn push_cjk_bucket(&self, bucket: CjkBucket, out: &mut Vec<PathBuf>) {
        let mut matches: Vec<(FamilyVariant, usize)> = self
            .families
            .iter()
            .enumerate()
            .filter(|(_, fam)| fam.langs.iter().any(|tag| tag_matches_cjk(tag, bucket)))
            .map(|(i, fam)| (fam.variant, i))
            .collect();
        matches.sort_by_key(|&(variant, i)| (variant, i));
        for (_, idx) in matches {
            push_sans_fallback_fonts(&self.families[idx], out);
        }
    }

    fn push_script_fallbacks(&self, script: &[u8; 4], locale: Option<&LangTag>, out: &mut Vec<PathBuf>) {
        let script_lower = std::str::from_utf8(script).unwrap_or_default().to_ascii_lowercase();
        let locale_lang = locale.map(|l| l.lang.as_str()).filter(|l| *l != "und");

        let mut ranked: Vec<(u8, FamilyVariant, usize)> = Vec::new();
        for (i, fam) in self.families.iter().enumerate() {
            if fam.langs.is_empty() {
                continue;
            }
            let mut best_tier: Option<u8> = None;
            for tag in &fam.langs {
                let tag_script = tag.effective_script();
                if tag_script != Some(script_lower.as_str()) {
                    continue;
                }
                let tier = if let Some(loc_lang) = locale_lang
                    && tag.lang == loc_lang
                {
                    0u8
                } else if tag.lang == "und" || is_generic_lang_for_script(&tag.lang, &script_lower) {
                    1u8
                } else {
                    2u8
                };
                best_tier = Some(best_tier.map_or(tier, |prev| prev.min(tier)));
            }
            if let Some(tier) = best_tier {
                ranked.push((tier, fam.variant, i));
            }
        }
        ranked.sort_by_key(|&(tier, variant, i)| (tier, variant, i));
        let has_primary = ranked.iter().any(|&(tier, _, _)| tier <= 1);
        for (tier, _, idx) in ranked {
            if has_primary && tier > 1 {
                break;
            }
            push_sans_fallback_fonts(&self.families[idx], out);
        }
    }

    fn push_tagged_or_stem(&self, script_lower: &str, stems: &[&str], out: &mut Vec<PathBuf>) {
        let before = out.len();
        for fam in &self.families {
            if fam.langs.iter().any(|t| t.script.as_deref() == Some(script_lower)) {
                push_sans_fallback_fonts(fam, out);
            }
        }
        if out.len() == before {
            for fam in &self.families {
                for font in sans_fallback_fonts(fam) {
                    if stems.iter().any(|s| font.base_stem == *s) {
                        push_unique(out, &font.path);
                    }
                }
            }
        }
    }

    /// Reads lightweight SFNT headers for every named family's font files so every returned
    /// `InstalledFamily::name()` matches the SFNT family name that `TextSystem::set_families`
    /// resolves, and populates `sfnt_index` so every returned name round-trips through
    /// `family_files()`.
    pub(super) fn installed_families(&self) -> Vec<RawFamily> {
        let mut raw = Vec::new();
        let mut sfnt_map: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut seen_faces: HashSet<(PathBuf, u32)> = HashSet::new();

        for fam in &self.families {
            let Some(xml_name) = fam.name.as_deref() else {
                continue;
            };
            let xml_mono = xml_name.eq_ignore_ascii_case("monospace")
                || xml_name.eq_ignore_ascii_case("serif-monospace");
            for font in &fam.fonts {
                if !seen_faces.insert((font.path.clone(), font.index)) {
                    continue;
                }
                let Ok(mut file) = std::fs::File::open(&font.path) else {
                    continue;
                };
                let Some(meta) = reader_sfnt_meta(&mut file, font.index) else {
                    continue;
                };
                let mut weights = vec![font.weight];
                weights.extend_from_slice(&font.wght_axis_values);
                if let Some(w) = meta.weight_class {
                    weights.push(w);
                }
                if let Some((min, max)) = meta.wght_axis {
                    push_weight_range(&mut weights, min, max);
                }
                let entry = sfnt_map.entry(meta.family.to_lowercase()).or_default();
                push_unique(entry, &font.path);
                raw.push(RawFamily {
                    name: meta.family,
                    monospace: meta.monospace || xml_mono,
                    weights,
                });
            }
        }

        drop(self.sfnt_index.set(sfnt_map));
        raw
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CjkBucket {
    Hans,
    Hant,
    HantHk,
    Jpan,
    Kore,
}

fn tag_matches_cjk(tag: &LangTag, bucket: CjkBucket) -> bool {
    match bucket {
        CjkBucket::Hans => {
            tag.script.as_deref() == Some("hans")
                || (tag.lang == "zh"
                    && tag.script.is_none()
                    && matches!(tag.region.as_deref(), None | Some("cn" | "sg")))
        }
        CjkBucket::Hant => {
            tag.script.as_deref() == Some("hant")
                || (tag.lang == "zh"
                    && tag.script.is_none()
                    && matches!(tag.region.as_deref(), Some("tw" | "hk" | "mo")))
        }
        CjkBucket::HantHk => {
            tag.lang == "zh" && matches!(tag.region.as_deref(), Some("hk" | "mo"))
        }
        CjkBucket::Jpan => {
            tag.lang == "ja" || matches!(tag.script.as_deref(), Some("jpan" | "hira" | "kana"))
        }
        CjkBucket::Kore => {
            tag.lang == "ko" || matches!(tag.script.as_deref(), Some("kore" | "hang"))
        }
    }
}

const GENERIC_LANG_SCRIPTS: &[(&str, &str)] = &[
    ("ar", "arab"), ("he", "hebr"), ("iw", "hebr"), ("hi", "deva"), ("bn", "beng"),
    ("th", "thai"), ("lo", "laoo"), ("my", "mymr"), ("km", "khmr"), ("bo", "tibt"),
    ("am", "ethi"), ("ta", "taml"), ("te", "telu"), ("kn", "knda"), ("ml", "mlym"),
    ("gu", "gujr"), ("pa", "guru"), ("or", "orya"), ("si", "sinh"), ("ka", "geor"),
    ("hy", "armn"), ("el", "grek"),
];

fn default_script_for_lang(lang: &str) -> Option<&'static str> {
    if let Some(&(_, script)) = GENERIC_LANG_SCRIPTS.iter().find(|&&(l, _)| l == lang) {
        return Some(script);
    }
    match lang {
        "fa" | "ur" | "ps" | "ug" => Some("arab"),
        "yi" => Some("hebr"),
        "mr" | "ne" | "sa" => Some("deva"),
        "as" => Some("beng"),
        "ti" => Some("ethi"),
        "ru" | "uk" | "bg" | "sr" | "be" => Some("cyrl"),
        "ja" => Some("jpan"),
        "ko" => Some("kore"),
        "zh" => Some("hani"),
        _ => None,
    }
}

fn is_generic_lang_for_script(lang: &str, script: &str) -> bool {
    GENERIC_LANG_SCRIPTS.iter().any(|&(l, s)| l == lang && s == script)
}

fn sans_fallback_fonts(fam: &XmlFamily) -> Vec<&XmlFont> {
    let default_fonts: Vec<&XmlFont> = fam
        .fonts
        .iter()
        .filter(|f| {
            f.fallback_for
                .as_deref()
                .is_none_or(|t| t.eq_ignore_ascii_case("sans-serif"))
        })
        .collect();
    if !default_fonts.is_empty() {
        default_fonts
    } else {
        fam.fonts.iter().collect()
    }
}

fn push_sans_fallback_fonts(fam: &XmlFamily, out: &mut Vec<PathBuf>) {
    for font in sans_fallback_fonts(fam) {
        push_unique(out, &font.path);
    }
}

fn push_unique(out: &mut Vec<PathBuf>, path: &Path) {
    if !out.iter().any(|p| p.as_path() == path) {
        out.push(path.to_path_buf());
    }
}

fn resolve_font_in_dirs(file: &str, font_dirs: &[&Path]) -> Option<PathBuf> {
    let candidate = Path::new(file);
    if candidate.is_absolute() {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    if candidate.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return None;
    }
    for dir in font_dirs {
        let path = dir.join(file);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn normalize_query(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn normalize_base_stem(file_name: &str) -> String {
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(base, _)| base);
    let base = stem
        .split(['-', '['])
        .next()
        .unwrap_or(stem);
    normalize_query(base)
}

fn parse_weight(raw: &str) -> Option<u16> {
    let w = raw.trim().parse::<u16>().ok()?;
    (1..=1000).contains(&w).then(|| w.clamp(100, 900))
}

fn script_tags(chars: &[char]) -> Vec<[u8; 4]> {
    use icu_properties::{CodePointMapData, PropertyNamesShort, props::Script as UScript};
    let names = PropertyNamesShort::<UScript>::new();
    let mut scripts: Vec<[u8; 4]> = Vec::new();
    for &ch in chars {
        let script = CodePointMapData::<UScript>::new().get(ch);
        if matches!(script, UScript::Common | UScript::Inherited | UScript::Unknown) {
            continue;
        }
        let Some(tag) = names.get_locale_script(script) else { continue };
        let Ok(bytes) = <[u8; 4]>::try_from(tag.as_str().as_bytes()) else { continue };
        if !scripts.contains(&bytes) {
            scripts.push(bytes);
        }
    }
    scripts
}

fn char_script_is_common_or_inherited(ch: char) -> bool {
    use icu_properties::{CodePointMapData, props::Script as UScript};
    matches!(CodePointMapData::<UScript>::new().get(ch), UScript::Common | UScript::Inherited)
}

fn system_config() -> &'static FontConfig {
    CONFIG.get_or_init(|| {
        let configs: Vec<&Path> = SYSTEM_CONFIG_PATHS.iter().map(Path::new).collect();
        let dirs: Vec<&Path> = SYSTEM_FONT_DIRS.iter().map(Path::new).collect();
        FontConfig::load_from_paths(&configs, &dirs)
    })
}

/// Android's XML config is small and parsed lazily on first lookup.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn prewarm() {}

/// Non-blocking cache-only lookup for `family_files(names)`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn try_cached_family_files(names: &[&str]) -> Option<Vec<PathBuf>> {
    if !super::system_fonts_enabled() || names.is_empty() {
        return Some(Vec::new());
    }
    CONFIG.get().map(|cfg| cfg.family_files(names))
}

/// Non-blocking cache-only lookup for `fallback_family_files(chars, locale)`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn try_cached_fallback_family_files(chars: &[char], locale: Option<&str>) -> Option<Vec<PathBuf>> {
    if !super::system_fonts_enabled() || chars.is_empty() {
        return Some(Vec::new());
    }
    CONFIG.get().map(|cfg| cfg.fallback_family_files(chars, locale))
}

/// Non-blocking cache-only lookup for `monospace_files()`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn try_cached_monospace_files() -> Option<Vec<PathBuf>> {
    if !super::system_fonts_enabled() {
        return Some(Vec::new());
    }
    CONFIG.get().map(FontConfig::monospace_files)
}

/// Non-blocking cache-only lookup for `emoji_files()`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn try_cached_emoji_files() -> Option<Vec<PathBuf>> {
    if !super::system_fonts_enabled() {
        return Some(Vec::new());
    }
    CONFIG.get().map(FontConfig::emoji_files)
}

/// The files of the installed families `names`, in order, each file once.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn family_files(names: &[&str]) -> Vec<PathBuf> {
    if !super::system_fonts_enabled() || names.is_empty() {
        return Vec::new();
    }
    system_config().family_files(names)
}

/// The fallback font files for `chars` in `locale`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn fallback_family_files(chars: &[char], locale: Option<&str>) -> Vec<PathBuf> {
    if !super::system_fonts_enabled() || chars.is_empty() {
        return Vec::new();
    }
    system_config().fallback_family_files(chars, locale)
}

/// The font files backing Android's `monospace` family.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn monospace_files() -> Vec<PathBuf> {
    if !super::system_fonts_enabled() {
        return Vec::new();
    }
    system_config().monospace_files()
}

/// The font files backing Android's colour emoji fallback (`und-Zsye`).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn emoji_files() -> Vec<PathBuf> {
    if !super::system_fonts_enabled() {
        return Vec::new();
    }
    system_config().emoji_files()
}

/// Every public named font family referenced by Android's `font_fallback.xml` / `fonts.xml`,
/// using the SFNT family name from each font's header so returned names round-trip through
/// `TextSystem::set_families`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn installed_families() -> Vec<RawFamily> {
    if !super::system_fonts_enabled() {
        return Vec::new();
    }
    system_config().installed_families()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
