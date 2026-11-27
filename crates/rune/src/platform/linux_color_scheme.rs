//! Off-UI-thread Linux desktop dark/light fallback readers (GSettings via `libgio`, GTK
//! `settings.ini`, and KDE `kdeglobals`) for desktops whose XDG Settings portal does not answer
//! `org.freedesktop.appearance` `color-scheme`, or builds without the `dbus` feature. Never
//! spawns a subprocess.

#[cfg(target_os = "linux")]
use std::ffi::CStr;

/// Returns whether a GTK theme name explicitly denotes a dark variant (such as `Adwaita-dark`,
/// `Yaru-dark`, or `Adwaita:dark`).
pub(crate) fn gtk_theme_is_dark(name: &str) -> bool {
    let trimmed = name.trim().trim_matches(['\'', '"']);
    let lower = trimmed.to_ascii_lowercase();
    lower.ends_with("-dark") || lower.ends_with(":dark") || lower == "dark"
}

/// Resolves GNOME GSettings `org.gnome.desktop.interface` `color-scheme` and optional
/// `gtk-theme` values into `Some(true)` (dark), `Some(false)` (light), or `None` (unknown).
pub(crate) fn resolve_gsettings_dark(color_scheme: Option<&str>, gtk_theme: Option<&str>) -> Option<bool> {
    if let Some(raw) = color_scheme {
        let value = raw.trim().trim_matches(['\'', '"']);
        if value.eq_ignore_ascii_case("prefer-dark") {
            return Some(true);
        }
        if value.eq_ignore_ascii_case("prefer-light") {
            return Some(false);
        }
        if value.eq_ignore_ascii_case("default") {
            return Some(gtk_theme.is_some_and(gtk_theme_is_dark));
        }
    }
    gtk_theme.and_then(|theme| gtk_theme_is_dark(theme).then_some(true))
}

/// Parses a GTK `settings.ini` (`gtk-4.0/settings.ini` or `gtk-3.0/settings.ini`) file.
/// Honors `gtk-application-prefer-dark-theme` under `[Settings]`, falling back to a dark
/// `gtk-theme-name` suffix when the boolean key is absent.
pub(crate) fn parse_gtk_settings_ini_dark(contents: &str) -> Option<bool> {
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let mut in_settings = false;
    let mut theme_dark = None;
    for line in contents.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_settings = section.trim().eq_ignore_ascii_case("Settings");
            continue;
        }
        if !in_settings {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let key = k.trim();
        let val = v.trim();
        if key.eq_ignore_ascii_case("gtk-application-prefer-dark-theme") {
            if val == "1" || val.eq_ignore_ascii_case("true") || val.eq_ignore_ascii_case("yes") {
                return Some(true);
            }
            if val == "0" || val.eq_ignore_ascii_case("false") || val.eq_ignore_ascii_case("no") {
                return Some(false);
            }
        } else if key.eq_ignore_ascii_case("gtk-theme-name") && gtk_theme_is_dark(val) {
            theme_dark = Some(true);
        }
    }
    theme_dark
}

/// Parses KDE's `kdeglobals` file: prefers `[Colors:Window] BackgroundNormal=R,G,B` perceptual
/// luminance (matching `xdg-desktop-portal-kde` / KColorScheme), falling back to a dark/light
/// `[General] ColorScheme` name when no window background color is recorded.
pub(crate) fn parse_kdeglobals_dark(contents: &str) -> Option<bool> {
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    enum Section {
        Other,
        General,
        ColorsWindow,
    }
    let mut section = Section::Other;
    let mut named_scheme = None;
    for line in contents.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(sec) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = match sec.trim() {
                "General" => Section::General,
                "Colors:Window" => Section::ColorsWindow,
                _ => Section::Other,
            };
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let key = k.trim();
        let val = v.trim();
        match section {
            Section::ColorsWindow if key == "BackgroundNormal" => {
                if let Some((r, g, b)) = parse_rgb_triplet(val) {
                    let luma = 299u32 * u32::from(r) + 587u32 * u32::from(g) + 114u32 * u32::from(b);
                    return Some(luma < 128_000);
                }
            }
            Section::General if key == "ColorScheme" => {
                let lower = val.to_ascii_lowercase();
                if lower.contains("dark") {
                    named_scheme = Some(true);
                } else if lower.contains("light") {
                    named_scheme = Some(false);
                }
            }
            _ => {}
        }
    }
    named_scheme
}

fn parse_rgb_triplet(value: &str) -> Option<(u8, u8, u8)> {
    let mut parts = value.split(',').map(|s| s.trim().parse::<u8>().ok());
    let (r, g, b) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    Some((r, g, b))
}

/// Orders desktop fallback sources so KDE does not misread an installed GNOME schema's `default`
/// value as light while GNOME/GTK desktops prefer GSettings, then GTK `settings.ini`, then
/// `kdeglobals`. Returns `None` when no source reports a preference.
pub(crate) fn resolve_color_scheme_fallback(
    is_kde: bool,
    kdeglobals: impl FnOnce() -> Option<bool>,
    gsettings: impl FnOnce() -> Option<bool>,
    gtk_ini: impl FnOnce() -> Option<bool>,
) -> Option<bool> {
    if is_kde {
        kdeglobals().or_else(gtk_ini)
    } else {
        gsettings().or_else(gtk_ini).or_else(kdeglobals)
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn gnome_interface_string(key: &CStr) -> Option<String> {
    use std::ffi::{c_char, c_void};
    // SAFETY: `g_settings_get_string` and `g_free` match GIO/GLib's C ABI; `g_free` is resolved
    // before allocation so every non-null returned pointer is freed after copying into Rust.
    unsafe {
        crate::platform::preferences::with_gsettings(c"org.gnome.desktop.interface", key, |gio, settings, key_ptr| {
            let get = gio
                .get::<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char>(b"g_settings_get_string\0")
                .ok()?;
            let free = gio.get::<unsafe extern "C" fn(*mut c_void)>(b"g_free\0").ok()?;
            let raw = get(settings, key_ptr);
            if raw.is_null() {
                return None;
            }
            let value = CStr::from_ptr(raw).to_str().ok().map(str::to_owned);
            free(raw.cast());
            value
        })
    }
}

#[cfg(target_os = "linux")]
fn read_gtk_settings_ini_dark() -> Option<bool> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    for dir in ["gtk-4.0", "gtk-3.0"] {
        if let Ok(contents) = std::fs::read_to_string(config.join(dir).join("settings.ini"))
            && let Some(dark) = parse_gtk_settings_ini_dark(&contents)
        {
            return Some(dark);
        }
    }
    None
}

/// Fallback dark/light preference read for Linux desktops whose XDG Settings portal does not
/// answer `org.freedesktop.appearance` `color-scheme`, or builds without the `dbus` feature.
/// Called at most once per process, on a background worker thread; never spawns a subprocess.
#[cfg(target_os = "linux")]
pub(crate) fn read_color_scheme_fallback() -> Option<bool> {
    resolve_color_scheme_fallback(
        crate::platform::preferences::is_kde_desktop(),
        || crate::platform::preferences::read_kdeglobals().as_deref().and_then(parse_kdeglobals_dark),
        || {
            let scheme = gnome_interface_string(c"color-scheme");
            let gtk_theme = gnome_interface_string(c"gtk-theme");
            resolve_gsettings_dark(scheme.as_deref(), gtk_theme.as_deref())
        },
        read_gtk_settings_ini_dark,
    )
}

#[cfg(test)]
#[path = "linux_color_scheme_tests.rs"]
mod tests;
