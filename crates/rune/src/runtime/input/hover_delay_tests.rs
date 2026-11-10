use super::*;

const DELAY: [f32; 2] = [0.5, 0.25];

fn settle(state: &mut Option<Delayed>, inside: bool, now: f64) -> Vec<Effect> {
    step(state, Event::Settle { inside, may_enter: true, delay: DELAY, now }).into_iter().collect()
}

fn due(state: &mut Option<Delayed>, now: f64) -> Vec<Effect> {
    step(state, Event::Due { now }).into_iter().collect()
}

#[test]
fn an_enter_waits_out_its_delay_then_reports() {
    let mut s = None;
    assert_eq!(settle(&mut s, true, 1.0), [Effect::WakeAt(1.5)]);
    assert!(settle(&mut s, true, 1.2).is_empty(), "still inside: the wait runs on");
    assert!(due(&mut s, 1.4).is_empty());
    assert_eq!(due(&mut s, 1.5), [Effect::Report(true)]);
    assert_eq!(s, Some(Delayed::Shown));
}

#[test]
fn leaving_before_the_enter_is_due_reports_nothing_and_its_wake_is_stale() {
    let mut s = None;
    settle(&mut s, true, 1.0);
    assert!(settle(&mut s, false, 1.1).is_empty());
    assert_eq!(s, None);
    // Back in: a new wait, and the first one's wake finds nothing due.
    assert_eq!(settle(&mut s, true, 1.3), [Effect::WakeAt(1.8)]);
    assert!(due(&mut s, 1.5).is_empty());
    assert_eq!(s, Some(Delayed::EnterDue { due: 1.8 }));
}

#[test]
fn a_shown_hover_leaves_after_the_exit_delay_unless_the_pointer_comes_back() {
    let mut s = Some(Delayed::Shown);
    assert_eq!(settle(&mut s, false, 2.0), [Effect::WakeAt(2.25)]);
    assert!(settle(&mut s, true, 2.1).is_empty(), "back inside: still shown, no report");
    assert_eq!(s, Some(Delayed::Shown));
    assert!(due(&mut s, 2.25).is_empty());
    settle(&mut s, false, 3.0);
    assert_eq!(due(&mut s, 3.25), [Effect::Report(false)]);
    assert_eq!(s, None);
}

#[test]
fn zero_delays_report_at_once() {
    let mut s = None;
    let now = step(&mut s, Event::Settle { inside: true, may_enter: true, delay: [0.0, 0.0], now: 0.0 });
    assert_eq!(now.into_iter().collect::<Vec<_>>(), [Effect::Report(true)]);
    assert_eq!(s, Some(Delayed::Shown));
}

#[test]
fn a_press_or_blur_keeps_an_enter_from_starting_but_not_a_shown_hover_from_staying() {
    let mut s = None;
    assert_eq!(step(&mut s, Event::Settle { inside: true, may_enter: false, delay: DELAY, now: 0.0 }), Effects::none());
    assert_eq!(s, None);
    let mut shown = Some(Delayed::Shown);
    assert_eq!(step(&mut shown, Event::Settle { inside: true, may_enter: false, delay: DELAY, now: 0.0 }), Effects::none());
    assert_eq!(shown, Some(Delayed::Shown));
}

#[test]
fn a_press_stops_an_enter_wait_but_not_a_leave_wait() {
    let mut entering = Some(Delayed::EnterDue { due: 1.0 });
    step(&mut entering, Event::Pressed);
    assert_eq!(entering, None);
    let mut leaving = Some(Delayed::LeaveDue { due: 1.0 });
    step(&mut leaving, Event::Pressed);
    assert_eq!(leaving, Some(Delayed::LeaveDue { due: 1.0 }));
}

#[test]
fn ending_reports_a_leave_only_to_a_hover_that_heard_the_enter() {
    for (from, heard) in [(Some(Delayed::Shown), true), (Some(Delayed::LeaveDue { due: 1.0 }), true), (Some(Delayed::EnterDue { due: 1.0 }), false), (None, false)] {
        let mut s = from;
        let effects: Vec<_> = step(&mut s, Event::Ended).into_iter().collect();
        assert_eq!(effects, if heard { vec![Effect::Report(false)] } else { vec![] }, "from {from:?}");
        assert_eq!(s, None);
    }
}

#[test]
fn the_list_reports_waits_in_the_order_they_started() {
    let mut list = HoverDelays::default();
    list.step("a", Event::Settle { inside: true, may_enter: true, delay: [0.0, 0.0], now: 0.0 });
    list.step("b", Event::Settle { inside: true, may_enter: true, delay: DELAY, now: 0.0 });
    // `a` starts its leave after `b` started its enter.
    list.step("a", Event::Settle { inside: false, may_enter: true, delay: [0.5, 0.5], now: 0.0 });
    assert_eq!(list.paths().collect::<Vec<_>>(), ["b", "a"]);
    assert!(list.waiting());
    assert!(list.forget("a"), "a had heard its enter");
    assert!(!list.forget("b"));
    assert!(list.is_empty() && !list.waiting());
}
