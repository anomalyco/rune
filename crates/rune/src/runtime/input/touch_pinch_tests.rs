use super::*;
use super::touch_machine::{Contact, MIN_SPAN, PinchChannel, SPAN_SLOP, TouchState};
use crate::{GesturePhase, Live, MagnifyEvent, Phase, ScrollSpec, col, el, row};

struct ChartState {
    magnifies: Vec<MagnifyEvent>,
    taps: usize,
    secondary: usize,
    drags: Vec<(Phase, bool)>,
    accept_magnify: bool,
    live_only: bool,
    live_zoom: Live<f32>,
    show_chart: bool,
    inert_chart: bool,
    modal_open: bool,
    sibling_magnifies: Vec<MagnifyEvent>,
}

impl ChartState {
    fn new() -> Self {
        Self {
            magnifies: Vec::new(),
            taps: 0,
            secondary: 0,
            drags: Vec::new(),
            accept_magnify: true,
            live_only: false,
            live_zoom: Live::new(1.0),
            show_chart: true,
            inert_chart: false,
            modal_open: false,
            sibling_magnifies: Vec::new(),
        }
    }
}

/// A 400x400 window with a scrollable list holding a 200x200 chart at (50, 50) and a sibling
/// 100x200 chart at (270, 50), plus tall content below so the list can scroll and fling.
fn chart_view(s: &ChartState, _: &mut Cx<ChartState>) -> El {
    let chart = s.show_chart.then(|| {
        let e = el()
            .key("chart")
            .absolute(50.0, 50.0)
            .size(200.0, 200.0)
            .live_scale(&s.live_zoom)
            .on_press(|s: &mut ChartState| s.taps += 1)
            .on_secondary_press(|s: &mut ChartState, _, _| s.secondary += 1)
            .on_drag(|s: &mut ChartState, d: &mut Drag, _| s.drags.push((d.phase, d.cancelled())))
            .on_magnify(|s: &mut ChartState, ev: MagnifyEvent, tx| {
                s.magnifies.push(ev);
                if s.live_only {
                    s.live_zoom.set((s.live_zoom.get() * (1.0 + ev.delta)).clamp(0.1, 8.0));
                    tx.live_only();
                }
                s.accept_magnify
            });
        if s.inert_chart { e.inert() } else { e }
    });
    let sibling = el()
        .key("sibling")
        .absolute(270.0, 50.0)
        .size(100.0, 200.0)
        .on_magnify(|s: &mut ChartState, ev: MagnifyEvent, _| {
            s.sibling_magnifies.push(ev);
            true
        });
    let body = el().key("body").w_full().h(1200.0).children(chart).child(sibling);
    let list = col()
        .key("list")
        .size(400.0, 400.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(body);
    let modal = s.modal_open.then(|| {
        el().key("sheet")
            .absolute(10.0, 10.0)
            .size(80.0, 80.0)
            .modal(true, |s: &mut ChartState| s.modal_open = false)
    });
    el().w_full().h_full().child(list).children(modal)
}

fn chart_rig(s: ChartState) -> Runtime<ChartState> {
    let mut rt = Runtime::new(s, chart_view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.frame(0.0);
    rt
}

fn list_offset(rt: &Runtime<ChartState>) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    rt.scroll.scrolls[&id].offset
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn two_finger_pinch_emits_began_moved_ended_after_span_slop_with_slop_subtracted() {
    let mut rt = chart_rig(ChartState::new());
    // Chart sits at (50..250, 50..250). Two fingers land 100 pt apart, midpoint (150, 150) -> local (100, 100).
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 200.0, 150.0, 0.01);
    rt.frame(1.0 / 60.0);
    assert!(rt.touching());
    assert!(rt.state.magnifies.is_empty(), "landing alone waits for SPAN_SLOP ({SPAN_SLOP} pt)");

    // Spread by exactly SPAN_SLOP (16 pt, span 100 -> 116): still within slop, emits nothing.
    rt.touch(2, TouchPhase::Moved, 216.0, 150.0, 0.02);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.magnifies.is_empty(), "span change <= SPAN_SLOP emits nothing");

    // Cross slop to span 126 (excess 10 pt past 116): Began(0.0) + Moved(126/116 - 1.0), wheel=false.
    rt.touch(2, TouchPhase::Moved, 226.0, 150.0, 0.03);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 2);
    let began = rt.state.magnifies[0];
    let first_move = rt.state.magnifies[1];
    assert_eq!(began.phase, GesturePhase::Began);
    assert_eq!(began.delta, 0.0);
    assert!(!began.wheel);
    // Midpoint of (100, 150) and (226, 150) is (163, 150) -> chart local (113, 100).
    assert!(approx(began.x, 113.0) && approx(began.y, 100.0), "began local: {began:?}");
    assert_eq!(first_move.phase, GesturePhase::Moved);
    assert!(!first_move.wheel);
    let expected_first = 126.0 / 116.0 - 1.0;
    assert!(approx(first_move.delta, expected_first), "slop subtracted: got {}, want {expected_first}", first_move.delta);
    assert!(approx(first_move.x, 113.0) && approx(first_move.y, 100.0));

    // Spread finger 1 left to x=0 -> span 226, midpoint (113, 150) -> local (63, 100).
    rt.touch(1, TouchPhase::Moved, 0.0, 150.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 3);
    let second_move = rt.state.magnifies[2];
    assert_eq!(second_move.phase, GesturePhase::Moved);
    assert!(approx(second_move.delta, 226.0 / 126.0 - 1.0));
    assert!(approx(second_move.x, 63.0) && approx(second_move.y, 100.0));
    // Cumulative scale from slop boundary 116 to 226 is exact.
    let cumulative = (1.0 + first_move.delta) * (1.0 + second_move.delta);
    assert!(approx(cumulative, 226.0 / 116.0), "cumulative={cumulative}");

    // Lift finger 2 -> Ended(0.0).
    rt.touch(2, TouchPhase::Ended, 226.0, 150.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 4);
    let ended = rt.state.magnifies[3];
    assert_eq!(ended.phase, GesturePhase::Ended);
    assert_eq!(ended.delta, 0.0);
    assert!(!ended.wheel);
    assert!(rt.touching(), "finger 1 is still down in PostPinch");

    // Lift finger 1 -> Idle, no second Ended, no tap.
    rt.touch(1, TouchPhase::Ended, 0.0, 150.0, 0.06);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 4);
    assert_eq!(rt.state.taps, 0);
    assert!(!rt.touching());

    // Pinch IN: initial span 100 -> 80 (crosses 16 pt inward, boundary = 84, delta = 80/84 - 1.0).
    rt.state.magnifies.clear();
    rt.touch(3, TouchPhase::Started, 100.0, 150.0, 0.10);
    rt.touch(4, TouchPhase::Started, 200.0, 150.0, 0.11);
    rt.touch(4, TouchPhase::Moved, 180.0, 150.0, 0.12);
    rt.touch(4, TouchPhase::Ended, 180.0, 150.0, 0.13);
    rt.touch(3, TouchPhase::Ended, 100.0, 150.0, 0.14);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 3);
    assert_eq!(rt.state.magnifies[0].phase, GesturePhase::Began);
    assert_eq!(rt.state.magnifies[1].phase, GesturePhase::Moved);
    assert!(approx(rt.state.magnifies[1].delta, 80.0 / 84.0 - 1.0));
    assert_eq!(rt.state.magnifies[2].phase, GesturePhase::Ended);
}

#[test]
fn pinch_below_slop_lifting_emits_no_began_no_ended_and_no_tap() {
    let mut rt = chart_rig(ChartState::new());
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 200.0, 150.0, 0.01);
    // Move only 10 pt (< 16 pt SPAN_SLOP), then lift both fingers.
    rt.touch(2, TouchPhase::Moved, 210.0, 150.0, 0.02);
    rt.touch(2, TouchPhase::Ended, 210.0, 150.0, 0.03);
    assert!(rt.touching(), "PostPinch keeps touching() true while finger 1 remains down");
    rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.magnifies.is_empty(), "never crossed SPAN_SLOP -> no Began and no orphan Ended");
    assert_eq!(rt.state.taps, 0, "cancelling finger 1 on finger 2 down prevents any tap");
    assert!(!rt.touching());
}

#[test]
fn pinch_owner_locks_across_bounds_and_sibling_handlers_and_same_frame_queue() {
    let mut rt = chart_rig(ChartState::new());
    // 1. Multi-frame: begin on chart, drift midpoint completely onto sibling (x=320) and outside both (x=395).
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 160.0, 150.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 186.0, 150.0, 0.02); // span 60 -> 86 (> 16 pt slop), Began + Moved
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 2);
    assert!(rt.state.sibling_magnifies.is_empty());

    // Move both fingers over sibling (270..370): midpoint (320, 150), span 120.
    rt.touch(1, TouchPhase::Moved, 260.0, 150.0, 0.03);
    rt.touch(2, TouchPhase::Moved, 380.0, 150.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 4, "locked chart owner receives both moves over sibling");
    assert!(rt.state.sibling_magnifies.is_empty(), "sibling must not steal active pinch");
    // Midpoint (320, 150) in chart's local points (chart starts at x=50) -> x = 270.
    assert!(approx(rt.state.magnifies[3].x, 270.0), "local coords stay relative to Began owner: {:?}", rt.state.magnifies[3]);

    // End outside every element (midpoint 395, 395).
    rt.touch(1, TouchPhase::Moved, 390.0, 395.0, 0.05);
    rt.touch(2, TouchPhase::Ended, 400.0, 395.0, 0.06);
    rt.touch(1, TouchPhase::Ended, 390.0, 395.0, 0.07);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.last().map(|e| e.phase), Some(GesturePhase::Ended));
    assert!(rt.state.sibling_magnifies.is_empty());

    // 2. Same-frame queue: two back-to-back pinches (first on chart drifting onto sibling,
    //    second on sibling drifting off sibling) all queued before a single rt.frame().
    rt.state.magnifies.clear();
    rt.state.sibling_magnifies.clear();
    // Pinch A on chart
    rt.touch(10, TouchPhase::Started, 100.0, 150.0, 0.20);
    rt.touch(11, TouchPhase::Started, 160.0, 150.0, 0.21);
    rt.touch(11, TouchPhase::Moved, 186.0, 150.0, 0.22);
    rt.touch(10, TouchPhase::Moved, 280.0, 150.0, 0.23); // drifts over sibling
    rt.touch(11, TouchPhase::Moved, 360.0, 150.0, 0.24);
    rt.touch(11, TouchPhase::Ended, 360.0, 150.0, 0.25);
    rt.touch(10, TouchPhase::Ended, 280.0, 150.0, 0.26);
    // Pinch B on sibling
    rt.touch(12, TouchPhase::Started, 290.0, 150.0, 0.27);
    rt.touch(13, TouchPhase::Started, 340.0, 150.0, 0.28);
    rt.touch(13, TouchPhase::Moved, 370.0, 150.0, 0.29); // span 50 -> 80 (> 16 slop)
    rt.touch(12, TouchPhase::Moved, 10.0, 150.0, 0.30);  // drifts off sibling
    rt.touch(13, TouchPhase::Ended, 370.0, 150.0, 0.31);
    rt.touch(12, TouchPhase::Ended, 10.0, 150.0, 0.32);
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.magnifies.first().map(|e| e.phase), Some(GesturePhase::Began));
    assert_eq!(rt.state.magnifies.last().map(|e| e.phase), Some(GesturePhase::Ended));
    assert_eq!(rt.state.magnifies.len(), 5, "chart gets Began + 3 Moved + Ended");
    assert_eq!(rt.state.sibling_magnifies.first().map(|e| e.phase), Some(GesturePhase::Began));
    assert_eq!(rt.state.sibling_magnifies.last().map(|e| e.phase), Some(GesturePhase::Ended));
    assert_eq!(rt.state.sibling_magnifies.len(), 4, "sibling gets its own Began + 2 Moved + Ended");
}

#[test]
fn pinch_started_near_edge_still_locks_initial_target_when_midpoint_crosses_edge_before_slop() {
    let mut rt = chart_rig(ChartState::new());
    // Both fingers land inside chart [50..250] near its right edge: initial_mid = (244, 150).
    rt.touch(1, TouchPhase::Started, 240.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 248.0, 150.0, 0.01);
    // Pull finger 2 outward to x=270 (span 8 -> 30, diff 22 > 16). Live mid = (255, 150), outside chart!
    rt.touch(2, TouchPhase::Moved, 270.0, 150.0, 0.02);
    rt.touch(2, TouchPhase::Ended, 270.0, 150.0, 0.03);
    rt.touch(1, TouchPhase::Ended, 240.0, 150.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 3, "chart receives Began, Moved, Ended despite mid crossing edge during slop");
    assert_eq!(rt.state.magnifies[0].phase, GesturePhase::Began);
    assert!(approx(rt.state.magnifies[0].x, 205.0), "delivers live mid in chart local coords: {:?}", rt.state.magnifies[0]);
    assert!(rt.state.sibling_magnifies.is_empty());
}

#[test]
fn pinch_cancels_pending_press_hold_timer_active_drag_and_touch_pan_without_click_or_fling() {
    // 1. Pending press + armed DirectTimer long-press hold + active on_drag.
    let mut rt = chart_rig(ChartState::new());
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    assert!(rt.wake_deadline().is_some(), "pending single touch arms hold timer");
    assert!(rt.pressing());
    // Move past TOUCH_SLOP (8 pt) on chart (which has on_drag) -> active drag.
    rt.touch(1, TouchPhase::Moved, 120.0, 150.0, 0.02);
    // Second finger lands -> cancels drag cleanly before pinch.
    rt.touch(2, TouchPhase::Started, 200.0, 150.0, 0.03);
    assert_eq!(rt.wake_deadline(), None, "pinch clears hold wake deadline");
    assert!(!rt.pressing(), "pinch drops pointer capture");
    assert_eq!(rt.pointer(), None, "pinch leaves no hover");
    rt.touch(2, TouchPhase::Moved, 230.0, 150.0, 0.04);
    rt.touch(2, TouchPhase::Ended, 230.0, 150.0, 0.05);
    rt.touch(1, TouchPhase::Ended, 120.0, 150.0, 0.06);
    rt.frame.test_clock_mut().unwrap().seek(1.0);
    rt.frame(1.0);
    assert_eq!(rt.state.taps, 0, "no click");
    assert_eq!(rt.state.secondary, 0, "no long-press secondary click");
    assert_eq!(rt.state.drags.last(), Some(&(Phase::Up, true)), "active drag receives cancelled Phase::Up");
    assert_eq!(rt.state.magnifies.len(), 3, "pinch still ran cleanly");

    // 2. Armed hold timer (DirectTimer & ConfirmingTimer) cancelled before crossing slop.
    let mut rt2 = chart_rig(ChartState::new());
    rt2.core_mut().set_touch_hold_policy(TouchHoldPolicy::ConfirmingTimer);
    rt2.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt2.frame.test_clock_mut().unwrap().seek(0.6);
    rt2.frame(0.6);
    let Some(TouchHoldCommand::Confirm(attempt)) = rt2.core_mut().take_touch_hold_command() else {
        panic!("expected ConfirmHold");
    };
    // Second finger lands during Confirming window -> cancels confirmation!
    rt2.touch(2, TouchPhase::Started, 180.0, 150.0, 0.61);
    assert_eq!(rt2.core_mut().take_touch_hold_command(), Some(TouchHoldCommand::Cancel));
    let stale_fired = {
        let (core, app) = rt2.split_mut();
        core.confirm_touch_hold(&app, attempt)
    };
    assert!(!stale_fired, "stale ConfirmHold after pinch takes over must not fire");
    rt2.touch(2, TouchPhase::Ended, 180.0, 150.0, 0.62);
    rt2.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.63);
    rt2.frame(0.0);
    assert_eq!((rt2.state.taps, rt2.state.secondary), (0, 0));

    // 3. Active touch pan on scrollable list containing an on_magnify-only element (no on_drag):
    //    second finger landing stops pan with zero fling, and two-finger movement scrolls nothing.
    #[derive(Default)]
    struct ScrollChart { magnifies: Vec<MagnifyEvent>, taps: usize }
    fn scroll_chart_view(_: &ScrollChart, _: &mut Cx<ScrollChart>) -> El {
        col().key("list").size(400.0, 400.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().w_full().h(1200.0).child(
                el().key("chart").size(400.0, 400.0)
                    .on_press(|s: &mut ScrollChart| s.taps += 1)
                    .on_magnify(|s: &mut ScrollChart, ev, _| { s.magnifies.push(ev); true }),
            ),
        )
    }
    let mut rt3 = Runtime::new(ScrollChart::default(), scroll_chart_view, (400.0, 400.0), Env::new(true, 1.0));
    rt3.frame(0.0);
    let list_id = rt3.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    // Start a fast upward pan with finger 1.
    rt3.touch(1, TouchPhase::Started, 150.0, 300.0, 0.0);
    for i in 1..=5 {
        rt3.touch(1, TouchPhase::Moved, 150.0, 300.0 - 20.0 * i as f32, i as f64 * 0.01);
    }
    rt3.frame(1.0 / 60.0);
    let panned = rt3.scroll.scrolls[&list_id].offset;
    assert!(panned >= 90.0, "finger 1 was panning: {panned}");
    // Finger 2 lands mid-pan, then both fingers spread and translate vertically.
    rt3.touch(2, TouchPhase::Started, 250.0, 200.0, 0.055);
    rt3.touch(1, TouchPhase::Moved, 120.0, 150.0, 0.065);
    rt3.touch(2, TouchPhase::Moved, 280.0, 150.0, 0.075);
    rt3.touch(2, TouchPhase::Ended, 280.0, 150.0, 0.085);
    rt3.touch(1, TouchPhase::Ended, 120.0, 150.0, 0.095);
    for _ in 0..30 {
        rt3.frame(1.0 / 60.0);
    }
    assert_eq!(rt3.scroll.scrolls[&list_id].offset, panned, "pinch stops pan with zero fling and scrolls nothing");
    assert_eq!(rt3.state.taps, 0);
    assert_eq!(rt3.state.magnifies.first().map(|e| e.phase), Some(GesturePhase::Began));
    assert_eq!(rt3.state.magnifies.last().map(|e| e.phase), Some(GesturePhase::Ended));
}

#[test]
fn post_pinch_keeps_touching_emits_no_tap_or_second_ended_and_supports_ratcheting() {
    let mut rt = chart_rig(ChartState::new());
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 160.0, 150.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 200.0, 150.0, 0.02); // active pinch
    rt.touch(2, TouchPhase::Ended, 200.0, 150.0, 0.03);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 3);
    assert_eq!(rt.state.magnifies[2].phase, GesturePhase::Ended);
    assert!(rt.touching());
    assert_eq!(rt.pointer.touch.state(), TouchState::PostPinch(Contact { id: 1, pos: (100.0, 150.0) }));

    // Moving the remaining finger in PostPinch emits nothing (no drag, no pan, no hover).
    let drags_before = rt.state.drags.len();
    rt.touch(1, TouchPhase::Moved, 130.0, 150.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 3);
    assert_eq!(rt.state.drags.len(), drags_before);
    assert_eq!(rt.pointer(), None);
    assert_eq!(rt.pointer.touch.state(), TouchState::PostPinch(Contact { id: 1, pos: (130.0, 150.0) }));

    // Ratcheting: a new second finger lands while finger 1 is in PostPinch -> fresh pinch!
    rt.touch(3, TouchPhase::Started, 190.0, 150.0, 0.05); // initial_span = 60 from updated pos (130, 150)
    rt.touch(3, TouchPhase::Moved, 220.0, 150.0, 0.06);   // span 90 (diff 30 > 16 slop) -> Began + Moved
    rt.touch(3, TouchPhase::Ended, 220.0, 150.0, 0.07);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 6, "second pinch emits fresh Began + Moved + Ended");
    assert_eq!(rt.state.magnifies[3].phase, GesturePhase::Began);
    assert_eq!(rt.state.magnifies[4].phase, GesturePhase::Moved);
    assert!(approx(rt.state.magnifies[4].delta, 90.0 / 76.0 - 1.0));
    assert_eq!(rt.state.magnifies[5].phase, GesturePhase::Ended);
    assert!(rt.touching());

    // Finally lift finger 1 -> Idle, no tap, no extra Ended.
    rt.touch(1, TouchPhase::Ended, 130.0, 150.0, 0.08);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 6);
    assert_eq!(rt.state.taps, 0);
    assert!(!rt.touching());
    assert_eq!(rt.pointer.touch.state(), TouchState::Idle);
}

#[test]
fn third_finger_during_pinch_is_ignored_without_flicker_or_jump() {
    let mut rt = chart_rig(ChartState::new());
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 160.0, 150.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 200.0, 150.0, 0.02); // active pinch, span 100
    // 3rd finger lands, moves, and lifts/cancels mid-pinch: ignored completely.
    rt.touch(3, TouchPhase::Started, 150.0, 200.0, 0.03);
    rt.touch(3, TouchPhase::Moved, 180.0, 220.0, 0.04);
    rt.touch(3, TouchPhase::Ended, 180.0, 220.0, 0.05);
    rt.touch(4, TouchPhase::Started, 120.0, 120.0, 0.055);
    rt.touch(4, TouchPhase::Cancelled, 120.0, 120.0, 0.056);
    // Original finger 2 continues smoothly from span 100 -> 150.
    rt.touch(2, TouchPhase::Moved, 250.0, 150.0, 0.06);
    rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.07);
    rt.touch(2, TouchPhase::Ended, 250.0, 150.0, 0.08);
    rt.frame(1.0 / 60.0);

    let phases: Vec<_> = rt.state.magnifies.iter().map(|e| e.phase).collect();
    assert_eq!(phases, [GesturePhase::Began, GesturePhase::Moved, GesturePhase::Moved, GesturePhase::Ended]);
    assert!(approx(rt.state.magnifies[2].delta, 150.0 / 100.0 - 1.0));
}

#[test]
fn cancelled_finger_during_pinch_emits_ended_once_and_cleanly_exits() {
    let mut rt = chart_rig(ChartState::new());
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 160.0, 150.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 200.0, 150.0, 0.02); // mid (150, 150) -> local (100, 100)
    // System cancels finger 1 (even with bogus (0, 0) coords): Ended keeps last valid midpoint.
    rt.touch(1, TouchPhase::Cancelled, 0.0, 0.0, 0.03);
    assert!(rt.touching(), "finger 2 remains down in PostPinch");
    rt.touch(2, TouchPhase::Cancelled, 0.0, 0.0, 0.04);
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.magnifies.len(), 3);
    let ended = rt.state.magnifies[2];
    assert_eq!(ended.phase, GesturePhase::Ended);
    assert_eq!(ended.delta, 0.0);
    assert!(approx(ended.x, 100.0) && approx(ended.y, 100.0), "cancel keeps last valid midpoint: {ended:?}");
    assert_eq!(rt.state.taps, 0);
    assert!(!rt.touching());
}

#[test]
fn coincident_and_collapsing_fingers_clamp_at_min_span_without_nan_or_inf() {
    let mut rt = chart_rig(ChartState::new());
    // 1. Coincident landing (initial_span == 0.0) spreading to 20 pt (> 16 pt slop).
    rt.touch(1, TouchPhase::Started, 150.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 150.0, 150.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 150.0, 150.0, 0.02);
    rt.touch(2, TouchPhase::Moved, 170.0, 150.0, 0.03); // span 20, boundary max(16, MIN_SPAN)=16 -> delta 20/16 - 1 = +0.25
    // 2. Collapse all the way to coincident (span 0.0), stay collapsed, then spread back to 32 pt.
    rt.touch(2, TouchPhase::Moved, 150.0, 150.0, 0.04); // clamped to MIN_SPAN=16 -> 16/20 - 1 = -0.2
    rt.touch(2, TouchPhase::Moved, 154.0, 150.0, 0.05); // span 4 < MIN_SPAN -> clamped 16/16 - 1 = 0.0
    rt.touch(2, TouchPhase::Moved, 182.0, 150.0, 0.06); // span 32 -> 32/16 - 1 = +1.0
    rt.touch(2, TouchPhase::Ended, 182.0, 150.0, 0.07);
    rt.touch(1, TouchPhase::Ended, 150.0, 150.0, 0.08);
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.magnifies.len(), 6);
    for ev in &rt.state.magnifies {
        assert!(ev.delta.is_finite() && ev.x.is_finite() && ev.y.is_finite(), "non-finite event: {ev:?}");
        assert!(1.0 + ev.delta > 0.0, "scale factor must remain strictly positive: {ev:?}");
    }
    assert!(approx(rt.state.magnifies[1].delta, 20.0 / MIN_SPAN - 1.0));
    assert!(approx(rt.state.magnifies[2].delta, MIN_SPAN / 20.0 - 1.0));
    assert!(approx(rt.state.magnifies[3].delta, 0.0));
    assert!(approx(rt.state.magnifies[4].delta, 32.0 / MIN_SPAN - 1.0));
}

#[test]
fn no_magnify_handler_or_inert_or_behind_modal_ignores_second_finger() {
    // 1. Plain scrollable list area (y=300, outside chart/sibling): second finger does not interrupt pan or fling.
    let mut rt = chart_rig(ChartState::new());
    rt.touch(1, TouchPhase::Started, 100.0, 350.0, 0.0);
    for i in 1..=5 {
        rt.touch(1, TouchPhase::Moved, 100.0, 350.0 - 15.0 * i as f32, i as f64 * 0.01);
    }
    // Second finger lands over empty list area (midpoint y=287.5, below chart [50..250]).
    rt.touch(2, TouchPhase::Started, 200.0, 300.0, 0.055);
    rt.touch(2, TouchPhase::Moved, 260.0, 300.0, 0.06);
    rt.touch(2, TouchPhase::Ended, 260.0, 300.0, 0.065);
    assert!(rt.pointer.touch.single().is_some(), "second finger without on_magnify stays in Single");
    rt.touch(1, TouchPhase::Ended, 100.0, 275.0, 0.068);
    let lifted = list_offset(&rt);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(list_offset(&rt) > lifted + 20.0, "single-finger pan and fling stay uninterrupted");
    assert!(rt.state.magnifies.is_empty());

    // 2. Inert on_magnify ignores second finger.
    let mut rt_inert = chart_rig(ChartState { inert_chart: true, ..ChartState::new() });
    rt_inert.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt_inert.touch(2, TouchPhase::Started, 200.0, 150.0, 0.01);
    rt_inert.touch(2, TouchPhase::Moved, 240.0, 150.0, 0.02);
    rt_inert.touch(2, TouchPhase::Ended, 240.0, 150.0, 0.03);
    rt_inert.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.04);
    rt_inert.frame(1.0 / 60.0);
    assert!(rt_inert.state.magnifies.is_empty());

    // 3. on_magnify behind an open modal ignores second finger.
    let mut rt_modal = chart_rig(ChartState { modal_open: true, ..ChartState::new() });
    rt_modal.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt_modal.touch(2, TouchPhase::Started, 200.0, 150.0, 0.01);
    rt_modal.touch(2, TouchPhase::Moved, 240.0, 150.0, 0.02);
    rt_modal.touch(2, TouchPhase::Ended, 240.0, 150.0, 0.03);
    rt_modal.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.04);
    rt_modal.frame(1.0 / 60.0);
    assert!(rt_modal.state.magnifies.is_empty());
}

#[test]
fn window_dragging_and_long_press_menu_ignore_second_finger() {
    // 1. Mode::Menu ignores second finger even when midpoint is over on_magnify.
    let mut rt = chart_rig(ChartState::new());
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.frame.test_clock_mut().unwrap().seek(0.6);
    rt.frame(0.6);
    assert!(rt.core().touch_in_menu());
    assert_eq!(rt.state.secondary, 1);
    rt.touch(2, TouchPhase::Started, 200.0, 150.0, 0.61);
    rt.touch(2, TouchPhase::Moved, 240.0, 150.0, 0.62);
    rt.touch(2, TouchPhase::Ended, 240.0, 150.0, 0.63);
    assert!(rt.core().touch_in_menu(), "second finger must not leave Mode::Menu");
    rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.64);
    rt.frame(0.0);
    assert!(rt.state.magnifies.is_empty());

    // 2. Mode::WindowDragging ignores second finger over an ancestor on_magnify.
    #[derive(Default)]
    struct BarPinch { magnifies: Vec<MagnifyEvent> }
    fn bar_pinch_view(_: &BarPinch, _: &mut Cx<BarPinch>) -> El {
        row().size(300.0, 100.0).window_drag()
            .on_magnify(|s: &mut BarPinch, ev, _| { s.magnifies.push(ev); true })
    }
    let mut rt_bar = Runtime::new(BarPinch::default(), bar_pinch_view, (300.0, 100.0), Env::default());
    rt_bar.frame(0.0);
    rt_bar.touch(1, TouchPhase::Started, 100.0, 50.0, 0.0);
    rt_bar.touch(1, TouchPhase::Moved, 120.0, 50.0, 0.02); // crosses slop -> WindowDragging
    assert_eq!(rt_bar.take_titlebar_action(), Some(TitlebarAction::Drag));
    rt_bar.touch(2, TouchPhase::Started, 200.0, 50.0, 0.03);
    rt_bar.touch(2, TouchPhase::Moved, 250.0, 50.0, 0.04);
    rt_bar.touch(2, TouchPhase::Ended, 250.0, 50.0, 0.05);
    rt_bar.touch(1, TouchPhase::Ended, 120.0, 50.0, 0.06);
    rt_bar.frame(0.0);
    assert!(rt_bar.state.magnifies.is_empty(), "WindowDragging must not hand off to pinch");
}

#[test]
fn declining_unmounting_owner_live_only_and_cancel_pointer() {
    // 1. Nested bubbling: inner declines Began -> outer accepts and owns Moved/Ended even outside both.
    #[derive(Default)]
    struct Nested { inner: Vec<MagnifyEvent>, outer: Vec<MagnifyEvent>, outer_takes: bool }
    fn nested_view(_: &Nested, _: &mut Cx<Nested>) -> El {
        el().key("outer").size(200.0, 200.0)
            .on_magnify(|s: &mut Nested, ev, _| { s.outer.push(ev); s.outer_takes })
            .child(el().key("inner").size(100.0, 100.0)
                .on_magnify(|s: &mut Nested, ev, _| { s.inner.push(ev); false }))
    }
    let mut rt = Runtime::new(Nested { outer_takes: true, ..Nested::default() }, nested_view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.touch(1, TouchPhase::Started, 20.0, 50.0, 0.0);
    rt.touch(2, TouchPhase::Started, 70.0, 50.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 100.0, 50.0, 0.02); // Began + Moved (inner declines, outer locks)
    rt.touch(1, TouchPhase::Moved, 240.0, 240.0, 0.03); // outside both
    rt.touch(2, TouchPhase::Moved, 290.0, 240.0, 0.04);
    rt.touch(2, TouchPhase::Ended, 290.0, 240.0, 0.05);
    rt.touch(1, TouchPhase::Ended, 240.0, 240.0, 0.06);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.inner.len(), 1, "declining inner hears Began only");
    assert_eq!(rt.state.outer.len(), 5, "outer owns Began + 3 Moved + Ended");

    // 2. If all handlers decline Began, later Moved over sibling is ignored and Ended resets cleanly.
    let mut rt_dec = chart_rig(ChartState { accept_magnify: false, ..ChartState::new() });
    rt_dec.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt_dec.touch(2, TouchPhase::Started, 160.0, 150.0, 0.01);
    rt_dec.touch(2, TouchPhase::Moved, 190.0, 150.0, 0.02);
    rt_dec.frame(1.0 / 60.0);
    assert_eq!(rt_dec.state.magnifies.len(), 1, "only Began offered; Moved dropped after decline");
    rt_dec.touch(1, TouchPhase::Moved, 280.0, 150.0, 0.03);
    rt_dec.touch(2, TouchPhase::Moved, 360.0, 150.0, 0.04);
    rt_dec.touch(2, TouchPhase::Ended, 360.0, 150.0, 0.05);
    rt_dec.touch(1, TouchPhase::Ended, 280.0, 150.0, 0.06);
    rt_dec.frame(1.0 / 60.0);
    assert!(rt_dec.state.sibling_magnifies.is_empty(), "declined pinch must not steal sibling");

    // 3. Unmounting owner mid-pinch drops subsequent events cleanly without stealing sibling.
    let mut rt_unmount = chart_rig(ChartState::new());
    rt_unmount.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt_unmount.touch(2, TouchPhase::Started, 160.0, 150.0, 0.01);
    rt_unmount.touch(2, TouchPhase::Moved, 190.0, 150.0, 0.02);
    rt_unmount.frame(1.0 / 60.0);
    assert_eq!(rt_unmount.state.magnifies.len(), 2);
    rt_unmount.update(|s, _| s.show_chart = false);
    rt_unmount.frame(1.0 / 60.0);
    rt_unmount.touch(1, TouchPhase::Moved, 280.0, 150.0, 0.03);
    rt_unmount.touch(2, TouchPhase::Moved, 360.0, 150.0, 0.04);
    rt_unmount.touch(2, TouchPhase::Ended, 360.0, 150.0, 0.05);
    rt_unmount.touch(1, TouchPhase::Ended, 280.0, 150.0, 0.06);
    rt_unmount.frame(1.0 / 60.0);
    assert_eq!(rt_unmount.state.magnifies.len(), 2);
    assert!(rt_unmount.state.sibling_magnifies.is_empty());

    // 4. tx.live_only() skips view commits during active touchscreen pinch.
    let mut rt_live = chart_rig(ChartState { live_only: true, ..ChartState::new() });
    rt_live.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt_live.touch(2, TouchPhase::Started, 160.0, 150.0, 0.01);
    rt_live.frame(1.0 / 60.0);
    let commits_before = rt_live.stats.commits;
    rt_live.touch(2, TouchPhase::Moved, 200.0, 150.0, 0.02);
    rt_live.frame(1.0 / 60.0);
    rt_live.touch(2, TouchPhase::Moved, 240.0, 150.0, 0.03);
    rt_live.frame(1.0 / 60.0);
    assert_eq!(rt_live.stats.commits, commits_before, "live_only touchscreen pinch skips view commits");
    assert!(rt_live.state.live_zoom.get() > 1.5, "live_zoom updated: {}", rt_live.state.live_zoom.get());

    // 5. cancel_pointer() resets MagnifyOwner to Idle.
    rt_live.cancel_pointer();
    assert_eq!(rt_live.pointer.gesture_owners.magnify, capture::GestureOwner::Idle);
}

#[test]
fn trackpad_phased_pinch_locks_owner_while_ctrl_wheel_and_unphased_magnify_stay_unchanged() {
    let mut rt = chart_rig(ChartState::new());
    // 1. Phased trackpad PinchGesture (wheel: false) locks Began owner across bounds.
    rt.magnify_event(MagnifyEvent::new(0.0).at(100.0, 100.0).phase(GesturePhase::Began));
    rt.frame(1.0 / 60.0);
    rt.magnify_event(MagnifyEvent::new(0.25).at(320.0, 100.0).phase(GesturePhase::Moved)); // over sibling
    rt.magnify_event(MagnifyEvent::new(0.0).at(395.0, 395.0).phase(GesturePhase::Ended));  // outside all
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 3);
    assert!(rt.state.sibling_magnifies.is_empty(), "phased trackpad pinch stays locked to Began owner");
    assert!(approx(rt.state.magnifies[1].x, 270.0), "local x relative to chart: {:?}", rt.state.magnifies[1]);

    // 2. Phased trackpad Began over empty space (no handler) declines later Moved over chart without scheduling a frame.
    rt.state.magnifies.clear();
    rt.magnify_event(MagnifyEvent::new(0.0).at(10.0, 10.0).phase(GesturePhase::Began));
    assert!(!rt.has_pending_input(), "empty-space Began queues nothing");
    rt.magnify_event(MagnifyEvent::new(0.2).at(100.0, 100.0).phase(GesturePhase::Moved));
    assert!(!rt.has_pending_input(), "declined phased Moved queues nothing");
    rt.magnify_event(MagnifyEvent::new(0.0).at(100.0, 100.0).phase(GesturePhase::Ended));
    assert!(!rt.has_pending_input());
    assert_eq!(rt.pointer.gesture_owners.magnify, capture::GestureOwner::Idle);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.magnifies.is_empty());

    // 3. Ctrl + wheel (wheel: true) and unphased MagnifyEvent::new(...) hit-test fresh every time.
    assert!(rt.pinch_wheel(100.0, 100.0, 20.0, None));
    assert!(rt.pinch_wheel(320.0, 100.0, 20.0, None));
    rt.magnify_event(MagnifyEvent::new(0.1).at(100.0, 100.0));
    rt.magnify_event(MagnifyEvent::new(0.2).at(320.0, 100.0));
    rt.magnify_event(MagnifyEvent::new(0.3).at(10.0, 10.0)); // outside all -> ignored
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.magnifies.len(), 2);
    assert!(rt.state.magnifies[0].wheel && !rt.state.magnifies[1].wheel);
    assert_eq!(rt.state.sibling_magnifies.len(), 2);
    assert!(rt.state.sibling_magnifies[0].wheel && !rt.state.sibling_magnifies[1].wheel);
}

#[test]
fn pinch_channel_disabled_emits_nothing_on_move_or_end() {
    use super::touch_machine::{TwoFinger, TwoFingerTargets, step_touch};
    let tf = TwoFinger::new(
        Contact { id: 1, pos: (100.0, 100.0) },
        Contact { id: 2, pos: (200.0, 100.0) },
        TwoFingerTargets::default(),
    );
    assert_eq!(tf.pinch, PinchChannel::Disabled);
    let mut state = TouchState::TwoFinger(tf);
    assert!(step_touch(&mut state, TouchInput::Move { id: 2, pos: (260.0, 100.0), time: 0.02, slop: None }).is_empty());
    assert!(step_touch(&mut state, TouchInput::End { id: 2, pos: (260.0, 100.0), time: 0.03, cancelled: false }).is_empty());
    assert_eq!(state, TouchState::PostPinch(Contact { id: 1, pos: (100.0, 100.0) }));
}

#[test]
fn edge_drag_band_allows_pinch_while_pending_and_ignores_second_finger_once_active() {
    #[derive(Default)]
    struct EdgePinch {
        edge: Vec<(Phase, bool)>,
        magnifies: Vec<MagnifyEvent>,
        drags: Vec<(Phase, bool)>,
    }
    fn edge_pinch_view(_: &EdgePinch, _: &mut Cx<EdgePinch>) -> El {
        col()
            .key("root")
            .size(300.0, 300.0)
            .on_edge_drag(crate::SwipeEdge::Left, 20.0, |s: &mut EdgePinch, d: &mut Drag, _| {
                s.edge.push((d.phase, d.cancelled()));
            })
            .child(
                el().key("chart")
                    .size(300.0, 300.0)
                    .touch_drags_at_once()
                    .on_drag(|s: &mut EdgePinch, d: &mut Drag, _| s.drags.push((d.phase, d.cancelled())))
                    .on_magnify(|s: &mut EdgePinch, ev, _| {
                        s.magnifies.push(ev);
                        true
                    }),
            )
    }

    // 1. Finger 1 lands inside the 20 pt left-edge band (stays Pending with edge candidate);
    //    before crossing slop, finger 2 lands over the chart -> pinch takes over and drops edge candidate.
    let mut rt = Runtime::new(EdgePinch::default(), edge_pinch_view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.touch(1, TouchPhase::Started, 8.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 108.0, 150.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 148.0, 150.0, 0.02); // span 100 -> 140 (> 16 pt slop)
    rt.touch(2, TouchPhase::Ended, 148.0, 150.0, 0.03);
    rt.touch(1, TouchPhase::Ended, 8.0, 150.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edge.is_empty(), "unclaimed edge candidate must not fire when pinch takes over");
    assert_eq!(rt.state.magnifies.len(), 3, "pinch emits Began, Moved, Ended");
    assert_eq!(rt.state.drags.last(), Some(&(Phase::Up, true)), "child drag receives cancelled Up");
    assert!(!rt.touching());

    // 2. Finger 1 lands in the left-edge band and moves rightward past TOUCH_SLOP -> enters Mode::Edge.
    //    A second finger landing over the on_magnify chart while edge drag is active is ignored.
    rt.state.edge.clear();
    rt.state.magnifies.clear();
    rt.state.drags.clear();
    rt.touch(10, TouchPhase::Started, 8.0, 150.0, 0.10);
    rt.touch(10, TouchPhase::Moved, 30.0, 150.0, 0.11); // crosses slop rightward -> Mode::Edge
    rt.touch(11, TouchPhase::Started, 130.0, 150.0, 0.12);
    rt.touch(11, TouchPhase::Moved, 190.0, 150.0, 0.13);
    rt.touch(11, TouchPhase::Ended, 190.0, 150.0, 0.14);
    rt.touch(10, TouchPhase::Moved, 80.0, 150.0, 0.15);
    rt.touch(10, TouchPhase::Ended, 80.0, 150.0, 0.16);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.magnifies.is_empty(), "active edge drag ignores second finger");
    assert_eq!(rt.state.edge.first(), Some(&(Phase::Down, false)));
    assert_eq!(rt.state.edge.last(), Some(&(Phase::Up, false)));
    assert!(!rt.touching());
}
