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
