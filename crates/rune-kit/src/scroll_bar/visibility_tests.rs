use rune::ScrollBars;

use super::*;
use super::tests::{HOVER_RUNS, frames, hover_view, info, opacity, rect, view};

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
fn a_held_thumb_whose_release_is_lost_narrows_once_the_pointer_moves_on() {
    fn shown(s: &(usize, bool), _: &mut rune::Cx<(usize, bool)>) -> El {
        let k = Kit::neutral(true);
        let area = col()
            .key("scroll")
            .w_full()
            .h(100.0)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .child(col().children((0..s.0).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
            .scroll_bar(&k);
        col().size(200.0, 100.0).children(s.1.then_some(area))
    }
    let mut rt = rune::Runtime::new((20usize, true), shown, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    rt.pointer_move(195.0, 10.0);
    rt.pointer_down(195.0, 10.0);
    rt.pointer_move(100.0, 30.0);
    frames(&mut rt, 30);
    assert_eq!(rect(&rt, "thumb")[2], THUMB_HOVER);
    // The bar goes mid-drag, so its release never reaches it, and comes back.
    rt.update(|s, _| s.1 = false);
    frames(&mut rt, 2);
    rt.pointer_up(100.0, 30.0);
    rt.update(|s, _| s.1 = true);
    frames(&mut rt, 30);
    rt.pointer_move(110.0, 40.0);
    rt.pointer_move(120.0, 50.0);
    frames(&mut rt, 30);
    assert_eq!(rect(&rt, "thumb")[2], THUMB, "the press ended once the pointer moved where the drag didn't hear it");
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
fn a_pulse_is_due_at_its_hold_and_fade_summed_before_now() {
    use super::visibility::{BarState, Effect, Event, Follow, Glide, Opacity, Phase, step};
    let hold = std::time::Duration::from_millis(750);
    let follow = |state: &mut BarState, now: f64, scrolled: bool, instant: bool, extent_changed: bool| {
        step(
            state,
            Event::Follow(Follow {
                shows: true,
                overlay: true,
                scrolled,
                instant,
                resized: false,
                lengthened: false,
                extent_changed,
                pressed: false,
                revealed: false,
                now,
                hold,
            }),
        )
    };
    // A wheel scroll at 0.3 s starts the pulse: its hold and fade are summed first, so the
    // deadline carries that sum's rounding rather than splitting it across `now`.
    let mut state = BarState::Overlay { phase: Phase::Hidden, glide: Glide::Idle };
    follow(&mut state, 0.3, true, false, false);
    assert_eq!(state.pulse_until(), 1.4000000000000001);
    // An instant content-size change at exactly 1.4 s lands inside it and renews it.
    let effects = follow(&mut state, 1.4, false, true, true);
    assert!(
        effects.into_iter().any(|e| matches!(e, Effect::Opacity(Opacity::Pulse { .. }))),
        "the pulse renewed"
    );
    // Letting go after a hold at 0.3 s gives the same deadline.
    let mut held = BarState::Overlay { phase: Phase::Held, glide: Glide::Idle };
    step(&mut held, Event::LetGo { pressed: false, revealed: false, area_reveals: false, now: 0.3, hold });
    assert_eq!(held.pulse_until(), 1.4000000000000001);
}

#[test]
fn area_reveals_post_scroll_hold_survives_idle_clock_gap() {
    fn styled_view(scale: &f32, cx: &mut rune::Cx<f32>) -> El {
        cx.time_scale(*scale);
        let k = Kit::neutral(true);
        col().size(200.0, 100.0).child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..20).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                .scroll_bar_styled(ScrollBarStyle::scroll_view(&k)),
        )
    }
    let mut rt = rune::Runtime::new(0.0f32, styled_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    rt.frame(0.0);
    // Advance unscaled clock 5 s ahead of scene animation time `t` (as after idle wake on a real window).
    rt.frame(5.0);
    rt.update(|s, _| *s = 1.0);
    frames(&mut rt, 2);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0);

    // 1. Hover area, wheel-scroll, leave 17 ms later: keeps remaining ~800 ms post-scroll hold.
    rt.pointer_move(100.0, 50.0);
    frames(&mut rt, 20);
    assert!(opacity(&rt, "scroll-bar") > 0.99);
    rt.wheel(100.0, 50.0, -40.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_leave();
    frames(&mut rt, 12); // ~200 ms after leave
    assert!(opacity(&rt, "scroll-bar") > 0.99, "area_reveals keeps post-scroll hold after 5 s clock/t skew: {}", opacity(&rt, "scroll-bar"));
    frames(&mut rt, 90); // past 800 ms hold + fade
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0, "then fades out cleanly");

    // 2. Pure hover-leave (no scroll): hides immediately without a false 800 ms hold.
    rt.pointer_move(100.0, 50.0);
    frames(&mut rt, 20);
    assert!(opacity(&rt, "scroll-bar") > 0.99);
    rt.pointer_leave();
    frames(&mut rt, 30); // 500 ms: past 200 ms leave spring, well under 800 ms hold
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0, "pure hover-leave hides without post-scroll hold");

    // 3. Scroll then stay hovered > 800 ms, then leave: expired hold hides immediately.
    rt.pointer_move(100.0, 50.0);
    frames(&mut rt, 20);
    rt.wheel(100.0, 50.0, -40.0);
    frames(&mut rt, 60); // 1.0 s hovered (past 800 ms hold)
    assert!(opacity(&rt, "scroll-bar") > 0.99);
    rt.pointer_leave();
    frames(&mut rt, 30);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0, "expired post-scroll hold does not re-hold on leave");
}

#[test]
fn scroll_only_thumb_hover_holds_after_idle_clock_gap() {
    fn menu_view(scale: &f32, cx: &mut rune::Cx<f32>) -> El {
        cx.time_scale(*scale);
        let k = Kit::neutral(true);
        col().size(200.0, 100.0).child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..20).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                .scroll_bar_styled(ScrollBarStyle::menu_scroll(k.c.border_muted, k.c.border_strong)),
        )
    }
    let mut rt = rune::Runtime::new(0.0f32, menu_view, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    rt.frame(0.0);
    // Advance unscaled clock 5 s ahead of scene animation time `t`.
    rt.frame(5.0);
    rt.update(|s, _| *s = 1.0);
    frames(&mut rt, 2);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0);

    // Wheel-scroll while pointer is away, then move onto the thumb 50 ms later.
    rt.wheel(100.0, 50.0, -40.0);
    frames(&mut rt, 3);
    let tr = rect(&rt, "thumb");
    rt.pointer_move(tr[0] + tr[2] / 2.0, tr[1] + tr[3] / 2.0);
    // Stay on the thumb for 1.5 s (past the 1.15 s pulse deadline): must stay held visible.
    frames(&mut rt, 90);
    assert!(opacity(&rt, "scroll-bar") > 0.99, "scroll_only thumb hover stays visible after 5 s clock/t skew: {}", opacity(&rt, "scroll-bar"));
    // Leave thumb: hides cleanly.
    rt.pointer_move(100.0, 50.0);
    frames(&mut rt, 90);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0, "then fades out after pointer leaves thumb");
}

/// CSS `cubic-bezier(0.25, 0.1, 0.25, 1)` (`ease`) at progress `x`, solved here rather than read
/// from `rune_motion::Easing`, so the curve is checked against the CSS definition.
fn css_ease(x: f64) -> f64 {
    let bez = |a: f64, b: f64, t: f64| 3.0 * a * t * (1.0 - t) * (1.0 - t) + 3.0 * b * t * t * (1.0 - t) + t * t * t;
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if bez(0.25, 0.25, mid) < x { lo = mid } else { hi = mid }
    }
    bez(0.1, 1.0, (lo + hi) / 2.0)
}

fn scroll_view_pane(_: &usize, _: &mut rune::Cx<usize>) -> El {
    let k = Kit::neutral(true);
    col().size(200.0, 100.0).child(
        col()
            .key("scroll")
            .w_full()
            .h(100.0)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .child(col().children((0..20).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
            .scroll_bar_styled(ScrollBarStyle::scroll_view(&k)),
    )
}

/// `scroll-view.css`'s `transition: opacity 200ms ease`, both ways: the pointer over the area
/// fades the bar in on CSS `ease` over 200 ms; a wheel and a leave 1 ms later fade it out on the
/// same curve 800 ms after the scroll. Sampled every millisecond, so a frame's own timing error
/// stays under 0.01.
#[test]
fn scroll_view_bar_fades_in_and_out_on_the_css_ease_curve() {
    let dt = 0.001;
    let mut rt = rune::Runtime::new(0usize, scroll_view_pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    frames(&mut rt, 2);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0);
    let mut t = 0.0;
    // The frame before the pointer comes: a fade that starts on the next frame starts after it.
    let mut samples = vec![(t, opacity(&rt, "scroll-bar"))];
    let mut run = |rt: &mut rune::Runtime<usize>, secs: f64, samples: &mut Vec<(f64, f32)>| {
        for _ in 0..(secs / dt).round() as usize {
            rt.frame(dt);
            t += dt;
            samples.push((t, opacity(rt, "scroll-bar")));
        }
    };
    rt.pointer_move(100.0, 50.0);
    run(&mut rt, 0.5, &mut samples);
    rt.wheel(100.0, 50.0, -40.0);
    run(&mut rt, 0.001, &mut samples);
    let scrolled = samples.last().unwrap().0;
    rt.pointer_leave();
    run(&mut rt, 1.3, &mut samples);

    let check = |name: &str, start: f64, fading_out: bool| {
        for ms in [25.0, 50.0, 100.0, 150.0, 175.0] {
            let want = if fading_out { 1.0 - css_ease(ms / 200.0) } else { css_ease(ms / 200.0) };
            let (at, got) = samples.iter().copied().find(|(t, _)| *t >= start + ms / 1000.0 - 1e-9).unwrap();
            assert!((got as f64 - want).abs() < 0.012, "{name} at +{ms} ms ({at:.3} s): {got} vs CSS ease {want:.4}");
        }
    };
    // Each fade starts after the last frame still at its starting value, within one 1 ms frame.
    let shown = samples.iter().rev().find(|(t, o)| *t < scrolled && *o == 0.0).unwrap().0;
    check("fade in", shown, false);
    let held = samples.iter().rev().find(|(t, o)| *t > scrolled && *o == 1.0).unwrap().0;
    assert!((held - (scrolled + 0.8)).abs() <= 0.002, "held 800 ms after the scroll: until {held:.3} s, scrolled at {scrolled:.3} s");
    check("fade out", held, true);
}

/// The fade lands on exactly 0 for a host that draws only the frames the runtime asks for (120
/// Hz here): the last frame drawn is the one left on screen.
#[test]
fn scroll_view_bar_fade_rests_at_exactly_zero() {
    let dt = 1.0 / 120.0;
    let mut rt = rune::Runtime::new(0usize, scroll_view_pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    frames(&mut rt, 2);
    rt.pointer_move(100.0, 50.0);
    frames(&mut rt, 30);
    rt.wheel(100.0, 50.0, -40.0);
    rt.frame(dt);
    rt.pointer_leave();
    let mut fading = false;
    let mut last = None;
    for _ in 0..240 {
        let wants = rt.frame(dt) || rt.live_pending();
        let o = opacity(&rt, "scroll-bar");
        fading |= o > 0.0 && o < 1.0 && last.is_some_and(|l: f32| o < l);
        last = Some(o);
        if fading && !wants { break; }
    }
    assert!(fading, "the bar faded out");
    assert_eq!(last, Some(0.0), "the last frame drawn shows the bar at exactly 0");
}

#[test]
fn always_visible_keeps_native_and_preset_bars_shown_when_overflowing_and_hidden_when_fitting() {
    const THUMB_C: Color = Color { r: 0.7, g: 0.7, b: 0.7, a: 1.0 };
    const HOVER_C: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    #[derive(Clone, Copy)]
    enum Preset { NativeAlways, NativeDefault, KitAlways, MenuAlways }

    fn pane(s: &(usize, Preset), _: &mut rune::Cx<(usize, Preset)>) -> El {
        let k = Kit::neutral(true);
        let style = match s.1 {
            Preset::NativeAlways => ScrollBarStyle::native(THUMB_C, HOVER_C).inside().always_visible(),
            Preset::NativeDefault => ScrollBarStyle::native(THUMB_C, HOVER_C).inside(),
            Preset::KitAlways => ScrollBarStyle::from_kit(&k).always_visible(),
            Preset::MenuAlways => ScrollBarStyle::menu_scroll(THUMB_C, HOVER_C).always_visible(),
        };
        col().size(200.0, 100.0).child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..s.0).map(|i| el().key(format!("r{i}")).w_full().h(20.0))))
                .scroll_bar_styled(style),
        )
    }

    // 1. native().always_visible() stays opacity 1.0 under WhenScrolling and Automatic+trackpad,
    //    goes idle with zero redraws, hides when content fits or is empty, and returns on overflow.
    let mut rt = rune::Runtime::new((20, Preset::NativeAlways), pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    assert_eq!(rt.system_scroll_bars(), ScrollBars::WhenScrolling);
    rt.frame(0.0);
    assert_eq!(opacity(&rt, "scroll-bar"), 1.0, "shown on frame 0 without hover or scroll");
    frames(&mut rt, 120);
    assert_eq!(opacity(&rt, "scroll-bar"), 1.0, "stays visible at rest");
    rt.pointer_move(100.0, 50.0);
    frames(&mut rt, 10);
    rt.pointer_leave();
    frames(&mut rt, 30);
    assert_eq!(opacity(&rt, "scroll-bar"), 1.0, "leaving the area keeps always_visible bar up");
    rt.set_system_scroll_bars(ScrollBars::Automatic);
    rt.set_system_scroller_overlay(true);
    frames(&mut rt, 30);
    assert_eq!(opacity(&rt, "scroll-bar"), 1.0, "ignores Automatic+trackpad overlay preference");
    assert_eq!((rt.frame(1.0 / 60.0), rt.live_pending()), (false, false), "idle window draws nothing");

    // Empty and fitting content hide the bar cleanly.
    for rows in [0usize, 3] {
        rt.update(|s, _| s.0 = rows);
        frames(&mut rt, 2);
        assert_eq!(rect(&rt, "scroll-bar")[3], 0.0, "hidden when {rows} rows fit");
        assert_eq!(rect(&rt, "thumb")[3], 0.0);
    }
    // Growing back to overflow shows it again at opacity 1.0.
    rt.update(|s, _| s.0 = 20);
    frames(&mut rt, 2);
    assert_eq!(opacity(&rt, "scroll-bar"), 1.0);
    assert!(rect(&rt, "thumb")[3] > 0.0);

    // 2. Unchanged preset: native() without always_visible() still rests at opacity 0.0.
    let mut default_native = rune::Runtime::new((20, Preset::NativeDefault), pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    default_native.frame(0.0);
    assert_eq!(opacity(&default_native, "scroll-bar"), 0.0, "native() default unchanged");

    // 3. from_kit().always_visible() ignores WhenScrolling.
    let mut kit = rune::Runtime::new((20, Preset::KitAlways), pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    kit.frame(0.0);
    frames(&mut kit, 60);
    assert_eq!(opacity(&kit, "scroll-bar"), 1.0);

    // 4. menu_scroll().always_visible() stays visible and highlights thumb on hover.
    let mut menu = rune::Runtime::new((20, Preset::MenuAlways), pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut menu, 2);
    assert_eq!(opacity(&menu, "scroll-bar"), 1.0);
    assert_eq!(info(&menu, "thumb").fill, THUMB_C);
    let tr = rect(&menu, "thumb");
    menu.pointer_move(tr[0] + tr[2] / 2.0, tr[1] + tr[3] / 2.0);
    frames(&mut menu, 2);
    assert_eq!(info(&menu, "thumb").fill, HOVER_C, "always_visible menu_scroll thumb highlights on hover");
}

#[test]
fn track_and_track_hover_paint_trough_at_rest_and_on_hover_without_view_runs() {
    thread_local! {
        static RUNS: Cell<usize> = const { Cell::new(0) };
    }
    const THUMB_C: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    const THUMB_HOVER_C: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
    const TROUGH: Color = Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 };
    const TROUGH_HOVER: Color = Color { r: 0.4, g: 0.5, b: 0.6, a: 1.0 };

    #[derive(Clone, Copy)]
    enum Mode { Both, RestOnly, HoverOnly, Neither, KitSmooth }

    fn pane(mode: &Mode, _: &mut rune::Cx<Mode>) -> El {
        RUNS.set(RUNS.get() + 1);
        let k = Kit::neutral(true);
        let style = match *mode {
            Mode::Both => ScrollBarStyle::native(THUMB_C, THUMB_HOVER_C)
                .inside()
                .always_visible()
                .track(TROUGH)
                .track_hover(TROUGH_HOVER),
            Mode::RestOnly => ScrollBarStyle::native(THUMB_C, THUMB_HOVER_C)
                .inside()
                .always_visible()
                .track(TROUGH),
            Mode::HoverOnly => ScrollBarStyle::native(THUMB_C, THUMB_HOVER_C)
                .inside()
                .always_visible()
                .track_hover(TROUGH_HOVER),
            Mode::Neither => ScrollBarStyle::native(THUMB_C, THUMB_HOVER_C)
                .inside()
                .always_visible(),
            Mode::KitSmooth => ScrollBarStyle::from_kit(&k)
                .always_visible()
                .track(TROUGH)
                .track_hover(TROUGH_HOVER),
        };
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

    // 1. Both track() and track_hover(): rest trough, strip hover (without thumb hover), thumb hover, drag.
    RUNS.set(0);
    let mut rt = rune::Runtime::new(Mode::Both, pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    rt.check_live_only(false);
    frames(&mut rt, 2);
    let (runs, generation) = (RUNS.get(), rune::commit_generation());
    assert_eq!(info(&rt, "scroll-bar").fill, TROUGH, "rest trough colour");
    assert_eq!(info(&rt, "thumb").fill, THUMB_C);
    // Still hidden from the accessibility tree.
    let a11y = rt.a11y_json();
    assert!(!a11y.contains("scroll-bar") && !a11y.contains("\"thumb\""), "scrollbar chrome stays a11y_hidden");

    // Hovering viewport middle leaves both at rest colours.
    rt.pointer_move(100.0, 50.0);
    frames(&mut rt, 2);
    assert_eq!(info(&rt, "scroll-bar").fill, TROUGH);
    assert_eq!(info(&rt, "thumb").fill, THUMB_C);

    // Hovering the track strip below the thumb activates track_hover while thumb stays at rest.
    rt.pointer_move(196.0, 85.0);
    frames(&mut rt, 2);
    assert_eq!(info(&rt, "scroll-bar").fill, TROUGH_HOVER, "track hover active over strip");
    assert_eq!(info(&rt, "thumb").fill, THUMB_C, "thumb stays at rest when pointer is only on track");

    // Hovering the thumb activates both track_hover and thumb_hover.
    let tr = rect(&rt, "thumb");
    let (tx, ty) = (tr[0] + tr[2] / 2.0, tr[1] + tr[3] / 2.0);
    rt.pointer_move(tx, ty);
    frames(&mut rt, 2);
    assert_eq!(info(&rt, "scroll-bar").fill, TROUGH_HOVER);
    assert_eq!(info(&rt, "thumb").fill, THUMB_HOVER_C);

    // Dragging the thumb off the strip holds track_hover until release.
    rt.pointer_down(tx, ty);
    rt.pointer_move(100.0, ty + 20.0);
    frames(&mut rt, 2);
    assert_eq!(info(&rt, "scroll-bar").fill, TROUGH_HOVER, "trough stays hovered while dragging");
    rt.pointer_up(100.0, ty + 20.0);
    frames(&mut rt, 2);
    assert_eq!(info(&rt, "scroll-bar").fill, TROUGH, "returns to rest trough on release");
    assert_eq!(info(&rt, "thumb").fill, THUMB_C);

    // Leaving window clears hover cleanly.
    rt.pointer_move(196.0, 85.0);
    frames(&mut rt, 2);
    assert_eq!(info(&rt, "scroll-bar").fill, TROUGH_HOVER);
    rt.pointer_leave();
    frames(&mut rt, 2);
    assert_eq!(info(&rt, "scroll-bar").fill, TROUGH);
    assert_eq!((RUNS.get(), rune::commit_generation()), (runs, generation), "zero view runs and commits");
    assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());

    // 2. track() alone keeps the same colour on hover.
    let mut rest_only = rune::Runtime::new(Mode::RestOnly, pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut rest_only, 2);
    assert_eq!(info(&rest_only, "scroll-bar").fill, TROUGH);
    rest_only.pointer_move(196.0, 85.0);
    frames(&mut rest_only, 2);
    assert_eq!(info(&rest_only, "scroll-bar").fill, TROUGH, "track() without track_hover stays visible on hover");

    // 3. track_hover() alone is transparent at rest and fills on strip hover.
    let mut hover_only = rune::Runtime::new(Mode::HoverOnly, pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut hover_only, 2);
    assert_eq!(info(&hover_only, "scroll-bar").fill, Color::TRANSPARENT);
    hover_only.pointer_move(196.0, 85.0);
    frames(&mut hover_only, 2);
    assert_eq!(info(&hover_only, "scroll-bar").fill, TROUGH_HOVER);
    hover_only.pointer_move(100.0, 50.0);
    frames(&mut hover_only, 2);
    assert_eq!(info(&hover_only, "scroll-bar").fill, Color::TRANSPARENT);

    // 4. Default (neither set) paints transparent at rest and on hover.
    let mut neither = rune::Runtime::new(Mode::Neither, pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    frames(&mut neither, 2);
    assert_eq!(info(&neither, "scroll-bar").fill, Color::TRANSPARENT);
    neither.pointer_move(196.0, 85.0);
    frames(&mut neither, 2);
    assert_eq!(info(&neither, "scroll-bar").fill, Color::TRANSPARENT);

    // 5. Smooth preset (from_kit) transitions track fill on hover, settles cleanly, and goes idle
    //    (and under reduced motion snaps spatial widening immediately while settling colour).
    let mut reduced = rune::Runtime::new(Mode::KitSmooth, pane, (200.0, 100.0), rune::Env::new(true, 1.0));
    reduced.set_system_reduced_motion(true);
    reduced.set_live_waker(std::sync::Arc::new(|| {}));
    frames(&mut reduced, 2);
    assert_eq!(info(&reduced, "scroll-bar").fill, TROUGH);
    reduced.pointer_move(195.0, 50.0);
    reduced.frame(1.0 / 60.0);
    assert_eq!(rect(&reduced, "thumb")[2], THUMB_HOVER, "spatial widening snaps under reduced motion");
    frames(&mut reduced, 30);
    assert_eq!(info(&reduced, "scroll-bar").fill, TROUGH_HOVER, "track hover colour settles");
    assert_eq!((reduced.frame(1.0 / 60.0), reduced.live_pending()), (false, false), "idle once settled");
}
