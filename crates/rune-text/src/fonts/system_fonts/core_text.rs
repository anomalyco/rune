//! macOS: system font files found through CoreText, as DirectWrite finds them on Windows
//! (`direct_write`), so no font folder is walked and no font is parsed until a text needs it.
//! A text the app's fonts lack goes to CoreText's own fallback (`CTFontCreateForStringWithLanguage`,
//! what Safari and AppKit fall back with), which names the font that covers it; a family's
//! files come from CoreText's descriptors for the family name.

#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::Mutex;

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRange, CFRetained, CFString, CFType, CFURL, CFURLPathStyle};
use objc2_core_text::{
    CTFont, CTFontCollection, CTFontDescriptor, CTFontManagerCreateFontDescriptorsFromURL,
    CTFontSymbolicTraits, CTFontUIFontType, kCTFontFamilyNameAttribute, kCTFontSymbolicTrait,
    kCTFontTraitsAttribute, kCTFontURLAttribute, kCTFontVariationAxesAttribute,
    kCTFontVariationAxisIdentifierKey, kCTFontVariationAxisMaximumValueKey,
    kCTFontVariationAxisMinimumValueKey, kCTFontWeightTrait,
};

/// The file behind a CoreText attribute holder's `kCTFontURLAttribute`.
fn path_of(url: Option<CFRetained<CFType>>) -> Option<PathBuf> {
    let url = url?.downcast::<CFURL>().ok()?;
    Some(PathBuf::from(url.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle)?.to_string()))
}

/// Every available public font family from CoreText's font collection, read from CoreText's
/// descriptor metadata without walking font directories or opening font files.
pub(super) fn installed_families() -> Vec<super::installed::RawFamily> {
    const WGHT_TAG: i64 = u32::from_be_bytes(*b"wght") as i64;
    let mut out = Vec::new();
    // SAFETY: CoreText collection and descriptor attributes are read-only and retain-counted.
    unsafe {
        let collection = CTFontCollection::from_available_fonts(None);
        let Some(descriptors) = collection.matching_font_descriptors() else {
            return out;
        };
        let descriptors: CFRetained<CFArray<CTFontDescriptor>> = CFRetained::cast_unchecked(descriptors);
        for i in 0..descriptors.len() {
            let Some(descriptor) = descriptors.get(i) else { continue };
            let Some(name) = descriptor
                .attribute(kCTFontFamilyNameAttribute)
                .and_then(|v| v.downcast::<CFString>().ok())
                .map(|s| s.to_string())
            else {
                continue;
            };
            if name.is_empty() || name.starts_with('.') {
                continue;
            }
            let mut monospace = false;
            let mut weights = Vec::new();
            if let Some(traits_raw) = descriptor.attribute(kCTFontTraitsAttribute)
                && let Ok(traits) = traits_raw.downcast::<CFDictionary>()
            {
                let traits: CFRetained<CFDictionary<CFString, CFType>> = CFRetained::cast_unchecked(traits);
                if let Some(sym) = traits
                    .get(kCTFontSymbolicTrait)
                    .and_then(|v| v.downcast::<CFNumber>().ok())
                    .and_then(|n| n.as_i64().or_else(|| n.as_i32().map(i64::from)))
                {
                    monospace = ((sym as u32) & CTFontSymbolicTraits::TraitMonoSpace.0) != 0;
                }
                if let Some(weight) = traits
                    .get(kCTFontWeightTrait)
                    .and_then(|v| v.downcast::<CFNumber>().ok())
                    .and_then(|n| n.as_f64())
                {
                    weights.push(super::installed::apple_weight_trait_to_css(weight));
                }
            }
            if let Some(axes_raw) = descriptor.attribute(kCTFontVariationAxesAttribute)
                && let Ok(axes) = axes_raw.downcast::<CFArray>()
            {
                let axes: CFRetained<CFArray<CFDictionary>> = CFRetained::cast_unchecked(axes);
                for j in 0..axes.len() {
                    let Some(axis_raw) = axes.get(j) else { continue };
                    let axis: CFRetained<CFDictionary<CFString, CFType>> = CFRetained::cast_unchecked(axis_raw);
                    let tag = axis
                        .get(kCTFontVariationAxisIdentifierKey)
                        .and_then(|v| v.downcast::<CFNumber>().ok())
                        .and_then(|n| n.as_i64().or_else(|| n.as_i32().map(i64::from)));
                    if tag != Some(WGHT_TAG) {
                        continue;
                    }
                    let min = axis
                        .get(kCTFontVariationAxisMinimumValueKey)
                        .and_then(|v| v.downcast::<CFNumber>().ok())
                        .and_then(|n| n.as_f64());
                    let max = axis
                        .get(kCTFontVariationAxisMaximumValueKey)
                        .and_then(|v| v.downcast::<CFNumber>().ok())
                        .and_then(|n| n.as_f64());
                    if let (Some(min), Some(max)) = (min, max) {
                        super::installed::push_weight_range(&mut weights, min as f32, max as f32);
                    }
                }
            }
            out.push(super::installed::RawFamily { name, monospace, weights });
        }
    }
    out
}

/// CoreText's font registry is process-wide and needs no eager collection build.
pub(super) fn prewarm() {}

/// The files of the installed families `names`, in order, each file once.
pub(super) fn family_files(names: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for name in names {
        for path in family_paths(name) {
            if !out.contains(&path) { out.push(path); }
        }
    }
    out
}

/// Every face file of family `name`, as CoreText matches the family name. San Francisco's
/// families (`.SF NS`, `.SF NS Mono`) are hidden from CoreText's lists: their files sit beside
/// the system UI font's, by name (`SFNS.ttf`, `SFNSMono.ttf`, `SFNSItalic.ttf`).
fn family_paths(name: &str) -> Vec<PathBuf> {
    if let Some(rest) = name.strip_prefix(".SF NS").or(if name == "System Font" { Some("") } else { None }) {
        let stem = format!("SFNS{}", rest.replace(' ', ""));
        // SAFETY: the system UI font, read for its file only.
        let ui = unsafe { CTFont::new_ui_font_for_language(CTFontUIFontType::System, 0.0, None) };
        // SAFETY: `f` is a live `CTFont` from above and `kCTFontURLAttribute` is CoreText's constant URL attribute key.
        let Some(dir) = ui.and_then(|f| path_of(unsafe { f.attribute(kCTFontURLAttribute) })).and_then(|p| p.parent().map(PathBuf::from)) else { return Vec::new() };
        return [format!("{stem}.ttf"), format!("{stem}Italic.ttf")].into_iter().map(|f| dir.join(f)).filter(|p| p.exists()).collect();
    }
    // SAFETY: CoreText descriptors built from and read for their attributes only.
    unsafe {
        let key: &CFString = kCTFontFamilyNameAttribute;
        let value = CFString::from_str(name);
        let attrs = CFDictionary::<CFString, CFType>::from_slices(&[key], &[value.as_ref()]);
        let descriptor = CTFontDescriptor::with_attributes(attrs.as_opaque());
        let Some(matches) = descriptor.matching_font_descriptors(None) else { return Vec::new() };
        // An array of font descriptors, as CoreText documents it.
        let matches: CFRetained<objc2_core_foundation::CFArray<CTFontDescriptor>> = CFRetained::cast_unchecked(matches);
        let mut out = Vec::new();
        for i in 0..matches.len() {
            let Some(d) = matches.get(i) else { continue };
            if let Some(path) = path_of(d.attribute(kCTFontURLAttribute))
                && !out.contains(&path)
            {
                out.push(path);
            }
        }
        out
    }
}

/// The files of the fonts CoreText falls back to for `chars` in `locale` (a BCP 47 tag), each
/// with the rest of its family's files, so a bold label draws in the family's own bold (Hiragino
/// Sans W6, not W3 emboldened). Chars no installed font covers add nothing.
pub(super) fn fallback_family_files(chars: &[char], locale: Option<&str>) -> Vec<PathBuf> {
    fallback_family_files_with(chars, locale, public_family)
}

fn fallback_family_files_with(
    chars: &[char],
    locale: Option<&str>,
    mut resolve_public: impl FnMut(&CTFont) -> Option<String>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut families: Vec<String> = Vec::new();
    // Raw family names CoreText already answered with in this call: later characters from the
    // same family add nothing and are skipped before reading font descriptors again.
    let mut seen: Vec<String> = Vec::new();
    // SAFETY: CoreText fonts made for the lookup and read for their names and files only.
    unsafe {
        let Some(base) = CTFont::new_ui_font_for_language(CTFontUIFontType::System, 0.0, None) else { return out };
        let base_name = base.family_name().to_string();
        let language = locale.map(CFString::from_str);
        for ch in chars {
            let text = CFString::from_str(ch.encode_utf8(&mut [0; 4]));
            let range = CFRange { location: 0, length: text.length() };
            let font = CTFont::for_string_with_language(&base, &text, range, language.as_deref());
            let family = font.family_name().to_string();
            // The system font itself (`.AppleSystemUIFont`, which is hidden too) already covered
            // the character, and `.LastResort` means no installed font covers it (loading it as a
            // fallback face would box every later fallback symbol). Check both before
            // `resolve_public`, which reads every face name in the file (369 faces in `SFNS.ttf`).
            if family == base_name || family.trim_start_matches('.') == "LastResort" || seen.contains(&family) { continue; }
            seen.push(family.clone());
            // PingFang (CoreText's Chinese fallback) draws with `hvgl` outlines, which no
            // outline reader here can draw: Chinese takes Hiragino Sans GB, or Heiti TC for
            // Traditional, as fontique does.
            if family.trim_start_matches('.').starts_with("PingFang") {
                let traditional = family.contains("TC") || family.contains("HK") || locale.is_some_and(|l| l.contains("Hant") || l.contains("TW") || l.contains("HK") || l.contains("MO"));
                let instead = if traditional { "Heiti TC" } else { "Hiragino Sans GB" };
                if !families.iter().any(|f| f == instead) {
                    for path in family_paths(instead) {
                        if !out.contains(&path) { out.push(path); }
                    }
                    families.push(instead.to_owned());
                }
                continue;
            }
            // A hidden family (`.Hiragino Kaku Gothic Interface`, CoreText's Japanese fallback)
            // matches no descriptors, so its weights would never load: take the family of its
            // file's first public face instead (Hiragino Sans, W0-W9).
            let family = if family.starts_with('.') { resolve_public(&font).unwrap_or(family) } else { family };
            if family == base_name || families.contains(&family) { continue; }
            if let Some(path) = path_of(font.attribute(kCTFontURLAttribute))
                && !out.contains(&path)
            {
                out.push(path);
            }
            for path in family_paths(&family) {
                if !out.contains(&path) { out.push(path); }
            }
            families.push(family);
        }
    }
    out
}

/// Bounds for `PublicFamilyCache` (entry count and slot + heap bytes).
const MAX_PUBLIC_FAMILY_ENTRIES: usize = 64;
const MAX_PUBLIC_FAMILY_BYTES: usize = 16 * 1024;

/// Bounded cache mapping a hidden-family font file path to the first public family name in that
/// file, so later fallback calls for the same hidden family avoid re-reading its descriptors.
struct PublicFamilyCache {
    entries: Vec<(String, Option<String>)>,
    bytes: usize,
}

impl PublicFamilyCache {
    const fn new() -> Self {
        Self { entries: Vec::new(), bytes: 0 }
    }

    fn entry_bytes(key: &str, value: &Option<String>) -> usize {
        std::mem::size_of::<(String, Option<String>)>()
            + key.len()
            + value.as_deref().map_or(0, str::len)
    }

    fn get(&self, key: &str) -> Option<Option<String>> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn insert(&mut self, key: String, value: Option<String>) {
        if self.entries.iter().any(|(k, _)| k == &key) {
            return;
        }
        let cost = Self::entry_bytes(&key, &value);
        if cost > MAX_PUBLIC_FAMILY_BYTES {
            return;
        }
        while self.entries.len() >= MAX_PUBLIC_FAMILY_ENTRIES || self.bytes + cost > MAX_PUBLIC_FAMILY_BYTES {
            let (old_key, old_val) = self.entries.remove(0);
            self.bytes -= Self::entry_bytes(&old_key, &old_val);
        }
        self.bytes += cost;
        self.entries.push((key, value));
    }
}

/// The family of the first face in `font`'s file whose family isn't hidden (`.`-prefixed).
fn public_family(font: &CTFont) -> Option<String> {
    static CACHE: Mutex<PublicFamilyCache> = Mutex::new(PublicFamilyCache::new());
    // SAFETY: `font` is a live `CTFont` and `kCTFontURLAttribute` is CoreText's constant URL attribute key.
    let url = unsafe { font.attribute(kCTFontURLAttribute) }?.downcast::<CFURL>().ok()?;
    let Some(key) = url.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle).map(|p| p.to_string()) else {
        return scan_public_family(&url);
    };
    if let Some(hit) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return hit;
    }
    let found = scan_public_family(&url);
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(key, found.clone());
    found
}

/// Reads the descriptors in `url` and returns the first non-hidden family name.
fn scan_public_family(url: &CFURL) -> Option<String> {
    // SAFETY: CoreText's descriptors for the file, read for their family names only.
    unsafe {
        let faces = CTFontManagerCreateFontDescriptorsFromURL(url)?;
        // An array of font descriptors, as CoreText documents it.
        let faces: CFRetained<objc2_core_foundation::CFArray<CTFontDescriptor>> = CFRetained::cast_unchecked(faces);
        (0..faces.len()).filter_map(|i| faces.get(i)).find_map(|d| {
            let name = d.attribute(kCTFontFamilyNameAttribute)?.downcast::<CFString>().ok()?.to_string();
            (!name.starts_with('.')).then_some(name)
        })
    }
}

#[cfg(test)]
#[path = "core_text_tests.rs"]
mod tests;