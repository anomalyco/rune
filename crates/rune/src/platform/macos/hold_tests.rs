use super::*;

const DOWN: Event = Event::Seen { down: true, at: (20.0, 20.0), over: true };
const UP: Event = Event::Seen { down: false, at: (100.0, 100.0), over: true };

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
    assert!(step(&mut hold, DOWN).is_empty(), "an Up is queued ahead of it, so nothing was lost");
    assert_eq!(effects(&mut hold, Event::Dispatched { down: false }), [Effect::Deliver((100.0, 100.0))]);
    step(&mut hold, Event::Released);
    assert!(hold.captured(), "the queued press owns the pointer");
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::Deliver((20.0, 20.0))]);
    step(&mut hold, UP);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: false }), [Effect::Deliver((100.0, 100.0))]);
    step(&mut hold, Event::Released);
    assert_eq!(hold, Hold::default());
}

#[test]
fn two_queued_presses_with_no_up_between_end_the_first_between_them() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, sample(false, now, (50.0, 50.0)));
    assert!(step(&mut hold, DOWN).is_empty());
    assert!(matches!(hold, Hold::Pressed { .. }));
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::Deliver((20.0, 20.0))]);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::ReleaseLost((50.0, 50.0)), Effect::Deliver((20.0, 20.0))]);
    step(&mut hold, UP);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: false }), [Effect::Deliver((100.0, 100.0))], "the second press ends normally");
    step(&mut hold, Event::Released);
    assert_eq!(hold, Hold::default());
}

#[test]
fn a_press_that_beats_the_grace_ends_the_dispatched_press_where_the_button_rose() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, Event::Dispatched { down: true });
    step(&mut hold, sample(false, now, (50.0, 50.0)));
    assert!(step(&mut hold, DOWN).is_empty());
    assert!(matches!(hold, Hold::Pressed { .. }), "the new press holds");
    assert_eq!(hold.deadline(), None);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::ReleaseLost((50.0, 50.0)), Effect::Deliver((20.0, 20.0))]);
}

#[test]
fn a_sample_that_beats_the_next_press_keeps_the_release_point() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, Event::Dispatched { down: true });
    step(&mut hold, sample(false, now, (50.0, 50.0)));
    assert!(step(&mut hold, sample(true, now, (55.0, 55.0))).is_empty()); // the sample beat the Down's event
    assert_eq!(hold.deadline(), None, "the grace rests while the button reads down");
    step(&mut hold, DOWN);
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::ReleaseLost((50.0, 50.0)), Effect::Deliver((20.0, 20.0))]);
}

#[test]
fn a_rebounding_button_gives_up_where_it_first_rose_after_a_fresh_grace() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, Event::Dispatched { down: true });
    step(&mut hold, sample(false, now, (50.0, 50.0)));
    step(&mut hold, sample(true, now, (55.0, 55.0)));
    let later = now + Duration::from_secs(1);
    step(&mut hold, sample(false, later, (60.0, 60.0)));
    assert_eq!(hold.deadline(), Some(later + UNSEEN_UP_GRACE));
    assert_eq!(effects(&mut hold, sample(false, later + UNSEEN_UP_GRACE, (70.0, 70.0))), [Effect::ReleaseLost((50.0, 50.0))]);
}

#[test]
fn acknowledging_an_older_up_keeps_the_queued_presss_release() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, DOWN);
    step(&mut hold, Event::Dispatched { down: true });
    step(&mut hold, UP);
    step(&mut hold, DOWN);
    step(&mut hold, sample(false, now, (50.0, 50.0)));
    step(&mut hold, Event::Dispatched { down: false });
    step(&mut hold, Event::Released);
    step(&mut hold, sample(false, now, (70.0, 70.0)));
    step(&mut hold, Event::Dispatched { down: true });
    assert_eq!(hold.deadline(), Some(now + UNSEEN_UP_GRACE), "the grace keeps running");
    assert_eq!(effects(&mut hold, sample(false, now + UNSEEN_UP_GRACE, (80.0, 80.0))), [Effect::ReleaseLost((50.0, 50.0))]);
}

#[test]
fn a_position_ends_the_press_before_the_front_click_without_taking_it() {
    let mut hold = Hold::default();
    step(&mut hold, DOWN);
    step(&mut hold, Event::Seen { down: true, at: (30.0, 30.0), over: true });
    assert!(step(&mut hold, Event::Positioning).is_empty(), "the front press is the first: nothing before it to end");
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::Deliver((20.0, 20.0))]);
    assert_eq!(effects(&mut hold, Event::Positioning), [Effect::ReleaseLost((20.0, 20.0))], "its own Down, never the new press's point");
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::Deliver((30.0, 30.0))], "landed once");
}

#[test]
fn with_no_rise_read_a_press_ends_where_it_was_last_read_down() {
    let mut hold = Hold::default();
    let now = Instant::now();
    step(&mut hold, sample(false, now, (5.0, 5.0))); // a hover older than the press
    step(&mut hold, DOWN);
    step(&mut hold, Event::Dispatched { down: true });
    step(&mut hold, sample(true, now, (25.0, 25.0)));
    step(&mut hold, Event::Seen { down: true, at: (30.0, 30.0), over: true });
    assert_eq!(effects(&mut hold, Event::Dispatched { down: true }), [Effect::ReleaseLost((25.0, 25.0)), Effect::Deliver((30.0, 30.0))]);
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
    step(&mut hold, sample(true, Instant::now(), (25.0, 15.0)));
    assert_eq!(effects(&mut hold, Event::ViewLost { last: Some((5.0, 5.0)) }), [Effect::ReleaseLost((25.0, 15.0))], "not a reading older than the press");
    assert_eq!(effects(&mut hold, Event::ViewLost { last: Some((25.0, 15.0)) }), [Effect::ReleaseLost((25.0, 15.0))], "no press seen: the last reading");
    assert_eq!(hold, Hold::default());
    step(&mut hold, DOWN);
    assert!(step(&mut hold, Event::Stopped).is_empty());
    assert_eq!(hold, Hold::default());
}

#[test]
fn the_button_is_known_up_once_a_sample_or_the_monitor_says_so() {
    let mut hold = Hold::default();
    let now = Instant::now();
    assert!(!hold.button_up());
    step(&mut hold, DOWN);
    assert!(!hold.button_up(), "the press itself is still on its way");
    step(&mut hold, Event::Dispatched { down: true });
    assert!(!hold.button_up(), "held");
    step(&mut hold, UP);
    assert!(hold.button_up(), "the monitor saw the Up");
    step(&mut hold, Event::Dispatched { down: false });
    step(&mut hold, Event::Released);
    step(&mut hold, DOWN);
    step(&mut hold, Event::Dispatched { down: true });
    step(&mut hold, sample(false, now, (30.0, 30.0)));
    assert!(hold.button_up(), "a sample read it rise");
    step(&mut hold, sample(true, now, (30.0, 30.0)));
    assert!(hold.button_up(), "rebounding: the held press's own Up is still missing");
    step(&mut hold, DOWN);
    assert!(!hold.button_up(), "a new press is on its way");
}
