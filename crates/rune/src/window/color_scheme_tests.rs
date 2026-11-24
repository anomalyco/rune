use super::*;
use winit::window::Theme;

#[test]
fn default_is_system_and_maps_to_none() {
    assert_eq!(ColorScheme::default(), ColorScheme::System);
    assert_eq!(ColorScheme::System.winit_theme(), None);
    assert_eq!(ColorScheme::Light.winit_theme(), Some(Theme::Light));
    assert_eq!(ColorScheme::Dark.winit_theme(), Some(Theme::Dark));
}

#[test]
fn macos_appearance_names_match_appkit_constants() {
    assert_eq!(ColorScheme::System.macos_appearance_name(), None);
    assert_eq!(ColorScheme::Light.macos_appearance_name(), Some("NSAppearanceNameAqua"));
    assert_eq!(ColorScheme::Dark.macos_appearance_name(), Some("NSAppearanceNameDarkAqua"));
}

#[test]
fn u8_encoding_round_trips_and_clamps_unknown_values_to_system() {
    for scheme in [ColorScheme::System, ColorScheme::Light, ColorScheme::Dark] {
        assert_eq!(ColorScheme::from_u8(scheme.to_u8()), scheme);
    }
    assert_eq!(ColorScheme::from_u8(3), ColorScheme::System);
    assert_eq!(ColorScheme::from_u8(u8::MAX), ColorScheme::System);
}

#[test]
fn per_window_override_takes_precedence_over_app_scheme_while_system_inherits() {
    let cases = [
        (ColorScheme::System, ColorScheme::System, ColorScheme::System),
        (ColorScheme::System, ColorScheme::Light, ColorScheme::Light),
        (ColorScheme::System, ColorScheme::Dark, ColorScheme::Dark),
        (ColorScheme::Light, ColorScheme::System, ColorScheme::Light),
        (ColorScheme::Light, ColorScheme::Light, ColorScheme::Light),
        (ColorScheme::Light, ColorScheme::Dark, ColorScheme::Light),
        (ColorScheme::Dark, ColorScheme::System, ColorScheme::Dark),
        (ColorScheme::Dark, ColorScheme::Light, ColorScheme::Dark),
        (ColorScheme::Dark, ColorScheme::Dark, ColorScheme::Dark),
    ];
    for (window, app, expected) in cases {
        assert_eq!(window.effective_for_window(app), expected, "window={window:?} app={app:?}");
    }
}

#[test]
fn platform_resolution_lets_macos_inherit_nsapp_and_resolves_fallback_elsewhere() {
    for app in [ColorScheme::System, ColorScheme::Light, ColorScheme::Dark] {
        // On macOS, a System window keeps NSWindow.appearance == nil so it inherits NSApp.appearance.
        assert_eq!(window_theme_for_platform(ColorScheme::System, app, true), None);
        assert_eq!(window_theme_for_platform(ColorScheme::Light, app, true), Some(Theme::Light));
        assert_eq!(window_theme_for_platform(ColorScheme::Dark, app, true), Some(Theme::Dark));

        // On Windows and Linux, a System window resolves directly to the app-wide scheme.
        assert_eq!(window_theme_for_platform(ColorScheme::System, app, false), app.winit_theme());
        assert_eq!(window_theme_for_platform(ColorScheme::Light, app, false), Some(Theme::Light));
        assert_eq!(window_theme_for_platform(ColorScheme::Dark, app, false), Some(Theme::Dark));
    }
}

#[test]
fn fake_window_sink_applies_only_on_change_and_retains_across_recreation() {
    struct FakeHost {
        window_scheme: ColorScheme,
        applied: Option<Theme>,
        applied_log: Vec<Option<Theme>>,
        live: bool,
    }
    impl FakeHost {
        fn create(&mut self, app_scheme: ColorScheme, is_macos: bool) -> Option<Theme> {
            let theme = window_theme_for_platform(self.window_scheme, app_scheme, is_macos);
            self.applied = theme;
            self.live = true;
            theme
        }
        fn destroy(&mut self) {
            self.live = false;
        }
        fn sync(&mut self, app_scheme: ColorScheme, is_macos: bool) {
            if !self.live { return; }
            let target = window_theme_for_platform(self.window_scheme, app_scheme, is_macos);
            let log = &mut self.applied_log;
            sync_window_theme(&mut self.applied, target, |t| log.push(t));
        }
    }

    // Windows/Linux: two windows, one inheriting (System) and one pinned (Light).
    let mut main = FakeHost { window_scheme: ColorScheme::System, applied: None, applied_log: Vec::new(), live: false };
    let mut pinned = FakeHost { window_scheme: ColorScheme::Light, applied: None, applied_log: Vec::new(), live: false };
    let mut app_scheme = ColorScheme::System;

    assert_eq!(main.create(app_scheme, false), None);
    assert_eq!(pinned.create(app_scheme, false), Some(Theme::Light));

    // App-wide Dark affects main (System), leaves pinned (Light) untouched.
    app_scheme = ColorScheme::Dark;
    main.sync(app_scheme, false);
    pinned.sync(app_scheme, false);
    // Repeated sync with same app_scheme is a no-op (no duplicate set_theme calls).
    main.sync(app_scheme, false);
    assert_eq!(main.applied_log, vec![Some(Theme::Dark)]);
    assert!(pinned.applied_log.is_empty());

    // Recreate both windows: main inherits app-wide Dark, pinned retains Light.
    main.destroy();
    pinned.destroy();
    assert_eq!(main.create(app_scheme, false), Some(Theme::Dark));
    assert_eq!(pinned.create(app_scheme, false), Some(Theme::Light));

    // Switching app-wide back to System returns main to None (OS theme); pinned stays Light.
    app_scheme = ColorScheme::System;
    main.sync(app_scheme, false);
    pinned.sync(app_scheme, false);
    assert_eq!(main.applied_log, vec![Some(Theme::Dark), None]);
    assert!(pinned.applied_log.is_empty());

    // Per-window override on main while hidden is retained when recreated.
    main.destroy();
    main.window_scheme = ColorScheme::Dark;
    main.sync(app_scheme, false);
    assert_eq!(main.applied_log.len(), 2);
    assert_eq!(main.create(app_scheme, false), Some(Theme::Dark));
}

#[test]
fn fake_macos_app_appearance_sink_maps_all_schemes_in_order() {
    struct FakeMacOsApp {
        current: Option<&'static str>,
        history: Vec<Option<&'static str>>,
    }
    impl FakeMacOsApp {
        fn set(&mut self, scheme: ColorScheme) {
            let name = scheme.macos_appearance_name();
            self.current = name;
            self.history.push(name);
        }
    }
    let mut fake = FakeMacOsApp { current: None, history: Vec::new() };

    fake.set(ColorScheme::Light);
    assert_eq!(fake.current, Some("NSAppearanceNameAqua"));
    fake.set(ColorScheme::Dark);
    assert_eq!(fake.current, Some("NSAppearanceNameDarkAqua"));
    fake.set(ColorScheme::System);
    assert_eq!(fake.current, None);
    assert_eq!(fake.history, vec![Some("NSAppearanceNameAqua"), Some("NSAppearanceNameDarkAqua"), None]);
}
