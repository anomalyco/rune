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
        .fit_content_height()
        .float_on_open()
        .inactive()
        .maximized(true)
        .full_screen(true)
        .modifier_keys()
        .tab_visits_controls(true)
        .release_gpu_when_closed()
        .on_focus(|_, _, _| {})
        .on_modifiers(|_, _, _| {})
        .on_visibility(|_, _, _| {})
        .on_rest(|_, _| false)
        .on_ime(|_, _, _| false)
        .on_close(|_, _| {})
        .on_quit(|_, _| {})
        .turn_scope(|_| {}, |_| {})
        .on_reopen(|_, _, _| false)
        .on_open_url(|_, _, _| {})
        .on_window(|_| {})
        .on_ready(|_| {})
        .on_frame_encoded(|_| {})
        .on_frame_drawn(|_| {})
        .compositor(DummyCompositor)
        .dev_stats(stats)
        .map_app(|app| app.keep_running());
    assert!(!window.app.runtime.srgb_blend);
    assert_eq!(window.app.runtime.font_smoothing, crate::FontSmoothing::Off);
    assert!(window.app.runtime.modifier_keys && window.app.runtime.tab_visits_controls);
    assert!(window.app.runtime.on_focus.is_some() && window.app.runtime.on_mods.is_some());
    assert!(window.app.runtime.on_visibility.is_some() && window.app.runtime.on_ime.is_some());
    assert_eq!(window.app.runtime.dev_stats.map(|f| f()).as_deref(), Some("stats"));
    assert!(window.app.fit_content_height && window.app.float_on_open && window.app.open_inactive);
    assert!(window.app.open_as.maximizes() && window.app.open_as.full_screens());
    assert!(window.app.release_gpu && window.app.keep_running);
    assert!(window.app.on_rest.is_some() && window.app.on_close.is_some() && window.app.on_quit.is_some());
    assert!(window.app.turn_scope.is_some() && window.app.on_reopen.is_some() && window.app.on_open_url.is_some());
    assert!(window.app.on_window.is_some() && window.app.on_ready.is_some());
    assert!(window.app.on_frame_encoded.is_some() && window.app.on_frame_drawn.is_some());
    assert!(window.app.compositor.is_some());
}
