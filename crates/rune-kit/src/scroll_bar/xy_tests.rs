use std::sync::atomic::{AtomicUsize, Ordering};

use rune::ScrollBars;

use super::*;
use super::tests::{frames, opacity};

fn find_track<S>(rt: &rune::Runtime<S>, track_key: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(track_key))
        .unwrap_or_else(|| panic!("missing track {track_key}"))
}

fn find_thumb<S>(rt: &rune::Runtime<S>, track_key: &str) -> rune::ElementInfo {
    let needle = format!("/{track_key}/thumb-slot/thumb");
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(&needle))
        .unwrap_or_else(|| panic!("missing thumb under {track_key}"))
}

fn content_rect<S>(rt: &rune::Runtime<S>) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("canvas"))
        .unwrap()
        .rect
}

fn xy_view(size: &(f32, f32), _: &mut rune::Cx<(f32, f32)>) -> El {
    let k = Kit::neutral(true);
    let (w, h) = *size;
    col().size(200.0, 120.0).child(
        col()
            .key("scroll")
            .w_full()
            .h(120.0)
            .scroll_xy()
            .child(el().key("canvas").size(w, h))
            .scroll_bars_xy(&k),
    )
}

#[test]
fn two_axis_overflow_shows_both_bars_with_clear_corner_and_drags_independently() {
    let mut rt = rune::Runtime::new((800.0f32, 600.0f32), xy_view, (200.0, 120.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);

    let v_track = find_track(&rt, "scroll-bar").rect;
    let h_track = find_track(&rt, "scroll-bar-x").rect;
    assert_eq!(v_track, [200.0 - TRACK, INSET, TRACK, 120.0 - 2.0 * INSET - TRACK]);
    assert_eq!(h_track, [INSET, 120.0 - TRACK, 200.0 - 2.0 * INSET - TRACK, TRACK]);
    // Tracks leave the bottom-right corner completely clear (zero overlap).
    assert!(v_track[1] + v_track[3] < h_track[1], "vertical ends above horizontal: {v_track:?} vs {h_track:?}");
    assert!(h_track[0] + h_track[2] < v_track[0], "horizontal ends left of vertical: {h_track:?} vs {v_track:?}");

    // Drag the vertical thumb to its end: scrolls vertically, leaves horizontal at 0.
    let vt = find_thumb(&rt, "scroll-bar").rect;
    let vx = vt[0] + vt[2] / 2.0;
    let vy0 = vt[1] + vt[3] / 2.0;
    rt.pointer_move(vx, vy0);
    rt.pointer_down(vx, vy0);
    rt.pointer_move(vx, v_track[1] + v_track[3]);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(vx, v_track[1] + v_track[3]);
    frames(&mut rt, 2);
    let c = content_rect(&rt);
    assert_eq!(c[0], 0.0, "dragging vertical thumb leaves horizontal offset at 0");
    assert_eq!(c[1], -480.0, "dragging vertical thumb to end scrolls to vertical max");

    // Drag the horizontal thumb to its end: scrolls horizontally, preserves vertical max.
    let ht = find_thumb(&rt, "scroll-bar-x").rect;
    let hx0 = ht[0] + ht[2] / 2.0;
    let hy = ht[1] + ht[3] / 2.0;
    rt.pointer_move(hx0, hy);
    rt.pointer_down(hx0, hy);
    rt.pointer_move(h_track[0] + h_track[2], hy);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(h_track[0] + h_track[2], hy);
    frames(&mut rt, 2);
    let c = content_rect(&rt);
    assert_eq!(c[0], -600.0, "dragging horizontal thumb to end scrolls to horizontal max");
    assert_eq!(c[1], -480.0, "dragging horizontal thumb preserves vertical offset");
    assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());
}

#[test]
fn single_axis_overflow_and_fit_adjust_corner_clearance_dynamically() {
    let mut rt = rune::Runtime::new((150.0f32, 600.0f32), xy_view, (200.0, 120.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    // Vertical overflow only: vertical bar takes the full edge, horizontal hides.
    assert_eq!(find_track(&rt, "scroll-bar").rect, [200.0 - TRACK, INSET, TRACK, 120.0 - 2.0 * INSET]);
    assert_eq!(find_track(&rt, "scroll-bar-x").rect[2], 0.0);

    // Content widens so both axes overflow: vertical shortens to clear the corner.
    rt.update(|sz, _| *sz = (600.0, 600.0));
    frames(&mut rt, 2);
    assert_eq!(find_track(&rt, "scroll-bar").rect[3], 120.0 - 2.0 * INSET - TRACK);
    assert_eq!(find_track(&rt, "scroll-bar-x").rect[2], 200.0 - 2.0 * INSET - TRACK);

    // Horizontal overflow only: horizontal takes full width, vertical hides.
    rt.update(|sz, _| *sz = (600.0, 80.0));
    frames(&mut rt, 2);
    assert_eq!(find_track(&rt, "scroll-bar").rect[3], 0.0);
    assert_eq!(find_track(&rt, "scroll-bar-x").rect, [INSET, 120.0 - TRACK, 200.0 - 2.0 * INSET, TRACK]);

    // Fits both ways: both bars hide.
    rt.update(|sz, _| *sz = (120.0, 80.0));
    frames(&mut rt, 2);
    assert_eq!(find_track(&rt, "scroll-bar").rect[3], 0.0);
    assert_eq!(find_track(&rt, "scroll-bar-x").rect[2], 0.0);
}

#[test]
fn when_scrolling_pulses_only_the_scrolled_axis_and_runs_zero_view_commits() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    fn counted(_: &(), _: &mut rune::Cx<()>) -> El {
        RUNS.fetch_add(1, Ordering::Relaxed);
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

    RUNS.store(0, Ordering::Relaxed);
    let mut rt = rune::Runtime::new((), counted, (200.0, 120.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::WhenScrolling);
    rt.check_live_only(false);
    frames(&mut rt, 2);
    assert_eq!(RUNS.load(Ordering::Relaxed), 1);
    let gen0 = rune::commit_generation();
    assert_eq!((opacity(&rt, "scroll-bar"), opacity(&rt, "scroll-bar-x")), (0.0, 0.0));

    // Vertical wheel reveals only the vertical bar.
    rt.wheel(100.0, 60.0, -40.0);
    frames(&mut rt, 15);
    assert!(opacity(&rt, "scroll-bar") > 0.9, "vertical wheel pulses vertical bar");
    assert_eq!(opacity(&rt, "scroll-bar-x"), 0.0, "vertical wheel leaves horizontal bar hidden");
    frames(&mut rt, 90);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0, "vertical bar fades back out");

    // Horizontal wheel reveals only the horizontal bar.
    rt.wheel_xy(100.0, 60.0, -40.0, 0.0);
    frames(&mut rt, 15);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0, "horizontal wheel leaves vertical bar hidden");
    assert!(opacity(&rt, "scroll-bar-x") > 0.9, "horizontal wheel pulses horizontal bar");
    frames(&mut rt, 90);
    assert_eq!(opacity(&rt, "scroll-bar-x"), 0.0, "horizontal bar fades back out");

    // Pointer near the right edge reveals vertical only; near bottom reveals horizontal only;
    // in the bottom-right corner reveals neither.
    rt.pointer_move(192.0, 50.0);
    frames(&mut rt, 25);
    assert!(opacity(&rt, "scroll-bar") > 0.9);
    assert_eq!(opacity(&rt, "scroll-bar-x"), 0.0);

    rt.pointer_move(100.0, 114.0);
    frames(&mut rt, 90);
    assert_eq!(opacity(&rt, "scroll-bar"), 0.0);
    assert!(opacity(&rt, "scroll-bar-x") > 0.9);

    rt.pointer_move(194.0, 114.0);
    frames(&mut rt, 90);
    assert_eq!((opacity(&rt, "scroll-bar"), opacity(&rt, "scroll-bar-x")), (0.0, 0.0));

    assert_eq!(RUNS.load(Ordering::Relaxed), 1, "zero view re-runs across two-axis scrolls and hover");
    assert_eq!(rune::commit_generation(), gen0, "zero commits across two-axis scrolls and hover");
}

#[test]
fn xy_styled_rtl_hover_target_and_extreme_sizes() {
    const TROUGH: Color = Color { r: 0.15, g: 0.15, b: 0.15, a: 1.0 };
    // Even if orientations are swapped by caller, vertical stays vertical and horizontal stays horizontal.
    fn rtl_xy(_: &(), _: &mut rune::Cx<()>) -> El {
        let base = ScrollBarStyle::native(Color::WHITE, Color::WHITE)
            .inside()
            .always_visible()
            .track(TROUGH);
        col().size(200.0, 120.0).dir(rune::Direction::Rtl).child(
            col()
                .key("scroll")
                .w_full()
                .h(120.0)
                .scroll_xy()
                .child(el().key("canvas").size(800.0, 600.0))
                .scroll_bars_xy_styled(base.horizontal(), base),
        )
    }
    let mut rt = rune::Runtime::new((), rtl_xy, (200.0, 120.0), rune::Env::new(true, 1.0));
    frames(&mut rt, 2);
    let vt = find_track(&rt, "scroll-bar");
    let ht = find_track(&rt, "scroll-bar-x");
    assert_eq!(vt.fill, TROUGH);
    assert_eq!(ht.fill, TROUGH);
    // RTL vertical bar sits on the left edge; horizontal bar clears the bottom-left corner.
    assert_eq!(vt.rect[0], 0.0, "RTL vertical track sits on left edge: {:?}", vt.rect);
    assert!(ht.rect[0] >= vt.rect[0] + vt.rect[2], "RTL horizontal track clears bottom-left corner: {:?} vs {:?}", ht.rect, vt.rect);
    assert!(vt.rect[1] + vt.rect[3] < ht.rect[1], "RTL vertical track stops above horizontal: {:?} vs {:?}", vt.rect, ht.rect);

    // scroll_bar_hover reveals both bars when styled with hover_target().
    fn hover_target_xy(_: &(), _: &mut rune::Cx<()>) -> El {
        let k = Kit::neutral(true);
        let style = ScrollBarStyle::scroll_view(&k).hover_target();
        col()
            .key("pane")
            .size(200.0, 120.0)
            .child(
                col()
                    .key("scroll")
                    .w_full()
                    .h(120.0)
                    .scroll_xy()
                    .child(el().key("canvas").size(800.0, 600.0))
                    .scroll_bars_xy_styled(style, style.horizontal()),
            )
            .scroll_bar_hover("scroll")
    }
    let mut rt = rune::Runtime::new((), hover_target_xy, (200.0, 120.0), rune::Env::new(true, 1.0));
    frames(&mut rt, 2);
    assert_eq!((opacity(&rt, "scroll-bar"), opacity(&rt, "scroll-bar-x")), (0.0, 0.0));
    rt.pointer_move(50.0, 50.0);
    frames(&mut rt, 30);
    assert!(opacity(&rt, "scroll-bar") > 0.99 && opacity(&rt, "scroll-bar-x") > 0.99);

    // Huge content clamps both thumbs to MIN_THUMB.
    let mut rt = rune::Runtime::new((100_000.0f32, 100_000.0f32), xy_view, (200.0, 120.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    assert_eq!(find_thumb(&rt, "scroll-bar").rect[3], MIN_THUMB);
    assert_eq!(find_thumb(&rt, "scroll-bar-x").rect[2], MIN_THUMB);

    // Tiny height where corner clearance hides vertical: horizontal gets the full width, no overlap.
    fn tiny_xy(_: &(), _: &mut rune::Cx<()>) -> El {
        let k = Kit::neutral(true);
        col().size(200.0, 36.0).child(
            col()
                .key("scroll")
                .w_full()
                .h(36.0)
                .scroll_xy()
                .child(el().key("canvas").size(800.0, 600.0))
                .scroll_bars_xy(&k),
        )
    }
    let mut rt = rune::Runtime::new((), tiny_xy, (200.0, 36.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    assert_eq!(find_track(&rt, "scroll-bar").rect[3], 0.0, "vertical hides cleanly when too short for corner");
    assert_eq!(find_track(&rt, "scroll-bar-x").rect[2], 200.0 - 2.0 * INSET, "horizontal reclaims full width");
}

#[test]
fn legacy_scroll_bar_impl_without_xy_methods_compiles_and_falls_back_to_vertical() {
    struct LegacyViewport(El);

    impl ScrollBar for LegacyViewport {
        fn scroll_bar(self, k: &Kit) -> El {
            self.0.scroll_bar(k)
        }

        fn scroll_bar_styled(self, style: ScrollBarStyle) -> El {
            self.0.scroll_bar_styled(style)
        }

        fn scroll_bar_hover(self, viewport_key: &str) -> El {
            self.0.scroll_bar_hover(viewport_key)
        }
    }

    fn legacy_view(styled: &bool, _: &mut rune::Cx<bool>) -> El {
        let k = Kit::neutral(true);
        let vp = LegacyViewport(
            col()
                .key("scroll")
                .w_full()
                .h(120.0)
                .scroll_xy()
                .child(el().key("canvas").size(800.0, 600.0)),
        );
        let wrapped = if *styled {
            let v = ScrollBarStyle::from_kit(&k).always_visible().width(9.0).hover_width(9.0);
            let h = ScrollBarStyle::from_kit(&k).horizontal();
            vp.scroll_bars_xy_styled(v, h)
        } else {
            vp.scroll_bars_xy(&k)
        };
        col().size(200.0, 120.0).child(wrapped)
    }

    // Default scroll_bars_xy falls back to scroll_bar(k).
    let mut rt = rune::Runtime::new(false, legacy_view, (200.0, 120.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 2);
    assert_eq!(find_track(&rt, "scroll-bar").rect, [200.0 - TRACK, INSET, TRACK, 120.0 - 2.0 * INSET]);
    assert!(rt.elements().into_iter().all(|e| e.key.as_deref() != Some("scroll-bar-x")));

    // Default scroll_bars_xy_styled falls back to scroll_bar_styled(vertical).
    let mut rt = rune::Runtime::new(true, legacy_view, (200.0, 120.0), rune::Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::WhenScrolling);
    frames(&mut rt, 2);
    assert_eq!(opacity(&rt, "scroll-bar"), 1.0);
    assert!((find_thumb(&rt, "scroll-bar").rect[2] - 9.0).abs() < 0.5);
    assert!(rt.elements().into_iter().all(|e| e.key.as_deref() != Some("scroll-bar-x")));
}
