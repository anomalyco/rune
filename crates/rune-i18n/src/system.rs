//! The languages the user prefers, most wanted first, as BCP 47 tags.

/// The OS's preferred languages: Windows' user UI languages (`GetUserPreferredUILanguages`),
/// macOS' and iOS' `NSLocale.preferredLanguages`, the browser's `navigator.languages`, and the
/// POSIX locale variables elsewhere ([`posix_languages`]).
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
        let tag = value.split(['.', '@']).next().unwrap_or("").replace('_', "-");
        if tag.is_empty() || tag == "C" || tag == "POSIX" || out.contains(&tag) {
            continue;
        }
        out.push(tag);
    }
    out
}

#[cfg(target_os = "windows")]
fn platform() -> Vec<String> {
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
    buffer.split(|&unit| unit == 0).filter(|name| !name.is_empty()).map(String::from_utf16_lossy).collect()
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn platform() -> Vec<String> {
    objc2_foundation::NSLocale::preferredLanguages().iter().map(|tag| tag.to_string()).collect()
}

#[cfg(target_arch = "wasm32")]
fn platform() -> Vec<String> {
    let Some(window) = web_sys::window() else { return Vec::new() };
    window.navigator().languages().iter().filter_map(|tag| tag.as_string()).collect()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "ios", target_arch = "wasm32")))]
fn platform() -> Vec<String> {
    posix_languages(|name| std::env::var_os(name).and_then(|value| value.into_string().ok()))
}
