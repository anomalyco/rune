//! Every short sequence of what reaches an overlay's frame and surface state, driven through
//! `Quiet`, `turn`, `Surface` and `Backoff`, checked after every event against a literal
//! transcription of the fields and branches they replaced.

use std::time::Duration;

use super::frame::{Gate, Quiet, QuietEffect, QuietEvent, Turn, turn};
use crate::window::{Backoff, BackoffEvent, Missed, OCCLUDED_MAX, OCCLUDED_RETRY, TIMEOUT_RETRY, TIMER_FRAME};
use super::surface::{Surface, SurfaceEffect};
use crate::time::Instant;

/// Every sequence of `alphabet` up to `len` long, prefixes first.
fn walk<E: Copy>(alphabet: &[E], len: usize, mut run: impl FnMut(&[E])) {
    let mut seq = Vec::new();
    fn go<E: Copy>(alphabet: &[E], len: usize, seq: &mut Vec<E>, run: &mut impl FnMut(&[E])) {
        run(seq);
        if seq.len() == len { return; }
        for &e in alphabet {
            seq.push(e);
            go(alphabet, len, seq, run);
            seq.pop();
        }
    }
    go(alphabet, len, &mut seq, &mut run);
}

#[derive(Clone, Copy, Debug)]
enum QuietOp {
    /// `wake_due` with a due timer and a frame coming; `resting`: `idle && !dirty`.
    TimerDue { resting: bool },
    /// A `draw_frame`: `None` returns before presenting (or draws nothing new), `Some(rests)`
    /// presents a frame the overlay may rest on.
    Draw { presents: Option<bool> },
    /// `occluded` or `stop_drawing`.
    Forget,
}

/// origin/dev's `frame::Quiet`, as it was.
#[derive(Default)]
struct DevQuiet {
    timer: bool,
    presented: Option<u32>,
}

impl DevQuiet {
    /// Returns the frame's quiet candidate and what `Presented::record` was handed.
    fn apply(&mut self, op: QuietOp, n: u32) -> (Option<u32>, Option<Option<u32>>) {
        match op {
            QuietOp::TimerDue { resting } => {
                self.timer |= resting;
                (None, None)
            }
            QuietOp::Draw { presents } => {
                let timer = std::mem::take(&mut self.timer);
                let candidate = self.presented.filter(|_| timer);
                let mut seen = None;
                if let Some(rests) = presents {
                    if rests {
                        seen = Some(self.presented);
                        *self.presented.get_or_insert(n) = n;
                    } else {
                        self.presented = None;
                    }
                }
                (candidate, seen)
            }
            QuietOp::Forget => {
                *self = Default::default();
                (None, None)
            }
        }
    }
}

fn apply_quiet(quiet: &mut Quiet<u32>, op: QuietOp, n: u32) -> (Option<u32>, Option<Option<u32>>) {
    match op {
        QuietOp::TimerDue { resting } => {
            assert!(quiet.step(QuietEvent::TimerDue { resting }).is_empty());
            (None, None)
        }
        QuietOp::Draw { presents } => {
            let timer = match quiet.step(QuietEvent::FrameStarts).into_iter().next() {
                Some(QuietEffect::TimerFrame) => true,
                None => false,
            };
            let candidate = quiet.presented().copied().filter(|_| timer);
            let mut seen = None;
            if let Some(rests) = presents {
                quiet.presented_frame(rests, |slot| {
                    seen = Some(*slot);
                    *slot.get_or_insert(n) = n;
                });
            }
            (candidate, seen)
        }
        QuietOp::Forget => {
            assert!(quiet.step(QuietEvent::Forget).is_empty());
            (None, None)
        }
    }
}

#[test]
fn quiet_matches_dev_for_every_sequence() {
    let alphabet = [
        QuietOp::TimerDue { resting: false },
        QuietOp::TimerDue { resting: true },
        QuietOp::Draw { presents: None },
        QuietOp::Draw { presents: Some(false) },
        QuietOp::Draw { presents: Some(true) },
        QuietOp::Forget,
    ];
    let mut cases = 0;
    walk(&alphabet, 6, |seq| {
        cases += 1;
        let (mut dev, mut quiet) = (DevQuiet::default(), Quiet::<u32>::default());
        for (i, &op) in seq.iter().enumerate() {
            let n = i as u32 + 1;
            assert_eq!(apply_quiet(&mut quiet, op, n), dev.apply(op, n), "{seq:?} at {i}");
            assert_eq!(quiet.presented().copied(), dev.presented, "{seq:?} at {i}");
            // Dev's timer with nothing presented was never read: the next frame took it first.
            let timer = matches!(quiet, Quiet::Rests { timer: true, .. });
            assert_eq!(timer, dev.timer && dev.presented.is_some(), "{seq:?} at {i}");
        }
    });
    assert_eq!(cases, (0..=6).map(|k| 6usize.pow(k)).sum::<usize>());
}

/// origin/dev's `Missed::retry` for an occluded miss: waits `occluded_wait` out (held to
/// `OCCLUDED_RETRY..=OCCLUDED_MAX`), then doubles it.
fn dev_occluded_retry(now: Instant, wait: &mut Duration) -> Option<Instant> {
    let at = now + (*wait).clamp(OCCLUDED_RETRY, OCCLUDED_MAX);
    *wait = (*wait * 2).min(OCCLUDED_MAX);
    Some(at)
}

/// origin/dev's `draw_frame` from the occlusion read to the drawable, as it was (after
/// `sync_occlusion`, which only sets `occluded` on macOS): whether it returned, whether it
/// counted an occluded miss, and the retry it left. An occluded miss runs `missed`, which
/// schedules the next re-check and backs off.
fn dev_gate(prewarm: bool, occluded: bool, dirty: bool, idle: bool, retry: &mut Option<Instant>, wait: &mut Duration, now: Instant) -> (bool, bool, Option<Instant>) {
    if !prewarm && occluded {
        if retry.is_none_or(|retry| retry <= now) {
            *retry = dev_occluded_retry(now, wait);
            return (true, true, *retry);
        }
        return (true, false, *retry);
    }
    if !dirty && idle { return (true, false, *retry); }
    if !prewarm && retry.is_some_and(|retry| retry > now) { return (true, false, *retry); }
    if !prewarm { *retry = None; }
    (false, false, *retry)
}

/// `draw_frame`'s arms for each answer, as the branch writes them: an occluded miss steps the
/// backoff, and a drawn frame clears the wait before it takes its drawable.
fn ours_gate(decided: Turn, prewarm: bool, backoff: &mut Backoff, now: Instant) -> (bool, bool, Option<Instant>) {
    match decided {
        Turn::Occluded { recheck } => {
            if recheck { backoff.step(BackoffEvent::Missed { missed: Missed::Occluded, paced: false, now }); }
            (true, recheck, backoff.deadline())
        }
        Turn::Rest | Turn::BackedOff => (true, false, backoff.deadline()),
        Turn::Draw => {
            if !prewarm { backoff.step(BackoffEvent::Lapsed); }
            (false, false, backoff.deadline())
        }
    }
}

/// A backoff the overlay starts with: it may draw again at `at`, and its occluded wait starts
/// at `wait` (as a long covered spell leaves it, or as dev's fields did).
fn backoff_start(at: Option<Instant>, wait: Duration) -> Backoff {
    let mut backoff = Backoff::seeded(wait, TIMEOUT_RETRY);
    if let Some(at) = at { backoff.step(BackoffEvent::At(at)); }
    backoff
}

/// The occluded wait behind `backoff`'s deadline, which no query shows: a miss now, on a copy.
fn occluded_wait(backoff: Backoff, now: Instant) -> Duration {
    let mut probe = backoff;
    probe.step(BackoffEvent::Missed { missed: Missed::Occluded, paced: false, now });
    probe.deadline().expect("an occluded miss always re-checks") - now
}

#[test]
fn turn_matches_dev_for_every_gate() {
    let now = Instant::now();
    let ms = Duration::from_millis(1);
    let retries = [None, Some(now - ms), Some(now), Some(now + ms)];
    let waits = [OCCLUDED_RETRY, Duration::from_millis(600), OCCLUDED_MAX];
    let mut cases = 0;
    for bits in 0..16u8 {
        let [prewarm, occluded, dirty, idle] = [0, 1, 2, 3].map(|b| bits & (1 << b) != 0);
        for start in retries {
            for start_wait in waits {
                cases += 1;
                let reads = std::cell::Cell::new(0);
                let decided = turn(Gate { prewarm, occluded, dirty, idle, retry: start }, || { reads.set(reads.get() + 1); now });
                let (mut dev_retry, mut dev_wait) = (start, start_wait);
                let mut ours = backoff_start(start, start_wait);
                let arms = ours_gate(decided, prewarm, &mut ours, now);
                assert_eq!(arms, dev_gate(prewarm, occluded, dirty, idle, &mut dev_retry, &mut dev_wait, now), "{decided:?} {prewarm} {occluded} {dirty} {idle} {start:?} {start_wait:?}");
                assert_eq!(occluded_wait(ours, now), dev_wait, "the wait it leaves: {decided:?} {start_wait:?}");
                assert!(reads.get() <= 1 && (start.is_some() || reads.get() == 0), "the clock is read only to compare a retry");
            }
        }
    }
    assert_eq!(cases, 192);
}

/// Each event happens `delta` after the last, so a re-check comes due and misses again, backing
/// the wait off towards its cap.
#[derive(Clone, Copy, Debug)]
struct TurnOp { delta: Duration, occluded: bool }

#[test]
fn occluded_misses_back_off_like_dev() {
    let base = Instant::now();
    let alphabet = [
        TurnOp { delta: Duration::ZERO, occluded: true },
        TurnOp { delta: Duration::from_millis(100), occluded: true },
        TurnOp { delta: Duration::from_millis(260), occluded: true },
        TurnOp { delta: Duration::from_secs(3), occluded: true },
        TurnOp { delta: Duration::from_secs(3), occluded: false },
    ];
    walk(&alphabet, 5, |seq| {
        let mut dev = (None, OCCLUDED_RETRY);
        let mut ours = backoff_start(None, OCCLUDED_RETRY);
        let mut now = base;
        for (i, &op) in seq.iter().enumerate() {
            now = now + op.delta;
            let decided = turn(Gate { prewarm: false, occluded: op.occluded, dirty: false, idle: true, retry: ours.deadline() }, || now);
            let ours_arms = ours_gate(decided, false, &mut ours, now);
            let dev_arms = dev_gate(false, op.occluded, false, true, &mut dev.0, &mut dev.1, now);
            assert_eq!(ours_arms, dev_arms, "{seq:?} at {i}");
            assert_eq!(ours.deadline(), dev.0, "{seq:?} at {i}");
            assert_eq!(occluded_wait(ours, now), dev.1, "{seq:?} at {i}");
        }
    });
}

/// origin/dev's overlay fields, as they were: `retry` and `occluded_wait`, written by `missed`,
/// `wake_due`, `draw_frame`, `stop_drawing`, a show and an occlusion change.
#[derive(Clone, Copy)]
struct DevBackoff {
    retry: Option<Instant>,
    wait: Duration,
}

impl DevBackoff {
    fn new(wait: Duration) -> Self { DevBackoff { retry: None, wait } }

    /// `missed`, through origin/dev's `Missed::retry` rule copied literally (above).
    fn missed(&mut self, missed: Missed, paced: bool, now: Instant) {
        self.retry = match missed {
            Missed::Occluded => dev_occluded_retry(now, &mut self.wait),
            Missed::Timeout if paced => None,
            Missed::Timeout => Some(now + TIMEOUT_RETRY),
            Missed::Outdated | Missed::Unconfigured => Some(now + OCCLUDED_RETRY),
        };
    }
}

/// One thing that reached the overlay's wait, as `overlay/input.rs`, `overlay/draw.rs` and
/// `overlay/lifetime.rs` fed it.
#[derive(Clone, Copy, Debug)]
enum BackoffOp {
    /// `missed`: a shown frame got no drawable. `paced`: a display link runs, whose next tick
    /// draws the frame itself.
    Missed { missed: Missed, paced: bool },
    /// `wake_due`: a retry due at the manual clock is cleared and asks for a frame.
    Due,
    /// `draw_frame`: the frame draws (`presents`: and presents), clearing the wait first.
    Frame { presents: bool },
    /// A timer paces motion with no display link to pace it (macOS).
    At,
    /// `occluded(false)` or a show: the occluded wait starts over; a wait stands.
    Reveal,
    /// `stop_drawing`, on hide: nothing waits.
    Stop,
}

/// Both sides take `seq` on a manual clock, checked after every event. `prewarm`: a prewarm
/// frame, which leaves the wait alone. `scale`: how far each event is from the last.
fn holds_up(seq: &[BackoffOp], prewarm: bool, scale: u32) {
    let t0 = Instant::now();
    let mut dev = DevBackoff::new(OCCLUDED_RETRY);
    let mut ours = Backoff::default();
    for (k, &op) in seq.iter().enumerate() {
        let now = t0 + Duration::from_millis(11) * scale * k as u32;
        match op {
            BackoffOp::Missed { missed, paced } => {
                let effect = ours.step(BackoffEvent::Missed { missed, paced, now });
                dev.missed(missed, paced, now);
                assert_eq!(!effect.is_empty(), dev.retry.is_some(), "a wait stops the display link: {op:?} at {k}");
            }
            BackoffOp::Due => {
                if ours.deadline().is_some_and(|at| at <= now) { ours.step(BackoffEvent::Lapsed); }
                if dev.retry.is_some_and(|at| at <= now) { dev.retry = None; }
            }
            BackoffOp::Frame { presents } => {
                if !prewarm {
                    ours.step(BackoffEvent::Lapsed);
                    dev.retry = None;
                }
                if presents && !prewarm {
                    ours.step(BackoffEvent::Presented);
                    dev.wait = OCCLUDED_RETRY;
                }
            }
            BackoffOp::At => if !prewarm {
                ours.step(BackoffEvent::At(now + TIMER_FRAME));
                dev.retry = Some(now + TIMER_FRAME);
            },
            BackoffOp::Reveal => {
                ours.step(BackoffEvent::Revealed);
                dev.wait = OCCLUDED_RETRY;
            }
            BackoffOp::Stop => {
                ours.step(BackoffEvent::Lapsed);
                dev.retry = None;
            }
        }
        assert_eq!(ours.deadline(), dev.retry, "{op:?} at {k}, prewarm {prewarm}, scale {scale}");
        assert_eq!(ours.waiting(now), dev.retry.is_some_and(|at| at > now), "waiting after {op:?} at {k}");
        assert_eq!(occluded_wait(ours, now), dev.wait, "the wait after {op:?} at {k}, prewarm {prewarm}");
    }
}

#[test]
fn overlay_backoff_matches_dev_for_every_sequence() {
    let alphabet = [
        BackoffOp::Missed { missed: Missed::Occluded, paced: false },
        BackoffOp::Missed { missed: Missed::Timeout, paced: false },
        BackoffOp::Missed { missed: Missed::Timeout, paced: true },
        BackoffOp::Missed { missed: Missed::Outdated, paced: false },
        BackoffOp::Missed { missed: Missed::Unconfigured, paced: false },
        BackoffOp::Due,
        BackoffOp::Frame { presents: false },
        BackoffOp::Frame { presents: true },
        BackoffOp::At,
        BackoffOp::Reveal,
        BackoffOp::Stop,
    ];
    // Sparse events leave waits pending; a slow clock runs them out and climbs to the cap.
    for (prewarm, scale) in [(false, 1), (true, 1), (false, 64)] {
        walk(&alphabet, 5, |seq| holds_up(seq, prewarm, scale));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform { Mac, Windows, X11, Wayland }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind { Window, Layer }

/// A window or surface, numbered by the step that made it.
type Made = (u32, Kind);

/// Which owner gave the window back on `release`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner { Live, Parked }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// The window is made, its GPU surface attaches, and its new scale configures.
    Ok,
    /// No native window.
    Window,
    /// The window is made; attaching its GPU surface fails.
    Attach,
    /// The surface attaches; configuring it at the overlay's scale fails.
    Rescale,
}

#[derive(Clone, Copy, Debug)]
enum SurfaceOp {
    /// `prepare(showing)`.
    Prepare { showing: bool, outcome: Outcome },
    /// `hide_window`'s surface: a layer or XWayland surface goes with the hide.
    HideWindow,
    /// `park`, for `release_gpu_when_hidden`.
    Park,
    /// `release`: `surface_refused` or the overlay dropping.
    Release,
}

#[derive(Clone, Copy)]
struct Config { platform: Platform, defers: bool, callback: bool }

/// origin/dev's `live`, `parked` and `announced`, written as `prepare`, `window`, `unpark`,
/// `ready`, `hide_window`, `park` and `release` wrote them.
#[derive(Default)]
struct DevSurface {
    live: Option<Made>,
    parked: Option<Made>,
    announced: bool,
    heard: Vec<Option<Made>>,
    released: Vec<(Owner, Made)>,
}

impl DevSurface {
    fn apply(&mut self, c: Config, op: SurfaceOp, n: u32) {
        let kind = if c.platform == Platform::Wayland { Kind::Layer } else { Kind::Window };
        match op {
            SurfaceOp::Prepare { showing, outcome } => {
                if self.live.is_some() { return; }
                let (creation, attach, rescale) = match outcome {
                    Outcome::Ok => (true, true, true),
                    Outcome::Window => (false, true, true),
                    Outcome::Attach => (true, false, true),
                    Outcome::Rescale => (true, true, false),
                };
                let surface = showing || !c.defers;
                let parks = c.platform == Platform::X11;
                if !surface && !parks { return; }
                let made = if c.platform == Platform::Wayland {
                    creation.then_some((n, kind))
                } else if self.parked.is_some() || self.window(c, creation, attach, n) {
                    match self.live.take() {
                        Some(live) => Some(live),
                        None if surface => self.unpark(attach),
                        None => return self.ready(c),
                    }
                } else {
                    None
                };
                if let Some(live) = made {
                    self.live = Some(live);
                    // The surface's display scale is known now; one that can't take it leaves
                    // the overlay live but not ready, so nothing is announced.
                    if live.1 == Kind::Window && !rescale { return; }
                    self.ready(c);
                }
            }
            SurfaceOp::HideWindow => {
                let Some(live) = &self.live else { return };
                if live.1 == Kind::Layer { self.live.take(); }
            }
            SurfaceOp::Park => {
                if !matches!(self.live, Some((_, Kind::Window))) { return; }
                if matches!(c.platform, Platform::Mac | Platform::Windows) {
                    self.release();
                } else {
                    let Some(live) = self.live.take() else { return };
                    self.parked = Some(live);
                }
            }
            SurfaceOp::Release => self.release(),
        }
    }

    /// Failing to make the window leaves nothing; failing to attach its surface parks nothing.
    fn window(&mut self, c: Config, creation: bool, attach: bool, n: u32) -> bool {
        if !creation { return false; }
        if c.platform == Platform::Mac {
            if !attach { return false; }
            self.live = Some((n, Kind::Window));
        } else {
            self.parked = Some((n, Kind::Window));
        }
        true
    }

    fn unpark(&mut self, attach: bool) -> Option<Made> {
        let window = self.parked.take()?;
        if attach { return Some(window); }
        self.parked = Some(window);
        None
    }

    fn ready(&mut self, c: Config) {
        if !self.announced && c.callback {
            self.announced = true;
            self.heard.push(self.live.or(self.parked));
        }
    }

    fn release(&mut self) {
        let released = self.live.take().map(|live| (Owner::Live, live)).or_else(|| self.parked.take().map(|backing| (Owner::Parked, backing)));
        self.released.extend(released);
        self.announced = false;
    }
}

/// `Overlay`'s same functions, through `Surface` as they now write it.
struct Ours {
    surface: Surface<Made, Made>,
    heard: Vec<Option<Made>>,
    released: Vec<(Owner, Made)>,
}

impl Ours {
    fn apply(&mut self, c: Config, op: SurfaceOp, n: u32) {
        let kind = if c.platform == Platform::Wayland { Kind::Layer } else { Kind::Window };
        match op {
            SurfaceOp::Prepare { showing, outcome } => {
                if self.surface.live().is_some() { return; }
                let (creation, attach, rescale) = match outcome {
                    Outcome::Ok => (true, true, true),
                    Outcome::Window => (false, true, true),
                    Outcome::Attach => (true, false, true),
                    Outcome::Rescale => (true, true, false),
                };
                let surface = showing || !c.defers;
                let parks = c.platform == Platform::X11;
                if !surface && !parks { return; }
                let made = if c.platform == Platform::Wayland {
                    creation.then_some((n, kind))
                } else if self.surface.parked().is_some() || self.window(c, creation, attach, n) {
                    match self.surface.take_live() {
                        Some(live) => Some(live),
                        None if surface => self.unpark(attach),
                        None => return self.ready(c),
                    }
                } else {
                    None
                };
                if let Some(live) = made {
                    self.surface.made(live);
                    if live.1 == Kind::Window && !rescale { return; }
                    self.ready(c);
                }
            }
            SurfaceOp::HideWindow => {
                let Some(live) = self.surface.live() else { return };
                if live.1 == Kind::Layer { self.surface.take_live(); }
            }
            SurfaceOp::Park => {
                if !matches!(self.surface.live(), Some((_, Kind::Window))) { return; }
                if matches!(c.platform, Platform::Mac | Platform::Windows) {
                    self.release();
                } else {
                    let Some(live) = self.surface.take_live() else { return };
                    self.surface.park(live);
                }
            }
            SurfaceOp::Release => self.release(),
        }
    }

    fn window(&mut self, c: Config, creation: bool, attach: bool, n: u32) -> bool {
        if !creation { return false; }
        if c.platform == Platform::Mac {
            if !attach { return false; }
            self.surface.made((n, Kind::Window));
        } else {
            self.surface.park((n, Kind::Window));
        }
        true
    }

    fn unpark(&mut self, attach: bool) -> Option<Made> {
        let window = self.surface.take_parked()?;
        if attach { return Some(window); }
        self.surface.park(window);
        None
    }

    fn ready(&mut self, c: Config) {
        if !self.surface.announced() && c.callback {
            self.surface.announce();
            self.heard.push(self.surface.live().or(self.surface.parked()).copied());
        }
    }

    fn release(&mut self) {
        match self.surface.release() {
            Some(SurfaceEffect::Live(live)) => self.released.push((Owner::Live, live)),
            Some(SurfaceEffect::Parked(backing)) => self.released.push((Owner::Parked, backing)),
            None => {}
        }
    }
}

#[test]
fn surface_matches_dev_for_every_sequence() {
    let alphabet = [
        SurfaceOp::Prepare { showing: false, outcome: Outcome::Ok },
        SurfaceOp::Prepare { showing: true, outcome: Outcome::Ok },
        SurfaceOp::Prepare { showing: false, outcome: Outcome::Window },
        SurfaceOp::Prepare { showing: true, outcome: Outcome::Window },
        SurfaceOp::Prepare { showing: false, outcome: Outcome::Attach },
        SurfaceOp::Prepare { showing: true, outcome: Outcome::Attach },
        SurfaceOp::Prepare { showing: false, outcome: Outcome::Rescale },
        SurfaceOp::Prepare { showing: true, outcome: Outcome::Rescale },
        SurfaceOp::HideWindow,
        SurfaceOp::Park,
        SurfaceOp::Release,
    ];
    // Each failure leaves a different state, so the walk must reach all of them.
    let (mut parked_after_attach, mut live_after_rescale) = (0, 0);
    let (mut released_live, mut released_parked) = (0, 0);
    for platform in [Platform::Mac, Platform::Windows, Platform::X11, Platform::Wayland] {
        for (defers, callback) in [(false, false), (false, true), (true, false), (true, true)] {
            let c = Config { platform, defers, callback };
            walk(&alphabet, 5, |seq| {
                let mut dev = DevSurface::default();
                let mut ours = Ours { surface: Surface::default(), heard: Vec::new(), released: Vec::new() };
                for (i, &op) in seq.iter().enumerate() {
                    let n = i as u32 + 1;
                    let (before_live, before_released) = (dev.live, dev.released.len());
                    dev.apply(c, op, n);
                    ours.apply(c, op, n);
                    if let SurfaceOp::Prepare { outcome, .. } = op {
                        if outcome == Outcome::Attach && before_live.is_none() && dev.parked.is_some() { parked_after_attach += 1; }
                        if outcome == Outcome::Rescale && before_live.is_none() && dev.live.is_some() { live_after_rescale += 1; }
                    }
                    for &(owner, _) in &dev.released[before_released..] {
                        if owner == Owner::Live { released_live += 1; } else { released_parked += 1; }
                    }
                    let at = || format!("{platform:?} defers {defers} callback {callback}: {seq:?} at {i}");
                    assert_eq!(ours.surface.live().map(|live| *live), dev.live, "{}", at());
                    assert_eq!(ours.surface.parked().map(|backing| *backing), dev.parked, "{}", at());
                    assert_eq!(ours.surface.announced(), dev.announced, "{}", at());
                    assert_eq!(ours.heard, dev.heard, "{}", at());
                    assert_eq!(ours.released, dev.released, "{}", at());
                }
            });
        }
    }
    assert!(parked_after_attach > 0, "an attach that fails leaves its window parked");
    assert!(live_after_rescale > 0, "a rescale that fails leaves the overlay live");
    assert!(released_live > 0 && released_parked > 0, "release gives a live or a parked window back");
}
