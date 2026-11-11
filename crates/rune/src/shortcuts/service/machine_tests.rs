//! Every short sequence of events through the machines, compared after each event with a
//! literal copy of the code they replaced. Both platform shapes run on every host, and a
//! teardown keeps running until the sequence says it finished.

use super::*;
use std::{cell::RefCell, sync::LazyLock, thread::ThreadId};

fn other_thread() -> ThreadId { std::thread::spawn(|| std::thread::current().id()).join().unwrap() }

fn address(wake: &Waker) -> usize { Arc::as_ptr(wake) as *const () as usize }

/// The sequences of `length` letters drawn from `0..letters`, each passed to `run`.
fn every_sequence(letters: usize, length: u32, mut run: impl FnMut(&[usize])) {
    let mut sequence = vec![0; length as usize];
    for code in 0..letters.pow(length) {
        let mut rest = code;
        for letter in &mut sequence { *letter = rest % letters; rest /= letters; }
        run(&sequence);
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Outcome {
    Attached,
    /// The attach replaced this engine, whose teardown is now running.
    Replacing { engine: u32 },
    Refused,
    Ran { engine: u32, wake: usize },
    Queued { wake: usize },
    Drained { ran: Vec<u32>, dropped: Vec<u32> },
    Closed { jobs: Vec<u32>, engine: Option<u32> },
    /// Parked behind a running teardown, to finish when it does.
    Blocked,
    /// That thread is inside a teardown or parked, so it can't do anything else.
    Skipped,
}

enum Op { Attach(Waker, u32), Call(u32), Drain, Close, ReleaseEngine }

/// Teardowns still running, oldest first, each with the wake of the attach it holds up, and the
/// operations parked behind them.
#[derive(Default)]
struct Schedule { teardowns: Vec<(ThreadId, Option<Waker>)>, parked: Vec<(ThreadId, Op)> }

impl Schedule {
    fn busy(&self, thread: ThreadId) -> bool {
        self.teardowns.iter().any(|(t, _)| *t == thread) || self.parked.iter().any(|(t, _)| *t == thread)
    }

    fn view(&self) -> (Vec<ThreadId>, Vec<ThreadId>) {
        (self.teardowns.iter().map(|(t, _)| *t).collect(), self.parked.iter().map(|(t, _)| *t).collect())
    }
}

/// Who owns the service, its wake, its waiting jobs, the Linux engine and the per-thread ones.
type RouteView = (Option<ThreadId>, Option<usize>, Vec<u32>, Option<u32>, Vec<(ThreadId, u32)>);

fn sorted(engines: &HashMap<ThreadId, u32>) -> Vec<(ThreadId, u32)> {
    let mut engines: Vec<_> = engines.iter().map(|(thread, engine)| (*thread, *engine)).collect();
    engines.sort_by_key(|(_, engine)| *engine);
    engines
}

/// One side of the comparison, split where the real code can block.
trait Model {
    fn schedule(&mut self) -> &mut Schedule;
    fn attach(&mut self, thread: ThreadId, wake: Waker, engine: u32) -> Outcome;
    /// The routing half of an attach, which runs once any replaced engine is gone.
    fn finish_attach(&mut self, thread: ThreadId, wake: Waker);
    fn call(&mut self, thread: ThreadId, job: u32) -> Outcome;
    fn drain(&mut self, thread: ThreadId) -> Outcome;
    fn close(&mut self, thread: ThreadId) -> Outcome;
    /// The engine half of a close on Linux.
    fn release_engine(&mut self, thread: ThreadId) -> Outcome;
    fn view(&self) -> RouteView;
}

fn perform(model: &mut impl Model, thread: ThreadId, op: Op) -> Outcome {
    match op {
        Op::Attach(wake, engine) => model.attach(thread, wake, engine),
        Op::Call(job) => model.call(thread, job),
        Op::Drain => model.drain(thread),
        Op::Close => model.close(thread),
        Op::ReleaseEngine => model.release_engine(thread),
    }
}

/// The oldest teardown ends. A parked caller holds the routing lock, so it runs before the
/// attach that teardown held up can finish.
fn finish_teardown(model: &mut impl Model) -> Vec<Outcome> {
    let schedule = model.schedule();
    if schedule.teardowns.is_empty() { return vec![Outcome::Skipped]; }
    let (thread, attaching) = schedule.teardowns.remove(0);
    let (calls, others): (Vec<_>, Vec<_>) = std::mem::take(&mut schedule.parked).into_iter().partition(|(_, op)| matches!(op, Op::Call(_)));
    let mut outcomes: Vec<_> = calls.into_iter().map(|(thread, op)| perform(model, thread, op)).collect();
    if let Some(wake) = attaching { model.finish_attach(thread, wake); }
    outcomes.extend(others.into_iter().map(|(thread, op)| perform(model, thread, op)));
    outcomes
}

// The routing as it was: Routing's fields, the Linux engine slot and the thread-local engines,
// with the current thread passed in and a job as a number. A running teardown on Linux holds
// the slot's lock.
#[derive(Default)]
struct OldRouting { owner: Option<ThreadId>, wake: Option<Waker>, jobs: Vec<u32> }

#[derive(Default)]
struct OldControl { routing: OldRouting, engine: Option<u32>, engines: HashMap<ThreadId, u32>, linux: bool, schedule: Schedule }

impl OldControl {
    fn slot_locked(&self) -> bool { self.linux && !self.schedule.teardowns.is_empty() }
}

impl Model for OldControl {
    fn schedule(&mut self) -> &mut Schedule { &mut self.schedule }

    fn attach(&mut self, thread: ThreadId, wake: Waker, engine: u32) -> Outcome {
        if self.slot_locked() { self.schedule.parked.push((thread, Op::Attach(wake, engine))); return Outcome::Blocked; }
        let replaced = if self.linux { self.engine.replace(engine) } else { self.engines.insert(thread, engine) };
        if let Some(engine) = replaced {
            self.schedule.teardowns.push((thread, Some(wake)));
            return Outcome::Replacing { engine };
        }
        self.finish_attach(thread, wake);
        Outcome::Attached
    }

    fn finish_attach(&mut self, thread: ThreadId, wake: Waker) {
        self.routing.owner = Some(thread);
        self.routing.wake = Some(wake);
    }

    fn call(&mut self, thread: ThreadId, job: u32) -> Outcome {
        if self.linux {
            if self.routing.owner.is_none() { return Outcome::Refused; }
            if self.slot_locked() { self.schedule.parked.push((thread, Op::Call(job))); return Outcome::Blocked; }
            let Some(engine) = self.engine else { return Outcome::Refused };
            let wake = self.routing.wake.clone();
            return Outcome::Ran { engine, wake: address(&wake.unwrap()) };
        }
        let route = &mut self.routing;
        let Some(owner) = route.owner else { return Outcome::Refused };
        if owner == thread {
            let wake = route.wake.clone();
            let Some(engine) = self.engines.get(&thread).copied() else { return Outcome::Refused };
            return Outcome::Ran { engine, wake: address(&wake.unwrap()) };
        }
        route.jobs.push(job);
        let wake = route.wake.clone();
        Outcome::Queued { wake: address(&wake.unwrap()) }
    }

    fn drain(&mut self, thread: ThreadId) -> Outcome {
        if self.linux { return Outcome::Drained { ran: vec![], dropped: vec![] }; }
        let jobs = std::mem::take(&mut self.routing.jobs);
        if self.engines.contains_key(&thread) { Outcome::Drained { ran: jobs, dropped: vec![] } } else { Outcome::Drained { ran: vec![], dropped: jobs } }
    }

    fn close(&mut self, thread: ThreadId) -> Outcome {
        let route = std::mem::take(&mut self.routing);
        if self.linux {
            if self.slot_locked() { self.schedule.parked.push((thread, Op::ReleaseEngine)); return Outcome::Blocked; }
            return self.release_engine(thread);
        }
        let engine = self.engines.remove(&thread);
        if engine.is_some() { self.schedule.teardowns.push((thread, None)); }
        Outcome::Closed { jobs: route.jobs, engine }
    }

    fn release_engine(&mut self, thread: ThreadId) -> Outcome {
        if self.slot_locked() { self.schedule.parked.push((thread, Op::ReleaseEngine)); return Outcome::Blocked; }
        let engine = self.engine.take();
        if engine.is_some() { self.schedule.teardowns.push((thread, None)); }
        Outcome::Closed { jobs: vec![], engine }
    }

    fn view(&self) -> RouteView {
        (self.routing.owner, self.routing.wake.as_ref().map(address), self.routing.jobs.clone(), self.engine, sorted(&self.engines))
    }
}

/// The machine with the glue `Guard` and `Control` wrap around it.
struct NewControl { attachment: Attachment<u32>, engine: Option<u32>, engines: HashMap<ThreadId, u32>, linux: bool, schedule: Schedule }

impl NewControl {
    fn slot_locked(&self) -> bool { self.linux && !self.schedule.teardowns.is_empty() }
}

impl Model for NewControl {
    fn schedule(&mut self) -> &mut Schedule { &mut self.schedule }

    fn attach(&mut self, thread: ThreadId, wake: Waker, engine: u32) -> Outcome {
        if self.slot_locked() { self.schedule.parked.push((thread, Op::Attach(wake, engine))); return Outcome::Blocked; }
        let replaced = if self.linux { self.engine.replace(engine) } else { self.engines.insert(thread, engine) };
        if let Some(engine) = replaced {
            self.schedule.teardowns.push((thread, Some(wake)));
            return Outcome::Replacing { engine };
        }
        self.finish_attach(thread, wake);
        Outcome::Attached
    }

    fn finish_attach(&mut self, thread: ThreadId, wake: Waker) {
        assert!(step_attachment(&mut self.attachment, AttachEvent::Attach { owner: thread, wake }).is_empty());
    }

    fn call(&mut self, thread: ThreadId, job: u32) -> Outcome {
        match step_attachment(&mut self.attachment, AttachEvent::Call { from: thread, shared: self.linux }).into_iter().next() {
            Some(AttachEffect::Run { wake }) => {
                if self.slot_locked() { self.schedule.parked.push((thread, Op::Call(job))); return Outcome::Blocked; }
                let engine = if self.linux { self.engine } else { self.engines.get(&thread).copied() };
                engine.map_or(Outcome::Refused, |engine| Outcome::Ran { engine, wake: address(&wake) })
            }
            Some(AttachEffect::Wait) => match step_attachment(&mut self.attachment, AttachEvent::Post(job)).into_iter().next() {
                Some(AttachEffect::Wake(wake)) => Outcome::Queued { wake: address(&wake) },
                _ => panic!("a posted job wakes the owner"),
            },
            _ => Outcome::Refused,
        }
    }

    fn drain(&mut self, thread: ThreadId) -> Outcome {
        if self.linux { return Outcome::Drained { ran: vec![], dropped: vec![] }; }
        let Some(AttachEffect::Jobs(jobs)) = step_attachment(&mut self.attachment, AttachEvent::Drain).into_iter().next() else {
            return Outcome::Drained { ran: vec![], dropped: vec![] };
        };
        if self.engines.contains_key(&thread) { Outcome::Drained { ran: jobs, dropped: vec![] } } else { Outcome::Drained { ran: vec![], dropped: jobs } }
    }

    fn close(&mut self, thread: ThreadId) -> Outcome {
        let mut released = vec![];
        for effect in step_attachment(&mut self.attachment, AttachEvent::Close) {
            if let AttachEffect::Release(jobs) = effect { released = jobs; }
        }
        if self.linux { return self.release_engine(thread); }
        let engine = self.engines.remove(&thread);
        if engine.is_some() { self.schedule.teardowns.push((thread, None)); }
        Outcome::Closed { jobs: released, engine }
    }

    fn release_engine(&mut self, thread: ThreadId) -> Outcome {
        if self.slot_locked() { self.schedule.parked.push((thread, Op::ReleaseEngine)); return Outcome::Blocked; }
        let engine = self.engine.take();
        if engine.is_some() { self.schedule.teardowns.push((thread, None)); }
        Outcome::Closed { jobs: vec![], engine }
    }

    fn view(&self) -> RouteView {
        let engines = sorted(&self.engines);
        match &self.attachment {
            Attachment::Attached { owner, wake, jobs } => (Some(*owner), Some(address(wake)), jobs.clone(), self.engine, engines),
            Attachment::Detached | Attachment::Closed => (None, None, vec![], self.engine, engines),
        }
    }
}

/// Letters 0..8 are Attach, Call, Drain and Close from either thread; 8 ends the oldest teardown.
fn play(model: &mut impl Model, letter: usize, threads: &[ThreadId; 2], wakes: &[Waker; 2], number: u32) -> Vec<Outcome> {
    if letter == 8 { return finish_teardown(model); }
    let thread = threads[letter % 2];
    if model.schedule().busy(thread) { return vec![Outcome::Skipped]; }
    let op = match letter / 2 {
        0 => Op::Attach(wakes[letter % 2].clone(), number),
        1 => Op::Call(number),
        2 => Op::Drain,
        _ => Op::Close,
    };
    vec![perform(model, thread, op)]
}

#[test]
fn every_attachment_sequence_matches_the_old_routing() {
    let threads = [std::thread::current().id(), other_thread()];
    let wakes: [Waker; 2] = [Arc::new(|| {}), Arc::new(|| { let _ = 1; })];
    for linux in [false, true] {
        every_sequence(9, 6, |sequence| {
            let mut old = OldControl { linux, ..OldControl::default() };
            let mut new = NewControl { attachment: Attachment::Detached, engine: None, engines: HashMap::new(), linux, schedule: Schedule::default() };
            for (n, letter) in sequence.iter().enumerate() {
                let number = n as u32 + 1;
                let expected = play(&mut old, *letter, &threads, &wakes, number);
                let actual = play(&mut new, *letter, &threads, &wakes, number);
                assert_eq!(actual, expected, "linux {linux}, {sequence:?} at {n}");
                assert_eq!(new.view(), old.view(), "linux {linux}, {sequence:?} at {n}");
                assert_eq!(new.schedule.view(), old.schedule.view(), "linux {linux}, {sequence:?} at {n}");
                if linux && actual.iter().any(|outcome| matches!(outcome, Outcome::Ran { .. })) {
                    assert!(new.schedule.teardowns.is_empty(), "a call reached an engine during a teardown: {sequence:?} at {n}");
                }
            }
        });
    }
}

#[test]
fn only_a_close_reaches_closed_and_a_later_host_can_attach() {
    let thread = std::thread::current().id();
    let mut state = Attachment::<u32>::Detached;
    assert!(matches!(step_attachment(&mut state, AttachEvent::Call { from: thread, shared: true }).into_iter().next(), Some(AttachEffect::Refuse)));
    assert!(matches!(state, Attachment::Detached));
    step_attachment(&mut state, AttachEvent::Attach { owner: thread, wake: Arc::new(|| {}) });
    assert!(matches!(state, Attachment::Attached { .. }));
    step_attachment(&mut state, AttachEvent::Close);
    assert!(matches!(state, Attachment::Closed));
    assert!(matches!(step_attachment(&mut state, AttachEvent::Call { from: thread, shared: true }).into_iter().next(), Some(AttachEffect::Refuse)));
    assert!(step_attachment(&mut state, AttachEvent::Attach { owner: thread, wake: Arc::new(|| {}) }).is_empty());
    assert!(matches!(state, Attachment::Attached { .. }));
}

// The engine as it was, with `super::backend` behind a field and its own process-wide list.
static OLD_NATIVE: Mutex<Vec<(ShortcutId, Shortcut, &'static str)>> = Mutex::new(Vec::new());

struct OldEngine {
    title: &'static str,
    headless: bool,
    wayland: bool,
    sink: Sink,
    connect: Connect,
    native: Option<NativeBackend>,
    registrations: HashMap<ShortcutId, Shortcut>,
}

impl OldEngine {
    fn register(&mut self, id: ShortcutId, shortcut: Shortcut) -> Result<(), ShortcutError> {
        if self.registrations.values().any(|s| s == &shortcut) { return Err(ShortcutError::Conflict); }
        if !self.headless {
            // Another App in this process holds it; some backends (the portal) wouldn't refuse.
            if OLD_NATIVE.lock().unwrap().iter().any(|(_, s, _)| s == &shortcut) { return Err(ShortcutError::Conflict); }
            if self.native.is_none() { self.native = Some((self.connect)(self.wayland, self.sink.clone())?); }
            if let Err(error) = self.native.as_mut().unwrap().register(id, &shortcut) {
                if self.registrations.is_empty() { self.native = None; }
                return Err(error);
            }
        }
        if !self.headless { OLD_NATIVE.lock().unwrap().push((id, shortcut.clone(), self.title)); }
        self.registrations.insert(id, shortcut);
        Ok(())
    }

    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> {
        if !self.registrations.contains_key(&id) { return Err(ShortcutError::UnknownId); }
        if let Some(native) = self.native.as_mut() { native.unregister(id)?; }
        self.registrations.remove(&id);
        OLD_NATIVE.lock().unwrap().retain(|(native, ..)| *native != id);
        // Drop the connection/listener when the last registration goes away.
        if self.registrations.is_empty() { self.native = None; }
        Ok(())
    }

    fn owner(&self, shortcut: &Shortcut) -> Option<String> {
        if self.registrations.values().any(|s| s == shortcut) { return Some(self.title.into()); }
        if self.headless { return None; }
        let other = OLD_NATIVE.lock().unwrap().iter().find(|(_, s, _)| s == shortcut).map(|(.., title)| (*title).into());
        other.or_else(|| super::super::system_owner(shortcut))
    }

    fn release(&mut self) {
        self.native = None;
        OLD_NATIVE.lock().unwrap().retain(|(id, ..)| !self.registrations.contains_key(id));
        self.registrations.clear();
    }
}

impl Drop for OldEngine {
    fn drop(&mut self) { self.release(); }
}

const X: &str = "ctrl+alt+shift+f20";
const Y: &str = "ctrl+alt+shift+f21";

static CHORDS: LazyLock<[Shortcut; 2]> = LazyLock::new(|| [chord(X), chord(Y)]);

fn chord(text: &str) -> Shortcut { text.parse().unwrap() }

/// Claims on the chords these tests use, in order. Other tests share `NATIVE`.
fn claims(native: &Mutex<Vec<(ShortcutId, Shortcut, &'static str)>>) -> Vec<(u32, &'static str)> {
    native.lock().unwrap().iter().filter(|(_, s, _)| CHORDS.contains(s)).map(|(id, _, title)| (id.0, *title)).collect()
}

#[derive(Clone, Debug, PartialEq)]
enum Note {
    Connect { wayland: bool },
    Register(u32),
    Unregister(u32),
    /// A backend closed while this many claims stood.
    Closed(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Side { Old, New }

struct Rig { connect: Result<(), ShortcutError>, register: Result<(), ShortcutError>, unregister: Result<(), ShortcutError>, old: Vec<Note>, new: Vec<Note> }

impl Rig {
    fn note(&mut self, side: Side, note: Note) { if side == Side::Old { self.old.push(note) } else { self.new.push(note) } }
}

thread_local! {
    static RIG: RefCell<Rig> = RefCell::new(Rig { connect: Ok(()), register: Ok(()), unregister: Ok(()), old: vec![], new: vec![] });
}

struct Scripted(Side);

impl Backend for Scripted {
    fn register(&mut self, id: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> {
        RIG.with(|rig| { let mut rig = rig.borrow_mut(); rig.note(self.0, Note::Register(id.0)); rig.register.clone() })
    }
    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> {
        RIG.with(|rig| { let mut rig = rig.borrow_mut(); rig.note(self.0, Note::Unregister(id.0)); rig.unregister.clone() })
    }
}

impl Drop for Scripted {
    fn drop(&mut self) {
        let standing = claims(if self.0 == Side::Old { &OLD_NATIVE } else { &NATIVE }).len();
        RIG.with(|rig| rig.borrow_mut().note(self.0, Note::Closed(standing)));
    }
}

fn connect(side: Side, wayland: bool) -> Result<NativeBackend, ShortcutError> {
    RIG.with(|rig| { let mut rig = rig.borrow_mut(); rig.note(side, Note::Connect { wayland }); rig.connect.clone() })?;
    Ok(Box::new(Scripted(side)))
}

fn connect_old(wayland: bool, _: Sink) -> Result<NativeBackend, ShortcutError> { connect(Side::Old, wayland) }

fn connect_new(wayland: bool, _: Sink) -> Result<NativeBackend, ShortcutError> { connect(Side::New, wayland) }

#[derive(Clone, Copy, Debug)]
enum Start { Headless, NoBackend, EmptyBackend }

fn old_engine(title: &'static str, start: Start) -> OldEngine {
    let native = matches!(start, Start::EmptyBackend).then(|| Box::new(Scripted(Side::Old)) as NativeBackend);
    OldEngine { title, headless: matches!(start, Start::Headless), wayland: true, sink: Arc::new(|_, _| {}), connect: connect_old, native, registrations: HashMap::new() }
}

fn new_engine(title: &'static str, start: Start) -> Engine {
    let bindings = match start {
        Start::Headless => Bindings::Virtual(HashMap::new()),
        Start::NoBackend => Bindings::Disconnected,
        Start::EmptyBackend => Bindings::Connected { native: Box::new(Scripted(Side::New)) as NativeBackend, registrations: HashMap::new() },
    };
    Engine { title, wayland: true, sink: Arc::new(|_, _| {}), connect: connect_new, bindings }
}

/// Whether a backend is connected, and the sorted registrations.
type EngineView = (bool, Vec<(u32, String)>);

fn listed(registrations: &HashMap<ShortcutId, Shortcut>) -> Vec<(u32, String)> {
    let mut list: Vec<_> = registrations.iter().map(|(id, s)| (id.0, format!("{s:?}"))).collect();
    list.sort();
    list
}

fn old_view(engine: &OldEngine) -> EngineView { (engine.native.is_some(), listed(&engine.registrations)) }

fn new_view(engine: &Engine) -> EngineView {
    match &engine.bindings {
        Bindings::Virtual(registrations) => (false, listed(registrations)),
        Bindings::Disconnected => (false, vec![]),
        Bindings::Connected { registrations, .. } => (true, listed(registrations)),
    }
}

/// Runs `body` against both engines' results with one script, then clears the script.
fn scripted<T>(connect: Result<(), ShortcutError>, register: Result<(), ShortcutError>, unregister: Result<(), ShortcutError>, body: impl FnOnce() -> T) -> T {
    RIG.with(|rig| { let mut rig = rig.borrow_mut(); rig.connect = connect; rig.register = register; rig.unregister = unregister; });
    let result = body();
    RIG.with(|rig| { let mut rig = rig.borrow_mut(); rig.connect = Ok(()); rig.register = Ok(()); rig.unregister = Ok(()); });
    result
}

const BASE_ID: u32 = 0x4000_0000;

fn run_engines(start: Start, sequence: &[usize]) {
    let (mut old, mut new) = (old_engine("Subject", start), new_engine("Subject", start));
    let (mut old_other, mut new_other) = (old_engine("Other", Start::NoBackend), new_engine("Other", Start::NoBackend));
    let [x, y] = &*CHORDS;
    let (mut next, mut held_x, mut held_y) = (BASE_ID, ShortcutId(BASE_ID - 1), ShortcutId(BASE_ID - 2));
    let denied = || Err(ShortcutError::PermissionDenied);
    let broken = || Err(ShortcutError::Backend("refused".into()));
    for (n, letter) in sequence.iter().enumerate() {
        next += 1;
        let id = ShortcutId(next);
        let (expected, actual) = match letter {
            0 | 1 => {
                let chord = if *letter == 0 { x } else { y };
                let results = (old.register(id, chord.clone()), new.register(id, chord.clone()));
                if results.1.is_ok() { if *letter == 0 { held_x = id } else { held_y = id } }
                results
            }
            2 => scripted(denied(), Ok(()), Ok(()), || (old.register(id, x.clone()), new.register(id, x.clone()))),
            3 => scripted(Ok(()), broken(), Ok(()), || (old.register(id, y.clone()), new.register(id, y.clone()))),
            4 => (old.unregister(held_x), new.unregister(held_x)),
            5 => (old.unregister(held_y), new.unregister(held_y)),
            6 => scripted(Ok(()), Ok(()), broken(), || (old.unregister(held_x), new.unregister(held_x))),
            7 => (old_other.register(id, x.clone()), new_other.register(id, x.clone())),
            8 => { old_other.release(); new_other.release(); (Ok(()), Ok(())) }
            _ => { old.release(); new.release(); (Ok(()), Ok(())) }
        };
        let at = || format!("{start:?} {sequence:?} at {n}");
        assert_eq!(actual, expected, "{}", at());
        assert_eq!(new_view(&new), old_view(&old), "{}", at());
        assert_eq!(new_view(&new_other), old_view(&old_other), "{}", at());
        assert_eq!(claims(&NATIVE), claims(&OLD_NATIVE), "{}", at());
        for chord in [x, y] {
            assert_eq!(new.owner(chord), old.owner(chord), "{}", at());
            assert_eq!(new.bindings.find(chord).is_some(), old.registrations.values().any(|s| s == chord), "{}", at());
        }
        RIG.with(|rig| { let rig = rig.borrow(); assert_eq!(rig.new, rig.old, "{}", at()); });
    }
    drop(old); drop(new); drop(old_other); drop(new_other);
    assert!(claims(&NATIVE).is_empty() && claims(&OLD_NATIVE).is_empty(), "{start:?} {sequence:?}");
    RIG.with(|rig| {
        let mut rig = rig.borrow_mut();
        assert_eq!(rig.new, rig.old, "teardown of {start:?} {sequence:?}");
        rig.new.clear();
        rig.old.clear();
    });
}

#[test]
fn every_engine_sequence_matches_the_old_engine() {
    for start in [Start::Headless, Start::NoBackend, Start::EmptyBackend] {
        every_sequence(10, 5, |sequence| run_engines(start, sequence));
    }
}

#[test]
fn no_backend_connects_while_an_empty_backend_binds() {
    let (id, shortcut) = (ShortcutId(1), chord("ctrl+alt+shift+f22"));
    let register = || BindEvent::Register { id, shortcut: shortcut.clone(), claimed: false };

    let mut none = Bindings::<()>::Disconnected;
    assert!(matches!(step_bindings(&mut none, register()).into_iter().next(), Some(BindEffect::Connect { .. })));
    assert!(matches!(none, Bindings::Disconnected), "connecting is the driver's to report");

    let mut empty = Bindings::Connected { native: (), registrations: HashMap::new() };
    assert!(matches!(step_bindings(&mut empty, register()).into_iter().next(), Some(BindEffect::Bind { .. })));
    assert!(matches!(step_bindings(&mut empty, BindEvent::Rejected(ShortcutError::Unsupported)).into_iter().next(), Some(BindEffect::Disconnect(()))));
    assert!(matches!(empty, Bindings::Disconnected), "a refused first registration closes the backend");
}

#[test]
fn the_last_unregistration_disclaims_before_closing_and_release_closes_first() {
    let (id, shortcut) = (ShortcutId(1), chord("ctrl+alt+shift+f22"));
    let mut state = Bindings::Connected { native: (), registrations: HashMap::from([(id, shortcut.clone())]) };
    let effects: Vec<_> = step_bindings(&mut state, BindEvent::Unbound(id)).into_iter().collect();
    assert!(matches!(effects.as_slice(), [BindEffect::Disclaim(_), BindEffect::Disconnect(())]));

    let mut state = Bindings::Connected { native: (), registrations: HashMap::from([(id, shortcut)]) };
    let effects: Vec<_> = step_bindings(&mut state, BindEvent::Release).into_iter().collect();
    assert!(matches!(effects.as_slice(), [BindEffect::Disconnect(()), BindEffect::DisclaimAll(_)]));
    assert!(matches!(state, Bindings::Disconnected));
}
