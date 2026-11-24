use std::sync::atomic::{AtomicUsize, Ordering};

use rune::ScrollBars;

use super::*;

pub(super) fn at(offset: f32, max: f32, shown: f32) -> ScrollPos {
    let mut at = ScrollPos::default();
    (at.offset, at.max, at.viewport) = ([0.0, offset], [0.0, max], [200.0, shown]);
    at
}

#[test]
fn the_thumb_is_as_long_as_the_share_shown_and_travels_the_track() {
    assert_eq!(thumb(at(0.0, 300.0, 100.0), 100.0), (0.0, 25.0));
    assert_eq!(thumb(at(300.0, 300.0, 100.0), 100.0), (75.0, 25.0));
    assert_eq!(thumb(at(0.0, 10_000.0, 100.0), 100.0).1, MIN_THUMB, "never shorter than the minimum");
}

pub(super) fn view(rows: &usize, _: &mut rune::Cx<usize>) -> El {
    let k = Kit::neutral(true);
    col()
        .size(200.0, 100.0)
        .child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..*rows).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                .scroll_bar(&k),
        )
}

pub(super) fn info<S>(rt: &rune::Runtime<S>, key: &str) -> rune::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

pub(super) fn rect<S>(rt: &rune::Runtime<S>, key: &str) -> [f32; 4] {
    info(rt, key).rect
}

pub(super) fn opacity<S>(rt: &rune::Runtime<S>, key: &str) -> f32 {
    info(rt, key).opacity
}

pub(super) fn top<S>(rt: &rune::Runtime<S>) -> f32 {
    rect(rt, "row0")[1]
}

#[test]
fn a_wrapped_viewport_lays_out_as_the_bare_one_in_a_grow_column_without_min_h() {
    fn pane(wrapped: &bool, _: &mut rune::Cx<bool>) -> El {
        let k = Kit::neutral(true);
        let viewport = col()
            .key("scroll")
            .w_full()
            .grow(1.0)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .child(col().children((0..2000).map(|i| el().key(format!("row{i}")).w_full().h(20.0))));
        col().size(200.0, 100.0).child(
            col()
                .key("pane")
                .w_full()
                .grow(1.0)
                .child(el().key("header").w_full().h(20.0))
                .child(if *wrapped { viewport.scroll_bar(&k) } else { viewport }),
        )
    }
    let rects = |wrapped: bool| {
        let mut rt = rune::Runtime::new(wrapped, pane, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        ["pane", "header", "scroll", "row0"].map(|key| rect(&rt, key))
    };
    let bare = rects(false);
    assert_eq!(bare[2][3], 80.0, "the bare viewport takes what's left of the window");
    assert_eq!(rects(true), bare);
}

#[test]
fn dragging_the_thumb_and_pressing_the_track_scroll_the_viewport() {
    let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let thumb = rect(&rt, "thumb");
    assert_eq!((thumb[1], thumb[3]), (2.0, 24.0), "a quarter of the 96-point track, at its top");
    let x = thumb[0] + 3.0;
    rt.pointer_move(x, 10.0);
    rt.pointer_down(x, 10.0);
    rt.pointer_move(x, 46.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(x, 82.0);
    rt.pointer_up(x, 82.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(top(&rt), -300.0, "the thumb dragged to the track's end shows the content's");
    rt.set_system_scroller_jumps(true);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(x, 5.0);
    rt.pointer_down(x, 5.0);
    rt.pointer_up(x, 5.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(top(&rt), 0.0, "set to jump to the spot, pressing the track's top jumps there");
    assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());
}

/// A wheel over the bar belongs to the viewport the bar follows, as Chromium's scrollbar belongs
/// to its scroller: the track draws over the viewport's right edge as its sibling, so it is the
/// topmost thing a wheel there hits and the viewport is not one of its ancestors.
#[test]
fn a_wheel_over_the_bar_scrolls_the_viewport_it_follows() {
    let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    let track = rect(&rt, "scroll-bar");
    assert_eq!((track[0], track[2]), (186.0, 14.0), "the strip over the viewport's right edge: {track:?}");
    rt.wheel(track[0] + track[2] / 2.0, 50.0, -40.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(top(&rt), -40.0, "a wheel over the track scrolls the viewport it follows");

    let thumb = rect(&rt, "thumb");
    rt.wheel(thumb[0] + thumb[2] / 2.0, thumb[1] + thumb[3] / 2.0, -40.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(top(&rt), -80.0, "and over the thumb in it");

    rt.wheel(60.0, 50.0, -40.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(top(&rt), -120.0, "the content left of the bar scrolls as it did");
}

#[test]
fn a_track_press_pages_toward_itself_while_held_and_option_jumps() {
    let alt = |rt: &mut rune::Runtime<usize>, down: bool| {
        rt.physical_input(rune::PhysicalInput::Modifiers(rune::Mods { alt: down, ..Default::default() }))
    };
    let press = |jumps: bool, option: bool, hold: usize| {
        let mut rt = rune::Runtime::new(20usize, hover_view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.set_system_scroll_bars(ScrollBars::Always);
        rt.set_system_scroller_jumps(jumps);
        rt.check_live_only(false);
        frames(&mut rt, 2);
        let (runs, generation) = (HOVER_RUNS.get(), rune::commit_generation());
        alt(&mut rt, option);
        rt.pointer_move(195.0, 95.0);
        rt.pointer_down(195.0, 95.0);
        frames(&mut rt, hold);
        rt.pointer_up(195.0, 95.0);
        alt(&mut rt, false);
        frames(&mut rt, 30);
        assert_eq!((HOVER_RUNS.get(), rune::commit_generation()), (runs, generation), "paging never runs the view");
        assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());
        top(&rt)
    };
    assert_eq!(press(false, false, 1), -90.0, "a click below the thumb pages down once, 90% of the viewport");
    assert_eq!(press(false, false, 12), -90.0, "and waits before paging again");
    assert_eq!(press(false, false, 60), -300.0, "held, it pages on to the end");
    assert_eq!(press(false, true, 1), -300.0, "Option-click jumps to the spot");
    assert_eq!(press(true, false, 1), -300.0, "as a click does when set to jump");
    assert_eq!(press(true, true, 1), -90.0, "where Option-click pages");

    // Held above the thumb, paging stops once the thumb reaches the pointer.
    let mut rt = rune::Runtime::new(40usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    rt.wheel(100.0, 50.0, -10_000.0);
    frames(&mut rt, 60);
    assert_eq!(top(&rt), -700.0);
    rt.pointer_move(195.0, 40.0);
    rt.pointer_down(195.0, 40.0);
    frames(&mut rt, 120);
    let thumb = rect(&rt, "thumb");
    assert!(thumb[1] <= 40.0 && 40.0 <= thumb[1] + thumb[3], "the thumb stops under the pointer: {thumb:?}");
    rt.pointer_up(195.0, 40.0);
}

#[test]
fn scrolling_and_dragging_the_bar_run_zero_view_commits_and_chain_existing_watchers() {
    static VIEW_RUNS: AtomicUsize = AtomicUsize::new(0);
    thread_local! {
        static USER_OFFSET: Cell<f32> = const { Cell::new(-1.0) };
    }
    fn counted_view(_: &(), _: &mut rune::Cx<()>) -> El {
        VIEW_RUNS.fetch_add(1, Ordering::Relaxed);
        let k = Kit::neutral(true);
        col()
            .w(200.0)
            .h(100.0)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .on_scroll_offset(|at| USER_OFFSET.set(at.offset[1]))
            .child(col().children((0..30).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
            .scroll_bar(&k)
    }

    VIEW_RUNS.store(0, Ordering::Relaxed);
    USER_OFFSET.set(-1.0);
    let mut rt = rune::Runtime::new((), counted_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.check_live_only(false);
    rt.frame(0.0);
    assert_eq!(VIEW_RUNS.load(Ordering::Relaxed), 1);
    assert_eq!(USER_OFFSET.get(), 0.0);
    let gen0 = rune::commit_generation();

    // Wheel scroll + hold + fade-out: zero view runs, zero commits, user watcher still fires.
    for step in 0..20 {
        rt.wheel(100.0, 50.0, -10.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(USER_OFFSET.get(), ((step + 1) * 10) as f32);
    }
    // Grab and drag the visible overlay thumb, hold, release, and let it fade out.
    let thumb_r = rect(&rt, "thumb");
    let x = thumb_r[0] + 3.0;
    let y0 = thumb_r[1] + 4.0;
    rt.pointer_move(x, y0);
    rt.pointer_down(x, y0);
    for dy in [12.0, 24.0, 36.0] {
        rt.pointer_move(x, y0 + dy);
        rt.frame(1.0 / 60.0);
    }
    // Held stationary past HOLD: stays visible while held.
    for _ in 0..70 {
        rt.frame(1.0 / 60.0);
    }
    assert!(opacity(&rt, "thumb") > 0.99, "stays visible while thumb is held");
    rt.pointer_up(x, y0 + 36.0);
    // The pointer still at the edge would keep it up.
    rt.pointer_move(100.0, y0 + 36.0);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(opacity(&rt, "thumb"), 0.0, "fades out after thumb release");
    assert_eq!(VIEW_RUNS.load(Ordering::Relaxed), 1, "zero view re-runs across wheel scroll, drag and fade");
    assert_eq!(rune::commit_generation(), gen0, "zero commits across wheel scroll, drag and fade");
}

thread_local! {
    pub(super) static HOVER_RUNS: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn hover_view(rows: &usize, cx: &mut rune::Cx<usize>) -> El {
    HOVER_RUNS.set(HOVER_RUNS.get() + 1);
    view(rows, cx)
}

pub(super) fn frames<S>(rt: &mut rune::Runtime<S>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

#[test]
fn hidden_overlay_bar_passes_presses_through_to_right_edge_content() {
    fn pressable(_: &usize, _: &mut rune::Cx<usize>) -> El {
        let k = Kit::neutral(true);
        col()
            .key("list")
            .w(200.0)
            .h(100.0)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .child(
                col().children((0..10).map(|i| {
                    el().key(format!("row{i}"))
                        .w_full()
                        .h(24.0)
                        .on_press(|c: &mut usize| *c += 1)
                })),
            )
            .scroll_bar(&k)
    }
    let mut rt = rune::Runtime::new(0usize, pressable, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0);
    // Click at x = 196 (inside the 10-pt right-edge track) while the overlay bar is hidden.
    rt.pointer_move(196.0, 12.0);
    rt.pointer_down(196.0, 12.0);
    rt.pointer_up(196.0, 12.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state, 1, "hidden overlay bar lets presses reach content underneath");
}

#[test]
fn content_that_fits_shows_no_bar_and_presses_through() {
    let mut rt = rune::Runtime::new(3usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rect(&rt, "scroll-bar")[3], 0.0);
    assert_eq!(rect(&rt, "thumb")[3], 0.0);
}

#[test]
fn a_thumb_position_maps_back_to_an_offset_clamped_to_the_ends() {
    let a = at(0.0, 300.0, 100.0);
    assert_eq!(offset_at(a, 100.0, 37.5), 150.0);
    assert_eq!(offset_at(a, 100.0, -20.0), 0.0);
    assert_eq!(offset_at(a, 100.0, 500.0), 300.0);
}

#[test]
fn two_panes_with_the_same_viewport_key_have_independent_scroll_bars_and_drag_targets() {
    fn pane(key: &str, k: &Kit) -> El {
        col().key(key).size(200.0, 200.0).child(
            col()
                .key("viewport")
                .w_full()
                .h_full()
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..50).map(|i| el().key(format!("r{i}")).w_full().h(20.0))))
                .scroll_bar_styled(ScrollBarStyle::scroll_view(k)),
        )
    }
    fn two(_: &(), _: &mut rune::Cx<()>) -> El {
        let k = Kit::neutral(true);
        rune::row().size(400.0, 200.0).child(pane("a", &k)).child(pane("b", &k))
    }
    let find_in = |rt: &rune::Runtime<()>, prefix: &str, key: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key) && e.path.starts_with(prefix))
            .unwrap_or_else(|| panic!("missing {key} under {prefix}"))
    };
    let scrolled_in = |rt: &rune::Runtime<()>, prefix: &str| {
        let es = rt.elements();
        let vp = es
            .iter()
            .position(|e| e.key.as_deref() == Some("viewport") && e.path.starts_with(prefix))
            .unwrap();
        es.iter().find(|c| c.parent == Some(vp)).unwrap().scrolled[1]
    };

    let mut rt = rune::Runtime::new((), two, (400.0, 200.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    frames(&mut rt, 2);

    // Wheel over pane b: only pane b scrolls and only pane b's bar reveals.
    rt.wheel(300.0, 100.0, -100.0);
    frames(&mut rt, 20);
    assert_eq!(scrolled_in(&rt, "#0/a/"), 0.0, "wheeling b leaves a at top");
    assert!(scrolled_in(&rt, "#0/b/") > 50.0, "wheeling b scrolls b");
    assert_eq!(find_in(&rt, "#0/a/", "scroll-bar").opacity, 0.0, "wheeling b leaves a's bar hidden");
    assert!(find_in(&rt, "#0/b/", "scroll-bar").opacity > 0.9, "wheeling b reveals b's bar");

    // Hover and drag pane b's thumb: only pane b scrolls; pane a stays at 0.
    rt.pointer_move(395.0, 100.0);
    frames(&mut rt, 20);
    assert_eq!(find_in(&rt, "#0/a/", "scroll-bar").opacity, 0.0, "hovering b leaves a's bar hidden");
    let thumb_b = find_in(&rt, "#0/b/", "thumb").rect;
    let (bx, by) = (thumb_b[0] + thumb_b[2] / 2.0, thumb_b[1] + thumb_b[3] / 2.0);
    let b_before = scrolled_in(&rt, "#0/b/");
    rt.pointer_move(bx, by);
    rt.pointer_down(bx, by);
    for step in 1..=8 {
        rt.pointer_move(bx, by + 10.0 * step as f32);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(bx, by + 80.0);
    frames(&mut rt, 5);
    assert_eq!(scrolled_in(&rt, "#0/a/"), 0.0, "dragging b's thumb must not scroll a");
    assert!(
        scrolled_in(&rt, "#0/b/") > b_before + 100.0,
        "dragging b's thumb must scroll b (was {b_before}, now {})",
        scrolled_in(&rt, "#0/b/"),
    );

    // Drag pane a's thumb: only pane a scrolls; pane b stays where it was.
    let b_after = scrolled_in(&rt, "#0/b/");
    rt.pointer_move(195.0, 100.0);
    frames(&mut rt, 20);
    let thumb_a = find_in(&rt, "#0/a/", "thumb").rect;
    let (ax, ay) = (thumb_a[0] + thumb_a[2] / 2.0, thumb_a[1] + thumb_a[3] / 2.0);
    rt.pointer_move(ax, ay);
    rt.pointer_down(ax, ay);
    for step in 1..=6 {
        rt.pointer_move(ax, ay + 10.0 * step as f32);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(ax, ay + 60.0);
    frames(&mut rt, 5);
    assert!(scrolled_in(&rt, "#0/a/") > 100.0, "dragging a's thumb scrolls a");
    assert_eq!(scrolled_in(&rt, "#0/b/"), b_after, "dragging a's thumb leaves b alone");
    assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());
}

#[test]
fn two_panes_with_the_same_key_and_scroll_bar_hover_reveal_only_their_own_bar() {
    fn pane(key: &str, k: &Kit) -> El {
        col()
            .key(key)
            .size(200.0, 200.0)
            .child(
                col()
                    .key("scroll")
                    .w_full()
                    .h_full()
                    .scroll(rune::ScrollSpec { stick_bottom: false })
                    .child(col().children((0..40).map(|i| el().key(format!("r{i}")).w_full().h(20.0))))
                    .scroll_bar_styled(ScrollBarStyle::scroll_view(k).hover_target()),
            )
            .scroll_bar_hover("scroll")
    }
    fn two(_: &(), _: &mut rune::Cx<()>) -> El {
        let k = Kit::neutral(true);
        rune::row().size(400.0, 200.0).child(pane("a", &k)).child(pane("b", &k))
    }
    let bar_opacity = |rt: &rune::Runtime<()>, prefix: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("scroll-bar") && e.path.starts_with(prefix))
            .unwrap()
            .opacity
    };

    let mut rt = rune::Runtime::new((), two, (400.0, 200.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    frames(&mut rt, 2);
    assert_eq!((bar_opacity(&rt, "#0/a/"), bar_opacity(&rt, "#0/b/")), (0.0, 0.0));

    rt.pointer_move(300.0, 100.0);
    frames(&mut rt, 30);
    assert_eq!(bar_opacity(&rt, "#0/a/"), 0.0, "hovering b must not reveal a's bar");
    assert!(bar_opacity(&rt, "#0/b/") > 0.99, "hovering b reveals b's bar");

    rt.pointer_move(100.0, 100.0);
    frames(&mut rt, 30);
    assert!(bar_opacity(&rt, "#0/a/") > 0.99, "hovering a reveals a's bar");
    assert_eq!(bar_opacity(&rt, "#0/b/"), 0.0, "leaving b hides b's bar");
}

#[test]
fn escaped_viewport_keys_and_moved_memo_scroll_bars_target_their_own_viewport() {
    fn scrolled_in<S>(rt: &rune::Runtime<S>, prefix: &str, key: &str) -> f32 {
        let es = rt.elements();
        let vp = es
            .iter()
            .position(|e| e.key.as_deref() == Some(key) && e.path.starts_with(prefix))
            .unwrap();
        es.iter().find(|c| c.parent == Some(vp)).unwrap().scrolled[1]
    }
    fn thumb_in<S>(rt: &rune::Runtime<S>, prefix: &str) -> [f32; 4] {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("thumb") && e.path.starts_with(prefix))
            .unwrap()
            .rect
    }

    // 1. Two panes with an escaped key ("a/b", #1531) and track paging on the second pane.
    fn escaped(_: &(), _: &mut rune::Cx<()>) -> El {
        let k = Kit::neutral(true);
        let pane = |name: &str| {
            col().key(name).size(200.0, 100.0).child(
                col()
                    .key("a/b")
                    .w_full()
                    .h_full()
                    .scroll(rune::ScrollSpec { stick_bottom: false })
                    .child(col().children((0..20).map(|i| el().key(format!("r{i}")).w_full().h(20.0))))
                    .scroll_bar(&k),
            )
        };
        rune::row().size(400.0, 100.0).child(pane("first")).child(pane("second"))
    }
    let mut rt = rune::Runtime::new((), escaped, (400.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    // Click below the thumb on the second pane's track to page it down.
    let t2 = thumb_in(&rt, "#0/second/");
    let px = t2[0] + t2[2] / 2.0;
    rt.pointer_move(px, 90.0);
    rt.pointer_down(px, 90.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(px, 90.0);
    frames(&mut rt, 2);
    assert_eq!(scrolled_in(&rt, "#0/first/", "a/b"), 0.0, "paging second pane must not scroll first");
    assert_eq!(scrolled_in(&rt, "#0/second/", "a/b"), 90.0, "paging second pane scrolls second by one page");

    // 2. A memoized scroll bar moved to a new parent path rebinds to the new path rather than
    //    keeping stale handlers from its previous parent.
    fn moved_memo(second: &bool, _: &mut rune::Cx<bool>) -> El {
        let memo_bar = rune::memo("shared-list", 1u8, || {
            let k = Kit::neutral(true);
            col()
                .key("vp")
                .size(200.0, 100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..20).map(|i| el().key(format!("r{i}")).w_full().h(20.0))))
                .scroll_bar(&k)
        });
        let parent = if *second { "p2" } else { "p1" };
        col().size(200.0, 100.0).child(col().key(parent).size(200.0, 100.0).child(memo_bar))
    }
    let mut rt = rune::Runtime::new(false, moved_memo, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    rt.update(|s, _| *s = true);
    frames(&mut rt, 2);
    let tm = thumb_in(&rt, "#0/p2/");
    let (mx, my) = (tm[0] + tm[2] / 2.0, tm[1] + tm[3] / 2.0);
    rt.pointer_move(mx, my);
    rt.pointer_down(mx, my);
    rt.pointer_move(mx, my + 40.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(mx, my + 40.0);
    frames(&mut rt, 2);
    assert!(scrolled_in(&rt, "#0/p2/", "vp") > 100.0, "dragging moved memo bar scrolls its new path");
}

#[test]
fn track_inset_adjusts_thumb_travel_endpoints_without_changing_thumb_width() {
    fn inset_view(inset: &f32, _: &mut rune::Cx<f32>) -> El {
        let k = Kit::neutral(true);
        let style = ScrollBarStyle::from_kit(&k).always_visible().track_inset(*inset);
        col().size(200.0, 100.0).child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..20).map(|i| el().key(format!("r{i}")).w_full().h(20.0))))
                .scroll_bar_styled(style),
        )
    }

    // Normal inset: track starts at 10 pt, ends at 90 pt, thumb width unchanged.
    let mut rt = rune::Runtime::new(10.0f32, inset_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut rt, 2);
    assert_eq!(rect(&rt, "scroll-bar"), [186.0, 10.0, TRACK, 80.0]);
    let t0 = rect(&rt, "thumb");
    assert_eq!((t0[1], t0[2], t0[3]), (10.0, THUMB, MIN_THUMB), "top at inset, width unchanged");
    rt.wheel(100.0, 50.0, -10_000.0);
    frames(&mut rt, 20);
    let t_end = rect(&rt, "thumb");
    assert_eq!(t_end[1] + t_end[3], 90.0, "bottom stops at viewport - inset");

    // Edge: zero and negative clamp to 0 (flush to both ends).
    for inset in [0.0f32, -8.0] {
        let mut rt = rune::Runtime::new(inset, inset_view, (200.0, 100.0), rune::Env::new(true, 1.0));
        frames(&mut rt, 2);
        assert_eq!(rect(&rt, "scroll-bar"), [186.0, 0.0, TRACK, 100.0]);
        assert_eq!(rect(&rt, "thumb")[1], 0.0);
    }

    // Edge: huge inset leaves less than min_thumb, hiding the bar cleanly.
    let mut rt = rune::Runtime::new(60.0f32, inset_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut rt, 2);
    assert_eq!(rect(&rt, "scroll-bar")[3], 0.0);
    assert_eq!(rect(&rt, "thumb")[3], 0.0);

    // Preserves webkit_parts strip width and thumb border while overriding only track inset.
    let parts = ScrollBarStyle::native(Color::WHITE, Color::WHITE)
        .webkit_parts(10.0, 2.0, 4.0)
        .track_inset(8.0);
    assert_eq!(parts.strip(), 10.0);
    assert_eq!((parts.width, parts.hover_width, parts.track.border, parts.track.inset), (6.0, 6.0, 2.0, 8.0));
}

#[test]
fn always_visible_track_inset_and_track_fill_work_in_rtl_and_horizontal() {
    const TROUGH: Color = Color { r: 0.2, g: 0.2, b: 0.2, a: 1.0 };

    // Vertical RTL: bar sits on the left edge with track inset and trough fill.
    fn rtl_view(_: &(), _: &mut rune::Cx<()>) -> El {
        let style = ScrollBarStyle::native(Color::WHITE, Color::WHITE)
            .inside()
            .always_visible()
            .track_inset(10.0)
            .track(TROUGH);
        col().size(200.0, 100.0).dir(rune::Direction::Rtl).child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..20).map(|i| el().key(format!("r{i}")).w_full().h(20.0))))
                .scroll_bar_styled(style),
        )
    }
    let mut rt = rune::Runtime::new((), rtl_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut rt, 2);
    let bar = info(&rt, "scroll-bar");
    assert_eq!(bar.rect, [0.0, 10.0, 8.0, 80.0], "RTL vertical track sits on left edge, inset 10 pt");
    assert_eq!(bar.opacity, 1.0);
    assert_eq!(bar.fill, TROUGH);

    // Horizontal: bar sits along the bottom edge, inset horizontally at both ends.
    fn horiz_view(_: &(), _: &mut rune::Cx<()>) -> El {
        let style = ScrollBarStyle::native(Color::WHITE, Color::WHITE)
            .inside()
            .horizontal()
            .always_visible()
            .track_inset(12.0)
            .track(TROUGH);
        col().size(200.0, 100.0).child(
            rune::row()
                .key("hscroll")
                .w_full()
                .h(100.0)
                .scroll_x()
                .child(el().key("wide").w(600.0).h(100.0))
                .scroll_bar_styled(style),
        )
    }
    let mut rt = rune::Runtime::new((), horiz_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut rt, 2);
    let bar = info(&rt, "scroll-bar");
    assert_eq!(bar.rect, [12.0, 92.0, 176.0, 8.0], "horizontal track inset 12 pt from both ends");
    assert_eq!(bar.opacity, 1.0);
    assert_eq!(bar.fill, TROUGH);
}

#[test]
fn scroll_bar_drag_and_track_page_report_direct_while_tx_scroll_reports_programmatic() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let seen: Rc<RefCell<Vec<ScrollPos>>> = Rc::new(RefCell::new(Vec::new()));
    let view = |seen: &Rc<RefCell<Vec<ScrollPos>>>, _: &mut rune::Cx<Rc<RefCell<Vec<ScrollPos>>>>| {
        let k = Kit::neutral(true);
        let style = ScrollBarStyle::from_kit(&k).always_visible();
        let log = seen.clone();
        col().size(200.0, 100.0).child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .on_scroll_offset(move |pos: ScrollPos| log.borrow_mut().push(pos))
                .child(col().children((0..40).map(|i| el().key(format!("r{i}")).w_full().h(20.0))))
                .scroll_bar_styled(style),
        )
    };

    let mut rt = rune::Runtime::new(seen.clone(), view, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut rt, 2);
    seen.borrow_mut().clear();

    // Drag the thumb down: must report ScrollSource::Direct.
    let thumb = rect(&rt, "thumb");
    let x = thumb[0] + thumb[2] * 0.5;
    let y0 = thumb[1] + thumb[3] * 0.5;
    rt.pointer_down(x, y0);
    rt.pointer_move(x, y0 + 30.0);
    frames(&mut rt, 2);
    rt.pointer_up(x, y0 + 30.0);
    frames(&mut rt, 2);
    assert!(
        seen.borrow().iter().any(|p| p.offset[1] > 1.0 && p.source == rune::ScrollSource::Direct),
        "thumb drag should report Direct: {:?}",
        seen.borrow()
    );
    seen.borrow_mut().clear();

    // Click the track below the thumb to page: must report ScrollSource::Direct.
    let bar = rect(&rt, "scroll-bar");
    let bx = bar[0] + bar[2] * 0.5;
    let by = bar[1] + bar[3] - 4.0;
    rt.pointer_down(bx, by);
    frames(&mut rt, 2);
    rt.pointer_up(bx, by);
    frames(&mut rt, 2);
    assert!(
        seen.borrow().iter().any(|p| p.source == rune::ScrollSource::Direct),
        "track page click should report Direct: {:?}",
        seen.borrow()
    );
    seen.borrow_mut().clear();

    // Programmatic instant jump on the same viewport: must report ScrollSource::Programmatic.
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 0.0));
    frames(&mut rt, 2);
    assert!(
        seen.borrow().iter().any(|p| p.offset[1] == 0.0 && p.source == rune::ScrollSource::Programmatic),
        "Tx::scroll_to_instant should report Programmatic: {:?}",
        seen.borrow()
    );
}
