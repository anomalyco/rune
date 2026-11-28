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

#[test]
fn viewport_uncovered_handles_pinned_gaps_padding_and_empty_ranges() {
    // 1) Pinned list with top edge in a gap (60 rows * 20 + 59 * 10 = 1790; viewport 1015 ->
    //    max_offset 775 inside gap [770, 780) before row 26).
    let mut v = VirtualList { viewport: 1015.0, indexed: true, ..Default::default() };
    v.set_estimate(20.0);
    v.set_gap(10.0);
    v.set_keys((0..60).map(|i| format!("r{i}")).collect(), 0, None);
    v.set_overscan(clamp_virtual_overscan(0.0, 0.0));
    v.select(0.0, true, None);
    assert_eq!(v.range, 26..60);
    assert!(!v.viewport_uncovered(0.0, true), "top edge in gap is covered when pinned");
    // When a mounted row shrinks past the gap, row 25 becomes visible and uncovered.
    v.set_height("r30", 10.0);
    assert!(v.viewport_uncovered(0.0, true), "shrinking a row past the gap must report uncovered");

    // 2) Pinned list with pad_bottom and zero overscan.
    let mut vp = VirtualList { viewport: 700.0, pad_bottom: 20.0, indexed: true, ..Default::default() };
    vp.set_estimate(20.0);
    vp.set_keys((0..50).map(|i| format!("r{i}")).collect(), 0, None);
    vp.set_overscan(clamp_virtual_overscan(0.0, 0.0));
    vp.select(0.0, true, None);
    assert_eq!(vp.range, 16..50);
    assert!(!vp.viewport_uncovered(0.0, true), "pad_bottom fills the bottom without making top uncovered");
    vp.set_height("r20", 15.0);
    assert!(vp.viewport_uncovered(0.0, true), "shrinking a row in a padded pinned list reports uncovered");

    // 3) Empty mounted range inside a gap wider than the viewport, or inside pad_top / pad_bottom.
    let mut vg = VirtualList { viewport: 10.0, pad_top: 30.0, pad_bottom: 40.0, indexed: true, ..Default::default() };
    vg.set_estimate(20.0);
    vg.set_gap(30.0);
    vg.set_keys((0..5).map(|i| format!("r{i}")).collect(), 0, None);
    vg.set_overscan(clamp_virtual_overscan(0.0, 0.0));
    // Wholly inside pad_top [0, 30): range 0..0.
    vg.select(5.0, false, None);
    assert_eq!(vg.range, 0..0);
    assert!(!vg.viewport_uncovered(5.0, false));
    assert!(vg.viewport_uncovered(25.0, false), "reaching past pad_top into row 0 is uncovered");
    // Wholly inside gap 0 [50, 80): range 1..1.
    vg.select(60.0, false, None);
    assert_eq!(vg.range, 1..1);
    assert!(!vg.viewport_uncovered(60.0, false));
    assert!(vg.viewport_uncovered(49.0, false), "crossing up into row 0 is uncovered");
    assert!(vg.viewport_uncovered(71.0, false), "crossing down into row 1 is uncovered");
    // Wholly inside pad_bottom (rows end at 30 + 5*20 + 4*30 = 250): range 5..5.
    vg.select(255.0, false, None);
    assert_eq!(vg.range, 5..5);
    assert!(!vg.viewport_uncovered(255.0, false));
    assert!(!vg.needs_window(255.0, false));
    // Scrolling back up into row 4 [230, 250) must immediately report uncovered (not off by gap!).
    assert!(vg.viewport_uncovered(248.0, false), "scrolling up from pad_bottom into last row is uncovered");
    assert!(vg.needs_window(248.0, false), "needs_window must trigger as soon as last row enters viewport");

    // 4) Fractional pad_top boundary consistency between overscan() and viewport_uncovered().
    let mut vf = VirtualList { viewport: 100.0, pad_top: 0.1, indexed: true, ..Default::default() };
    vf.set_estimate(20.0);
    vf.set_gap(8.0);
    vf.set_keys((0..20).map(|i| format!("r{i}")).collect(), 0, None);
    vf.set_overscan(clamp_virtual_overscan(0.0, 0.0));
    for i in 1..10 {
        let edge = vf.pad_top + vf.row_end(i);
        for offset in [f32::from_bits(edge.to_bits() - 1), edge, f32::from_bits(edge.to_bits() + 1)] {
            vf.select(offset, false, None);
            assert!(!vf.viewport_uncovered(offset, false), "ULP boundary at row {i}, offset {offset}");
            assert!(!vf.needs_window(offset, false), "needs_window ULP boundary at row {i}, offset {offset}");
        }
    }
}

#[test]
fn pinned_virtual_overscan_drops_the_32_row_floor_while_default_keeps_it() {
    let make = |override_vp: Option<(f32, f32)>| {
        let mut v = VirtualList { viewport: 192.0, indexed: true, ..Default::default() };
        v.set_estimate(24.0);
        v.set_keys((0..500).map(|i| format!("r{i}")).collect(), 0, None);
        for i in 0..500 {
            let h = [20.0, 24.0, 28.0][i % 3];
            v.set_height(&format!("r{i}"), h);
        }
        v.set_overscan(override_vp);
        v
    };

    // Unset overscan preserves the legacy 32-row floor at the pinned end.
    let mut def = make(None);
    def.select(0.0, true, None);
    assert_eq!(def.range.len(), PINNED_ROWS);
    assert_eq!(def.unfilled(0.0, true), 0);
    def.range = 0..10;
    assert_eq!(def.unfilled(0.0, true), PINNED_ROWS);

    // Explicit (2.0, 1.75): viewport (192 pt) + 2.0 vp overscan (384 pt) = 576 pt = 24 rows.
    let mut wide = make(clamp_virtual_overscan(2.0, 1.75));
    wide.select(0.0, true, None);
    assert_eq!(wide.range.len(), 24, "explicit (2.0, 1.75) pinned range: {:?}", wide.range);
    assert_eq!(wide.unfilled(0.0, true), 0);
    wide.range = 0..10;
    assert_eq!(wide.unfilled(0.0, true), 24);

    // Explicit (0.5, 0.25): viewport (192 pt) + 0.5 vp overscan (96 pt) = 288 pt = 12 rows.
    let mut tight = make(clamp_virtual_overscan(0.5, 0.25));
    tight.select(0.0, true, None);
    assert_eq!(tight.range.len(), 12, "explicit (0.5, 0.25) pinned range: {:?}", tight.range);
    assert_eq!(tight.unfilled(0.0, true), 0);
    tight.range = 0..10;
    assert_eq!(tight.unfilled(0.0, true), 12);

    // Explicit (0.0, 0.0): viewport only (192 pt -> 8..=9 rows), never uncovered or oscillating.
    let mut zero = make(clamp_virtual_overscan(0.0, 0.0));
    zero.select(0.0, true, None);
    assert!((8..=9).contains(&zero.range.len()), "explicit (0.0, 0.0) pinned range: {:?}", zero.range);
    assert_eq!(zero.unfilled(0.0, true), 0);
    assert!(!zero.viewport_uncovered(0.0, true));
    assert!(!zero.needs_window(0.0, true));

    // Small pinned list (< 32 visible rows) with gap and zero overscan where max_offset lands in a gap:
    // 60 rows * 20 + 59 * 10 = 1790; viewport 205 -> max_offset 1585 in gap 52 [1580, 1590).
    let mut vg = VirtualList { viewport: 205.0, indexed: true, ..Default::default() };
    vg.set_estimate(20.0);
    vg.set_gap(10.0);
    vg.set_keys((0..60).map(|i| format!("r{i}")).collect(), 0, None);
    vg.set_overscan(clamp_virtual_overscan(0.0, 0.0));
    vg.select(0.0, true, None);
    assert_eq!(vg.range, 53..60, "small pinned list with gap mounts 7 visible rows, not 32");
    assert_eq!(vg.unfilled(0.0, true), 0);
    assert!(!vg.viewport_uncovered(0.0, true));
    assert!(!vg.needs_window(0.0, true));
    vg.set_height("r55", 10.0);
    assert!(vg.viewport_uncovered(0.0, true), "shrinking a row past the gap still reports uncovered");
}
