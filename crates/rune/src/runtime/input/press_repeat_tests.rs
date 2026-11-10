use super::*;

fn armed(seq: &mut Seq, path: &str, due: f64) -> (Repeat, Vec<Effect>) {
    let mut state = Repeat::default();
    let effects = step(seq, &mut state, Event::Arm { path, due }).into_iter().collect();
    (state, effects)
}

#[test]
fn an_arm_keeps_its_attempt_and_wakes_at_its_due_time() {
    let mut seq = Seq::default();
    let (state, effects) = armed(&mut seq, "row", 1.5);
    let Repeat::Armed { attempt, path, due } = state.clone() else { panic!("{state:?}") };
    assert_eq!((path.as_str(), due), ("row", 1.5));
    assert_eq!(effects, vec![Effect::WakeAt(1.5)]);
    assert_eq!(state.attempt(), Some(attempt));
}

#[test]
fn a_due_fires_once_at_or_after_its_time() {
    let mut seq = Seq::default();
    let (mut state, _) = armed(&mut seq, "row", 1.5);
    let attempt = state.attempt().unwrap();
    let early = step(&mut seq, &mut state, Event::Due { attempt, now: 1.0 });
    assert!(early.is_empty(), "before its time");
    assert_eq!(state.attempt(), Some(attempt), "still armed");
    let fired: Vec<Effect> = step(&mut seq, &mut state, Event::Due { attempt, now: 1.5 }).into_iter().collect();
    assert_eq!(fired, vec![Effect::Fire("row".to_owned())]);
    assert_eq!(state, Repeat::Idle);
    let again = step(&mut seq, &mut state, Event::Due { attempt, now: 9.0 });
    assert!(again.is_empty(), "a spent repeat fires nothing");
}

#[test]
fn a_wake_for_an_earlier_arm_changes_nothing() {
    let mut seq = Seq::default();
    let (mut state, _) = armed(&mut seq, "row", 1.5);
    let first = state.attempt().unwrap();
    // The release and a second arm inside one frame replace the first, with a later due.
    step(&mut seq, &mut state, Event::End);
    let (state2, _) = armed(&mut seq, "row", 5.0);
    state = state2;
    let stale = step(&mut seq, &mut state, Event::Due { attempt: first, now: 9.0 });
    assert!(stale.is_empty(), "an earlier arm's wake fires nothing");
    assert!(state.attempt().is_some(), "the later arm still waits");
}

#[test]
fn a_release_ends_the_repeat() {
    let mut seq = Seq::default();
    let (mut state, _) = armed(&mut seq, "row", 1.5);
    let attempt = state.attempt().unwrap();
    step(&mut seq, &mut state, Event::End);
    assert_eq!(state, Repeat::Idle);
    assert!(step(&mut seq, &mut state, Event::Due { attempt, now: 2.0 }).is_empty());
}

#[test]
fn an_arm_for_another_element_is_the_new_one() {
    let mut seq = Seq::default();
    let (mut state, _) = armed(&mut seq, "row", 1.5);
    let first = state.attempt().unwrap();
    step(&mut seq, &mut state, Event::Arm { path: "cell", due: 2.0 });
    let second = state.attempt().unwrap();
    assert_ne!(first, second, "a new arm is a new attempt");
    assert_eq!(state, Repeat::Armed { attempt: second, path: "cell".to_owned(), due: 2.0 });
}
