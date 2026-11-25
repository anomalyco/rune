use super::*;
use winit::dpi::{LogicalSize, Size};

#[test]
fn launcher_preset_is_hidden_and_preserves_ordinary_defaults() {
    let preset = WindowOptions::launcher_panel();
    assert!(preset.native.nonactivating_panel && preset.native.launcher);
    assert!(!preset.options.visible && preset.options.borderless && preset.options.transparent);
    assert!(!MacWindowOptions::default().nonactivating_panel(true).launcher);
    assert!(!preset.native.nonactivating_panel(false).launcher);
    assert!(WindowOptions::default().visible);
    let spec = AppSpec::new("launcher", (300.0, 100.0), || (), |_, _| crate::el());
    let app = App::new(spec).configured_options(WindowOptions::launcher_panel());
    assert!(app.macos_window.launcher);
    assert!(!app.window_options.visible);
    let window = WindowSpec::from_app("launcher", app);
    assert!(!window.options.visible && window.options.transparent);
}

#[test]
fn amending_a_preset_keeps_its_launcher_behaviour() {
    let preset = WindowOptions::launcher_panel().macos(|native| native.corner_radius(16.0).shadow(true));
    assert!(preset.native.launcher && preset.native.nonactivating_panel);
    assert_eq!((preset.native.corner_radius, preset.native.shadow), (Some(16.0), Some(true)));
    assert!(!preset.options.visible && preset.options.transparent);
    let spec = AppSpec::new("launcher", (300.0, 100.0), || (), |_, _| crate::el());
    let app = App::new(spec).configured_options(preset);
    assert!(app.macos_window.launcher);
    assert_eq!(app.macos_window.corner_radius, Some(16.0));
}

#[test]
fn app_windows_follow_their_app_and_share_its_key_space() {
    let mut main = App::new(AppSpec::new("main", (100.0, 100.0), || 0u32, |_, _| crate::el()));
    let settings = || WindowSpec::new("settings", AppSpec::new("settings", (100.0, 100.0), String::new, |_, _| crate::el()));
    let (_, handle) = main.window(settings());
    assert_eq!(handle.key(), "settings");
    let other = WindowSpec::new("other", AppSpec::new("other", (100.0, 100.0), || (), |_, _| crate::el()));
    let flat = flatten([WindowSpec::from_app("main", main).boxed(), other.into()]);
    assert_eq!(flat.iter().map(AnyWindowSpec::key).collect::<Vec<_>>(), ["main", "settings", "other"]);
    assert!(validate(&flat).is_ok());
    let mut clash = App::new(AppSpec::new("main", (100.0, 100.0), || 0u32, |_, _| crate::el()));
    clash.window(settings());
    assert!(validate(&flatten([WindowSpec::from_app("settings", clash).boxed()])).is_err());
}

#[test]
fn default_window_options_remain_inferable_at_existing_call_sites() {
    // This fails to compile if options becomes generic over Into<...>.
    let _configure: fn(WindowSpec<()>) -> WindowSpec<()> = |spec| spec.options(Default::default());
}

#[test]
fn explicit_empty_args_never_read_the_host_cli() {
    let options = RunOptions { accessory: true };
    let copied = options;
    assert!(options.accessory && copied.accessory);
    let args = crate::tooling::window_args(Some(vec![]), || panic!("must not inspect host --trace/--snap flags"));
    assert!(!args.headless());
    assert!(args.scene.is_none() && args.size.is_none());
    let inherited = crate::tooling::window_args(None, || crate::tooling::Args::parse_from(
        vec!["--trace".into(), "out.jsonl".into()]));
    assert!(inherited.headless());
}

#[test]
fn window_app_args_override_inherited_scene_and_size() {
    fn spec() -> AppSpec<()> { AppSpec::new("test", (100.0, 100.0), || (), |_, _| crate::el()) }
    let inherited = crate::tooling::Args::parse_from(vec!["--scene".into(), "host-scene".into()]);
    let mut window = WindowSpec::from_app("pill", App::new(spec()).args(vec!["--size".into(), "320x80".into()]));
    let resolved = window.app.resolve_args(inherited.clone());
    assert_eq!(resolved.size, Some((320.0, 80.0)));
    assert!(resolved.scene.is_none());
    let mut plain = WindowSpec::new("plain", spec());
    assert_eq!(plain.app.resolve_args(inherited).scene.as_deref(), Some("host-scene"));
    let trace = crate::tooling::Args::parse_from(vec!["--trace".into(), "host.jsonl".into()]);
    let mut isolated = WindowSpec::from_app("isolated", App::new(spec()).args(vec![]));
    assert!(!isolated.app.resolve_args(trace).headless());
    let mut traced = WindowSpec::from_app("traced", App::new(spec()).args(vec!["--trace".into(), "window.jsonl".into()]));
    assert!(traced.app.resolve_args(crate::tooling::Args::default()).headless());
}

#[test]
fn compact_window_overrides_the_legacy_minimum_without_changing_defaults() {
    let defaults = WindowOptions::default().attributes("app", (560.0, 440.0), None, true);
    assert_eq!(defaults.min_inner_size, Some(Size::Logical(LogicalSize::new(420.0, 400.0))));
    assert!(defaults.resizable && defaults.decorations && defaults.visible);
    assert_eq!(defaults.max_inner_size, None);

    let compact = WindowOptions {
        min_size: Some((280.0, 80.0)), max_size: Some((320.0, 120.0)),
        resizable: false, borderless: true, ..Default::default()
    }.attributes("compact", (300.0, 100.0), Some((860.0, 560.0)), false);
    assert_eq!(compact.inner_size, Some(Size::Logical(LogicalSize::new(300.0, 100.0))));
    assert_eq!(compact.min_inner_size, Some(Size::Logical(LogicalSize::new(280.0, 80.0))));
    assert_eq!(compact.max_inner_size, Some(Size::Logical(LogicalSize::new(320.0, 120.0))));
    assert!(!compact.resizable && !compact.decorations && !compact.visible);
}

#[test]
fn maximum_alone_caps_the_inherited_minimum_per_axis() {
    let options = WindowOptions { max_size: Some((320.0, 120.0)), ..Default::default() };
    for (app_min, expected) in [
        (None, (320.0, 120.0)),
        (Some((860.0, 80.0)), (320.0, 80.0)),
    ] {
        let attrs = options.attributes("compact", (300.0, 100.0), app_min, true);
        assert_eq!(attrs.min_inner_size, Some(Size::Logical(LogicalSize::new(expected.0, expected.1))));
        assert_eq!(attrs.max_inner_size, Some(Size::Logical(LogicalSize::new(320.0, 120.0))));
    }
}

#[test]
fn default_options_inherit_the_app_minimum() {
    let attrs = WindowOptions::default().attributes("app", (900.0, 600.0), Some((860.0, 560.0)), true);
    assert_eq!(attrs.min_inner_size, Some(Size::Logical(LogicalSize::new(860.0, 560.0))));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_transparent_attributes_disable_redirection_bitmap() {
    let opaque = WindowOptions::default().attributes("opaque", (400.0, 300.0), None, true);
    assert!(!opaque.transparent);
    assert!(format!("{opaque:?}").contains("no_redirection_bitmap: false"), "{opaque:?}");

    let transparent = WindowOptions { transparent: true, ..Default::default() }
        .attributes("transparent", (400.0, 300.0), None, true);
    assert!(transparent.transparent);
    assert!(format!("{transparent:?}").contains("no_redirection_bitmap: true"), "{transparent:?}");

    let display = Display {
        id: None, name: None, position: (0, 0), size: (1920, 1080), scale: 1.0, primary: true,
    };
    let overlay = WindowOptions::overlay(&display).attributes("overlay", (400.0, 300.0), None, true);
    assert!(overlay.transparent);
    assert!(format!("{overlay:?}").contains("no_redirection_bitmap: true"), "{overlay:?}");

    let launcher = WindowOptions::launcher_panel().options.attributes("launcher", (300.0, 100.0), None, false);
    assert!(launcher.transparent);
    assert!(format!("{launcher:?}").contains("no_redirection_bitmap: true"), "{launcher:?}");
}

#[test]
fn a_window_that_fits_its_content_has_no_default_minimum_height() {
    let fit = WindowOptions::default().fit_min_size(None);
    assert_eq!(WindowOptions::default().size_limits(fit), ((420.0, 0.0), None));
    assert_eq!(WindowOptions::default().fit_min_size(Some((300.0, 200.0))), Some((300.0, 200.0)));
    let options = WindowOptions { min_size: Some((300.0, 500.0)), ..Default::default() };
    assert_eq!(options.size_limits(options.fit_min_size(None)), ((300.0, 500.0), None));
}

#[test]
fn forwarded_window_spec_builders_and_map_app_configure_the_inner_app() {
    struct DummyCompositor;
    impl crate::compose::Compositor<()> for DummyCompositor {
        fn render(&mut self, f: &mut crate::compose::Frame<'_, ()>) { f.draw_ui(); }
    }
    fn stats() -> String { "stats".into() }
    let spec = AppSpec::new("secondary", (300.0, 200.0), || (), |_, _| crate::el());
    let window = WindowSpec::new("secondary", spec)
        .srgb_blend(false)
        .font_smoothing(crate::FontSmoothing::Off)
        .max_native_materials(8)
        .fit_content_height()
        .float_on_open()
        .inactive()
        .maximized(true)
        .full_screen(true)
        .modifier_keys()
        .media_keys()
        .tab_visits_controls(true)
        .release_gpu_when_closed()
        .on_focus(|_, _, _| {})
        .on_modifiers(|_, _, _| {})
        .on_visibility(|_, _, _| {})
        .on_rest(|_, _| false)
        .on_ime(|_, _, _| false)
        .on_field_menu(|_, _, _| {})
        .on_close_request(|_, _| crate::CloseResponse::Close)
        .on_close(|_, _| {})
        .on_quit(|_, _| {})
        .turn_scope(|_| {}, |_| {})
        .on_reopen(|_, _, _| false)
        .on_open_url(|_, _, _| {})
        .on_open_files(|_, _, _| {})
        .on_window(|_| {})
        .on_ready(|_| {})
        .on_display_close(|| {})
        .on_frame_encoded(|_| {})
        .on_frame_presented(|_| {})
        .on_frame_drawn(|_| {})
        .compositor(DummyCompositor)
        .dev_stats(stats)
        .map_app(|app| app.keep_running());
    assert!(!window.app.runtime.srgb_blend);
    assert_eq!(window.app.runtime.font_smoothing, crate::FontSmoothing::Off);
    assert_eq!(window.app.runtime.max_native_materials, 8);
    assert!(window.app.runtime.modifier_keys && window.app.runtime.media_keys && window.app.runtime.tab_visits_controls);
    assert!(window.app.runtime.on_focus.is_some() && window.app.runtime.on_mods.is_some());
    assert!(window.app.runtime.on_visibility.is_some() && window.app.runtime.on_ime.is_some() && window.app.runtime.on_field_menu.is_some());
    assert_eq!(window.app.runtime.dev_stats.map(|f| f()).as_deref(), Some("stats"));
    assert!(window.app.fit_content_height && window.app.float_on_open && window.app.open_inactive);
    assert!(window.app.open_as.maximizes() && window.app.open_as.full_screens());
    assert!(window.app.release_gpu && window.app.keep_running);
    assert!(window.app.on_rest.is_some() && window.app.on_close_request.is_some() && window.app.on_close.is_some() && window.app.on_quit.is_some());
    assert!(window.app.turn_scope.is_some() && window.app.on_reopen.is_some() && window.app.on_open_url.is_some() && window.app.on_open_files.is_some());
    assert!(window.app.on_window.is_some() && window.app.on_ready.is_some() && window.app.on_display_close.is_some());
    assert!(window.app.on_frame_encoded.is_some() && window.app.on_frame_presented.is_some() && window.app.on_frame_drawn.is_some());
    assert!(window.app.compositor.is_some());

    let clamped = WindowSpec::new("clamped", AppSpec::new("clamped", (100.0, 100.0), || (), |_, _| crate::el()))
        .max_native_materials(usize::MAX);
    assert_eq!(clamped.app.runtime.max_native_materials, 64);
    let zero = WindowSpec::new("zero", AppSpec::new("zero", (100.0, 100.0), || (), |_, _| crate::el()))
        .max_native_materials(0);
    assert_eq!(zero.app.runtime.max_native_materials, 0);
    let overlay = crate::OverlaySpec::new("notes", (300.0, 400.0), |_: &(), _| crate::el())
        .options(crate::OverlayOptions::default())
        .max_native_materials(10);
    assert_eq!(overlay.runtime.max_native_materials, 10);
    let overlay_clamped = crate::OverlaySpec::new("notes-max", (300.0, 400.0), |_: &(), _| crate::el())
        .options(crate::OverlayOptions::default())
        .max_native_materials(usize::MAX);
    assert_eq!(overlay_clamped.runtime.max_native_materials, 64);
}

#[test]
fn display_work_area_falls_back_to_full_bounds_for_headless_displays() {
    let primary = Display {
        id: None,
        name: Some("headless".into()),
        position: (0, 0),
        size: (1920, 1080),
        scale: 1.0,
        primary: true,
    };
    assert_eq!(primary.work_area(), ((0, 0), (1920, 1080)));

    let secondary = Display {
        id: None,
        name: None,
        position: (-1920, -240),
        size: (2560, 1440),
        scale: 1.5,
        primary: false,
    };
    assert_eq!(secondary.work_area(), ((-1920, -240), (2560, 1440)));

    let empty = Display {
        id: None,
        name: None,
        position: (0, 0),
        size: (0, 0),
        scale: 1.0,
        primary: false,
    };
    assert_eq!(empty.work_area(), ((0, 0), (0, 0)));
}

#[cfg(target_os = "windows")]
#[test]
fn win32_rect_to_work_area_handles_all_four_edges_negative_origins_and_clamping() {
    use crate::platform::win32::rect_to_work_area;
    assert_eq!(rect_to_work_area(0, 0, 1920, 1032), ((0, 0), (1920, 1032)));
    assert_eq!(rect_to_work_area(0, 48, 1920, 1080), ((0, 48), (1920, 1032)));
    assert_eq!(rect_to_work_area(64, 0, 1920, 1080), ((64, 0), (1856, 1080)));
    assert_eq!(rect_to_work_area(0, 0, 1856, 1080), ((0, 0), (1856, 1080)));
    assert_eq!(rect_to_work_area(-1856, -200, 0, 832), ((-1856, -200), (1856, 1032)));
    assert_eq!(rect_to_work_area(100, 200, 50, 150), ((100, 200), (0, 0)));
    assert_eq!(rect_to_work_area(i32::MIN, i32::MIN, i32::MAX, i32::MAX), ((i32::MIN, i32::MIN), (u32::MAX, u32::MAX)));
}

#[test]
fn windows_window_options_defaults_and_builders() {
    let defaults = WindowsWindowOptions::default();
    assert_eq!(defaults.exclude_from_capture, None);
    assert!(!defaults.no_activate);
    assert!(!defaults.tool_window);
    assert_eq!(defaults.undecorated_shadow, None);

    let configured = WindowsWindowOptions::default()
        .exclude_from_capture(true)
        .no_activate(true)
        .tool_window(true)
        .undecorated_shadow(true);
    assert_eq!(configured.exclude_from_capture, Some(true));
    assert!(configured.no_activate);
    assert!(configured.tool_window);
    assert_eq!(configured.undecorated_shadow, Some(true));

    let cleared = configured
        .exclude_from_capture(false)
        .no_activate(false)
        .tool_window(false)
        .undecorated_shadow(false);
    assert_eq!(cleared.exclude_from_capture, Some(false));
    assert!(!cleared.no_activate);
    assert!(!cleared.tool_window);
    assert_eq!(cleared.undecorated_shadow, Some(false));
}

#[test]
fn configured_options_chains_windows_and_macos_without_clobbering() {
    let from_windows = WindowOptions { borderless: true, ..Default::default() }
        .windows(WindowsWindowOptions::default().undecorated_shadow(true).tool_window(true))
        .macos(|mac| mac.shadow(false).exclude_from_capture(true));
    assert!(from_windows.options.borderless);
    assert_eq!(from_windows.windows.undecorated_shadow, Some(true));
    assert!(from_windows.windows.tool_window && !from_windows.windows.no_activate);
    assert_eq!(from_windows.native.shadow, Some(false));
    assert_eq!(from_windows.native.exclude_from_capture, Some(true));

    let from_launcher = WindowOptions::launcher_panel()
        .windows(|win| win.no_activate(true).exclude_from_capture(true));
    assert!(from_launcher.native.launcher && from_launcher.native.nonactivating_panel);
    assert!(from_launcher.windows.no_activate);
    assert_eq!(from_launcher.windows.exclude_from_capture, Some(true));

    let plain: ConfiguredWindowOptions = WindowOptions::default().into();
    assert_eq!(plain.windows, WindowsWindowOptions::default());
}

#[test]
fn app_and_window_spec_forward_windows_window_and_exclude_from_capture() {
    let spec = || AppSpec::new("panel", (320.0, 200.0), || (), |_, _| crate::el());
    let win_opts = WindowsWindowOptions::default()
        .no_activate(true)
        .tool_window(true)
        .undecorated_shadow(true);

    let app = App::new(spec())
        .windows_window(win_opts.clone())
        .exclude_from_capture(true);
    assert_eq!(app.windows_window, win_opts);
    assert_eq!(app.exclude_from_capture, Some(true));

    let window = WindowSpec::new("panel", spec())
        .windows_window(win_opts.clone())
        .exclude_from_capture(true);
    assert_eq!(window.app.windows_window, win_opts);
    assert_eq!(window.app.exclude_from_capture, Some(true));

    // Replacing WindowOptions via WindowSpec::options resets platform-specific native options
    // (matching macos_window) while preserving portable App::exclude_from_capture.
    let reset = window.options(WindowOptions::default());
    assert_eq!(reset.app.windows_window, WindowsWindowOptions::default());
    assert_eq!(reset.app.exclude_from_capture, Some(true));

    let configured = WindowSpec::new("configured", spec()).configured_options(
        WindowOptions { borderless: true, ..Default::default() }
            .windows(win_opts.clone()),
    );
    assert_eq!(configured.app.windows_window, win_opts);
    assert!(configured.options.borderless);
}

#[test]
fn portable_exclude_from_capture_overrides_platform_options_and_falls_back_when_unset() {
    for (portable, platform, expected) in [
        (None, None, None),
        (None, Some(false), Some(false)),
        (None, Some(true), Some(true)),
        (Some(false), None, Some(false)),
        (Some(false), Some(false), Some(false)),
        (Some(false), Some(true), Some(false)),
        (Some(true), None, Some(true)),
        (Some(true), Some(false), Some(true)),
        (Some(true), Some(true), Some(true)),
    ] {
        let win = match platform {
            Some(v) => WindowsWindowOptions::default().exclude_from_capture(v),
            None => WindowsWindowOptions::default(),
        };
        assert_eq!(win.effective_exclude_from_capture(portable), expected);

        let mac = match platform {
            Some(v) => MacWindowOptions::default().exclude_from_capture(v),
            None => MacWindowOptions::default(),
        };
        assert_eq!(mac.with_portable_exclude(portable).exclude_from_capture, expected);
    }
}

#[test]
fn reveal_focus_skips_activation_for_no_activate_windows() {
    use crate::window::host::lifecycle::reveal_focus;
    use crate::window::state::Reveal;

    // Ordinary window activates on Open, not on Show.
    assert!(reveal_focus(Reveal::Open, false, false, false));
    assert!(reveal_focus(Reveal::Open, false, true, false));
    assert!(!reveal_focus(Reveal::Show, false, false, false));
    // open_inactive suppresses first open only.
    assert!(!reveal_focus(Reveal::Open, true, false, false));
    assert!(reveal_focus(Reveal::Open, true, true, false));
    // no_activate suppresses activation on every reveal, including subsequent opens.
    assert!(!reveal_focus(Reveal::Open, false, false, true));
    assert!(!reveal_focus(Reveal::Open, false, true, true));
    assert!(!reveal_focus(Reveal::Open, true, true, true));
    assert!(!reveal_focus(Reveal::Show, false, true, true));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_undecorated_shadow_applies_to_attributes_and_preserves_default() {
    let base = WindowOptions { borderless: true, ..Default::default() }
        .attributes("panel", (320.0, 200.0), None, false);
    let unset = WindowsWindowOptions::default().apply_attributes(base.clone());
    assert!(format!("{unset:?}").contains("decoration_shadow: false"), "{unset:?}");
    assert_eq!(unset.inner_size, Some(Size::Logical(LogicalSize::new(320.0, 200.0))));

    let enabled = WindowsWindowOptions::default()
        .undecorated_shadow(true)
        .apply_attributes(base.clone());
    assert!(format!("{enabled:?}").contains("decoration_shadow: true"), "{enabled:?}");
    assert_eq!(enabled.inner_size, Some(Size::Logical(LogicalSize::new(320.0, 200.0))));

    let disabled = WindowsWindowOptions::default()
        .undecorated_shadow(false)
        .apply_attributes(base);
    assert!(format!("{disabled:?}").contains("decoration_shadow: false"), "{disabled:?}");
}

#[test]
fn resize_edge_maps_to_winit_direction_and_default_cursors() {
    use crate::Cursor;
    use winit::window::ResizeDirection;
    let cases = [
        (ResizeEdge::North, ResizeDirection::North, Cursor::ResizeNs),
        (ResizeEdge::South, ResizeDirection::South, Cursor::ResizeNs),
        (ResizeEdge::East, ResizeDirection::East, Cursor::ResizeEw),
        (ResizeEdge::West, ResizeDirection::West, Cursor::ResizeEw),
        (ResizeEdge::NorthEast, ResizeDirection::NorthEast, Cursor::ResizeNesw),
        (ResizeEdge::NorthWest, ResizeDirection::NorthWest, Cursor::ResizeNwse),
        (ResizeEdge::SouthEast, ResizeDirection::SouthEast, Cursor::ResizeNwse),
        (ResizeEdge::SouthWest, ResizeDirection::SouthWest, Cursor::ResizeNesw),
    ];
    for (edge, direction, cursor) in cases {
        assert_eq!(ResizeDirection::from(edge), direction);
        assert_eq!(edge.cursor(), cursor);
    }
}

#[test]
fn window_options_attributes_with_theme_sets_preferred_theme_without_changing_default() {
    use winit::window::Theme;
    let default_attrs = WindowOptions::default().attributes("app", (640.0, 480.0), None, true);
    assert_eq!(default_attrs.preferred_theme, None);

    let system = WindowOptions::default().attributes_with_theme(
        "app",
        (640.0, 480.0),
        None,
        true,
        crate::ColorScheme::System.winit_theme(),
    );
    assert_eq!(system.preferred_theme, None);

    let light = WindowOptions::default().attributes_with_theme(
        "app",
        (640.0, 480.0),
        None,
        true,
        crate::ColorScheme::Light.winit_theme(),
    );
    assert_eq!(light.preferred_theme, Some(Theme::Light));

    let dark = WindowOptions::default().attributes_with_theme(
        "app",
        (640.0, 480.0),
        None,
        true,
        crate::ColorScheme::Dark.winit_theme(),
    );
    assert_eq!(dark.preferred_theme, Some(Theme::Dark));
}

#[test]
fn app_and_window_spec_default_to_system_and_preserve_color_scheme() {
    let spec = || AppSpec::new("notes", (480.0, 360.0), || (), |_, _| crate::el());
    assert_eq!(App::new(spec()).color_scheme, crate::ColorScheme::System);

    let app = App::new(spec()).color_scheme(crate::ColorScheme::Dark);
    assert_eq!(app.color_scheme, crate::ColorScheme::Dark);
    let from_app = WindowSpec::from_app("notes", app);
    assert_eq!(from_app.app.color_scheme, crate::ColorScheme::Dark);

    let window = WindowSpec::new("notes", spec()).color_scheme(crate::ColorScheme::Light);
    assert_eq!(window.app.color_scheme, crate::ColorScheme::Light);
    // Replacing WindowOptions resets platform-specific native structs while keeping portable color_scheme.
    let reset = window.options(WindowOptions::default());
    assert_eq!(reset.app.color_scheme, crate::ColorScheme::Light);
}

#[test]
fn app_and_window_spec_cpu_first_default_auto_and_retain_builder_value() {
    let spec = || AppSpec::new("plan", (640.0, 480.0), || (), |_, _| crate::el());
    // Default is Auto on both App and WindowSpec.
    assert_eq!(App::new(spec()).cpu_first, crate::CpuFirst::Auto);
    assert_eq!(WindowSpec::new("plan", spec()).app.cpu_first, crate::CpuFirst::Auto);

    // App::cpu_first stores Only/Off/Auto and survives WindowSpec::from_app.
    let app_only = App::new(spec()).cpu_first(crate::CpuFirst::Only);
    assert_eq!(app_only.cpu_first, crate::CpuFirst::Only);
    let from_app = WindowSpec::from_app("plan", app_only);
    assert_eq!(from_app.app.cpu_first, crate::CpuFirst::Only);

    // WindowSpec::cpu_first forwards and overrides per window, and survives options() replacement.
    let win_off = WindowSpec::new("gpu", spec()).cpu_first(crate::CpuFirst::Off);
    assert_eq!(win_off.app.cpu_first, crate::CpuFirst::Off);
    let reset = win_off.options(WindowOptions::default());
    assert_eq!(reset.app.cpu_first, crate::CpuFirst::Off);
    let back_to_auto = reset.cpu_first(crate::CpuFirst::Auto);
    assert_eq!(back_to_auto.app.cpu_first, crate::CpuFirst::Auto);

    // Literal construction of WindowOptions stays unchanged.
    let _literal = WindowOptions {
        min_size: None,
        max_size: None,
        resizable: true,
        borderless: false,
        transparent: false,
        visible: true,
        always_on_top: false,
        click_through: false,
        position: None,
        physical_size: None,
    };
}

#[test]
fn mac_window_options_document_and_first_mouse_defaults_and_retention() {
    let default = MacWindowOptions::default();
    assert_eq!(default.accepts_first_mouse, None);
    assert_eq!(default.represented_file, None);
    assert!(!default.title_from_file);
    assert!(!default.document_edited);
    assert!(!default.show_title);

    // Empty represented_file normalizes to None.
    assert_eq!(MacWindowOptions::default().represented_file("").represented_file, None);

    let opts = MacWindowOptions::default()
        .accepts_first_mouse(false)
        .represented_file("/tmp/rune-notes/groceries.md")
        .title_from_file(true)
        .document_edited(true)
        .show_title(true);
    assert_eq!(opts.accepts_first_mouse, Some(false));
    assert_eq!(opts.represented_file.as_deref(), Some(std::path::Path::new("/tmp/rune-notes/groceries.md")));
    assert!(opts.title_from_file);
    assert!(opts.document_edited);
    assert!(opts.show_title);

    let spec = || AppSpec::new("notes", (480.0, 360.0), || (), |_, _| crate::el());
    let app = App::new(spec()).macos_window(opts.clone());
    assert_eq!(app.macos_window.accepts_first_mouse, Some(false));
    assert_eq!(app.macos_window.represented_file, opts.represented_file);
    assert!(app.macos_window.title_from_file && app.macos_window.document_edited && app.macos_window.show_title);

    let from_app = WindowSpec::from_app("notes", app);
    assert_eq!(from_app.app.macos_window.accepts_first_mouse, Some(false));
    assert_eq!(from_app.app.macos_window.represented_file, opts.represented_file);

    let via_spec = WindowSpec::new("doc", spec()).macos_window(opts.clone());
    assert_eq!(via_spec.app.macos_window.accepts_first_mouse, Some(false));
    assert!(via_spec.app.macos_window.show_title);

    let configured = WindowSpec::new("cfg", spec()).configured_options(
        WindowOptions::default().macos(opts.clone()),
    );
    assert_eq!(configured.app.macos_window.accepts_first_mouse, Some(false));
    assert_eq!(configured.app.macos_window.represented_file, opts.represented_file);
    assert!(configured.app.macos_window.title_from_file);
    assert!(configured.app.macos_window.document_edited);
    assert!(configured.app.macos_window.show_title);

    // Amending launcher_panel keeps launcher behavior while setting accepts_first_mouse.
    let amended = WindowOptions::launcher_panel().macos(|m| m.accepts_first_mouse(false));
    assert!(amended.native.launcher && amended.native.nonactivating_panel);
    assert_eq!(amended.native.accepts_first_mouse, Some(false));

    // Runtime document overlay keeps other MacWindowOptions fields intact and applies updates/clears.
    let updated_path = Some(std::path::PathBuf::from("/tmp/rune-notes/ideas.md"));
    let overlaid = opts.with_runtime_document(Some(&updated_path), Some(&false));
    assert_eq!(overlaid.represented_file, updated_path);
    assert!(!overlaid.document_edited);
    assert_eq!(overlaid.accepts_first_mouse, Some(false));
    assert!(overlaid.title_from_file && overlaid.show_title);

    let cleared = opts.with_runtime_document(Some(&None), None);
    assert_eq!(cleared.represented_file, None);
    assert!(cleared.document_edited);
}

#[test]
fn represented_file_title_and_dir_helpers_handle_edges() {
    use std::path::Path;
    assert_eq!(represented_file_title(Path::new("")), None);
    assert_eq!(represented_file_title(Path::new("/tmp/notes/groceries.md")).as_deref(), Some("groceries.md"));
    assert_eq!(represented_file_title(Path::new("/tmp/notes/")).as_deref(), Some("notes"));
    assert_eq!(represented_file_title(Path::new("draft.txt")).as_deref(), Some("draft.txt"));
    assert_eq!(represented_file_title(Path::new("/")).as_deref(), Some("/"));
    assert_eq!(represented_file_title(Path::new("/tmp/My Notes (日本語).md")).as_deref(), Some("My Notes (日本語).md"));

    assert!(!represented_path_is_dir(Path::new("")));
    assert!(!represented_path_is_dir(Path::new("/tmp/notes.md")));
    assert!(represented_path_is_dir(Path::new("/tmp/notes/")));
    assert!(represented_path_is_dir(Path::new("/")));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_window_options_apply_attributes_preserves_defaults_and_applies_overrides() {
    let base = WindowOptions::default().attributes("notes", (520.0, 320.0), None, true);

    // Omitted options keep winit's accepts_first_mouse = true default and hide title under full_size_content.
    let default_full = MacWindowOptions::default().apply_attributes(base.clone(), false, true);
    let dbg = format!("{default_full:?}");
    assert!(dbg.contains("accepts_first_mouse: true"), "{dbg}");
    assert!(dbg.contains("titlebar_transparent: true"), "{dbg}");
    assert!(dbg.contains("title_hidden: true"), "{dbg}");
    assert!(dbg.contains("fullsize_content_view: true"), "{dbg}");

    // show_title(true) keeps the native title and proxy icon visible under full_size_content.
    let shown = MacWindowOptions::default()
        .show_title(true)
        .apply_attributes(base.clone(), false, true);
    let dbg = format!("{shown:?}");
    assert!(dbg.contains("accepts_first_mouse: true"), "{dbg}");
    assert!(dbg.contains("titlebar_transparent: true"), "{dbg}");
    assert!(dbg.contains("title_hidden: false"), "{dbg}");
    assert!(dbg.contains("fullsize_content_view: true"), "{dbg}");

    // accepts_first_mouse(false) applies to full-size-content, standard titled, and borderless windows.
    for (borderless, full_size_content) in [(false, true), (false, false), (true, false), (true, true)] {
        let attrs = WindowOptions { borderless, ..Default::default() }
            .attributes("doc", (400.0, 300.0), None, true);
        let applied = MacWindowOptions::default()
            .accepts_first_mouse(false)
            .show_title(true)
            .apply_attributes(attrs, borderless, full_size_content);
        let dbg = format!("{applied:?}");
        assert!(dbg.contains("accepts_first_mouse: false"), "borderless={borderless} full_size={full_size_content}: {dbg}");
        if !borderless && full_size_content {
            assert!(dbg.contains("fullsize_content_view: true"), "{dbg}");
            assert!(dbg.contains("title_hidden: false"), "{dbg}");
        } else {
            assert!(dbg.contains("fullsize_content_view: false"), "{dbg}");
        }
    }

    // Explicit accepts_first_mouse(true) sets true.
    let explicit_true = MacWindowOptions::default()
        .accepts_first_mouse(true)
        .apply_attributes(base, false, false);
    assert!(format!("{explicit_true:?}").contains("accepts_first_mouse: true"));
}

#[test]
fn mac_window_options_color_space_defaults_to_srgb_and_retains_override() {
    assert_eq!(MacColorSpace::default(), MacColorSpace::Srgb);
    assert_eq!(MacWindowOptions::default().color_space, MacColorSpace::Srgb);

    let unmanaged = MacWindowOptions::default().color_space(MacColorSpace::Unmanaged);
    assert_eq!(unmanaged.color_space, MacColorSpace::Unmanaged);
    assert_eq!(unmanaged.clone().color_space(MacColorSpace::Srgb).color_space, MacColorSpace::Srgb);

    let spec = || AppSpec::new("color", (480.0, 360.0), || (), |_, _| crate::el());
    let app = App::new(spec()).macos_window(unmanaged.clone());
    assert_eq!(app.macos_window.color_space, MacColorSpace::Unmanaged);

    let from_app = WindowSpec::from_app("color", app);
    assert_eq!(from_app.app.macos_window.color_space, MacColorSpace::Unmanaged);

    let via_spec = WindowSpec::new("color", spec()).macos_window(unmanaged.clone());
    assert_eq!(via_spec.app.macos_window.color_space, MacColorSpace::Unmanaged);
    // Replacing WindowOptions via WindowSpec::options resets MacWindowOptions back to defaults.
    assert_eq!(via_spec.options(WindowOptions::default()).app.macos_window.color_space, MacColorSpace::Srgb);

    let configured = WindowSpec::new("cfg", spec()).configured_options(
        WindowOptions::default().macos(unmanaged.clone()),
    );
    assert_eq!(configured.app.macos_window.color_space, MacColorSpace::Unmanaged);

    let amended = WindowOptions::launcher_panel().macos(|m| m.color_space(MacColorSpace::Unmanaged));
    assert!(amended.native.launcher && amended.native.nonactivating_panel);
    assert_eq!(amended.native.color_space, MacColorSpace::Unmanaged);

    assert_eq!(unmanaged.with_portable_exclude(Some(true)).color_space, MacColorSpace::Unmanaged);
    assert_eq!(unmanaged.with_runtime_document(None, Some(&true)).color_space, MacColorSpace::Unmanaged);
}
