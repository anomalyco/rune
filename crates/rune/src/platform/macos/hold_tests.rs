use super::*;

const DOWN: Event = Event::Seen { click: Click { down: true, at: (20.0, 20.0) }, over: true };
const UP: Event = Event::Seen { click: Click { down: false, at: (100.0, 100.0) }, over: true };

fn sample(button_down: bool, now: Instant, at: Point) -> Event { Event::Sampled { button_down, now, at } }

fn effects(hold: &mut Hold, event: Event) -> Vec<Effect> { step(hold, event).into_iter().collect() }

#[test]
fn a_lost_release_while_dragging_gives_the_press_up_once_where_the_button_rose() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, Event::Dispatched { down: true });
    assert!(step(&mut hold, sample(true, now, (40.0, 40.0))).is_empty(), "the drag is under way");
    assert!(matches!(hold, Hold::Pressed { .. }));
    step(&mut hold, sample(false, now, (50.0, 50.0)));
    assert!(matches!(hold, Hold::Releasing { .. }));
    assert_eq!(hold.deadline(), Some(now + UNSEEN_UP_GRACE));
    assert!(step(&mut hold, sample(false, now + Duration::from_millis(99), (60.0, 60.0))).is_empty());
    assert!(hold.captured());
    assert_eq!(effects(&mut hold, sample(false, now + UNSEEN_UP_GRACE, (70.0, 70.0))), [Effect::ReleaseLost((50.0, 50.0))]);
    assert_eq!(hold, Hold::default());
    assert_eq!(hold.deadline(), None);
    assert!(step(&mut hold, sample(false, now + Duration::from_secs(1), (80.0, 80.0))).is_empty(), "lost once");
}

#[test]
fn a_quick_press_queued_before_the_release_reaches_the_runtime_stays_held() {
    let mut hold = Hold::default();
    step(&mut hold, DOWN);
    step(&mut hold, UP);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::Deliver((20.0, 20.0))]);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: false }), [Effect::Deliver((100.0, 100.0))]);
    step(&mut hold, DOWN); // the next native press, before the host acknowledges the release
    step(&mut hold, Event::Released);
    assert!(hold.captured(), "the queued press owns the pointer");
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::Deliver((20.0, 20.0))]);
    step(&mut hold, UP);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: false }), [Effect::Deliver((100.0, 100.0))]);
    step(&mut hold, Event::Released);
    assert_eq!(hold, Hold::default());
}

#[test]
fn a_press_during_the_grace_cancels_the_lost_release() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, sample(false, now, (50.0, 50.0)));
    step(&mut hold, DOWN);
    assert!(matches!(hold, Hold::Pressed { .. }));
    assert!(step(&mut hold, sample(true, now + Duration::from_secs(1), (50.0, 50.0))).is_empty());
    step(&mut hold, sample(false, now + Duration::from_secs(1), (55.0, 55.0)));
    assert!(step(&mut hold, sample(true, now + Duration::from_secs(1), (55.0, 55.0))).is_empty());
    assert!(matches!(hold, Hold::Pressed { .. }), "the button came back down");
}

#[test]
fn a_press_after_a_lost_release_starts_a_new_hold() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, sample(false, now, (50.0, 50.0)));
    assert_eq!(effects(&mut hold, sample(false, now + UNSEEN_UP_GRACE, (50.0, 50.0))), [Effect::ReleaseLost((50.0, 50.0))]);
    step(&mut hold, DOWN);
    assert!(hold.captured());
    assert!(step(&mut hold, sample(true, now + Duration::from_secs(1), (50.0, 50.0))).is_empty(), "the old release is spent");
}

#[test]
fn a_queued_up_gets_the_longer_grace() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, UP);
    step(&mut hold, sample(false, now, (20.0, 20.0)));
    assert_eq!(hold.deadline(), Some(now + QUEUED_UP_GRACE));
    assert!(step(&mut hold, sample(false, now + Duration::from_millis(1999), (20.0, 20.0))).is_empty());
    assert_eq!(effects(&mut hold, sample(false, now + QUEUED_UP_GRACE, (30.0, 30.0))), [Effect::ReleaseLost((20.0, 20.0))]);
}

#[test]
fn an_up_with_no_seen_press_stays_free_and_still_dispatches() {
    let mut hold = Hold::default();
    step(&mut hold, UP);
    assert!(!hold.captured());
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), []);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: false }), [Effect::Deliver((100.0, 100.0))]);
}

#[test]
fn losing_the_view_gives_up_where_the_pointer_was_and_hiding_stays_silent() {
    let mut hold = Hold::default();
    step(&mut hold, DOWN);
    assert_eq!(effects(&mut hold, Event::ViewLost { last: Some((25.0, 15.0)) }), [Effect::ReleaseLost((25.0, 15.0))]);
    assert_eq!(hold, Hold::default());
    step(&mut hold, DOWN);
    assert!(step(&mut hold, Event::Stopped).is_empty());
    assert_eq!(hold, Hold::default());
}
