//! `El::scroll_anchor`: rows inserted or grown above the view leave the row at its top where
//! it was; without it, or at the very top, the content moves, and a viewport stuck to its
//! bottom keeps following it.
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, text};

#[derive(Clone, Copy)]
struct Feed {
    /// Rows added above the first.
    above: usize,
    /// Height of the first row.
    first_h: f32,
    /// Rows added below the last.
    below: usize,
    anchor: bool,
    stick_bottom: bool,
}

fn view(s: &Feed, _: &mut Cx<Feed>) -> El {
    let rows = (0..s.above).map(|k| el().key(format!("new{k}")).w_full().h(30.0))
        .chain(std::iter::once(el().key("r0").w_full().h(s.first_h)))
        .chain((1..30 + s.below).map(|k| el().key(format!("r{k}")).w_full().h(40.0)));
    let list = col().key("list").w_full().children(rows.collect::<Vec<_>>());
    let viewport = col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: s.stick_bottom }).child(list);
    if s.anchor { viewport.scroll_anchor() } else { viewport }
}

fn top_of(rt: &Runtime<Feed>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap().rect[1]
}

/// Scrolls to `at`, applies `change`, and returns where row 5's top was and is.
fn shift(feed: Feed, at: f32, change: impl FnOnce(&mut Feed) + 'static) -> (f32, f32) {
    let mut rt = Runtime::new(feed, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(move |_, tx| tx.scroll_to_instant("scroll", 0.0, at));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let before = top_of(&rt, "r5");
    rt.update(move |s, _| change(s));
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    (before, top_of(&rt, "r5"))
}

const FEED: Feed = Feed { above: 0, first_h: 40.0, below: 0, anchor: true, stick_bottom: false };

#[test]
fn rows_inserted_above_leave_the_top_row_in_place() {
    let (before, after) = shift(FEED, 150.0, |s| s.above = 3);
    assert!((after - before).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn a_row_above_that_grows_leaves_the_top_row_in_place() {
    let (before, after) = shift(FEED, 150.0, |s| s.first_h = 140.0);
    assert!((after - before).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn without_the_anchor_the_rows_move_down() {
    let (before, after) = shift(Feed { anchor: false, ..FEED }, 150.0, |s| s.above = 3);
    assert!((after - before - 90.0).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn at_the_very_top_new_rows_show() {
    let (before, after) = shift(FEED, 0.0, |s| s.above = 3);
    assert!((after - before - 90.0).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn a_viewport_following_its_bottom_keeps_following_it() {
    let (before, after) = shift(Feed { stick_bottom: true, ..FEED }, 10_000.0, |s| s.below = 3);
    assert!((after - before + 120.0).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn no_scroll_anchor_on_child_and_subtree_skips_them_for_next_visible_sibling() {
    struct S {
        banner_h: f32,
        group_h: f32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let list = col().key("list").w_full()
            .child(el().key("above").w_full().h(200.0))
            .child(el().key("banner").w_full().h(s.banner_h).no_scroll_anchor())
            .child(
                col().key("opted_group").w_full().no_scroll_anchor()
                    .child(el().key("opted_child").w_full().h(s.group_h)),
            )
            .child(el().key("keep_me").w_full().h(40.0))
            .children((0..20).map(|i| el().key(format!("tail{i}")).w_full().h(40.0)));
        col().key("scroll").size(300.0, 200.0)
            .scroll(ScrollSpec::default())
            .scroll_anchor()
            .child(list)
    }
    let mut rt = Runtime::new(S { banner_h: 40.0, group_h: 40.0 }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Scroll to 210: banner (-10..30) and opted_group (30..70) are visible above keep_me (70..110),
    // but both are marked no_scroll_anchor(), so keep_me becomes the anchor.
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 210.0));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let y = |rt: &Runtime<S>, k: &str| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).unwrap().rect[1]
    };
    let before = y(&rt, "keep_me");
    assert!((before - 70.0).abs() < 0.5, "keep_me starts at 70, got {before}");
    rt.update(|s, _| { s.banner_h = 120.0; s.group_h = 100.0; });
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    let after = y(&rt, "keep_me");
    assert!((after - before).abs() < 0.5, "keep_me moved from {before} to {after}");
}

#[test]
fn no_scroll_anchor_on_scroll_viewport_disables_anchoring() {
    fn view(above: &usize, _: &mut Cx<usize>) -> El {
        let rows = (0..*above).map(|k| el().key(format!("new{k}")).w_full().h(30.0))
            .chain((0..30).map(|k| el().key(format!("r{k}")).w_full().h(40.0)));
        col().key("scroll").size(300.0, 200.0)
            .scroll(ScrollSpec::default())
            .scroll_anchor()
            .no_scroll_anchor()
            .child(col().key("list").w_full().children(rows))
    }
    let mut rt = Runtime::new(0usize, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 150.0));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let y = |rt: &Runtime<usize>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r5")).unwrap().rect[1];
    let before = y(&rt);
    rt.update(|n, _| *n = 3);
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    let after = y(&rt);
    assert!((after - before - 90.0).abs() < 0.5, "no_scroll_anchor viewport must not anchor: {before} -> {after}");
}

#[test]
fn no_scroll_anchor_on_virtual_row_anchors_first_real_row_on_prepend() {
    struct Chat {
        keys: Vec<String>,
        use_virtualize: bool,
    }
    fn view(s: &Chat, _: &mut Cx<Chat>) -> El {
        let stack = col().key("rows").w_full().growth(GrowthSpec::default());
        let stack = if s.use_virtualize {
            stack.virtualize(40.0).children(s.keys.iter().map(|k| {
                let row = el().key(k.clone()).w_full().h(40.0);
                if k == "loading" { row.no_scroll_anchor() } else { row }
            }))
        } else {
            stack.virtual_rows(
                s.keys.len(),
                40.0,
                |s: &Chat, i| s.keys[i].as_str().into(),
                |s: &Chat, i| {
                    let row = el().w_full().h(40.0);
                    if s.keys[i] == "loading" { row.no_scroll_anchor() } else { row }
                },
            )
        };
        col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec::default()).child(stack)
    }
    for use_virtualize in [false, true] {
        let mut keys = vec!["loading".to_owned()];
        keys.extend((100..130).map(|i| format!("m{i}")));
        let mut rt = Runtime::new(Chat { keys, use_virtualize }, view, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let y = |rt: &Runtime<Chat>, k: &str| {
            rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).map(|e| e.rect[1])
        };
        assert_eq!(y(&rt, "loading"), Some(0.0));
        let m100_before = y(&rt, "m100").expect("m100 visible");
        assert!((m100_before - 40.0).abs() < 0.5, "m100 at {m100_before}");

        // Insert 15 older messages beneath the loading placeholder (at index 1).
        // Without no_scroll_anchor on "loading", "loading" stays at y=0 and m100 jumps 600pt down.
        // With no_scroll_anchor on "loading", m100 stays at y=40 and "loading" moves above view.
        rt.update(|s, _| {
            let older: Vec<String> = (85..100).map(|i| format!("m{i}")).collect();
            s.keys.splice(1..1, older);
        });
        for _ in 0..20 { rt.frame(1.0 / 120.0); }
        let m100_after = y(&rt, "m100").expect("m100 still visible");
        assert!(
            (m100_after - m100_before).abs() < 0.5,
            "use_virtualize={use_virtualize}: m100 moved from {m100_before} to {m100_after}"
        );
        let loading_y = y(&rt, "loading");
        assert!(
            loading_y.is_none_or(|top| top < -100.0),
            "use_virtualize={use_virtualize}: loading placeholder should scroll out above view, got {loading_y:?}"
        );
    }
}

#[test]
fn no_scroll_anchor_on_virtual_rows_list_or_viewport_reveals_prepends_at_offset_zero() {
    #[derive(Clone, Copy)]
    enum OptOut {
        List,
        Viewport,
    }
    struct S {
        front: usize,
        opt_out: Option<OptOut>,
    }
    fn key(s: &S, i: usize) -> String {
        if i < s.front { format!("a{}", s.front - 1 - i) } else { format!("b{:02}", i - s.front) }
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut list = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(s.front + 30, 20.0, |s: &S, i| key(s, i).into(), |_: &S, _| el().w_full().h(20.0));
        if matches!(s.opt_out, Some(OptOut::List)) {
            list = list.no_scroll_anchor();
        }
        let mut vp = col().key("vp").size(300.0, 200.0).scroll(ScrollSpec::default()).child(list);
        if matches!(s.opt_out, Some(OptOut::Viewport)) {
            vp = vp.no_scroll_anchor();
        }
        vp
    }
    for mode in [OptOut::List, OptOut::Viewport] {
        let mut rt = Runtime::new(S { front: 0, opt_out: Some(mode) }, view, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(0.016);
        rt.update(|s, _| s.front = 2);
        for _ in 0..30 { rt.frame(0.016); }
        let y = |rt: &Runtime<S>, k: &str| {
            rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).map(|e| e.rect[1])
        };
        assert_eq!(y(&rt, "a1"), Some(0.0), "newly inserted top row shows at offset 0");
        assert_eq!(y(&rt, "b00"), Some(40.0), "previous top row shifts down by 2 * 20pt");

        // Re-enabling anchoring on a later commit restores default prepend anchoring when new
        // rows prepend above a1/a0/b00.
        rt.update(|s, _| s.opt_out = None);
        rt.frame(0.016);
        let a1_before = y(&rt, "a1").unwrap();
        rt.update(|s, _| s.front = 5);
        for _ in 0..30 { rt.frame(0.016); }
        let a1_after = y(&rt, "a1").unwrap();
        let b00_after = y(&rt, "b00").unwrap();
        assert!((a1_after - a1_before).abs() < 0.5, "re-enabled anchoring holds top row ({a1_before} -> {a1_after})");
        assert!((b00_after - 40.0).abs() < 0.5, "re-enabled anchoring holds b00 (40 -> {b00_after})");
    }
}

#[test]
fn no_scroll_anchor_handles_empty_list_and_all_excluded_rows() {
    struct S {
        keys: Vec<String>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let list = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(
                s.keys.len(),
                30.0,
                |s: &S, i| s.keys[i].as_str().into(),
                |_: &S, _| el().w_full().h(30.0).no_scroll_anchor(),
            );
        col().key("vp").size(300.0, 150.0).scroll(ScrollSpec::default()).scroll_anchor().child(list)
    }
    let mut rt = Runtime::new(S { keys: Vec::new() }, view, (300.0, 150.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.keys = vec!["r0".into(), "r1".into(), "r2".into()]);
    rt.frame(0.016);
    rt.update(|s, _| { s.keys.insert(0, "new0".into()); s.keys.insert(1, "new1".into()); });
    for _ in 0..10 { rt.frame(0.016); }
    let y = |k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).map(|e| e.rect[1]);
    assert_eq!(y("new0"), Some(0.0), "with every row excluded, prepends stay at top without shifting");
    assert_eq!(y("r0"), Some(60.0));
}

#[test]
fn virtual_scroll_anchor_holds_surviving_descendant_on_interior_height_change() {
    fn view(_: &bool, _: &mut Cx<bool>) -> El {
        let rows = col().key("rows").growth(GrowthSpec::default())
            .virtual_rows(1, 700.0, |_: &bool, _| "message".into(), |hide: &bool, _| {
                col().child(col().h(300.0))
                    .children((!*hide).then(|| col().h(120.0)))
                    .child(text("Current passage").key("passage"))
                    .child(col().h(600.0))
            });
        col().key("scroll").h(220.0).w(400.0)
            .scroll(ScrollSpec::default())
            .scroll_anchor()
            .child(rows)
    }
    for reduced_motion in [false, true] {
        let mut rt = Runtime::new(false, view, (400.0, 220.0), Env::new(reduced_motion, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_key_to("message/passage", 20.0));
        for _ in 0..20 { rt.frame(0.016); }
        let y = |rt: &Runtime<bool>| {
            rt.elements().into_iter().find(|e| e.key.as_deref() == Some("passage")).unwrap().rect[1]
        };
        let before = y(&rt);
        assert!((before - 20.0).abs() < 1.0, "reduced_motion={reduced_motion}: initial y {before}");

        // Collapse 120pt block above passage inside the same mounted virtual row.
        rt.update(|hide, _| *hide = true);
        for frame in 0..20 {
            rt.frame(0.016);
            let now = y(&rt);
            assert!(
                (before - now).abs() < 1.0,
                "reduced_motion={reduced_motion} collapse frame {frame}: {before} -> {now}"
            );
        }

        // Re-expand the 120pt block above passage inside the same mounted virtual row.
        rt.update(|hide, _| *hide = false);
        for frame in 0..20 {
            rt.frame(0.016);
            let now = y(&rt);
            assert!(
                (before - now).abs() < 1.0,
                "reduced_motion={reduced_motion} expand frame {frame}: {before} -> {now}"
            );
        }
    }
}

#[test]
fn virtual_scroll_anchor_skips_no_scroll_anchor_interior_descendant() {
    fn view(_: &f32, _: &mut Cx<f32>) -> El {
        let rows = col().key("rows").growth(GrowthSpec::default())
            .virtual_rows(1, 700.0, |_: &f32, _| "message".into(), |banner_h: &f32, _| {
                col()
                    .child(col().key("lead").h(300.0))
                    .child(col().key("banner").h(*banner_h).no_scroll_anchor())
                    .child(text("Current passage").key("passage"))
                    .child(col().key("tail").h(600.0))
            });
        col().key("scroll").size(400.0, 220.0)
            .scroll(ScrollSpec::default())
            .scroll_anchor()
            .child(rows)
    }
    let mut rt = Runtime::new(60.0f32, view, (400.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Scroll to 320: banner is visible at -20..40 (excluded), passage is at y=40.
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 320.0));
    for _ in 0..4 { rt.frame(0.016); }
    let y = |rt: &Runtime<f32>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("passage")).unwrap().rect[1];
    let before = y(&rt);
    assert!((before - 40.0).abs() < 1.0, "before={before}");

    rt.update(|h, _| *h = 180.0);
    for _ in 0..20 { rt.frame(0.016); }
    let after = y(&rt);
    assert!((after - before).abs() < 1.0, "banner with no_scroll_anchor must not steal anchor: {before} -> {after}");
}

#[test]
fn virtual_scroll_anchor_no_double_shift_when_row_above_and_interior_change_together() {
    struct S {
        above_h: f32,
        hide_interior: bool,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        let rows = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(
                2,
                400.0,
                |_: &S, i| if i == 0 { "r0".into() } else { "r1".into() },
                |s: &S, i| {
                    if i == 0 {
                        el().w_full().h(s.above_h)
                    } else {
                        col().w_full()
                            .child(col().key("top").w_full().h(300.0))
                            .children((!s.hide_interior).then(|| col().key("tool").w_full().h(120.0)))
                            .child(text("Current passage").key("passage"))
                            .child(col().key("tail").w_full().h(600.0))
                    }
                },
            );
        col().key("scroll").size(400.0, 220.0)
            .scroll(ScrollSpec::default())
            .scroll_anchor()
            .child(rows)
    }
    for reduced_motion in [false, true] {
        let mut rt = Runtime::new(S { above_h: 200.0, hide_interior: false }, view, (400.0, 220.0), Env::new(reduced_motion, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_key_to("r1/passage", 20.0));
        for _ in 0..20 { rt.frame(0.016); }
        let y = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("passage")).unwrap().rect[1];
        let before = y(&rt);
        assert!((before - 20.0).abs() < 1.0, "before={before}");

        // Simultaneous +100pt growth in r0 above AND -120pt collapse inside r1 above passage.
        rt.update(|s, _| { s.above_h = 300.0; s.hide_interior = true; });
        for frame in 0..30 {
            rt.frame(0.016);
            let now = y(&rt);
            assert!(
                (before - now).abs() < 1.0,
                "reduced_motion={reduced_motion} frame {frame}: double-shift or drift {before} -> {now}"
            );
        }
    }
}

#[test]
fn virtual_scroll_anchor_falls_back_when_tall_row_unmounts_on_large_prepend() {
    struct S {
        prepended: usize,
        hide_interior: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let total = s.prepended + 1;
        let rows = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(
                total,
                100.0,
                |s: &S, i| {
                    if i < s.prepended { format!("old{i}").into() } else { "message".into() }
                },
                |s: &S, i| {
                    if i < s.prepended {
                        el().w_full().h(100.0)
                    } else {
                        col().w_full()
                            .child(col().key("top").w_full().h(300.0))
                            .children((!s.hide_interior).then(|| col().key("tool").w_full().h(120.0)))
                            .child(text("Current passage").key("passage"))
                            .child(col().key("tail").w_full().h(600.0))
                    }
                },
            );
        col().key("scroll").size(400.0, 220.0)
            .scroll(ScrollSpec::default())
            .scroll_anchor()
            .child(rows)
    }
    for also_hide in [false, true] {
        let mut rt = Runtime::new(S { prepended: 0, hide_interior: false }, view, (400.0, 220.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_key_to("message/passage", 20.0));
        for _ in 0..10 { rt.frame(0.016); }
        let y = |rt: &Runtime<S>| {
            rt.elements().into_iter().find(|e| e.key.as_deref() == Some("passage")).map(|e| e.rect[1])
        };
        let before = y(&rt).expect("passage visible");
        assert!((before - 20.0).abs() < 1.0, "before={before}");

        // Prepend 80 rows (8000pt) pushing "message" outside the mounted window, optionally also
        // collapsing the 120pt block inside "message" in the same commit.
        rt.update(move |s, _| {
            s.prepended = 80;
            s.hide_interior = also_hide;
        });
        for _ in 0..30 { rt.frame(0.016); }
        let after = y(&rt).expect("message remounted via virtual_row fallback");
        assert!(
            (before - after).abs() < 1.0,
            "also_hide={also_hide}: unmounted fallback jumped {before} -> {after}"
        );
    }
}

#[test]
fn virtual_scroll_anchor_preserves_whole_row_anchoring_when_row_starts_in_view() {
    struct S {
        prepended: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let rows = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(
                s.prepended + 40,
                40.0,
                |s: &S, i| {
                    if i < s.prepended { format!("p{i}").into() } else { format!("r{}", i - s.prepended).into() }
                },
                |_: &S, _| el().w_full().h(40.0),
            );
        col().key("scroll").size(300.0, 200.0)
            .scroll(ScrollSpec::default())
            .scroll_anchor()
            .child(rows)
    }
    let mut rt = Runtime::new(S { prepended: 0 }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Scroll to exact top of r10 (y=0.0): whole-row virtual anchoring owns this.
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 400.0));
    for _ in 0..4 { rt.frame(0.016); }
    let y = |rt: &Runtime<S>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).unwrap().rect[1];
    assert!((y(&rt, "r10") - 0.0).abs() < 0.5);

    rt.update(|s, _| s.prepended = 25);
    for _ in 0..20 { rt.frame(0.016); }
    assert!((y(&rt, "r10") - 0.0).abs() < 0.5, "r10 stayed at viewport top: {}", y(&rt, "r10"));
}

