//! Windows: system font files found through DirectWrite, so the Fonts folder is never listed and
//! no font file is opened until a text needs it. A family's files come by name from the system
//! font collection (the font cache service already knows every installed face), and a text the
//! app's fonts lack goes to Windows' own fallback (`IDWriteFontFallback::MapCharacters`, what
//! Edge and Notepad fall back with), which names the one font that covers it.

#![allow(unsafe_code)]

use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{BOOL, HSTRING, Interface, PCWSTR, implement};

/// The factory's system collection and fallback, made the first time a text needs a system font.
struct DirectWrite {
    collection: IDWriteFontCollection1,
    fallback: IDWriteFontFallback,
}

thread_local! {
    static DW: Option<DirectWrite> = make();
}

fn make() -> Option<DirectWrite> {
    // SAFETY: the shared factory and what it hands out, used on this thread only.
    unsafe {
        let factory: IDWriteFactory3 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let mut collection = None;
        factory.GetSystemFontCollection(false, &mut collection, false).ok()?;
        Some(DirectWrite { collection: collection?, fallback: factory.GetSystemFontFallback().ok()? })
    }
}

/// DirectWrite reads from Windows' system font cache service and needs no eager build.
pub(super) fn prewarm() {}

/// The files of the installed families `names` (any of their names, case aside), in order, each
/// file once. Nothing is opened: the paths come from the font cache.
pub(super) fn family_files(names: &[&str]) -> Vec<PathBuf> {
    DW.with(|dw| {
        let Some(dw) = dw else { return Vec::new() };
        let mut out: Vec<PathBuf> = Vec::new();
        for name in names {
            let (mut index, mut exists) = (0u32, BOOL(0));
            // SAFETY: the system collection, asked by name; the family's face references read only.
            unsafe {
                if dw.collection.FindFamilyName(&HSTRING::from(*name), &mut index, &mut exists).is_err() || !exists.as_bool() {
                    continue;
                }
                let Ok(family) = dw.collection.GetFontFamily(index) else { continue };
                for i in 0..family.GetFontCount() {
                    if let Some(path) = family.GetFontFaceReference(i).ok().and_then(|r| path_of(&r))
                        && !out.contains(&path)
                    {
                        out.push(path);
                    }
                }
            }
        }
        out
    })
}

/// Every installed font family in DirectWrite's system font collection, read from Windows' font
/// cache without listing the Fonts folder or opening font files.
pub(super) fn installed_families() -> Vec<super::installed::RawFamily> {
    DW.with(|dw| {
        let Some(dw) = dw else { return Vec::new() };
        let mut out = Vec::new();
        // SAFETY: `dw.collection` is a live `IDWriteFontCollection1` queried for read-only metadata.
        unsafe {
            let count = dw.collection.GetFontFamilyCount();
            for i in 0..count {
                let Ok(family) = dw.collection.GetFontFamily(i) else { continue };
                let Some(name) = localized_family_name(&family) else { continue };
                if name.is_empty() || name.starts_with('.') {
                    continue;
                }
                let mut monospace = false;
                let mut weights = Vec::new();
                for j in 0..family.GetFontCount() {
                    let Ok(font) = family.GetFont(j) else { continue };
                    if font.GetSimulations() != DWRITE_FONT_SIMULATIONS_NONE {
                        continue;
                    }
                    monospace |= font.IsMonospacedFont().as_bool();
                    weights.push(super::installed::clamp_weight(font.GetWeight().0 as f32));
                }
                // Variable fonts: collect each face's `wght` axis range from the family's font set
                // (Windows 10 1703+) without opening font files.
                if let Ok(family2) = family.cast::<IDWriteFontFamily2>()
                    && let Ok(set) = family2.GetFontSet()
                {
                    collect_weight_axes(&set, &mut weights);
                }
                if !weights.is_empty() {
                    out.push(super::installed::RawFamily { name, monospace, weights });
                }
            }
        }
        out
    })
}

/// Reads the preferred family name (`en-US` when present, else index 0) from `family`.
unsafe fn localized_family_name(family: &IDWriteFontFamily1) -> Option<String> {
    // SAFETY: `family` is a live `IDWriteFontFamily1`; the returned strings are read only.
    unsafe {
        let names = family.GetFamilyNames().ok()?;
        if names.GetCount() == 0 {
            return None;
        }
        let (mut index, mut exists) = (0u32, BOOL(0));
        let index = if names.FindLocaleName(&HSTRING::from("en-US"), &mut index, &mut exists).is_ok() && exists.as_bool() {
            index
        } else {
            0
        };
        let len = names.GetStringLength(index).ok()? as usize;
        let mut buf = vec![0u16; len + 1];
        names.GetString(index, &mut buf).ok()?;
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// Appends hundred-step weights for any variable `wght` axis in `set`, querying each face
/// individually so static weights in the same family are never merged into a fake range.
unsafe fn collect_weight_axes(set: &IDWriteFontSet1, weights: &mut Vec<u16>) {
    // SAFETY: `set` is a live `IDWriteFontSet1`; axis buffers are sized from DirectWrite's count.
    unsafe {
        let mut ranges = Vec::new();
        for index in 0..set.GetFontCount() {
            let mut actual = 0u32;
            set.GetFontAxisRanges(index, &mut [], &mut actual).ok();
            if actual == 0 || actual > 64 {
                continue;
            }
            ranges.resize(actual as usize, DWRITE_FONT_AXIS_RANGE::default());
            if set.GetFontAxisRanges(index, &mut ranges, &mut actual).is_err() {
                continue;
            }
            for range in &ranges[..actual.min(ranges.len() as u32) as usize] {
                if range.axisTag == DWRITE_FONT_AXIS_TAG_WEIGHT && range.maxValue > range.minValue {
                    super::installed::push_weight_range(weights, range.minValue, range.maxValue);
                }
            }
        }
    }
}

/// The files of the fonts Windows falls back to for `chars` in `locale` (a BCP 47 tag; the
/// user's locale when None), each file once: usually one per script. Chars no installed font
/// covers add nothing.
pub(super) fn fallback_files(chars: &[char], locale: Option<&str>) -> Vec<PathBuf> {
    mapped_files(chars, locale, false)
}

/// [`fallback_files`], each with every other face of its family: a script's fallback then
/// draws a medium or bold label in the family's own Semibold or Bold (Segoe UI's for Arabic and
/// Hebrew) rather than the Regular face, faked bold.
pub(super) fn fallback_family_files(chars: &[char], locale: Option<&str>) -> Vec<PathBuf> {
    mapped_files(chars, locale, true)
}

fn mapped_files(chars: &[char], locale: Option<&str>, whole_family: bool) -> Vec<PathBuf> {
    DW.with(|dw| {
        let Some(dw) = dw else { return Vec::new() };
        let text: Vec<u16> = chars.iter().collect::<String>().encode_utf16().collect();
        let len = text.len() as u32;
        let source: IDWriteTextAnalysisSource = Source { text, len, locale: locale_name(locale) }.into();
        let mut out: Vec<PathBuf> = Vec::new();
        let mut at = 0;
        while at < len {
            let (mut mapped, mut font, mut scale) = (0u32, None, 0f32);
            // SAFETY: `source` lives across the call; the mapped font is read for its file only.
            // A regular face: the file is what's loaded, and its family brings its own weights.
            let ok = unsafe {
                dw.fallback.MapCharacters(
                    &source,
                    at,
                    len - at,
                    None::<&IDWriteFontCollection>,
                    PCWSTR::null(),
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    &mut mapped,
                    &mut font,
                    &mut scale,
                )
            };
            if ok.is_err() || mapped == 0 {
                break;
            }
            // SAFETY: as above.
            let path = font.as_ref().and_then(|f| f.cast::<IDWriteFont3>().ok()).and_then(|f| unsafe { f.GetFontFaceReference() }.ok()).and_then(|r| unsafe { path_of(&r) });
            if let Some(path) = path
                && !out.contains(&path)
            {
                out.push(path);
            }
            // SAFETY: the mapped font's family and its face references, read only.
            if whole_family && let Some(family) = font.and_then(|f| unsafe { f.GetFontFamily() }.ok()).and_then(|f| f.cast::<IDWriteFontFamily1>().ok()) {
                // SAFETY: `family` is a valid `IDWriteFontFamily1` returned by DirectWrite.
                for i in 0..unsafe { family.GetFontCount() } {
                    // SAFETY: `i` is below `family.GetFontCount()`, and `path_of` reads only the
                    // reference key owned by `r`'s font file.
                    if let Some(path) = unsafe { family.GetFontFaceReference(i) }.ok().and_then(|r| unsafe { path_of(&r) })
                        && !out.contains(&path)
                    {
                        out.push(path);
                    }
                }
            }
            at += mapped;
        }
        out
    })
}

/// The local file behind a face; None for a font that isn't a file on this machine.
unsafe fn path_of(face: &IDWriteFontFaceReference) -> Option<PathBuf> {
    // SAFETY: the reference key belongs to `file` and is read while it lives.
    unsafe {
        let file = face.GetFontFile().ok()?;
        let (mut key, mut size) = (std::ptr::null_mut(), 0u32);
        file.GetReferenceKey(&mut key, &mut size).ok()?;
        let loader: IDWriteLocalFontFileLoader = file.GetLoader().ok()?.cast().ok()?;
        let len = loader.GetFilePathLengthFromKey(key, size).ok()? as usize;
        let mut path = vec![0u16; len + 1];
        loader.GetFilePathFromKey(key, size, &mut path).ok()?;
        Some(std::ffi::OsString::from_wide(&path[..len]).into())
    }
}

/// `locale` NUL-terminated, or the user's locale.
fn locale_name(locale: Option<&str>) -> Vec<u16> {
    if let Some(tag) = locale {
        return tag.encode_utf16().chain([0]).collect();
    }
    let mut buf = [0u16; 85];
    // SAFETY: a buffer of LOCALE_NAME_MAX_LENGTH characters.
    let n = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut buf) };
    if n <= 0 {
        return "en-US".encode_utf16().chain([0]).collect();
    }
    buf[..n as usize].to_vec()
}

/// The text `MapCharacters` reads: one left-to-right run in one locale.
#[implement(IDWriteTextAnalysisSource)]
struct Source {
    text: Vec<u16>,
    len: u32,
    locale: Vec<u16>,
}

impl IDWriteTextAnalysisSource_Impl for Source_Impl {
    fn GetTextAtPosition(&self, at: u32, text: *mut *mut u16, len: *mut u32) -> windows::core::Result<()> {
        // SAFETY: DirectWrite's out parameters; the text lives as long as the source.
        unsafe {
            if at >= self.len {
                *text = std::ptr::null_mut();
                *len = 0;
            } else {
                *text = self.text.as_ptr().add(at as usize).cast_mut();
                *len = self.len - at;
            }
        }
        Ok(())
    }

    fn GetTextBeforePosition(&self, at: u32, text: *mut *mut u16, len: *mut u32) -> windows::core::Result<()> {
        // SAFETY: as above.
        unsafe {
            if at == 0 || at > self.len {
                *text = std::ptr::null_mut();
                *len = 0;
            } else {
                *text = self.text.as_ptr().cast_mut();
                *len = at;
            }
        }
        Ok(())
    }

    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }

    fn GetLocaleName(&self, at: u32, len: *mut u32, locale: *mut *mut u16) -> windows::core::Result<()> {
        // SAFETY: as above; the locale lives as long as the source.
        unsafe {
            *len = self.len.saturating_sub(at);
            *locale = self.locale.as_ptr().cast_mut();
        }
        Ok(())
    }

    fn GetNumberSubstitution(&self, at: u32, len: *mut u32, substitution: windows::core::OutRef<IDWriteNumberSubstitution>) -> windows::core::Result<()> {
        // SAFETY: as above.
        unsafe { *len = self.len.saturating_sub(at) };
        substitution.write(None)
    }
}
