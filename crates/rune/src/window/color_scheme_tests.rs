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
fn platform_resolution_lets_macos_inherit_nsapp_and_resolves_linux_explicitly() {
    for app in [ColorScheme::System, ColorScheme::Light, ColorScheme::Dark] {
        // On macOS, a System window keeps NSWindow.appearance == nil so it inherits NSApp.appearance.
        assert_eq!(window_theme_for_platform(ColorScheme::System, app, ThemePlatform::MacOs), None);
        assert_eq!(window_theme_for_platform(ColorScheme::Light, app, ThemePlatform::MacOs), Some(Theme::Light));
        assert_eq!(window_theme_for_platform(ColorScheme::Dark, app, ThemePlatform::MacOs), Some(Theme::Dark));

        // On Windows, a System window inherits the app-wide scheme and passes None for System.
        assert_eq!(window_theme_for_platform(ColorScheme::System, app, ThemePlatform::Windows), app.winit_theme());
        assert_eq!(window_theme_for_platform(ColorScheme::Light, app, ThemePlatform::Windows), Some(Theme::Light));
        assert_eq!(window_theme_for_platform(ColorScheme::Dark, app, ThemePlatform::Windows), Some(Theme::Dark));

        // On Linux, every window resolves to an explicit Some(Theme::Dark | Light) so winit never
        // passes None to sctk-adwaita's dbus-send fallback (or X11's hardcoded dark None).
        for system_dark in [true, false] {
            let linux = ThemePlatform::Linux { system_dark };
            let expected_system = match app {
                ColorScheme::Dark => Some(Theme::Dark),
                ColorScheme::Light => Some(Theme::Light),
                ColorScheme::System => Some(if system_dark { Theme::Dark } else { Theme::Light }),
            };
            assert_eq!(window_theme_for_platform(ColorScheme::System, app, linux), expected_system);
            assert_eq!(window_theme_for_platform(ColorScheme::Light, app, linux), Some(Theme::Light));
            assert_eq!(window_theme_for_platform(ColorScheme::Dark, app, linux), Some(Theme::Dark));
        }
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
        fn create(&mut self, app_scheme: ColorScheme, platform: ThemePlatform) -> Option<Theme> {
            let theme = window_theme_for_platform(self.window_scheme, app_scheme, platform);
            self.applied = theme;
            self.live = true;
            theme
        }
        fn destroy(&mut self) {
            self.live = false;
        }
        fn sync(&mut self, app_scheme: ColorScheme, platform: ThemePlatform) {
            if !self.live { return; }
            let target = window_theme_for_platform(self.window_scheme, app_scheme, platform);
            let log = &mut self.applied_log;
            sync_window_theme(&mut self.applied, target, |t| log.push(t));
        }
    }

    // Windows: two windows, one inheriting (System) and one pinned (Light).
    let mut main = FakeHost { window_scheme: ColorScheme::System, applied: None, applied_log: Vec::new(), live: false };
    let mut pinned = FakeHost { window_scheme: ColorScheme::Light, applied: None, applied_log: Vec::new(), live: false };
    let mut app_scheme = ColorScheme::System;

    assert_eq!(main.create(app_scheme, ThemePlatform::Windows), None);
    assert_eq!(pinned.create(app_scheme, ThemePlatform::Windows), Some(Theme::Light));

    // App-wide Dark affects main (System), leaves pinned (Light) untouched.
    app_scheme = ColorScheme::Dark;
    main.sync(app_scheme, ThemePlatform::Windows);
    pinned.sync(app_scheme, ThemePlatform::Windows);
    // Repeated sync with same app_scheme is a no-op (no duplicate set_theme calls).
    main.sync(app_scheme, ThemePlatform::Windows);
    assert_eq!(main.applied_log, vec![Some(Theme::Dark)]);
    assert!(pinned.applied_log.is_empty());

    // Recreate both windows: main inherits app-wide Dark, pinned retains Light.
    main.destroy();
    pinned.destroy();
    assert_eq!(main.create(app_scheme, ThemePlatform::Windows), Some(Theme::Dark));
    assert_eq!(pinned.create(app_scheme, ThemePlatform::Windows), Some(Theme::Light));

    // Switching app-wide back to System returns main to None (OS theme); pinned stays Light.
    app_scheme = ColorScheme::System;
    main.sync(app_scheme, ThemePlatform::Windows);
    pinned.sync(app_scheme, ThemePlatform::Windows);
    assert_eq!(main.applied_log, vec![Some(Theme::Dark), None]);
    assert!(pinned.applied_log.is_empty());

    // Per-window override on main while hidden is retained when recreated.
    main.destroy();
    main.window_scheme = ColorScheme::Dark;
    main.sync(app_scheme, ThemePlatform::Windows);
    assert_eq!(main.applied_log.len(), 2);
    assert_eq!(main.create(app_scheme, ThemePlatform::Windows), Some(Theme::Dark));
}

#[test]
fn linux_system_window_tracks_desktop_scheme_live_without_falling_back_to_none() {
    struct FakeLinuxHost {
        window_scheme: ColorScheme,
        applied: Option<Theme>,
        applied_log: Vec<Option<Theme>>,
    }
    impl FakeLinuxHost {
        fn create(window_scheme: ColorScheme, app_scheme: ColorScheme, system_dark: bool) -> Self {
            let applied = window_theme_for_platform(
                window_scheme,
                app_scheme,
                ThemePlatform::Linux { system_dark },
            );
            Self { window_scheme, applied, applied_log: Vec::new() }
        }
        fn sync(&mut self, app_scheme: ColorScheme, system_dark: bool) {
            let target = window_theme_for_platform(
                self.window_scheme,
                app_scheme,
                ThemePlatform::Linux { system_dark },
            );
            let log = &mut self.applied_log;
            sync_window_theme(&mut self.applied, target, |t| log.push(t));
        }
    }

    // Dark GNOME/Wayland desktop (e.g. Fedora Silverblue without dbus-send): ColorScheme::System
    // starts explicitly Dark, never None.
    let mut main = FakeLinuxHost::create(ColorScheme::System, ColorScheme::System, true);
    let mut pinned = FakeLinuxHost::create(ColorScheme::Light, ColorScheme::System, true);
    assert_eq!(main.applied, Some(Theme::Dark));
    assert_eq!(pinned.applied, Some(Theme::Light));

    // Repeated paint with unchanged desktop theme issues zero set_theme calls.
    main.sync(ColorScheme::System, true);
    assert!(main.applied_log.is_empty());

    // Live desktop switch to light updates System window's CSD theme; pinned window stays Light.
    main.sync(ColorScheme::System, false);
    pinned.sync(ColorScheme::System, false);
    main.sync(ColorScheme::System, false);
    assert_eq!(main.applied_log, vec![Some(Theme::Light)]);
    assert!(pinned.applied_log.is_empty());

    // App-wide Dark override pins System window to Dark even while desktop is light.
    main.sync(ColorScheme::Dark, false);
    assert_eq!(main.applied_log, vec![Some(Theme::Light), Some(Theme::Dark)]);

    // Returning app-wide scheme to System immediately follows the current light desktop.
    main.sync(ColorScheme::System, false);
    assert_eq!(main.applied_log, vec![Some(Theme::Light), Some(Theme::Dark), Some(Theme::Light)]);

    // Desktop switching back to dark updates the System window to Dark again.
    main.sync(ColorScheme::System, true);
    assert_eq!(
        main.applied_log,
        vec![Some(Theme::Light), Some(Theme::Dark), Some(Theme::Light), Some(Theme::Dark)],
    );
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

#[test]
fn system_theme_change_updates_cx_system_dark_invalidates_memos_and_resolves_backdrop_dark() {
    use std::cell::Cell;
    use std::rc::Rc;

    // Effective dark follows system_dark when ColorScheme::System, and stays pinned under
    // explicit Light/Dark overrides (matching both titlebar and WindowBackdrop DWM dark sync).
    assert!(ColorScheme::System.effective_dark(ColorScheme::System, true));
    assert!(!ColorScheme::System.effective_dark(ColorScheme::System, false));
    assert!(ColorScheme::System.effective_dark(ColorScheme::Dark, false));
    assert!(!ColorScheme::System.effective_dark(ColorScheme::Light, true));
    assert!(ColorScheme::Dark.effective_dark(ColorScheme::Light, false));
    assert!(!ColorScheme::Light.effective_dark(ColorScheme::Dark, true));

    // Feeding winit's WindowEvent::ThemeChanged payload into Runtime::set_system_dark updates
    // Cx::system_dark(), marks the view dirty, and invalidates memos.
    struct State {
        seen_dark: Rc<Cell<bool>>,
        view_runs: Rc<Cell<u32>>,
        memo_runs: Rc<Cell<u32>>,
    }
    let seen_dark = Rc::new(Cell::new(true));
    let view_runs = Rc::new(Cell::new(0));
    let memo_runs = Rc::new(Cell::new(0));
    let mut rt = crate::Runtime::new(
        State {
            seen_dark: seen_dark.clone(),
            view_runs: view_runs.clone(),
            memo_runs: memo_runs.clone(),
        },
        |s: &State, cx: &mut crate::Cx<State>| {
            s.view_runs.set(s.view_runs.get() + 1);
            let dark = cx.system_dark();
            s.seen_dark.set(dark);
            let memo_runs = s.memo_runs.clone();
            crate::memo("theme-memo", 0u32, move || {
                memo_runs.set(memo_runs.get() + 1);
                crate::el().size(if dark { 80.0 } else { 40.0 }, 20.0)
            })
        },
        (160.0, 80.0),
        crate::Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(seen_dark.get());
    assert_eq!((view_runs.get(), memo_runs.get()), (1, 1));

    // Simulates WindowEvent::ThemeChanged(Theme::Light) -> rt.set_system_dark(false).
    rt.set_system_dark(Theme::Light == Theme::Dark);
    rt.frame(1.0);
    assert!(!seen_dark.get(), "Cx::system_dark() must follow ThemeChanged(Light)");
    assert_eq!((view_runs.get(), memo_runs.get()), (2, 2));

    // Duplicate ThemeChanged(Light) is a no-op (zero view/memo rebuilds).
    rt.set_system_dark(Theme::Light == Theme::Dark);
    rt.frame(2.0);
    assert_eq!((view_runs.get(), memo_runs.get()), (2, 2));

    // Simulates WindowEvent::ThemeChanged(Theme::Dark) -> rt.set_system_dark(true).
    rt.set_system_dark(Theme::Dark == Theme::Dark);
    rt.frame(3.0);
    assert!(seen_dark.get(), "Cx::system_dark() must follow ThemeChanged(Dark)");
    assert_eq!((view_runs.get(), memo_runs.get()), (3, 3));
}
