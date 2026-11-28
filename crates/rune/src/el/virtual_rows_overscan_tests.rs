use std::borrow::Cow;
use std::cell::Cell;

use crate::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, TextStyle, col, memo, row, text};

const ROW_H: f32 = 22.0;
const VIEW_H: f32 = 600.0;
const COLS: usize = 8;

fn table_row(r: usize) -> El {
    (0..COLS).fold(row().w_full().h(ROW_H).px(8.0).gap(8.0), |rw, c| {
        rw.child(col().w(120.0).child(text(format!("r{r}-c{c}")).font(TextStyle::new(12.0, 400.0))))
    })
}

fn run_table_scenario(overscan: Option<(f32, f32)>) -> (usize, usize, u64) {
    struct S {
        rows: usize,
        overscan: Option<(f32, f32)>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut list = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(
                s.rows,
                ROW_H,
                |_: &S, i| Cow::Owned(format!("r{i}")),
                |_: &S, i| table_row(i),
            )
            .virtual_rows_version(s.rows);
        if let Some((o, e)) = s.overscan {
            list = list.virtual_overscan(o, e);
        }
        col().w_full().h_full().child(
            col().key("table").w_full().h(VIEW_H).scroll(ScrollSpec { stick_bottom: false }).child(list),
        )
    }

    let mut rt = Runtime::new(S { rows: 10_000, overscan }, view, (1100.0, 700.0), Env::new(true, 2.0));
    for _ in 0..60 {
        if !rt.frame(1.0 / 120.0) && rt.wake_deadline().is_none() { break; }
    }
    rt.update(|_, tx| {
        tx.scroll_to_instant("table", 0.0, 5000.0 * ROW_H);
    });
    for _ in 0..120 {
        if !rt.frame(1.0 / 120.0) && rt.wake_deadline().is_none() { break; }
    }
    // Must reach idle once settled (no oscillation, no pending wake).
    assert!(!rt.frame(1.0 / 120.0), "overscan={overscan:?} must settle idle without oscillating");
    assert_eq!(rt.wake_deadline(), None, "overscan={overscan:?} must arm no wake at rest");

    let mounted_rows = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with('r') && k[1..].bytes().all(|b| b.is_ascii_digit()))).count();
    let layout_nodes = rt.stats.layout_nodes;

    // 240 frames of 3 pt wheel steps down then up (720 pt total = 1.2 viewports).
    let commits0 = rt.stats.commits;
    for i in 0..240 {
        rt.wheel(500.0, 300.0, if i < 120 { -3.0 } else { 3.0 });
        rt.frame(1.0 / 120.0);
    }
    for _ in 0..60 {
        if !rt.frame(1.0 / 120.0) && rt.wake_deadline().is_none() { break; }
    }
    assert!(!rt.frame(1.0 / 120.0), "overscan={overscan:?} must settle idle after scrolling");
    (mounted_rows, layout_nodes, rt.stats.commits - commits0)
}

#[test]
fn default_virtual_overscan_is_unchanged_and_opt_in_reduces_mounted_nodes() {
    let (def_rows, def_nodes, def_commits) = run_table_scenario(None);
    let (explicit_def_rows, explicit_def_nodes, explicit_def_commits) = run_table_scenario(Some((2.0, 1.75)));
    assert_eq!(def_rows, explicit_def_rows, "explicit (2.0, 1.75) matches omitted default rows");
    assert_eq!(def_nodes, explicit_def_nodes, "explicit (2.0, 1.75) matches omitted default nodes");
    assert_eq!(def_commits, explicit_def_commits, "explicit (2.0, 1.75) matches omitted default commits");

    let (tight_rows, tight_nodes, tight_commits) = run_table_scenario(Some((0.5, 0.25)));
    let (zero_rows, zero_nodes, zero_commits) = run_table_scenario(Some((0.0, 0.0)));

    // 600 pt viewport / 22 pt row ~= 28 visible rows.
    // Default mounts ~5 viewports (~136 rows); (0.5, 0.25) mounts ~2 viewports (~55 rows);
    // (0.0, 0.0) mounts ~1 viewport (~28 rows).
    assert!((130..=145).contains(&def_rows), "default rows={def_rows}");
    assert!((50..=62).contains(&tight_rows), "tight rows={tight_rows}");
    assert!((27..=30).contains(&zero_rows), "zero rows={zero_rows}");
    assert!(tight_nodes < def_nodes / 2, "tight nodes ({tight_nodes}) must be under half of default ({def_nodes})");
    assert!(zero_nodes < tight_nodes, "zero nodes ({zero_nodes}) must be fewer than tight ({tight_nodes})");
    assert!(tight_commits >= def_commits && tight_commits < 40, "tight commits={tight_commits}, def={def_commits}");
    assert!(zero_commits > tight_commits, "zero overscan commits on window edges: zero={zero_commits}, tight={tight_commits}");
}

#[test]
fn degenerate_and_non_finite_virtual_overscan_values_clamp_safely_without_oscillating() {
    struct S {
        overscan: f32,
        edge: f32,
        gap: f32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let list = col().key("rows").w_full().gap(s.gap).growth(GrowthSpec::default())
            .virtual_rows(
                500,
                20.0,
                |_: &S, i| Cow::Owned(format!("r{i}")),
                |_: &S, i| text(format!("row {i}")).h(20.0),
            )
            .virtual_overscan(s.overscan, s.edge);
        col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(list)
    }

    let cases = [
        (0.0, 0.0, 0.0),
        (0.0, 0.0, 6.0),
        (0.5, 0.5, 0.0),
        (0.5, 5.0, 0.0),
        (-1.0, -2.0, 0.0),
        (f32::NAN, 0.25, 0.0),
        (0.5, f32::NAN, 0.0),
        (f32::INFINITY, 0.25, 0.0),
        (0.5, f32::INFINITY, 0.0),
        (f32::NEG_INFINITY, f32::NEG_INFINITY, 0.0),
        (50.0, 50.0, 0.0),
    ];

    for (overscan, edge, gap) in cases {
        let mut rt = Runtime::new(S { overscan, edge, gap }, view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.update(|_, tx| { tx.scroll_to_instant("scroll", 0.0, 2000.0); });
        let mut settled = false;
        for _ in 0..120 {
            if !rt.frame(1.0 / 60.0) && rt.wake_deadline().is_none() {
                settled = true;
                break;
            }
        }
        assert!(settled, "case ({overscan}, {edge}, gap={gap}) failed to settle (possible oscillation)");
        let c0 = rt.stats.commits;
        for _ in 0..5 {
            assert!(!rt.frame(1.0 / 60.0), "case ({overscan}, {edge}, gap={gap}) must stay idle");
        }
        assert_eq!(rt.wake_deadline(), None, "no pending wake at rest for ({overscan}, {edge}, gap={gap})");
        assert_eq!(rt.stats.commits, c0, "no extra commits at rest for ({overscan}, {edge}, gap={gap})");
    }
}

#[test]
fn memo_reuse_detects_virtual_overscan_changes() {
    thread_local! {
        static MEMO_BUILDS: Cell<usize> = const { Cell::new(0) };
    }
    struct S {
        overscan: Option<(f32, f32)>,
        tick: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        // Attach virtual_overscan to the memo placeholder root itself so memo_reuse::same_root_el
        // must detect when the builder changes while memo inputs stay constant.
        let mut list = memo("vlist", (), || {
            MEMO_BUILDS.set(MEMO_BUILDS.get() + 1);
            col().w_full().growth(GrowthSpec::default()).virtual_rows(
                500,
                20.0,
                |_: &S, i| Cow::Owned(format!("r{i}")),
                |_: &S, i| text(format!("row {i}")).h(20.0),
            )
        });
        if let Some((o, e)) = s.overscan {
            list = list.virtual_overscan(o, e);
        }
        col().w_full().h(200.0)
            .child(text(format!("tick {}", s.tick)).h(0.0))
            .child(col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }

    MEMO_BUILDS.set(0);
    let mut rt = Runtime::new(S { overscan: None, tick: 0 }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| { tx.scroll_to_instant("scroll", 0.0, 2000.0); });
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) && rt.wake_deadline().is_none() { break; }
    }
    let count_rows = |rt: &Runtime<S>| {
        rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with('r') && k[1..].bytes().all(|b| b.is_ascii_digit()))).count()
    };
    let default_rows = count_rows(&rt);
    assert!(default_rows >= 45, "default mounts ~5 viewports (~50 rows), got {default_rows}");

    // Changing only virtual_overscan on the memo root must take effect immediately.
    rt.update(|s, _| {
        s.overscan = Some((0.5, 0.25));
        s.tick += 1;
    });
    for _ in 0..20 {
        if !rt.frame(1.0 / 60.0) { break; }
    }
    let tight_rows = count_rows(&rt);
    assert!((18..=24).contains(&tight_rows), "tight (0.5, 0.25) mounts ~2 viewports (~20 rows), got {tight_rows}");

    // Changing again to (0.0, 0.0) shrinks further to ~1 viewport (~10 rows).
    rt.update(|s, _| {
        s.overscan = Some((0.0, 0.0));
        s.tick += 1;
    });
    for _ in 0..20 {
        if !rt.frame(1.0 / 60.0) { break; }
    }
    let zero_rows = count_rows(&rt);
    assert!((9..=12).contains(&zero_rows), "zero (0.0, 0.0) mounts ~1 viewport (~10 rows), got {zero_rows}");
    assert_eq!(MEMO_BUILDS.get(), 1, "memo closure itself stayed cached");
}

#[test]
fn eager_virtualize_also_respects_opt_in_virtual_overscan() {
    struct S {
        overscan: Option<(f32, f32)>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut list = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtualize(20.0)
            .children((0..200).map(|i| text(format!("r{i}")).key(format!("r{i}")).h(20.0)));
        if let Some((o, e)) = s.overscan {
            list = list.virtual_overscan(o, e);
        }
        col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(list)
    }

    let measure = |overscan| {
        let mut rt = Runtime::new(S { overscan }, view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.update(|_, tx| { tx.scroll_to_instant("scroll", 0.0, 2000.0); });
        for _ in 0..30 {
            if !rt.frame(1.0 / 60.0) && rt.wake_deadline().is_none() { break; }
        }
        rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with('r') && k[1..].bytes().all(|b| b.is_ascii_digit()))).count()
    };

    let def = measure(None);
    let tight = measure(Some((0.5, 0.25)));
    assert!(def >= 45, "eager default mounts ~5 viewports, got {def}");
    assert!((18..=24).contains(&tight), "eager tight mounts ~2 viewports, got {tight}");
}

#[test]
fn zero_overscan_with_gap_does_not_force_mount_all_rows_when_viewport_edge_lies_in_gap() {
    thread_local! {
        static BUILDS: Cell<usize> = const { Cell::new(0) };
    }
    struct S {
        tick: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let list = col().key("rows").w_full().gap(8.0).growth(GrowthSpec::default())
            .virtual_rows(
                100,
                20.0,
                |_: &S, i| Cow::Owned(format!("r{i}")),
                |_: &S, i| {
                    BUILDS.set(BUILDS.get() + 1);
                    text(format!("row {i}")).h(20.0)
                },
            )
            .virtual_overscan(0.0, 0.0);
        col().w_full().h_full()
            .child(text(format!("tick {}", s.tick)).h(0.0))
            .child(col().key("scroll").w_full().h(100.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }

    let count_rows = |rt: &Runtime<S>| {
        rt.elements().into_iter().filter(|e| {
            e.key.as_deref().is_some_and(|k| k.starts_with('r') && k[1..].bytes().all(|b| b.is_ascii_digit()))
        }).count()
    };

    let mut rt = Runtime::new(S { tick: 0 }, view, (300.0, 100.0), Env::default());
    rt.frame(0.0);
    // Stride is 20 + 8 = 28. Offset 24 puts top edge in gap [20, 28); offset 36 puts bottom edge
    // at 136 in gap [132, 140). Sweep every 2 pt across a full stride.
    for step in 0..=20 {
        let offset = 20.0 + step as f32 * 2.0;
        rt.update(|_, tx| { tx.scroll_to_instant("scroll", 0.0, offset); });
        for _ in 0..20 {
            if !rt.frame(1.0 / 60.0) && rt.wake_deadline().is_none() { break; }
        }
        let mounted = count_rows(&rt);
        assert!(mounted <= 5, "at offset {offset}, expected <= 5 mounted rows, got {mounted}");
        // A redraw at rest must build only the visible rows (single pass, no retry storm).
        BUILDS.set(0);
        rt.update(|s, _| { s.tick += 1; });
        rt.frame(1.0 / 60.0);
        let mounted_after = count_rows(&rt);
        let built = BUILDS.get();
        assert!(mounted_after <= 5, "after redraw at offset {offset}, expected <= 5 mounted rows, got {mounted_after}");
        assert!(built <= 5, "redraw at rest at offset {offset} must build <= 5 rows, built {built}");
    }

    // Also exercise wheel scrolling (remount_virtuals fast path) across multiple row+gap strides.
    for _ in 0..40 {
        rt.wheel(150.0, 50.0, -3.0);
        rt.frame(1.0 / 60.0);
        let mounted = count_rows(&rt);
        assert!(mounted <= 5, "wheel scrolling into gap mounted {mounted} rows (expected <= 5)");
    }
}

#[test]
fn variable_height_virtual_rows_with_gap_and_zero_overscan_cover_viewport_without_forcing_all() {
    thread_local! {
        static VAR_BUILDS: Cell<usize> = const { Cell::new(0) };
    }
    struct S {
        heights: Vec<f32>,
        estimate: f32,
        tick: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let list = col().key("rows").w_full().gap(8.0).growth(GrowthSpec::default())
            .virtual_rows(
                s.heights.len(),
                s.estimate,
                |_: &S, i| Cow::Owned(format!("r{i}")),
                |s: &S, i| {
                    VAR_BUILDS.set(VAR_BUILDS.get() + 1);
                    text(format!("row {i}")).h(s.heights[i])
                },
            )
            .virtual_overscan(0.0, 0.0);
        col().w_full().h_full()
            .child(text(format!("tick {}", s.tick)).h(0.0))
            .child(col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }

    let count_rows = |rt: &Runtime<S>| {
        rt.elements().into_iter().filter(|e| {
            e.key.as_deref().is_some_and(|k| k.starts_with('r') && k[1..].bytes().all(|b| b.is_ascii_digit()))
        }).count()
    };

    // 1) Unequal heights (18..=78 pt) with 8 pt gap: scroll through offsets landing in gaps.
    let heights: Vec<f32> = (0..200).map(|i| 18.0 + ((i * 17) % 61) as f32).collect();
    let mut rt = Runtime::new(S { heights: heights.clone(), estimate: 45.0, tick: 0 }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    // Compute exact gap midpoints of the first 10 rows and probe them directly.
    let mut y = 0.0;
    let mut gap_offsets = Vec::new();
    for &h in heights.iter().take(12) {
        y += h;
        gap_offsets.push(y + 4.0); // squarely inside the 8 pt gap after this row
        y += 8.0;
    }
    for offset in gap_offsets {
        rt.update(|_, tx| { tx.scroll_to_instant("scroll", 0.0, offset); });
        for _ in 0..20 {
            if !rt.frame(1.0 / 60.0) && rt.wake_deadline().is_none() { break; }
        }
        let mounted = count_rows(&rt);
        assert!(mounted <= 12, "variable-height list at gap offset {offset} mounted {mounted} rows (expected <= 12)");
        VAR_BUILDS.set(0);
        rt.update(|s, _| { s.tick += 1; });
        rt.frame(1.0 / 60.0);
        let built = VAR_BUILDS.get();
        assert!(built <= 12, "variable-height redraw at gap offset {offset} built {built} rows (expected <= 12)");
        assert!(count_rows(&rt) <= 12);
    }

    // 2) Overestimated row heights (estimate 90 pt, actual 20 pt) with 8 pt gap and viewport 80 pt:
    //    rows [0,20), [28,48), [56,76), gap [76,84), row 3 [84,104). Must retry to mount all 3
    //    visible rows without leaving a blank, and must NOT force-mount all 100 rows even though
    //    the bottom edge (80 pt) lies inside gap 2 [76, 84).
    let small_heights = vec![20.0; 100];
    let mut rt_shrink = Runtime::new(
        S { heights: small_heights, estimate: 90.0, tick: 0 },
        |s: &S, _: &mut Cx<S>| {
            let list = col().key("rows").w_full().gap(8.0).growth(GrowthSpec::default())
                .virtual_rows(
                    s.heights.len(),
                    s.estimate,
                    |_: &S, i| Cow::Owned(format!("r{i}")),
                    |s: &S, i| text(format!("row {i}")).h(s.heights[i]),
                )
                .virtual_overscan(0.0, 0.0);
            col().key("scroll").w_full().h(80.0).scroll(ScrollSpec { stick_bottom: false }).child(list)
        },
        (300.0, 80.0),
        Env::default(),
    );
    rt_shrink.frame(0.0);
    let mounted = count_rows(&rt_shrink);
    assert_eq!(mounted, 3, "overestimated shrinking rows must retry to mount all 3 visible rows, without forcing all 100; got {mounted}");
}

#[test]
fn pinned_and_eager_virtual_lists_with_gap_and_zero_overscan_do_not_force_all_rows() {
    // 1) Pinned (stick_bottom) list with > PINNED_ROWS (32) visible rows where max_offset lies in
    //    a gap: 60 rows * 20 pt + 59 * 10 pt gap = 1790 pt; viewport 1015 pt -> max_offset 775 pt,
    //    which sits inside gap 25 [770, 780) before row 26 [780, 800).
    let mut rt_pinned = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            let list = col().key("rows").w_full().gap(10.0).growth(GrowthSpec::default())
                .virtual_rows(
                    60,
                    20.0,
                    |_: &(), i| Cow::Owned(format!("r{i}")),
                    |_: &(), i| text(format!("row {i}")).h(20.0),
                )
                .virtual_overscan(0.0, 0.0);
            col().key("scroll").w_full().h(1015.0).scroll(ScrollSpec { stick_bottom: true }).child(list)
        },
        (300.0, 1015.0),
        Env::default(),
    );
    rt_pinned.frame(0.0);
    let pinned_mounted = rt_pinned.elements().into_iter().filter(|e| {
        e.key.as_deref().is_some_and(|k| k.starts_with('r') && k[1..].bytes().all(|b| b.is_ascii_digit()))
    }).count();
    assert_eq!(pinned_mounted, 34, "pinned list with top edge in gap must mount rows 26..60 (34 rows), not all 60; got {pinned_mounted}");

    // 2) Eager virtualize() with gap + zero overscan, top edge in gap (offset 24 in [20, 28)).
    let mut rt_eager = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            let list = col().key("rows").w_full().gap(8.0).growth(GrowthSpec::default())
                .virtualize(20.0)
                .virtual_overscan(0.0, 0.0)
                .children((0..100).map(|i| text(format!("r{i}")).key(format!("r{i}")).h(20.0)));
            col().key("scroll").w_full().h(100.0).scroll(ScrollSpec { stick_bottom: false }).child(list)
        },
        (300.0, 100.0),
        Env::default(),
    );
    rt_eager.frame(0.0);
    rt_eager.update(|_, tx| { tx.scroll_to_instant("scroll", 0.0, 24.0); });
    for _ in 0..20 {
        if !rt_eager.frame(1.0 / 60.0) && rt_eager.wake_deadline().is_none() { break; }
    }
    let eager_mounted = rt_eager.elements().into_iter().filter(|e| {
        e.key.as_deref().is_some_and(|k| k.starts_with('r') && k[1..].bytes().all(|b| b.is_ascii_digit()))
    }).count();
    assert!(eager_mounted <= 5, "eager virtualize at gap offset 24 must mount <= 5 rows, got {eager_mounted}");
}
