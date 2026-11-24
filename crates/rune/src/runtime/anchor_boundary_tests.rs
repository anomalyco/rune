//! Boundary-clamped anchored overlays (`El::anchored_in`) and the pure `anchor_rect_in` helper:
//! flipping, align-flipping, shifting, `anchored_fit` sizing, scrolled and forward-declared
//! boundaries, window intersection, and fallbacks.
use std::cell::Cell;

use super::*;
use crate::el::{Align, ScrollSpec, Side, anchor_rect, anchor_rect_in};
use crate::{Cx, El, Env, col, el, row};

#[test]
fn anchor_rect_in_flips_clamps_and_matches_window_helper() {
    let win = (800.0, 600.0);
    let win_bounds = [0.0, 0.0, win.0, win.1];
    // Matches anchor_rect identically when bounds equal the window.
    for (t, side, align) in [
        ([20.0, 550.0, 60.0, 570.0], Side::Bottom, Align::Start),
        ([740.0, 100.0, 780.0, 130.0], Side::Right, Align::Start),
        ([12.0, 20.0, 52.0, 40.0], Side::Bottom, Align::End),
        ([200.0, 200.0, 260.0, 224.0], Side::Top, Align::Center),
    ] {
        assert_eq!(
            anchor_rect_in(t, (120.0, 80.0), side, align, 4.0, win_bounds),
            anchor_rect(t, (120.0, 80.0), side, align, 4.0, win),
        );
    }

    // List pane [100, 0, 500, 600]: card anchored Right near x=480 flips Left to x=120..376.
    let r = anchor_rect_in([380.0, 100.0, 480.0, 130.0], (256.0, 80.0), Side::Right, Align::Start, 4.0, [100.0, 0.0, 500.0, 600.0]);
    assert_eq!(r, [120.0, 100.0, 376.0, 180.0]);

    // Shift clamp along cross axis inside a boundary [100, 50, 300, 250].
    let r = anchor_rect_in([105.0, 80.0, 125.0, 100.0], (100.0, 40.0), Side::Bottom, Align::Center, 4.0, [100.0, 50.0, 300.0, 250.0]);
    assert_eq!(r[0], 108.0, "clamped 8pt inside boundary left edge: {r:?}");
    assert_eq!(r[1], 104.0);

    // Degenerate / zero / huge sizes never panic and stay finite.
    for (size, bounds) in [
        ((0.0, 0.0), [100.0, 50.0, 400.0, 300.0]),
        ((2000.0, 2000.0), [100.0, 50.0, 400.0, 300.0]),
        ((80.0, 40.0), [100.0, 100.0, 100.0, 100.0]),
    ] {
        let out = anchor_rect_in([150.0, 120.0, 190.0, 144.0], size, Side::Bottom, Align::Start, 4.0, bounds);
        assert!(out.iter().all(|v| v.is_finite()), "non-finite result {out:?}");
    }
}

#[test]
fn list_plus_drawer_repro_flips_left_inside_boundary_and_reports_placement() {
    struct S {
        bounded: bool,
        seen: Cell<Option<(Side, Align)>>,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        s.seen.set(cx.anchor_placement("card"));
        // 400pt list at x=100..500 next to a 300pt drawer at x=500..800 in an 800x600 window.
        let list = col().key("list").absolute(100.0, 0.0).size(400.0, 600.0)
            .child(el().key("row-trigger").absolute(300.0, 100.0).size(90.0, 30.0));
        let drawer = col().key("drawer").absolute(500.0, 0.0).size(300.0, 600.0);
        let mut card = col().key("card").size(256.0, 80.0)
            .anchored("row-trigger", Side::Right, Align::Start, 8.0);
        if s.bounded {
            card = card.anchored_in("list");
        }
        col().size(800.0, 600.0).child(list).child(drawer).child(card)
    }

    // Without anchored_in: lands at x = 498 (overlaps the drawer at x = 500..800).
    let mut rt_unbounded = Runtime::new(S { bounded: false, seen: Cell::new(None) }, view, (800.0, 600.0), Env::new(true, 1.0));
    rt_unbounded.frame(0.0);
    let r = rt_unbounded.presented("card").unwrap().0;
    assert_eq!(r, [498.0, 100.0, 256.0, 80.0]);
    assert_eq!(rt_unbounded.state.seen.get(), Some((Side::Right, Align::Start)));

    // With anchored_in("list"): flips Left to x = 400 - 256 - 8 = 136, staying inside [108, 492].
    let mut rt_bounded = Runtime::new(S { bounded: true, seen: Cell::new(None) }, view, (800.0, 600.0), Env::new(true, 1.0));
    rt_bounded.frame(0.0);
    let r = rt_bounded.presented("card").unwrap().0;
    assert_eq!(r, [136.0, 100.0, 256.0, 80.0]);
    assert!(r[0] >= 108.0 && r[0] + r[2] <= 492.0, "stays inside list boundary: {r:?}");
    assert_eq!(rt_bounded.state.seen.get(), Some((Side::Left, Align::Start)));
}

#[test]
fn alignment_flip_and_shift_clamping_inside_boundary() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let pane = col().key("pane").absolute(100.0, 80.0).size(260.0, 200.0)
            // Near pane's right edge -> Bottom/Start flips align to End (x = 340 - 120 = 220).
            .child(el().key("t-right").absolute(200.0, 20.0).size(40.0, 24.0))
            // Near pane's left edge -> Bottom/Center shifts right to pane_left + 8 = 108.
            .child(el().key("t-left").absolute(4.0, 100.0).size(20.0, 24.0));
        let pop_flip = col().key("pop-flip").size(120.0, 50.0)
            .anchored("t-right", Side::Bottom, Align::Start, 4.0)
            .anchored_in("pane");
        let pop_shift = col().key("pop-shift").size(120.0, 50.0)
            .anchored("t-left", Side::Bottom, Align::Center, 4.0)
            .anchored_in("pane");
        col().size(600.0, 400.0).child(pane).child(pop_flip).child(pop_shift)
    }

    let mut rt = Runtime::new((), view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.presented("pop-flip").unwrap().0, [220.0, 128.0, 120.0, 50.0]);
    assert_eq!(rt.presented("pop-shift").unwrap().0, [108.0, 208.0, 120.0, 50.0]);
}

#[test]
fn anchored_fit_clamps_max_size_inside_boundary() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        // Pane at y=100..320 (220pt tall), x=80..360 (280pt wide) inside a large 800x600 window.
        let pane = col().key("pane").absolute(80.0, 100.0).size(280.0, 220.0)
            .child(el().key("v-trig").absolute(20.0, 150.0).size(80.0, 24.0))
            .child(el().key("h-trig").absolute(210.0, 40.0).size(50.0, 24.0));
        let rows = col().key("rows").children((0..30).map(|i| el().key(format!("r{i}")).w(100.0).h(20.0)));
        let list = col().key("scroll").shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
        // Above v-trig (y=250) inside pane (y0=100): 250 - 4 - (100 + 8) = 138pt -> fitted height = 137pt.
        // Below v-trig (y=274) inside pane (y1=320): 320 - 8 - (274 + 4) = 34pt.
        let v_menu = col().key("v-menu").w(100.0).child(list)
            .anchored_fit("v-trig", Side::Bottom, Align::Start, 4.0)
            .anchored_in("pane");
        // Left of h-trig (x=290) inside pane (x0=80): 290 - 4 - (80 + 8) = 198pt -> fitted width = 197pt.
        let h_card = col().key("h-card").h(40.0)
            .child(el().key("wide").w(400.0).h(40.0).shrink())
            .anchored_fit("h-trig", Side::Right, Align::Start, 4.0)
            .anchored_in("pane");
        col().size(800.0, 600.0).child(pane).child(v_menu).child(h_card)
    }

    let mut rt = Runtime::new((), view, (800.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let vr = rt.presented("v-menu").unwrap().0;
    assert_eq!(vr[3], 137.0, "vertical anchored_fit bounded to pane room above: {vr:?}");
    assert!(vr[1] >= 108.0 && vr[1] + vr[3] <= 246.0, "flipped above inside pane: {vr:?}");

    let hr = rt.presented("h-card").unwrap().0;
    assert_eq!(hr[2], 197.0, "horizontal anchored_fit bounded to pane room left: {hr:?}");
    assert!(hr[0] >= 88.0 && hr[0] + hr[2] <= 286.0, "flipped left inside pane: {hr:?}");
}

#[test]
fn scrolled_boundary_updates_flip_and_clamp() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        // A tall scrolling column holding a 260pt-tall pane at y = 200..460 initially.
        let pane = col().key("pane").w(300.0).h(260.0)
            .child(el().key("trig").absolute(20.0, 170.0).size(80.0, 24.0));
        let scroller = col().key("scroller").size(400.0, 400.0).scroll(ScrollSpec { stick_bottom: false })
            .child(el().w(300.0).h(200.0))
            .child(pane)
            .child(el().w(300.0).h(300.0));
        // Card 80pt tall below trig: inside pane, space below trig is 260 - 194 - 4 - 8 = 54pt -> flips Above!
        // When scroller is at 0, pane's bottom is 460 (clamped to window 400), trig is at y=370..394,
        // so card flips above to y = 370 - 80 - 4 = 286.
        // After scrolling down 150pt, pane moves to y=50..310 and trig moves to y=220..244;
        // card still flips above inside the scrolled pane to y = 220 - 80 - 4 = 136!
        let card = col().key("card").size(100.0, 80.0)
            .anchored("trig", Side::Bottom, Align::Start, 4.0)
            .anchored_in("pane");
        col().size(600.0, 400.0).child(scroller).child(card)
    }

    let mut rt = Runtime::new((), view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.presented("card").unwrap().0, [20.0, 286.0, 100.0, 80.0]);

    rt.wheel(100.0, 100.0, -150.0);
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let pane_r = rt.presented("pane").unwrap().0;
    let card_r = rt.presented("card").unwrap().0;
    assert_eq!(pane_r, [0.0, 50.0, 300.0, 260.0]);
    assert_eq!(card_r, [20.0, 136.0, 100.0, 80.0]);
    assert!(card_r[1] >= pane_r[1] + 8.0 && card_r[1] + card_r[3] <= pane_r[1] + pane_r[3] - 8.0);
}

#[test]
fn forward_declared_boundary_defers_and_resolves() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        // Mount overlay BEFORE both its anchor and its boundary.
        let card = col().key("card").size(200.0, 60.0)
            .anchored("trig", Side::Right, Align::Start, 4.0)
            .anchored_in("later-pane");
        let pane = col().key("later-pane").absolute(100.0, 50.0).size(320.0, 200.0)
            .child(el().key("trig").absolute(240.0, 40.0).size(60.0, 24.0));
        col().size(700.0, 400.0).child(card).child(pane)
    }

    let mut rt = Runtime::new((), view, (700.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Trigger sits at x = 340..400 inside pane [100, 50, 420, 250].
    // Right would end at 604 > 412, so flips Left to x = 340 - 200 - 4 = 136 >= 108.
    assert_eq!(rt.presented("card").unwrap().0, [136.0, 90.0, 200.0, 60.0]);
}

#[test]
fn boundary_exceeding_window_is_intersected_with_window() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        // Boundary extends past the right edge of a 400pt window (x = 50..700).
        let wide_pane = col().key("wide-pane").absolute(50.0, 20.0).size(650.0, 260.0)
            .child(el().key("trig").absolute(290.0, 60.0).size(40.0, 24.0));
        // Trigger is at x = 340..380; window ends at 400 so right overflows even though pane extends to 700.
        let card = col().key("card").size(120.0, 50.0)
            .anchored("trig", Side::Right, Align::Start, 4.0)
            .anchored_in("wide-pane");
        col().size(400.0, 300.0).child(wide_pane).child(card)
    }

    let mut rt = Runtime::new((), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Flips Left to x = 340 - 120 - 4 = 216 instead of escaping past x = 400.
    assert_eq!(rt.presented("card").unwrap().0, [216.0, 80.0, 120.0, 50.0]);
}

#[test]
fn missing_and_self_boundary_keys_fall_back_to_window() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let trig = el().key("trig").absolute(300.0, 60.0).size(40.0, 24.0);
        let missing = col().key("missing-b").size(120.0, 50.0)
            .anchored("trig", Side::Right, Align::Start, 4.0)
            .anchored_in("no-such-pane");
        let self_b = col().key("self-b").size(120.0, 50.0)
            .anchored("trig", Side::Right, Align::Start, 4.0)
            .anchored_in("self-b");
        let plain = col().key("plain").size(120.0, 50.0)
            .anchored("trig", Side::Right, Align::Start, 4.0);
        col().size(600.0, 300.0).child(trig).child(missing).child(self_b).child(plain)
    }

    let mut rt = Runtime::new((), view, (600.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let expected = rt.presented("plain").unwrap().0;
    assert_eq!(expected, [344.0, 60.0, 120.0, 50.0]);
    assert_eq!(rt.presented("missing-b").unwrap().0, expected);
    assert_eq!(rt.presented("self-b").unwrap().0, expected);
}

#[test]
fn wrap_scroll_area_preserves_anchored_in_boundary() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let pane = col().key("pane").absolute(100.0, 0.0).size(300.0, 400.0)
            .child(el().key("trig").absolute(220.0, 60.0).size(60.0, 24.0));
        let inner = col().key("card-scroll").size(160.0, 60.0)
            .anchored("trig", Side::Right, Align::Start, 4.0)
            .anchored_in("pane");
        let (_, _, outer, inner) = inner.wrap_scroll_area(("test", 1, 1));
        let card = outer.child(inner);
        row().size(800.0, 400.0).child(pane).child(card)
    }

    let mut rt = Runtime::new((), view, (800.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Trigger is at x = 320..380 inside pane [100, 0, 400, 400].
    // Card flips Left to x = 320 - 160 - 4 = 156 because wrap_scroll_area carried above_boundary.
    assert_eq!(rt.presented("card-scroll-scroll-area").unwrap().0, [156.0, 60.0, 160.0, 60.0]);
}
