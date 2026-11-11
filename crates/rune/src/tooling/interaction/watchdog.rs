//! The watchdog: a thread that looks at the UI thread's stack while a frame is still running
//! long, so a slow interaction says which functions its time went to. Sampling after the frame
//! is too late; the hitch is over by then.
//!
//! A frame arms it ([`arm`]) by publishing its start, and [`disarm`] ends it. The watchdog
//! sleeps until the frame is [`AFTER_MS`] old ([`HURRY_MS`] for a signature that was slow
//! before), then samples every [`EVERY`] until the frame ends. Between frames it checks every
//! [`IDLE_POLL`], and after a second with no frame it parks, so a resting app wakes it never
//! and the UI thread unparks it only for the first frame after a rest.
//!
//! The thread's loop is one machine ([`Watch`], [`step`]) that decides from what it looks at;
//! [`Shared`]'s atomics only carry frames across to it, so the UI thread takes no lock.
//!
//! Started by the first armed frame, on the thread that armed it; frames on other threads (a
//! second runtime on a worker) aren't watched. Apple Silicon Macs only (`stack`).
#![cfg_attr(not(all(target_os = "macos", target_arch = "aarch64", not(test))), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use super::stack::Frame;
use crate::machine::Effects;

/// A frame this old is sampled.
pub(crate) const AFTER_MS: u32 = 12;
/// A frame whose interaction was slow before is sampled from this age.
pub(crate) const HURRY_MS: u32 = 4;
/// Samples per frame at most.
const MOST: usize = 64;
/// Between samples of one long frame.
const EVERY: Duration = Duration::from_millis(4);
/// Between looks for a new frame while frames are coming.
const IDLE_POLL: Duration = Duration::from_millis(8);
/// No frame for this long and the watchdog parks until the next one unparks it.
const REST: Duration = Duration::from_secs(1);

/// The frame running when the watchdog looked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Running {
    seq: u64,
    /// In ns on the watchdog's clock.
    start: u64,
    after_us: u32,
}

/// What the watchdog's thread is doing. Clock values are ns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Watch {
    /// It hasn't looked yet.
    Unarmed,
    /// Frame `seq` runs, not yet sampled; old enough at `due`.
    Waiting { seq: u64, due: u64 },
    /// Frame `seq` runs and `taken` samples were tried; the next is at `due`.
    Sampling { seq: u64, taken: usize, due: u64 },
    /// No frame since `since`.
    Quiet { since: u64 },
    /// No frame for [`REST`]: asleep until a frame unparks it.
    Parked,
}

#[derive(Clone, Copy, Debug)]
enum Event {
    /// The thread looked at what's running.
    Look { frame: Option<Running>, now: u64 },
    /// A sample of frame `seq` was tried, and ended at `now`.
    Taken { seq: u64, now: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    /// Sleep this long, or until unparked, then look again.
    Sleep(Duration),
    /// Sleep until unparked, then look again.
    Park,
    /// Try a sample of frame `seq`, stamped `at`, then report [`Event::Taken`].
    Sample { seq: u64, at: u64 },
}

fn step(watch: &mut Watch, event: Event) -> Effects<Effect> {
    match event {
        Event::Look { frame: None, now } => {
            let since = match *watch {
                Watch::Parked => return Effects::one(Effect::Park),
                Watch::Quiet { since } => since,
                _ => now,
            };
            if now.saturating_sub(since) < REST.as_nanos() as u64 {
                *watch = Watch::Quiet { since };
                Effects::one(Effect::Sleep(IDLE_POLL))
            } else {
                *watch = Watch::Parked;
                Effects::one(Effect::Park)
            }
        }
        Event::Look { frame: Some(Running { seq, start, after_us }), now } => {
            // Unsampled, the due time follows `after_us`, which `hurry` can bring forward.
            let (taken, due) = match *watch {
                Watch::Sampling { seq: held, taken, due } if held == seq => (taken, due),
                _ => (0, start + u64::from(after_us) * 1000),
            };
            *watch = if taken == 0 { Watch::Waiting { seq, due } } else { Watch::Sampling { seq, taken, due } };
            Effects::one(if taken >= MOST {
                Effect::Sleep(IDLE_POLL)
            } else if now < due {
                Effect::Sleep(Duration::from_nanos(due - now))
            } else {
                Effect::Sample { seq, at: now }
            })
        }
        Event::Taken { seq, now } => {
            let taken = match *watch {
                Watch::Waiting { seq: held, .. } if held == seq => 0,
                Watch::Sampling { seq: held, taken, .. } if held == seq => taken,
                _ => return Effects::none(),
            };
            *watch = Watch::Sampling { seq, taken: taken + 1, due: now + EVERY.as_nanos() as u64 };
            Effects::none()
        }
    }
}

/// The next read of a look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    Version,
    Payload(u64),
    Recheck(Running),
}

/// The next write of `arm`, one atomic each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arming {
    /// A frame nobody ended takes a new number first. It stays odd, so a frame being replaced
    /// never looks idle, and a look holding the old number finds it moved.
    Renumber { version: u64, now: u64 },
    After { version: u64, now: u64 },
    Start { version: u64, now: u64 },
    Publish { version: u64 },
}

impl Arming {
    const fn first(version: u64, now: u64) -> Self { Self::Renumber { version, now } }
}

/// What the UI thread tells the watchdog. Only the watched thread writes a frame, and it keeps
/// the last `version` itself, so it never reads this one back.
struct Shared {
    /// Moved on by each frame's start and end: odd while one runs, when it names that frame.
    version: AtomicU64,
    /// The running frame's start. Written only after `version` has moved off the frame before,
    /// so a look that reads a newer one finds the change when it rechecks.
    start: AtomicU64,
    after_us: AtomicU32,
    parked: AtomicBool,
}

impl Shared {
    const fn new() -> Self {
        Self { version: AtomicU64::new(0), start: AtomicU64::new(0), after_us: AtomicU32::new(AFTER_MS * 1000), parked: AtomicBool::new(false) }
    }

    /// A frame starts at `now`, whether or not the last one ended: the new `version`.
    fn arm(&self, version: u64, now: u64) -> u64 {
        let mut at = Arming::first(version, now);
        loop {
            match self.arm_step(at) {
                Ok(version) => return version,
                Err(next) => at = next,
            }
        }
    }

    /// One write of `arm`: the new `version`, or the write to make next.
    #[inline(always)]
    fn arm_step(&self, at: Arming) -> Result<u64, Arming> {
        match at {
            Arming::Renumber { version, now } => {
                let version = if version % 2 == 1 {
                    self.version.store(version + 2, Ordering::SeqCst);
                    version + 2
                } else {
                    version
                };
                Err(Arming::After { version, now })
            }
            Arming::After { version, now } => {
                self.after_us.store(AFTER_MS * 1000, Ordering::Release);
                Err(Arming::Start { version, now })
            }
            Arming::Start { version, now } => {
                self.start.store(now, Ordering::Release);
                Err(Arming::Publish { version })
            }
            Arming::Publish { version } => {
                let version = (version + 1) | 1;
                self.version.store(version, Ordering::SeqCst);
                Ok(version)
            }
        }
    }

    fn hurry(&self) { self.after_us.fetch_min(HURRY_MS * 1000, Ordering::Relaxed); }

    /// The running frame ends, if one runs: the new `version`, one past the frame that ran last.
    fn disarm(&self, version: u64) -> u64 {
        if version % 2 == 0 { return version; }
        self.version.store(version + 1, Ordering::SeqCst);
        version + 1
    }

    /// The frame running now, with its own start: a look a frame change lands in starts over.
    fn look(&self) -> Option<Running> {
        let mut read = Read::Version;
        loop {
            match self.read(read) {
                Ok(found) => return found,
                Err(next) => read = next,
            }
        }
    }

    /// One read of a look: what it found, or the read to make next.
    fn read(&self, read: Read) -> Result<Option<Running>, Read> {
        match read {
            Read::Version => {
                let seq = self.version.load(Ordering::SeqCst);
                if seq % 2 == 0 { Ok(None) } else { Err(Read::Payload(seq)) }
            }
            Read::Payload(seq) => Err(Read::Recheck(Running { seq, start: self.start.load(Ordering::Acquire), after_us: self.after_us.load(Ordering::Acquire) })),
            Read::Recheck(frame) => if self.still(frame.seq) { Ok(Some(frame)) } else { Err(Read::Version) },
        }
    }

    /// Whether frame `seq` is the one running.
    fn still(&self, seq: u64) -> bool { self.version.load(Ordering::SeqCst) == seq }

    /// The watchdog is about to park: false if a frame has started. `arm` publishes and then
    /// reads `is_parked`, so one side always sees the other.
    fn may_park(&self) -> bool {
        self.parked.store(true, Ordering::SeqCst);
        self.version.load(Ordering::SeqCst) % 2 == 0
    }

    fn is_parked(&self) -> bool { self.parked.load(Ordering::SeqCst) }

    fn unparked(&self) { self.parked.store(false, Ordering::SeqCst); }
}

/// The samples a slow frame took, summarized.
pub(crate) struct Stacks {
    pub taken: u32,
    /// Samples that found the thread waiting (IO, a lock, a sleep) rather than running.
    pub waiting: u32,
    /// The thread's CPU time over the sampled stretch as a share of its wall time, from two or
    /// more samples.
    pub cpu_share: Option<f64>,
    /// The functions samples were in, by count: each sample's innermost app frame, else its
    /// innermost frame.
    pub hot: Vec<(Arc<str>, u32)>,
    /// A sample at the hottest function, innermost frame first.
    pub stack: Vec<Arc<str>>,
}

/// One sample, named.
pub(crate) struct Sampled {
    pub frames: Vec<Frame>,
    pub waiting: bool,
    pub cpu_us: u64,
    pub at_ns: u64,
}

impl Stacks {
    pub(crate) fn of(samples: &[Sampled]) -> Option<Self> {
        let first = samples.first()?;
        let spot = |s: &Sampled| s.frames.iter().find(|f| f.app).or(s.frames.first()).map(|f| f.name.clone());
        let mut hot: Vec<(Arc<str>, u32)> = Vec::new();
        for name in samples.iter().filter_map(spot) {
            match hot.iter_mut().find(|(n, _)| *n == name) {
                Some((_, count)) => *count += 1,
                None => hot.push((name, 1)),
            }
        }
        hot.sort_by_key(|h| std::cmp::Reverse(h.1));
        let top = hot.first().map(|h| h.0.clone());
        let stack = samples.iter().find(|s| spot(s) == top).map(|s| s.frames.iter().take(16).map(|f| f.name.clone()).collect()).unwrap_or_default();
        hot.truncate(3);
        let last = samples.last()?;
        let wall_us = last.at_ns.saturating_sub(first.at_ns) / 1000;
        Some(Stacks {
            taken: samples.len() as u32,
            waiting: samples.iter().filter(|s| s.waiting).count() as u32,
            cpu_share: (samples.len() >= 2 && wall_us > 0).then(|| last.cpu_us.saturating_sub(first.cpu_us) as f64 / wall_us as f64),
            hot,
            stack,
        })
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64", not(test)))]
pub(crate) use mac::{arm, disarm, hurry};

/// Without a sampler (other systems, unit tests), frames aren't watched.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64", not(test))))]
mod none {
    pub(crate) fn arm() {}
    pub(crate) fn hurry() {}
    pub(crate) fn disarm() -> Option<super::Stacks> { None }
}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64", not(test))))]
pub(crate) use none::{arm, disarm, hurry};

#[cfg(all(target_os = "macos", target_arch = "aarch64", not(test)))]
mod mac {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::thread::{Thread, ThreadId};
    use std::time::Instant;

    use super::super::stack::{Symbols, Target};
    use super::{Effect, Event, MOST, Sampled, Shared, Stacks, Watch, step};

    static SHARED: Shared = Shared::new();
    static TAKEN: AtomicBool = AtomicBool::new(false);
    static SAMPLES: Mutex<Vec<(u64, Sampled)>> = Mutex::new(Vec::new());

    /// Whether a thread's frames are watched.
    #[derive(Clone, Copy)]
    enum Watched {
        NotAsked,
        /// `version` is the last one this thread wrote to `SHARED`.
        Yes { version: u64 },
        No,
    }

    thread_local! {
        static WATCHED: Cell<Watched> = const { Cell::new(Watched::NotAsked) };
    }

    fn epoch() -> Instant {
        static AT: OnceLock<Instant> = OnceLock::new();
        *AT.get_or_init(Instant::now)
    }

    fn now_ns() -> u64 { epoch().elapsed().as_nanos() as u64 }

    /// The watchdog's thread, started on the first call, watching the calling thread.
    fn watchdog() -> Option<&'static (Thread, ThreadId)> {
        static ONE: OnceLock<Option<(Thread, ThreadId)>> = OnceLock::new();
        ONE.get_or_init(|| {
            let target = Target::current()?;
            epoch();
            let thread = std::thread::Builder::new().name("rune-watchdog".into()).spawn(move || run(target)).ok()?;
            Some((thread.thread().clone(), std::thread::current().id()))
        }).as_ref()
    }

    /// A frame starts on this thread.
    pub(crate) fn arm() {
        let version = match WATCHED.get() {
            Watched::Yes { version } => version,
            Watched::No => return,
            Watched::NotAsked => {
                if !watchdog().is_some_and(|w| w.1 == std::thread::current().id()) {
                    WATCHED.set(Watched::No);
                    return;
                }
                0
            }
        };
        WATCHED.set(Watched::Yes { version: SHARED.arm(version, now_ns()) });
        if SHARED.is_parked() { wake(); }
    }

    /// The running frame's interaction was slow before: sample it sooner.
    pub(crate) fn hurry() {
        if !matches!(WATCHED.get(), Watched::Yes { .. }) { return; }
        SHARED.hurry();
        wake();
    }

    fn wake() {
        SHARED.unparked();
        if let Some((thread, _)) = watchdog() { thread.unpark(); }
    }

    /// The frame on this thread ended: what the watchdog saw of it, if it ran long.
    pub(crate) fn disarm() -> Option<Stacks> {
        let Watched::Yes { version } = WATCHED.get() else { return None };
        let ended = SHARED.disarm(version);
        WATCHED.set(Watched::Yes { version: ended });
        if !TAKEN.swap(false, Ordering::Acquire) { return None; }
        let seq = ended - 1;
        let samples: Vec<Sampled> = SAMPLES.lock().ok()?.drain(..).filter(|(s, _)| *s == seq).map(|(_, s)| s).collect();
        Stacks::of(&samples)
    }

    fn run(target: Target) {
        let mut symbols = Symbols::default();
        let mut watch = Watch::Unarmed;
        loop {
            for effect in step(&mut watch, Event::Look { frame: SHARED.look(), now: now_ns() }) {
                match effect {
                    Effect::Sleep(wait) => std::thread::park_timeout(wait),
                    Effect::Park => {
                        if SHARED.may_park() { std::thread::park(); }
                        SHARED.unparked();
                    }
                    Effect::Sample { seq, at } => {
                        if let Some(raw) = target.sample(|| SHARED.still(seq)) {
                            let frames = raw.pcs[..raw.len].iter().enumerate().map(|(i, &pc)| symbols.frame(pc, i == 0)).collect();
                            if let Ok(mut all) = SAMPLES.lock() {
                                // A frame nobody disarmed leaves its samples; keep the buffer bounded.
                                if all.len() >= 4 * MOST { all.clear(); }
                                all.push((seq, Sampled { frames, waiting: raw.waiting, cpu_us: raw.cpu_us, at_ns: at }));
                            }
                            TAKEN.store(true, Ordering::Release);
                        }
                        step(&mut watch, Event::Taken { seq, now: now_ns() });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "watchdog_tests.rs"]
mod tests;
