//! A walk over a growth row's events, checked after every one against `origin/dev`'s own fields
//! and branches, transcribed literally below. The reference never calls the code under test, not
//! even a helper: its constants are the numbers written out, and every branch is dev's.
//!
//! The walk drives the real entry points one event at a time (`set_kind`, `retarget`,
//! `hide_above`, `ride_up`, one substep) and compares the whole state after each: every sequence
//! of these events up to [`DEPTH`] long, from two resting heights.

use super::*;
use rune_motion::Spring;

/// `origin/dev`'s limits, written out.
mod devcaps {
    pub const START: f32 = 1.875 * 60.0;
    pub const RAMP_PER_S: f32 = 28.2; // ln(1.6) * 60
    pub const MAX: f32 = 11.5 * 60.0;
    pub const MAX_LONG: f32 = 16.0 * 60.0;
    pub const LONG_DISTANCE: f32 = 240.0;
    pub const CARRYING_TAIL: f32 = 7.5 * 60.0;
    pub const NEAR: f32 = 36.0;
    pub const MASK_GROW: f32 = 5.0 * 60.0;
    pub const MAX_DECEL: f32 = 1800.0;
    pub const APPROACH: f32 = 14.0;
    pub const APPROACH_MIN: f32 = 3.0;
}

/// `origin/dev`'s stream pace, written out.
mod devpace {
    pub const WINDOW: f32 = 0.5;
    pub const TRAIL: f32 = 4.0;
    pub const MIN: f32 = 2.5;
    pub const LAND: f32 = 18.0;
    pub const LAND_RAMP: f32 = 0.15;
    pub const LAND_MIN: f32 = 8.0;
    pub const MAX: f32 = 40.0;
    pub const ACCEL: f32 = 6000.0;
}

const DEV_SUBSTEP: f32 = 1.0 / 240.0;
const DEV_FOLD_FEATHER_GROW: f32 = 600.0;
const DEV_WHOLE_REACH: f32 = 300.0;
const DEV_SETTLE: f32 = 1.0 / 16.0;
const DEV_SETTLE_V: f32 = 1.5;

/// `origin/dev`'s `GrowTrack` before the conversion: the fields as they were, and every branch
/// as it was, moved onto `Dev` without a change.
#[derive(Clone, Copy, Debug, Default)]
struct Dev {
    h: f32,
    v: f32,
    target: f32,
    age: f32,
    mask: f32,
    streaming: bool,
    whole: bool,
    duration: f32,
    span: f32,
    glide: Option<Spring>,
    feather: Option<f32>,
    limit: f32,
    stage: Option<f32>,
    slide: Option<(f32, f32)>,
    restore: f32,
    give: Option<f32>,
    paced: bool,
    rate: f32,
    ended: f32,
    shut: bool,
}

impl Dev {
    fn settled(h: f32) -> Dev {
        Dev { h, v: 0.0, target: h, age: 0.0, mask: 0.0, streaming: false, whole: false, duration: 0.0, span: 0.0, glide: None, feather: None, limit: f32::INFINITY, stage: None, slide: None, restore: 0.0, give: None, paced: false, rate: 0.0, ended: 0.0, shut: false }
    }

    fn moving(&self) -> bool {
        !self.at_rest(self.target - self.h) || self.mask > 0.01
    }

    fn at_rest(&self, remaining: f32) -> bool {
        let (distance, speed) = if self.duration > 0.0 || self.target == 0.0 {
            (1.0 / 128.0, 0.5)
        } else {
            (DEV_SETTLE, DEV_SETTLE_V)
        };
        remaining.abs() <= distance && self.v.abs() <= speed
    }

    fn retarget(&mut self, target: f32) {
        if (target - self.target).abs() > 1e-4 {
            let lift = self.lift();
            if !self.moving() {
                self.age = 0.0;
            }
            if self.streaming {
                self.rate += (target - self.target).max(0.0) / devpace::WINDOW;
            }
            self.target = target;
            self.span = (target - self.h).abs();
            self.stage = None;
            self.give = None;
            self.shut = self.glide.is_some() && target < self.h;
            if self.glide.is_some() {
                if target < self.h && self.h > self.limit {
                    self.h = self.limit.max(target);
                } else if target > self.h && target > self.limit {
                    self.stage = Some(self.limit.max(self.h));
                }
                self.span = (target - self.h).abs();
            }
            let d = target - self.h;
            self.slide = (lift > 1.0 / 64.0 && d.abs() > 1.0 / 64.0).then(|| (lift * target / d, -lift / d));
        }
    }

    fn lift(&self) -> f32 {
        self.slide.map_or(0.0, |(a, b)| (a + b * self.h).max(0.0))
    }

    fn hide_above(&mut self, above: f32) -> f32 {
        let pinned = self.lift() + self.h;
        let cut = above.min(self.h - self.target).max(0.0);
        self.h -= cut;
        self.span = (self.target - self.h).abs();
        self.slide = Some((pinned, -1.0));
        cut
    }

    fn ride_up(&mut self, rise: f32) {
        let (from, h) = (self.lift(), self.h);
        let stage = (h + rise.max(0.0)).min(self.target);
        let (d, end) = (stage - h, self.target - stage);
        let b = if d > 1.0 / 64.0 { (end - from) / d } else { 0.0 };
        self.slide = Some((if d > 1.0 / 64.0 { from - b * h } else { end }, b));
        self.stage = Some(stage);
        self.span = d;
    }

    fn shutting_fold(&self) -> bool {
        self.glide.is_some() && self.h > self.target + 0.01
    }

    fn set_kind(&mut self, streaming: bool, whole: bool, glide: Option<(FoldGlide, f32)>) {
        self.streaming = streaming;
        self.paced |= streaming;
        self.whole = whole;
        let glide = glide.filter(|_| !streaming);
        if glide.is_none() {
            self.stage = None;
            self.shut = false;
        }
        self.glide = glide.map(|g| g.0.spring);
        self.feather = glide.and_then(|g| g.0.feather);
        self.limit = glide.map_or(f32::INFINITY, |g| g.1);
    }

    fn step(&mut self, spring: Spring, edge: &StreamEdge, carrying_tail: bool) {
        let dt = DEV_SUBSTEP;
        let spring = spring.with_duration(spring.duration.max(self.duration) * edge.duration.max(0.05));
        let aim = self.stage.unwrap_or(self.target);
        let rem = aim - self.h;
        if self.stage.is_some() && rem.abs() < 1.0 {
            self.restore += self.lift();
            self.slide = None;
            self.h = self.target;
            self.v = 0.0;
            self.stage = None;
        } else if self.at_rest(rem) {
            self.h = self.target;
            self.v = 0.0;
        } else if self.paced && rem > 0.0 {
            self.climb(rem, carrying_tail);
        } else if let Some(own) = self.glide.filter(|_| !self.streaming) {
            let (x, v) = own.sample(self.h, self.v, aim, dt);
            let landed = (rem >= 0.0 && x >= aim) || (rem <= 0.0 && x <= aim);
            self.h = if landed { aim } else { x };
            self.v = if landed { 0.0 } else { v };
            self.age += dt;
        } else if self.whole {
            let spring = spring.with_duration(spring.duration * (self.span / DEV_WHOLE_REACH).sqrt().max(1.0));
            let (x, v) = spring.sample(self.h, self.v, self.target, dt);
            let landed = (rem >= 0.0 && x >= self.target) || (rem <= 0.0 && x <= self.target);
            self.h = if landed { self.target } else { x };
            self.v = if landed { 0.0 } else { v };
            self.age += dt;
        } else {
            let (x, _v) = spring.sample(self.h, self.v, self.target, dt);
            let mut cap = if rem.abs() > devcaps::LONG_DISTANCE { devcaps::MAX_LONG } else { devcaps::MAX };
            if carrying_tail {
                cap = cap.min(devcaps::CARRYING_TAIL);
            }
            cap = cap.min(devcaps::START * (devcaps::RAMP_PER_S * self.age).exp());
            if self.streaming && rem > 0.0 && rem < devcaps::NEAR {
                cap = cap.min((0.085 * rem + 0.65 * (rem / 4.0).min(1.0)) * 60.0 * edge.catch_up);
            }
            let braking = (2.0 * devcaps::MAX_DECEL * rem.abs()).sqrt();
            cap = cap.max((self.v.abs() - devcaps::MAX_DECEL * dt).min(braking));
            cap = cap.min(devcaps::APPROACH * rem.abs() + devcaps::APPROACH_MIN);
            let mut d = x - self.h;
            let lim = cap * dt;
            d = d.clamp(-lim, lim);
            let landed = (rem >= 0.0 && d >= rem) || (rem <= 0.0 && d <= rem);
            if landed {
                d = rem;
            }
            self.h += d;
            self.v = if landed { 0.0 } else { d / dt };
            self.age += dt;
        }
        let left = (self.target - self.h).max(0.0);
        let aim = if left > 0.25 {
            edge.mask_max.min(edge.floor.max(1.5 * left))
        } else {
            0.0
        };
        if self.glide.is_some() && !self.streaming {
            let left = (self.target - self.h).abs();
            // differs from dev: this is the sentinel the branch no longer carries, kept here so
            // the reference stays dev's own reading of an easing that lands on exactly -1.0.
            let riding = self.slide.is_some_and(|(_, b)| b == -1.0);
            let aim = if left > 0.25 && !riding { self.feather.unwrap_or(edge.mask_max).min(1.5 * left) } else { 0.0 };
            self.mask = if aim > self.mask { (self.mask + DEV_FOLD_FEATHER_GROW * dt).min(aim) } else { aim };
        } else if aim > self.mask {
            self.mask = (self.mask + devcaps::MASK_GROW * dt).min(aim);
        } else {
            self.mask = (self.mask - edge.shrink * dt).max(aim);
        }
        self.rate *= (-dt / devpace::WINDOW).exp();
        if !self.moving() {
            if !self.shut {
                self.slide = None;
            }
            if !self.streaming {
                self.paced = false;
                self.rate = 0.0;
                self.ended = 0.0;
            }
        }
    }

    fn climb(&mut self, rem: f32, carrying_tail: bool) {
        let dt = DEV_SUBSTEP;
        self.ended = if self.streaming { 0.0 } else { self.ended + dt };
        let landing = (self.ended / devpace::LAND_RAMP).min(1.0);
        let w = (self.rate / devpace::TRAIL).clamp(devpace::MIN + (devpace::LAND - devpace::MIN) * landing, devpace::MAX);
        let a = (w * w * rem - 2.0 * w * self.v).clamp(-devpace::ACCEL, devpace::ACCEL);
        let mut cap = if rem > devcaps::LONG_DISTANCE { devcaps::MAX_LONG } else { devcaps::MAX };
        if carrying_tail {
            cap = cap.min(devcaps::CARRYING_TAIL);
        }
        let v = (self.v + a * dt).max(devpace::LAND_MIN * landing).min(cap.min((2.0 * devpace::ACCEL * rem).sqrt()));
        if v * dt >= rem {
            self.h = self.target;
            self.v = 0.0;
        } else {
            self.h += v * dt;
            self.v = v;
        }
        self.age += dt;
    }
}

/// One event of a row's life.
#[derive(Clone, Copy, Debug)]
enum Ev {
    Kind { streaming: bool, whole: bool, fold: Option<(FoldGlide, f32)> },
    Retarget(f32),
    HideAbove(f32),
    RideUp(f32),
    Step { carrying_tail: bool },
}

const FOLD: Spring = Spring::new(0.18, 0.0);

/// Every event a row can be handed, from the app's kinds to the frame's substeps.
fn alphabet() -> Vec<Ev> {
    let mut events = vec![
        Ev::Kind { streaming: false, whole: false, fold: None },
        Ev::Kind { streaming: true, whole: false, fold: None },
        Ev::Kind { streaming: false, whole: true, fold: None },
        Ev::Kind { streaming: false, whole: false, fold: Some((FoldGlide { spring: FOLD, feather: None }, 900.0)) },
        Ev::Kind { streaming: false, whole: false, fold: Some((FoldGlide { spring: FOLD, feather: Some(20.0) }, 400.0)) },
        Ev::Retarget(120.0),
        Ev::Retarget(640.0),
        Ev::HideAbove(12.0),
        Ev::RideUp(90.0),
        Ev::Step { carrying_tail: false },
        Ev::Step { carrying_tail: true },
    ];
    events.shrink_to_fit();
    events
}

/// Hand one event to both, then check every field.
fn drive(t: &mut GrowTrack, d: &mut Dev, e: Ev, path: &[Ev]) {
    match e {
        Ev::Kind { streaming, whole, fold } => {
            t.set_kind(streaming, whole, fold);
            d.set_kind(streaming, whole, fold);
        }
        Ev::Retarget(to) => {
            t.retarget(to);
            d.retarget(to);
        }
        Ev::HideAbove(above) => {
            assert_eq!(t.hide_above(above).to_bits(), d.hide_above(above).to_bits(), "{path:?} then {e:?}: points cut");
        }
        Ev::RideUp(rise) => {
            t.ride_up(rise);
            d.ride_up(rise);
        }
        Ev::Step { carrying_tail } => {
            t.step(GROWTH, &edge::FEATHER, carrying_tail);
            d.step(GROWTH, &edge::FEATHER, carrying_tail);
        }
    }
    compare(t, d, path, e);
}

fn compare(t: &GrowTrack, d: &Dev, path: &[Ev], e: Ev) {
    let what = || format!("{path:?} then {e:?}");
    let same = |name: &str, a: f32, b: f32| assert!(a.to_bits() == b.to_bits(), "{name}: {} vs dev's {b} after {}", a, what());
    same("h", t.h, d.h);
    same("v", t.v, d.v);
    same("target", t.target, d.target);
    same("age", t.age, d.age);
    same("mask", t.mask, d.mask);
    same("span", t.span, d.span);
    same("restore", t.restore, d.restore);
    same("rate", t.rate, d.rate);
    same("lift", t.lift(), d.lift());
    assert_eq!(t.streaming, d.streaming, "streaming after {}", what());
    assert_eq!(t.give.map(f32::to_bits), d.give.map(f32::to_bits), "give after {}", what());
    assert_eq!(t.shut(), d.shut, "shut after {}", what());
    assert_eq!(t.shutting_fold(), d.shutting_fold(), "shutting_fold after {}", what());

    // The kind: a fold's own data, the whole glide, or the capped spring.
    assert_eq!(t.glide(), d.glide, "fold spring after {}", what());
    assert_eq!(t.feather(), d.feather, "fold feather after {}", what());
    if let Motion::Fold { limit, .. } = t.motion {
        same("fold limit", limit, d.limit);
    }
    assert_eq!(t.whole, d.whole, "whole after {}", what());

    // Where the glide stops.
    let stage = match t.stage {
        Stage::Free => None,
        Stage::StopAt(at) => Some(at),
    };
    assert_eq!(t.staged(), d.stage.is_some(), "staged after {}", what());
    assert_eq!(stage.map(f32::to_bits), d.stage.map(f32::to_bits), "stage after {}", what());

    // How the contents draw.
    let riding = matches!(t.lift, Lift::Riding { .. });
    assert_eq!(riding, d.slide.is_some_and(|(_, b)| b == -1.0), "riding after {}", what());
    assert_eq!(!matches!(t.lift, Lift::None), d.slide.is_some(), "lifted at all after {}", what());

    // The climb on the stream's pace.
    let (paced, ended) = match t.pace {
        Pace::Rest => (false, 0.0),
        Pace::Climbing { ended } => (true, ended),
    };
    assert_eq!(paced, d.paced, "paced after {}", what());
    if paced {
        same("ended", ended, d.ended);
    }
}

/// How many events deep the walk goes: every sequence of [`alphabet`] up to this length.
const DEPTH: usize = 6;

fn explore(t: GrowTrack, d: Dev, events: &[Ev], left: usize, path: &mut Vec<Ev>) {
    if left == 0 {
        return;
    }
    for &e in events {
        let (mut t, mut d) = (t, d);
        path.push(e);
        drive(&mut t, &mut d, e, &path[..path.len() - 1]);
        explore(t, d, events, left - 1, path);
        path.pop();
    }
}

#[test]
fn every_sequence_of_events_matches_dev() {
    let events = alphabet();
    for from in [0.0, 640.0] {
        explore(GrowTrack::settled(from), Dev::settled(from), &events, DEPTH, &mut Vec::new());
    }
}
