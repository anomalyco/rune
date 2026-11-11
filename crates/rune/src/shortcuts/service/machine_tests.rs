//! Every short sequence of events through the machines, compared after each event with a
//! literal copy of the code they replaced. Both platform shapes run on every host, and a
//! teardown keeps running until the sequence says it finished.

use super::*;
use std::{cell::RefCell, collections::BTreeSet, rc::Rc, sync::LazyLock, thread::ThreadId};

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

// Two hosts overlapping, and calls stopped partway through an engine. Three sides play each
// sequence: the code as it was, that code with the marked departure, and the new code. The new
// code must match the marked reference at every event, and the reference must match the old
// code up to the event where the departure first shows.
static OVERLAP_CHORD: LazyLock<Shortcut> = LazyLock::new(|| chord("ctrl+alt+shift+f18"));
const OVERLAP_ID: u32 = 0x5000_0000;

const STALE_CLAIM: &str = "a replaced engine nobody is using kept its claim";
const STALE_CONFLICT: &str = "a conflict came from an engine nobody is using";
const GRAB_WITHOUT_OWNER: &str = "a live engine holds a chord with no owner on record";
const OWNER_WITHOUT_GRAB: &str = "an owner is on record for a chord no live engine holds";
const WAKE_LETS_GO: &str = "a call in its wake no longer keeps the engine's claims";

fn overlap_claims(native: &Mutex<Vec<(ShortcutId, Shortcut, &'static str)>>) -> Vec<u32> {
    let mut claims: Vec<_> = native.lock().unwrap().iter().filter(|(_, s, _)| *s == *OVERLAP_CHORD).map(|(id, ..)| id.0).collect();
    claims.sort();
    claims
}

trait OverlapEngine {
    fn fresh() -> Self;
    fn add(&mut self, id: u32) -> Result<(), ShortcutError>;
    fn remove(&mut self, id: u32) -> Result<(), ShortcutError>;
    fn held(&self) -> Vec<u32>;
    fn close(&mut self);
}

impl OverlapEngine for OldEngine {
    fn fresh() -> Self { old_engine("Overlap", Start::NoBackend) }
    fn add(&mut self, id: u32) -> Result<(), ShortcutError> { self.register(ShortcutId(id), OVERLAP_CHORD.clone()) }
    fn remove(&mut self, id: u32) -> Result<(), ShortcutError> { self.unregister(ShortcutId(id)) }
    fn held(&self) -> Vec<u32> { self.registrations.keys().map(|id| id.0).collect() }
    fn close(&mut self) { self.release(); }
}

impl OverlapEngine for Engine {
    fn fresh() -> Self { new_engine("Overlap", Start::NoBackend) }
    fn add(&mut self, id: u32) -> Result<(), ShortcutError> { self.register(ShortcutId(id), OVERLAP_CHORD.clone()) }
    fn remove(&mut self, id: u32) -> Result<(), ShortcutError> { self.unregister(ShortcutId(id)) }
    fn held(&self) -> Vec<u32> {
        let (Bindings::Virtual(registrations) | Bindings::Connected { registrations, .. }) = &self.bindings else { return vec![] };
        registrations.keys().map(|id| id.0).collect()
    }
    fn close(&mut self) { self.release(); }
}

#[derive(Clone, Debug, PartialEq)]
enum Seen { Skipped, Done, Parked, Answer(Result<(), ShortcutError>) }

type Handle<E> = Rc<RefCell<E>>;

#[derive(Clone, Copy)]
enum Waiter { Register(u32), Unregister(u32) }

/// What every side shares: the hosts, the slot, the handlers and the stopped calls.
struct Stage<E> {
    attached: bool,
    open: [bool; 2],
    slot: Option<Handle<E>>,
    handlers: BTreeSet<u32>,
    /// A call is stopped in its wake, past the engine.
    in_wake: bool,
    /// What that call still holds.
    held: Option<Handle<E>>,
    /// A call stopped inside the engine, holding its lock.
    inside: Option<Handle<E>>,
    /// A call parked on the engine's lock, holding the routing lock.
    waiting: Option<(Handle<E>, Waiter)>,
    /// A host whose engine is in the slot but whose attach waits for that routing lock.
    pending: Option<usize>,
    next: u32,
    latest: Option<u32>,
    broken: Vec<&'static str>,
}

impl<E: OverlapEngine> Stage<E> {
    fn new() -> Self {
        Self { attached: false, open: [false; 2], slot: None, handlers: BTreeSet::new(), in_wake: false, held: None, inside: None, waiting: None, pending: None, next: OVERLAP_ID, latest: None, broken: vec![] }
    }

    fn slot_held(&self) -> Vec<u32> { self.slot.as_ref().map_or_else(Vec::new, |engine| engine.borrow().held()) }

    fn slot_is_inside(&self) -> bool {
        matches!((&self.slot, &self.inside), (Some(slot), Some(inside)) if Rc::ptr_eq(slot, inside))
    }

    /// Every engine something still keeps alive.
    fn alive(&self) -> Vec<&Handle<E>> {
        self.slot.iter().chain(&self.held).chain(&self.inside).chain(self.waiting.iter().map(|(engine, _)| engine)).collect()
    }

    /// A call reaches `engine`, and the caller lets go of it.
    fn finish(&mut self, engine: Handle<E>, waiter: Waiter) -> Seen {
        let on_slot = self.slot.as_ref().is_some_and(|slot| Rc::ptr_eq(slot, &engine));
        let answer = match waiter {
            Waiter::Register(id) => engine.borrow_mut().add(id),
            Waiter::Unregister(id) => engine.borrow_mut().remove(id),
        };
        drop(engine);
        match (waiter, &answer) {
            (Waiter::Register(id), Ok(())) => self.latest = Some(id),
            (Waiter::Register(id), Err(error)) => {
                self.handlers.remove(&id);
                if *error == ShortcutError::Conflict && on_slot && self.slot_held().is_empty() && self.inside.is_none() { self.broken.push(STALE_CONFLICT); }
            }
            (Waiter::Unregister(id), Ok(())) => { self.handlers.remove(&id); }
            (Waiter::Unregister(_), Err(_)) => {}
        }
        Seen::Answer(answer)
    }

    fn call(&mut self, waiter: Waiter) -> Seen {
        let Some(engine) = self.slot.clone().filter(|_| self.attached) else {
            if let Waiter::Register(id) = waiter { self.handlers.remove(&id); }
            return Seen::Answer(Err(ShortcutError::NotRunning));
        };
        if self.slot_is_inside() {
            self.waiting = Some((engine, waiter));
            return Seen::Parked;
        }
        self.finish(engine, waiter)
    }

    fn register(&mut self) -> Seen {
        if self.waiting.is_some() { return Seen::Skipped; }
        self.next += 1;
        self.handlers.insert(self.next);
        self.call(Waiter::Register(self.next))
    }

    fn unregister(&mut self) -> Seen {
        let (Some(id), None) = (self.latest, &self.waiting) else { return Seen::Skipped };
        self.call(Waiter::Unregister(id))
    }

    /// The engine goes in the slot under the slot's own lock; the attach then waits for routing.
    fn attach(&mut self, host: usize) -> Seen {
        self.slot = Some(Rc::new(RefCell::new(E::fresh())));
        if self.waiting.is_some() {
            self.pending = Some(host);
            return Seen::Parked;
        }
        self.finish_attach(host);
        Seen::Done
    }

    fn finish_attach(&mut self, host: usize) {
        self.open[host] = true;
        self.attached = true;
    }

    fn close(&mut self, host: usize) -> Seen {
        self.open[host] = false;
        self.attached = false;
        if let Some(engine) = self.slot.take() { engine.borrow_mut().close(); }
        self.handlers.clear();
        Seen::Done
    }

    /// The call inside leaves. A call parked behind it takes the engine and lets go of routing,
    /// and only then can an attach waiting for routing finish.
    fn leave(&mut self) -> Seen {
        self.inside = None;
        let Some((engine, waiter)) = self.waiting.take() else { return Seen::Done };
        let seen = self.finish(engine, waiter);
        if let Some(host) = self.pending.take() { self.finish_attach(host); }
        seen
    }
}

trait Overlap {
    type E: OverlapEngine;
    fn stage(&mut self) -> &mut Stage<Self::E>;
    fn stage_ref(&self) -> &Stage<Self::E>;
    fn stop_in_wake(&mut self);
    fn claims(&self) -> Vec<u32>;
}

/// The code as it was, or with `departures` the same code plus the marked change.
struct Reference { stage: Stage<OldEngine>, departures: bool }

impl Overlap for Reference {
    type E = OldEngine;
    fn stage(&mut self) -> &mut Stage<OldEngine> { &mut self.stage }
    fn stage_ref(&self) -> &Stage<OldEngine> { &self.stage }

    fn stop_in_wake(&mut self) {
        // differs from dev: a call lets go of the engine before its wake.
        if self.departures { return; }
        self.stage.held = self.stage.slot.clone();
    }

    fn claims(&self) -> Vec<u32> { overlap_claims(&OLD_NATIVE) }
}

/// The new code around the real engine: a call holds nothing during its wake.
struct NewOverlap(Stage<Engine>);

impl Overlap for NewOverlap {
    type E = Engine;
    fn stage(&mut self) -> &mut Stage<Engine> { &mut self.0 }
    fn stage_ref(&self) -> &Stage<Engine> { &self.0 }
    fn stop_in_wake(&mut self) {}
    fn claims(&self) -> Vec<u32> { overlap_claims(&NATIVE) }
}

/// Letters: 0, 1 attach a host; 2, 3 close one; 4 register the chord; 5 unregister the latest;
/// 6, 7 a call stops in its wake and goes; 8 a call stops inside the engine; 9 it leaves;
/// 10 it leaves and the backend then refuses what was waiting.
fn overlap_play(world: &mut impl Overlap, letter: usize) -> Seen {
    let stage = world.stage();
    let reachable = stage.attached && stage.slot.is_some();
    match letter {
        0 | 1 if stage.open[letter] || stage.pending.is_some() => Seen::Skipped,
        0 | 1 => stage.attach(letter),
        2 | 3 if !stage.open[letter - 2] || stage.inside.is_some() || stage.waiting.is_some() => Seen::Skipped,
        2 | 3 => stage.close(letter - 2),
        4 => stage.register(),
        5 => stage.unregister(),
        6 if stage.in_wake || !reachable || stage.slot_is_inside() || stage.waiting.is_some() => Seen::Skipped,
        6 => { stage.in_wake = true; world.stop_in_wake(); Seen::Done }
        7 if !stage.in_wake => Seen::Skipped,
        7 => { stage.in_wake = false; stage.held = None; Seen::Done }
        8 if stage.inside.is_some() || !reachable || stage.waiting.is_some() => Seen::Skipped,
        8 => { stage.inside = stage.slot.clone(); Seen::Done }
        9 if stage.inside.is_none() => Seen::Skipped,
        9 => stage.leave(),
        _ if stage.inside.is_none() || stage.waiting.is_none() => Seen::Skipped,
        _ => {
            let broken = || Err(ShortcutError::Backend("refused".into()));
            scripted(Ok(()), broken(), broken(), || stage.leave())
        }
    }
}

fn overlap_check(world: &mut impl Overlap) {
    let claims = world.claims();
    let stage = world.stage();
    let live = stage.slot_held();
    let alive: Vec<u32> = stage.alive().iter().flat_map(|engine| engine.borrow().held()).collect();
    if stage.inside.is_none() && stage.waiting.is_none() && claims.iter().any(|id| !live.contains(id)) { stage.broken.push(STALE_CLAIM); }
    if alive.iter().any(|id| !claims.contains(id)) { stage.broken.push(GRAB_WITHOUT_OWNER); }
    if claims.iter().any(|id| !alive.contains(id)) { stage.broken.push(OWNER_WITHOUT_GRAB); }
}

type OverlapView = (Seen, bool, [bool; 2], Vec<u32>, Vec<u32>, Vec<u32>);

fn overlap_view(world: &impl Overlap, seen: Seen) -> OverlapView {
    let stage = world.stage_ref();
    let mut live = stage.slot_held();
    live.sort();
    (seen, stage.attached, stage.open, stage.handlers.iter().copied().collect(), world.claims(), live)
}

fn clear_notes() { RIG.with(|rig| { let mut rig = rig.borrow_mut(); rig.new.clear(); rig.old.clear(); }); }

/// What the code as it was does with `sequence`, and which invariants it broke.
fn overlap_dev_trace(sequence: &[usize], found: &mut BTreeSet<&'static str>) -> Vec<OverlapView> {
    let mut dev = Reference { stage: Stage::new(), departures: false };
    let mut trace = vec![];
    for letter in sequence {
        let seen = overlap_play(&mut dev, *letter);
        overlap_check(&mut dev);
        trace.push(overlap_view(&dev, seen));
    }
    found.extend(dev.stage.broken.iter());
    drop(dev);
    assert!(overlap_claims(&OLD_NATIVE).is_empty(), "{sequence:?}");
    trace
}

/// Replays `sequence`. Returns whether its last letter did anything.
fn overlap_replay(sequence: &[usize], found: &mut BTreeSet<&'static str>, fired: &mut BTreeSet<&'static str>) -> bool {
    let dev = overlap_dev_trace(sequence, found);
    let mut reference = Reference { stage: Stage::new(), departures: true };
    let mut new = NewOverlap(Stage::new());
    let (mut moved, mut departed) = (false, false);
    for (n, letter) in sequence.iter().enumerate() {
        let expected = overlap_play(&mut reference, *letter);
        let actual = overlap_play(&mut new, *letter);
        moved = actual != Seen::Skipped;
        overlap_check(&mut new);
        assert!(new.0.broken.is_empty(), "{:?}: {sequence:?} at {n}", new.0.broken);
        let want = overlap_view(&reference, expected);
        assert_eq!(overlap_view(&new, actual), want, "{sequence:?} at {n}");
        if !departed && want != dev[n] {
            // differs from dev: the first difference is only ever dev keeping claims for an
            // engine that nothing but a call in its wake holds.
            let (seen, attached, open, handlers, claims, live) = &dev[n];
            assert_eq!((seen, attached, open, handlers, live), (&want.0, &want.1, &want.2, &want.3, &want.5), "{sequence:?} at {n}");
            assert!(want.4.iter().all(|id| claims.contains(id)) && claims.len() > want.4.len(), "{sequence:?} at {n}");
            assert!(sequence[..=n].contains(&6), "{sequence:?} at {n}");
            departed = true;
            fired.insert(WAKE_LETS_GO);
        }
    }
    drop(reference);
    drop(new);
    assert!(overlap_claims(&NATIVE).is_empty() && overlap_claims(&OLD_NATIVE).is_empty(), "{sequence:?}");
    clear_notes();
    moved
}

fn overlap_walk(sequence: &mut Vec<usize>, depth: usize, found: &mut BTreeSet<&'static str>, fired: &mut BTreeSet<&'static str>) {
    for letter in 0..11 {
        sequence.push(letter);
        if overlap_replay(sequence, found, fired) && sequence.len() < depth { overlap_walk(sequence, depth, found, fired); }
        sequence.pop();
    }
}

#[test]
fn an_attach_waits_for_a_call_parked_on_routing() {
    let mut dev = Reference { stage: Stage::new(), departures: false };
    let mut new = NewOverlap(Stage::new());
    // Attach A, a call stops inside A's engine, a registration parks behind it, B attaches, the
    // call inside leaves.
    let script = [(0, Seen::Done), (8, Seen::Done), (4, Seen::Parked), (1, Seen::Parked), (9, Seen::Answer(Ok(())))];
    for (n, (letter, expected)) in script.into_iter().enumerate() {
        assert_eq!(overlap_play(&mut dev, letter), expected, "dev, event {n}");
        assert_eq!(overlap_play(&mut new, letter), expected, "new, event {n}");
        let hosts = (dev.stage.attached, dev.stage.open);
        assert_eq!((new.0.attached, new.0.open), hosts, "event {n}");
        if n == 3 { assert_eq!(hosts, (true, [true, false]), "B's attach can't finish while the parked call holds routing"); }
    }
    assert_eq!((dev.stage.attached, dev.stage.open), (true, [true, true]), "B attaches once the call lets go of routing");
    drop(dev);
    drop(new);
    clear_notes();
}

#[test]
fn every_overlap_sequence_differs_from_dev_only_at_its_marked_departure() {
    let (mut found, mut fired) = (BTreeSet::new(), BTreeSet::new());
    overlap_walk(&mut vec![], 7, &mut found, &mut fired);
    for invariant in [STALE_CLAIM, STALE_CONFLICT] {
        assert!(found.contains(invariant), "the walk never saw the old code break this, so it proves nothing: {invariant}");
    }
    assert!(fired.contains(WAKE_LETS_GO), "the departure never took effect");
}
