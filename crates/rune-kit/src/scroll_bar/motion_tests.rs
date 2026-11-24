use rune::ScrollBars;

use super::*;
use super::tests::{HOVER_RUNS, at, frames, hover_view, info, opacity, rect, top, view};

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
        (
            "scroll-bar/two-axis-xy".into(),
            Box::new(|| {
                fn xy(_: &(), _: &mut rune::Cx<()>) -> El {
                    let k = Kit::neutral(true);
                    col().size(200.0, 120.0).child(
                        col()
                            .key("scroll")
                            .w_full()
                            .h(120.0)
                            .scroll_xy()
                            .child(el().key("canvas").size(800.0, 600.0))
                            .scroll_bars_xy(&k),
                    )
                }
                let mut r = Run::new(rune::Runtime::new((), xy, (200.0, 120.0), rune::Env::new(true, 1.0)), 120.0);
                r.idle(0.3);
                for _ in 0..8 {
                    r.act("wheel-y", |rt| rt.wheel(100.0, 60.0, -12.0));
                    r.step();
                }
                r.play(1.6);
                for _ in 0..8 {
                    r.act("wheel-x", |rt| rt.wheel_xy(100.0, 60.0, -12.0, 0.0));
                    r.step();
                }
                r.play(1.6);
                r.act("near-right", |rt| rt.pointer_move(192.0, 50.0));
                r.play(0.4);
                r.act("near-bottom", |rt| rt.pointer_move(100.0, 114.0));
                r.play(1.6);
                r.act("away", |rt| rt.pointer_move(100.0, 50.0));
                r.play(1.6);
                r.act("always", |rt| rt.set_system_scroll_bars(ScrollBars::Always));
                r.play(0.6);
                r.act("when-scrolling", |rt| rt.set_system_scroll_bars(ScrollBars::WhenScrolling));
                r.play(1.2);
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

/// Opacity of the default bar and the native bar, frame by frame through a scroll,
/// an edge reveal and a hover, as FNV-1a over the f32 bits and each frame's request for another.
/// `GOLDEN` was taken on Rune `dev` `14b59a3434`, before ScrollView's fade became a CSS tween: the
/// bars that keep their springs (and the native bar's instant change) draw the same bits.
#[test]
fn bars_that_keep_their_springs_draw_the_same_frames() {
    fn styled(which: &u8, _: &mut rune::Cx<u8>) -> El {
        let k = Kit::neutral(true);
        let style = match which {
            0 => ScrollBarStyle::from_kit(&k),
            _ => ScrollBarStyle::native(k.c.border_base, k.c.border_strong),
        };
        col().size(200.0, 100.0).child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().children((0..24).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                .scroll_bar_styled(style),
        )
    }
    let record = |which: u8| {
        let mut rt = rune::Runtime::new(which, styled, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        fn eat(h: &mut u64, v: u32) {
            for b in v.to_le_bytes() {
                *h ^= u64::from(b);
                *h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let play = |rt: &mut rune::Runtime<u8>, secs: f64, h: &mut u64| {
            for _ in 0..(secs * 120.0).round() as usize {
                let wants = rt.frame(1.0 / 120.0) || rt.live_pending();
                eat(h, u32::from(wants));
                eat(h, opacity(rt, "scroll-bar").to_bits());
                eat(h, opacity(rt, "thumb").to_bits());
            }
        };
        play(&mut rt, 0.3, &mut h);
        for _ in 0..6 {
            rt.wheel(100.0, 50.0, -12.0);
            play(&mut rt, 1.0 / 120.0, &mut h);
        }
        play(&mut rt, 1.6, &mut h);
        rt.pointer_move(184.0, 50.0);
        play(&mut rt, 0.5, &mut h);
        rt.pointer_move(100.0, 50.0);
        play(&mut rt, 0.5, &mut h);
        rt.pointer_leave();
        play(&mut rt, 1.6, &mut h);
        h
    };
    let got = [record(0), record(1)];
    eprintln!("GOLDEN = [{:#018x}, {:#018x}]", got[0], got[1]);
    const GOLDEN: [u64; 2] = [0x64ef_8842_6b44_fac8, 0x629b_5e0d_a6dd_5f85];
    assert_eq!(got, GOLDEN, "kit bar, native bar");
}
