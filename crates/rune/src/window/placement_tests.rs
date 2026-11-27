use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, Position, Size};

use super::*;
use crate::window::state::ReopenState;

fn sample_placement(
    position: (i32, i32),
    content_size: (u32, u32),
    scale: f64,
    maximized: bool,
    minimized: bool,
    full_screen: bool,
) -> WindowPlacement {
    WindowPlacement {
        position,
        content_position: position,
        content_size,
        scale,
        maximized,
        minimized,
        full_screen,
        restore: None,
        display: Some(((0, 0), (1920, 1080))),
    }
}

#[test]
fn placement_watch_performs_zero_queries_when_unregistered_or_clean() {
    let mut watch = PlacementWatch::default();
    let mut queries = 0usize;
    watch.mark_dirty();
    for _ in 0..5 {
        assert_eq!(
            watch.poll_if_watched(false, false, || {
                queries += 1;
                Some(sample_placement((10, 20), (800, 600), 1.0, false, false, false))
            }),
            None,
        );
    }
    assert_eq!(queries, 0, "apps without on_placement must never query the native window");

    // Opted-in but clean (no move/resize/state event): still zero native queries on idle turns.
    for _ in 0..5 {
        assert_eq!(
            watch.poll_if_watched(true, false, || {
                queries += 1;
                Some(sample_placement((10, 20), (800, 600), 1.0, false, false, false))
            }),
            None,
        );
    }
    assert_eq!(queries, 0, "clean idle turns must not query the native window");
}

#[test]
fn placement_watch_coalesces_bursts_deduplicates_and_tracks_state_changes() {
    let mut watch = PlacementWatch::default();
    let mut queries = 0usize;

    // Simulate a burst of WindowEvent::Moved notifications before idle.
    for _ in 0..12 {
        watch.mark_dirty();
    }
    let first = sample_placement((100, 200), (1000, 700), 2.0, false, false, false);
    assert_eq!(
        watch.poll_if_watched(true, false, || {
            queries += 1;
            Some(first)
        }),
        Some(first),
    );
    assert_eq!(queries, 1, "burst of move events coalesces into one native query at idle");

    // Redundant move landing on the exact same placement delivers nothing.
    watch.mark_dirty();
    assert_eq!(
        watch.poll_if_watched(true, false, || {
            queries += 1;
            Some(first)
        }),
        None,
    );
    assert_eq!(queries, 2);

    // Move to a new position delivers once.
    watch.mark_dirty();
    let moved = sample_placement((-500, -1300), (1500, 900), 1.0, false, false, false);
    assert_eq!(watch.poll_if_watched(true, false, || Some(moved)), Some(moved));

    // Maximize / minimize / full-screen transitions each deliver once.
    watch.mark_dirty();
    let maximized = sample_placement((-500, -1300), (1500, 900), 1.0, true, false, false);
    assert_eq!(watch.poll_if_watched(true, false, || Some(maximized)), Some(maximized));

    watch.mark_dirty();
    let minimized = sample_placement((-500, -1300), (1500, 900), 1.0, true, true, false);
    assert_eq!(watch.poll_if_watched(true, false, || Some(minimized)), Some(minimized));

    // macOS FullScreenWatch force_dirty catches full-screen flip even if dirty flag was false.
    let full = sample_placement((-500, -1300), (1500, 900), 1.0, false, false, true);
    assert_eq!(watch.poll_if_watched(true, true, || Some(full)), Some(full));
    assert_eq!(watch.last(), Some(full));

    // Wayland (sample returns None) clears dirty and delivers nothing.
    watch.mark_dirty();
    assert_eq!(watch.poll_if_watched(true, false, || None), None);
    assert_eq!(watch.poll_if_watched(true, false, || Some(first)), None);
}

#[test]
fn window_placement_logical_helpers_handle_valid_and_invalid_scales() {
    let p = sample_placement((300, 240), (2200, 1400), 2.0, false, false, false);
    assert_eq!(p.logical_position(), (150.0, 120.0));
    assert_eq!(p.logical_content_size(), (1100.0, 700.0));

    let neg = sample_placement((-500, -1300), (1500, 900), 1.0, false, false, false);
    assert_eq!(neg.logical_position(), (-500.0, -1300.0));
    assert_eq!(neg.logical_content_size(), (1500.0, 900.0));

    for bad_scale in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        let bad = sample_placement((40, 80), (640, 480), bad_scale, false, false, false);
        assert_eq!(bad.logical_position(), (40.0, 80.0));
        assert_eq!(bad.logical_content_size(), (640.0, 480.0));
    }
}

#[test]
fn macos_physical_position_resolves_stacked_mixed_scale_displays_without_main_screen_error() {
    // Issue #2523 setup: 2x built-in display at (0, 0) and 1x external display above at negative y.
    let screens = [
        MacScreenBounds { origin_pt: (0.0, 0.0), size_pt: (1512.0, 982.0), scale: 2.0 },
        MacScreenBounds { origin_pt: (-500.0, -1440.0), size_pt: (2560.0, 1440.0), scale: 1.0 },
    ];

    // Saved on 1x external display while mainScreen (or current window screen) is 2x:
    // must NOT be halved to (-250, -650).
    let ext = resolve_mac_physical_position((-500, -1300), Some((1500, 900)), &screens, 2.0);
    assert_eq!(ext.logical_position, (-500.0, -1300.0));
    assert_eq!(ext.logical_size, Some((1500.0, 900.0)));

    // Saved on 2x built-in display ((150, 120) pt * 2 = (300, 240)) while mainScreen (or current
    // window screen on set_position) is 1x: must NOT be doubled to (300, 240).
    let builtin = resolve_mac_physical_position((300, 240), Some((2200, 1400)), &screens, 1.0);
    assert_eq!(builtin.logical_position, (150.0, 120.0));
    assert_eq!(builtin.logical_size, Some((1100.0, 700.0)));

    // Slightly off the left edge of the 1x external display picks nearest screen (1x).
    let off_edge = resolve_mac_physical_position((-520, -1300), None, &screens, 2.0);
    assert_eq!(off_edge.logical_position, (-520.0, -1300.0));
}

#[test]
fn macos_physical_position_resolves_side_by_side_overlay_even_in_physical_overlap() {
    // 2x built-in: physical [0..3024, 0..1964). 1x external to right: physical [1512..3432, 0..1080).
    // Note (1512, 0) lies inside BOTH physical rects!
    let screens = [
        MacScreenBounds { origin_pt: (0.0, 0.0), size_pt: (1512.0, 982.0), scale: 2.0 },
        MacScreenBounds { origin_pt: (1512.0, 0.0), size_pt: (1920.0, 1080.0), scale: 1.0 },
    ];

    // WindowOptions::overlay(&external_1x) passes both physical origin and physical size, matching
    // the 1x display exactly even in the physical overlap region.
    let overlay = resolve_mac_physical_position((1512, 0), Some((1920, 1080)), &screens, 2.0);
    assert_eq!(overlay.logical_position, (1512.0, 0.0));
    assert_eq!(overlay.logical_size, Some((1920.0, 1080.0)));

    // A bare overlapping coordinate without size falls back to fallback_scale.
    let ambiguous_2x = resolve_mac_physical_position((2000, 500), None, &screens, 2.0);
    assert_eq!(ambiguous_2x.logical_position, (1000.0, 250.0));
    let ambiguous_1x = resolve_mac_physical_position((2000, 500), None, &screens, 1.0);
    assert_eq!(ambiguous_1x.logical_position, (2000.0, 500.0));

    // Empty screen list falls back cleanly.
    let empty = resolve_mac_physical_position((300, 200), None, &[], 2.0);
    assert_eq!(empty.logical_position, (150.0, 100.0));
}

#[test]
fn apply_initial_position_attrs_handles_logical_macos_physical_and_non_macos() {
    let screens = [
        MacScreenBounds { origin_pt: (0.0, 0.0), size_pt: (1512.0, 982.0), scale: 2.0 },
        MacScreenBounds { origin_pt: (-500.0, -1440.0), size_pt: (2560.0, 1440.0), scale: 1.0 },
    ];

    // Explicit logical position always sets Position::Logical directly.
    let opts = WindowOptions::default();
    let attrs = opts.attributes("Test", (640.0, 480.0), None, true);
    let attrs = apply_initial_position_attrs(attrs, Some((2000.0, 500.0)), &opts, Some((&screens, 2.0)));
    assert_eq!(attrs.position, Some(Position::Logical(LogicalPosition::new(2000.0, 500.0))));

    // macOS physical WindowOptions::position resolves through target screen scale -> Position::Logical.
    let opts = WindowOptions {
        position: Some((-500, -1300)),
        physical_size: Some((1500, 900)),
        ..Default::default()
    };
    let attrs = opts.attributes("Test", (640.0, 480.0), None, true);
    let attrs = apply_initial_position_attrs(attrs, None, &opts, Some((&screens, 2.0)));
    assert_eq!(attrs.position, Some(Position::Logical(LogicalPosition::new(-500.0, -1300.0))));
    assert_eq!(attrs.inner_size, Some(Size::Logical(LogicalSize::new(1500.0, 900.0))));

    // Non-macOS leaves PhysicalPosition untouched.
    let attrs = opts.attributes("Test", (640.0, 480.0), None, true);
    let attrs = apply_initial_position_attrs(attrs, None, &opts, None);
    assert_eq!(attrs.position, Some(Position::Physical(PhysicalPosition::new(-500, -1300))));
}

#[test]
fn placement_to_reopen_state_uses_native_coordinate_space_per_platform() {
    // macOS side-by-side overlap: same physical (2000, 500), different scales -> unambiguous logical points!
    let on_2x = sample_placement((2000, 500), (2200, 1400), 2.0, false, false, false);
    assert_eq!(
        placement_to_reopen_state(on_2x, true, false),
        (
            Some(Position::Logical(LogicalPosition::new(1000.0, 250.0))),
            Some(RestoredSize::Logical(1100.0, 700.0)),
        ),
    );
    let on_1x = sample_placement((2000, 500), (1500, 900), 1.0, false, false, false);
    assert_eq!(
        placement_to_reopen_state(on_1x, true, false),
        (
            Some(Position::Logical(LogicalPosition::new(2000.0, 500.0))),
            Some(RestoredSize::Logical(1500.0, 900.0)),
        ),
    );

    // Windows maximized with OS restore frame: uses restore frame in physical pixels.
    let mut win_max = sample_placement((0, 0), (3840, 2100), 2.0, true, false, false);
    win_max.restore = Some(((1920 + 80, 120), (1200, 800)));
    assert_eq!(
        placement_to_reopen_state(win_max, false, false),
        (
            Some(Position::Physical(PhysicalPosition::new(2000, 120))),
            Some(RestoredSize::Physical(1200, 800)),
        ),
    );

    // Wayland: ignores position, keeps logical size.
    assert_eq!(
        placement_to_reopen_state(on_2x, false, true),
        (None, Some(RestoredSize::Logical(1100.0, 700.0))),
    );

    // Default (zero size): leaves size None so AppSpec::size is kept.
    let zero = WindowPlacement::default();
    assert_eq!(placement_to_reopen_state(zero, true, false).1, None);
    assert_eq!(placement_to_reopen_state(zero, false, false).1, None);

    // ReopenState::seed_initial feeds into create_plan cleanly.
    let mut reopen = ReopenState::new(OpenAs::Maximized);
    let (pos, size) = placement_to_reopen_state(on_2x, true, false);
    reopen.seed_initial(pos, size);
    let plan = reopen.create_plan(true, false);
    assert_eq!(plan.open_as, OpenAs::Maximized);
    assert_eq!(plan.position, Some(Position::Logical(LogicalPosition::new(1000.0, 250.0))));
    assert_eq!(plan.size, Some(RestoredSize::Logical(1100.0, 700.0)));
}

#[test]
fn builders_preserve_hooks_positions_and_override_precedence() {
    let make = || crate::App::new(crate::AppSpec::new("Place", (640.0, 480.0), || (), |_, _| crate::el()));

    // Default opt-in: no hook, no logical position, no initial placement.
    let app = make();
    assert!(app.placement.on_placement.is_none());
    assert!(app.placement.logical_position.is_none());
    assert!(app.placement.initial_placement.is_none());

    // App -> WindowSpec::from_app preserves on_placement, logical_position, and window_placement.
    let saved = sample_placement((-500, -1300), (1500, 900), 1.0, true, true, false);
    let app = make().window_placement(saved).on_placement(|_, _, _| false);
    assert_eq!(app.open_as, OpenAs::Maximized);
    assert!(app.placement.on_placement.is_some());
    assert_eq!(app.placement.initial_placement, Some(saved));
    let spec = crate::WindowSpec::from_app("main", app);
    assert!(spec.app.placement.on_placement.is_some());
    assert_eq!(spec.app.placement.initial_placement, Some(saved));

    // Precedence: logical_position overrides WindowOptions::position, and later WindowOptions::position
    // overrides logical_position, while WindowOptions { position: None, .. } keeps logical_position.
    let app = make()
        .window_options(WindowOptions { position: Some((10, 20)), ..Default::default() })
        .logical_position((-500.0, -1300.0))
        .window_options(WindowOptions { transparent: true, ..Default::default() });
    assert_eq!(app.window_options.position, None);
    assert_eq!(app.placement.logical_position, Some((-500.0, -1300.0)));

    let app = app.window_options(WindowOptions { position: Some((30, 40)), ..Default::default() });
    assert_eq!(app.window_options.position, Some((30, 40)));
    assert_eq!(app.placement.logical_position, None);

    // WindowSpec builders mirror App builders.
    let spec = crate::WindowSpec::new("second", crate::AppSpec::new("Second", (640.0, 480.0), || (), |_, _| crate::el()))
        .options(WindowOptions { position: Some((1, 2)), ..Default::default() })
        .logical_position((150.0, 120.0))
        .on_placement(|_, _, _| false);
    assert_eq!(spec.options.position, None);
    assert_eq!(spec.app.placement.logical_position, Some((150.0, 120.0)));
    assert!(spec.app.placement.on_placement.is_some());
}

#[test]
fn handle_set_logical_position_queues_and_supersedes_physical_position_and_place() {
    let handle = crate::WindowHandle::new("main");
    handle.set_logical_position((-500.0, -1300.0));
    let taken = handle.take_headless();
    assert!(matches!(taken[..], [crate::window::WakeEvent::LogicalPosition(0, (-500.0, -1300.0))]));

    let app = crate::App::new(crate::AppSpec::new("Place", (640.0, 480.0), || (), |_, _| crate::el()));
    let app_handle = app.handle();
    app_handle.set_position((10, 20));
    app_handle.set_logical_position((150.0, 120.0));
    let req = app.shared.take();
    assert_eq!(req.position, None);
    assert_eq!(req.logical_position, Some((150.0, 120.0)));
    assert!(req.place.is_none());

    app_handle.set_logical_position((150.0, 120.0));
    app_handle.set_position((30, 40));
    let req = app.shared.take();
    assert_eq!(req.position, Some((30, 40)));
    assert_eq!(req.logical_position, None);
}
