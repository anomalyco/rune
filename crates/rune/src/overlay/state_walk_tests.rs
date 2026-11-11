//! Every short sequence of what reaches an overlay's frame and surface state, driven through
//! `Quiet`, `turn` and `Surface`, checked after every event against a literal transcription of
//! the fields and branches they replaced.

use std::time::Duration;

use super::frame::{Gate, Quiet, QuietEffect, QuietEvent, Turn, turn};
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

/// origin/dev's `draw_frame` from the occlusion read to the drawable, as it was (macOS, after
/// `sync_occlusion`; elsewhere `occluded` is never set): whether it returned, whether it
/// counted an occluded miss, and the retry it left.
fn dev_gate(prewarm: bool, occluded: bool, dirty: bool, idle: bool, mut retry: Option<Instant>, now: Instant) -> (bool, bool, Option<Instant>) {
    if !prewarm && occluded {
        if retry.is_none_or(|retry| retry <= now) {
            return (true, true, retry);
        }
        return (true, false, retry);
    }
    if !dirty && idle { return (true, false, retry); }
    if !prewarm && retry.is_some_and(|retry| retry > now) { return (true, false, retry); }
    if !prewarm { retry = None; }
    (false, false, retry)
}

#[test]
fn turn_matches_dev_for_every_gate() {
    let now = Instant::now();
    let ms = Duration::from_millis(1);
    let retries = [None, Some(now - ms), Some(now), Some(now + ms)];
    let mut cases = 0;
    for bits in 0..16u8 {
        let [prewarm, occluded, dirty, idle] = [0, 1, 2, 3].map(|b| bits & (1 << b) != 0);
        for retry in retries {
            cases += 1;
            let reads = std::cell::Cell::new(0);
            let decided = turn(Gate { prewarm, occluded, dirty, idle, retry }, || { reads.set(reads.get() + 1); now });
            // `draw_frame`'s arms for each answer.
            let ours = match decided {
                Turn::Occluded { recheck } => (true, recheck, retry),
                Turn::Rest | Turn::BackedOff => (true, false, retry),
                Turn::Draw => (false, false, if prewarm { retry } else { None }),
            };
            assert_eq!(ours, dev_gate(prewarm, occluded, dirty, idle, retry, now), "{decided:?} {prewarm} {occluded} {dirty} {idle} {retry:?}");
            assert!(reads.get() <= 1 && (retry.is_some() || reads.get() == 0), "the clock is read only to compare a retry");
        }
    }
    assert_eq!(cases, 64);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform { Mac, Windows, X11, Wayland }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind { Window, Layer }

/// A window or surface, numbered by the step that made it.
type Made = (u32, Kind);

#[derive(Clone, Copy, Debug)]
enum SurfaceOp {
    /// `prepare(showing)`; `ok`: the window and its GPU surface could be made.
    Prepare { showing: bool, ok: bool },
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
}

impl DevSurface {
    fn apply(&mut self, c: Config, op: SurfaceOp, n: u32) {
        let kind = if c.platform == Platform::Wayland { Kind::Layer } else { Kind::Window };
        match op {
            SurfaceOp::Prepare { showing, ok } => {
                if self.live.is_some() { return; }
                let surface = showing || !c.defers;
                let parks = c.platform == Platform::X11;
                if !surface && !parks { return; }
                let made = if c.platform == Platform::Wayland {
                    ok.then_some((n, kind))
                } else if self.parked.is_some() || self.window(c, ok, n) {
                    match self.live.take() {
                        Some(live) => Some(live),
                        None if surface => self.unpark(ok),
                        None => return self.ready(c),
                    }
                } else {
                    None
                };
                if let Some(live) = made {
                    self.live = Some(live);
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

    fn window(&mut self, c: Config, ok: bool, n: u32) -> bool {
        if !ok { return false; }
        if c.platform == Platform::Mac { self.live = Some((n, Kind::Window)); } else { self.parked = Some((n, Kind::Window)); }
        true
    }

    fn unpark(&mut self, ok: bool) -> Option<Made> {
        let window = self.parked.take()?;
        if ok { return Some(window); }
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
        let _ = self.live.take().or_else(|| self.parked.take());
        self.announced = false;
    }
}

/// `Overlay`'s same functions, through `Surface` as they now write it.
struct Ours {
    surface: Surface<Made, Made>,
    heard: Vec<Option<Made>>,
}

impl Ours {
    fn apply(&mut self, c: Config, op: SurfaceOp, n: u32) {
        let kind = if c.platform == Platform::Wayland { Kind::Layer } else { Kind::Window };
        match op {
            SurfaceOp::Prepare { showing, ok } => {
                if self.surface.live().is_some() { return; }
                let surface = showing || !c.defers;
                let parks = c.platform == Platform::X11;
                if !surface && !parks { return; }
                let made = if c.platform == Platform::Wayland {
                    ok.then_some((n, kind))
                } else if self.surface.parked().is_some() || self.window(c, ok, n) {
                    match self.surface.take_live() {
                        Some(live) => Some(live),
                        None if surface => self.unpark(ok),
                        None => return self.ready(c),
                    }
                } else {
                    None
                };
                if let Some(live) = made {
                    self.surface.made(live);
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
                    self.surface.release();
                } else {
                    let Some(live) = self.surface.take_live() else { return };
                    self.surface.park(live);
                }
            }
            SurfaceOp::Release => { let _: Option<SurfaceEffect<Made, Made>> = self.surface.release(); }
        }
    }

    fn window(&mut self, c: Config, ok: bool, n: u32) -> bool {
        if !ok { return false; }
        if c.platform == Platform::Mac { self.surface.made((n, Kind::Window)); } else { self.surface.park((n, Kind::Window)); }
        true
    }

    fn unpark(&mut self, ok: bool) -> Option<Made> {
        let window = self.surface.take_parked()?;
        if ok { return Some(window); }
        self.surface.park(window);
        None
    }

    fn ready(&mut self, c: Config) {
        if !self.surface.announced() && c.callback {
            self.surface.announce();
            self.heard.push(self.surface.live().or(self.surface.parked()).copied());
        }
    }
}

#[test]
fn surface_matches_dev_for_every_sequence() {
    let alphabet = [
        SurfaceOp::Prepare { showing: false, ok: true },
        SurfaceOp::Prepare { showing: true, ok: true },
        SurfaceOp::Prepare { showing: false, ok: false },
        SurfaceOp::Prepare { showing: true, ok: false },
        SurfaceOp::HideWindow,
        SurfaceOp::Park,
        SurfaceOp::Release,
    ];
    for platform in [Platform::Mac, Platform::Windows, Platform::X11, Platform::Wayland] {
        for (defers, callback) in [(false, false), (false, true), (true, false), (true, true)] {
            let c = Config { platform, defers, callback };
            walk(&alphabet, 5, |seq| {
                let mut dev = DevSurface::default();
                let mut ours = Ours { surface: Surface::default(), heard: Vec::new() };
                for (i, &op) in seq.iter().enumerate() {
                    let n = i as u32 + 1;
                    dev.apply(c, op, n);
                    ours.apply(c, op, n);
                    let at = || format!("{platform:?} defers {defers} callback {callback}: {seq:?} at {i}");
                    assert_eq!(ours.surface.live().copied(), dev.live, "{}", at());
                    assert_eq!(ours.surface.parked().copied(), dev.parked, "{}", at());
                    assert_eq!(ours.surface.announced(), dev.announced, "{}", at());
                    assert_eq!(ours.heard, dev.heard, "{}", at());
                }
            });
        }
    }
}
