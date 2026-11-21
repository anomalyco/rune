use super::*;

fn control() -> Control {
    Control::new(PathBuf::from("note.m4a"))
}

fn start(c: &mut Control) -> Attempt {
    let effects: Vec<_> = c.step(Event::Start { path: PathBuf::from("note.m4a"), virtual_clock: false }, Some(0.0)).into_iter().collect();
    match effects.as_slice() {
        [Effect::Silence, Effect::Ask { attempt }] => *attempt,
        _ => panic!("a start asks for permission"),
    }
}

fn is_empty(effects: Effects<Effect>) -> bool {
    effects.into_iter().next().is_none()
}

#[test]
fn a_permission_answer_after_cancel_opens_nothing() {
    let mut c = control();
    let attempt = start(&mut c);
    assert_eq!(c.status(), RecorderState::Starting);
    c.step(Event::Cancel, None);
    assert!(is_empty(c.step(Event::Permission { attempt, granted: Ok(()) }, None)));
    assert_eq!(c.status(), RecorderState::Idle);

    // A later start's answer is the only one that opens.
    let again = start(&mut c);
    assert!(is_empty(c.step(Event::Permission { attempt, granted: Ok(()) }, None)));
    let opened: Vec<_> = c.step(Event::Permission { attempt: again, granted: Ok(()) }, None).into_iter().collect();
    assert!(matches!(opened.as_slice(), [Effect::Open { attempt }] if *attempt == again));
}

#[test]
fn stale_reports_change_nothing() {
    let mut c = control();
    let stale = start(&mut c);
    c.step(Event::Stop, None);
    let attempt = start(&mut c);
    let generation = c.generation;
    for event in [
        Event::Failed { attempt: stale, error: "late".into() },
        Event::Permission { attempt: stale, granted: Err("late".into()) },
        Event::Interrupted { attempt: stale, reason: "late".into(), duration: 1.0 },
        Event::Stopped { attempt: stale, result: Ok(1.0) },
        // Nothing has opened for this attempt yet, so it can't have begun.
        Event::Began { attempt },
    ] {
        assert!(is_empty(c.step(event, Some(1.0))));
    }
    assert!(matches!(c.phase, Phase::Asking { attempt: a } if a == attempt));
    assert_eq!(c.generation, generation, "the app's waker hears nothing");
}

#[test]
fn a_start_while_starting_changes_nothing() {
    let mut c = control();
    let attempt = start(&mut c);
    assert!(is_empty(c.step(Event::Start { path: PathBuf::from("other.m4a"), virtual_clock: true }, None)));
    assert!(matches!(c.phase, Phase::Asking { attempt: a } if a == attempt));
    assert_eq!((c.path.as_path(), c.virtual_clock), (std::path::Path::new("note.m4a"), false));
}

#[test]
fn an_interruption_while_the_device_finishes_returns_what_it_saved() {
    let mut c = control();
    let attempt = c.seq.next();
    c.phase = Phase::Stopping { attempt, paused: false };
    assert_eq!(c.status(), RecorderState::Recording);
    c.step(Event::Interrupted { attempt, reason: "the input device changed".into(), duration: 2.0 }, None);
    assert_eq!(c.status(), RecorderState::Interrupted("the input device changed".into()));
    let reply: Vec<_> = c.step(Event::Stopped { attempt, result: Err("the recording already ended".into()) }, None).into_iter().collect();
    assert!(matches!(reply.as_slice(), [Effect::Silence, Effect::Reply(Ok(r))] if r.duration == 2.0));
    assert_eq!(c.status(), RecorderState::Stopped);
}

#[test]
fn stopping_reads_as_the_phase_it_left() {
    let mut c = control();
    let attempt = c.seq.next();
    c.phase = Phase::Stopping { attempt, paused: true };
    assert_eq!(c.status(), RecorderState::Paused);
    assert!(matches!(c.step(Event::Stop, None).into_iter().next(), Some(Effect::Reply(Err(_)))));
    assert!(is_empty(c.step(Event::Cancel, None)));
    assert!(is_empty(c.step(Event::Start { path: PathBuf::from("note.m4a"), virtual_clock: false }, None)));
    assert_eq!(c.status(), RecorderState::Paused);
}

/// An input that opens for an attempt that's gone is cancelled rather than left recording.
#[cfg(target_os = "macos")]
#[test]
fn an_input_opened_for_a_gone_attempt_is_cancelled() {
    let mut c = control();
    let attempt = start(&mut c);
    c.step(Event::Cancel, None);
    let (session, _commands) = backend::idle_session();
    let effects: Vec<_> = c.step(Event::DeviceOpened { attempt, opened: Ok(session) }, None).into_iter().collect();
    assert!(matches!(effects.as_slice(), [Effect::CancelDevice(_)]));
    assert_eq!(c.status(), RecorderState::Idle);
}

#[cfg(target_os = "macos")]
#[test]
fn a_device_recording_pauses_resumes_and_hands_its_session_to_stop() {
    let mut c = control();
    let attempt = start(&mut c);
    c.step(Event::Permission { attempt, granted: Ok(()) }, Some(0.0));
    let (session, _commands) = backend::idle_session();
    c.step(Event::DeviceOpened { attempt, opened: Ok(session) }, Some(0.0));
    assert_eq!(c.status(), RecorderState::Starting);
    c.step(Event::Began { attempt }, Some(1.0));
    c.step(Event::Tick, Some(3.0));
    assert_eq!((c.status(), c.elapsed), (RecorderState::Recording, 2.0));
    let paused: Vec<_> = c.step(Event::Pause, Some(4.0)).into_iter().collect();
    assert!(matches!(paused.as_slice(), [Effect::Silence, Effect::PauseDevice]));
    c.step(Event::Tick, Some(10.0));
    assert_eq!(c.elapsed, 3.0, "held while paused");
    let resumed: Vec<_> = c.step(Event::Resume, Some(10.0)).into_iter().collect();
    assert!(matches!(resumed.as_slice(), [Effect::ResumeDevice]));
    let stop: Vec<_> = c.step(Event::Stop, Some(11.0)).into_iter().collect();
    assert!(matches!(stop.as_slice(), [Effect::StopDevice { attempt: a, .. }] if *a == attempt));
    assert_eq!((c.status(), c.elapsed), (RecorderState::Recording, 4.0));
    c.step(Event::Stopped { attempt, result: Ok(4.0) }, Some(11.5));
    assert_eq!(c.status(), RecorderState::Stopped);
}
