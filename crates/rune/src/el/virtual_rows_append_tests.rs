use std::borrow::Cow;
use std::cell::Cell;
use std::ops::Range;

use crate::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, memo, text};

thread_local! {
    static KEY_CALLS: Cell<usize> = const { Cell::new(0) };
}

fn count_append_key_calls(initial: usize, added: usize, append_only: bool, stick_bottom: bool) -> (usize, Option<Range<usize>>) {
    struct S {
        keys: Vec<String>,
        append_only: bool,
        stick_bottom: bool,
        visible: Option<Range<usize>>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut list = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(
                s.keys.len(),
                20.0,
                |s: &S, i| {
                    KEY_CALLS.set(KEY_CALLS.get() + 1);
                    Cow::Borrowed(s.keys[i].as_str())
                },
                |s: &S, i| text(s.keys[i].clone()).h(20.0),
            )
            .virtual_rows_version(0u32)
            .on_visible_rows(|s: &mut S, r| {
                s.visible = Some(r);
                false
            });
        if s.append_only {
            list = list.virtual_rows_append_only();
        }
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: s.stick_bottom }).child(list)
    }
    let keys = (0..initial).map(|i| format!("r{i}")).collect();
    let mut rt = Runtime::new(S { keys, append_only, stick_bottom, visible: None }, view, (300.0, 400.0), Env::default());
    rt.frame(0.0);
    // Let overscan fill settle before measuring an append commit.
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    KEY_CALLS.set(0);
    rt.update(|s, _| {
        let start = s.keys.len();
        for i in start..start + added {
            s.keys.push(format!("r{i}"));
        }
    });
    rt.frame(1.0 / 60.0);
    let calls = KEY_CALLS.get();
    if stick_bottom {
        let last = format!("r{}", initial + added - 1);
        assert!(rt.find(&last).is_some(), "appended tail row {last} must mount when pinned");
    }
    (calls, rt.state.visible.clone())
}

#[test]
fn append_only_reads_only_new_keys_while_default_reads_all() {
    let (default_calls, default_vis) = count_append_key_calls(10_000, 1, false, true);
    assert_eq!(default_calls, 10_001, "without opt-in, appending reads every key");

    let (fast_10k, fast_vis) = count_append_key_calls(10_000, 1, true, true);
    let (fast_50k, _) = count_append_key_calls(50_000, 1, true, true);
    assert_eq!(fast_vis, default_vis, "on_visible_rows reports the same range on append");
    assert!(
        fast_10k < 200,
        "with virtual_rows_append_only, appending 1 row to 10,000 must not scan the list (got {fast_10k} calls)"
    );
    assert_eq!(
        fast_10k, fast_50k,
        "append key calls must be independent of list length (10k={fast_10k}, 50k={fast_50k})"
    );
    #[cfg(not(debug_assertions))]
    assert_eq!(fast_10k, 4, "release builds check start, midpoint, end and the 1 new key");
}

#[test]
fn append_only_works_without_virtual_rows_version() {
    struct S { keys: Vec<String> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: true }).child(
            col().key("rows").w_full().growth(GrowthSpec::default())
                .virtual_rows(
                    s.keys.len(),
                    20.0,
                    |s: &S, i| {
                        KEY_CALLS.set(KEY_CALLS.get() + 1);
                        Cow::Borrowed(s.keys[i].as_str())
                    },
                    |s: &S, i| text(s.keys[i].clone()).h(20.0),
                )
                .virtual_rows_append_only(),
        )
    }
    let keys = (0..10_000).map(|i| format!("r{i}")).collect();
    let mut rt = Runtime::new(S { keys }, view, (300.0, 400.0), Env::default());
    rt.frame(0.0);
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    KEY_CALLS.set(0);
    rt.update(|s, _| {
        for i in 10_000..10_005 {
            s.keys.push(format!("r{i}"));
        }
    });
    rt.frame(1.0 / 60.0);
    let calls = KEY_CALLS.get();
    assert!(calls < 200, "append_only without version still skips scanning all keys (got {calls})");
    assert!(rt.find("r10004").is_some());
}

#[test]
fn append_only_preserves_anchor_and_measured_heights_when_scrolled() {
    struct S {
        keys: Vec<String>,
        estimate: f32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("rows").w_full().growth(GrowthSpec::default())
                .virtual_rows(
                    s.keys.len(),
                    s.estimate,
                    |s: &S, i| Cow::Borrowed(s.keys[i].as_str()),
                    |_: &S, _| el().w_full().h(36.0),
                )
                .virtual_rows_version(0u32)
                .virtual_rows_append_only(),
        )
    }
    let keys = (0..2_000).map(|i| format!("r{i}")).collect();
    let mut rt = Runtime::new(S { keys, estimate: 20.0 }, view, (300.0, 400.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 10_000.0));
    for _ in 0..10 {
        rt.frame(1.0 / 60.0);
    }
    let anchor_row = rt.elements().into_iter()
        .find(|e| e.path.starts_with("scroll/rows/r") && e.visible[3] > 0.0)
        .expect("a visible row");
    let anchor_key = anchor_row.path.rsplit('/').next().unwrap().to_owned();
    let anchor_top = anchor_row.rect[1];
    let before_max = rt.commit.virtuals()["scroll/rows"].max_offset();

    rt.update(|s, _| {
        for i in 2_000..2_020 {
            s.keys.push(format!("r{i}"));
        }
    });
    rt.frame(1.0 / 60.0);

    let after = rt.elements().into_iter()
        .find(|e| e.path == anchor_row.path)
        .expect("anchor row stays mounted");
    assert_eq!(after.rect[1], anchor_top, "appending at the end must not move the visible anchor");
    let v = &rt.commit.virtuals()["scroll/rows"];
    assert_eq!(v.height(&anchor_key), 36.0, "measured height persists across append");
    assert!((v.max_offset() - (before_max + 20.0 * 20.0)).abs() < 0.01, "extent grows by 20 estimated rows");

    // Changing the estimate in the same commit as an append still compensates unmeasured rows above.
    rt.update(|s, tx| {
        s.estimate = 25.0;
        s.keys.push("r2020".into());
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    let after_est = rt.elements().into_iter()
        .find(|e| e.path == anchor_row.path)
        .expect("anchor row stays mounted after estimate change");
    assert!(
        (after_est.rect[1] - anchor_top).abs() < 0.5,
        "anchor row stays at same viewport y when estimate grows on append (was {anchor_top}, now {})",
        after_est.rect[1],
    );
}

#[test]
fn append_only_with_version_bump_or_shrink_takes_full_path_without_warning() {
    #[cfg(debug_assertions)]
    crate::tooling::perf_warn::take_log();

    struct S { version: u32, keys: Vec<String> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("rows-versioned").w_full().growth(GrowthSpec::default())
                .virtual_rows(
                    s.keys.len(),
                    20.0,
                    |s: &S, i| Cow::Borrowed(s.keys[i].as_str()),
                    |s: &S, i| text(s.keys[i].clone()).h(20.0),
                )
                .virtual_rows_version(s.version)
                .virtual_rows_append_only(),
        )
    }
    let keys = (0..500).map(|i| format!("a{i}")).collect();
    let mut rt = Runtime::new(S { version: 0, keys }, view, (300.0, 400.0), Env::default());
    rt.frame(0.0);

    // Replacing with a longer list (e.g. clearing a filter) + version bump takes the full path cleanly.
    rt.update(|s, _| {
        s.version = 1;
        s.keys = (0..1_000).map(|i| format!("b{i}")).collect();
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.find("b0").is_some() && rt.find("a0").is_none());

    // Shrinking the list without a version bump also takes the full path cleanly.
    rt.update(|s, _| {
        s.keys = (0..100).map(|i| format!("c{i}")).collect();
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.find("c0").is_some() && rt.find("b0").is_none());

    #[cfg(debug_assertions)]
    {
        let log: Vec<_> = crate::tooling::perf_warn::take_log()
            .into_iter()
            .filter(|m| m.contains("append-only"))
            .collect();
        assert!(log.is_empty(), "expected no append-only warning on version bump or shrink: {log:?}");
    }
}

#[test]
fn append_only_duplicate_new_key_falls_back_safely() {
    struct S { keys: Vec<String> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: true }).child(
            col().key("rows-dup").w_full().growth(GrowthSpec::default())
                .virtual_rows(
                    s.keys.len(),
                    20.0,
                    |s: &S, i| Cow::Borrowed(s.keys[i].as_str()),
                    |s: &S, i| text(s.keys[i].clone()).h(20.0),
                )
                .virtual_rows_version(0u32)
                .virtual_rows_append_only(),
        )
    }
    let keys = (0..100).map(|i| format!("r{i}")).collect();
    let mut rt = Runtime::new(S { keys }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);

    // Appending an already-existing key falls back to full rekey and renames the duplicate.
    rt.update(|s, _| s.keys.push("r5".into()));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.commit.virtuals()["scroll/rows-dup"].keys.len(), 101);
    assert!(rt.find("r5#dup1").is_some());

    // Appending two identical brand-new keys also falls back cleanly without panicking.
    rt.update(|s, _| {
        s.keys.push("dup".into());
        s.keys.push("dup".into());
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.commit.virtuals()["scroll/rows-dup"].keys.len(), 103);
    assert!(rt.find("dup").is_some() && rt.find("dup#dup1").is_some());
}

#[test]
fn append_only_broken_promise_middle_row_warns_once_and_stays_safe() {
    #[cfg(debug_assertions)]
    crate::tooling::perf_warn::take_log();

    struct S { keys: Vec<String> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("rows-broken-mid").w_full().growth(GrowthSpec::default())
                .virtual_rows(
                    s.keys.len(),
                    20.0,
                    |s: &S, i| Cow::Borrowed(s.keys[i].as_str()),
                    |s: &S, i| text(s.keys[i].clone()).h(20.0),
                )
                .virtual_rows_version(0u32)
                .virtual_rows_append_only(),
        )
    }
    let keys = (0..1_000).map(|i| format!("r{i}")).collect();
    let mut rt = Runtime::new(S { keys }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);

    // Break the promise at the midpoint while appending.
    rt.update(|s, _| {
        s.keys[500] = "changed-500".into();
        s.keys.push("r1000".into());
    });
    rt.frame(1.0 / 60.0);
    let v = &rt.commit.virtuals()["scroll/rows-broken-mid"];
    assert_eq!(v.keys.len(), 1_001);
    assert_eq!(v.index_of("changed-500"), Some(500), "fallback refreshes the changed middle key");
    assert_eq!(v.index_of("r1000"), Some(1_000));

    #[cfg(debug_assertions)]
    {
        let log: Vec<_> = crate::tooling::perf_warn::take_log()
            .into_iter()
            .filter(|m| m.contains("append-only"))
            .collect();
        assert_eq!(log.len(), 1, "expected one debug warning: {log:?}");
        assert!(
            log[0].contains("rows-broken-mid")
                && log[0].contains("row 500")
                && log[0].contains("r500")
                && log[0].contains("changed-500")
                && log[0].contains("virtual_rows_append_only"),
            "{}",
            log[0]
        );
    }

    // Breaking the promise a second time on the same list still recovers safely and does not spam.
    rt.update(|s, _| {
        s.keys[500] = "changed-again".into();
        s.keys.push("r1001".into());
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.commit.virtuals()["scroll/rows-broken-mid"].index_of("changed-again"), Some(500));

    #[cfg(debug_assertions)]
    {
        let log: Vec<_> = crate::tooling::perf_warn::take_log()
            .into_iter()
            .filter(|m| m.contains("append-only"))
            .collect();
        assert!(log.is_empty(), "same list must warn only once: {log:?}");
    }
}

#[test]
fn append_only_broken_unmounted_middle_row_is_safe_and_warns_when_scrolled_into_view() {
    #[cfg(debug_assertions)]
    crate::tooling::perf_warn::take_log();

    struct S { keys: Vec<String> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("rows-broken-offscreen").w_full().growth(GrowthSpec::default())
                .virtual_rows(
                    s.keys.len(),
                    20.0,
                    |s: &S, i| Cow::Borrowed(s.keys[i].as_str()),
                    |s: &S, i| text(s.keys[i].clone()).h(20.0),
                )
                .virtual_rows_version(0u32)
                .virtual_rows_append_only(),
        )
    }
    let keys = (0..1_000).map(|i| format!("r{i}")).collect();
    let mut rt = Runtime::new(S { keys }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }

    // Mutate an unmounted non-probe middle row (index 333) alongside an append.
    rt.update(|s, _| {
        s.keys[333] = "changed-333".into();
        s.keys.push("r1000".into());
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.commit.virtuals()["scroll/rows-broken-offscreen"].keys.len(), 1_001);

    // Scrolling to row 333 remains safe and (in debug builds) warns once when row 333 mounts.
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 333.0 * 20.0));
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    #[cfg(debug_assertions)]
    {
        let log: Vec<_> = crate::tooling::perf_warn::take_log()
            .into_iter()
            .filter(|m| m.contains("append-only"))
            .collect();
        assert_eq!(log.len(), 1, "expected one debug warning when stale row mounts: {log:?}");
        assert!(
            log[0].contains("rows-broken-offscreen")
                && log[0].contains("row 333")
                && log[0].contains("r333")
                && log[0].contains("changed-333"),
            "{}",
            log[0]
        );
    }
}

#[test]
fn append_only_inside_memo_region_expands_and_mounts_new_rows() {
    struct S { keys: crate::Tracked<Vec<String>> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: true }).child(
            crate::memo::region("list-region", || {
                let len = s.keys.len();
                col().key("rows").w_full().growth(GrowthSpec::default())
                    .virtual_rows(
                        len,
                        20.0,
                        |s: &S, i| Cow::Borrowed(crate::Tracked::untracked(&s.keys)[i].as_str()),
                        |s: &S, i| {
                            let k = crate::Tracked::untracked(&s.keys)[i].clone();
                            memo(k.clone(), 0u32, || text(k).h(20.0))
                        },
                    )
                    .virtual_rows_version(0u32)
                    .virtual_rows_append_only()
            }),
        )
    }
    let keys = crate::Tracked::new((0..1_000).map(|i| format!("r{i}")).collect());
    let mut rt = Runtime::new(S { keys }, view, (300.0, 400.0), Env::default());
    rt.frame(0.0);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("r999").is_some());

    rt.update(|s, _| s.keys.push("r1000".into()));
    rt.frame(1.0 / 60.0);
    assert!(rt.find("r1000").is_some(), "appended row inside a memo region must mount");
}


