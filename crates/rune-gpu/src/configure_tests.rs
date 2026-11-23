use super::Gate;
use std::sync::mpsc;

#[test]
fn a_submission_runs_when_no_surface_is_being_configured() {
    let gate = Gate::new();
    assert_eq!(gate.try_submit(|| 7), Some(7));
    assert_eq!(gate.configuring(|| 8), 8);
    assert_eq!(gate.try_submit(|| 9), Some(9));
}

#[test]
fn a_submission_from_another_thread_is_refused_while_a_surface_is_configured() {
    let gate = &Gate::new();
    let (configuring, entered) = mpsc::channel();
    let (finish, finished) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            gate.configuring(|| {
                configuring.send(()).unwrap();
                finished.recv().unwrap();
            })
        });
        entered.recv().unwrap();
        assert_eq!(gate.try_submit(|| ()), None);
        finish.send(()).unwrap();
    });
    assert_eq!(gate.try_submit(|| ()), Some(()));
}

/// wgpu runs completion callbacks inside `Surface::configure`, on the thread that configures.
#[test]
fn a_submission_from_inside_the_configure_is_refused_without_waiting() {
    let gate = Gate::new();
    assert_eq!(gate.configuring(|| gate.try_submit(|| ())), None);
}

#[test]
fn a_surface_waits_for_a_submission_under_way() {
    let gate = &Gate::new();
    let (submitting, entered) = mpsc::channel();
    let (finish, finished) = mpsc::channel::<()>();
    let (configured, done) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            gate.try_submit(|| {
                submitting.send(()).unwrap();
                finished.recv().unwrap();
            })
        });
        entered.recv().unwrap();
        scope.spawn(move || gate.configuring(|| configured.send(()).unwrap()));
        assert!(done.recv_timeout(std::time::Duration::from_millis(50)).is_err(), "the configure ran beside a submission");
        finish.send(()).unwrap();
        done.recv().unwrap();
    });
}
