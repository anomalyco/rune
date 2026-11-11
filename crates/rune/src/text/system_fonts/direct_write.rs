//! Windows: system font files found through DirectWrite, so the Fonts folder is never listed and
//! no font file is opened until a text needs it. A family's files come by name from the system
//! font collection (the font cache service already knows every installed face), and a text the
//! app's fonts lack goes to Windows' own fallback (`IDWriteFontFallback::MapCharacters`, what
//! Edge and Notepad fall back with), which names the one font that covers it.

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

/// The files of the fonts Windows falls back to for `chars` in `locale` (a BCP 47 tag; the
/// user's locale when None), each file once: usually one per script. Chars no installed font
/// covers add nothing.
pub(super) fn fallback_files(chars: &[char], locale: Option<&str>) -> Vec<PathBuf> {
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
            let path = font.and_then(|f| f.cast::<IDWriteFont3>().ok()).and_then(|f| unsafe { f.GetFontFaceReference() }.ok()).and_then(|r| unsafe { path_of(&r) });
            if let Some(path) = path
                && !out.contains(&path)
            {
                out.push(path);
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
