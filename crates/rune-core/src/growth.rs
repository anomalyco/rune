//! Timeline growth: rows of a growth stack ease their heights on one spring with speed limits
//! per second, and positions are derived from the presented heights above (so rows never
//! overlap or open gaps). Streaming rows reveal through a feathered bottom edge.
//!
//! Desktop's constants (research/18, `row-growth.ts @ 39eb86d`) are per frame at 60 fps; here
//! every cap is per second and the integrator runs fixed 1/240 s substeps, so 30, 60 and 120 Hz
//! sample the same curve (guarantee 1). Retargets keep height and velocity (guarantee 2).

use rune_motion::Spring;

/// One streaming-edge variant (research/18 §1). Desktop's per-frame values × 60.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamEdge {
    pub name: &'static str,
    /// Largest feather height (points).
    pub mask_max: f32,
    /// Smallest feather height while growing (points).
    pub floor: f32,
    /// How fast the feather shrinks back (points per second).
    pub shrink: f32,
    /// Multiplier on the near-target slowdown cap.
    pub catch_up: f32,
    /// Multiplier on the growth spring's duration.
    pub duration: f32,
    /// Clip the row at its presented height (else content overflows while it grows).
    pub clip: bool,
    /// A streaming row, and its catch-up once the stream ends, draws past its presented height,
    /// unclipped and unmasked, into space the app keeps below it; other rows still clip.
    pub overflow: bool,
    /// Word fade-in duration in seconds (0 = words appear at once).
    pub word_fade: f32,
}

pub mod edge {
    use super::StreamEdge;
    pub const FEATHER: StreamEdge = StreamEdge {
        name: "feather",
        mask_max: 28.0,
        floor: 18.0,
        shrink: 1.8 * 60.0,
        catch_up: 1.0,
        duration: 1.0,
        clip: true,
        overflow: false,
        word_fade: 0.18,
    };
    pub const FAST_FEATHER: StreamEdge = StreamEdge {
        name: "fast-feather",
        mask_max: 12.0,
        floor: 6.0,
        shrink: 4.0 * 60.0,
        catch_up: 2.4,
        duration: 0.55,
        clip: true,
        overflow: false,
        word_fade: 0.18,
    };
    pub const NO_MASK: StreamEdge = StreamEdge {
        name: "no-mask",
        mask_max: 0.0,
        floor: 0.0,
        shrink: 28.0 * 60.0,
        catch_up: 1.0,
        duration: 1.0,
        clip: false,
        overflow: false,
        word_fade: 0.0,
    };
    pub const WORD_FADE: StreamEdge = StreamEdge {
        name: "word-fade",
        mask_max: 0.0,
        floor: 0.0,
        shrink: 28.0 * 60.0,
        catch_up: 1.0,
        duration: 1.0,
        clip: false,
        overflow: false,
        word_fade: 0.36,
    };
    pub const ALL: [StreamEdge; 4] = [FEATHER, FAST_FEATHER, NO_MASK, WORD_FADE];
}

/// `timeline.growth`: 0.32 s, bounce 0 (`row-growth.ts:6-14 @ 39eb86d`).
pub const GROWTH: Spring = Spring::new(0.32, 0.0);

/// The farthest a whole row (`El::arrives_whole`) glides in the spring's own duration. A
/// critically damped spring opens with an acceleration of its distance over its duration squared,
/// so a farther glide lengthens with the square root of its distance and opens no harder than one
/// this long: a pinned viewport following a tall insert doesn't lurch as it starts.
pub const WHOLE_REACH: f32 = 300.0;

/// Speed limits in points per second (Desktop's per-frame caps × 60).
pub mod caps {
    pub const START: f32 = 1.875 * 60.0;
    /// Ramp factor per second. Desktop ramps 2.2× per 60 fps frame; Rune ramps 1.6× so a
    /// growth starting from rest doesn't read as a velocity snap.
    pub const RAMP_PER_S: f32 = 28.2; // ln(1.6) * 60
    pub const MAX: f32 = 11.5 * 60.0;
    pub const MAX_LONG: f32 = 16.0 * 60.0;
    pub const LONG_DISTANCE: f32 = 240.0;
    pub const CARRYING_TAIL: f32 = 7.5 * 60.0;
    pub const NEAR: f32 = 36.0;
    /// Feather growth limit.
    pub const MASK_GROW: f32 = 5.0 * 60.0;
    /// Hardest braking when a cap drops, in points per second squared (Rune's addition: Desktop
    /// switches caps between frames, which motion-lint reads as a velocity snap).
    pub const MAX_DECEL: f32 = 1800.0;
    /// Speed ceiling near the target: APPROACH × points left (per second) plus APPROACH_MIN, so
    /// every row decays into its target (Rune's addition, for the same reason).
    pub const APPROACH: f32 = 14.0;
    pub const APPROACH_MIN: f32 = 3.0;
}

/// A streaming row's climb (`El::streaming`): one critically damped follower whose stiffness
/// tracks how fast its target has been rising, so a slow stream creeps on steadily between
/// words and lines, a fast one keeps up, and the speed never kicks up from rest per arrival.
/// It trails by `2 × rate / ω` (`2 × TRAIL` once the rate sets ω), held back under the feather.
pub mod pace {
    /// Seconds the arrival rate is averaged over.
    pub const WINDOW: f32 = 0.5;
    /// Half the points a steady stream's climb trails its text by.
    pub const TRAIL: f32 = 4.0;
    /// Least stiffness (1/s) while the stream runs: a slow one creeps rather than stopping.
    pub const MIN: f32 = 2.5;
    /// Least stiffness once the stream has ended, reached over [`LAND_RAMP`] seconds, so the
    /// last of it lands in under half a second.
    pub const LAND: f32 = 18.0;
    pub const LAND_RAMP: f32 = 0.15;
    /// Least speed (points per second) once the stream has ended: it stops rather than creeps.
    pub const LAND_MIN: f32 = 8.0;
    /// Most stiffness, for the fastest stream.
    pub const MAX: f32 = 40.0;
    /// Hardest change of speed (points per second squared).
    pub const ACCEL: f32 = 6000.0;
}

pub const SUBSTEP: f32 = 1.0 / 240.0;

/// The steps that advance growth by `dt` seconds: whole `SUBSTEP`s, then what is left, so a
/// frame shows the motion at its own time rather than at the last whole substep before it (at
/// 144 or 165 Hz a frame would otherwise advance one substep or two, unevenly). At most a second.
#[doc(hidden)]
pub fn substeps(dt: f32) -> Substeps {
    let dt = dt.clamp(0.0, 1.0);
    let whole = (dt / SUBSTEP + 1e-4).floor();
    let left = dt - whole * SUBSTEP;
    Substeps { whole: whole as u32, left: if left > 1e-6 { left } else { 0.0 } }
}

/// [`substeps`]: a frame's growth steps, copied to each track that takes them.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct Substeps {
    whole: u32,
    left: f32,
}

/// `n` whole substeps.
impl From<usize> for Substeps {
    fn from(n: usize) -> Substeps { Substeps { whole: n as u32, left: 0.0 } }
}

impl Iterator for Substeps {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.whole > 0 {
            self.whole -= 1;
            Some(SUBSTEP)
        } else if self.left > 0.0 {
            Some(std::mem::take(&mut self.left))
        } else {
            None
        }
    }
}

/// How fast a fold's feather deepens once its edge starts moving (points per second): 28 pt in
/// under 50 ms, so the edge softens as the spring picks up rather than in its first frame.
const FOLD_FEATHER_GROW: f32 = 600.0;

/// `El::fold_glide` and `El::fold_feather`: a fold's own spring and the depth of its moving
/// edge's feather (the stack's `StreamEdge::mask_max` when `None`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoldGlide {
    pub spring: Spring,
    pub feather: Option<f32>,
}

/// The glide a row has of its own, in place of the stack's spring.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Motion {
    /// The stack's spring (or, for a [`GrowTrack::whole`] row, the spring alone).
    #[default]
    Spring,
    /// `El::fold_glide`: this row's own spring for its own time, in place of the stack's; its
    /// moving edge feathered `feather` deep (the stack edge's when `None`), and its bottom
    /// leaving the window at `limit`.
    Fold {
        spring: Spring,
        feather: Option<f32>,
        limit: f32,
        /// Retargeted shut: landed, it still lifts and clips what it folded away until that has
        /// gone (`Runtime::place_growth_rows`).
        shut: bool,
    },
}

/// Whether a row climbs on its stream's [`pace`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Pace {
    /// Not on a stream: the capped spring, the whole glide or the fold.
    #[default]
    Rest,
    /// Climbing on the arrival rate: from a row's first streaming frame until it rests off the
    /// stream. `ended` counts the seconds since the stream stopped feeding it.
    Climbing { ended: f32 },
}

/// Where a glide's spring stops.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Stage {
    /// At the target.
    #[default]
    Free,
    /// `at` points in: the rest of a fold opening past its window's edge lands at once, out of
    /// sight, and contents still lifted come back above it.
    StopAt(f32),
}

impl Stage {
    fn aim(self, target: f32) -> f32 {
        match self {
            Stage::Free => target,
            Stage::StopAt(at) => at,
        }
    }

    fn is_set(self) -> bool {
        !matches!(self, Stage::Free)
    }
}

/// How a growth row's contents draw against its height: `a + b × h` points up from their layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Lift {
    /// At their layout.
    #[default]
    None,
    /// A fold shutting under a header held stuck to its viewport's top: contents ride its bottom
    /// edge from the height `pinned` they were cut at, so what shows slides up under the header
    /// on the fold's spring rather than racing past. Nothing to feather. Only
    /// [`GrowTrack::hide_above`] cuts a fold to this; a slope computed from a lift and a distance
    /// is [`Lift::Easing`] however it lands.
    Riding { pinned: f32 },
    /// Easing back to their layout over what is left of the glide, `a + b × h` points up.
    Easing { a: f32, b: f32 },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GrowTrack {
    pub h: f32,
    pub v: f32,
    pub target: f32,
    /// Seconds since this growth began (drives the cap ramp).
    pub age: f32,
    /// Presented feather height (points).
    pub mask: f32,
    /// A live tail grows under a linear ramp (negative width in the renderer).
    #[doc(hidden)]
    pub linear_edge: bool,
    /// `El::streaming`: this row climbs on its stream's pace, which overrides its whole or fold
    /// kind until it rests off the stream.
    pub streaming: bool,
    /// `El::arrives_whole`, or a caller's own: no speed limits, only the spring, however far, so
    /// a pinned viewport follows one glide (its [`WHOLE_REACH`] duration comes from `span`). A
    /// fold's own spring takes precedence while the row is a fold.
    pub whole: bool,
    /// Commit spring duration floor (`Tx::motion`), so a slower transaction (`REFLOW`) slows the row too.
    #[doc(hidden)]
    pub duration: f32,
    /// Points between the presented height and the target when it was last set (see [`WHOLE_REACH`]).
    pub(crate) span: f32,
    /// What moves the height.
    pub(crate) motion: Motion,
    /// Where the glide stops, when it stops short of the target.
    pub(crate) stage: Stage,
    /// How the contents draw against the height.
    pub(crate) lift: Lift,
    /// Points of lifted contents put back above the row as its glide landed whole, for its
    /// viewport's offset to take up (`Runtime::restore_above`).
    #[doc(hidden)]
    pub restore: f32,
    /// The share of each point it shuts by that a header held over it comes down, when the
    /// viewport's end will stop its bottom short (`Runtime::give_way`).
    #[doc(hidden)]
    pub give: Option<f32>,
    /// Whether it climbs on its stream's pace.
    pub(crate) pace: Pace,
    /// Points per second the target has been rising at, averaged over [`pace::WINDOW`].
    ///
    /// It sits beside [`Pace`] rather than inside it because it outlives a restarted climb: a
    /// streaming row snapped to a new height keeps the rate it had when the pace picks up again.
    pub(crate) rate: f32,
}

/// A row within this many points of its height, moving slower than `SETTLE_V` points a second,
/// lands there: its last creep moves it under a hundredth of a point a frame, which no pixel shows,
/// and would keep the window drawing frames that change nothing.
const SETTLE: f32 = 1.0 / 16.0;
const SETTLE_V: f32 = 1.5;

impl GrowTrack {
    pub fn settled(h: f32) -> GrowTrack {
        GrowTrack { h, v: 0.0, target: h, age: 0.0, mask: 0.0, linear_edge: false, streaming: false, whole: false, duration: 0.0, span: 0.0, motion: Motion::Spring, stage: Stage::Free, lift: Lift::None, restore: 0.0, give: None, pace: Pace::Rest, rate: 0.0 }
    }

    #[doc(hidden)]
    pub fn with_duration(mut self, duration: f32) -> GrowTrack {
        self.duration = duration;
        self
    }

    pub fn moving(&self) -> bool {
        !self.at_rest(self.target - self.h) || self.mask > 0.01
    }

    fn at_rest(&self, remaining: f32) -> bool {
        // Closing slack under a pinned stack and rows on a reflow override position the
        // viewport: their last step remains visible. A fold glide lands as other rows do:
        // its spring's tail under a sixteenth of a point creeps in steps the pixel snap
        // holds and lets go, which reads as the row stopping and setting off again.
        let (distance, speed) = if self.duration > 0.0 || self.target == 0.0 {
            (1.0 / 128.0, 0.5)
        } else { (SETTLE, SETTLE_V) };
        remaining.abs() <= distance && self.v.abs() <= speed
    }

    pub fn retarget(&mut self, target: f32) {
        if (target - self.target).abs() > 1e-4 {
            let lift = self.lift();
            if !self.moving() {
                self.age = 0.0;
            }
            if self.streaming {
                self.rate += (target - self.target).max(0.0) / pace::WINDOW;
            }
            self.target = target;
            self.span = (target - self.h).abs();
            self.stage = Stage::Free;
            self.give = None;
            if let Motion::Fold { limit, shut, .. } = &mut self.motion {
                *shut = target < self.h;
                // Only what shows of a long fold moves: shutting, the part below the window goes
                // first; opening, it lands once the spring has carried the row to the window's edge.
                if target < self.h && self.h > *limit {
                    self.h = (*limit).max(target);
                } else if target > self.h && target > *limit {
                    self.stage = Stage::StopAt((*limit).max(self.h));
                }
                self.span = (target - self.h).abs();
            }
            // Lifted contents ease back to their layout over what's left of the glide. A fold cut
            // by N points and reopened by exactly N draws on its bottom edge by arithmetic, with
            // a slope of exactly -1.0: that's still an easing, and its edge still feathers.
            let d = target - self.h;
            self.lift = if lift > 1.0 / 64.0 && d.abs() > 1.0 / 64.0 {
                Lift::Easing { a: lift * target / d, b: -lift / d }
            } else {
                Lift::None
            };
        }
    }

    /// How far up from their layout the row's contents draw.
    #[doc(hidden)]
    pub fn lift(&self) -> f32 {
        match self.lift {
            Lift::None => 0.0,
            Lift::Riding { pinned } => (pinned - self.h).max(0.0),
            Lift::Easing { a, b } => (a + b * self.h).max(0.0),
        }
    }

    /// A fold shutting under a header held stuck to its viewport's top, with `above` points of
    /// it hidden above that header: those go at once, out of sight, and its contents ride its
    /// bottom edge from here, so what shows slides up under the header on the fold's spring
    /// rather than racing past. Returns the points cut, which the viewport's offset drops by so
    /// nothing on screen moves.
    #[doc(hidden)]
    pub fn hide_above(&mut self, above: f32) -> f32 {
        let pinned = self.lift() + self.h;
        let cut = above.min(self.h - self.target).max(0.0);
        self.h -= cut;
        self.span = (self.target - self.h).abs();
        self.lift = Lift::Riding { pinned };
        cut
    }

    /// A fold opening again while its contents draw lifted, its viewport pinned to its end so
    /// it grows from its bottom: its spring stops `rise` points on, where its header has risen
    /// back to the top and what its contents hid below has shown, its contents easing up as
    /// much on the way; the rest then lands at once, all of it lifted, which the end takes up.
    #[doc(hidden)]
    pub fn ride_up(&mut self, rise: f32) {
        let (from, h) = (self.lift(), self.h);
        let stage = (h + rise.max(0.0)).min(self.target);
        let (d, end) = (stage - h, self.target - stage);
        let b = if d > 1.0 / 64.0 { (end - from) / d } else { 0.0 };
        self.lift = Lift::Easing { a: if d > 1.0 / 64.0 { from - b * h } else { end }, b };
        self.stage = Stage::StopAt(stage);
        self.span = d;
    }

    /// A fold shutting over what it held, which its clip folds away (`El::fold_glide`).
    #[doc(hidden)]
    pub fn shutting_fold(&self) -> bool {
        matches!(self.motion, Motion::Fold { .. }) && self.h > self.target + 0.01
    }

    /// Whether a glide last retargeted shut: landed, it still lifts and clips what it folded
    /// away until that has gone.
    #[doc(hidden)]
    pub fn shut(&self) -> bool {
        matches!(self.motion, Motion::Fold { shut: true, .. })
    }

    /// Whether it climbs on a stream's pace.
    #[doc(hidden)]
    pub fn paced(&self) -> bool {
        matches!(self.pace, Pace::Climbing { .. })
    }

    /// What a shut fold held has left: let go of its lift and of the shut flag that kept it.
    #[doc(hidden)]
    pub fn forget_folded(&mut self) {
        self.lift = Lift::None;
        if let Motion::Fold { shut, .. } = &mut self.motion {
            *shut = false;
        }
    }

    /// The fold's own spring, when this row is a fold.
    #[doc(hidden)]
    pub fn glide(&self) -> Option<Spring> {
        match self.motion {
            Motion::Fold { spring, .. } => Some(spring),
            Motion::Spring => None,
        }
    }

    /// The fold's own edge depth, when it sets one.
    #[doc(hidden)]
    pub fn feather(&self) -> Option<f32> {
        match self.motion {
            Motion::Fold { feather, .. } => feather,
            Motion::Spring => None,
        }
    }

    /// Whether the glide has stopped `Stage::StopAt` short of its target, still to land whole.
    #[doc(hidden)]
    pub fn staged(&self) -> bool {
        self.stage.is_set()
    }

    /// The same row kind, restarted at `h` with nothing else carried over.
    #[doc(hidden)]
    pub fn restarted(&self, h: f32) -> GrowTrack {
        GrowTrack {
            streaming: self.streaming,
            whole: self.whole,
            motion: match self.motion {
                Motion::Fold { spring, feather, limit, .. } => Motion::Fold { spring, feather, limit, shut: false },
                other => other,
            },
            ..GrowTrack::settled(h)
        }
    }

    /// The row's `El::streaming`, `El::arrives_whole` and `El::fold_glide` (which streaming
    /// overrides) with the height at which its bottom leaves the window.
    #[doc(hidden)]
    pub fn set_kind(&mut self, streaming: bool, whole: bool, glide: Option<(FoldGlide, f32)>) {
        self.streaming = streaming;
        self.whole = whole;
        if streaming {
            // `paced |= streaming`: a row already off its stream climbs again from where it was.
            if self.pace == Pace::Rest {
                self.pace = Pace::Climbing { ended: 0.0 };
            }
        }
        let glide = glide.filter(|_| !streaming);
        let shut = self.shut();
        self.motion = match glide {
            Some((g, limit)) => Motion::Fold { spring: g.spring, feather: g.feather, limit, shut },
            None => {
                // A glide's stop at the window's edge goes with it: the row it hands over to
                // carries on from where it is, rather than landing the rest at once.
                self.stage = Stage::Free;
                Motion::Spring
            }
        };
    }

    /// Advance one fixed substep.
    pub fn step(&mut self, spring: Spring, edge: &StreamEdge, carrying_tail: bool) {
        self.step_by(SUBSTEP, spring, edge, carrying_tail);
    }

    /// [`Self::step`] for `dt` seconds, at most a `SUBSTEP`.
    #[doc(hidden)]
    pub fn step_by(&mut self, dt: f32, spring: Spring, edge: &StreamEdge, carrying_tail: bool) {
        let spring = spring.with_duration(spring.duration.max(self.duration) * edge.duration.max(0.05));
        let aim = self.stage.aim(self.target);
        let rem = aim - self.h;
        if self.stage.is_set() && rem.abs() < 1.0 {
            // The glide carried the row to the window's edge: the rest lands at once, out of view,
            // and contents still lifted come back above it.
            self.restore += self.lift();
            self.lift = Lift::None;
            self.h = self.target;
            self.v = 0.0;
            self.stage = Stage::Free;
        } else if self.at_rest(rem) {
            self.h = self.target;
            self.v = 0.0;
        } else if matches!(self.pace, Pace::Climbing { .. }) && rem > 0.0 {
            self.climb(rem, carrying_tail, dt);
        } else if !self.streaming && let Motion::Fold { spring: own, .. } = self.motion {
            // A fold: its own spring for its own time, however far, from the velocity it has.
            let (x, v) = own.sample(self.h, self.v, aim, dt);
            let landed = (rem >= 0.0 && x >= aim) || (rem <= 0.0 && x <= aim);
            self.h = if landed { aim } else { x };
            self.v = if landed { 0.0 } else { v };
            self.age += dt;
        } else if self.whole {
            // Content that arrived complete has nothing to read as it opens: one critically
            // damped glide on the spring, however far, which a pinned viewport follows.
            let spring = spring.with_duration(spring.duration * (self.span / WHOLE_REACH).sqrt().max(1.0));
            let (x, v) = spring.sample(self.h, self.v, self.target, dt);
            let landed = (rem >= 0.0 && x >= self.target) || (rem <= 0.0 && x <= self.target);
            self.h = if landed { self.target } else { x };
            self.v = if landed { 0.0 } else { v };
            self.age += dt;
        } else {
            let (x, _v) = spring.sample(self.h, self.v, self.target, dt);
            let mut cap = if rem.abs() > caps::LONG_DISTANCE { caps::MAX_LONG } else { caps::MAX };
            if carrying_tail {
                cap = cap.min(caps::CARRYING_TAIL);
            }
            cap = cap.min(caps::START * (caps::RAMP_PER_S * self.age).exp());
            if self.streaming && rem > 0.0 && rem < caps::NEAR {
                // Desktop's near-target slowdown, with its 0.65 px floor tapering over the last
                // 4 points so the row settles instead of stopping dead.
                cap = cap.min((0.085 * rem + 0.65 * (rem / 4.0).min(1.0)) * 60.0 * edge.catch_up);
            }
            // A cap that drops (near the target, carrying the tail) brakes at most MAX_DECEL, and
            // never faster than lets the row still stop exactly at its target.
            let braking = (2.0 * caps::MAX_DECEL * rem.abs()).sqrt();
            cap = cap.max((self.v.abs() - caps::MAX_DECEL * dt).min(braking));
            // The last points ease out exponentially, so a lagging row never arrives at speed.
            cap = cap.min(caps::APPROACH * rem.abs() + caps::APPROACH_MIN);
            let mut d = x - self.h;
            let lim = cap * dt;
            d = d.clamp(-lim, lim);
            // A growth bent toward a nearer target lands there instead of overshooting.
            let landed = (rem >= 0.0 && d >= rem) || (rem <= 0.0 && d <= rem);
            if landed {
                d = rem;
            }
            self.h += d;
            self.v = if landed { 0.0 } else { d / dt };
            self.age += dt;
        }
        // Feather: aims for min(max, max(floor, 1.5 × left to grow)); grows ≤ MASK_GROW/s.
        let left = (self.target - self.h).max(0.0);
        let aim = if left > 0.25 {
            edge.mask_max.min(edge.floor.max(1.5 * left))
        } else {
            0.0
        };
        if !self.streaming && let Motion::Fold { feather, .. } = self.motion {
            // A fold's edge feathers both ways, opening and shutting: it softens as the spring
            // picks up, holds through a reversal, and goes crisp as the glide lands.
            // Contents riding the bottom edge (`hide_above`) aren't cut by it: nothing to soften.
            // Riding is the state that says so, not a slope that happens to land on -1.0.
            let left = (self.target - self.h).abs();
            let riding = matches!(self.lift, Lift::Riding { .. });
            let aim = if left > 0.25 && !riding { feather.unwrap_or(edge.mask_max).min(1.5 * left) } else { 0.0 };
            self.mask = if aim > self.mask { (self.mask + FOLD_FEATHER_GROW * dt).min(aim) } else { aim };
        } else if aim > self.mask {
            self.mask = (self.mask + caps::MASK_GROW * dt).min(aim);
        } else {
            self.mask = (self.mask - edge.shrink * dt).max(aim);
        }
        self.rate *= (-dt / pace::WINDOW).exp();
        if !self.moving() {
            // A glide that shut keeps its contents lifted until what it folded away has left.
            if !self.shut() { self.lift = Lift::None; }
            if !self.streaming {
                self.pace = Pace::Rest;
                self.rate = 0.0;
            }
        }
    }

    /// One substep of a streaming row's climb `rem` points short of its target (see [`pace`]).
    fn climb(&mut self, rem: f32, carrying_tail: bool, dt: f32) {
        let Pace::Climbing { ended } = &mut self.pace else { return };
        *ended = if self.streaming { 0.0 } else { *ended + dt };
        let landing = (*ended / pace::LAND_RAMP).min(1.0);
        let w = (self.rate / pace::TRAIL).clamp(pace::MIN + (pace::LAND - pace::MIN) * landing, pace::MAX);
        let a = (w * w * rem - 2.0 * w * self.v).clamp(-pace::ACCEL, pace::ACCEL);
        let mut cap = if rem > caps::LONG_DISTANCE { caps::MAX_LONG } else { caps::MAX };
        if carrying_tail {
            cap = cap.min(caps::CARRYING_TAIL);
        }
        // No faster than braking at ACCEL still stops on the target. Once the stream has ended,
        // no slower than LAND_MIN, so the last of it lands rather than crawling; while it runs,
        // no floor, which would outrun a reveal rising a fraction of a point a word.
        let v = (self.v + a * dt).max(pace::LAND_MIN * landing).min(cap.min((2.0 * pace::ACCEL * rem).sqrt()));
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod walk;
