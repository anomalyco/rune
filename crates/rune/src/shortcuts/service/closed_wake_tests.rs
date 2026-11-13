//! A host's wake ends with the host: a call that finishes after the host closed wakes nothing,
//! and a close waits for a wake already running. Real `Guard`s, real threads.

use super::*;
use crate::{App, AppSpec, el};
use std::{sync::atomic::AtomicUsize, time::Duration};

const PATIENCE: Duration = Duration::from_secs(30);
/// How long a close must stay blocked to count as waiting. A correct run always waits this out.
const STILL_WAITING: Duration = Duration::from_millis(200);

fn app(title: &'static str) -> App<()> { App::new(AppSpec::new(title, (10.0, 10.0), || (), |_, _| el())) }

/// Pauses this thread just before its next wake, until `resume` is sent.
fn pause_before_wake(paused: mpsc::Sender<()>, resume: mpsc::Receiver<()>) {
    BEFORE_WAKE.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
        paused.send(()).unwrap();
        resume.recv().unwrap();
    })));
}

#[test]
fn a_call_that_finishes_after_its_host_closed_wakes_nothing() {
    let app = app("Closed");
    let wakes = Arc::new(AtomicUsize::new(0));
    let guard = { let wakes = wakes.clone(); Guard::attach(&app.shared, Arc::new(move || { wakes.fetch_add(1, Ordering::Relaxed); }), true, false) };

    // The call is done with the engine (or, off Linux, has posted its job) and is about to wake.
    let (paused, is_paused) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let handle = app.handle();
    let caller = std::thread::spawn(move || {
        pause_before_wake(paused, resumed);
        handle.shortcut_owner(&"ctrl+alt+shift+f19".parse().unwrap())
    });
    is_paused.recv_timeout(PATIENCE).expect("the call reached its wake");

    drop(guard);
    let at_close = wakes.load(Ordering::Relaxed);
    resume.send(()).unwrap();
    assert_eq!(caller.join().unwrap(), None);
    assert_eq!(wakes.load(Ordering::Relaxed), at_close, "a host that has closed was woken");
}

#[test]
fn a_close_waits_for_a_wake_already_running() {
    let app = app("Waking");
    // The host's wake stops once, when armed, until it is let go.
    let armed = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let (inside, is_inside) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let wake: Waker = {
        let (armed, finished, released) = (armed.clone(), finished.clone(), Mutex::new(released));
        Arc::new(move || {
            if !armed.swap(false, Ordering::Relaxed) { return; }
            inside.send(()).unwrap();
            released.lock().unwrap().recv().unwrap();
            finished.store(true, Ordering::Relaxed);
        })
    };

    let (attached, is_attached) = mpsc::channel();
    let (close, closes) = mpsc::channel::<()>();
    let (closed, is_closed) = mpsc::channel();
    let host = {
        let (shared, finished) = (app.shared.clone(), finished.clone());
        std::thread::spawn(move || {
            let guard = Guard::attach(&shared, wake, true, false);
            attached.send(()).unwrap();
            closes.recv().unwrap();
            drop(guard);
            closed.send(finished.load(Ordering::Relaxed)).unwrap();
        })
    };
    is_attached.recv_timeout(PATIENCE).unwrap();

    armed.store(true, Ordering::Relaxed);
    let handle = app.handle();
    let caller = std::thread::spawn(move || handle.shortcut_owner(&"ctrl+alt+shift+f20".parse().unwrap()));
    is_inside.recv_timeout(PATIENCE).expect("the call's wake started");

    close.send(()).unwrap();
    let early = is_closed.recv_timeout(STILL_WAITING);
    release.send(()).unwrap();
    let late = if early.is_err() { is_closed.recv_timeout(PATIENCE).ok() } else { None };
    host.join().unwrap();
    caller.join().unwrap();

    assert_eq!(early.ok(), None, "the host closed while its wake was still running");
    assert_eq!(late, Some(true), "the close finishes once the wake has");
}
