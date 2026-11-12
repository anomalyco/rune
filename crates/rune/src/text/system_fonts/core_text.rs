//! macOS: system font files found through CoreText, as DirectWrite finds them on Windows
//! (`direct_write`), so no font folder is walked and no font is parsed until a text needs it.
//! A text the app's fonts lack goes to CoreText's own fallback (`CTFontCreateForStringWithLanguage`,
//! what Safari and AppKit fall back with), which names the font that covers it; a family's
//! files come from CoreText's descriptors for the family name.

use std::path::PathBuf;

use objc2_core_foundation::{CFDictionary, CFRange, CFRetained, CFString, CFType, CFURL, CFURLPathStyle};
use objc2_core_text::{CTFont, CTFontDescriptor, CTFontManagerCreateFontDescriptorsFromURL, CTFontUIFontType, kCTFontFamilyNameAttribute, kCTFontURLAttribute};

/// The file behind a CoreText attribute holder's `kCTFontURLAttribute`.
fn path_of(url: Option<CFRetained<CFType>>) -> Option<PathBuf> {
    let url = url?.downcast::<CFURL>().ok()?;
    Some(PathBuf::from(url.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle)?.to_string()))
}

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
    let mut out: Vec<PathBuf> = Vec::new();
    let mut families: Vec<String> = Vec::new();
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
            let family = if family.starts_with('.') { public_family(&font).unwrap_or(family) } else { family };
            // The system font itself: it had the character, nothing to add.
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

/// The family of the first face in `font`'s file whose family isn't hidden (`.`-prefixed).
fn public_family(font: &CTFont) -> Option<String> {
    // SAFETY: CoreText's descriptors for the file, read for their family names only.
    unsafe {
        let url = font.attribute(kCTFontURLAttribute)?.downcast::<CFURL>().ok()?;
        let faces = CTFontManagerCreateFontDescriptorsFromURL(&url)?;
        // An array of font descriptors, as CoreText documents it.
        let faces: CFRetained<objc2_core_foundation::CFArray<CTFontDescriptor>> = CFRetained::cast_unchecked(faces);
        (0..faces.len()).filter_map(|i| faces.get(i)).find_map(|d| {
            let name = d.attribute(kCTFontFamilyNameAttribute)?.downcast::<CFString>().ok()?.to_string();
            (!name.starts_with('.')).then_some(name)
        })
    }
}