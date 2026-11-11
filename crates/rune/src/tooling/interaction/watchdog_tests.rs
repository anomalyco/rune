use std::cell::{Cell, RefCell};
use std::sync::mpsc;

use super::super::stack::{DEPTH, Symbols};
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

/// A sample whose CPU time says which one it is.
fn raw(id: u64) -> Raw {
    let mut pcs = [0; DEPTH];
    pcs[0] = id as usize;
    Raw { pcs, len: 1, waiting: false, cpu_us: id, images: 0 }
}

fn ids(stacks: &Stacks) -> Vec<u64> { stacks.raw.iter().map(|r| r.cpu_us).collect() }

fn was_taken(taken: &AtomicBool) -> bool { taken.load(Ordering::Acquire) }

/// The real capture path, over a stop that finds `sample`.
fn captured(caught: &Caught, taken: &AtomicBool, seq: u64, at: u64, sample: Raw) {
    capture(caught, taken, seq, at, |stopped| {
        stopped(&sample);
        Some(sample)
    });
}

#[test]
fn a_disarm_at_the_resume_boundary_finds_the_stack_of_its_frame() {
    let (caught, taken) = (Caught::new(), AtomicBool::new(false));
    let mut found = None;
    capture(&caught, &taken, 7, 1, |stopped| {
        let sample = raw(42);
        stopped(&sample);
        // The thread has resumed and ends its frame before the watchdog runs another line.
        found = collected(&caught, &taken, || 7);
        Some(sample)
    });
    assert_eq!(found.as_ref().map(ids), Some(vec![42]), "the frame gets its one sample");
    assert!(collected(&caught, &taken, || 9).is_none(), "the next frame gets none");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn a_real_thread_ending_its_frame_as_it_resumes_finds_the_stack() {
    use super::super::stack::Target;
    let (caught, taken, end) = (Caught::new(), AtomicBool::new(false), AtomicBool::new(false));
    let (caught, taken, end) = (&caught, &taken, &end);
    let (ready, got) = mpsc::channel();
    let (finished, on_finished) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            ready.send(Target::current().unwrap()).unwrap();
            while !end.load(Ordering::Acquire) { std::hint::spin_loop(); }
            finished.send(collected(caught, taken, || 7).as_ref().map(ids)).unwrap();
        });
        let target: Target = got.recv().unwrap();
        let mut found = None;
        capture(caught, taken, 7, 1, |stopped| {
            // `end` is set while the thread is stopped, so it ends the frame the moment it resumes.
            let sample = target.sample(|| { end.store(true, Ordering::Release); true }, stopped);
            found = Some(on_finished.recv_timeout(Duration::from_secs(30)).expect("the thread ended its frame"));
            sample
        });
        assert_eq!(found.flatten().map(|samples| samples.len()), Some(1), "the frame gets its one sample");
    });
}

#[test]
fn a_disarm_finishes_while_naming_is_paused() {
    // differs from dev: naming happens after `disarm`, on the exporter's thread. Dev named on
    // the watchdog before storing the sample, so a `disarm` during naming lost it.
    let (caught, taken) = (Caught::new(), AtomicBool::new(false));
    let (caught, taken) = (&caught, &taken);
    captured(caught, taken, 1, 1, raw(11));
    let earlier = collected(caught, taken, || 1).expect("frame 1's stack");
    let (entered, on_entered) = mpsc::channel();
    let (release, on_release) = mpsc::channel::<()>();
    let (done, on_done) = mpsc::channel();
    std::thread::scope(|scope| {
        let namer = scope.spawn(move || {
            earlier.names(|sample| {
                entered.send(()).unwrap();
                on_release.recv().unwrap();
                vec![Frame { name: format!("app::{}", sample.pcs[0]).into(), app: true }]
            })
        });
        on_entered.recv().unwrap();
        scope.spawn(move || {
            captured(caught, taken, 3, 2, raw(33));
            done.send(collected(caught, taken, || 3).map(|s| ids(&s))).unwrap();
        });
        let finished = on_done.recv_timeout(Duration::from_secs(30));
        // Released before any assert, so a failure fails rather than hangs.
        release.send(()).unwrap();
        let names = namer.join().unwrap();
        assert_eq!(finished.expect("disarm waited on naming"), Some(vec![33]), "the raw sample belongs to its own frame");
        assert_eq!(names.stack.first().map(|n| &**n), Some("app::11"));
    });
}

fn frame_named(name: &str) -> Frame { Frame { name: name.into(), app: true } }

fn names_of(frames: &[Frame]) -> Vec<&str> { frames.iter().map(|f| &*f.name).collect() }

#[test]
fn a_sample_is_named_while_its_images_are_still_loaded() {
    let mut symbols = Symbols::default();
    let mut lookups = 0;
    let frames = symbols.frames(&raw(0x40), || 0, |_| { lookups += 1; frame_named("app::work") });
    assert_eq!(names_of(&frames), ["app::work"]);
    let frames = symbols.frames(&raw(0x40), || 0, |_| { lookups += 1; frame_named("app::work") });
    assert_eq!((names_of(&frames), lookups), (vec!["app::work"], 1), "remembered");
}

#[test]
fn an_address_whose_image_was_unloaded_since_the_sample_stays_unresolved() {
    // differs from dev: dev named a sample straight after taking it. Named at export, an address
    // can by then hold another image's code, so it stays bare once any image has been unloaded.
    let mut symbols = Symbols::default();
    let frames = symbols.frames(&raw(0x40), || 1, |_| panic!("no lookup for a sample older than the images"));
    assert_eq!(names_of(&frames), ["0x40"]);
    assert!(!frames[0].app);
}

#[test]
fn an_unload_during_the_lookups_leaves_the_sample_unresolved() {
    let (mut symbols, generation) = (Symbols::default(), Cell::new(0));
    let frames = symbols.frames(&raw(0x40), || generation.get(), |_| { generation.set(1); frame_named("app::replaced") });
    assert_eq!(names_of(&frames), ["0x40"]);
}

#[test]
fn an_unload_forgets_remembered_names() {
    let mut symbols = Symbols::default();
    symbols.frames(&raw(0x40), || 0, |_| frame_named("app::old"));
    let mut newer = raw(0x40);
    newer.images = 1;
    let frames = symbols.frames(&newer, || 1, |_| frame_named("app::new"));
    assert_eq!(names_of(&frames), ["app::new"], "the old image's name isn't reused for the address");
}

#[test]
fn a_frame_keeps_every_sample_up_to_the_cap_and_a_new_frame_replaces_them() {
    let (caught, taken) = (Caught::new(), AtomicBool::new(false));
    for id in 0..MOST as u64 + 1 { captured(&caught, &taken, 1, id, raw(id)); }
    assert_eq!(ids(&caught.collect(1).unwrap()), (0..MOST as u64).collect::<Vec<_>>());
    captured(&caught, &taken, 2, 9, raw(9));
    assert_eq!(ids(&caught.collect(2).unwrap()), [9]);
    assert!(caught.collect(1).is_none(), "the older frame's are gone");
}

#[test]
fn a_sample_round_trips_through_the_store() {
    let (caught, taken) = (Caught::new(), AtomicBool::new(false));
    let mut pcs = [0; DEPTH];
    pcs[..3].copy_from_slice(&[10, 20, 30]);
    captured(&caught, &taken, 1, 77, Raw { pcs, len: 3, waiting: true, cpu_us: 5, images: 4 });
    let stacks = caught.collect(1).unwrap();
    assert_eq!((stacks.raw[0].pcs, stacks.raw[0].len, stacks.raw[0].waiting, stacks.raw[0].cpu_us, stacks.raw[0].images), (pcs, 3, true, 5, 4));
    captured(&caught, &taken, 1, 90_077, raw(6));
    assert_eq!(caught.collect(1).unwrap().cpu_share, Some(1.0 / 90.0), "stamped with each look's clock");
    assert!(was_taken(&taken));
}

/// `SAMPLES` and `TAKEN` on dev, one capture at a time.
#[derive(Default)]
struct DevSamples {
    samples: Vec<(u64, u64)>,
    taken: bool,
}

impl DevSamples {
    // differs from dev: `taken` is set as the sample lands, while the thread is stopped. Dev set
    // it after naming, so a `disarm` in between found nothing and the next frame's filtered the
    // sample out. And there's no clear at 4 * MOST: storage holds one frame's samples.
    fn captured(&mut self, seq: u64, id: u64) {
        self.samples.push((seq, id));
        self.taken = true;
    }

    fn disarm(&mut self, seq: u64) -> Option<Vec<u64>> {
        if !std::mem::take(&mut self.taken) { return None; }
        let samples: Vec<u64> = self.samples.drain(..).filter(|(s, _)| *s == seq).map(|(_, s)| s).collect();
        (!samples.is_empty()).then_some(samples)
    }
}

#[test]
fn every_short_run_of_frames_collects_what_the_old_store_did() {
    #[derive(Clone, Copy, Debug)]
    enum Move { Arm, Capture, Disarm }
    const MOVES: [Move; 3] = [Move::Arm, Move::Capture, Move::Disarm];
    let (mut kept, mut empty) = (false, false);
    for length in 1..=8u32 {
        for number in 0..3usize.pow(length) {
            let path: Vec<Move> = (0..length).map(|i| MOVES[number / 3usize.pow(i) % 3]).collect();
            let (caught, taken, mut dev) = (Caught::new(), AtomicBool::new(false), DevSamples::default());
            let (mut seq, mut running, mut id) = (0, false, 0);
            for &step in &path {
                match step {
                    Move::Arm => (seq, running) = (seq + 1, true),
                    // A stopped thread between frames fails `still`: nothing is stored.
                    Move::Capture if running => {
                        id += 1;
                        captured(&caught, &taken, seq, id, raw(id));
                        dev.captured(seq, id);
                    }
                    Move::Capture => {}
                    Move::Disarm => {
                        running = false;
                        let got = collected(&caught, &taken, || seq).map(|s| ids(&s));
                        kept |= got.is_some();
                        empty |= got.is_none() && id > 0;
                        assert_eq!(got, dev.disarm(seq), "after {path:?}");
                    }
                }
            }
        }
    }
    assert!(kept && empty);
}
