//! The routing and per-thread engine model, compared event by event with a literal copy of the
//! code they replaced. A Linux teardown keeps running until the sequence says it finished.

use super::machine_tests::every_sequence;
use super::*;
use std::thread::ThreadId;

fn other_thread() -> ThreadId { std::thread::spawn(|| std::thread::current().id()).join().unwrap() }

fn address(wake: &Waker) -> usize { Arc::as_ptr(wake) as *const () as usize }

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
struct NewControl { attachment: Attachment<u32>, engine: Option<u32>, slot_wake: Option<Waker>, engines: HashMap<ThreadId, u32>, linux: bool, schedule: Schedule }

impl NewControl {
    fn slot_locked(&self) -> bool { self.linux && !self.schedule.teardowns.is_empty() }
}

impl Model for NewControl {
    fn schedule(&mut self) -> &mut Schedule { &mut self.schedule }

    fn attach(&mut self, thread: ThreadId, wake: Waker, engine: u32) -> Outcome {
        if self.slot_locked() { self.schedule.parked.push((thread, Op::Attach(wake, engine))); return Outcome::Blocked; }
        let replaced = if self.linux {
            // differs from dev: the wake travels with the engine's slot, so a call that reaches a
            // newer engine wakes its host, not the host routing still names.
            self.slot_wake = Some(wake.clone());
            self.engine.replace(engine)
        } else { self.engines.insert(thread, engine) };
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
                engine.map_or(Outcome::Refused, |engine| {
                    // differs from dev: routing's wake only says a call may run; the engine's
                    // slot names the host it wakes.
                    let wake = if self.linux { self.slot_wake.clone().expect("an engine in the slot has a host") } else { wake };
                    Outcome::Ran { engine, wake: address(&wake) }
                })
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
        self.slot_wake = None;
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

/// The outcomes in order, allowing only the marked wake departure: the same engine ran the call,
/// and the wake names the host whose engine the slot holds.
fn assert_outcomes(old: &[Outcome], new: &[Outcome], linux: bool, at: &str) -> bool {
    assert_eq!(new.len(), old.len(), "{at}");
    let mut departed = false;
    for (new, old) in new.iter().zip(old) {
        if let (Outcome::Ran { engine: a, wake: a_wake }, Outcome::Ran { engine: b, wake: b_wake }) = (old, new) {
            assert_eq!(a, b, "the same engine ran the call: {at}");
            if a_wake != b_wake {
                assert!(linux, "a wake moved between owner threads: {at}");
                departed = true;
                continue;
            }
        }
        assert_eq!(new, old, "{at}");
    }
    departed
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
    let mut departed = false;
    for linux in [false, true] {
        every_sequence(9, 6, |sequence| {
            let mut old = OldControl { linux, ..OldControl::default() };
            let mut new = NewControl { attachment: Attachment::Detached, engine: None, slot_wake: None, engines: HashMap::new(), linux, schedule: Schedule::default() };
            for (n, letter) in sequence.iter().enumerate() {
                let number = n as u32 + 1;
                let expected = play(&mut old, *letter, &threads, &wakes, number);
                let actual = play(&mut new, *letter, &threads, &wakes, number);
                departed |= assert_outcomes(&expected, &actual, linux, &format!("linux {linux}, {sequence:?} at {n}"));
                assert_eq!(new.view(), old.view(), "linux {linux}, {sequence:?} at {n}");
                assert_eq!(new.schedule.view(), old.schedule.view(), "linux {linux}, {sequence:?} at {n}");
                if linux && actual.iter().any(|outcome| matches!(outcome, Outcome::Ran { .. })) {
                    assert!(new.schedule.teardowns.is_empty(), "a call reached an engine during a teardown: {sequence:?} at {n}");
                }
            }
        });
    }
    assert!(departed, "the walk never saw a call that reached the engine a newer host installed");
}

