//! Preferred user languages as BCP 47 tags (RFC 5646), queried lazily on first read so apps
//! pay zero cost before their first frame.

use std::sync::OnceLock;
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicU64, Ordering};

static CACHED: OnceLock<Vec<String>> = OnceLock::new();

#[cfg(target_os = "windows")]
static LANG_SETTING_GEN: AtomicU64 = AtomicU64::new(0);

/// Lazily queried, process-cached platform preferred languages (most preferred first).
pub(crate) fn cached() -> &'static [String] {
    CACHED.get_or_init(platform).as_slice()
}

/// Whether the process-wide lazy language cache has already been populated.
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn is_cached() -> bool {
    CACHED.get().is_some()
}

/// Queries the platform preferred languages directly (used when Windows broadcasts a
/// `WM_SETTINGCHANGE` `"Intl"` update).
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn current() -> Vec<String> {
    platform()
}

/// Whether a `WM_SETTINGCHANGE` area string (`lParam`) announces a Windows locale/language change.
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn is_locale_setting_change_area(area: Option<&str>) -> bool {
    matches!(area, Some(s) if s.eq_ignore_ascii_case("intl"))
}

/// Bumps the Windows locale-change generation when `WM_SETTINGCHANGE` (`"Intl"`) arrives.
#[cfg(target_os = "windows")]
#[inline]
pub(crate) fn notify_setting_change() {
    LANG_SETTING_GEN.fetch_add(1, Ordering::Release);
}

/// Current Windows `WM_SETTINGCHANGE` (`"Intl"`) generation.
#[cfg(target_os = "windows")]
#[inline]
pub(crate) fn setting_generation() -> u64 {
    LANG_SETTING_GEN.load(Ordering::Acquire)
}

/// Trims whitespace, drops empty entries, and removes duplicates while keeping preference order.
pub(crate) fn normalize<I, S>(languages: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = Vec::new();
    for raw in languages {
        push_tag(&mut out, raw.as_ref());
    }
    out
}

fn push_tag(out: &mut Vec<String>, raw: &str) {
    let tag = raw.trim();
    if tag.is_empty() || out.iter().any(|existing| existing == tag) {
        return;
    }
    out.push(tag.to_owned());
}

/// Parses a Windows double-NUL-terminated (`ZZ` / `MULTI_SZ`) UTF-16 language list, stopping at
/// the first empty segment (the double-NUL sentinel), trimming whitespace, and deduplicating while
/// preserving preference order.
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn parse_wide_multi_sz(units: &[u16]) -> Vec<String> {
    normalize(
        units
            .split(|&unit| unit == 0)
            .take_while(|segment| !segment.is_empty())
            .map(String::from_utf16_lossy),
    )
}

/// The languages POSIX locale variables name, read through `var`: `LANGUAGE`'s colon list, then
/// `LC_ALL`, `LC_MESSAGES` and `LANG`, each turned from `de_AT.UTF-8@euro` into `de-AT`, without
/// `C`, `POSIX` and repeats.
#[cfg(any(test, not(any(target_os = "windows", target_os = "macos", target_os = "ios", target_arch = "wasm32"))))]
pub(crate) fn posix_languages(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let names = var("LANGUAGE").unwrap_or_default();
    let singles = ["LC_ALL", "LC_MESSAGES", "LANG"].map(|name| var(name).unwrap_or_default());
    for value in names.split(':').chain(singles.iter().map(String::as_str)) {
        let tag = value.split(['.', '@']).next().unwrap_or("").trim().replace('_', "-");
        if tag.is_empty() || tag == "C" || tag == "POSIX" || out.contains(&tag) {
            continue;
        }
        out.push(tag);
    }
    out
}

#[cfg(target_os = "windows")]
fn windows_globalization_languages() -> Option<Vec<String>> {
    use windows::System::UserProfile::GlobalizationPreferences;

    let list = GlobalizationPreferences::Languages().ok()?;
    let langs = normalize(list.into_iter().map(|tag| tag.to_string_lossy()));
    (!langs.is_empty()).then_some(langs)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn windows_ui_languages() -> Vec<String> {
    const MUI_LANGUAGE_NAME: u32 = 0x8;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserPreferredUILanguages(
            flags: u32,
            num_languages: *mut u32,
            buffer: *mut u16,
            buffer_len: *mut u32,
        ) -> i32;
    }

    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: with a null buffer pointer, Windows only writes the language count and required
    // UTF-16 buffer length (including terminating NULs) into `count` and `len`.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut len) } == 0
        || len == 0
    {
        return Vec::new();
    }
    let mut buffer = vec![0u16; len as usize];
    // SAFETY: `buffer` holds `len` UTF-16 units as requested by the first call.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buffer.as_mut_ptr(), &mut len) } == 0 {
        return Vec::new();
    }
    let written = usize::min(len as usize, buffer.len());
    parse_wide_multi_sz(&buffer[..written])
}

#[cfg(target_os = "windows")]
fn platform() -> Vec<String> {
    windows_globalization_languages().unwrap_or_else(windows_ui_languages)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
fn platform() -> Vec<String> {
    use std::ffi::CStr;
    use std::os::raw::c_char;

    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    autoreleasepool(|_| {
        let mut out = Vec::new();
        // SAFETY: `NSLocale.preferredLanguages` returns an autoreleased `NSArray<NSString *>`,
        // and each element's UTF-8 string is copied into an owned Rust `String` inside this pool.
        unsafe {
            let langs: *mut AnyObject = msg_send![class!(NSLocale), preferredLanguages];
            if langs.is_null() {
                return out;
            }
            let count: usize = msg_send![langs, count];
            for i in 0..count {
                let item: *mut AnyObject = msg_send![langs, objectAtIndex: i];
                if item.is_null() {
                    continue;
                }
                let ptr: *const c_char = msg_send![item, UTF8String];
                if ptr.is_null() {
                    continue;
                }
                if let Ok(tag) = CStr::from_ptr(ptr).to_str() {
                    push_tag(&mut out, tag);
                }
            }
        }
        out
    })
}

#[cfg(target_arch = "wasm32")]
fn platform() -> Vec<String> {
    let mut out = Vec::new();
    for value in crate::platform::web::platform::preferred_languages().iter() {
        if let Some(tag) = value.as_string() {
            push_tag(&mut out, &tag);
        }
    }
    out
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "ios", target_arch = "wasm32")))]
fn platform() -> Vec<String> {
    posix_languages(|name| std::env::var_os(name).and_then(|value| value.into_string().ok()))
}

#[cfg(test)]
#[path = "languages_tests.rs"]
mod tests;
