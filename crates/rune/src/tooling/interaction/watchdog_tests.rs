use std::cell::{Cell, RefCell};
use std::sync::{TryLockError, mpsc};

use super::super::stack::DEPTH;
use super::*;

const MS: u64 = 1_000_000;
const EVERY_NS: u64 = EVERY.as_nanos() as u64;
const REST_NS: u64 = REST.as_nanos() as u64;
const AFTER_US: u32 = AFTER_MS * 1000;
const HURRY_US: u32 = HURRY_MS * 1000;

fn running(seq: u64, start: u64) -> Option<Running> { Some(Running { seq, start, after_us: AFTER_US }) }

fn look(watch: &mut Watch, frame: Option<Running>, now: u64) -> Effect {
    match step(watch, Event::Look { frame, now }) {
        Effects::One(effect) => effect,
        other => panic!("a look decides one thing, got {other:?}"),
    }
}

fn taken(watch: &mut Watch, seq: u64, now: u64) { assert!(step(watch, Event::Taken { seq, now }).is_empty()); }

fn sampling(seq: u64, taken: usize, due: u64) -> Watch { Watch::Sampling(Progress { seq, taken, due }) }

#[test]
fn a_frame_is_sampled_once_old_enough_and_then_every_few_ms() {
    let mut watch = Watch::Unarmed;
    assert_eq!(look(&mut watch, running(1, 100 * MS), 103 * MS), Effect::Sleep(Duration::from_millis(9)));
    assert_eq!(watch, Watch::Waiting { seq: 1, due: 112 * MS });
    assert_eq!(look(&mut watch, running(1, 100 * MS), 112 * MS), Effect::Sample { seq: 1, start: 100 * MS, at: 112 * MS });
    taken(&mut watch, 1, 113 * MS);
    assert_eq!(watch, sampling(1, 1, 113 * MS + EVERY_NS));
    assert_eq!(look(&mut watch, running(1, 100 * MS), 114 * MS), Effect::Sleep(Duration::from_millis(3)));
    assert_eq!(look(&mut watch, running(1, 100 * MS), 117 * MS), Effect::Sample { seq: 1, start: 100 * MS, at: 117 * MS });
}

#[test]
fn hurry_brings_a_waiting_frame_forward_but_not_a_sampling_one() {
    let mut watch = Watch::Unarmed;
    let hurried = Some(Running { seq: 1, start: 100 * MS, after_us: HURRY_US });
    look(&mut watch, running(1, 100 * MS), 101 * MS);
    assert_eq!(look(&mut watch, hurried, 104 * MS), Effect::Sample { seq: 1, start: 100 * MS, at: 104 * MS });
    taken(&mut watch, 1, 104 * MS);
    assert_eq!(look(&mut watch, hurried, 105 * MS), Effect::Sleep(Duration::from_millis(3)), "sampling keeps its own pace");
}

#[test]
fn a_frame_armed_while_sampling_waits_its_own_age() {
    let mut watch = Watch::Unarmed;
    assert_eq!(look(&mut watch, running(1, 100 * MS), 120 * MS), Effect::Sample { seq: 1, start: 100 * MS, at: 120 * MS });
    // Frame 2 starts while frame 1's sample is in flight; the sample still lands on frame 1.
    taken(&mut watch, 1, 121 * MS);
    assert_eq!(look(&mut watch, running(2, 121 * MS), 122 * MS), Effect::Sleep(Duration::from_millis(11)));
    assert_eq!(watch, Watch::Waiting { seq: 2, due: 133 * MS });
}

#[test]
fn a_sample_answering_an_older_frame_changes_nothing() {
    for mut watch in [Watch::Unarmed, Watch::Waiting { seq: 3, due: 9 }, sampling(3, 2, 9), Watch::Quiet { since: 9, left: None }, Watch::Parked { left: None }] {
        let before = watch;
        taken(&mut watch, 1, 100 * MS);
        assert_eq!(watch, before);
    }
}

#[test]
fn disarm_while_waiting_goes_quiet_without_a_sample() {
    let mut watch = Watch::Unarmed;
    look(&mut watch, running(1, 100 * MS), 101 * MS);
    assert_eq!(look(&mut watch, None, 105 * MS), Effect::Sleep(IDLE_POLL));
    assert_eq!(watch, Watch::Quiet { since: 105 * MS, left: None });
}

#[test]
fn going_idle_mid_sample_rests_from_the_next_look_and_then_parks() {
    let mut watch = Watch::Unarmed;
    look(&mut watch, running(1, 100 * MS), 120 * MS);
    taken(&mut watch, 1, 121 * MS);
    assert_eq!(look(&mut watch, None, 122 * MS), Effect::Sleep(IDLE_POLL));
    assert_eq!(look(&mut watch, None, 122 * MS + REST_NS - 1), Effect::Sleep(IDLE_POLL));
    assert_eq!(look(&mut watch, None, 122 * MS + REST_NS), Effect::Park);
    assert!(matches!(watch, Watch::Parked { .. }));
}

#[test]
fn parked_stays_parked_through_a_wake_with_no_frame_and_watches_the_next_frame() {
    let mut watch = Watch::Parked { left: None };
    assert_eq!(look(&mut watch, None, 5000 * MS), Effect::Park);
    assert_eq!(watch, Watch::Parked { left: None });
    assert_eq!(look(&mut watch, running(9, 5000 * MS), 5001 * MS), Effect::Sleep(Duration::from_millis(11)));
    assert_eq!(watch, Watch::Waiting { seq: 9, due: 5012 * MS });
}

#[test]
fn a_full_frame_stops_sampling_and_polls() {
    let mut watch = sampling(1, MOST - 1, 100 * MS);
    assert_eq!(look(&mut watch, running(1, 0), 100 * MS), Effect::Sample { seq: 1, start: 0, at: 100 * MS });
    taken(&mut watch, 1, 100 * MS);
    assert_eq!(look(&mut watch, running(1, 0), 500 * MS), Effect::Sleep(IDLE_POLL));
}

const A: u64 = 100 * MS;
const B: u64 = 1000 * MS;

/// A look that read the old start and the new number tries a sample the thread's `still` check
/// will refuse (`START` has moved on from `start`), and comes back 4 ms later.
fn refused_at_13_ms(watch: &mut Watch) {
    assert_eq!(look(watch, running(2, A), B + 13 * MS), Effect::Sample { seq: 2, start: A, at: B + 13 * MS });
    taken(watch, 2, B + 13 * MS);
}

#[test]
fn a_frame_change_between_reads_never_sleeps_on_the_older_frames_cap() {
    let mut watch = sampling(1, MOST, 0);
    refused_at_13_ms(&mut watch);
    assert_eq!(look(&mut watch, running(2, B), B + 13 * MS), Effect::Sleep(Duration::from_millis(4)));
    assert_eq!(look(&mut watch, running(2, B), B + 17 * MS), Effect::Sample { seq: 2, start: B, at: B + 17 * MS });
}

#[test]
fn a_frame_replaced_mid_look_is_sampled_at_17_ms_of_a_20_ms_stall() {
    let mut watch = sampling(1, 3, 0);
    refused_at_13_ms(&mut watch);
    assert_eq!(look(&mut watch, running(2, B), B + 13 * MS), Effect::Sleep(Duration::from_millis(4)), "not the 8 ms idle poll, to 21 ms");
    assert_eq!(look(&mut watch, running(2, B), B + 17 * MS), Effect::Sample { seq: 2, start: B, at: B + 17 * MS }, "before the stall ends at 20 ms");
}

#[test]
fn a_sample_taken_while_the_ui_stalls_in_arm_is_filed_under_the_new_frame() {
    // `arm` has moved `SEQ` to 2 and stalled before storing `START`: `still` holds for A's start.
    let mut watch = sampling(1, 3, 0);
    assert_eq!(look(&mut watch, running(2, A), B + 17 * MS), Effect::Sample { seq: 2, start: A, at: B + 17 * MS }, "under the number `disarm` will collect");
    taken(&mut watch, 2, B + 17 * MS);
    // `arm` finishes at 19 ms; the frame ends at 20 ms and collects sequence 2.
    assert_eq!(look(&mut watch, running(2, B + 19 * MS), B + 19 * MS), Effect::Sleep(Duration::from_millis(2)));
    assert_eq!(watch, sampling(2, 1, B + 21 * MS));
}

#[test]
fn a_number_read_before_its_start_lands_keeps_its_progress_through_a_quiet_look() {
    // A ended and `arm` moved `SEQ`, so a look can pair A's start with 2, and the next read 0.
    let mut watch = sampling(1, 3, 0);
    refused_at_13_ms(&mut watch);
    assert_eq!(look(&mut watch, None, B + 13 * MS), Effect::Sleep(IDLE_POLL));
    assert_eq!(look(&mut watch, running(2, B + 14 * MS), B + 21 * MS), Effect::Sample { seq: 2, start: B + 14 * MS, at: B + 21 * MS }, "the 4 ms retry is already due");
}

/// The loop on `dev` before the machine, line for line, given the values it read.
#[derive(Clone, Copy)]
struct Dev {
    seen: u64,
    taken: usize,
    next: u64,
    quiet_since: Option<u64>,
}

impl Dev {
    fn look(&mut self, start: u64, seq: u64, after_us: u32, now: u64) -> Effect {
        if start == 0 {
            let quiet = *self.quiet_since.get_or_insert(now);
            return if now - quiet < REST_NS { Effect::Sleep(IDLE_POLL) } else { Effect::Park };
        }
        self.quiet_since = None;
        if seq != self.seen { (self.seen, self.taken) = (seq, 0); }
        if self.taken == 0 { self.next = start + u64::from(after_us) * 1000; }
        if now < self.next || self.taken >= MOST {
            return Effect::Sleep(if self.taken >= MOST { IDLE_POLL } else { Duration::from_nanos(self.next - now) });
        }
        Effect::Sample { seq, start, at: now }
    }

    fn sampled(&mut self, now: u64) {
        self.taken += 1;
        self.next = now + EVERY_NS;
    }
}

#[derive(Clone, Copy, Debug)]
enum Letter {
    /// A look read these values, in any combination two frames could leave. No frame is 0.
    See { start: u64, seq: u64, after_us: u32 },
    Pass(u64),
    /// The sample in flight ended.
    Finish,
}

const OLD: u64 = 1000;
const NEW: u64 = 20 * MS;
const NONE: Letter = Letter::See { start: 0, seq: 1, after_us: AFTER_US };
const SEE_1: Letter = Letter::See { start: OLD, seq: 1, after_us: AFTER_US };
const ALPHABET: [Letter; 13] = [
    NONE,
    SEE_1,
    Letter::See { start: OLD, seq: 1, after_us: HURRY_US },
    Letter::See { start: OLD, seq: 2, after_us: AFTER_US },
    Letter::See { start: OLD, seq: 2, after_us: HURRY_US },
    Letter::See { start: NEW, seq: 1, after_us: AFTER_US },
    Letter::See { start: NEW, seq: 1, after_us: HURRY_US },
    Letter::See { start: NEW, seq: 2, after_us: AFTER_US },
    Letter::See { start: NEW, seq: 2, after_us: HURRY_US },
    Letter::Pass(5 * MS),
    Letter::Pass(13 * MS),
    Letter::Pass(REST_NS + 100 * MS),
    Letter::Finish,
];

/// What the walk reached, so it can't pass by never getting anywhere.
#[derive(Default)]
struct Reached {
    waiting: bool,
    sampling: bool,
    quiet: bool,
    parked: bool,
    full: bool,
    sampled: bool,
    resumed: bool,
}

#[derive(Clone, Copy)]
struct World {
    watch: Watch,
    dev: Dev,
    now: u64,
    in_flight: Option<u64>,
}

impl World {
    fn new() -> Self { Self { watch: Watch::Unarmed, dev: Dev { seen: 0, taken: 0, next: 0, quiet_since: None }, now: 1000, in_flight: None } }

    fn after(letters: &[Letter]) -> Self {
        let mut world = Self::new();
        for &letter in letters { world.apply(letter, letters, &mut Reached::default()); }
        world
    }

    fn apply(&mut self, letter: Letter, path: &[Letter], reached: &mut Reached) {
        match letter {
            Letter::Pass(ns) => self.now += ns,
            Letter::Finish => {
                let seq = self.in_flight.take().expect("a sample is in flight");
                taken(&mut self.watch, seq, self.now);
                self.dev.sampled(self.now);
            }
            Letter::See { start, seq, after_us } => self.see(start, seq, after_us, path, reached),
        }
        let progress = self.watch.progress(self.dev.seen);
        assert_eq!(progress.map_or(0, |p| p.taken), self.dev.taken, "taken differs after {path:?}");
        if let Some(progress) = progress { assert_eq!(progress.due, self.dev.next, "next differs after {path:?}"); }
    }

    fn see(&mut self, start: u64, seq: u64, after_us: u32, path: &[Letter], reached: &mut Reached) {
        let frame = (start != 0).then_some(Running { seq, start, after_us });
        if frame.is_some() && matches!(self.watch, Watch::Quiet { .. } | Watch::Parked { .. }) && self.watch.progress(seq).is_some() { reached.resumed = true; }
        let effect = look(&mut self.watch, frame, self.now);
        assert_eq!(effect, self.dev.look(start, seq, after_us, self.now), "the old loop decided otherwise after {path:?}");
        match effect {
            Effect::Sample { seq, .. } => {
                assert!(frame.is_some(), "sampled with no frame after {path:?}");
                self.in_flight = Some(seq);
                reached.sampled = true;
            }
            Effect::Park => assert!(frame.is_none(), "parked through a running frame after {path:?}"),
            Effect::Sleep(wait) => {
                assert!(wait > Duration::ZERO, "a zero sleep spins, after {path:?}");
                if self.dev.taken >= MOST { reached.full = true; }
            }
        }
        assert!(self.dev.taken <= MOST, "too many samples after {path:?}");
        match self.watch {
            Watch::Waiting { .. } => reached.waiting = true,
            Watch::Sampling(_) => reached.sampling = true,
            Watch::Quiet { .. } => {
                assert_eq!(effect, Effect::Sleep(IDLE_POLL), "after {path:?}");
                reached.quiet = true;
            }
            Watch::Parked { .. } => {
                assert_eq!(effect, Effect::Park, "a parked watchdog has no timer, after {path:?}");
                reached.parked = true;
            }
            Watch::Unarmed => panic!("a look left it unarmed after {path:?}"),
        }
    }
}

fn walk(world: World, depth: usize, path: &mut Vec<Letter>, reached: &mut Reached) {
    for letter in ALPHABET {
        // The thread is one thread: it finishes a sample before it looks again.
        let allowed = match letter {
            Letter::Finish => world.in_flight.is_some(),
            Letter::See { .. } => world.in_flight.is_none(),
            Letter::Pass(_) => true,
        };
        if !allowed { continue; }
        path.push(letter);
        let mut next = world;
        next.apply(letter, path, reached);
        if depth > 1 { walk(next, depth - 1, path, reached); }
        path.pop();
    }
}

#[test]
fn every_short_sequence_of_reads_matches_the_old_loop() {
    let mut reached = Reached::default();
    walk(World::new(), 7, &mut Vec::new(), &mut reached);
    assert!(reached.waiting && reached.sampling && reached.quiet && reached.parked && reached.sampled && reached.resumed);
}

#[test]
fn every_sequence_from_a_parked_watchdog_that_was_sampling_matches_the_old_loop() {
    let world = World::after(&[Letter::Pass(13 * MS), SEE_1, Letter::Finish, NONE, Letter::Pass(REST_NS), NONE]);
    assert!(matches!(world.watch, Watch::Parked { left: Some(_) }));
    let mut reached = Reached::default();
    walk(world, 6, &mut Vec::new(), &mut reached);
    assert!(reached.waiting && reached.sampling && reached.parked && reached.resumed);
}

#[test]
fn every_sequence_from_a_sample_in_flight_matches_the_old_loop() {
    let world = World::after(&[Letter::Pass(13 * MS), SEE_1]);
    assert!(world.in_flight.is_some());
    let mut reached = Reached::default();
    walk(world, 6, &mut Vec::new(), &mut reached);
    assert!(reached.waiting && reached.sampling && reached.quiet && reached.resumed);
}

#[test]
fn every_sequence_from_a_nearly_full_frame_matches_the_old_loop() {
    let mut letters = vec![Letter::Pass(13 * MS)];
    for _ in 0..MOST - 1 { letters.extend([SEE_1, Letter::Finish, Letter::Pass(5 * MS)]); }
    let world = World::after(&letters);
    assert!(matches!(world.watch, Watch::Sampling(Progress { taken, .. }) if taken == MOST - 1));
    let mut reached = Reached::default();
    walk(world, 6, &mut Vec::new(), &mut reached);
    assert!(reached.full && reached.sampling && reached.quiet && reached.resumed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Memory {
    seq: u64,
    start: u64,
    after_us: u32,
}

/// One atomic write of the protocol.
#[derive(Clone, Copy, Debug)]
enum Write {
    NextSeq,
    AfterUs(u32),
    Start(u64),
    Hurry,
}

impl Memory {
    fn apply(&mut self, write: Write) {
        match write {
            Write::NextSeq => self.seq += 1,
            Write::AfterUs(us) => self.after_us = us,
            Write::Start(ns) => self.start = ns,
            Write::Hurry => self.after_us = self.after_us.min(HURRY_US),
        }
    }
}

/// Records the real `begin`'s writes.
#[derive(Default)]
struct Recorder(RefCell<Vec<Write>>);

impl Frames for Recorder {
    fn start(&self) -> u64 { unreachable!("begin only writes") }
    fn seq(&self) -> u64 { unreachable!("begin only writes") }
    fn after_us(&self) -> u32 { unreachable!("begin only writes") }
    fn next_seq(&self) { self.0.borrow_mut().push(Write::NextSeq); }
    fn set_after_us(&self, us: u32) { self.0.borrow_mut().push(Write::AfterUs(us)); }
    fn set_start(&self, ns: u64) { self.0.borrow_mut().push(Write::Start(ns)); }
}

/// Memory where the writes scheduled before each read land as that read happens.
struct Scripted<'a> {
    memory: Cell<Memory>,
    writes: &'a [Write],
    landed_by: [usize; 3],
    landed: Cell<usize>,
    reads: Cell<usize>,
}

impl Scripted<'_> {
    fn read(&self) -> Memory {
        let mut memory = self.memory.get();
        let upto = self.landed_by[self.reads.get()];
        for &write in &self.writes[self.landed.get()..upto] { memory.apply(write); }
        self.landed.set(upto);
        self.reads.set(self.reads.get() + 1);
        self.memory.set(memory);
        memory
    }
}

impl Frames for Scripted<'_> {
    fn start(&self) -> u64 { self.read().start }
    fn seq(&self) -> u64 { self.read().seq }
    fn after_us(&self) -> u32 { self.read().after_us }
    fn next_seq(&self) { unreachable!("a look only reads") }
    fn set_after_us(&self, _: u32) { unreachable!("a look only reads") }
    fn set_start(&self, _: u64) { unreachable!("a look only reads") }
}

/// `arm`'s stores on `dev`, in its order.
fn dev_arm(now: u64) -> [Write; 3] { [Write::NextSeq, Write::AfterUs(AFTER_US), Write::Start(now)] }

/// The loop's reads on `dev`, in its order, each after the writes scheduled before it.
fn dev_reads(mut memory: Memory, writes: &[Write], landed_by: [usize; 3]) -> (u64, u64, u32) {
    let mut landed = 0;
    let mut land = |memory: &mut Memory, upto: usize| {
        for &write in &writes[landed..upto] { memory.apply(write); }
        landed = upto;
    };
    land(&mut memory, landed_by[0]);
    let start = memory.start;
    if start == 0 { return (0, 0, 0); }
    land(&mut memory, landed_by[1]);
    let seq = memory.seq;
    // differs from dev: `AFTER_US` is read even once a frame has samples; the value goes unused then.
    land(&mut memory, landed_by[2]);
    (start, seq, memory.after_us)
}

#[derive(Clone, Copy, Debug)]
enum Program {
    Arm,
    Disarm,
    Hurry,
}

fn writes_of(program: &[Program], arm: &[Write]) -> Vec<Write> {
    program.iter().flat_map(|step| match step {
        Program::Arm => arm.to_vec(),
        Program::Disarm => vec![Write::Start(0)],
        Program::Hurry => vec![Write::Hurry],
    }).collect()
}

#[test]
fn every_write_landing_between_a_looks_reads_matches_the_old_reads() {
    let recorder = Recorder::default();
    begin(&recorder, || NEW);
    let real_arm = recorder.0.into_inner();
    let reference_arm = dev_arm(NEW);
    let programs: [&[Program]; 6] = [
        &[Program::Arm],
        &[Program::Disarm, Program::Arm],
        &[Program::Hurry, Program::Arm],
        &[Program::Hurry, Program::Disarm, Program::Arm, Program::Hurry],
        &[Program::Arm, Program::Disarm],
        &[Program::Arm, Program::Hurry],
    ];
    let memories = [Memory { seq: 1, start: OLD, after_us: AFTER_US }, Memory { seq: 1, start: OLD, after_us: HURRY_US }, Memory { seq: 1, start: 0, after_us: AFTER_US }];
    let now = NEW + 13 * MS;
    let seeds = [
        (Watch::Unarmed, Dev { seen: 0, taken: 0, next: 0, quiet_since: None }),
        (sampling(1, 3, now - MS), Dev { seen: 1, taken: 3, next: now - MS, quiet_since: None }),
        (sampling(1, MOST, 0), Dev { seen: 1, taken: MOST, next: 0, quiet_since: None }),
        (sampling(2, 3, now + MS), Dev { seen: 2, taken: 3, next: now + MS, quiet_since: None }),
    ];
    let mut mixed = false;
    for memory in memories {
        for program in programs {
            let (real, reference) = (writes_of(program, &real_arm), writes_of(program, &reference_arm));
            assert_eq!(real.len(), reference.len(), "arm's writes changed");
            for first in 0..=real.len() {
                for second in first..=real.len() {
                    for third in second..=real.len() {
                        let landed_by = [first, second, third];
                        let scripted = Scripted { memory: Cell::new(memory), writes: &real, landed_by, landed: Cell::new(0), reads: Cell::new(0) };
                        let frame = super::look(&scripted);
                        let (start, seq, after_us) = dev_reads(memory, &reference, landed_by);
                        let context = format!("{memory:?} {program:?} {landed_by:?}");
                        assert_eq!(frame, (start != 0).then_some(Running { seq, start, after_us }), "the reads differ: {context}");
                        if start == OLD && seq == 2 { mixed = true; }
                        for (mut watch, mut dev) in seeds {
                            assert_eq!(look(&mut watch, frame, now), dev.look(start, seq, after_us, now), "the decision differs: {context}");
                        }
                    }
                }
            }
        }
    }
    assert!(mixed, "the walk never read one frame's start with the next frame's number");
}

fn raw() -> Raw { Raw { pcs: [0; DEPTH], len: 0, waiting: false, cpu_us: 0 } }

fn named(at_ns: u64) -> Sampled { Sampled { frames: vec![Frame { name: "app::stall".into(), app: true }], waiting: false, cpu_us: 0, at_ns } }

#[test]
fn a_disarm_during_naming_still_gets_the_stack_captured_for_its_frame() {
    let (samples, taken) = (Mutex::new(Vec::new()), AtomicBool::new(false));
    let (samples, taken) = (&samples, &taken);
    let (resumed, on_resume) = mpsc::channel();
    let (flag_read, on_flag_read) = mpsc::channel();
    let stacks = std::thread::scope(|scope| {
        let ui = scope.spawn(move || {
            on_resume.recv().unwrap();
            let published = was_taken(taken);
            flag_read.send(()).unwrap();
            if published { collect(samples, 7) } else { None }
        });
        capture(samples, taken, 7, |caught| {
            caught();
            resumed.send(()).unwrap();
            Some(raw())
        }, |_| {
            on_flag_read.recv().unwrap();
            named(1)
        });
        ui.join().unwrap()
    });
    assert_eq!(stacks.map(|s| s.taken), Some(1), "the frame that was captured reports it");
    assert!(!was_taken(taken), "nothing is left over for the next frame");
}

fn is_locked(samples: &Mutex<Vec<(u64, Sampled)>>) -> bool { matches!(samples.try_lock(), Err(TryLockError::WouldBlock)) }

#[test]
fn a_capture_publishes_while_the_thread_is_stopped_and_holds_the_samples_until_the_sample_is_in() {
    // differs from dev: "published" comes before "resumed". Dev published after "pushed", so a
    // `disarm` between them found nothing and the next frame's filtered the sample out.
    const REFERENCE: [&str; 5] = ["stopped", "published", "resumed", "named", "pushed"];
    let (samples, taken) = (Mutex::new(Vec::new()), AtomicBool::new(false));
    let events = RefCell::new(Vec::new());
    capture(&samples, &taken, 7, |caught| {
        assert!(is_locked(&samples), "locked before the stop");
        events.borrow_mut().push("stopped");
        caught();
        if taken.load(Ordering::Acquire) { events.borrow_mut().push("published"); }
        events.borrow_mut().push("resumed");
        Some(raw())
    }, |_| {
        assert!(is_locked(&samples), "still locked while naming");
        events.borrow_mut().push("named");
        named(1)
    });
    if samples.lock().unwrap().len() == 1 { events.borrow_mut().push("pushed"); }
    assert_eq!(*events.borrow(), REFERENCE);
}

#[test]
fn a_refused_sample_publishes_nothing_and_a_full_buffer_is_cleared() {
    let (samples, taken) = (Mutex::new(Vec::new()), AtomicBool::new(false));
    capture(&samples, &taken, 7, |_| None, |_| panic!("nothing to name"));
    assert!(!taken.load(Ordering::Acquire));
    assert!(samples.lock().unwrap().is_empty());
    assert!(!is_locked(&samples), "released on the way out");

    samples.lock().unwrap().extend((0..4 * MOST).map(|_| (1, named(0))));
    capture(&samples, &taken, 7, |caught| { caught(); Some(raw()) }, |_| named(1));
    assert_eq!(samples.lock().unwrap().len(), 1);
}
