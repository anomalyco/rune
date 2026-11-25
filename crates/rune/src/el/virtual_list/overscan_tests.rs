use super::*;

#[test]
fn defaults_match_existing_constants() {
    let mut v = VirtualList::default();
    assert_eq!(v.overscan_vp(), OVERSCAN);
    assert_eq!(v.edge_vp(), EAGER_EDGE);
    v.indexed = true;
    assert_eq!(v.overscan_vp(), OVERSCAN);
    assert_eq!(v.edge_vp(), INDEXED_EDGE);
}

#[test]
fn clamp_virtual_overscan_handles_normal_zero_and_degenerate_inputs() {
    assert_eq!(clamp_virtual_overscan(2.0, 1.75), Some((2.0, 1.75)));
    assert_eq!(clamp_virtual_overscan(0.5, 0.25), Some((0.5, 0.25)));
    assert_eq!(clamp_virtual_overscan(0.0, 0.0), Some((0.0, 0.0)));
    assert_eq!(clamp_virtual_overscan(0.0, 1.5), Some((0.0, 0.0)));
    assert_eq!(clamp_virtual_overscan(-2.0, -1.0), Some((0.0, 0.0)));
    assert_eq!(clamp_virtual_overscan(-0.0, -0.0), Some((0.0, 0.0)));

    // Negative edge clamps to 0.0 while keeping overscan.
    assert_eq!(clamp_virtual_overscan(0.5, -1.0), Some((0.5, 0.0)));

    // Edge >= overscan clamps strictly below overscan.
    for (o, e) in [(0.5, 0.5), (0.5, 3.0), (0.1, 0.1), (0.02, 0.02), (1e-4, 1e-4), (20.0, 20.0)] {
        let (co, ce) = clamp_virtual_overscan(o, e).expect("finite");
        assert!(co > 0.0 && co <= MAX_OVERSCAN, "overscan {co} out of bounds for ({o}, {e})");
        assert!(ce >= 0.0 && ce < co, "edge {ce} must stay strictly below overscan {co} for ({o}, {e})");
    }

    // Upper cap at MAX_OVERSCAN (8.0).
    let (co, ce) = clamp_virtual_overscan(99.0, 99.0).expect("finite");
    assert_eq!(co, MAX_OVERSCAN);
    assert!(ce < MAX_OVERSCAN);

    // Non-finite values are ignored.
    for (o, e) in [
        (f32::NAN, 0.25),
        (0.5, f32::NAN),
        (f32::NAN, f32::NAN),
        (f32::INFINITY, 0.25),
        (0.5, f32::INFINITY),
        (f32::NEG_INFINITY, 0.0),
        (0.5, f32::NEG_INFINITY),
    ] {
        assert_eq!(clamp_virtual_overscan(o, e), None, "non-finite ({o}, {e}) must return None");
    }
}

#[test]
fn custom_overscan_controls_mounted_range_edge_and_budget() {
    let make = |override_vp: Option<(f32, f32)>| {
        let mut v = VirtualList { viewport: 200.0, indexed: true, ..Default::default() };
        v.set_estimate(20.0);
        v.set_keys((0..500).map(|i| format!("r{i}")).collect(), 0, None);
        v.set_overscan(override_vp);
        v
    };

    let offset = 200.0 * 20.0; // row 200 at top, 10 visible rows (200..210)

    // Default (2.0, 1.75): 2 viewports either side = 5 viewports = 50 rows (180..230).
    let mut def = make(None);
    def.select(offset, false, None);
    assert_eq!(def.range, 180..230);
    assert!(!def.needs_window(offset, false));
    assert!(!def.needs_window(offset + 49.0, false));
    assert!(def.needs_window(offset + 51.0, false), "default triggers after 0.25 vp (50 pt)");

    // Custom (0.5, 0.25): 0.5 viewport either side = 2 viewports = 20 rows (195..215).
    let mut tight = make(clamp_virtual_overscan(0.5, 0.25));
    tight.select(offset, false, None);
    assert_eq!(tight.range, 195..215);
    assert!(!tight.needs_window(offset, false));
    assert!(!tight.needs_window(offset + 49.0, false));
    assert!(tight.needs_window(offset + 51.0, false), "0.5/0.25 triggers after 0.25 vp (50 pt)");
    assert!(!tight.needs_window(offset - 49.0, false));
    assert!(tight.needs_window(offset - 51.0, false));

    // Budget caps new rows per side to INDEXED_BUDGET.min(overscan_vp).
    let empty = HashSet::default();
    let mut small = make(clamp_virtual_overscan(0.25, 0.1));
    small.select(offset, false, Some(&empty));
    // Viewport (10) + 0.25 vp above (2.5 -> 3) + 0.25 vp below (3) = 16 rows max.
    assert!(small.range.len() <= 16, "budget respected smaller overscan: {:?}", small.range);
    assert!(!small.needs_window(offset, false));

    // Viewport-only (0.0, 0.0): mounts exactly the 10 visible rows, never oscillates.
    let mut zero = make(clamp_virtual_overscan(0.0, 0.0));
    zero.select(offset, false, Some(&empty));
    assert_eq!(zero.range, 200..210);
    assert!(!zero.needs_window(offset, false));
    assert!(!zero.viewport_uncovered(offset, false));
    assert_eq!(zero.unfilled(offset, false), 0);
}

#[test]
fn zero_overscan_with_gap_never_oscillates_or_reports_uncovered() {
    let mut v = VirtualList { viewport: 200.0, indexed: true, ..Default::default() };
    v.set_estimate(20.0);
    v.set_gap(6.0);
    v.set_keys((0..200).map(|i| format!("r{i}")).collect(), 0, None);
    v.set_overscan(clamp_virtual_overscan(0.0, 0.0));

    // Probe offsets landing inside rows, on boundaries, and squarely inside gaps.
    for step in 0..80 {
        let offset = 1000.0 + step as f32 * 1.5;
        v.select(offset, false, None);
        assert!(!v.needs_window(offset, false), "must not re-trigger at offset {offset}");
        assert!(!v.viewport_uncovered(offset, false), "must not report uncovered at offset {offset}");
        assert_eq!(v.unfilled(offset, false), 0, "must have 0 unfilled at offset {offset}");
    }
}
