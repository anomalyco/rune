use super::*;

fn spec(edge: ScreenEdge, margin: f32, align: Align, inset: f32, size: (f32, f32)) -> Placement {
    Placement { size, edge, margin, align, inset }
}

fn edge_opt(edge: ScreenEdge, margin: f32, align: Align, inset: f32) -> Option<OverlayPlacement> {
    Some(OverlayPlacement::Edge { edge, margin, align, inset })
}

#[test]
fn bottom_taskbar_matches_for_spec_edge_and_overlay_placement_edge() {
    let display = ((0, 0), (1920, 1080));
    let work = ((0, 0), (1920, 1032)); // 48 px bottom taskbar
    let at = spec(ScreenEdge::Bottom, 16.0, Align::Start, 12.0, (300.0, 80.0));
    let expected = PhysicalPosition::new(12, 1032 - 80 - 16);
    assert_eq!(position_in(display, work, 1.0, at, None), expected);
    assert_eq!(
        position_in(
            display,
            work,
            1.0,
            spec(ScreenEdge::Top, 0.0, Align::Center, 0.0, (300.0, 80.0)),
            edge_opt(ScreenEdge::Bottom, 16.0, Align::Start, 12.0),
        ),
        expected,
        "OverlayPlacement::Edge overrides spec fields and uses the same work area",
    );
    // Align::End clears the bottom taskbar too.
    assert_eq!(
        position_in(display, work, 1.0, at, edge_opt(ScreenEdge::Bottom, 0.0, Align::End, 0.0)),
        PhysicalPosition::new(1620, 952),
    );
}

#[test]
fn top_left_and_right_taskbars_anchor_all_four_sides_of_work_area() {
    let display = ((0, 0), (1920, 1080));
    let size = (300.0, 80.0);

    // 48 px top taskbar.
    let top_bar = ((0, 48), (1920, 1032));
    for placement in [None, edge_opt(ScreenEdge::Top, 16.0, Align::Center, 0.0)] {
        assert_eq!(
            position_in(display, top_bar, 1.0, spec(ScreenEdge::Top, 16.0, Align::Center, 0.0, size), placement),
            PhysicalPosition::new((1920 - 300) / 2, 48 + 16),
        );
    }

    // 64 px left taskbar shifts Start, Center, and End inside [64, 1920).
    let left_bar = ((64, 0), (1856, 1080));
    for (align, expected_x) in [
        (Align::Start, 64 + 12),
        (Align::Center, 64 + (1856 - 300) / 2),
        (Align::End, 1920 - 300 - 12),
    ] {
        let at = spec(ScreenEdge::Top, 10.0, align, 12.0, size);
        assert_eq!(position_in(display, left_bar, 1.0, at, None), PhysicalPosition::new(expected_x, 10));
        assert_eq!(
            position_in(display, left_bar, 1.0, at, edge_opt(ScreenEdge::Top, 10.0, align, 12.0)),
            PhysicalPosition::new(expected_x, 10),
        );
    }

    // 64 px right taskbar pulls End and Center inward inside [0, 1856).
    let right_bar = ((0, 0), (1856, 1080));
    for (align, expected_x) in [
        (Align::Start, 12),
        (Align::Center, (1856 - 300) / 2),
        (Align::End, 1856 - 300 - 12),
    ] {
        let at = spec(ScreenEdge::Bottom, 10.0, align, 12.0, size);
        assert_eq!(position_in(display, right_bar, 1.0, at, None), PhysicalPosition::new(expected_x, 1080 - 80 - 10));
        assert_eq!(
            position_in(display, right_bar, 1.0, at, edge_opt(ScreenEdge::Bottom, 10.0, align, 12.0)),
            PhysicalPosition::new(expected_x, 1080 - 80 - 10),
        );
    }
}

#[test]
fn negative_monitor_origin_and_dpi_scaling_keep_origin_unscaled() {
    // 1.5x monitor left of primary with a 60 px left app bar and a 48 px bottom taskbar.
    let display = ((-1920, -200), (1920, 1080));
    let work = ((-1860, -200), (1860, 1032));
    // 200x60 pt -> 300x90 px; margin 10 pt -> 15 px; inset 8 pt -> 12 px.
    let at = spec(ScreenEdge::Bottom, 10.0, Align::End, 8.0, (200.0, 60.0));
    let expected_end = PhysicalPosition::new(-312, 727);
    assert_eq!(position_in(display, work, 1.5, at, None), expected_end);
    assert_eq!(position_in(display, work, 1.5, at, edge_opt(ScreenEdge::Bottom, 10.0, Align::End, 8.0)), expected_end);

    let top_start = spec(ScreenEdge::Top, 10.0, Align::Start, 8.0, (200.0, 60.0));
    let expected_start = PhysicalPosition::new(-1860 + 12, -200 + 15);
    assert_eq!(position_in(display, work, 1.5, top_start, None), expected_start);
    assert_eq!(
        position_in(display, work, 1.5, top_start, edge_opt(ScreenEdge::Top, 10.0, Align::Start, 8.0)),
        expected_start,
    );
}

#[test]
fn rect_and_fullscreen_stay_in_full_display_coordinates() {
    // Taskbars on all four sides: Rect and FullScreen must ignore rcWork and anchor to display.
    let display = ((-1920, -200), (1920, 1080));
    let work = ((-1856, -152), (1792, 984));
    let at = spec(ScreenEdge::Top, 0.0, Align::Start, 0.0, (200.0, 60.0));

    assert_eq!(
        position_in(display, work, 1.5, at, Some(OverlayPlacement::FullScreen)),
        PhysicalPosition::new(-1920, -200),
    );
    assert_eq!(
        position_in(display, work, 1.5, at, Some(OverlayPlacement::Rect { x: 10.0, y: 20.0, width: 200.0, height: 60.0 })),
        PhysicalPosition::new(-1920 + 15, -200 + 30),
    );
}

#[test]
fn full_display_work_area_preserves_non_windows_placement_and_handles_edge_sizes() {
    let display = ((0, 0), (1920, 1080));
    // When work_area == display (non-Windows or no docked bars), placement matches full monitor.
    let at = spec(ScreenEdge::Bottom, 24.0, Align::End, 12.0, (300.0, 80.0));
    assert_eq!(
        position_in(display, display, 2.0, at, None),
        PhysicalPosition::new(1920 - 600 - 24, 1080 - 160 - 48),
    );

    // Zero overlay size clamps to 1 pt (2 px at 2x), matching local_rect.
    let zero = spec(ScreenEdge::Bottom, 0.0, Align::End, 0.0, (0.0, 0.0));
    assert_eq!(position_in(display, display, 2.0, zero, None), PhysicalPosition::new(1918, 1078));

    // Overlay larger than work area centers cleanly without underflow.
    let huge = spec(ScreenEdge::Bottom, 0.0, Align::Center, 0.0, (2500.0, 1200.0));
    assert_eq!(
        position_in(display, ((64, 0), (1856, 1032)), 1.0, huge, None),
        PhysicalPosition::new(64 + (1856 - 2500) / 2, 1032 - 1200),
    );
}
