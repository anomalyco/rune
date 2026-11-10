use super::*;
use std::collections::VecDeque;

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
    pointer.step(Event::Seen { click: Click { down: true, at: (0.0, 0.0) }, over: true });
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
    dispatch(&pointer, true);
    dispatch(&pointer, false);
    pointer.record_click(true, 20.0, 20.0); // next native Down queued
    pointer.released();
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
    pointer.last.set(Some((25.0, 15.0)));
    pointer.record_click(true, 20.0, 20.0);
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
    assert_eq!(queued(&pointer), VecDeque::from([Click { down: true, at: (20.0, 20.0) }, Click { down: false, at: (100.0, 100.0) }]));
    // Physical Up must not turn off native hit testing before winit dispatches it.
    assert!(pointer.hit(100.0, 100.0));
    pointer.step(Event::Stopped);
    assert!(!pointer.hit(100.0, 100.0));
}
