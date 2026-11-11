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

static GATES: [Gate; 3] = [Gate::new(), Gate::new(), Gate::new()];

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

/// Chords held by live `Exclusive` backends, as a display server's grabs are.
static GRABS: Mutex<Vec<Shortcut>> = Mutex::new(Vec::new());
const BUSY_CHORD: &str = "ctrl+alt+shift+f14";

/// A backend that refuses a chord another backend holds, until that backend is gone. Registering
/// `BUSY_CHORD` stops at gate 2 first, for a call stopped inside the engine.
#[derive(Default)]
struct Exclusive(Vec<Shortcut>);

impl Backend for Exclusive {
    fn register(&mut self, _: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        if *shortcut == BUSY_CHORD.parse().unwrap() { GATES[2].pause(); }
        let mut grabs = GRABS.lock().unwrap();
        if grabs.contains(shortcut) { return Err(ShortcutError::Conflict); }
        grabs.push(shortcut.clone());
        self.0.push(shortcut.clone());
        Ok(())
    }
    fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
}

impl Drop for Exclusive {
    fn drop(&mut self) { GRABS.lock().unwrap().retain(|grab| !self.0.contains(grab)); }
}

fn connect_exclusive(_: bool, _: Sink) -> Result<NativeBackend, ShortcutError> { Ok(Box::<Exclusive>::default()) }

/// A second App in this process, to ask who holds a chord.
fn witness() -> (App<()>, Guard) {
    let app = app("Witness");
    let guard = Guard::attach_with(&app.shared, Arc::new(|| {}), false, false, connect_exclusive);
    (app, guard)
}

/// A host on its own thread, attached until `finish` is sent.
fn host(app: &App<()>, wake: Waker) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let shared = app.shared.clone();
    let (attached, is_attached) = mpsc::channel::<()>();
    let (finish, finishes) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let guard = Guard::attach_with(&shared, wake, false, false, connect_exclusive);
        attached.send(()).unwrap();
        finishes.recv().unwrap();
        drop(guard);
    });
    is_attached.recv_timeout(PATIENCE).expect("the host attached");
    (finish, thread)
}

#[test]
fn a_call_in_its_wake_does_not_keep_a_replaced_backends_grab() {
    static STOPPED: Gate = Gate::new();
    let app = app("Held");
    let handle = app.handle();
    let chord: Shortcut = "ctrl+alt+shift+f16".parse().unwrap();
    let (finish_first, first) = host(&app, Arc::new(|| STOPPED.pause()));
    handle.register_shortcut(chord.clone(), |_, _| {}).unwrap();

    STOPPED.arm();
    let holder = { let (handle, chord) = (handle.clone(), chord.clone()); std::thread::spawn(move || handle.shortcut_owner(&chord)) };
    STOPPED.wait_until_paused();

    let (finish_second, second) = host(&app, Arc::new(|| {}));
    let again = handle.register_shortcut(chord.clone(), |_, _| {});
    let (witness, witness_guard) = witness();
    let owner = witness.handle().shortcut_owner(&chord);
    STOPPED.open();
    holder.join().unwrap();
    drop(witness_guard);
    finish_second.send(()).unwrap();
    second.join().unwrap();
    finish_first.send(()).unwrap();
    first.join().unwrap();

    assert!(again.is_ok(), "the old backend's grab went with the replacement: {again:?}");
    assert_eq!(owner.as_deref(), Some("Held"), "another App can still name who holds the chord");
}

#[test]
fn a_busy_replaced_engine_keeps_its_grab_and_owner_without_holding_up_the_new_host() {
    let inside_gate = &GATES[2];
    let app = app("Busy");
    let handle = app.handle();
    let chord: Shortcut = "ctrl+alt+shift+f15".parse().unwrap();
    let (finish_first, first) = host(&app, Arc::new(|| {}));
    handle.register_shortcut(chord.clone(), |_, _| {}).unwrap();

    // A consent prompt keeps this call inside the engine, holding its lock.
    inside_gate.arm();
    let inside = { let handle = handle.clone(); std::thread::spawn(move || handle.register_shortcut(BUSY_CHORD.parse().unwrap(), |_, _| {})) };
    inside_gate.wait_until_paused();

    let shared = app.shared.clone();
    let (attached, is_attached) = mpsc::channel::<()>();
    let (finish_second, second_finishes) = mpsc::channel::<()>();
    let second = std::thread::spawn(move || {
        let guard = Guard::attach_with(&shared, Arc::new(|| {}), false, false, connect_exclusive);
        attached.send(()).unwrap();
        second_finishes.recv().unwrap();
        drop(guard);
    });
    let returned = is_attached.recv_timeout(PATIENCE);

    // The old backend is still open, so the record says so.
    let (witness, witness_guard) = witness();
    let owner = witness.handle().shortcut_owner(&chord);
    let refused = handle.register_shortcut(chord.clone(), |_, _| {});
    inside_gate.open();
    let finished = inside.join().unwrap();
    let again = handle.register_shortcut(chord.clone(), |_, _| {});
    drop(witness_guard);
    finish_second.send(()).unwrap();
    second.join().unwrap();
    finish_first.send(()).unwrap();
    first.join().unwrap();

    assert!(returned.is_ok(), "the new host attached while the old call was still inside");
    assert_eq!(owner.as_deref(), Some("Busy"), "the owner stays on record while the grab stands");
    assert_eq!(refused, Err(ShortcutError::Conflict), "the old backend really does still hold it");
    assert!(finished.is_ok(), "the old call finishes on its own engine: {finished:?}");
    assert!(again.is_ok(), "the chord is free once the old call has gone: {again:?}");
}

/// Waits for `ready`, which another thread makes true.
fn wait_until(ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + PATIENCE;
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "the other thread never got there");
        std::thread::yield_now();
    }
}

#[test]
fn a_call_parked_on_a_replaced_engine_still_runs_on_it_and_wakes_its_host() {
    use std::sync::atomic::AtomicUsize;
    static FIRST_WAKES: AtomicUsize = AtomicUsize::new(0);
    static SECOND_WAKES: AtomicUsize = AtomicUsize::new(0);
    let app = app("Parked");
    let handle = app.handle();
    let chord: Shortcut = "ctrl+alt+shift+f13".parse().unwrap();
    let guard = Guard::attach_with(&app.shared, Arc::new(|| { FIRST_WAKES.fetch_add(1, Ordering::Relaxed); }), false, false, connect_exclusive);
    let id = handle.register_shortcut(chord, |_, _| {}).unwrap();
    let wakes_before = FIRST_WAKES.load(Ordering::Relaxed);
    let control = app.shared.shortcuts.control.clone();

    // Holding the engine's lock parks the call on it. Its own clone of the Arc shows it got there.
    let old = control.engine.lock().unwrap().clone().unwrap();
    let locked = old.lock().unwrap();
    let caller = { let handle = handle.clone(); std::thread::spawn(move || handle.unregister_shortcut(id)) };
    wait_until(|| Arc::strong_count(&old) == 3);

    let (finish_second, second_finishes) = mpsc::channel::<()>();
    let second = {
        let shared = app.shared.clone();
        std::thread::spawn(move || {
            let guard = Guard::attach_with(&shared, Arc::new(|| { SECOND_WAKES.fetch_add(1, Ordering::Relaxed); }), false, false, connect_exclusive);
            second_finishes.recv().unwrap();
            drop(guard);
        })
    };
    wait_until(|| control.engine.try_lock().is_ok_and(|slot| slot.as_ref().is_some_and(|current| !Arc::ptr_eq(current, &old))));
    drop(locked);

    let answer = caller.join().unwrap();
    let (first_wakes, second_wakes) = (FIRST_WAKES.load(Ordering::Relaxed), SECOND_WAKES.load(Ordering::Relaxed));
    finish_second.send(()).unwrap();
    second.join().unwrap();
    drop(old);
    drop(guard);
    assert_eq!(answer, Ok(()), "the old engine still holds the registration");
    assert_eq!((first_wakes - wakes_before, second_wakes), (1, 0), "the call wakes the host whose engine ran it");
}
