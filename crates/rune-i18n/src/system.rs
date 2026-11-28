//! The languages the user prefers, most wanted first, as BCP 47 tags.

/// The OS's preferred languages: Windows' Preferred languages list
/// (`Windows.System.UserProfile.GlobalizationPreferences.Languages`, falling back to
/// `GetUserPreferredUILanguages`), macOS' and iOS' `NSLocale.preferredLanguages`, the browser's
/// `navigator.languages`, and the POSIX locale variables elsewhere ([`posix_languages`]).
pub fn system_languages() -> Vec<String> {
    platform()
}

/// The languages POSIX locale variables name, read through `var`: `LANGUAGE`'s colon list, then
/// `LC_ALL`, `LC_MESSAGES` and `LANG`, each turned from `de_AT.UTF-8@euro` into `de-AT`, without
/// `C`, `POSIX` and repeats.
pub fn posix_languages(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
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

/// Trims whitespace, skips empty entries, and removes duplicates while keeping preference order.
#[cfg_attr(
    not(any(test, target_os = "windows", target_os = "macos", target_os = "ios", target_arch = "wasm32")),
    allow(dead_code)
)]
fn normalize_tags<I, S>(tags: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = Vec::new();
    for raw in tags {
        let tag = raw.as_ref().trim();
        if tag.is_empty() || out.iter().any(|existing| existing == tag) {
            continue;
        }
        out.push(tag.to_owned());
    }
    out
}

/// Parses a Windows double-NUL-terminated (`ZZ` / `MULTI_SZ`) UTF-16 language list, stopping at
/// the first empty segment (the double-NUL sentinel), trimming whitespace, and deduplicating while
/// preserving preference order.
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn parse_wide_multi_sz(units: &[u16]) -> Vec<String> {
    let entries = units
        .split(|&unit| unit == 0)
        .take_while(|segment| !segment.is_empty())
        .map(String::from_utf16_lossy);
    normalize_tags(entries)
}

#[cfg(target_os = "windows")]
fn windows_globalization_languages() -> Option<Vec<String>> {
    use windows::System::UserProfile::GlobalizationPreferences;

    let list = GlobalizationPreferences::Languages().ok()?;
    let langs = normalize_tags(list.into_iter().map(|tag| tag.to_string_lossy()));
    (!langs.is_empty()).then_some(langs)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn windows_ui_languages() -> Vec<String> {
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    use windows::core::PWSTR;

    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: the first call only writes the count and the buffer length it needs.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut len) }.is_err() || len == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; len as usize];
    // SAFETY: `buffer` holds the `len` UTF-16 units the first call asked for.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, Some(PWSTR(buffer.as_mut_ptr())), &mut len) }
        .is_err()
    {
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
fn platform() -> Vec<String> {
    normalize_tags(objc2_foundation::NSLocale::preferredLanguages().iter().map(|tag| tag.to_string()))
}

#[cfg(target_arch = "wasm32")]
fn platform() -> Vec<String> {
    let Some(window) = web_sys::window() else { return Vec::new() };
    let nav = window.navigator();
    let langs = normalize_tags(nav.languages().iter().filter_map(|tag| tag.as_string()));
    if !langs.is_empty() {
        return langs;
    }
    normalize_tags(nav.language())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "ios", target_arch = "wasm32")))]
fn platform() -> Vec<String> {
    posix_languages(|name| std::env::var_os(name).and_then(|value| value.into_string().ok()))
}

#[cfg(test)]
#[path = "system_tests.rs"]
mod tests;
