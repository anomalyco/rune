use super::*;
use crate::{ScrollSpec, col, el};

#[derive(Default)]
struct S { taps: Vec<usize>, drags: Vec<f32> }

fn view(_: &S, _: &mut Cx<S>) -> El {
    col().size(200.0, 300.0)
        .child(el().key("knob").size(200.0, 40.0).on_drag(|s: &mut S, d: &mut Drag, _| s.drags.push(d.dx)))
        .child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false })
            .children((0..30).map(|i| el().key(format!("row{i}")).w_full().h(40.0).on_press(move |s: &mut S| s.taps.push(i)))))
}

fn rig() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (200.0, 300.0), Env::default());
    rt.frame(0.0);
    rt
}

fn offset(rt: &Runtime<S>) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    rt.scroll.scrolls[&id].offset
}

/// A finger from (x, y) moving by `dy` over `steps` events 10 ms apart, then lifted.
fn swipe(rt: &mut Runtime<S>, (x, y): (f32, f32), dy: f32, steps: usize) {
    rt.touch(1, TouchPhase::Started, x, y, 0.0);
    for i in 1..=steps {
        rt.touch(1, TouchPhase::Moved, x, y + dy * i as f32 / steps as f32, i as f64 * 0.01);
    }
    rt.touch(1, TouchPhase::Ended, x, y + dy, steps as f64 * 0.01 + 0.005);
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_tap_clicks_what_it_lands_on_and_leaves_no_hover() {
    let mut rt = rig();
    rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 103.0, 72.0, 0.02);
    rt.touch(1, TouchPhase::Ended, 103.0, 72.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.taps, [0]);
    assert_eq!(rt.pointer(), None);
    assert!(!rt.touching());
}

#[test]
fn a_drag_on_a_list_scrolls_it_without_clicking_and_flings_on() {
    let mut rt = rig();
    swipe(&mut rt, (100.0, 200.0), -100.0, 5);
    assert!(rt.state.taps.is_empty(), "a pan presses nothing");
    let lifted = offset(&rt);
    assert!(lifted >= 90.0, "the list followed the finger: {lifted}");
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(offset(&rt) > lifted + 50.0, "and coasted on: {} after {lifted}", offset(&rt));
}

#[test]
fn a_finger_held_still_before_lifting_throws_nothing() {
    let mut rt = rig();
    rt.touch(1, TouchPhase::Started, 100.0, 200.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 100.0, 150.0, 0.01);
    rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.5);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert_eq!(offset(&rt), 50.0);
}

#[test]
fn a_drag_on_an_element_that_drags_drags_it_not_the_page() {
    let mut rt = rig();
    rt.touch(1, TouchPhase::Started, 20.0, 20.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 60.0, 30.0, 0.01);
    rt.touch(1, TouchPhase::Ended, 60.0, 30.0, 0.02);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.drags.last(), Some(&40.0));
    assert_eq!(offset(&rt), 0.0);
}

#[test]
fn a_second_finger_is_ignored_and_a_cancelled_touch_clicks_nothing() {
    let mut rt = rig();
    rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
    rt.touch(2, TouchPhase::Started, 100.0, 110.0, 0.0);
    rt.touch(2, TouchPhase::Ended, 100.0, 110.0, 0.01);
    rt.touch(1, TouchPhase::Cancelled, 100.0, 70.0, 0.02);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.taps.is_empty());
    assert!(!rt.touching());
}

#[test]
fn a_finger_past_the_slop_picks_one_mode_with_only_its_own_data() {
    let mut rt = rig();
    let list = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    rt.touch(1, TouchPhase::Started, 100.0, 200.0, 0.0);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Pending { scroll: Some(list) }), "pending, it holds the viewport under it");
    rt.touch(1, TouchPhase::Moved, 100.0, 150.0, 0.01);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Pan { scroll: list, horizontal: false }));
    rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.02);
    rt.touch(2, TouchPhase::Started, 20.0, 20.0, 1.0);
    rt.touch(2, TouchPhase::Moved, 60.0, 20.0, 1.01);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Drag), "on an element that drags it holds no viewport");
}

#[derive(Default)]
struct BarState {
    button_taps: usize,
    no_drag_taps: usize,
    modal_open: bool,
    note: crate::TextEdit,
}

fn bar_view(s: &BarState, _: &mut Cx<BarState>) -> El {
    let titlebar = crate::row().key("titlebar").size(440.0, 44.0).window_drag()
        .child(el().key("btn").size(60.0, 44.0).on_press(|s: &mut BarState| s.button_taps += 1))
        .child(el().key("nodrag").size(60.0, 44.0).no_window_drag().on_press(|s: &mut BarState| s.no_drag_taps += 1))
        .child(crate::text_input(&s.note, crate::TextStyle::new(14.0, 400.0)).key("field").label("Field").size(80.0, 32.0).on_edit(|s: &mut BarState| &mut s.note));
    let modal = s.modal_open.then(|| {
        el().key("sheet").size(120.0, 80.0).modal(true, |s: &mut BarState| s.modal_open = false)
    });
    col().size(440.0, 200.0).child(titlebar).children(modal)
}

fn bar_rig() -> Runtime<BarState> {
    let mut rt = Runtime::new(BarState::default(), bar_view, (440.0, 200.0), Env::default());
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.double_click_interval = 0.3;
    rt.frame(0.0);
    rt
}

#[test]
fn a_tap_on_a_window_drag_region_within_slop_emits_no_drag_and_second_tap_double_clicks() {
    let mut rt = bar_rig();
    // First tap on bare titlebar (x=250, y=20): within TOUCH_SLOP (8 pt).
    rt.touch(1, TouchPhase::Started, 250.0, 20.0, 0.0);
    assert_eq!(rt.take_titlebar_action(), None, "touch down must hold Drag until movement passes TOUCH_SLOP");
    rt.touch(1, TouchPhase::Moved, 254.0, 23.0, 0.02);
    assert_eq!(rt.take_titlebar_action(), None, "movement within slop (5 pt <= 8 pt) must not drag");
    rt.touch(1, TouchPhase::Ended, 254.0, 23.0, 0.05);
    assert_eq!(rt.take_titlebar_action(), None);
    assert_eq!(rt.pointer(), None);
    assert!(!rt.touching());

    // Second tap within double-click interval and click slop emits DoubleClick once.
    rt.frame.test_clock_mut().unwrap().seek(0.15);
    rt.touch(2, TouchPhase::Started, 251.0, 20.0, 0.15);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::DoubleClick));
    rt.touch(2, TouchPhase::Moved, 270.0, 20.0, 0.17);
    assert_eq!(rt.take_titlebar_action(), None, "moving after DoubleClick must not also emit Drag");
    rt.touch(2, TouchPhase::Ended, 270.0, 20.0, 0.20);
    assert_eq!(rt.take_titlebar_action(), None);
    assert_eq!(rt.pointer(), None);
    assert!(!rt.touching());
}

#[test]
fn a_touch_drag_past_slop_on_a_window_drag_region_emits_drag_once_and_resets_cleanly() {
    let mut rt = bar_rig();
    rt.touch(1, TouchPhase::Started, 250.0, 20.0, 0.0);
    assert_eq!(rt.take_titlebar_action(), None);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::WindowDragPending));
    // 10 pt move crosses TOUCH_SLOP (8 pt) and emits Drag once.
    rt.touch(1, TouchPhase::Moved, 260.0, 20.0, 0.02);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::WindowDragging));
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag));
    // Further moves emit nothing.
    rt.touch(1, TouchPhase::Moved, 280.0, 30.0, 0.04);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(1, TouchPhase::Ended, 280.0, 30.0, 0.06);
    assert_eq!(rt.take_titlebar_action(), None);
    assert_eq!(rt.pointer(), None);
    assert!(!rt.touching());
    // A tap right after a completed drag must not pair with the drag as a DoubleClick.
    rt.frame.test_clock_mut().unwrap().seek(0.1);
    rt.touch(2, TouchPhase::Started, 250.0, 20.0, 0.1);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(2, TouchPhase::Ended, 250.0, 20.0, 0.12);
}

#[test]
fn a_cancelled_touch_on_a_window_drag_region_clears_state_and_double_tap_pair() {
    let mut rt = bar_rig();
    rt.touch(1, TouchPhase::Started, 250.0, 20.0, 0.0);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(1, TouchPhase::Cancelled, 250.0, 20.0, 0.02);
    assert_eq!(rt.take_titlebar_action(), None);
    assert_eq!(rt.pointer(), None);
    assert!(!rt.touching());
    // Cancelled touch must not pair with a subsequent tap.
    rt.frame.test_clock_mut().unwrap().seek(0.1);
    rt.touch(2, TouchPhase::Started, 250.0, 20.0, 0.1);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(2, TouchPhase::Moved, 265.0, 20.0, 0.12);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag));
    rt.touch(2, TouchPhase::Cancelled, 265.0, 20.0, 0.14);
    assert_eq!(rt.pointer(), None);
    assert!(!rt.touching());
}

#[test]
fn a_long_press_on_a_window_drag_region_opens_the_window_menu_and_never_drags() {
    let mut rt = Runtime::new(BarState::default(), bar_view, (440.0, 200.0), Env::default());
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.double_click_interval = 0.3;
    rt.core_mut().set_caption(Some(crate::titlebar::Caption::new(44.0)));
    rt.frame(0.0);
    rt.touch(1, TouchPhase::Started, 250.0, 20.0, 0.0);
    let target = rt.core().primary_target(250.0, 20.0).map(|i| rt.tree.nodes[i].path.clone());
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::WindowDragPending), "target={target:?}");
    assert_eq!(rt.pointer.touch.hold_deadline(), Some(LONG_PRESS));
    assert_eq!(rt.take_titlebar_action(), None);
    assert_eq!(rt.take_window_menu(), None);
    // Advance past LONG_PRESS (0.5 s) using the injected clock.
    rt.frame.test_clock_mut().unwrap().seek(0.55);
    rt.frame(0.55);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Menu), "clock={}", rt.core().clock);
    assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer));
    // Moving after the menu opened must not emit Drag.
    rt.touch(1, TouchPhase::Moved, 280.0, 20.0, 0.6);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(1, TouchPhase::Ended, 280.0, 20.0, 0.65);
    assert_eq!(rt.pointer(), None);
    assert!(!rt.touching());
}

#[test]
fn interactive_children_and_no_window_drag_inside_titlebar_stay_interactive() {
    let mut rt = bar_rig();
    // Button inside .window_drag().
    rt.touch(1, TouchPhase::Started, 30.0, 20.0, 0.0);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(1, TouchPhase::Ended, 30.0, 20.0, 0.02);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.button_taps, 1);
    assert_eq!(rt.take_titlebar_action(), None);

    // .no_window_drag() element inside .window_drag().
    rt.touch(2, TouchPhase::Started, 90.0, 20.0, 0.1);
    rt.touch(2, TouchPhase::Moved, 110.0, 20.0, 0.12);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(2, TouchPhase::Ended, 110.0, 20.0, 0.14);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.take_titlebar_action(), None);

    // Text input inside .window_drag().
    rt.touch(3, TouchPhase::Started, 150.0, 20.0, 0.2);
    rt.touch(3, TouchPhase::Ended, 150.0, 20.0, 0.22);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.note.focused, "tapping a text field inside .window_drag() focuses it");
    assert_eq!(rt.take_titlebar_action(), None);
}

#[test]
fn an_active_modal_blocks_touch_window_drag() {
    let mut rt = bar_rig();
    rt.update(|s, _| s.modal_open = true);
    rt.frame(1.0 / 60.0);
    rt.touch(1, TouchPhase::Started, 250.0, 20.0, 0.0);
    assert_eq!(rt.pointer.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Pending { scroll: None }));
    rt.touch(1, TouchPhase::Moved, 270.0, 20.0, 0.02);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(1, TouchPhase::Ended, 270.0, 20.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.modal_open, "touch outside modal still dismisses it");
}

#[test]
fn touch_on_window_resize_emits_resize_action_and_clears_cleanly() {
    use crate::ResizeEdge;
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().key("grip").size(100.0, 40.0).window_resize(ResizeEdge::SouthEast)
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.touch(1, TouchPhase::Started, 50.0, 20.0, 0.0);
    assert_eq!(rt.take_titlebar_action(), None);
    assert_eq!(rt.take_resize_action(), Some(ResizeEdge::SouthEast));
    assert_eq!(rt.wake_deadline(), None, "resize touch does not arm long-press hold timer");
    rt.touch(1, TouchPhase::Moved, 90.0, 40.0, 0.02);
    assert_eq!(rt.take_resize_action(), None);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.touch(1, TouchPhase::Ended, 90.0, 40.0, 0.04);
    assert!(!rt.touching());
}

#[derive(Default)]
struct HoldStateTest {
    taps: usize,
    secondary: usize,
}

fn hold_view(_: &HoldStateTest, _: &mut Cx<HoldStateTest>) -> El {
    col().size(200.0, 300.0)
        .child(el().key("btn").size(200.0, 60.0)
            .on_press(|s: &mut HoldStateTest| s.taps += 1)
            .on_secondary_press(|s: &mut HoldStateTest, _, _| s.secondary += 1))
        .child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false })
            .children((0..30).map(|i| el().key(format!("row{i}")).w_full().h(40.0)
                .on_press(|s: &mut HoldStateTest| s.taps += 1)
                .on_secondary_press(|s: &mut HoldStateTest, _, _| s.secondary += 1))))
}

fn confirming_rig() -> Runtime<HoldStateTest> {
    let mut rt = Runtime::new(HoldStateTest::default(), hold_view, (200.0, 300.0), Env::default());
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.core_mut().set_touch_hold_policy(TouchHoldPolicy::ConfirmingTimer);
    rt.frame(0.0);
    rt
}

#[test]
fn a_completed_tap_leaves_no_hold_wake_deadline() {
    let mut rt = rig();
    assert_eq!(rt.wake_deadline(), None, "idle runtime starts with no wake scheduled");
    rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
    assert_eq!(rt.wake_deadline(), Some(LONG_PRESS), "pending touch arms hold wake");
    rt.touch(1, TouchPhase::Ended, 100.0, 70.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.taps, [0]);
    assert_eq!(rt.wake_deadline(), None, "lifting a finger must not leave a hold wake scheduled on an idle page");

    // Panning past slop and cancelling a touch also leave no hold wake scheduled.
    rt.touch(2, TouchPhase::Started, 100.0, 200.0, 0.1);
    assert!(rt.wake_deadline().is_some());
    rt.touch(2, TouchPhase::Moved, 100.0, 150.0, 0.12);
    assert_eq!(rt.wake_deadline(), None, "crossing TOUCH_SLOP into pan clears hold wake");
    rt.touch(2, TouchPhase::Cancelled, 100.0, 150.0, 0.14);
    assert_eq!(rt.wake_deadline(), None, "cancelling a touch leaves no hold wake");
}

#[test]
fn confirming_timer_does_not_instant_fire_on_700ms_delayed_press_and_tap_wins() {
    let mut rt = confirming_rig();
    // Both press (event_time = 0.0) and release (event_time = 0.08) arrive 700 ms late.
    rt.frame.test_clock_mut().unwrap().seek(0.70);
    rt.touch(1, TouchPhase::Started, 100.0, 30.0, 0.0);
    // Chromium's kCompensateGestureTimeoutsForLongDelayedSequences=false: delay (0.70) >= LONG_PRESS
    // arms a full 0.50 s from arrival (1.20), never firing immediately.
    assert_eq!(rt.wake_deadline(), Some(0.70 + LONG_PRESS));
    rt.frame(0.0);
    assert_eq!(rt.core_mut().take_touch_hold_command(), None);
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 0));

    rt.touch(1, TouchPhase::Ended, 100.0, 30.0, 0.08);
    rt.frame(0.0);
    assert_eq!((rt.state.taps, rt.state.secondary), (1, 0), "80 ms tap delivered 700 ms late must tap");
    assert_eq!(rt.wake_deadline(), None, "no wake scheduled once finger lifts");
}

#[test]
fn confirming_timer_drains_queued_release_or_move_after_700ms_stall_before_firing() {
    let mut rt = confirming_rig();
    // 1. Short 80 ms tap stalled 700 ms on the main thread: rAF wakes at 0.70 before pointerup.
    rt.touch(1, TouchPhase::Started, 100.0, 30.0, 0.0);
    assert_eq!(rt.wake_deadline(), Some(LONG_PRESS));

    rt.frame.test_clock_mut().unwrap().seek(0.70);
    rt.frame(0.70);
    let confirm = rt.core_mut().take_touch_hold_command();
    let Some(TouchHoldCommand::Confirm(attempt)) = confirm else {
        panic!("expected ConfirmHold command, got {confirm:?}");
    };
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 0), "must not fire secondary press inside rAF");
    assert_eq!(rt.wake_deadline(), None, "Confirming state schedules no extra runtime wake");

    // An intermediate post-stall animation frame (raf1) runs before Chromium delivers pointerup:
    // Confirming must stay pending without re-emitting ConfirmHold or firing early.
    rt.frame.test_clock_mut().unwrap().seek(0.716);
    rt.frame(0.016);
    assert_eq!(rt.core_mut().take_touch_hold_command(), None);
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 0));

    // Waiting pointerup (event_time = 0.046) arrives after raf1 commits, cancelling raf2 + confirm.
    rt.touch(1, TouchPhase::Ended, 100.0, 30.0, 0.046);
    assert_eq!(rt.core_mut().take_touch_hold_command(), Some(TouchHoldCommand::Cancel));
    rt.frame(0.0);
    assert_eq!((rt.state.taps, rt.state.secondary), (1, 0), "draining release must commit a tap");
    // Even if a stale confirm arrived later, it does nothing.
    let fired = {
        let (core, app) = rt.split_mut();
        core.confirm_touch_hold(&app, attempt)
    };
    assert!(!fired);
    assert_eq!((rt.state.taps, rt.state.secondary), (1, 0));
    assert_eq!(rt.wake_deadline(), None);

    // 2. Queued touch_move past slop drains ahead of confirmation and scrolls instead of opening menu.
    rt.frame.test_clock_mut().unwrap().seek(1.0);
    rt.touch(2, TouchPhase::Started, 100.0, 200.0, 1.0);
    rt.frame.test_clock_mut().unwrap().seek(1.7);
    rt.frame(0.7);
    let Some(TouchHoldCommand::Confirm(move_attempt)) = rt.core_mut().take_touch_hold_command() else {
        panic!("expected ConfirmHold for stalled swipe");
    };
    rt.touch(2, TouchPhase::Moved, 100.0, 140.0, 1.03);
    assert_eq!(rt.core_mut().take_touch_hold_command(), Some(TouchHoldCommand::Cancel));
    let fired = {
        let (core, app) = rt.split_mut();
        core.confirm_touch_hold(&app, move_attempt)
    };
    assert!(!fired);
    rt.touch(2, TouchPhase::Ended, 100.0, 140.0, 1.06);
    rt.frame(0.0);
    assert_eq!(rt.state.secondary, 0, "stalled swipe must not trigger secondary press");
    let list = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    assert!(rt.scroll.scrolls[&list].offset >= 50.0, "stalled swipe must scroll the list");
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn confirming_timer_fires_on_matching_attempt_and_ignores_stale_attempt() {
    let mut rt = confirming_rig();
    rt.touch(1, TouchPhase::Started, 100.0, 30.0, 0.0);
    rt.frame.test_clock_mut().unwrap().seek(0.55);
    rt.frame(0.55);
    let Some(TouchHoldCommand::Confirm(attempt)) = rt.core_mut().take_touch_hold_command() else {
        panic!("expected ConfirmHold");
    };
    // Stale attempt does nothing.
    let stale = crate::machine::Attempt(attempt.0.wrapping_add(1));
    let fired_stale = {
        let (core, app) = rt.split_mut();
        core.confirm_touch_hold(&app, stale)
    };
    assert!(!fired_stale);
    assert_eq!(rt.state.secondary, 0);

    // Matching attempt fires on_secondary_press once and enters Mode::Menu.
    let fired = {
        let (core, app) = rt.split_mut();
        core.confirm_touch_hold(&app, attempt)
    };
    assert!(fired);
    rt.frame(0.0);
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 1));
    assert!(rt.core().touch_in_menu());
    assert_eq!(rt.wake_deadline(), None);

    // Duplicate confirm does nothing, and lifting the finger does not click.
    let fired_again = {
        let (core, app) = rt.split_mut();
        core.confirm_touch_hold(&app, attempt)
    };
    assert!(!fired_again);
    rt.touch(1, TouchPhase::Ended, 100.0, 30.0, 0.8);
    rt.frame(0.0);
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 1));
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn confirming_timer_compensates_positive_delay_below_long_press_only() {
    // 200 ms delayed press: compensates to 0.20 + (0.50 - 0.20) = 0.50.
    assert!((compensated_hold_due(0.20, 0.0, TouchHoldPolicy::ConfirmingTimer) - 0.50).abs() < 1e-9);
    // 700 ms delayed press (>= LONG_PRESS): arms full 0.50 from arrival (1.20).
    assert!((compensated_hold_due(0.70, 0.0, TouchHoldPolicy::ConfirmingTimer) - 1.20).abs() < 1e-9);
    // Malformed future timestamp (negative delay) or NaN: arms full 0.50 from arrival.
    assert!((compensated_hold_due(0.20, 0.80, TouchHoldPolicy::ConfirmingTimer) - 0.70).abs() < 1e-9);
    assert!((compensated_hold_due(0.20, f64::NAN, TouchHoldPolicy::ConfirmingTimer) - 0.70).abs() < 1e-9);
    // DirectTimer (native & headless default) always arms now + LONG_PRESS.
    assert!((compensated_hold_due(0.20, 0.0, TouchHoldPolicy::DirectTimer) - 0.70).abs() < 1e-9);
}

#[test]
fn pan_velocity_and_fling_hold_use_event_timestamps_when_release_arrives_late() {
    let mut rt = rig();
    // Swipe moves stamped at 0.00..0.05, release stamped at 0.08 (30 ms after last move) even if
    // the runtime clock jumped 700 ms before the release was delivered.
    rt.touch(1, TouchPhase::Started, 100.0, 200.0, 0.0);
    for i in 1..=5 {
        rt.touch(1, TouchPhase::Moved, 100.0, 200.0 - 20.0 * i as f32, i as f64 * 0.01);
    }
    rt.frame(0.70);
    rt.touch(1, TouchPhase::Ended, 100.0, 100.0, 0.08);
    let lifted = offset(&rt);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(offset(&rt) > lifted + 50.0, "event timestamps keep fling alive despite late delivery: {} after {lifted}", offset(&rt));
}

#[test]
fn web_touch_event_time_maps_timestamps_and_falls_back_on_malformed_values() {
    use crate::platform::web_math::web_touch_event_time;
    // Prompt press at perf=1000 ms (rt_now=0.50 s) -> 0.50 s.
    let t_down = web_touch_event_time(0.50, 1000.0, 1000.0);
    assert!((t_down - 0.50).abs() < 1e-9);
    // Release stamped 80 ms later (1080 ms) delivered 700 ms late (perf=1700 ms, rt_now=1.20 s)
    // keeps the exact 80 ms delta (0.58 s - 0.50 s).
    let t_up = web_touch_event_time(1.20, 1080.0, 1700.0);
    assert!((t_up - 0.58).abs() < 1e-9);
    assert!(((t_up - t_down) - 0.08).abs() < 1e-9);
    // Future timestamp is clamped to perf_now (no negative age).
    assert!((web_touch_event_time(1.20, 1900.0, 1700.0) - 1.20).abs() < 1e-9);
    // Malformed or non-positive timestamps fall back to rt_now (delivery time).
    assert!((web_touch_event_time(1.20, f64::NAN, 1700.0) - 1.20).abs() < 1e-9);
    assert!((web_touch_event_time(1.20, 0.0, 1700.0) - 1.20).abs() < 1e-9);
    assert!((web_touch_event_time(1.20, -10.0, 1700.0) - 1.20).abs() < 1e-9);
    assert!((web_touch_event_time(1.20, 1080.0, 0.0) - 1.20).abs() < 1e-9);
}

/// Pure simulation of the touch hold state machine across a sequence of clock ticks, an optional
/// browser `contextmenu` (`HostHold`) arrival, and a finger lift, returning `(fires, fire_time,
/// final_deadline)` so every platform-guess and timing combination is proven in a table test.
fn simulate_hold(
    live_contextmenu_guess: bool,
    host_event_at: Option<f64>,
    lift_at: f64,
) -> (usize, Option<f64>, Option<f64>) {
    use crate::platform::web_math::web_touch_hold_policy;
    let policy = web_touch_hold_policy(live_contextmenu_guess);
    let mut seq = Seq::default();
    let mut state = HoldState::Idle;
    let mut fires = 0usize;
    let mut fire_time = None;
    let mut pending_confirm: Option<(Attempt, f64)> = None;

    let _ = step_hold(&mut seq, &mut state, HoldEvent::Start { now: 0.0, event_time: 0.0, policy });

    // Collect ordered timestamps to visit up to `lift_at`.
    let mut t = 0.0;
    loop {
        let next_due = state.deadline().filter(|&d| d <= lift_at);
        let next_confirm = pending_confirm.map(|(_, at)| at).filter(|&at| at <= lift_at);
        let next_host = host_event_at.filter(|&at| at >= t && at <= lift_at);
        let next_t = [next_due, next_confirm, next_host].into_iter().flatten().min_by(f64::total_cmp);
        let Some(now) = next_t else { break };
        t = now;

        if host_event_at == Some(now) {
            for e in step_hold(&mut seq, &mut state, HoldEvent::HostHold) {
                match e {
                    HoldEffect::CancelConfirm => pending_confirm = None,
                    HoldEffect::Fire => { fires += 1; fire_time.get_or_insert(now); }
                    _ => {}
                }
            }
        }
        if let Some((attempt, at)) = pending_confirm && at == now {
            pending_confirm = None;
            for e in step_hold(&mut seq, &mut state, HoldEvent::Confirm { attempt }) {
                if e == HoldEffect::Fire { fires += 1; fire_time.get_or_insert(now); }
            }
        }
        if state.deadline().is_some_and(|d| d <= now) {
            for e in step_hold(&mut seq, &mut state, HoldEvent::Tick { now, policy }) {
                match e {
                    HoldEffect::ConfirmHold(attempt) => pending_confirm = Some((attempt, now + 0.01)),
                    HoldEffect::Fire => { fires += 1; fire_time.get_or_insert(now); }
                    _ => {}
                }
            }
        }
        // Prevent revisiting a host event already processed at `now`.
        if host_event_at == Some(now) {
            t = now + 1e-6;
        }
    }

    // Finger lifts at `lift_at`.
    let _ = step_hold(&mut seq, &mut state, HoldEvent::Cancel);
    // Any deferred post-lift contextmenu (e.g. Windows HandleGestureLongTap) must not fire.
    if host_event_at.is_some_and(|at| at > lift_at) {
        for e in step_hold(&mut seq, &mut state, HoldEvent::HostHold) {
            if e == HoldEffect::Fire { fires += 1; }
        }
    }
    (fires, fire_time, state.deadline())
}

#[test]
fn host_contextmenu_and_fallback_table_never_double_fires_and_survives_wrong_platform_guess() {
    // (live_guess, host_contextmenu_at, lift_at, expected_fires, expected_fire_time)
    let cases: &[(bool, Option<f64>, f64, usize, Option<f64>)] = &[
        // Android Short (500 ms), Linux desktop (650 ms), Android Medium (1100 ms), Android Long (1600 ms):
        // browser contextmenu fires once at the browser's own timing; fallback never double-fires.
        (true, Some(0.50), 2.50, 1, Some(0.50)),
        (true, Some(0.65), 2.50, 1, Some(0.65)),
        (true, Some(1.10), 2.50, 1, Some(1.10)),
        (true, Some(1.60), 2.50, 1, Some(1.60)),
        // Wrong platform guess (spoofed Android UA on Windows DevTools, embedded WebView, kiosk):
        // contextmenu never arrives while holding -> fallback confirming timer fires once at 1.71 s.
        (true, None, 2.50, 1, Some(HOST_HOLD_FALLBACK + 0.01)),
        // Wrong platform guess with deferred Windows contextmenu after lift -> fallback fired once;
        // post-lift contextmenu does not double-fire.
        (true, Some(2.60), 2.50, 1, Some(HOST_HOLD_FALLBACK + 0.01)),
        // Host contextmenu arrives during fallback Confirming window (1.70..1.71): cancels confirm
        // and fires once.
        (true, Some(HOST_HOLD_FALLBACK + 0.005), 2.50, 1, Some(HOST_HOLD_FALLBACK + 0.005)),
        // Host contextmenu arrives after fallback already fired -> still fires exactly once.
        (true, Some(2.00), 2.50, 1, Some(HOST_HOLD_FALLBACK + 0.01)),
        // Short tap (80 ms) or 700 ms stall on Android without contextmenu -> zero holds fired.
        (true, None, 0.08, 0, None),
        (true, None, 0.70, 0, None),
        // Non-live-hold platform (iOS Safari, Windows Chrome, macOS): 500 ms confirming timer fires once.
        (false, None, 1.00, 1, Some(LONG_PRESS + 0.01)),
        // Misclassified browser that DOES send live contextmenu early (0.45 s) -> fires once, never twice.
        (false, Some(0.45), 1.00, 1, Some(0.45)),
        // Misclassified browser whose contextmenu arrives after 500 ms timer already fired -> still once.
        (false, Some(0.65), 1.00, 1, Some(LONG_PRESS + 0.01)),
    ];
    for &(guess, host_at, lift_at, want_fires, want_time) in cases {
        let (fires, fire_time, deadline) = simulate_hold(guess, host_at, lift_at);
        assert_eq!(fires, want_fires, "guess={guess}, host_at={host_at:?}, lift_at={lift_at}");
        match (fire_time, want_time) {
            (Some(got), Some(want)) => assert!((got - want).abs() < 1e-6, "got={got}, want={want} for guess={guess}, host_at={host_at:?}"),
            (a, b) => assert_eq!(a, b, "guess={guess}, host_at={host_at:?}, lift_at={lift_at}"),
        }
        assert_eq!(deadline, None, "lifted finger must leave no wake scheduled");
    }
}

#[test]
fn host_touch_hold_fires_once_cancels_confirm_and_ignores_stale_or_duplicate_events() {
    let mut rt = Runtime::new(HoldStateTest::default(), hold_view, (200.0, 300.0), Env::default());
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.core_mut().set_touch_hold_policy(TouchHoldPolicy::HostContextMenu);
    rt.frame(0.0);

    // 1. Android/Linux live contextmenu at 0.65 s fires on_secondary_press once and clears fallback wake.
    rt.touch(1, TouchPhase::Started, 100.0, 30.0, 0.0);
    assert_eq!(rt.wake_deadline(), Some(HOST_HOLD_FALLBACK));
    rt.frame.test_clock_mut().unwrap().seek(0.65);
    rt.frame(0.65);
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 0));
    let fired = {
        let (core, app) = rt.split_mut();
        core.host_touch_hold(&app, 1)
    };
    assert!(fired);
    rt.frame(0.0);
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 1));
    assert!(rt.core().touch_in_menu());
    assert_eq!(rt.wake_deadline(), None, "host hold clears fallback wake");

    // Advancing past HOST_HOLD_FALLBACK or receiving a duplicate host hold must never double-fire.
    rt.frame.test_clock_mut().unwrap().seek(2.0);
    rt.frame(1.35);
    assert_eq!(rt.core_mut().take_touch_hold_command(), None);
    let fired_again = {
        let (core, app) = rt.split_mut();
        core.host_touch_hold(&app, 1)
    };
    assert!(!fired_again);
    rt.touch(1, TouchPhase::Ended, 100.0, 30.0, 2.0);
    rt.frame(0.0);
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 1));

    // 2. Wrong platform guess: no contextmenu arrives -> fallback enters Confirming at 1.7 s and fires once.
    rt.frame.test_clock_mut().unwrap().seek(3.0);
    rt.touch(2, TouchPhase::Started, 100.0, 30.0, 3.0);
    assert_eq!(rt.wake_deadline(), Some(3.0 + HOST_HOLD_FALLBACK));
    rt.frame.test_clock_mut().unwrap().seek(3.0 + HOST_HOLD_FALLBACK);
    rt.frame(HOST_HOLD_FALLBACK);
    let Some(TouchHoldCommand::Confirm(attempt)) = rt.core_mut().take_touch_hold_command() else {
        panic!("expected fallback ConfirmHold");
    };
    // If contextmenu arrives right during Confirming, it cancels confirm and fires once.
    let fired_mid = {
        let (core, app) = rt.split_mut();
        core.host_touch_hold(&app, 2)
    };
    assert!(fired_mid);
    assert_eq!(rt.core_mut().take_touch_hold_command(), Some(TouchHoldCommand::Cancel));
    let stale_confirm = {
        let (core, app) = rt.split_mut();
        core.confirm_touch_hold(&app, attempt)
    };
    assert!(!stale_confirm);
    rt.frame(0.0);
    assert_eq!((rt.state.taps, rt.state.secondary), (0, 2));
    rt.touch(2, TouchPhase::Ended, 100.0, 30.0, 5.0);
    rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), None);
}
