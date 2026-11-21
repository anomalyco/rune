use super::{ErrorLog, Note};

/// An invalid pipeline set every frame logs once, not once a frame.
#[test]
fn a_repeated_error_is_logged_once() {
    let mut log = ErrorLog::default();
    let error = "[Invalid RenderPipeline \"uber\"] is invalid due to a previous error.";
    assert_eq!(log.note(error), Note::First);
    for _ in 0..300 { assert_eq!(log.note(error), Note::Quiet); }
}

/// Different errors are each logged, up to a limit, then one line says the rest are muted.
#[test]
fn different_errors_are_logged_until_the_limit() {
    let mut log = ErrorLog::default();
    let notes: Vec<Note> = (0..8).map(|n| log.note(&format!("error {n}"))).collect();
    assert_eq!(notes, [Note::First, Note::New, Note::New, Note::New, Note::Mute, Note::Quiet, Note::Quiet, Note::Quiet]);
    assert_eq!(log.note("error 0"), Note::Quiet);
}
