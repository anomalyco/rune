use super::*;
use std::collections::VecDeque;
use super::super::hold::Click;

/// winit dispatches the click at the front of the queue.
fn dispatch(pointer: &PointerState, down: bool) -> Option<Point> { pointer.step(Event::Dispatched { down }) }
fn queued(pointer: &PointerState) -> VecDeque<Click> { pointer.hold.borrow().clone().into_clicks() }

#[test]
fn overlay_handlerless_hit_releases_native_ownership_in_every_input_mode() {
    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
        crate::el().child(crate::el().absolute(10.0, 10.0).size(20.0, 20.0).hit())
    }
    for mode in [OverlayInput::Regions, OverlayInput::None] {
        let mut rt = crate::Runtime::new((), view, (100.0, 100.0), crate::Env::default());
        rt.native_overlay = true;
        rt.frame(0.0);
        let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
        pointer.regions.borrow_mut().extend_from_slice(rt.hit_regions());
        pointer.record_click(true, 20.0, 20.0);
        dispatch(&pointer, true);
        rt.pointer_down(20.0, 20.0);
        assert!(!rt.pointer_captured(), "a handlerless hit owns only the native press");
        assert!(pointer.captured());
        pointer.record_click(false, 50.0, 50.0);
        pointer.input.set(mode);
        dispatch(&pointer, false);
        pointer.released(); // acknowledged even when Runtime input is disabled
        assert!(!pointer.captured(), "native release cannot depend on Runtime capture or mode");
        assert!(!pointer.hit(50.0, 50.0));
    }
}

#[test]
fn a_click_through_overlay_that_follows_nothing_never_samples_the_pointer() {
    let pointer = PointerState { input: Cell::new(OverlayInput::None), ..Default::default() };
    assert!(!pointer.wanted());
    pointer.follow.set(true);
    assert!(pointer.wanted(), "following the pointer reads it");
    pointer.follow.set(false);
    pointer.step(Event::Seen { down: true, at: (0.0, 0.0), over: true });
    assert!(pointer.wanted(), "a press it holds still needs its release");
    pointer.step(Event::Stopped);
    for input in [OverlayInput::Regions, OverlayInput::All] {
        pointer.input.set(input);
        assert!(pointer.wanted());
    }
}

#[test]
fn overlay_lost_up_recovers_once_after_grace_using_release_position() {
    let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
    pointer.regions.borrow_mut().push([10.0, 10.0, 30.0, 30.0]);
    pointer.record_click(true, 20.0, 20.0);
    let now = Instant::now();
    pointer.observe_release(1, now, (40.0, 40.0));
    pointer.observe_release(0, now, (50.0, 50.0));
    pointer.observe_release(0, now + Duration::from_millis(99), (60.0, 60.0));
    assert!(pointer.captured());
    pointer.observe_release(0, now + Duration::from_millis(100), (70.0, 70.0));
    assert!(!pointer.captured());
    assert!(queued(&pointer).is_empty());
    assert_eq!(pointer.recovered_release.take(), Some((50.0, 50.0)));
    pointer.observe_release(0, now + Duration::from_secs(1), (80.0, 80.0));
    assert_eq!(pointer.recovered_release.take(), None);
}
#[test]
fn overlay_queued_up_and_next_down_keep_their_ownership_until_dispatched() {
    let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
    pointer.regions.borrow_mut().push([10.0, 10.0, 30.0, 30.0]);
    pointer.record_click(true, 20.0, 20.0);
    pointer.record_click(false, 100.0, 100.0);
    pointer.observe_release(0, Instant::now(), (100.0, 100.0));
    pointer.observe_release(0, Instant::now() + Duration::from_secs(1), (100.0, 100.0));
    assert!(pointer.captured());
    assert_eq!(pointer.recovered_release.get(), None);
    pointer.record_click(true, 20.0, 20.0); // next native Down queued behind the Up
    dispatch(&pointer, true);
    dispatch(&pointer, false);
    pointer.released();
    assert_eq!(pointer.recovered_release.get(), None);
    assert!(pointer.captured());
    dispatch(&pointer, true);
    pointer.released();
    assert!(!pointer.captured());
}
#[test]
fn overlay_undelivered_click_releases_capture_after_stall() {
    // The local monitor saw a click that never reached winit, so neither entry dispatches.
    let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
    pointer.regions.borrow_mut().push([10.0, 10.0, 30.0, 30.0]);
    pointer.record_click(true, 20.0, 20.0);
    pointer.record_click(false, 20.0, 20.0);
    let now = Instant::now();
    pointer.observe_release(0, now, (20.0, 20.0));
    pointer.observe_release(0, now + Duration::from_millis(1999), (20.0, 20.0));
    assert!(pointer.captured(), "a briefly queued Up still belongs to winit");
    pointer.observe_release(0, now + Duration::from_secs(2), (20.0, 20.0));
    assert!(!pointer.captured());
    assert!(queued(&pointer).is_empty());
    assert!(!pointer.hit(50.0, 50.0), "outside the regions passes through again");
}
#[test]
fn overlay_lost_view_releases_capture_where_the_pointer_was() {
    let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
    pointer.regions.borrow_mut().push([10.0, 10.0, 30.0, 30.0]);
    pointer.record_click(true, 20.0, 20.0);
    pointer.read(1, Instant::now(), (25.0, 15.0), true, false);
    pointer.view_lost();
    assert!(!pointer.captured());
    assert!(queued(&pointer).is_empty());
    assert_eq!(pointer.recovered_release.take(), Some((25.0, 15.0)), "the runtime's press is released");
}
#[test]
fn overlay_rejected_down_cannot_poison_later_click_and_owned_up_survives_exit() {
    let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
    pointer.regions.borrow_mut().push([10.0, 10.0, 30.0, 30.0]);
    pointer.record_click(true, 1.0, 1.0);
    pointer.record_click(true, 20.0, 20.0);
    pointer.record_click(false, 100.0, 100.0);
    assert_eq!(queued(&pointer), VecDeque::from([Click { down: true, at: (20.0, 20.0), ends: None }, Click { down: false, at: (100.0, 100.0), ends: None }]));
    // Physical Up must not turn off native hit testing before winit dispatches it.
    assert!(pointer.hit(100.0, 100.0));
    pointer.step(Event::Stopped);
    assert!(!pointer.hit(100.0, 100.0));
}
#[test]
fn a_covered_overlay_rests_its_poll_but_keeps_a_lost_releases_deadline() {
    let pointer = PointerState { input: Cell::new(OverlayInput::All), ..Default::default() };
    let now = Instant::now();
    let poll = Some(now + Duration::from_millis(8));
    assert_eq!(pointer.deadline(poll, false), poll);
    assert_eq!(pointer.deadline(poll, true), None, "covered, the poll rests");
    pointer.record_click(true, 20.0, 20.0);
    dispatch(&pointer, true);
    assert_eq!(pointer.deadline(poll, true), None, "a press with the button down waits for nothing");
    pointer.observe_release(0, now, (20.0, 20.0));
    let due = now + Duration::from_millis(100);
    assert_eq!(pointer.deadline(poll, true), Some(due), "covered, the grace still wakes the loop");
    assert_eq!(pointer.deadline(poll, false), poll, "the sooner of the two");
    pointer.observe_release(0, due, (20.0, 20.0));
    assert_eq!(pointer.deadline(poll, true), None, "once: it does not restart the poll");
}
#[test]
fn a_queued_press_hands_over_the_release_lost_before_it() {
    let pointer = PointerState { input: Cell::new(OverlayInput::All), ..Default::default() };
    pointer.record_click(true, 20.0, 20.0);
    pointer.observe_release(0, Instant::now(), (50.0, 50.0));
    pointer.record_click(true, 30.0, 30.0);
    assert_eq!(pointer.take_edge(true), (None, Some((20.0, 20.0))));
    assert_eq!(pointer.take_edge(true), (Some((50.0, 50.0)), Some((30.0, 30.0))), "landing before the second press");
}

#[test]
fn a_sample_beating_the_next_press_cancels_the_old_drag_where_the_button_rose() {
    type State = Vec<(crate::Phase, bool, f32)>;
    fn view(_: &State, _: &mut crate::Cx<State>) -> crate::El {
        crate::el().size(100.0, 100.0).on_drag(|s: &mut State, d, _| s.push((d.phase, d.cancelled(), d.x)))
    }
    let mut rt = crate::Runtime::new(Vec::new(), view, (100.0, 100.0), crate::Env::default());
    rt.frame(0.0);
    let pointer = PointerState { input: Cell::new(OverlayInput::All), ..Default::default() };
    let now = Instant::now();
    pointer.record_click(true, 10.0, 10.0);
    pointer.take_edge(true);
    rt.pointer_down(10.0, 10.0);
    pointer.observe_release(0, now, (50.0, 50.0));
    pointer.observe_release(1, now, (70.0, 70.0));
    pointer.record_click(true, 70.0, 70.0);
    let (lost, _) = pointer.take_edge(true);
    let (x, y) = lost.expect("the old press lost its release");
    rt.pointer_release_lost(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.last(), Some(&(crate::Phase::Up, true, 50.0)));
}

#[test]
fn a_press_lost_before_any_sample_cancels_at_its_own_down_and_keeps_the_run() {
    type State = Vec<(crate::Phase, bool, f32, u32)>;
    fn view(_: &State, _: &mut crate::Cx<State>) -> crate::El {
        crate::el().size(100.0, 100.0).on_drag(|s: &mut State, d, _| s.push((d.phase, d.cancelled(), d.x, d.clicks)))
    }
    let mut rt = crate::Runtime::new(Vec::new(), view, (100.0, 100.0), crate::Env::default());
    rt.frame(0.0);
    let pointer = PointerState { input: Cell::new(OverlayInput::All), ..Default::default() };
    pointer.read(0, Instant::now(), (5.0, 5.0), true, false); // a hover older than the press
    pointer.record_click(true, 50.0, 10.0);
    let (_, down) = pointer.take_edge(true);
    let (x, y) = down.unwrap();
    rt.pointer_down(x, y);
    pointer.record_click(true, 51.0, 10.0);
    let (lost, down) = pointer.take_edge(true);
    let (x, y) = lost.expect("the first press lost its release");
    rt.pointer_release_lost(x, y);
    let (x, y) = down.unwrap();
    rt.pointer_down(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state, [(crate::Phase::Down, false, 50.0, 1), (crate::Phase::Up, true, 50.0, 1), (crate::Phase::Down, false, 51.0, 2)]);
}
