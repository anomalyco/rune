//! A host's wake ends with the host: a call that finishes after the host closed wakes nothing, a
//! press wakes only its own host, and a close never waits on a wake. Real `Guard`s, real threads.

use super::*;
use crate::{App, AppSpec, el};
use std::{sync::atomic::AtomicUsize, time::Duration};

const PATIENCE: Duration = Duration::from_secs(30);

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

/// The deadlock a wake callback held under a lock made: the host's wake re-enters routing and
/// waits for a call the host's close must release.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_close_releases_a_call_its_own_wake_is_waiting_on() {
    let app = app("Reentrant");
    let armed = Arc::new(AtomicBool::new(false));
    let (nested, is_nested) = mpsc::channel();
    let (nested_answer, nested_answered) = mpsc::channel();
    let wake: Waker = {
        let (armed, handle) = (armed.clone(), app.handle());
        let (nested, nested_answer) = (Mutex::new(Some(nested)), Mutex::new(Some(nested_answer)));
        Arc::new(move || {
            if !armed.swap(false, Ordering::Relaxed) { return; }
            // Once the inner call has posted its job and is about to wake, it is waiting on it.
            let nested = nested.lock().unwrap().take().unwrap();
            BEFORE_WAKE.with(|hook| *hook.borrow_mut() = Some(Box::new(move || nested.send(()).unwrap())));
            let answer = handle.shortcut_owner(&"ctrl+alt+shift+f9".parse().unwrap());
            nested_answer.lock().unwrap().take().unwrap().send(answer).unwrap();
        })
    };

    let (attached, is_attached) = mpsc::channel();
    let (close, closes) = mpsc::channel::<()>();
    let (closed, is_closed) = mpsc::channel();
    let host = {
        let shared = app.shared.clone();
        std::thread::spawn(move || {
            let guard = Guard::attach(&shared, wake, true, false);
            attached.send(()).unwrap();
            closes.recv().unwrap();
            drop(guard);
            closed.send(()).unwrap();
        })
    };
    is_attached.recv_timeout(PATIENCE).unwrap();

    armed.store(true, Ordering::Relaxed);
    let handle = app.handle();
    let caller = std::thread::spawn(move || handle.shortcut_owner(&"ctrl+alt+shift+f10".parse().unwrap()));
    is_nested.recv_timeout(PATIENCE).expect("the wake's own call posted and waits");

    close.send(()).unwrap();
    // A deadlocked close never answers: fail rather than join threads that never finish.
    assert!(is_closed.recv_timeout(PATIENCE).is_ok(), "the host's close waited on its own wake");
    assert_eq!(nested_answered.recv_timeout(PATIENCE).unwrap(), None, "the close released the wake's call");
    host.join().unwrap();
    assert_eq!(caller.join().unwrap(), None);
}

/// The sink each `capture` backend was handed, so the test can press its chord.
static PRESS: Mutex<Option<Sink>> = Mutex::new(None);

struct Accept;

impl Backend for Accept {
    fn register(&mut self, _: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> { Ok(()) }
    fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
}

fn capture(_: bool, sink: Sink) -> Result<NativeBackend, ShortcutError> {
    *PRESS.lock().unwrap() = Some(sink);
    Ok(Box::new(Accept))
}

fn counter() -> (Arc<AtomicUsize>, Waker) {
    let count = Arc::new(AtomicUsize::new(0));
    let wake = { let count = count.clone(); Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); }) };
    (count, wake)
}

#[test]
fn a_press_wakes_its_own_host_and_never_a_closed_window() {
    let app = app("Pressed");
    let (first_wakes, first_wake) = counter();
    app.shared.connect(first_wake.clone());
    let first = Guard::attach_with(&app.shared, first_wake, false, false, capture);
    let id = app.handle().register_shortcut("ctrl+alt+shift+f8".parse().unwrap(), |_, _| {}).unwrap();
    let press = PRESS.lock().unwrap().take().expect("the backend connected");

    // A second window connects, takes over shortcuts and closes; the first stays open.
    let (second_wakes, second_wake) = counter();
    app.shared.connect(second_wake.clone());
    drop(Guard::attach_with(&app.shared, second_wake, false, false, capture));
    let (first_before, second_before) = (first_wakes.load(Ordering::Relaxed), second_wakes.load(Ordering::Relaxed));

    press(id, None);
    assert_eq!(second_wakes.load(Ordering::Relaxed), second_before, "a press woke a window that had closed");
    assert_eq!(first_wakes.load(Ordering::Relaxed), first_before + 1, "a press wakes the host whose engine it came from");
    assert_eq!(app.shared.take().updates.len(), 1);

    // Once its host has gone too, a press queues and wakes nothing.
    drop(first);
    press(id, None);
    assert_eq!(first_wakes.load(Ordering::Relaxed), first_before + 1, "a press woke its closed host");
    assert!(app.shared.take().updates.is_empty(), "a press for a closed host was queued");
}
