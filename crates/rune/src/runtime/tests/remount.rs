//! Virtual-window shifts (`CommitWhy::VIRTUAL`) remount only newly entering `virtual_rows`,
//! leaving `view`, sibling subtrees and already-mounted rows untouched unless a read moves.
use super::*;
use crate::el::GrowthSpec;
use crate::{ScrollSpec, memo, row, text};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};

thread_local! {
    static VIEWS: Cell<u32> = const { Cell::new(0) };
    static SIDES: Cell<u32> = const { Cell::new(0) };
    static BUILT_ROWS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

fn take_views<R>(f: impl FnOnce() -> R) -> (u32, R) {
    let before = VIEWS.get();
    let r = f();
    (VIEWS.get() - before, r)
}

fn take_built() -> Vec<usize> {
    BUILT_ROWS.with_borrow_mut(std::mem::take)
}

fn shown_elements<S>(rt: &Runtime<S>) -> Vec<(String, [i32; 4], Option<String>)> {
    let mut out: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|e| e.visible[2] > 0.0 && e.visible[3] > 0.0 && !e.virtual_extent)
        .map(|e| (e.path, e.rect.map(|x| (x * 100.0).round() as i32), e.text))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn plain_workspace(tick: &u32, _: &mut Cx<u32>) -> El {
    VIEWS.set(VIEWS.get() + 1);
    SIDES.set(SIDES.get() + 1);
    let side = col().key("side").w(180.0).h_full().gap(4.0).children(
        (0..24).map(|i| text(format!("Side {i}: {tick}")).key(format!("s{i}")).h(18.0)),
    );
    let list = col().key("rows").w_full().px(8.0).growth(GrowthSpec::default()).virtual_rows(
        500,
        36.0,
        |_: &u32, i| Cow::Owned(format!("r{i}")),
        |t: &u32, i| {
            BUILT_ROWS.with_borrow_mut(|b| b.push(i));
            row().w_full().h(36.0).gap(6.0).children([
                text(format!("Row {i}")).key("title").grow(1.0),
                text(format!("v{t}")).key("ver"),
            ])
        },
    );
    let footer = text(format!("Footer {tick}")).key("footer").h(28.0);
    row().w_full().h_full().children([
        side,
        col().key("main").grow(1.0).h_full().children([
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
            footer,
        ]),
    ])
}

#[test]
fn scrolling_virtual_rows_without_memo_skips_view_and_keeps_mounted_rows() {
    let mut rt = Runtime::new(0u32, plain_workspace, (640.0, 420.0), Env::new(true, 2.0));
    let mut full = Runtime::new(0u32, plain_workspace, (640.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    full.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    while full.frame(1.0 / 120.0) {}

    SIDES.set(0);
    take_built();
    let mut scroll_commits = 0u32;
    let mut full_commits = 0u32;
    for step in 0..40 {
        take_built();
        let c0 = rt.stats.commits;
        let (views_ran, _) = take_views(|| {
            rt.wheel(400.0, 200.0, -90.0);
            rt.frame(1.0 / 120.0);
        });
        assert_eq!(views_ran, 0, "step {step}: scrolling must not re-run view");
        let built = take_built();
        if rt.stats.commits > c0 {
            scroll_commits += 1;
            let mounted_len = rt.commit.virtuals["#0/main/scroll/rows"].range.len();
            assert!(
                built.len() < mounted_len,
                "step {step}: built {} rows for a {mounted_len}-row window",
                built.len()
            );
        } else {
            assert!(built.is_empty(), "step {step}: non-committing frame built {built:?}");
        }

        // Force full view on the reference runtime and check exact visible parity.
        let fc0 = full.stats.commits;
        full.commit.set_reuse_scale(f32::NAN);
        full.wheel(400.0, 200.0, -90.0);
        full.frame(1.0 / 120.0);
        if full.stats.commits > fc0 {
            full_commits += 1;
        }
        assert_eq!(shown_elements(&rt), shown_elements(&full), "step {step}");
    }
    assert!(scroll_commits >= 10, "expected window shifts, got {scroll_commits}");
    assert_eq!(SIDES.get(), full_commits, "only full-commit reference steps ran the sidebar; rt ran 0");

    // Hover and press on both a kept row and a newly scrolled-in row still work, and a state
    // mutation runs the view again with updated content everywhere.
    rt.update(|t, _| *t = 7);
    let (views_ran, _) = take_views(|| {
        rt.frame(1.0 / 120.0);
    });
    assert_eq!(views_ran, 1);
    full.update(|t, _| *t = 7);
    full.frame(1.0 / 120.0);
    assert_eq!(shown_elements(&rt), shown_elements(&full));
}

#[test]
fn virtual_remount_reruns_view_when_a_layout_read_of_a_culled_row_changes() {
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        let r0_mounted = cx.layout("r0").is_some();
        let list = col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
            200,
            40.0,
            |_: &(), i| Cow::Owned(format!("r{i}")),
            |_: &(), i| text(format!("Row {i}")).h(40.0),
        );
        col().w_full().h_full().children([
            text(format!("r0={r0_mounted}")).key("status").h(24.0),
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        ])
    }

    let mut rt = Runtime::new((), view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    let status = |rt: &Runtime<()>| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("status")).and_then(|e| e.text)
    };
    assert_eq!(status(&rt).as_deref(), Some("r0=true"));

    // Scroll far enough down that `r0` leaves the mounted window; `settle_layout_reads` sees
    // `cx.layout("r0")` become `None` and re-runs `view` to update `status`.
    rt.wheel(150.0, 120.0, -3000.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(status(&rt).as_deref(), Some("r0=false"));
}

#[test]
fn virtual_remount_preserves_text_selection_across_scrolled_out_rows() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        let list = col()
            .key("rows")
            .w_full()
            .selection_group(true)
            .growth(GrowthSpec::default())
            .virtual_rows(
                200,
                32.0,
                |_: &(), i| Cow::Owned(format!("r{i}")),
                |_: &(), i| text(format!("Line {i}")).key("txt").selectable(true).h(32.0),
            );
        col().w_full().h_full().child(
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        )
    }

    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}

    // Drag-select starting at x=0 in row 0, then scroll row 0 out of the mounted window while dragging.
    rt.pointer_down(0.0, 10.0);
    rt.pointer_move(120.0, 50.0);
    rt.frame(1.0 / 120.0);
    for _ in 0..10 {
        let (views_ran, _) = take_views(|| {
            rt.wheel(150.0, 100.0, -300.0);
            rt.pointer_move(120.0, 150.0);
            rt.frame(1.0 / 120.0);
        });
        assert_eq!(views_ran, 0);
    }
    rt.pointer_up(120.0, 150.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.find("r0").is_none(), "r0 scrolled out of the mounted window");
    let copied = rt.selected_text().unwrap_or_default();
    assert!(copied.starts_with("Line 0\n"), "pinned row 0 stays in selection: {copied:?}");
}

#[test]
fn virtual_remount_keeps_view_and_mounted_row_memos_and_evicts_culled_ones() {
    thread_local! {
        static HEADER_BUILDS: Cell<u32> = const { Cell::new(0) };
        static ROW_MEMO_BUILDS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }
    struct S {
        tick: u32,
        clicked: Option<usize>,
        edit: crate::TextEdit,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let _ = s.tick;
        let header = memo("hdr", 0u32, || {
            HEADER_BUILDS.set(HEADER_BUILDS.get() + 1);
            text("Header").h(24.0)
        });
        let list = col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
            300,
            32.0,
            |_: &S, i| Cow::Owned(format!("r{i}")),
            |s: &S, i| {
                memo(format!("m{i}"), 0u32, || {
                    ROW_MEMO_BUILDS.with_borrow_mut(|b| b.push(i));
                    row()
                        .w_full()
                        .h(32.0)
                        .on_press(move |s: &mut S| s.clicked = Some(i))
                        .children([
                            text(format!("Row {i}")).grow(1.0),
                            crate::text_input(&s.edit, crate::TextStyle::new(13.0, 400.0)).w(60.0),
                        ])
                })
            },
        );
        col().w_full().h_full().children([
            header,
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        ])
    }

    let mut rt = Runtime::new(
        S { tick: 0, clicked: None, edit: crate::TextEdit::default() },
        view,
        (320.0, 220.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    assert_eq!(HEADER_BUILDS.get(), 1);

    // Scroll far down across many windows: non-reusable `text_input` row memos that leave must be
    // evicted, while `hdr` and currently mounted rows stay cached when `tick` finally runs `view`.
    for _ in 0..30 {
        rt.wheel(160.0, 120.0, -240.0);
        rt.frame(1.0 / 120.0);
    }
    while rt.frame(1.0 / 120.0) {}

    let mounted = rt.commit.virtuals["#0/scroll/rows"].range.clone();
    assert!(mounted.start > 50, "scrolled past early rows: {mounted:?}");
    ROW_MEMO_BUILDS.with_borrow_mut(Vec::clear);

    // A state mutation runs `view`: neither `hdr` nor any row already in `mounted` rebuilds.
    rt.update(|s, _| s.tick += 1);
    rt.frame(1.0 / 120.0);
    assert_eq!(HEADER_BUILDS.get(), 1, "view-level memo survived partial commits");
    let rebuilt: Vec<usize> = ROW_MEMO_BUILDS
        .with_borrow_mut(std::mem::take)
        .into_iter()
        .filter(|i| mounted.contains(i))
        .collect();
    assert!(rebuilt.is_empty(), "mounted row memos survived partial commits, got {rebuilt:?}");
    // And culled early rows (`0..mounted.start`) were evicted rather than accumulating in MemoCache.
    assert!(
        rt.commit.memo_len() <= rt.commit.virtuals["#0/scroll/rows"].range.len() + 2,
        "culled row memos evicted: memo len {}, mounted {:?}",
        rt.commit.memo_len(),
        rt.commit.virtuals["#0/scroll/rows"].range
    );

    // Press an on-screen scrolled-in row and confirm hit testing + handler dispatch works.
    let (expected_idx, vis_row) = rt
        .elements()
        .into_iter()
        .find_map(|e| {
            let idx = e.key.as_deref()?.strip_prefix('r')?.parse::<usize>().ok()?;
            (e.visible[3] >= 20.0).then_some((idx, e))
        })
        .expect("on-screen row");
    let (cx, cy) = (vis_row.visible[0] + 10.0, vis_row.visible[1] + 10.0);
    rt.pointer_move(cx, cy);
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.clicked, Some(expected_idx));
}

#[test]
fn virtual_remount_keeps_live_text_bound_without_running_view() {
    struct State {
        tick: u32,
        clock: crate::LiveText,
        row_badge: crate::LiveText,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        let list = col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
            400,
            32.0,
            |_: &State, i| Cow::Owned(format!("r{i}")),
            |s: &State, i| {
                row().w_full().h(32.0).gap(6.0).children([
                    text(format!("Row {i} v{}", s.tick)).key("title").grow(1.0),
                    text("00").key("badge").live_text(&s.row_badge),
                ])
            },
        );
        col().w_full().h_full().children([
            row().key("bar").w_full().h(28.0).gap(8.0).children([
                text("00:00").key("clock").live_text(&s.clock),
                text(format!("v{}", s.tick)).key("ver"),
            ]),
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        ])
    }

    let make = || State {
        tick: 0,
        clock: crate::LiveText::new("00:00"),
        row_badge: crate::LiveText::new("00"),
    };
    let mut rt = Runtime::new(make(), view, (420.0, 260.0), Env::new(true, 2.0));
    let mut full = Runtime::new(make(), view, (420.0, 260.0), Env::new(true, 2.0));
    rt.frame(0.0);
    full.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    while full.frame(1.0 / 120.0) {}

    let clock_info = |rt: &Runtime<State>| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("clock")).expect("clock")
    };
    let initial_clock_rect = clock_info(&rt).rect;

    // Publish before scrolling so both staying live_text nodes already carry latched content.
    for s in [&rt.state, &full.state] {
        s.clock.set("00:01");
        s.row_badge.set("b1");
    }
    let (views_ran, _) = take_views(|| { rt.frame(1.0 / 120.0); });
    assert_eq!(views_ran, 0);
    full.frame(1.0 / 120.0);
    assert_eq!(clock_info(&rt).text.as_deref(), Some("00:01"));
    assert_eq!(shown_elements(&rt), shown_elements(&full));

    let mut scroll_commits = 0u32;
    for step in 0..30 {
        if step == 10 {
            for s in [&rt.state, &full.state] {
                s.clock.set("a much longer clock value");
                s.row_badge.set("b2");
            }
        }
        let c0 = rt.stats.commits;
        let (views_ran, _) = take_views(|| {
            rt.wheel(200.0, 150.0, -90.0);
            rt.frame(1.0 / 120.0);
        });
        assert_eq!(views_ran, 0, "step {step}: scrolling with live_text must not re-run view");
        if rt.stats.commits > c0 {
            scroll_commits += 1;
        }

        full.commit.set_reuse_scale(f32::NAN);
        full.wheel(200.0, 150.0, -90.0);
        full.frame(1.0 / 120.0);
        assert_eq!(shown_elements(&rt), shown_elements(&full), "step {step}");
        assert_eq!(clock_info(&rt).rect, initial_clock_rect, "step {step}: placeholder box holds");
    }
    assert!(scroll_commits >= 10, "expected window shifts, got {scroll_commits}");

    // Publishing after remounts still wakes and updates without running view.
    for s in [&rt.state, &full.state] {
        s.clock.set("09:59");
        s.row_badge.set("b3");
    }
    assert!(rt.live_pending(), "live_text subscription survived remounts");
    let (views_ran, _) = take_views(|| { rt.frame(1.0 / 120.0); });
    assert_eq!(views_ran, 0);
    full.frame(1.0 / 120.0);
    assert_eq!(clock_info(&rt).text.as_deref(), Some("09:59"));
    assert_eq!(shown_elements(&rt), shown_elements(&full));

    // A state mutation still runs view once and stays in sync.
    rt.update(|s, _| s.tick = 1);
    let (views_ran, _) = take_views(|| { rt.frame(1.0 / 120.0); });
    assert_eq!(views_ran, 1);
    full.update(|s, _| s.tick = 1);
    full.frame(1.0 / 120.0);
    assert_eq!(shown_elements(&rt), shown_elements(&full));
}

#[test]
fn scroll_requests_past_mounted_rows_remount_once_without_running_view() {
    #[derive(Default)]
    struct S {
        views: Cell<u32>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        s.views.set(s.views.get() + 1);
        col().w_full().h_full().child(
            col().key("vp").size(300.0, 400.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("list").w_full().growth(GrowthSpec::default()).virtual_rows(
                    1000,
                    20.0,
                    |_: &S, i| Cow::Owned(format!("r{i}")),
                    |_: &S, _| el().w_full().h(20.0),
                ),
            ),
        )
    }
    fn make() -> Runtime<S> {
        let mut rt = Runtime::new(S::default(), view, (300.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for _ in 0..10 {
            rt.frame(0.016);
        }
        rt
    }
    fn top_row(rt: &Runtime<S>) -> (Option<String>, f32) {
        let e = rt
            .elements()
            .into_iter()
            .filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with('r')) && e.rect[1] >= -0.01)
            .min_by(|a, b| a.rect[1].total_cmp(&b.rect[1]))
            .expect("visible row");
        (e.key, e.rect[1])
    }
    fn offset(rt: &Runtime<S>) -> f64 {
        rt.scroll.scrolls.values().next().unwrap().offset
    }

    let step = 1250.0f32;
    let steps = 10usize;
    let run = |mut ask: Box<dyn FnMut(&mut Runtime<S>, usize)>| {
        #[cfg(debug_assertions)]
        let _ = crate::tooling::perf_warn::take_log();
        let mut rt = make();
        let mut counts = Vec::with_capacity(steps);
        for i in 0..steps {
            let (c0, v0) = (rt.stats.commits, rt.state.views.get());
            ask(&mut rt, i);
            rt.frame(0.016);
            counts.push((rt.stats.commits - c0, rt.state.views.get() - v0));
        }
        let v_before_settle = rt.state.views.get();
        for _ in 0..60 {
            rt.frame(0.016);
        }
        let settle_views = rt.state.views.get() - v_before_settle;
        #[cfg(debug_assertions)]
        let warnings: Vec<_> = crate::tooling::perf_warn::take_log()
            .into_iter()
            .filter(|l| l.contains("an instant scroll"))
            .collect();
        #[cfg(not(debug_assertions))]
        let warnings: Vec<String> = Vec::new();
        (counts, settle_views, offset(&rt), top_row(&rt), shown_elements(&rt), warnings)
    };

    let (wheel_counts, wheel_settle_views, wheel_off, wheel_top, wheel_shown, _) =
        run(Box::new(move |rt, _| {
            rt.pointer_move(150.0, 200.0);
            rt.wheel(150.0, 200.0, -step);
        }));
    let (inst_counts, inst_settle_views, inst_off, inst_top, inst_shown, inst_warnings) =
        run(Box::new(move |rt, i| {
            rt.update_if(|_, tx| {
                tx.scroll_to_instant("vp", 0.0, step * (i + 1) as f32);
                false
            });
        }));
    let (glide_counts, glide_settle_views, glide_off, glide_top, glide_shown, _) =
        run(Box::new(move |rt, i| {
            rt.update_if(|_, tx| {
                tx.scroll_to_with("vp", 0.0, step * (i + 1) as f32, crate::Spring::new(0.18, 0.0));
                false
            });
        }));

    let expected_counts = vec![(1, 0); steps];
    assert_eq!(wheel_counts, expected_counts, "wheel baseline");
    assert_eq!(inst_counts, expected_counts, "scroll_to_instant must remount once with 0 view runs");
    assert_eq!(glide_counts, expected_counts, "scroll_to_with must remount once with 0 view runs");
    assert_eq!((wheel_settle_views, inst_settle_views, glide_settle_views), (0, 0, 0));
    assert!(inst_warnings.is_empty(), "instant scroll must not trip perf warning: {inst_warnings:?}");

    assert_eq!(inst_off, wheel_off, "scroll_to_instant lands at the same offset as wheel");
    assert_eq!(glide_off, wheel_off, "scroll_to_with lands at the same offset as wheel");
    assert_eq!(inst_top, wheel_top, "scroll_to_instant lands on the same top row");
    assert_eq!(glide_top, wheel_top, "scroll_to_with lands on the same top row");
    assert_eq!(inst_top.0.as_deref(), Some("r625"));
    assert_eq!(inst_shown, wheel_shown);
    assert_eq!(glide_shown, wheel_shown);
}

#[test]
fn scroll_requests_on_variable_height_virtual_list_land_exact_without_running_view() {
    #[derive(Default)]
    struct S {
        views: Cell<u32>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        s.views.set(s.views.get() + 1);
        col().w_full().h_full().child(
            col().key("vp").size(300.0, 400.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("list").w_full().growth(GrowthSpec::default()).virtual_rows(
                    1000,
                    20.0,
                    |_: &S, i| Cow::Owned(format!("r{i}")),
                    |_: &S, i| {
                        let h = if i >= 950 { 60.0 } else if i % 5 == 0 { 34.0 } else { 20.0 };
                        el().w_full().h(h)
                    },
                ),
            ),
        )
    }
    fn make() -> Runtime<S> {
        let mut rt = Runtime::new(S::default(), view, (300.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        while rt.frame(0.016) {}
        rt
    }
    fn offset(rt: &Runtime<S>) -> f64 {
        rt.scroll.scrolls.values().next().unwrap().offset
    }

    // 1. Anchored jump (scroll_key_to) onto an unmounted row with variable heights.
    let (mut rt, mut full) = (make(), make());
    full.commit.set_reuse_scale(f32::NAN);
    let v0 = rt.state.views.get();
    rt.update_if(|_, tx| { tx.scroll_key_to("r500", 40.0); false });
    rt.frame(0.016);
    full.update_if(|_, tx| { tx.scroll_key_to("r500", 40.0); false });
    full.frame(0.016);
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_key_to must not run view");
    assert_eq!(offset(&rt), offset(&full), "scroll_key_to lands at the exact same offset");
    let r500 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r500")).expect("r500 mounted");
    assert!((r500.rect[1] - 40.0).abs() < 0.01, "r500 anchored at y=40: {:?}", r500.rect);

    // 2. Instant jump past the estimated end when tail rows are taller than estimated.
    let (mut rt, mut full) = (make(), make());
    full.commit.set_reuse_scale(f32::NAN);
    let v0 = rt.state.views.get();
    rt.update_if(|_, tx| { tx.scroll_to_instant("vp", 0.0, 99_999.0); false });
    rt.frame(0.016);
    full.update_if(|_, tx| { tx.scroll_to_instant("vp", 0.0, 99_999.0); false });
    full.frame(0.016);
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_to_instant to end must not run view");
    assert_eq!(offset(&rt), offset(&full), "scroll_to_instant lands at the exact measured bottom");
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r999")).expect("r999 mounted");
    assert!((last.rect[1] + last.rect[3] - 400.0).abs() < 0.01, "r999 bottom flush with viewport: {:?}", last.rect);

    // 3. scroll_to_bottom_instant on the same variable-height list.
    let (mut rt, mut full) = (make(), make());
    full.commit.set_reuse_scale(f32::NAN);
    let v0 = rt.state.views.get();
    rt.update_if(|_, tx| { tx.scroll_to_bottom_instant("vp"); false });
    rt.frame(0.016);
    full.update_if(|_, tx| { tx.scroll_to_bottom_instant("vp"); false });
    full.frame(0.016);
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_to_bottom_instant must not run view");
    assert_eq!(offset(&rt), offset(&full), "scroll_to_bottom_instant lands at the exact measured bottom");
}

#[derive(Default)]
struct ScopedListsState {
    views: Cell<u32>,
}

fn two_scoped_virtual_lists() -> Runtime<ScopedListsState> {
    fn pane(pane_key: &'static str, scroll_key: &'static str, list_key: &'static str) -> El {
        col().key(pane_key).size(200.0, 200.0).child(
            col().key(scroll_key).size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col().key(list_key).w_full().growth(GrowthSpec::default()).virtual_rows(
                    1_000,
                    40.0,
                    |_: &ScopedListsState, i| Cow::Owned(format!("row-{i}")),
                    |_: &ScopedListsState, _| {
                        col().w_full().h(40.0).children([
                            el().w_full().h(20.0),
                            el().key("mark").w_full().h(10.0),
                            el().w_full().h(10.0),
                        ])
                    },
                ),
            ),
        )
    }
    fn view(s: &ScopedListsState, _: &mut Cx<ScopedListsState>) -> El {
        s.views.set(s.views.get() + 1);
        row().w_full().h_full().children([
            pane("pane-a", "scroll-a", "list-a"),
            pane("pane-b", "scroll-b", "list-b"),
        ])
    }
    let mut rt = Runtime::new(ScopedListsState::default(), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    while rt.frame(0.016) {}
    rt
}

fn scroll_offset_of(rt: &Runtime<ScopedListsState>, key: &str) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).expect(key).id;
    rt.scroll.scrolls[&id].offset
}

fn element_with_suffix(rt: &Runtime<ScopedListsState>, suffix: &str) -> Option<crate::ElementInfo> {
    rt.elements().into_iter().find(|e| e.path.ends_with(suffix))
}

#[test]
fn scroll_key_to_scoped_unmounted_virtual_row_mounts_target_list_without_running_view() {
    #[cfg(debug_assertions)]
    let _ = crate::tooling::perf_warn::take_log();
    let mut rt = two_scoped_virtual_lists();
    let (c0, v0) = (rt.stats.commits, rt.state.views.get());
    rt.update_if(|_, tx| { tx.scroll_key_to("list-b/row-500", 40.0); false });
    rt.frame(0.016);
    assert_eq!(rt.stats.commits - c0, 1, "scroll_key_to must remount once");
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_key_to must not run view");
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0, "list-a must stay at top");
    assert!(element_with_suffix(&rt, "list-a/row-500").is_none());
    let row = element_with_suffix(&rt, "list-b/row-500").expect("list-b/row-500 mounted");
    assert!((row.rect[1] - 40.0).abs() < 0.01, "row-500 anchored at y=40: {:?}", row.rect);

    let (c1, v1) = (rt.stats.commits, rt.state.views.get());
    rt.update_if(|_, tx| { tx.scroll_key_to("list-b/row-750/mark", 40.0); false });
    rt.frame(0.016);
    assert_eq!(rt.stats.commits - c1, 1, "scroll_key_to scope/row/mark must remount once");
    assert_eq!(rt.state.views.get() - v1, 0, "scroll_key_to scope/row/mark must not run view");
    let mark = element_with_suffix(&rt, "list-b/row-750/mark").expect("list-b/row-750/mark mounted");
    assert!((mark.rect[1] - 40.0).abs() < 0.01, "mark anchored at y=40: {:?}", mark.rect);
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0, "list-a still at top");

    // Settle remaining overscan fill, then verify non-matching scoped keys stay quiet no-ops.
    while rt.frame(0.016) {}
    let (c2, v2, off_b) = (rt.stats.commits, rt.state.views.get(), scroll_offset_of(&rt, "scroll-b"));
    rt.update_if(|_, tx| {
        tx.scroll_key_to("list-b/missing", 40.0);
        tx.scroll_key_to("missing/row-500", 40.0);
        false
    });
    rt.frame(0.016);
    assert_eq!((rt.stats.commits - c2, rt.state.views.get() - v2), (0, 0));
    assert_eq!(scroll_offset_of(&rt, "scroll-b"), off_b);
    #[cfg(debug_assertions)]
    assert!(
        !crate::tooling::perf_warn::take_log().iter().any(|l| l.contains("an instant scroll")),
        "scoped scroll_key_to must not trip instant-scroll perf warning"
    );
}

#[test]
fn scroll_key_center_at_scoped_unmounted_virtual_row_mounts_target_list_without_running_view() {
    #[cfg(debug_assertions)]
    let _ = crate::tooling::perf_warn::take_log();
    let mut rt = two_scoped_virtual_lists();
    let (c0, v0) = (rt.stats.commits, rt.state.views.get());
    rt.update_if(|_, tx| { tx.scroll_key_center_at("list-b/row-500", 100.0); false });
    rt.frame(0.016);
    assert_eq!(rt.stats.commits - c0, 1, "scroll_key_center_at must remount once");
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_key_center_at must not run view");
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0, "list-a must stay at top");
    let row = element_with_suffix(&rt, "list-b/row-500").expect("list-b/row-500 mounted");
    let mid = row.rect[1] + row.rect[3] * 0.5;
    assert!((mid - 100.0).abs() < 0.5, "row-500 midpoint at y=100: {:?}", row.rect);

    let (c1, v1) = (rt.stats.commits, rt.state.views.get());
    rt.update_if(|_, tx| { tx.scroll_key_center_at("list-b/row-750/mark", 100.0); false });
    rt.frame(0.016);
    assert_eq!(rt.stats.commits - c1, 1, "scroll_key_center_at scope/row/mark must remount once");
    assert_eq!(rt.state.views.get() - v1, 0, "scroll_key_center_at scope/row/mark must not run view");
    let mark = element_with_suffix(&rt, "list-b/row-750/mark").expect("list-b/row-750/mark mounted");
    let mark_mid = mark.rect[1] + mark.rect[3] * 0.5;
    assert!((mark_mid - 100.0).abs() < 0.5, "mark midpoint at y=100: {:?}", mark.rect);
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0);
    #[cfg(debug_assertions)]
    assert!(
        !crate::tooling::perf_warn::take_log().iter().any(|l| l.contains("an instant scroll")),
        "scoped scroll_key_center_at must not trip instant-scroll perf warning"
    );
}

#[test]
fn scroll_into_view_scoped_unmounted_virtual_row_mark_mounts_target_list_without_running_view() {
    let mut rt = two_scoped_virtual_lists();
    let (c0, v0) = (rt.stats.commits, rt.state.views.get());
    rt.update_if(|_, tx| { tx.scroll_into_view("list-b/row-500/mark"); false });
    rt.frame(0.016);
    assert_eq!(rt.stats.commits - c0, 1, "scroll_into_view must remount once");
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_into_view must not run view");
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0, "list-a must stay at top");
    let mark = element_with_suffix(&rt, "list-b/row-500/mark").expect("list-b/row-500/mark mounted");
    assert!(mark.rect[1] >= -0.01 && mark.rect[1] + mark.rect[3] <= 200.01, "mark visible: {:?}", mark.rect);
}

#[test]
fn scroll_into_view_in_scoped_unmounted_virtual_row_mark_mounts_target_list_without_running_view() {
    let mut rt = two_scoped_virtual_lists();
    let (c0, v0) = (rt.stats.commits, rt.state.views.get());
    rt.update_if(|_, tx| { tx.scroll_into_view_in("pane-b", "list-b/row-500/mark"); false });
    rt.frame(0.016);
    assert_eq!(rt.stats.commits - c0, 1, "scroll_into_view_in must remount once");
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_into_view_in must not run view");
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0, "list-a must stay at top");
    let mark = element_with_suffix(&rt, "list-b/row-500/mark").expect("list-b/row-500/mark mounted");
    assert!(mark.rect[1] >= -0.01 && mark.rect[1] + mark.rect[3] <= 200.01, "mark visible: {:?}", mark.rect);
}

#[test]
fn scroll_center_scoped_unmounted_virtual_row_mark_mounts_target_list_without_running_view() {
    let mut rt = two_scoped_virtual_lists();
    let v0 = rt.state.views.get();
    rt.update_if(|_, tx| { tx.scroll_center("list-b/row-500/mark"); false });
    for _ in 0..600 { if !rt.frame(1.0 / 60.0) { break; } }
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_center must not run view");
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0, "list-a must stay at top");
    let mark = element_with_suffix(&rt, "list-b/row-500/mark").expect("list-b/row-500/mark mounted");
    let mid = mark.rect[1] + mark.rect[3] * 0.5;
    assert!((mid - 100.0).abs() < 1.0, "mark centred in 200pt viewport: {:?}", mark.rect);
}

#[test]
fn scroll_center_in_scoped_unmounted_virtual_row_mark_mounts_target_list_without_running_view() {
    let mut rt = two_scoped_virtual_lists();
    let v0 = rt.state.views.get();
    rt.update_if(|_, tx| { tx.scroll_center_in("pane-b", "list-b/row-500/mark"); false });
    for _ in 0..600 { if !rt.frame(1.0 / 60.0) { break; } }
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_center_in must not run view");
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0, "list-a must stay at top");
    let mark = element_with_suffix(&rt, "list-b/row-500/mark").expect("list-b/row-500/mark mounted");
    let mid = mark.rect[1] + mark.rect[3] * 0.5;
    assert!((mid - 100.0).abs() < 1.0, "mark centred in 200pt viewport: {:?}", mark.rect);
}

#[test]
fn scroll_center_in_instant_scoped_unmounted_virtual_row_mark_mounts_target_list_without_running_view() {
    let mut rt = two_scoped_virtual_lists();
    let (c0, v0) = (rt.stats.commits, rt.state.views.get());
    rt.update_if(|_, tx| { tx.scroll_center_in_instant("pane-b", "list-b/row-500/mark"); false });
    rt.frame(0.016);
    assert_eq!(rt.stats.commits - c0, 1, "scroll_center_in_instant must remount once");
    assert_eq!(rt.state.views.get() - v0, 0, "scroll_center_in_instant must not run view");
    assert_eq!(scroll_offset_of(&rt, "scroll-a"), 0.0, "list-a must stay at top");
    let mark = element_with_suffix(&rt, "list-b/row-500/mark").expect("list-b/row-500/mark mounted");
    let mid = mark.rect[1] + mark.rect[3] * 0.5;
    assert!((mid - 100.0).abs() < 1.0, "mark centred in 200pt viewport: {:?}", mark.rect);
}

#[test]
fn scrolling_virtual_rows_with_pixel_aspect_image_skips_view_when_aspect_is_unchanged() {
    struct State {
        img: crate::Pixels,
        views: Cell<u32>,
    }
    let img = crate::Pixels::new();
    img.set(200, 100, vec![255; 200 * 100 * 4]);
    let view = |s: &State, _: &mut Cx<State>| {
        s.views.set(s.views.get() + 1);
        let list = col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
            200,
            40.0,
            |_: &State, i| Cow::Owned(format!("r{i}")),
            |s: &State, i| {
                if i % 20 == 0 {
                    // Markdown-style image: explicit width, auto height -> sets cold.pixel_aspect.
                    col().w_full().child(el().key("pic").w_full().max_w(200.0).pixels(&s.img))
                } else {
                    row().w_full().h(40.0).child(text(format!("Row {i}")).key("label"))
                }
            },
        );
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(list)
    };
    let mut rt = Runtime::new(State { img, views: Cell::new(0) }, view, (400.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}

    let v0 = rt.state.views.get();
    let mut remounts = 0u32;
    for _ in 0..30 {
        let c0 = rt.stats.commits;
        rt.wheel(200.0, 120.0, -80.0);
        rt.frame(1.0 / 120.0);
        if rt.stats.commits > c0 {
            remounts += 1;
        }
    }
    assert!(remounts > 0, "scrolling should trigger at least one virtual window remount");
    assert_eq!(
        rt.state.views.get() - v0,
        0,
        "virtual scroll with an unchanged pixel_aspect image must stay on CommitKind::Remount"
    );

    // Re-setting pixels with the SAME aspect ratio must keep virtual_only() == true.
    rt.state.img.set(400, 200, vec![200; 400 * 200 * 4]);
    assert!(
        rt.commit.virtual_only(&rt.tree, rt.frame.has_hot(), false),
        "same-aspect pixel update must keep virtual_only() == true"
    );

    // Changing the image's aspect ratio must make virtual_only() itself return false (Rebuild)
    // even before frame() sets commit.relayout.
    rt.state.img.set(100, 200, vec![128; 100 * 200 * 4]);
    assert!(
        !rt.commit.virtual_only(&rt.tree, rt.frame.has_hot(), false),
        "aspect ratio change on a mounted pixel_aspect image must make virtual_only() return false"
    );
    let v1 = rt.state.views.get();
    rt.frame(1.0 / 120.0);
    assert!(rt.state.views.get() > v1, "aspect ratio change must rebuild the view");
    let pic = rt.elements().into_iter().find(|e| e.path.ends_with("/pic")).expect("mounted pic after aspect change");
    assert!(
        (pic.rect[3] - 400.0).abs() < 1.0,
        "200pt-wide 1:2 portrait image should lay out at ~400pt tall, got {:?}",
        pic.rect
    );
}

#[test]
fn scrolling_into_first_pixel_aspect_image_via_remount_refreshes_flag_and_catches_aspect_change() {
    struct State {
        img: crate::Pixels,
        views: Cell<u32>,
    }
    let img = crate::Pixels::new();
    img.set(200, 100, vec![255; 200 * 100 * 4]);
    let view = |s: &State, _: &mut Cx<State>| {
        s.views.set(s.views.get() + 1);
        let list = col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
            200,
            40.0,
            |_: &State, i| Cow::Owned(format!("r{i}")),
            |s: &State, i| {
                if i == 40 {
                    col().w_full().child(el().key("pic").w_full().max_w(200.0).pixels(&s.img))
                } else {
                    row().w_full().h(40.0).child(text(format!("Row {i}")).key("label"))
                }
            },
        );
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(list)
    };
    let mut rt = Runtime::new(State { img, views: Cell::new(0) }, view, (400.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    assert!(!rt.tree.has_pixel_aspect, "initial window has no pixel_aspect image mounted");

    let v0 = rt.state.views.get();
    for _ in 0..40 {
        rt.wheel(200.0, 120.0, -80.0);
        rt.frame(1.0 / 120.0);
        if rt.elements().iter().any(|e| e.path.ends_with("/pic")) {
            break;
        }
    }
    assert_eq!(rt.state.views.get() - v0, 0, "scrolling into first pixel_aspect row must stay on Remount");
    assert!(rt.tree.has_pixel_aspect, "remounting a pixel_aspect row must refresh tree.has_pixel_aspect");
    let pic = rt.elements().into_iter().find(|e| e.path.ends_with("/pic")).expect("row 40 pic mounted");
    assert!((pic.rect[3] - 100.0).abs() < 1.0, "200x100 image should lay out at 100pt tall, got {:?}", pic.rect);

    rt.state.img.set(100, 200, vec![64; 100 * 200 * 4]);
    assert!(
        !rt.commit.virtual_only(&rt.tree, rt.frame.has_hot(), false),
        "aspect change on remount-entered pixel_aspect image must make virtual_only() return false"
    );
}
