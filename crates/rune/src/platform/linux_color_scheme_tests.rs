use super::*;

#[test]
fn gsettings_color_scheme_and_gtk_theme_resolve_dark_light_or_unknown() {
    // GNOME 42+ color-scheme values (including optional single quotes from CLI/tooling).
    assert_eq!(resolve_gsettings_dark(Some("prefer-dark"), None), Some(true));
    assert_eq!(resolve_gsettings_dark(Some("'prefer-dark'"), Some("Adwaita")), Some(true));
    assert_eq!(resolve_gsettings_dark(Some("prefer-light"), Some("Adwaita-dark")), Some(false));
    assert_eq!(resolve_gsettings_dark(Some("default"), Some("Adwaita")), Some(false));
    assert_eq!(resolve_gsettings_dark(Some("default"), None), Some(false));
    // Legacy GNOME/GTK dark theme paired with default color-scheme resolves to dark.
    assert_eq!(resolve_gsettings_dark(Some("default"), Some("Adwaita-dark")), Some(true));
    assert_eq!(resolve_gsettings_dark(Some("default"), Some("Yaru:dark")), Some(true));

    // Missing color-scheme key: detect explicit dark GTK theme, otherwise stay unknown so
    // settings.ini or kdeglobals can still answer.
    assert_eq!(resolve_gsettings_dark(None, Some("Adwaita-dark")), Some(true));
    assert_eq!(resolve_gsettings_dark(None, Some("Adwaita")), None);
    assert_eq!(resolve_gsettings_dark(None, None), None);
}

#[test]
fn gtk_settings_ini_parses_prefer_dark_and_dark_theme_name() {
    assert_eq!(
        parse_gtk_settings_ini_dark("[Settings]\ngtk-application-prefer-dark-theme=1\n"),
        Some(true),
    );
    assert_eq!(
        parse_gtk_settings_ini_dark("\u{feff}[Settings]\ngtk-application-prefer-dark-theme = true\r\n"),
        Some(true),
    );
    assert_eq!(
        parse_gtk_settings_ini_dark("[Settings]\ngtk-application-prefer-dark-theme=0\ngtk-theme-name=Adwaita-dark\n"),
        Some(false),
    );
    assert_eq!(
        parse_gtk_settings_ini_dark("[Settings]\n# gtk-application-prefer-dark-theme=1\ngtk-theme-name=Adwaita-dark\n"),
        Some(true),
    );
    assert_eq!(
        parse_gtk_settings_ini_dark("[Other]\ngtk-application-prefer-dark-theme=1\n[Settings]\ngtk-theme-name=Adwaita\n"),
        None,
    );
    assert_eq!(parse_gtk_settings_ini_dark(""), None);
}

#[test]
fn kdeglobals_parses_window_background_luminance_and_color_scheme_name() {
    // Breeze Dark window background (RGB 42, 46, 50) -> dark.
    assert_eq!(
        parse_kdeglobals_dark("[Colors:Window]\nBackgroundNormal=42,46,50\n"),
        Some(true),
    );
    // Breeze Light window background (RGB 239, 240, 241) -> light.
    assert_eq!(
        parse_kdeglobals_dark("[Colors:Window]\nBackgroundNormal=239,240,241\n"),
        Some(false),
    );
    // BackgroundNormal takes precedence over ColorScheme name.
    assert_eq!(
        parse_kdeglobals_dark("[General]\nColorScheme=BreezeLight\n[Colors:Window]\nBackgroundNormal=30,30,30\n"),
        Some(true),
    );
    // Falls back to ColorScheme name when BackgroundNormal is absent.
    assert_eq!(
        parse_kdeglobals_dark("[General]\nColorScheme=BreezeDark\n"),
        Some(true),
    );
    assert_eq!(
        parse_kdeglobals_dark("[General]\nColorScheme=BreezeLight\n"),
        Some(false),
    );
    // Neutral scheme name with no colors stays unknown.
    assert_eq!(
        parse_kdeglobals_dark("[General]\nColorScheme=Breeze\n[Colors:Window]\nBackgroundNormal=invalid\n"),
        None,
    );
}

#[test]
fn fallback_precedence_skips_gsettings_on_kde_and_stays_none_when_all_sources_missing() {
    let mut gsettings_called = false;
    assert_eq!(
        resolve_color_scheme_fallback(
            true,
            || Some(true),
            || { gsettings_called = true; Some(false) },
            || Some(false),
        ),
        Some(true),
    );
    assert!(!gsettings_called, "KDE must not read GNOME GSettings");

    // KDE without kdeglobals falls back to GTK settings.ini, still skipping GNOME GSettings.
    let mut gsettings_called = false;
    assert_eq!(
        resolve_color_scheme_fallback(
            true,
            || None,
            || { gsettings_called = true; Some(false) },
            || Some(true),
        ),
        Some(true),
    );
    assert!(!gsettings_called);

    // Non-KDE prefers GSettings, then GTK settings.ini, then kdeglobals.
    let mut ini_called = false;
    let mut kde_called = false;
    assert_eq!(
        resolve_color_scheme_fallback(
            false,
            || { kde_called = true; Some(false) },
            || Some(true),
            || { ini_called = true; Some(false) },
        ),
        Some(true),
    );
    assert!(!ini_called && !kde_called);

    assert_eq!(
        resolve_color_scheme_fallback(false, || Some(false), || None, || Some(true)),
        Some(true),
    );
    assert_eq!(
        resolve_color_scheme_fallback(false, || Some(true), || None, || None),
        Some(true),
    );
    // Missing everywhere must return None rather than defaulting to light.
    assert_eq!(
        resolve_color_scheme_fallback(false, || None, || None, || None),
        None,
    );
}
