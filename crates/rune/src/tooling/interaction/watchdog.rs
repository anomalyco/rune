//! The watchdog: a thread that looks at the UI thread's stack while a frame is still running
//! long, so a slow interaction says which functions its time went to. Sampling after the frame
//! is too late; the hitch is over by then.
//!
//! A frame arms it ([`arm`]) with a relaxed store of its start, and [`disarm`] clears it. The
//! watchdog sleeps until the frame is [`AFTER_MS`] old ([`HURRY_MS`] for a signature that was
//! slow before), then samples every [`EVERY`] until the frame ends. Between frames it checks
//! every [`IDLE_POLL`], and after a second with no frame it parks, so a resting app wakes it
//! never and the UI thread unparks it only for the first frame after a rest.
//!
//! The thread decodes what it reads once (`look`), and its decisions are one machine ([`Watch`],
//! [`step`]). The atomics are only how a frame gets across.
//!
//! Started by the first armed frame, on the thread that armed it; frames on other threads (a
//! second runtime on a worker) aren't watched. Apple Silicon Macs only (`stack`).
#![cfg_attr(not(all(target_os = "macos", target_arch = "aarch64", not(test))), allow(dead_code))]

use std::sync::Arc;
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

/// What a look read of the running frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Running {
    seq: u64,
    /// In ns on the watchdog's clock.
    start: u64,
    after_us: u32,
}

/// How far the watchdog got with frame `seq`: `taken` samples tried, the next at `due`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Progress {
    seq: u64,
    taken: usize,
    due: u64,
}

/// What the watchdog's thread is doing. Clock values are ns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Watch {
    /// It hasn't looked yet.
    Unarmed,
    /// Frame `seq` runs, not yet sampled; old enough at `due`.
    Waiting { seq: u64, due: u64 },
    Sampling(Progress),
    /// No frame since `since`. `left` is the frame it was sampling: a look can land inside
    /// `arm`, after the number moved and before the start did, so that number can come back.
    Quiet { since: u64, left: Option<Progress> },
    /// No frame for [`REST`]: asleep until a frame unparks it.
    Parked { left: Option<Progress> },
}

impl Watch {
    /// How far it got with frame `seq`, if it has tried to sample it.
    fn progress(self, seq: u64) -> Option<Progress> {
        match self {
            Watch::Sampling(p) | Watch::Quiet { left: Some(p), .. } | Watch::Parked { left: Some(p) } if p.seq == seq => Some(p),
            _ => None,
        }
    }
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
    /// Try a sample of frame `seq`, if it still runs from `start`, stamped `at`; then report
    /// [`Event::Taken`].
    Sample { seq: u64, start: u64, at: u64 },
}

fn step(watch: &mut Watch, event: Event) -> Effects<Effect> {
    match event {
        Event::Look { frame: None, now } => {
            let (since, left) = match *watch {
                Watch::Parked { .. } => return Effects::one(Effect::Park),
                Watch::Quiet { since, left } => (since, left),
                Watch::Sampling(progress) => (now, Some(progress)),
                Watch::Unarmed | Watch::Waiting { .. } => (now, None),
            };
            if now.saturating_sub(since) < REST.as_nanos() as u64 {
                *watch = Watch::Quiet { since, left };
                Effects::one(Effect::Sleep(IDLE_POLL))
            } else {
                *watch = Watch::Parked { left };
                Effects::one(Effect::Park)
            }
        }
        Event::Look { frame: Some(Running { seq, start, after_us }), now } => {
            // Unsampled, the due time follows `after_us`, which `hurry` can bring forward.
            let progress = watch.progress(seq).unwrap_or(Progress { seq, taken: 0, due: start + u64::from(after_us) * 1000 });
            let Progress { taken, due, .. } = progress;
            *watch = if taken == 0 { Watch::Waiting { seq, due } } else { Watch::Sampling(progress) };
            Effects::one(if taken >= MOST {
                Effect::Sleep(IDLE_POLL)
            } else if now < due {
                Effect::Sleep(Duration::from_nanos(due - now))
            } else {
                Effect::Sample { seq, start, at: now }
            })
        }
        Event::Taken { seq, now } => {
            let taken = match *watch {
                Watch::Waiting { seq: held, .. } if held == seq => 0,
                Watch::Sampling(Progress { seq: held, taken, .. }) if held == seq => taken,
                _ => return Effects::none(),
            };
            *watch = Watch::Sampling(Progress { seq, taken: taken + 1, due: now + EVERY.as_nanos() as u64 });
            Effects::none()
        }
    }
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
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::thread::{Thread, ThreadId};
    use std::time::Instant;

    use super::super::stack::{Symbols, Target};
    use super::{AFTER_MS, Effect, Event, HURRY_MS, MOST, Running, Sampled, Stacks, Watch, step};

    /// Bumped by each frame, so a sample can tell its frame is the one still running.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    /// The running frame's start in ns since `epoch` (plus one), or 0 between frames.
    static START: AtomicU64 = AtomicU64::new(0);
    static AFTER_US: AtomicU32 = AtomicU32::new(AFTER_MS * 1000);
    static PARKED: AtomicBool = AtomicBool::new(false);
    static TAKEN: AtomicBool = AtomicBool::new(false);
    static SAMPLES: Mutex<Vec<(u64, Sampled)>> = Mutex::new(Vec::new());

    /// Whether a thread's frames are watched.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Watched {
        NotAsked,
        Yes,
        No,
    }

    thread_local! {
        static WATCHED: Cell<Watched> = const { Cell::new(Watched::NotAsked) };
    }

    fn epoch() -> Instant {
        static AT: OnceLock<Instant> = OnceLock::new();
        *AT.get_or_init(Instant::now)
    }

    fn now_ns() -> u64 { epoch().elapsed().as_nanos() as u64 + 1 }

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
        let watched = match WATCHED.get() {
            Watched::NotAsked => {
                let yes = watchdog().is_some_and(|w| w.1 == std::thread::current().id());
                WATCHED.set(if yes { Watched::Yes } else { Watched::No });
                yes
            }
            w => w == Watched::Yes,
        };
        if !watched { return; }
        SEQ.fetch_add(1, Ordering::Relaxed);
        AFTER_US.store(AFTER_MS * 1000, Ordering::Relaxed);
        START.store(now_ns(), Ordering::SeqCst);
        if PARKED.load(Ordering::SeqCst) { wake(); }
    }

    /// The running frame's interaction was slow before: sample it sooner.
    pub(crate) fn hurry() {
        if WATCHED.get() != Watched::Yes { return; }
        AFTER_US.fetch_min(HURRY_MS * 1000, Ordering::Relaxed);
        wake();
    }

    fn wake() {
        PARKED.store(false, Ordering::SeqCst);
        if let Some((thread, _)) = watchdog() { thread.unpark(); }
    }

    /// The frame on this thread ended: what the watchdog saw of it, if it ran long.
    pub(crate) fn disarm() -> Option<Stacks> {
        if WATCHED.get() != Watched::Yes { return None; }
        START.store(0, Ordering::SeqCst);
        if !TAKEN.swap(false, Ordering::Acquire) { return None; }
        let seq = SEQ.load(Ordering::Relaxed);
        let samples: Vec<Sampled> = SAMPLES.lock().ok()?.drain(..).filter(|(s, _)| *s == seq).map(|(_, s)| s).collect();
        Stacks::of(&samples)
    }

    /// Reads what's running and decodes it. Past here, no frame is `None`, not a zero.
    fn look() -> Option<Running> {
        let start = START.load(Ordering::SeqCst);
        if start == 0 { return None; }
        let seq = SEQ.load(Ordering::SeqCst);
        Some(Running { seq, start, after_us: AFTER_US.load(Ordering::Relaxed) })
    }

    fn run(target: Target) {
        let mut symbols = Symbols::default();
        let mut watch = Watch::Unarmed;
        loop {
            for effect in step(&mut watch, Event::Look { frame: look(), now: now_ns() }) {
                match effect {
                    Effect::Sleep(wait) => std::thread::park_timeout(wait),
                    Effect::Park => {
                        PARKED.store(true, Ordering::SeqCst);
                        if START.load(Ordering::SeqCst) == 0 { std::thread::park(); }
                        PARKED.store(false, Ordering::SeqCst);
                    }
                    Effect::Sample { seq, start, at } => {
                        let still = || SEQ.load(Ordering::SeqCst) == seq && START.load(Ordering::SeqCst) == start;
                        if let Some(raw) = target.sample(still) {
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
