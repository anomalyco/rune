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

/// An engine's number is also the generation of the host that installed it.
enum Op { Attach(Waker, u32), Call(u32), Drain, Close, ReleaseEngine(u32) }

/// Teardowns still running, oldest first, each with the wake and generation of the attach it
/// holds up, the operations parked behind them, and each thread's guard, by its generation. A
/// thread's later attach takes the place of its guard here, and the guard it replaced never
/// closes.
#[derive(Default)]
struct Schedule { teardowns: Vec<(ThreadId, Option<(Waker, u32)>)>, parked: Vec<(ThreadId, Op)>, guards: HashMap<ThreadId, u32> }

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
    /// The routing half of an attach by the host of generation `host`, which runs once any
    /// replaced engine is gone.
    fn finish_attach(&mut self, thread: ThreadId, wake: Waker, host: u32);
    fn call(&mut self, thread: ThreadId, job: u32) -> Outcome;
    /// A drain by the thread's guard, of generation `host`.
    fn drain(&mut self, thread: ThreadId, host: u32) -> Outcome;
    /// The close of the thread's guard, of generation `host`.
    fn close(&mut self, thread: ThreadId, host: u32) -> Outcome;
    /// The engine half of a close on Linux.
    fn release_engine(&mut self, thread: ThreadId, host: u32) -> Outcome;
    fn view(&self) -> RouteView;
}

fn perform(model: &mut impl Model, thread: ThreadId, op: Op) -> Outcome {
    match op {
        Op::Attach(wake, engine) => {
            let outcome = model.attach(thread, wake, engine);
            // A parked attach hasn't installed yet; it gets its guard when it does.
            if outcome != Outcome::Blocked { model.schedule().guards.insert(thread, engine); }
            outcome
        }
        Op::Call(job) => model.call(thread, job),
        Op::Drain => {
            let host = model.schedule().guards[&thread];
            model.drain(thread, host)
        }
        Op::Close => {
            let host = model.schedule().guards.remove(&thread).expect("only a thread with a guard closes");
            model.close(thread, host)
        }
        Op::ReleaseEngine(host) => model.release_engine(thread, host),
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
    if let Some((wake, host)) = attaching { model.finish_attach(thread, wake, host); }
    outcomes.extend(others.into_iter().map(|(thread, op)| perform(model, thread, op)));
    outcomes
}

// The routing as it was: Routing's fields, the Linux engine slot and the thread-local engines,
// with the current thread passed in and a job as a number. A running teardown on Linux holds
// the slot's lock.
#[derive(Default)]
struct OldRouting {
    owner: Option<ThreadId>,
    wake: Option<Waker>,
    jobs: Vec<u32>,
    // differs from dev: routing names the generation of the host it routes to.
    host: Option<u32>,
}

/// `left_newer` and `left_calls` note that a marked departure took effect.
#[derive(Default)]
struct OldControl { routing: OldRouting, engine: Option<u32>, engines: HashMap<ThreadId, u32>, linux: bool, schedule: Schedule, left_newer: bool, left_calls: bool }

impl OldControl {
    fn slot_locked(&self) -> bool { self.linux && !self.schedule.teardowns.is_empty() }
}

impl Model for OldControl {
    fn schedule(&mut self) -> &mut Schedule { &mut self.schedule }

    fn attach(&mut self, thread: ThreadId, wake: Waker, engine: u32) -> Outcome {
        if self.slot_locked() { self.schedule.parked.push((thread, Op::Attach(wake, engine))); return Outcome::Blocked; }
        let replaced = if self.linux { self.engine.replace(engine) } else { self.engines.insert(thread, engine) };
        if let Some(replaced) = replaced {
            self.schedule.teardowns.push((thread, Some((wake, engine))));
            return Outcome::Replacing { engine: replaced };
        }
        self.finish_attach(thread, wake, engine);
        Outcome::Attached
    }

    fn finish_attach(&mut self, thread: ThreadId, wake: Waker, host: u32) {
        self.routing.owner = Some(thread);
        self.routing.wake = Some(wake);
        self.routing.host = Some(host);
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

    fn drain(&mut self, thread: ThreadId, host: u32) -> Outcome {
        if self.linux { return Outcome::Drained { ran: vec![], dropped: vec![] }; }
        // differs from dev: only the routed host takes the waiting calls; another leaves them.
        if self.routing.host != Some(host) {
            self.left_calls |= !self.routing.jobs.is_empty();
            return Outcome::Drained { ran: vec![], dropped: vec![] };
        }
        let jobs = std::mem::take(&mut self.routing.jobs);
        // differs from dev: they run on the drainer's engine only if its own host installed it.
        if self.engines.get(&thread) == Some(&host) { Outcome::Drained { ran: jobs, dropped: vec![] } } else { Outcome::Drained { ran: vec![], dropped: jobs } }
    }

    fn close(&mut self, thread: ThreadId, host: u32) -> Outcome {
        let route = std::mem::take(&mut self.routing);
        if self.linux {
            if self.slot_locked() { self.schedule.parked.push((thread, Op::ReleaseEngine(host))); return Outcome::Blocked; }
            return self.release_engine(thread, host);
        }
        // differs from dev: a close takes only the engine its own host installed.
        let engine = if self.engines.get(&thread) == Some(&host) { self.engines.remove(&thread) } else { None };
        if engine.is_some() { self.schedule.teardowns.push((thread, None)); }
        Outcome::Closed { jobs: route.jobs, engine }
    }

    fn release_engine(&mut self, thread: ThreadId, host: u32) -> Outcome {
        if self.slot_locked() { self.schedule.parked.push((thread, Op::ReleaseEngine(host))); return Outcome::Blocked; }
        // differs from dev: a close takes only the engine its own host installed, never a newer one.
        let engine = if self.engine == Some(host) { self.engine.take() } else { None };
        self.left_newer |= engine.is_none() && self.engine.is_some();
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
        if let Some(replaced) = replaced {
            self.schedule.teardowns.push((thread, Some((wake, engine))));
            return Outcome::Replacing { engine: replaced };
        }
        self.finish_attach(thread, wake, engine);
        Outcome::Attached
    }

    fn finish_attach(&mut self, thread: ThreadId, wake: Waker, host: u32) {
        let host = host.into();
        let own = address(&wake);
        // differs from dev: a host that inherits waiting calls is woken for them.
        for effect in step_attachment(&mut self.attachment, AttachEvent::Attach { owner: thread, wake, host }) {
            assert!(matches!(effect, AttachEffect::Wake(wake) if address(&wake) == own), "an attach only wakes the host it routes to");
        }
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

    fn drain(&mut self, thread: ThreadId, host: u32) -> Outcome {
        if self.linux { return Outcome::Drained { ran: vec![], dropped: vec![] }; }
        let Some(AttachEffect::Jobs(jobs)) = step_attachment(&mut self.attachment, AttachEvent::Drain { host: host.into() }).into_iter().next() else {
            return Outcome::Drained { ran: vec![], dropped: vec![] };
        };
        // The routed host's thread holds its engine.
        if self.engines.get(&thread) == Some(&host) { Outcome::Drained { ran: jobs, dropped: vec![] } } else { Outcome::Drained { ran: vec![], dropped: jobs } }
    }

    fn close(&mut self, thread: ThreadId, host: u32) -> Outcome {
        let mut released = vec![];
        for effect in step_attachment(&mut self.attachment, AttachEvent::Close) {
            if let AttachEffect::Release(jobs) = effect { released = jobs; }
        }
        if self.linux { return self.release_engine(thread, host); }
        let engine = if self.engines.get(&thread) == Some(&host) { self.engines.remove(&thread) } else { None };
        if engine.is_some() { self.schedule.teardowns.push((thread, None)); }
        Outcome::Closed { jobs: released, engine }
    }

    fn release_engine(&mut self, thread: ThreadId, host: u32) -> Outcome {
        if self.slot_locked() { self.schedule.parked.push((thread, Op::ReleaseEngine(host))); return Outcome::Blocked; }
        // Only the engine this host installed, with the wake that travels with it.
        if self.engine != Some(host) { return Outcome::Closed { jobs: vec![], engine: None }; }
        let engine = self.engine.take();
        self.slot_wake = None;
        self.schedule.teardowns.push((thread, None));
        Outcome::Closed { jobs: vec![], engine }
    }

    fn view(&self) -> RouteView {
        let engines = sorted(&self.engines);
        match &self.attachment {
            Attachment::Attached { owner, wake, jobs, .. } => (Some(*owner), Some(address(wake)), jobs.clone(), self.engine, engines),
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
/// Only a thread with a guard drains or closes.
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
    if matches!(op, Op::Drain | Op::Close) && !model.schedule().guards.contains_key(&thread) { return vec![Outcome::Skipped]; }
    vec![perform(model, thread, op)]
}

/// Plays `steps` through both models, each letter with the outcomes given, and returns who
/// routing names at the end.
fn script(linux: bool, steps: &[(usize, Vec<Outcome>)], threads: &[ThreadId; 2], wakes: &[Waker; 2]) -> Option<ThreadId> {
    let mut old = OldControl { linux, ..OldControl::default() };
    let mut new = NewControl { attachment: Attachment::Detached, engine: None, slot_wake: None, engines: HashMap::new(), linux, schedule: Schedule::default() };
    for (n, (letter, expected)) in steps.iter().enumerate() {
        let number = n as u32 + 1;
        assert_eq!(&play(&mut old, *letter, threads, wakes, number), expected, "linux {linux}, the old routing at {n}");
        assert_eq!(&play(&mut new, *letter, threads, wakes, number), expected, "linux {linux}, the new routing at {n}");
        assert_eq!(new.view(), old.view(), "linux {linux} at {n}");
    }
    new.view().0
}

/// The cases a host generation must leave as they were: the newest host's close takes its own
/// engine, the routed host runs the calls waiting for it, and any close still ends routing and
/// releases those calls.
#[test]
fn the_newest_hosts_close_and_drain_and_any_close_ending_routing_stay_as_they_were() {
    let threads = [std::thread::current().id(), other_thread()];
    let wakes: [Waker; 2] = [Arc::new(|| {}), Arc::new(|| { let _ = 1; })];
    let (w0, w1) = (address(&wakes[0]), address(&wakes[1]));
    use Outcome::*;
    // Off Linux: the owner drains a call from the other thread, then closes and takes its engine.
    let routed = script(false, &[
        (0, vec![Attached]),
        (3, vec![Queued { wake: w0 }]),
        (4, vec![Drained { ran: vec![2], dropped: vec![] }]),
        (6, vec![Closed { jobs: vec![], engine: Some(1) }]),
        (8, vec![]),
    ], &threads, &wakes);
    assert_eq!(routed, None);
    // The routed host drains the call that waits for it, though another host attached first.
    let routed = script(false, &[
        (0, vec![Attached]),
        (1, vec![Attached]),
        (2, vec![Queued { wake: w1 }]),
        (5, vec![Drained { ran: vec![3], dropped: vec![] }]),
    ], &threads, &wakes);
    assert_eq!(routed, Some(threads[1]));
    // A replaced host's close still ends routing and releases its successor's waiting caller.
    let routed = script(false, &[
        (0, vec![Attached]),
        (1, vec![Attached]),
        (2, vec![Queued { wake: w1 }]),
        (6, vec![Closed { jobs: vec![3], engine: Some(1) }]),
        (3, vec![Refused]),
    ], &threads, &wakes);
    assert_eq!(routed, None);
    // On Linux: a lone host's close takes its engine.
    let routed = script(true, &[
        (0, vec![Attached]),
        (2, vec![Ran { engine: 1, wake: w0 }]),
        (6, vec![Closed { jobs: vec![], engine: Some(1) }]),
        (8, vec![]),
    ], &threads, &wakes);
    assert_eq!(routed, None);
    // The newest host's close takes its own engine, once the one it replaced has gone.
    let routed = script(true, &[
        (0, vec![Attached]),
        (1, vec![Replacing { engine: 1 }]),
        (8, vec![]),
        (7, vec![Closed { jobs: vec![], engine: Some(2) }]),
        (8, vec![]),
        (2, vec![Refused]),
    ], &threads, &wakes);
    assert_eq!(routed, None);
}

#[test]
fn every_attachment_sequence_matches_the_old_routing() {
    let threads = [std::thread::current().id(), other_thread()];
    let wakes: [Waker; 2] = [Arc::new(|| {}), Arc::new(|| { let _ = 1; })];
    let (mut departed, mut left_newer, mut left_calls) = (false, false, false);
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
            left_newer |= old.left_newer;
            left_calls |= old.left_calls;
        });
    }
    assert!(departed, "the walk never saw a call that reached the engine a newer host installed");
    assert!(left_newer, "the walk never saw a replaced host's close leave the newer engine");
    assert!(left_calls, "the walk never saw a replaced host's drain leave the calls to the routed host");
}

