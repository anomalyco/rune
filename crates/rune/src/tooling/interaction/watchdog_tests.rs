use super::*;

const MS: u64 = 1_000_000;
const EVERY_NS: u64 = EVERY.as_nanos() as u64;
const REST_NS: u64 = REST.as_nanos() as u64;

fn running(seq: u64, start: u64) -> Option<Running> { Some(Running { seq, start, after_us: AFTER_MS * 1000 }) }

fn look(watch: &mut Watch, frame: Option<Running>, now: u64) -> Effect {
    match step(watch, Event::Look { frame, now }) {
        Effects::One(effect) => effect,
        other => panic!("a look decides one thing, got {other:?}"),
    }
}

fn taken(watch: &mut Watch, seq: u64, now: u64) { assert!(step(watch, Event::Taken { seq, now }).is_empty()); }

#[test]
fn a_frame_is_sampled_once_old_enough_and_then_every_few_ms() {
    let mut watch = Watch::Unarmed;
    assert_eq!(look(&mut watch, running(1, 100 * MS), 103 * MS), Effect::Sleep(Duration::from_millis(9)));
    assert_eq!(watch, Watch::Waiting { seq: 1, due: 112 * MS });
    assert_eq!(look(&mut watch, running(1, 100 * MS), 112 * MS), Effect::Sample { seq: 1, at: 112 * MS });
    taken(&mut watch, 1, 113 * MS);
    assert_eq!(watch, Watch::Sampling { seq: 1, taken: 1, due: 113 * MS + EVERY_NS });
    assert_eq!(look(&mut watch, running(1, 100 * MS), 114 * MS), Effect::Sleep(Duration::from_millis(3)));
    assert_eq!(look(&mut watch, running(1, 100 * MS), 117 * MS), Effect::Sample { seq: 1, at: 117 * MS });
}

#[test]
fn hurry_brings_a_waiting_frame_forward_but_not_a_sampling_one() {
    let mut watch = Watch::Unarmed;
    let hurried = Some(Running { seq: 1, start: 100 * MS, after_us: HURRY_MS * 1000 });
    look(&mut watch, running(1, 100 * MS), 101 * MS);
    assert_eq!(look(&mut watch, hurried, 104 * MS), Effect::Sample { seq: 1, at: 104 * MS });
    taken(&mut watch, 1, 104 * MS);
    assert_eq!(look(&mut watch, hurried, 105 * MS), Effect::Sleep(Duration::from_millis(3)), "sampling keeps its own pace");
}

#[test]
fn a_frame_armed_while_sampling_waits_its_own_age() {
    let mut watch = Watch::Unarmed;
    assert_eq!(look(&mut watch, running(1, 100 * MS), 120 * MS), Effect::Sample { seq: 1, at: 120 * MS });
    // Frame 3 starts while frame 1's sample is in flight; the sample still lands on frame 1.
    taken(&mut watch, 1, 121 * MS);
    assert_eq!(look(&mut watch, running(3, 121 * MS), 122 * MS), Effect::Sleep(Duration::from_millis(11)));
    assert_eq!(watch, Watch::Waiting { seq: 3, due: 133 * MS });
}

#[test]
fn a_sample_answering_an_older_frame_changes_nothing() {
    for mut watch in [Watch::Unarmed, Watch::Waiting { seq: 3, due: 9 }, Watch::Sampling { seq: 3, taken: 2, due: 9 }, Watch::Quiet { since: 9 }, Watch::Parked] {
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
    assert_eq!(watch, Watch::Quiet { since: 105 * MS });
}

#[test]
fn going_idle_mid_sample_rests_from_the_next_look_and_then_parks() {
    let mut watch = Watch::Unarmed;
    look(&mut watch, running(1, 100 * MS), 120 * MS);
    taken(&mut watch, 1, 121 * MS);
    assert_eq!(look(&mut watch, None, 122 * MS), Effect::Sleep(IDLE_POLL));
    assert_eq!(look(&mut watch, None, 122 * MS + REST_NS - 1), Effect::Sleep(IDLE_POLL));
    assert_eq!(look(&mut watch, None, 122 * MS + REST_NS), Effect::Park);
    assert_eq!(watch, Watch::Parked);
}

#[test]
fn parked_stays_parked_through_a_wake_with_no_frame_and_watches_the_next_frame() {
    let mut watch = Watch::Parked;
    assert_eq!(look(&mut watch, None, 5000 * MS), Effect::Park);
    assert_eq!(watch, Watch::Parked);
    assert_eq!(look(&mut watch, running(9, 5000 * MS), 5001 * MS), Effect::Sleep(Duration::from_millis(11)));
    assert_eq!(watch, Watch::Waiting { seq: 9, due: 5012 * MS });
}

#[test]
fn a_full_frame_stops_sampling_and_polls() {
    let mut watch = Watch::Sampling { seq: 1, taken: MOST - 1, due: 100 * MS };
    assert_eq!(look(&mut watch, running(1, 0), 100 * MS), Effect::Sample { seq: 1, at: 100 * MS });
    taken(&mut watch, 1, 100 * MS);
    assert_eq!(look(&mut watch, running(1, 0), 500 * MS), Effect::Sleep(IDLE_POLL));
}

#[test]
fn shared_says_which_frame_runs_with_no_reserved_start() {
    let shared = Shared::new();
    assert_eq!(shared.look(), None);
    let version = shared.arm(0, 0);
    assert_eq!(shared.look(), Some(Running { seq: 1, start: 0, after_us: AFTER_MS * 1000 }), "a frame can start at zero");
    assert!(shared.still(1));
    shared.hurry();
    assert_eq!(shared.look().map(|r| r.after_us), Some(HURRY_MS * 1000));
    let ended = shared.disarm(version);
    assert_eq!(ended - 1, 1, "the end names the frame that ran");
    assert_eq!(shared.look(), None);
    assert!(!shared.still(1));
    assert_eq!(shared.disarm(ended), ended, "a second end still names the last frame");
    let version = shared.arm(ended, 7);
    assert_eq!(shared.look(), Some(Running { seq: 3, start: 7, after_us: AFTER_MS * 1000 }));
    shared.arm(version, 8);
    assert_eq!(shared.look().map(|r| r.seq), Some(5), "a frame nobody ended is replaced by a new number");
    assert!(!shared.still(3));
}

#[test]
fn a_frame_and_a_park_always_see_each_other() {
    let armed_first = Shared::new();
    armed_first.arm(0, 1);
    assert!(!armed_first.may_park(), "the frame came first: don't sleep");

    let parked_first = Shared::new();
    assert!(parked_first.may_park());
    parked_first.arm(0, 1);
    assert!(parked_first.is_parked(), "the park came first: the frame unparks it");
    parked_first.unparked();
    assert!(!parked_first.is_parked());

    let ended = Shared::new();
    let version = ended.arm(0, 1);
    ended.disarm(version);
    assert!(ended.may_park(), "between frames it can sleep");
}

#[test]
fn a_repeated_end_writes_nothing() {
    let shared = Shared::new();
    let ended = shared.disarm(shared.arm(0, 1));
    shared.version.store(99, Ordering::Relaxed);
    assert_eq!(shared.disarm(ended), ended);
    assert_eq!(shared.version.load(Ordering::Relaxed), 99, "the writer's own copy says no frame runs");
}

/// The loop as it was with `start == 0` for "no frame", kept as the reference.
#[derive(Clone)]
struct Old {
    seq: u64,
    start: u64,
    after_us: u32,
    seen: u64,
    taken: usize,
    next: u64,
    quiet_since: Option<u64>,
}

impl Old {
    fn look(&mut self, now: u64) -> Effect {
        if self.start == 0 {
            let quiet = *self.quiet_since.get_or_insert(now);
            return if now - quiet < REST_NS { Effect::Sleep(IDLE_POLL) } else { Effect::Park };
        }
        self.quiet_since = None;
        if self.seq != self.seen { (self.seen, self.taken) = (self.seq, 0); }
        if self.taken == 0 { self.next = self.start + u64::from(self.after_us) * 1000; }
        if now < self.next || self.taken >= MOST {
            return Effect::Sleep(if self.taken >= MOST { IDLE_POLL } else { Duration::from_nanos(self.next - now) });
        }
        Effect::Sample { seq: self.seq * 2 - 1, at: now }
    }
}

#[derive(Clone, Copy, Debug)]
enum Letter {
    Arm,
    Disarm,
    Hurry,
    Pass(u64),
    /// The thread's next move: finish the sample in flight, or look.
    Turn,
}

const ALPHABET: [Letter; 7] = [Letter::Arm, Letter::Disarm, Letter::Hurry, Letter::Pass(5 * MS), Letter::Pass(13 * MS), Letter::Pass(REST_NS + 100 * MS), Letter::Turn];

/// What the walk reached, so it can't pass by never getting anywhere.
#[derive(Default)]
struct Reached {
    waiting: bool,
    sampling: bool,
    quiet: bool,
    parked: bool,
    full: bool,
    sampled: bool,
    stale_answer: bool,
}

fn copied(shared: &Shared) -> Shared {
    Shared {
        version: AtomicU64::new(shared.version.load(Ordering::Relaxed)),
        start: AtomicU64::new(shared.start.load(Ordering::Relaxed)),
        after_us: AtomicU32::new(shared.after_us.load(Ordering::Relaxed)),
        parked: AtomicBool::new(shared.parked.load(Ordering::Relaxed)),
    }
}

struct World {
    shared: Shared,
    version: u64,
    watch: Watch,
    old: Old,
    now: u64,
    in_flight: Option<u64>,
    /// Samples started on the frame armed last.
    samples: usize,
}

impl Clone for World {
    fn clone(&self) -> Self {
        Self { shared: copied(&self.shared), version: self.version, watch: self.watch, old: self.old.clone(), now: self.now, in_flight: self.in_flight, samples: self.samples }
    }
}

impl World {
    fn new() -> Self {
        let old = Old { seq: 0, start: 0, after_us: AFTER_MS * 1000, seen: 0, taken: 0, next: 0, quiet_since: None };
        Self { shared: Shared::new(), version: 0, watch: Watch::Unarmed, old, now: 1000, in_flight: None, samples: 0 }
    }

    fn after(letters: &[Letter]) -> Self {
        let mut world = Self::new();
        for &letter in letters { world.apply(letter, letters, &mut Reached::default()); }
        world
    }

    fn apply(&mut self, letter: Letter, path: &[Letter], reached: &mut Reached) {
        match letter {
            Letter::Arm => {
                self.version = self.shared.arm(self.version, self.now);
                self.old.seq += 1;
                self.old.after_us = AFTER_MS * 1000;
                self.old.start = self.now;
                self.samples = 0;
            }
            Letter::Disarm => {
                self.version = self.shared.disarm(self.version);
                self.old.start = 0;
            }
            Letter::Hurry => {
                self.shared.hurry();
                self.old.after_us = self.old.after_us.min(HURRY_MS * 1000);
            }
            Letter::Pass(ns) => self.now += ns,
            Letter::Turn => match self.in_flight.take() {
                Some(seq) => {
                    if !self.shared.still(seq) { reached.stale_answer = true; }
                    taken(&mut self.watch, seq, self.now);
                    self.old.taken += 1;
                    self.old.next = self.now + EVERY_NS;
                }
                None => self.look(path, reached),
            },
        }
    }

    fn look(&mut self, path: &[Letter], reached: &mut Reached) {
        let frame = self.shared.look();
        let effect = look(&mut self.watch, frame, self.now);
        assert_eq!(effect, self.old.look(self.now), "the old loop decided otherwise after {path:?}");
        match (frame, effect) {
            (Some(frame), Effect::Sample { seq, at }) => {
                assert_eq!((seq, at), (frame.seq, self.now), "after {path:?}");
                if self.samples == 0 { assert!(self.now >= frame.start + u64::from(frame.after_us) * 1000, "sampled too young after {path:?}"); }
                self.samples += 1;
                assert!(self.samples <= MOST, "too many samples after {path:?}");
                self.in_flight = Some(seq);
                reached.sampled = true;
            }
            (None, Effect::Sample { .. }) => panic!("sampled with no frame after {path:?}"),
            (Some(_), Effect::Park) => panic!("parked through a running frame after {path:?}"),
            (_, Effect::Sleep(wait)) => {
                assert!(wait > Duration::ZERO && wait <= Duration::from_millis(u64::from(AFTER_MS)), "slept {wait:?} after {path:?}");
                if self.samples >= MOST { reached.full = true; }
            }
            (None, Effect::Park) => {}
        }
        match (frame, self.watch) {
            (Some(frame), Watch::Waiting { seq, .. }) => {
                assert_eq!(seq, frame.seq, "after {path:?}");
                reached.waiting = true;
            }
            (Some(frame), Watch::Sampling { seq, .. }) => {
                assert_eq!(seq, frame.seq, "after {path:?}");
                reached.sampling = true;
            }
            (None, Watch::Quiet { .. }) => {
                assert_eq!(effect, Effect::Sleep(IDLE_POLL), "after {path:?}");
                reached.quiet = true;
            }
            (None, Watch::Parked) => {
                assert_eq!(effect, Effect::Park, "a parked watchdog has no timer, after {path:?}");
                reached.parked = true;
            }
            (frame, watch) => panic!("{watch:?} with {frame:?} after {path:?}"),
        }
    }
}

fn walk(world: &World, depth: usize, path: &mut Vec<Letter>, reached: &mut Reached) {
    for letter in ALPHABET {
        path.push(letter);
        let mut next = world.clone();
        next.apply(letter, path, reached);
        if depth > 1 { walk(&next, depth - 1, path, reached); }
        path.pop();
    }
}

#[test]
fn every_short_interleaving_matches_the_old_loop() {
    let mut reached = Reached::default();
    walk(&World::new(), 7, &mut Vec::new(), &mut reached);
    assert!(reached.waiting && reached.sampling && reached.quiet && reached.parked && reached.sampled && reached.stale_answer);
}

#[test]
fn every_interleaving_from_a_parked_watchdog_matches_the_old_loop() {
    let world = World::after(&[Letter::Turn, Letter::Pass(REST_NS), Letter::Turn]);
    assert_eq!(world.watch, Watch::Parked);
    let mut reached = Reached::default();
    walk(&world, 6, &mut Vec::new(), &mut reached);
    assert!(reached.waiting && reached.parked && reached.sampled);
}

#[test]
fn every_interleaving_from_a_sample_in_flight_matches_the_old_loop() {
    let world = World::after(&[Letter::Arm, Letter::Pass(13 * MS), Letter::Turn]);
    assert!(world.in_flight.is_some());
    let mut reached = Reached::default();
    walk(&world, 6, &mut Vec::new(), &mut reached);
    assert!(reached.waiting && reached.sampling && reached.quiet && reached.stale_answer);
}

#[test]
fn every_interleaving_from_a_nearly_full_frame_matches_the_old_loop() {
    let mut letters = vec![Letter::Arm, Letter::Pass(13 * MS)];
    for _ in 0..MOST - 1 { letters.extend([Letter::Turn, Letter::Turn, Letter::Pass(5 * MS)]); }
    let world = World::after(&letters);
    assert!(matches!(world.watch, Watch::Sampling { taken, .. } if taken == MOST - 1));
    let mut reached = Reached::default();
    walk(&world, 6, &mut Vec::new(), &mut reached);
    assert!(reached.full && reached.sampling && reached.quiet);
}

/// Where the old loop is in its separate reads of the shared state.
#[derive(Clone, Copy, Debug)]
enum OldAt {
    Start,
    Seq { start: u64 },
    After { start: u64, seq: u64 },
    /// The thread is stopped: the seq and start check, then the sample.
    Suspend { start: u64, seq: u64, at: u64 },
}

/// What one turn of a thread came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turned {
    Reading,
    Retried,
    Decided(Effect),
    Sampled { seq: u64, at: u64 },
    Refused,
}

#[derive(Clone, Copy, Debug)]
struct Truth {
    seq: u64,
    start: u64,
    after_us: u32,
}

const SPLIT_ALPHABET: [Letter; 6] = [Letter::Arm, Letter::Disarm, Letter::Hurry, Letter::Pass(5 * MS), Letter::Pass(13 * MS), Letter::Turn];

/// What the split walk reached.
#[derive(Default)]
struct SplitReached {
    retried: bool,
    refused: bool,
    sampled: bool,
    capped: bool,
    old_refused: bool,
    old_capped: bool,
}

/// Both loops on one schedule, each making one shared read per turn and each write of a frame's
/// start landing on its own, so a frame can change between any two of them.
struct Split {
    shared: Shared,
    version: u64,
    /// The next write of the frame being armed: 0 when none is underway.
    arm_at: u8,
    written: u64,
    truth: Option<Truth>,
    watch: Watch,
    read: Read,
    in_flight: Option<(u64, u64)>,
    old: Old,
    old_at: OldAt,
    now: u64,
}

impl Clone for Split {
    fn clone(&self) -> Self {
        Self { shared: copied(&self.shared), version: self.version, arm_at: self.arm_at, written: self.written, truth: self.truth, watch: self.watch, read: self.read, in_flight: self.in_flight, old: self.old.clone(), old_at: self.old_at, now: self.now }
    }
}

impl Split {
    fn new() -> Self {
        let old = Old { seq: 0, start: 0, after_us: AFTER_MS * 1000, seen: 0, taken: 0, next: 0, quiet_since: None };
        Self { shared: Shared::new(), version: 0, arm_at: 0, written: 0, truth: None, watch: Watch::Unarmed, read: Read::Version, in_flight: None, old, old_at: OldAt::Start, now: 1000 }
    }

    /// Frame 1 has run long and been sampled `taken` times, and both loops are about to look.
    fn sampled(taken: usize) -> Self {
        let mut split = Self::new();
        for _ in 0..3 { split.write(); }
        split.now = 1000 * MS;
        split.watch = Watch::Sampling { seq: 1, taken, due: split.now };
        (split.old.seen, split.old.taken, split.old.next) = (1, taken, split.now);
        split
    }

    /// One write of `arm`, in its order; the old loop's three stores beside them.
    fn write(&mut self) {
        match self.arm_at {
            0 => {
                self.version = self.shared.disarm(self.version);
                self.truth = None;
                self.old.seq += 1;
            }
            1 => {
                self.shared.write(self.now);
                self.written = self.now;
                self.old.after_us = AFTER_MS * 1000;
            }
            _ => {
                self.version = self.shared.publish(self.version);
                self.truth = Some(Truth { seq: self.version, start: self.written, after_us: AFTER_MS * 1000 });
                self.old.start = self.now;
            }
        }
        self.arm_at = (self.arm_at + 1) % 3;
    }

    fn disarm(&mut self) {
        self.version = self.shared.disarm(self.version);
        self.truth = None;
        self.old.start = 0;
    }

    /// The watchdog's next move: one read of a look, or the stopped thread's check.
    fn turn(&mut self) -> Turned {
        if let Some((seq, at)) = self.in_flight.take() {
            let accepted = self.shared.still(seq);
            taken(&mut self.watch, seq, self.now);
            return if accepted { Turned::Sampled { seq, at } } else { Turned::Refused };
        }
        let before = self.read;
        match self.shared.read(self.read) {
            Err(next) => {
                self.read = next;
                if matches!((before, next), (Read::Recheck(_), Read::Version)) { Turned::Retried } else { Turned::Reading }
            }
            Ok(found) => {
                self.read = Read::Version;
                if let Some(frame) = found {
                    let truth = self.truth.expect("a look found a frame while none ran");
                    assert_eq!((frame.seq, frame.start), (truth.seq, truth.start), "a look mixed two frames");
                    assert!(frame.after_us >= truth.after_us);
                } else {
                    assert!(self.truth.is_none(), "a look missed the running frame");
                }
                let effect = look(&mut self.watch, found, self.now);
                if let Effect::Sample { seq, at } = effect { self.in_flight = Some((seq, at)); }
                Turned::Decided(effect)
            }
        }
    }

    fn old_decide(&mut self, start: u64, seq: u64) -> Turned {
        if self.now < self.old.next || self.old.taken >= MOST {
            self.old_at = OldAt::Start;
            return Turned::Decided(Effect::Sleep(if self.old.taken >= MOST { IDLE_POLL } else { Duration::from_nanos(self.old.next - self.now) }));
        }
        self.old_at = OldAt::Suspend { start, seq, at: self.now };
        Turned::Reading
    }

    /// The old loop's next move.
    fn old_turn(&mut self) -> Turned {
        match self.old_at {
            OldAt::Start => {
                let start = self.old.start;
                if start == 0 {
                    let quiet = *self.old.quiet_since.get_or_insert(self.now);
                    return Turned::Decided(if self.now - quiet < REST_NS { Effect::Sleep(IDLE_POLL) } else { Effect::Park });
                }
                self.old.quiet_since = None;
                self.old_at = OldAt::Seq { start };
                Turned::Reading
            }
            OldAt::Seq { start } => {
                let seq = self.old.seq;
                if seq != self.old.seen { (self.old.seen, self.old.taken) = (seq, 0); }
                if self.old.taken == 0 {
                    self.old_at = OldAt::After { start, seq };
                    return Turned::Reading;
                }
                self.old_decide(start, seq)
            }
            OldAt::After { start, seq } => {
                self.old.next = start + u64::from(self.old.after_us) * 1000;
                self.old_decide(start, seq)
            }
            OldAt::Suspend { start, seq, at } => {
                self.old_at = OldAt::Start;
                let accepted = seq == self.old.seq && start == self.old.start;
                self.old.taken += 1;
                self.old.next = self.now + EVERY_NS;
                if accepted { Turned::Sampled { seq, at } } else { Turned::Refused }
            }
        }
    }

    fn apply(&mut self, letter: Letter, path: &[Letter], reached: &mut SplitReached) {
        match letter {
            Letter::Arm => self.write(),
            Letter::Disarm => self.disarm(),
            Letter::Hurry => {
                self.shared.hurry();
                if let Some(truth) = &mut self.truth { truth.after_us = truth.after_us.min(HURRY_MS * 1000); }
                self.old.after_us = self.old.after_us.min(HURRY_MS * 1000);
            }
            Letter::Pass(ns) => self.now += ns,
            Letter::Turn => {
                match self.turn() {
                    Turned::Retried => reached.retried = true,
                    Turned::Refused => reached.refused = true,
                    Turned::Sampled { seq, .. } => {
                        assert_eq!(self.truth.map(|t| t.seq), Some(seq), "a sample of a frame not running after {path:?}");
                        reached.sampled = true;
                    }
                    Turned::Decided(Effect::Sleep(IDLE_POLL)) if matches!(self.watch, Watch::Sampling { taken, .. } if taken >= MOST) => reached.capped = true,
                    _ => {}
                }
                match self.old_turn() {
                    Turned::Refused => reached.old_refused = true,
                    Turned::Decided(Effect::Sleep(IDLE_POLL)) if self.old.start != 0 && self.old.taken >= MOST => {
                        assert_eq!(self.old.seen, self.old.seq, "the old loop slept on another frame's cap after {path:?}");
                        reached.old_capped = true;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn walk_split(world: &Split, depth: usize, path: &mut Vec<Letter>, reached: &mut SplitReached) {
    for letter in SPLIT_ALPHABET {
        // The UI thread is inside `arm`: it can't also end or hurry a frame.
        if world.arm_at != 0 && matches!(letter, Letter::Disarm | Letter::Hurry) { continue; }
        path.push(letter);
        let mut next = world.clone();
        next.apply(letter, path, reached);
        if depth > 1 { walk_split(&next, depth - 1, path, reached); }
        path.pop();
    }
}

#[test]
fn a_frame_change_between_a_looks_reads_is_sampled_at_its_own_age() {
    let mut split = Split::sampled(MOST);
    assert_eq!(split.turn(), Turned::Reading, "the capped frame's version is read");
    assert_eq!(split.old_turn(), Turned::Reading, "the capped frame's start is read");
    split.disarm();
    for _ in 0..3 { split.write(); }
    let start = split.now;
    split.now += 13 * MS;

    assert_eq!(split.turn(), Turned::Reading, "the new frame's start is read");
    assert_eq!(split.turn(), Turned::Retried, "the recheck sees the change");
    assert_eq!(split.turn(), Turned::Reading);
    assert_eq!(split.turn(), Turned::Reading);
    assert_eq!(split.turn(), Turned::Decided(Effect::Sample { seq: 3, at: start + 13 * MS }), "the cap belonged to the older frame");
    assert_eq!(split.turn(), Turned::Sampled { seq: 3, at: start + 13 * MS });

    // The old loop read the new number with the old start, so its first try was refused and it
    // came back 4 ms later: both catch a frame that stalls for 20 ms.
    assert_eq!(split.old_turn(), Turned::Reading);
    assert_eq!(split.old_turn(), Turned::Reading);
    assert_eq!(split.old_turn(), Turned::Refused);
    split.now += EVERY_NS;
    assert_eq!(split.old_turn(), Turned::Reading);
    assert_eq!(split.old_turn(), Turned::Reading);
    assert_eq!(split.old_turn(), Turned::Sampled { seq: 2, at: start + 17 * MS });
}

#[test]
fn a_look_split_by_a_frame_change_never_mixes_frames_from_a_fresh_start() {
    let mut reached = SplitReached::default();
    walk_split(&Split::new(), 8, &mut Vec::new(), &mut reached);
    assert!(reached.retried);
}

#[test]
fn a_look_split_by_a_frame_change_never_mixes_frames_while_sampling() {
    let mut reached = SplitReached::default();
    walk_split(&Split::sampled(3), 8, &mut Vec::new(), &mut reached);
    assert!(reached.retried && reached.refused && reached.sampled && reached.old_refused);
}

#[test]
fn a_look_split_by_a_frame_change_never_sleeps_on_an_older_cap() {
    let mut reached = SplitReached::default();
    walk_split(&Split::sampled(MOST), 8, &mut Vec::new(), &mut reached);
    assert!(reached.retried && reached.capped && reached.old_capped);
}
