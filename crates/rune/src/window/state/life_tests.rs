use super::Life;
use super::super::sight::{Cover, Sight};

#[test]
fn showing_uncovers_and_hiding_stamps_only_a_shown_window() {
    let mut life = Life::Unopened;
    life.hide(|| panic!("a window that never showed has no hide to stamp"));
    assert_eq!(life, Life::Unopened);
    assert!(life.show() && life.uncovered());
    assert!(!life.cover(Cover::System, true) && life.covered());
    life.hide(|| 7);
    assert_eq!(life.hidden_at(), Some(7));
    life.hide(|| panic!("hiding again keeps the first stamp"));
    assert!(!life.cover(Cover::System, true), "a hidden window has nothing to cover");
    assert_eq!(life, Life::Hidden { at: 7 });
    assert!(life.show() && life.uncovered(), "showing uncovers");
}

#[test]
fn a_finished_window_never_shows_or_closes_again() {
    let mut life = Life::Shown { sight: Sight::Seen };
    life.finish();
    assert!(!life.show() && life.finished());
    assert!(!life.closes(false, true), "its close hook ran once");
    assert!(!life.cover(Cover::System, false));
}

#[test]
fn a_hidden_window_closes_alike_whether_or_not_it_kept_its_window() {
    let hidden = Life::Hidden { at: 1 };
    // Released while hidden (`release_gpu_when_hidden`, or Wayland unmapping it): without
    // `keep_running` a close still runs `on_close` and ends the window.
    assert!(hidden.closes(false, false));
    assert!(hidden.closes(false, true));
    assert!(!hidden.closes(true, false) && !hidden.closes(true, true));
    assert!(Life::Shown { sight: Sight::Seen.step(Cover::System, true) }.closes(true, true));
    assert!(!Life::Unopened.closes(false, false), "nothing to close");
    assert!(Life::Unopened.closes(false, true) && !Life::Unopened.closes(true, true));
}

#[test]
fn first_window_creation_failure_finishes_a_normal_host_only() {
    let mut normal = Life::Unopened;
    assert!(normal.fail_create(false, false), "an ordinary app ends when its first window fails");
    assert!(normal.finished());

    let mut keep_running = Life::Unopened;
    assert!(!keep_running.fail_create(true, false), "keep_running and windowless stay alive to retry");
    assert_eq!(keep_running, Life::Unopened);

    let mut reopened = Life::Hidden { at: 1 };
    assert!(!reopened.fail_create(false, true), "a window that already showed stays alive when reopening fails");
    assert_eq!(reopened, Life::Hidden { at: 1 });
}
