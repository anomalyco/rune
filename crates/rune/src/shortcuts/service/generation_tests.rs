//! A replaced host acting on its successor, through the real `Guard`, with each thread's steps
//! run one at a time and checked after every step. An outgoing close must not take a newer
//! engine or clear its handlers, a replaced engine's activation must not run once a newer host
//! attached, and a replaced host must not run the calls that wait for the newest one.

use super::*;
use crate::{App, AppSpec, el};
use std::{sync::atomic::AtomicI32, time::Duration};

const PATIENCE: Duration = Duration::from_secs(30);

fn app(title: &'static str) -> App<u32> { App::new(AppSpec::new(title, (10.0, 10.0), || 0, |_: &u32, _| el())) }

fn chord(text: &str) -> Shortcut { text.parse().unwrap() }

/// Backends each engine opened and hasn't dropped, by the index its `connect` names. The close
/// walk uses 0 and 1, the activation walk 2 and 3.
static LIVE: [AtomicI32; 4] = [const { AtomicI32::new(0) }; 4];
/// The sink each backend was opened with, so a test can press a chord as that backend would.
static SINKS: Mutex<Vec<(usize, Sink)>> = Mutex::new(Vec::new());

struct Counted(usize);

impl Backend for Counted {
    fn register(&mut self, _: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> { Ok(()) }
    fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
}

impl Drop for Counted {
    fn drop(&mut self) { LIVE[self.0].fetch_sub(1, Ordering::Relaxed); }
}

fn connect<const N: usize>(_: bool, sink: Sink) -> Result<NativeBackend, ShortcutError> {
    LIVE[N].fetch_add(1, Ordering::Relaxed);
    SINKS.lock().unwrap().push((N, sink));
    Ok(Box::new(Counted(N)))
}

fn live(n: usize) -> i32 { LIVE[n].load(Ordering::Relaxed) }

fn sink_of(n: usize) -> Sink { SINKS.lock().unwrap().iter().rev().find(|(i, _)| *i == n).map(|(_, sink)| sink.clone()).unwrap() }

/// Host A's guard and its close in progress, on the thread the guard belongs to.
#[derive(Default)]
struct Host { guard: Option<Guard>, closing: Option<Closing> }

type Step = Box<dyn FnOnce(&mut Host) + Send>;

/// Where host A lives: on the test's thread beside host B, or on a thread of its own that runs
/// one step at a time while the test waits.
enum Place { Here(Host), Away(mpsc::Sender<Step>, std::thread::JoinHandle<()>) }

impl Place {
    fn new(away: bool) -> Self {
        if !away { return Self::Here(Host::default()); }
        let (send, steps) = mpsc::channel::<Step>();
        let thread = std::thread::spawn(move || {
            let mut host = Host::default();
            for step in steps { step(&mut host); }
        });
        Self::Away(send, thread)
    }

    fn run<T: Send + 'static>(&mut self, step: impl FnOnce(&mut Host) -> T + Send + 'static) -> T {
        match self {
            Self::Here(host) => step(host),
            Self::Away(send, _) => {
                let (answer, answered) = mpsc::channel();
                send.send(Box::new(move |host| { let _ = answer.send(step(host)); })).unwrap();
                answered.recv_timeout(PATIENCE).expect("host A's thread finished the step")
            }
        }
    }

    /// Drops whatever A still holds, on its own thread.
    fn finish(self) {
        if let Self::Away(send, thread) = self {
            drop(send);
            thread.join().unwrap();
        }
    }
}

// A's close, one step at a time, as `Drop` runs them.
fn a_closes_routing(host: &mut Host) { host.closing = Some(host.guard.as_ref().unwrap().close_routing()); }

fn a_releases_engine(host: &mut Host) { host.guard.as_ref().unwrap().release_engine(host.closing.as_mut().unwrap()); }

fn a_ends(host: &mut Host) {
    let mut guard = host.guard.take().unwrap();
    guard.end(host.closing.take().unwrap());
    // Its close already ran, step by step; dropping it would close again.
    std::mem::forget(guard);
}

/// Every order of `lanes` that keeps each lane's own order.
fn interleavings<T: Copy>(lanes: &[&[T]]) -> Vec<Vec<T>> {
    if lanes.iter().all(|lane| lane.is_empty()) { return vec![vec![]]; }
    let mut all = vec![];
    for (n, lane) in lanes.iter().enumerate() {
        let Some((first, rest)) = lane.split_first() else { continue };
        let mut next = lanes.to_vec();
        next[n] = rest;
        for tail in interleavings(&next) { all.push([vec![*first], tail].concat()); }
    }
    all
}

#[derive(Clone, Copy, Debug)]
enum Close { AClosesRouting, AReleasesEngine, AEnds, BInstalls, BRoutes, BRegisters, CRegisters }

/// What the walk expects, from the steps so far.
#[derive(Default)]
struct Expect {
    /// The host routing names: A at first, nobody once A closes it, B once B takes it.
    routed: Option<char>,
    /// Whether A was still the newest host when it closed routing.
    a_current: Option<bool>,
    a_released: bool,
    a_ended: bool,
    b_installed: bool,
    b_registered: bool,
    /// The engine a third caller's registration reached.
    c: Option<char>,
}

struct Ids { a: ShortcutId, b: Option<ShortcutId>, c: Option<ShortcutId> }

/// Who holds each backend and handler. `replaces`: B's engine takes the place of A's.
fn check(app: &App<u32>, expect: &Expect, ids: &Ids, replaces: bool, at: &str) {
    let a_alive = !(expect.b_installed && replaces) && !expect.a_released;
    assert_eq!(live(0), i32::from(a_alive), "A's backend: {at}");
    let b_holds = expect.b_registered || expect.c == Some('B');
    assert_eq!(live(1), i32::from(b_holds), "B's backend lives while B holds a chord: {at}");
    let a_cleared = expect.a_ended && expect.a_current == Some(true);
    let mut want = vec![];
    if !a_cleared { want.push(ids.a.0); }
    want.extend(ids.b.map(|id| id.0));
    if let Some(c) = ids.c && !(expect.c == Some('A') && a_cleared) { want.push(c.0); }
    want.sort();
    let mut have: Vec<u32> = app.shared.shortcuts.handlers.lock().unwrap().keys().map(|id| id.0).collect();
    have.sort();
    assert_eq!(have, want, "handlers: {at}");
}

const A_CHORD: &str = "ctrl+alt+super+f1";
const B_CHORD: &str = "ctrl+alt+super+f2";
const C_CHORD: &str = "ctrl+alt+super+f3";
const D_CHORD: &str = "ctrl+alt+super+f4";

#[test]
fn an_outgoing_close_leaves_a_newer_hosts_engine_and_handlers_alone() {
    use Close::*;
    let a: &[Close] = &[AClosesRouting, AReleasesEngine, AEnds];
    let b: &[Close] = &[BInstalls, BRoutes, BRegisters];
    // On Linux any thread reaches the engine, so another caller can register at any point.
    let lanes: Vec<&[Close]> = if cfg!(target_os = "linux") { vec![a, b, &[CRegisters]] } else { vec![a, b] };
    let (mut stale, mut current) = (false, false);
    for away in [false, true] {
        let replaces = cfg!(target_os = "linux") || !away;
        for order in interleavings(&lanes) {
            assert_eq!((live(0), live(1)), (0, 0), "the last walk let its backends go");
            let app = app("Successor");
            let handle = app.handle();
            let mut place = Place::new(away);
            let a_id = {
                let (shared, handle) = (app.shared.clone(), handle.clone());
                place.run(move |host| {
                    host.guard = Some(Guard::attach_with(&shared, Arc::new(|| {}), false, false, connect::<0>));
                    handle.register_shortcut(chord(A_CHORD), |state, _| *state += 1).unwrap()
                })
            };
            let mut expect = Expect { routed: Some('A'), ..Expect::default() };
            let mut ids = Ids { a: a_id, b: None, c: None };
            let (mut attaching, mut b) = (None, None);
            check(&app, &expect, &ids, replaces, "A attached");
            for (n, step) in order.iter().enumerate() {
                let at = format!("away {away}, {order:?} at {n}");
                match step {
                    AClosesRouting => {
                        place.run(a_closes_routing);
                        expect.routed = None;
                        expect.a_current = Some(!expect.b_installed);
                    }
                    AReleasesEngine => { place.run(a_releases_engine); expect.a_released = true; }
                    AEnds => { place.run(a_ends); expect.a_ended = true; }
                    BInstalls => {
                        attaching = Some(Guard::install(&app.shared, Arc::new(|| {}), false, false, connect::<1>));
                        expect.b_installed = true;
                    }
                    BRoutes => { b = Some(attaching.take().unwrap().route(&app.shared)); expect.routed = Some('B'); }
                    BRegisters => {
                        let answer = handle.register_shortcut(chord(B_CHORD), |state, _| *state += 10);
                        expect.b_registered = expect.routed == Some('B');
                        assert_eq!(answer.is_ok(), expect.b_registered, "B's registration: {at}: {answer:?}");
                        ids.b = answer.ok();
                    }
                    CRegisters => {
                        let answer = handle.register_shortcut(chord(C_CHORD), |state, _| *state += 100);
                        expect.c = expect.routed.map(|_| if expect.b_installed { 'B' } else { 'A' });
                        assert_eq!(answer.is_ok(), expect.c.is_some(), "another caller's registration: {at}: {answer:?}");
                        ids.c = answer.ok();
                    }
                }
                check(&app, &expect, &ids, replaces, &at);
            }
            let at = format!("away {away}, {order:?}");
            let fresh = handle.register_shortcut(chord(D_CHORD), |_, _| {});
            assert_eq!(fresh.is_ok(), expect.routed == Some('B'), "a call once A has gone reaches B: {at}: {fresh:?}");
            if fresh.is_ok() { assert_eq!(handle.shortcut_owner(&chord(D_CHORD)).as_deref(), Some("Successor"), "{at}"); }
            stale |= expect.a_current == Some(false);
            current |= expect.a_current == Some(true);
            drop(b);
            place.finish();
            assert!(app.shared.shortcuts.handlers.lock().unwrap().is_empty(), "B's close ends every handler: {at}");
            assert_eq!((live(0), live(1)), (0, 0), "every backend went: {at}");
        }
    }
    assert!(stale && current, "the walk closed A both before and after B installed");
}

/// Letters: 0 A's backend presses its chord; 1 B attaches and registers; 2 B's backend presses
/// its chord; 3 the queued updates are delivered.
#[test]
fn a_replaced_engines_activation_runs_nothing_once_a_newer_host_attached() {
    let mut replaced = false;
    for away in [false, true] {
        super::machine_tests::every_sequence(4, 5, |sequence| {
            let app = app("Pressed");
            let handle = app.handle();
            let mut place = Place::new(away);
            let a_id = {
                let (shared, handle) = (app.shared.clone(), handle.clone());
                place.run(move |host| {
                    host.guard = Some(Guard::attach_with(&shared, Arc::new(|| {}), false, false, connect::<2>));
                    handle.register_shortcut(chord("ctrl+alt+super+f5"), |state, _| *state += 1).unwrap()
                })
            };
            let a_sink = sink_of(2);
            let (mut b, mut b_sink) = (None, None);
            // The reference: activations wait in order, and one runs only if its host is the
            // newest when it is delivered.
            let (mut queued, mut want, mut state) = (vec![], 0, 0);
            for (n, letter) in sequence.iter().enumerate() {
                match letter {
                    0 => { a_sink(a_id, None); queued.push('A'); }
                    1 if b.is_none() => {
                        b = Some(Guard::attach_with(&app.shared, Arc::new(|| {}), false, false, connect::<3>));
                        let id = handle.register_shortcut(chord("ctrl+alt+super+f6"), |state, _| *state += 10).unwrap();
                        b_sink = Some((sink_of(3), id));
                    }
                    2 => if let Some((sink, id)) = &b_sink { sink(*id, None); queued.push('B'); },
                    3 => {
                        for f in app.shared.take().updates { f(&mut state, &mut Tx::default()); }
                        for host in queued.drain(..) {
                            match host {
                                'A' if b.is_none() => want += 1,
                                'A' => replaced = true,
                                _ => want += 10,
                            }
                        }
                    }
                    _ => {}
                }
                assert_eq!(state, want, "away {away}, {sequence:?} at {n}");
            }
            drop(b);
            place.finish();
            SINKS.lock().unwrap().retain(|(n, _)| !matches!(n, 2 | 3));
        });
    }
    assert!(replaced, "the walk delivered A's activation after B attached");
}

/// A caller off the owner thread waits for the newest host, B. A replaced host A on its own
/// thread drains before or after B: the call runs once, on B's engine.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_replaced_host_leaves_waiting_calls_to_the_newest_engine() {
    let held = |key: usize, shortcut: &Shortcut| ENGINES.with(|engines| engines.borrow().get(&key).is_some_and(|(_, engine)| engine.borrow().bindings.find(shortcut).is_some()));
    for a_first in [true, false] {
        let app = app("Newest");
        let handle = app.handle();
        let control = app.shared.shortcuts.control.clone();
        let key = control.key;
        let mut place = Place::new(true);
        {
            let shared = app.shared.clone();
            place.run(move |host| host.guard = Some(Guard::attach(&shared, Arc::new(|| {}), true, false)));
        }
        let (wake, woke) = mpsc::channel();
        let b = Guard::attach(&app.shared, Arc::new(move || { let _ = wake.send(()); }), true, false);
        let shortcut = chord("ctrl+alt+super+f7");
        let waiting = || matches!(&*control.attachment.lock().unwrap(), Attachment::Attached { jobs, .. } if jobs.len() == 1);
        let on_a = |place: &mut Place| { let shortcut = shortcut.clone(); place.run(move |_| held(key, &shortcut)) };

        let caller = { let (handle, shortcut) = (handle.clone(), shortcut.clone()); std::thread::spawn(move || handle.register_shortcut(shortcut, |_, _| {})) };
        woke.recv_timeout(PATIENCE).expect("the call woke B");
        assert!(waiting(), "the call waits for B");
        for drainer in if a_first { ['A', 'B'] } else { ['B', 'A'] } {
            let at = format!("A first {a_first}, after {drainer} drained");
            if drainer == 'A' { place.run(|host| host.guard.as_ref().unwrap().drain()); } else { b.drain(); }
            let ran = drainer == 'B' || !a_first;
            assert_eq!(waiting(), !ran, "the call waits until B drains: {at}");
            assert!(!on_a(&mut place), "A's engine never takes the call: {at}");
            assert_eq!(held(key, &shortcut), ran, "B's engine holds the chord once B drained: {at}");
        }
        assert!(caller.join().unwrap().is_ok());
        assert_eq!(handle.shortcut_owner(&shortcut).as_deref(), Some("Newest"));
        drop(b);
        place.finish();
    }
}
