use std::sync::atomic::{AtomicUsize, Ordering};

use rune::ScrollBars;

use super::*;

fn at(offset: f32, max: f32, shown: f32) -> ScrollPos {
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

fn view(rows: &usize, _: &mut rune::Cx<usize>) -> El {
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

fn info<S>(rt: &rune::Runtime<S>, key: &str) -> rune::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

fn rect<S>(rt: &rune::Runtime<S>, key: &str) -> [f32; 4] {
    info(rt, key).rect
}

fn opacity<S>(rt: &rune::Runtime<S>, key: &str) -> f32 {
    info(rt, key).opacity
}

fn top<S>(rt: &rune::Runtime<S>) -> f32 {
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
fn every_scroll_bar_setting_and_automatic_device_style_behave_as_specified() {
    // 1. Always: visible immediately when overflowing, stays visible at rest.
    let mut always = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    always.set_system_scroll_bars(ScrollBars::Always);
    always.frame(0.0);
    assert_eq!(opacity(&always, "thumb"), 1.0);
    for _ in 0..120 {
        always.frame(1.0 / 60.0);
    }
    assert_eq!(opacity(&always, "thumb"), 1.0);

    // 2. WhenScrolling (also the headless default): hidden at rest, overlays on scroll, fades out after rest.
    let mut overlay = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    assert_eq!(overlay.system_scroll_bars(), ScrollBars::WhenScrolling);
    overlay.frame(0.0);
    assert_eq!(opacity(&overlay, "thumb"), 0.0, "hidden before scrolling");
    overlay.wheel(100.0, 50.0, -60.0);
    for _ in 0..20 {
        overlay.frame(1.0 / 60.0);
    }
    assert!(opacity(&overlay, "thumb") > 0.95, "visible while scrolling and holding");
    for _ in 0..90 {
        overlay.frame(1.0 / 60.0);
    }
    assert_eq!(opacity(&overlay, "thumb"), 0.0, "faded out after resting");

    // 3. Automatic with a mouse (`scroller_overlay = false`): shown whenever overflowing.
    let mut auto_mouse = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    auto_mouse.set_system_scroll_bars(ScrollBars::Automatic);
    auto_mouse.set_system_scroller_overlay(false);
    auto_mouse.frame(0.0);
    assert_eq!(opacity(&auto_mouse, "thumb"), 1.0);

    // 4. Automatic with a trackpad (`scroller_overlay = true`): overlay when scrolling.
    let mut auto_pad = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    auto_pad.set_system_scroll_bars(ScrollBars::Automatic);
    auto_pad.set_system_scroller_overlay(true);
    auto_pad.frame(0.0);
    assert_eq!(opacity(&auto_pad, "thumb"), 0.0);
    auto_pad.wheel(100.0, 50.0, -40.0);
    for _ in 0..20 {
        auto_pad.frame(1.0 / 60.0);
    }
    assert!(opacity(&auto_pad, "thumb") > 0.95);
    for _ in 0..90 {
        auto_pad.frame(1.0 / 60.0);
    }
    assert_eq!(opacity(&auto_pad, "thumb"), 0.0);
}

#[test]
fn live_preference_and_pointing_device_changes_update_mounted_scroll_bars() {
    fn memo_view(rows: &usize, _: &mut rune::Cx<usize>) -> El {
        rune::memo("pane", *rows, || {
            let k = Kit::neutral(true);
            col().size(200.0, 100.0).child(k.scroll_bar(
                col()
                    .key("memo-scroll")
                    .w(200.0)
                    .h(100.0)
                    .scroll(rune::ScrollSpec { stick_bottom: false })
                    .child(col().children((0..*rows).map(|i| el().key(format!("row{i}")).w_full().h(20.0)))),
            ))
        })
    }
    let mut rt = rune::Runtime::new(20usize, memo_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(opacity(&rt, "thumb"), 0.0, "starts WhenScrolling");

    // Switch live to Always: memo invalidates and thumb fades in to 1.0.
    rt.set_system_scroll_bars(ScrollBars::Always);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(opacity(&rt, "thumb"), 1.0);

    // Switch live to Automatic with a trackpad: fades out to 0.0.
    rt.set_system_scroll_bars(ScrollBars::Automatic);
    rt.set_system_scroller_overlay(true);
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(opacity(&rt, "thumb"), 0.0);

    // Plugging in a mouse under Automatic (`scroller_overlay = false`): fades in to 1.0.
    rt.set_system_scroller_overlay(false);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(opacity(&rt, "thumb"), 1.0);
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
    static HOVER_RUNS: Cell<usize> = const { Cell::new(0) };
}

fn hover_view(rows: &usize, cx: &mut rune::Cx<usize>) -> El {
    HOVER_RUNS.set(HOVER_RUNS.get() + 1);
    view(rows, cx)
}

fn frames<S>(rt: &mut rune::Runtime<S>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

#[test]
fn the_thumb_widens_under_the_pointer_and_while_held_without_a_view_run() {
    let mut rt = rune::Runtime::new(20usize, hover_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.check_live_only(false);
    frames(&mut rt, 2);
    let (runs, generation) = (HOVER_RUNS.get(), rune::commit_generation());
    let rest = rect(&rt, "thumb");
    assert_eq!((rest[0], rest[2]), (192.0, THUMB), "a thin thumb 2 points in from the edge");

    rt.pointer_move(195.0, 10.0);
    frames(&mut rt, 30);
    let wide = rect(&rt, "thumb");
    assert_eq!((wide[0], wide[2]), (188.0, THUMB_HOVER), "wider over the bar, still 2 points in");
    assert_eq!((wide[1], wide[3]), (rest[1], rest[3]));
    rt.pointer_move(170.0, 10.0);
    frames(&mut rt, 30);
    assert_eq!(rect(&rt, "thumb")[2], THUMB, "thin again once the pointer leaves the bar");

    rt.pointer_move(195.0, 10.0);
    rt.pointer_down(195.0, 10.0);
    rt.pointer_move(100.0, 30.0);
    frames(&mut rt, 30);
    assert_eq!(rect(&rt, "thumb")[2], THUMB_HOVER, "a held thumb stays wide off the bar");
    rt.pointer_up(100.0, 30.0);
    frames(&mut rt, 30);
    assert_eq!(rect(&rt, "thumb")[2], THUMB, "and narrows on release");

    rt.pointer_move(195.0, 10.0);
    frames(&mut rt, 30);
    rt.pointer_leave();
    frames(&mut rt, 30);
    assert_eq!(rect(&rt, "thumb")[2], THUMB, "the pointer leaving the window leaves the bar");
    assert_eq!(HOVER_RUNS.get(), runs, "hovering never runs the view");
    assert_eq!(rune::commit_generation(), generation, "or commits");
}

#[test]
fn a_styled_thumb_takes_its_colours_and_widths_at_rest_and_under_the_pointer() {
    thread_local! {
        static STYLED_RUNS: Cell<usize> = const { Cell::new(0) };
    }
    const RED: rune::Color = rune::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    const BLUE: rune::Color = rune::Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
    fn styled(_: &(), _: &mut rune::Cx<()>) -> El {
        STYLED_RUNS.set(STYLED_RUNS.get() + 1);
        let style = ScrollBarStyle::neutral(true).thumb(RED).thumb_hover(BLUE).width(4.0).hover_width(12.0);
        col().size(200.0, 100.0).child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..20).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                .scroll_bar_styled(style),
        )
    }
    let mut rt = rune::Runtime::new((), styled, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.check_live_only(false);
    frames(&mut rt, 2);
    let runs = STYLED_RUNS.get();
    let rest = info(&rt, "thumb");
    assert_eq!((rest.rect[0], rest.rect[2], rest.fill), (194.0, 4.0, RED), "thin and red at rest, 2 points in");
    assert_eq!(rect(&rt, "scroll-bar")[2], 16.0, "the press strip widens to hold the widest thumb");

    rt.pointer_move(185.0, 10.0);
    frames(&mut rt, 30);
    let hovered = info(&rt, "thumb");
    assert_eq!((hovered.rect[0], hovered.rect[2], hovered.fill), (186.0, 12.0, BLUE), "wide and blue over the bar");
    rt.pointer_move(150.0, 10.0);
    frames(&mut rt, 30);
    assert_eq!(info(&rt, "thumb").fill, RED, "red again off it");
    assert_eq!(STYLED_RUNS.get(), runs, "hovering never runs the view");
}

#[test]
fn the_pointer_at_the_right_edge_brings_up_an_overlay_bar() {
    let mut rt = rune::Runtime::new(20usize, hover_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.check_live_only(false);
    frames(&mut rt, 2);
    let (runs, generation) = (HOVER_RUNS.get(), rune::commit_generation());
    assert_eq!(opacity(&rt, "thumb"), 0.0);
    rt.pointer_move(150.0, 50.0);
    frames(&mut rt, 20);
    assert_eq!(opacity(&rt, "thumb"), 0.0, "the middle of the viewport doesn't bring it up");

    rt.pointer_move(184.0, 50.0);
    frames(&mut rt, 20);
    assert!(opacity(&rt, "thumb") > 0.95, "the pointer near the edge does");
    assert_eq!(rect(&rt, "thumb")[2], THUMB, "near the bar, not over it");
    rt.wheel(100.0, 50.0, -20.0);
    frames(&mut rt, 150);
    assert!(opacity(&rt, "thumb") > 0.99, "it stays up while the pointer waits there, through a scroll");

    rt.pointer_move(150.0, 50.0);
    frames(&mut rt, 20);
    assert!(opacity(&rt, "thumb") > 0.99, "and holds a moment after it leaves");
    frames(&mut rt, 120);
    assert_eq!(opacity(&rt, "thumb"), 0.0, "then fades out");

    rt.pointer_move(196.0, 50.0);
    frames(&mut rt, 20);
    rt.pointer_leave();
    frames(&mut rt, 150);
    assert_eq!(opacity(&rt, "thumb"), 0.0, "leaving the window lets it go");
    assert_eq!(HOVER_RUNS.get(), runs, "bringing it up never runs the view");
    assert_eq!(rune::commit_generation(), generation, "or commits");
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
fn a_still_view_stops_asking_for_frames() {
    let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt.wheel(100.0, 50.0, -40.0);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    rt.invalidate();
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let wants: Vec<_> = (0..5).map(|_| (rt.frame(1.0 / 60.0), rt.live_pending())).collect();
    assert_eq!(wants, vec![(false, false); 5]);
}

#[test]
fn new_content_moves_the_thumb_at_once() {
    let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt.update(|rows, tx| {
        *rows = 40;
        tx.instant();
    });
    rt.frame(1.0 / 120.0);
    let moving: Vec<_> = rt.elements().into_iter().filter(|e| e.moving).map(|e| e.path).collect();
    assert!(moving.is_empty(), "{moving:?}");
    assert_eq!(rect(&rt, "thumb")[3], 24.0f32.max(96.0 * 100.0 / 800.0));
}

#[test]
fn the_thumb_glides_to_a_new_length_without_a_view_run() {
    let mut rt = rune::Runtime::new(10usize, hover_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.check_live_only(false);
    frames(&mut rt, 30);
    assert_eq!(rect(&rt, "thumb")[3], 48.0, "half the content shown, half the track");
    rt.update(|rows, _| *rows = 20);
    frames(&mut rt, 3);
    let runs = HOVER_RUNS.get();
    let (drawn, slot) = (rect(&rt, "thumb"), rect(&rt, "thumb-slot"));
    assert!(24.0 < drawn[3] && drawn[3] < 48.0, "on its way: {drawn:?}");
    assert_eq!(drawn[3], slot[3], "the thumb fills its slot as both glide");
    assert!(info(&rt, "thumb").moving);
    frames(&mut rt, 60);
    assert_eq!(rect(&rt, "thumb")[3], 24.0, "then rests at its new length");
    assert_eq!(HOVER_RUNS.get(), runs, "the glide never runs the view");

    rt.update(|rows, _| *rows = 10);
    frames(&mut rt, 3);
    rt.wheel(100.0, 50.0, -40.0);
    rt.frame(1.0 / 60.0);
    let drawn = rect(&rt, "thumb");
    assert!((drawn[1] - thumb_at(&rt)).abs() < 0.01 && drawn[3] == 48.0, "a hand scroll lands the glide at once: {drawn:?}");
    frames(&mut rt, 60);
    rt.wheel(100.0, 50.0, -10.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
        let (drawn, shown) = (rect(&rt, "thumb")[1], thumb_at(&rt));
        assert!((drawn - shown).abs() < 0.01, "with no glide running the thumb keeps up with the scroll: {drawn} vs {shown}");
    }
}

#[test]
fn a_hidden_overlay_thumb_lands_its_new_length_at_once() {
    let mut rt = rune::Runtime::new(10usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    frames(&mut rt, 30);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0, "an overlay bar at rest is hidden");
    rt.update(|rows, _| *rows = 20);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rect(&rt, "thumb")[3], 24.0, "no glide runs while nobody can see it");
    assert!(!info(&rt, "thumb").moving && !info(&rt, "thumb-slot").moving);
}

fn thumb_at<S>(rt: &rune::Runtime<S>) -> f32 {
    2.0 + thumb(at(-top(rt), 100.0, 100.0), 96.0).0
}

#[test]
fn a_new_bar_lands_at_once() {
    fn shown(rows: &usize, cx: &mut rune::Cx<usize>) -> El {
        if *rows == 0 { col().size(200.0, 100.0) } else { view(rows, cx) }
    }
    let mut rt = rune::Runtime::new(0usize, shown, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt.update(|rows, tx| {
        *rows = 40;
        tx.instant();
    });
    rt.frame(1.0 / 120.0);
    let moving: Vec<_> = rt.elements().into_iter().filter(|e| e.moving).map(|e| e.path).collect();
    assert!(moving.is_empty(), "{moving:?}");
    assert_eq!(rect(&rt, "thumb")[3], 24.0);
}

#[test]
fn following_the_end_and_gliding_to_it_dont_show_an_overlay_bar() {
    fn feed(rows: &usize, _: &mut rune::Cx<usize>) -> El {
        let k = Kit::neutral(true);
        col().size(200.0, 100.0).child(
            col()
                .key("feed")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: true })
                .child(col().children((0..*rows).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                .scroll_bar(&k),
        )
    }
    let mut rt = rune::Runtime::new(20usize, feed, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    for _ in 0..30 {
        rt.update(|rows, _| *rows += 1);
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
            assert_eq!(opacity(&rt, "thumb"), 0.0, "following the end as rows arrive isn't a scroll");
        }
    }
    rt.wheel(100.0, 50.0, 400.0);
    for _ in 0..150 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(opacity(&rt, "thumb"), 0.0, "rested after the wheel");
    rt.update(|_, tx| tx.scroll_to_bottom("feed"));
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
        assert_eq!(opacity(&rt, "thumb"), 0.0, "a glide to the end isn't a hand scroll");
    }
    rt.update(|_, tx| tx.scroll_page("feed", -1));
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(opacity(&rt, "thumb") > 0.95, "paging is");
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
fn scroll_bar_motion_gate() {
    use rune::motion_gate::{self, Run};

    motion_gate::gate_rows(vec![
        (
            "scroll-bar/when-scrolling-pulse-and-preference-switch".into(),
            Box::new(|| {
                let mut r = Run::new(rune::Runtime::new(24usize, view, (200.0, 100.0), rune::Env::new(true, 1.0)), 120.0);
                r.idle(0.3);
                for _ in 0..12 {
                    r.act("wheel", |rt| rt.wheel(100.0, 50.0, -12.0));
                    r.step();
                }
                r.play(1.6);
                r.act("always", |rt| rt.set_system_scroll_bars(ScrollBars::Always));
                r.play(0.6);
                r.act("when-scrolling", |rt| rt.set_system_scroll_bars(ScrollBars::WhenScrolling));
                r.play(1.2);
                r.rec
            }),
        ),
        (
            "scroll-bar/hover-widen-and-edge-reveal".into(),
            Box::new(|| {
                let mut r = Run::new(rune::Runtime::new(24usize, view, (200.0, 100.0), rune::Env::new(true, 1.0)), 120.0);
                r.idle(0.3);
                r.act("near-edge", |rt| rt.pointer_move(184.0, 50.0));
                r.play(0.5);
                r.act("over-bar", |rt| rt.pointer_move(195.0, 10.0));
                r.play(0.4);
                r.act("off-bar", |rt| rt.pointer_move(170.0, 10.0));
                r.play(0.4);
                r.act("away", |rt| rt.pointer_move(100.0, 50.0));
                r.play(1.6);
                r.act("always", |rt| rt.set_system_scroll_bars(ScrollBars::Always));
                r.play(0.4);
                r.act("over-always", |rt| rt.pointer_move(195.0, 10.0));
                r.play(0.4);
                r.act("away-always", |rt| rt.pointer_move(100.0, 50.0));
                r.play(0.4);
                r.rec
            }),
        ),
        (
            "scroll-bar/length-glide".into(),
            Box::new(|| {
                let mut rt = rune::Runtime::new(10usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
                rt.set_system_scroll_bars(ScrollBars::Always);
                let mut r = Run::new(rt, 120.0);
                r.idle(0.3);
                r.act("grow", |rt| rt.update(|rows, _| *rows = 20));
                r.play(0.6);
                r.act("shrink", |rt| rt.update(|rows, _| *rows = 12));
                r.play(0.6);
                r.rec
            }),
        ),
    ], &[]);
}

thread_local! {
    static PANE_RUNS: Cell<usize> = const { Cell::new(0) };
}

/// A 200-point pane whose viewport takes what a composer below it leaves.
fn composer_pane(composer: &f32, _: &mut rune::Cx<f32>) -> El {
    PANE_RUNS.set(PANE_RUNS.get() + 1);
    let k = Kit::neutral(true);
    col()
        .size(200.0, 200.0)
        .child(
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..40).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                .scroll_bar(&k),
        )
        .child(el().key("composer").w_full().h(*composer))
}

#[test]
fn the_bar_follows_the_viewport_while_its_height_animates() {
    let mut rt = rune::Runtime::new(40.0f32, composer_pane, (200.0, 200.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    frames(&mut rt, 30);
    rt.wheel(100.0, 50.0, -10_000.0);
    frames(&mut rt, 60);
    rt.update(|h, _| *h = 95.0);
    let (mut runs, mut between) = (0, 0);
    for i in 0..60 {
        rt.frame(1.0 / 60.0);
        if i == 0 {
            runs = PANE_RUNS.get();
        }
        let (vp, track, thumb) = (rect(&rt, "scroll"), rect(&rt, "scroll-bar"), rect(&rt, "thumb"));
        let bottom = vp[1] + vp[3];
        assert!((track[1] + track[3] - (bottom - INSET)).abs() < 0.05, "the track ends {INSET} above the viewport's drawn bottom: {track:?} in {vp:?}");
        assert!(thumb[1] + thumb[3] <= bottom - INSET + 0.05, "the thumb stays on it: {thumb:?} in {vp:?}");
        between += usize::from(105.0 < vp[3] && vp[3] < 160.0);
    }
    assert!(between > 5, "the viewport's height animated over several frames ({between})");
    frames(&mut rt, 120);
    assert_eq!(rect(&rt, "scroll-bar")[3], 105.0 - 2.0 * INSET);
    assert_eq!(PANE_RUNS.get(), runs, "following it never runs the view");
}
