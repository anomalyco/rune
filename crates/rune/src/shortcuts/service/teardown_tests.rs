//! A Linux backend still closing, through the real `Guard` and real threads. A closing backend
//! holds its chords until it is gone, so nothing may reach the next engine before then.

use super::*;
use crate::{App, AppSpec, el};
use std::{sync::Condvar, time::Duration};

const PATIENCE: Duration = Duration::from_secs(30);
/// How long a call must stay parked to count as waiting. A correct run always waits this out.
const STILL_WAITING: Duration = Duration::from_millis(200);

/// Holds a backend's `Drop` open while armed.
struct Gate { state: Mutex<(bool, bool)>, changed: Condvar }

static GATES: [Gate; 2] = [Gate::new(), Gate::new()];

impl Gate {
    const fn new() -> Self { Self { state: Mutex::new((false, false)), changed: Condvar::new() } }

    fn arm(&self) { self.state.lock().unwrap().0 = true; }

    fn open(&self) {
        self.state.lock().unwrap().0 = false;
        self.changed.notify_all();
    }

    fn pause(&self) {
        let mut state = self.state.lock().unwrap();
        if !state.0 { return; }
        state.1 = true;
        self.changed.notify_all();
        while state.0 { state = self.changed.wait(state).unwrap(); }
    }

    fn wait_until_paused(&self) {
        let state = self.state.lock().unwrap();
        let (state, timeout) = self.changed.wait_timeout_while(state, PATIENCE, |state| !state.1).unwrap();
        drop(state);
        assert!(!timeout.timed_out(), "the backend never started closing");
    }
}

struct Pausing(&'static Gate);

impl Backend for Pausing {
    fn register(&mut self, _: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> { Ok(()) }
    fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
}

impl Drop for Pausing {
    fn drop(&mut self) { self.0.pause(); }
}

fn connect<const GATE: usize>(_: bool, _: Sink) -> Result<NativeBackend, ShortcutError> {
    Ok(Box::new(Pausing(&GATES[GATE])))
}

fn app(title: &'static str) -> App<()> { App::new(AppSpec::new(title, (10.0, 10.0), || (), |_, _| el())) }

#[test]
fn a_new_host_waits_for_the_closing_hosts_backend() {
    let gate = &GATES[0];
    let app = app("Closing");
    let handle = app.handle();
    let chord: Shortcut = "ctrl+alt+shift+f23".parse().unwrap();

    let (registered, first_id) = mpsc::channel();
    let (close, closing) = mpsc::channel::<()>();
    let closer = {
        let (shared, handle, chord) = (app.shared.clone(), handle.clone(), chord.clone());
        std::thread::spawn(move || {
            let guard = Guard::attach_with(&shared, Arc::new(|| {}), false, false, connect::<0>);
            registered.send(handle.register_shortcut(chord, |_, _| {})).unwrap();
            closing.recv().unwrap();
            drop(guard);
        })
    };
    first_id.recv_timeout(PATIENCE).unwrap().unwrap();

    gate.arm();
    close.send(()).unwrap();
    gate.wait_until_paused();
    let refused = handle.register_shortcut(chord.clone(), |_, _| {});

    let (answer, answered) = mpsc::channel();
    let next = {
        let (shared, handle, chord) = (app.shared.clone(), handle.clone(), chord.clone());
        std::thread::spawn(move || {
            let guard = Guard::attach_with(&shared, Arc::new(|| {}), false, false, connect::<0>);
            answer.send(handle.register_shortcut(chord, |_, _| {})).unwrap();
            drop(guard);
        })
    };
    let early = answered.recv_timeout(STILL_WAITING);
    gate.open();
    let late = if early.is_err() { answered.recv_timeout(PATIENCE).ok() } else { None };
    closer.join().unwrap();
    next.join().unwrap();

    assert_eq!(refused, Err(ShortcutError::NotRunning), "calls are refused while the host closes");
    assert_eq!(early.ok(), None, "the next host reached an engine while the old backend held the chord");
    assert!(matches!(late, Some(Ok(_))), "the chord is free once the old backend closed: {late:?}");
}

#[test]
fn a_replacement_engine_is_out_of_reach_until_the_old_backend_closes() {
    let gate = &GATES[1];
    let app = app("Replacing");
    let handle = app.handle();
    let chord: Shortcut = "ctrl+alt+shift+f24".parse().unwrap();

    let (registered, first_id) = mpsc::channel();
    let (finish_first, first_finishes) = mpsc::channel::<()>();
    let first = {
        let (shared, handle, chord) = (app.shared.clone(), handle.clone(), chord.clone());
        std::thread::spawn(move || {
            let guard = Guard::attach_with(&shared, Arc::new(|| {}), false, false, connect::<1>);
            registered.send(handle.register_shortcut(chord, |_, _| {})).unwrap();
            first_finishes.recv().unwrap();
            drop(guard);
        })
    };
    let old_id = first_id.recv_timeout(PATIENCE).unwrap().unwrap();

    gate.arm();
    let (finish_second, second_finishes) = mpsc::channel::<()>();
    let second = {
        let shared = app.shared.clone();
        std::thread::spawn(move || {
            let guard = Guard::attach_with(&shared, Arc::new(|| {}), false, false, connect::<1>);
            second_finishes.recv().unwrap();
            drop(guard);
        })
    };
    gate.wait_until_paused();

    let (answer, answered) = mpsc::channel();
    let caller = {
        let (handle, chord) = (handle.clone(), chord.clone());
        std::thread::spawn(move || {
            let forgotten = handle.unregister_shortcut(old_id);
            answer.send((forgotten, handle.register_shortcut(chord, |_, _| {}))).unwrap();
        })
    };
    let early = answered.recv_timeout(STILL_WAITING);
    gate.open();
    let late = if early.is_err() { answered.recv_timeout(PATIENCE).ok() } else { None };
    caller.join().unwrap();
    finish_second.send(()).unwrap();
    second.join().unwrap();
    finish_first.send(()).unwrap();
    first.join().unwrap();

    assert_eq!(early.ok(), None, "a caller reached the replacement while the old backend held the chord");
    let (forgotten, again) = late.expect("the caller finishes once the old backend closed");
    assert_eq!(forgotten, Err(ShortcutError::UnknownId));
    assert!(again.is_ok(), "the chord is free once the old backend closed: {again:?}");
}
