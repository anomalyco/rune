//! One host acting on another host's work, through the real `Guard`, with each thread's steps
//! run one at a time and checked after every step. An outgoing close must not take a newer
//! engine or clear its handlers; an activation runs only on its own host's state, and not once
//! a newer host attached; and only the routed host runs the calls that wait for it, however the
//! hosts' installs and routes interleave.

use super::*;
use crate::{App, AppSpec, el};
use std::{sync::atomic::AtomicI32, time::Duration};

const PATIENCE: Duration = Duration::from_secs(30);

fn app(title: &'static str) -> App<u32> { App::new(AppSpec::new(title, (10.0, 10.0), || 0, |_: &u32, _| el())) }

fn chord(text: &str) -> Shortcut { text.parse().unwrap() }

/// Backends each engine opened and hasn't dropped, by the index its `connect` names. The close
/// walk uses 0 and 1, the activation walk 2 and 3, the reversed routes 4 and 5.
static LIVE: [AtomicI32; 6] = [const { AtomicI32::new(0) }; 6];
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

/// A host's guard, its attach or close in progress, and its own state, on the thread the guard
/// belongs to.
#[derive(Default)]
struct Host {
    guard: Option<Guard>,
    // Only the reversed routes, off Linux, attach in two steps on a host's own thread.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    attaching: Option<Attaching>,
    closing: Option<Closing>,
    state: u32,
    held: Vec<crate::app::Update<u32>>,
}

impl Host {
    /// Runs the App's queued updates on this host's state, as its event loop does.
    fn deliver(&mut self, shared: &Shared<u32>) -> u32 {
        self.take(shared);
        self.run_held()
    }

    /// Takes the App's queued updates, as an event loop's drain starts by doing.
    fn take(&mut self, shared: &Shared<u32>) { self.held.extend(shared.take().updates); }

    /// Runs the updates it took on its own state.
    fn run_held(&mut self) -> u32 {
        let _receiving = self.guard.as_ref().unwrap().receiving();
        for f in std::mem::take(&mut self.held) { f(&mut self.state, &mut Tx::default()); }
        self.state
    }
}

/// Whether this thread's engine for the App keyed `key` holds `shortcut`.
#[cfg(not(target_os = "linux"))]
fn held(key: usize, shortcut: &Shortcut) -> bool {
    ENGINES.with(|engines| engines.borrow().get(&key).is_some_and(|(_, engine)| engine.borrow().bindings.find(shortcut).is_some()))
}

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
            Self::Away(..) => self.start(step).recv_timeout(PATIENCE).expect("the host's thread finished the step"),
        }
    }

    /// Starts a step on the host's own thread without waiting for it.
    fn start<T: Send + 'static>(&self, step: impl FnOnce(&mut Host) -> T + Send + 'static) -> mpsc::Receiver<T> {
        let (answer, answered) = mpsc::channel();
        self.sender().send(Box::new(move |host| { let _ = answer.send(step(host)); })).unwrap();
        answered
    }

    fn sender(&self) -> mpsc::Sender<Step> {
        let Self::Away(send, _) = self else { panic!("only a host on its own thread takes steps") };
        send.clone()
    }

    /// Drops whatever the host still holds, on its own thread, then ends the thread. A wake can
    /// hold a way to send the thread steps, so the host goes before the thread is asked to end.
    fn finish(mut self) {
        if let Self::Away(..) = self { self.run(|host| drop(std::mem::take(host))); }
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
    b_routed: bool,
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
                        // B is the newest once it installs on Linux, and once it routes elsewhere.
                        expect.a_current = Some(!if cfg!(target_os = "linux") { expect.b_installed } else { expect.b_routed });
                    }
                    AReleasesEngine => { place.run(a_releases_engine); expect.a_released = true; }
                    AEnds => { place.run(a_ends); expect.a_ended = true; }
                    BInstalls => {
                        attaching = Some(Guard::install(&app.shared, Arc::new(|| {}), false, false, connect::<1>));
                        expect.b_installed = true;
                    }
                    BRoutes => { b = Some(attaching.take().unwrap().route(&app.shared)); expect.routed = Some('B'); expect.b_routed = true; }
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

/// Two hosts of one App, each with its own state, share the App's update queue. Letters: 0 A's
/// backend presses its chord; 1 B attaches and registers; 2 B's backend presses its chord; 3 A
/// delivers the queue on A's state; 4 B delivers it on B's state.
#[test]
fn an_activation_runs_only_on_its_own_hosts_state_while_that_host_is_newest() {
    let (mut replaced, mut left) = (false, false);
    for away in [false, true] {
        super::machine_tests::every_sequence(5, 5, |sequence| {
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
            let b_woken = Arc::new(AtomicI32::new(0));
            let (mut b, mut b_sink) = (None::<Host>, None);
            // The reference: activations wait in order. One runs only on its own host's state,
            // and only while that host is the newest; another host leaves it, and wakes its host.
            let (mut queued, mut want_a, mut want_b) = (vec![], 0, 0);
            for (n, letter) in sequence.iter().enumerate() {
                let at = format!("away {away}, {sequence:?} at {n}");
                match letter {
                    0 => { a_sink(a_id, None); queued.push('A'); }
                    1 if b.is_none() => {
                        let woken = b_woken.clone();
                        let guard = Guard::attach_with(&app.shared, Arc::new(move || { woken.fetch_add(1, Ordering::Relaxed); }), false, false, connect::<3>);
                        b = Some(Host { guard: Some(guard), ..Host::default() });
                        let id = handle.register_shortcut(chord("ctrl+alt+super+f6"), |state, _| *state += 10).unwrap();
                        b_sink = Some((sink_of(3), id));
                    }
                    2 => if let Some((sink, id)) = &b_sink { sink(*id, None); queued.push('B'); },
                    3 => {
                        for &host in &queued {
                            if host == 'A' && b.is_none() { want_a += 1; }
                            replaced |= host == 'A' && b.is_some();
                        }
                        queued.retain(|&host| host == 'B');
                        left |= !queued.is_empty();
                        let (shared, woken) = (app.shared.clone(), b_woken.load(Ordering::Relaxed));
                        place.run(move |host| host.deliver(&shared));
                        assert_eq!(b_woken.load(Ordering::Relaxed) - woken, queued.len() as i32, "A woke B for each activation it left: {at}");
                    }
                    4 => if let Some(b) = &mut b {
                        want_b += 10 * queued.iter().filter(|&&host| host == 'B').count() as u32;
                        queued.clear();
                        b.deliver(&app.shared);
                    },
                    _ => {}
                }
                assert_eq!(place.run(|host| host.state), want_a, "A's state: {at}");
                assert_eq!(b.as_ref().map_or(0, |b| b.state), want_b, "B's state: {at}");
            }
            drop(b);
            place.finish();
            SINKS.lock().unwrap().retain(|(n, _)| !matches!(n, 2 | 3));
        });
    }
    assert!(replaced, "the walk delivered A's activation after B attached");
    assert!(left, "the walk had A deliver B's activation before B did");
}

/// A replaced host A stays alive on its own thread, and its engine's sink keeps taking presses.
/// Whichever host delivers them, each is discarded, so A's presses never pile up.
#[test]
fn a_replaced_engines_presses_are_discarded_not_kept() {
    let app = app("Replaced");
    let mut a = Place::new(true);
    let (generation, live) = {
        let shared = app.shared.clone();
        a.run(move |host| {
            let guard = Guard::attach(&shared, Arc::new(|| {}), true, false);
            let seen = (guard.generation, Arc::downgrade(&guard._live));
            host.guard = Some(guard);
            seen
        })
    };
    // A's engine's sink, as `install` builds it.
    let delivery = Arc::new(Delivery { shared: Arc::downgrade(&app.shared), generation, wake: Arc::new(|| {}), live, pressed: Mutex::default() });
    let press = sink(delivery.clone());
    let mut b = Host { guard: Some(Guard::attach(&app.shared, Arc::new(|| {}), true, false)), ..Host::default() };
    for n in 0..50 {
        press(ShortcutId(u32::MAX - 3), None);
        if n % 2 == 0 { b.deliver(&app.shared); } else { let shared = app.shared.clone(); a.run(move |host| { host.deliver(&shared); }); }
        assert_eq!(delivery.pressed.lock().unwrap().len(), 0, "press {n} was discarded");
        assert!(app.shared.take().updates.is_empty(), "nothing waits for another host: press {n}");
    }
    assert_eq!((a.run(|host| host.state), b.state), (0, 0), "no press ran");
    drop(b);
    a.finish();
}

/// A caller off the owner thread waits for the newest host, B. A replaced host A on its own
/// thread drains before or after B: the call runs once, on B's engine.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_replaced_host_leaves_waiting_calls_to_the_newest_engine() {
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

/// Two owner threads attach, and their routes finish in the reverse of their installs: B
/// installs after A but routes first, so routing ends on A. A waiting call wakes A, and that wake
/// alone drives A's drain: the call runs on A's engine, and A's chord reaches A's state.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_host_that_installed_first_but_routed_last_runs_its_calls_and_activations() {
    let app = app("Reversed");
    let handle = app.handle();
    let key = app.shared.shortcuts.control.key;
    let (mut a, mut b) = (Place::new(true), Place::new(true));
    let connects: [Connect; 2] = [connect::<4>, connect::<5>];
    for (place, connect) in [(&mut a, connects[0]), (&mut b, connects[1])] {
        // The host's wake asks its own thread to drain, as its event loop does.
        let send = place.sender();
        let wake: Waker = Arc::new(move || { let _ = send.send(Box::new(|host: &mut Host| if let Some(guard) = &host.guard { guard.drain() })); });
        let shared = app.shared.clone();
        place.run(move |host| host.attaching = Some(Guard::install(&shared, wake, false, false, connect)));
    }
    for place in [&mut b, &mut a] {
        let shared = app.shared.clone();
        place.run(move |host| host.guard = Some(host.attaching.take().unwrap().route(&shared)));
    }
    let shortcut = chord("ctrl+alt+super+f8");
    let (answer, answered) = mpsc::channel();
    {
        let (handle, shortcut) = (handle.clone(), shortcut.clone());
        std::thread::spawn(move || { let _ = answer.send(handle.register_shortcut(shortcut, |state, _| *state += 1)); });
    }
    let id = answered.recv_timeout(PATIENCE).expect("A's wake drained the waiting call").unwrap();
    let on = |place: &mut Place| { let shortcut = shortcut.clone(); place.run(move |_| held(key, &shortcut)) };
    assert!(on(&mut a), "A's engine ran the call");
    assert!(!on(&mut b), "B's engine never took it");
    sink_of(4)(id, None);
    let shared = app.shared.clone();
    assert_eq!(a.run(move |host| host.deliver(&shared)), 1, "A's chord reaches A's state");
    // A's sink holds A's wake, which can send A's thread steps, so it goes before the thread.
    SINKS.lock().unwrap().retain(|(n, _)| !matches!(n, 4 | 5));
    a.finish();
    b.finish();
    assert_eq!((live(4), live(5)), (0, 0), "both backends went");
}

/// The routed host A starts to drain a waiting call and pauses just before it takes the calls.
/// B attaches meanwhile, so the call is B's: A resumes and takes nothing, and B runs it.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_host_paused_before_it_takes_the_calls_leaves_them_to_the_host_routed_meanwhile() {
    let app = app("Paused");
    let handle = app.handle();
    let control = app.shared.shortcuts.control.clone();
    let key = control.key;
    let mut a = Place::new(true);
    let (a_woken, a_woke) = mpsc::channel();
    {
        let shared = app.shared.clone();
        a.run(move |host| host.guard = Some(Guard::attach(&shared, Arc::new(move || { let _ = a_woken.send(()); }), true, false)));
    }
    let shortcut = chord("ctrl+alt+super+f9");
    let caller = { let (handle, shortcut) = (handle.clone(), shortcut.clone()); std::thread::spawn(move || handle.register_shortcut(shortcut, |_, _| {})) };
    a_woke.recv_timeout(PATIENCE).expect("the call woke A");
    let waiting = || matches!(&*control.attachment.lock().unwrap(), Attachment::Attached { jobs, .. } if jobs.len() == 1);
    assert!(waiting(), "the call waits for A");

    let (paused, pauses) = mpsc::channel();
    let (resume, resumes) = mpsc::channel::<()>();
    let drained = a.start(move |host| {
        BEFORE_DEQUEUE.with(|hook| *hook.borrow_mut() = Some(Box::new(move || { paused.send(()).unwrap(); resumes.recv().unwrap(); })));
        host.guard.as_ref().unwrap().drain();
    });
    pauses.recv_timeout(PATIENCE).expect("A paused before it took the calls");
    let (b_woken, b_woke) = mpsc::channel();
    let b = Guard::attach(&app.shared, Arc::new(move || { let _ = b_woken.send(()); }), true, false);
    resume.send(()).unwrap();
    drained.recv_timeout(PATIENCE).expect("A's drain finished");

    let on_a = { let shortcut = shortcut.clone(); a.run(move |_| held(key, &shortcut)) };
    assert!(!on_a, "A's engine never ran B's call");
    assert!(waiting(), "the call waits for B");
    b_woke.recv_timeout(PATIENCE).expect("B was woken for the call it took over");
    b.drain();
    assert!(caller.join().unwrap().is_ok());
    assert!(held(key, &shortcut), "B's engine ran it");
    drop(b);
    a.finish();
}

/// A caller waits for A, and only A is woken. B takes routing, and the call with it, before A
/// drains, so A's drain takes nothing. Each host drains only when its own wake asks: B's
/// attach wakes B for the call it took over, and B runs it.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_host_that_takes_over_a_waiting_call_is_woken_for_it() {
    let app = app("Inherited");
    let handle = app.handle();
    let key = app.shared.shortcuts.control.key;
    let (mut a, mut b) = (Place::new(true), Place::new(true));
    // A's wake is only noted: A drains when the test says it answers it.
    let (a_woken, a_woke) = mpsc::channel();
    {
        let shared = app.shared.clone();
        a.run(move |host| host.guard = Some(Guard::attach(&shared, Arc::new(move || { let _ = a_woken.send(()); }), true, false)));
    }
    let shortcut = chord("ctrl+alt+super+f10");
    let (answer, answered) = mpsc::channel();
    {
        let (handle, shortcut) = (handle.clone(), shortcut.clone());
        std::thread::spawn(move || { let _ = answer.send(handle.register_shortcut(shortcut, |_, _| {})); });
    }
    a_woke.recv_timeout(PATIENCE).expect("the call woke A");
    // B's wake asks B's thread to drain, as its event loop does.
    let send = b.sender();
    let wake: Waker = Arc::new(move || { let _ = send.send(Box::new(|host: &mut Host| if let Some(guard) = &host.guard { guard.drain() })); });
    {
        let shared = app.shared.clone();
        b.run(move |host| host.guard = Some(Guard::attach(&shared, wake, true, false)));
    }
    // A answers its wake, after B took the call over.
    a.run(|host| host.guard.as_ref().unwrap().drain());
    let answer = answered.recv_timeout(PATIENCE).expect("B's own wake drained the call it took over");
    assert!(answer.is_ok(), "{answer:?}");
    let on = |place: &mut Place| { let shortcut = shortcut.clone(); place.run(move |_| held(key, &shortcut)) };
    assert!(!on(&mut a), "A's engine never took the call");
    assert!(on(&mut b), "B's engine ran it");
    // B's wake, held by routing, can send B's thread steps, so B closes first.
    b.finish();
    a.finish();
}

#[derive(Clone, Copy, Debug)]
enum Press { X, Y, ATakes, ARuns, BDelivers }

/// B is the newest host throughout, and presses X, then Y, whose handlers don't commute. A
/// replaced host on its own thread takes the App's updates in one step and runs them in a later
/// one, and B delivers at any point: B runs X, then Y, whoever took them first.
#[test]
fn a_hosts_presses_run_in_the_order_they_came_whoever_takes_them_first() {
    use Press::*;
    let lanes: [&[Press]; 3] = [&[X, Y], &[ATakes, ARuns], &[BDelivers]];
    for order in interleavings(&lanes) {
        let app = app("Ordered");
        let handle = app.handle();
        let mut a = Place::new(true);
        {
            let shared = app.shared.clone();
            a.run(move |host| host.guard = Some(Guard::attach(&shared, Arc::new(|| {}), true, false)));
        }
        let mut b = Host { guard: Some(Guard::attach(&app.shared, Arc::new(|| {}), true, false)), ..Host::default() };
        let (x, y) = (chord("ctrl+alt+super+f11"), chord("ctrl+alt+super+f12"));
        handle.register_shortcut(x.clone(), |state, _| *state = *state * 10 + 1).unwrap();
        handle.register_shortcut(y.clone(), |state, _| *state = *state * 10 + 2).unwrap();
        for (n, step) in order.iter().enumerate() {
            let at = format!("{order:?} at {n}");
            match step {
                X => app.shared.shortcuts.fire(&x).unwrap(),
                Y => app.shared.shortcuts.fire(&y).unwrap(),
                ATakes => { let shared = app.shared.clone(); a.run(move |host| host.take(&shared)); }
                ARuns => { a.run(|host| host.run_held()); }
                BDelivers => { b.deliver(&app.shared); }
            }
            assert_eq!(a.run(|host| host.state), 0, "A runs none of B's presses: {at}");
            assert!([0, 1, 12].contains(&b.state), "B ran its presses in order: {at}: {}", b.state);
        }
        assert_eq!(b.deliver(&app.shared), 12, "B ran X, then Y: {order:?}");
        drop(b);
        a.finish();
    }
}
