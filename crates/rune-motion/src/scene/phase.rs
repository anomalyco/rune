//! Where a node is in its life: present, entering, or exiting, with the two looks its presence
//! blends between. Each variant carries its own effect data, so a node can't be, say, exiting
//! while an entrance's look is still installed beside it.
//!
//! [`step`] moves the phase and returns what the scene should do, in order, instead of doing it.
//! The scene carries the acts out (`presence::Scene::apply`), so a phase change is plain data in
//! and out, and the one place that reads the effect pair can't disagree with the one that writes
//! it.
//!
//! This crate doesn't depend on rune, so the effects list here is its own small one.

use super::*;

/// Effect lanes: opacity, scale, offset x/y, size-fraction offset x/y, blur, hidden reveal,
/// drawn shape (dev's `fx_a`/`fx_b`).
pub(super) type Fx = [f32; 9];
pub(super) const FX_ID: Fx = [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];

pub(super) fn fx_of(e: &Effect) -> Fx {
    let hidden = if e.reveal.is_some() { 1.0 } else { 0.0 };
    [e.opacity, e.scale, e.offset.x, e.offset.y, e.offset_fraction.x, e.offset_fraction.y, e.blur, hidden, e.draw]
}

/// Exact at both ends: `a` at p = 0, `b` at p = 1.
pub(super) fn lerp_fx(a: &Fx, b: &Fx, p: f32) -> Fx {
    core::array::from_fn(|k| a[k] * (1.0 - p) + b[k] * p)
}

/// Where a node is in its life, with the looks its presence blends between: `at0` at presence 0
/// (absent) and `at1` at presence 1 (present).
#[derive(Clone, Copy, Debug)]
pub(super) enum Presence {
    /// On screen. `at0` is the look it arrived from, still blended in by the presence tail a
    /// settled entrance leaves behind.
    Present { at0: Fx, at1: Fx },
    /// Entering: drawn at `at0` at presence 0, arriving at `at1` at presence 1.
    Entering { at0: Fx, at1: Fx },
    /// Removed and still drawn until its presence settles: leaving `at1` for `at0`.
    Exiting { at0: Fx, at1: Fx },
}

impl Presence {
    /// A node that has not entered yet: the placeholder a new node is built with, before its
    /// first [`step`].
    pub(super) const LANDED: Presence = Presence::Present { at0: FX_ID, at1: FX_ID };

    /// The looks at presence 0 and at presence 1.
    pub(super) fn pair(&self) -> (&Fx, &Fx) {
        let (Presence::Present { at0, at1 }
        | Presence::Entering { at0, at1 }
        | Presence::Exiting { at0, at1 }) = self;
        (at0, at1)
    }

    /// Both looks, by value.
    pub(super) fn looks(&self) -> (Fx, Fx) {
        (*self.pair().0, *self.pair().1)
    }

    pub(super) fn entering(&self) -> bool {
        matches!(self, Presence::Entering { .. })
    }

    pub(super) fn exiting(&self) -> bool {
        matches!(self, Presence::Exiting { .. })
    }

    /// Entered and on screen: no entrance or exit in flight.
    pub(super) fn resting(&self) -> bool {
        matches!(self, Presence::Present { .. })
    }

    /// The phase apps read, tagged with how many exits the node has had.
    pub(super) fn public(&self, generation: u32) -> Phase {
        match self {
            Presence::Present { .. } => Phase::Present,
            Presence::Entering { .. } => Phase::Entering,
            Presence::Exiting { .. } => Phase::Exiting { generation },
        }
    }
}

/// What asks a node's phase to change.
pub(super) enum Event {
    /// A keyed node appears: entering from `ins` when `animate`, else landing present.
    Mount { ins: Effect, animate: bool, entrance: Anim },
    /// The app brought it back at presence `p` (a re-inserted key, or `reenter`).
    Revive { p: f32, ins: Effect, entrance: Anim },
    /// The app made it present at once, with its insertion look.
    Land { ins: Effect },
    /// The app started it fully present from somewhere else (a launch, matched geometry).
    Appear,
    /// The app made it enter in place from the look it already has (Reduce Motion's fade).
    FadeIn { entrance: Anim },
    /// The app removed it at presence `p`, leaving with `rem`.
    Exit { p: f32, rem: Effect, exit: Anim },
    /// Its presence reached its target: an entrance has arrived, an exit is over.
    Settle,
    /// How faded it is: `p` is its presence this frame, so an exit nobody can see is over.
    Faded { p: f32 },
}

/// What a phase change asks the scene to do, in order.
#[derive(Clone, Copy, Debug)]
pub(super) enum Act {
    /// Land the presence track on this value.
    Land(f32),
    /// Land it only when the node keeps no track and already shows this value.
    LandIfNeeded(f32),
    /// Make sure a presence track holds the node's current look before it runs.
    Ensure,
    /// Run the presence track to its target.
    Run { target: f32, anim: Anim },
    /// The node presents again.
    Dirty,
    /// Its transform pivots here (a removal that scales).
    Anchor(Vec2),
    /// What it hides behind while it leaves.
    Reveal(Option<Reveal>),
    /// Report it in this frame's `exited` and unmount it. `Scene::frame` carries this one out
    /// itself, as it collects the list.
    Retire,
    /// Unmount it now, with no exit to play. Nothing follows it.
    Unmount,
    /// Record a motion-trace intent.
    Intent { bits: u32, seconds: f32 },
}

/// The acts a step asks for, in order. An exit asks for at most six, so they fit inline and
/// nothing here allocates.
pub(super) struct Acts {
    items: [Act; 6],
    len: usize,
}

impl Acts {
    fn none() -> Acts {
        Acts { items: [Act::Dirty; 6], len: 0 }
    }

    fn push(&mut self, act: Act) {
        if let Some(slot) = self.items.get_mut(self.len) {
            *slot = act;
            self.len += 1;
        }
    }

    fn extend<const N: usize>(&mut self, acts: [Act; N]) {
        for act in acts {
            self.push(act);
        }
    }
}

impl IntoIterator for Acts {
    type Item = Act;
    type IntoIter = core::iter::Take<core::array::IntoIter<Act, 6>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(self.len)
    }
}

/// Moves `state` one event on, bumping `generation` where a new exit begins, and returns what the
/// scene should do about it.
pub(super) fn step(state: &mut Presence, generation: &mut u32, event: Event) -> Acts {
    use Presence::{Entering, Exiting, Present};
    let mut acts = Acts::none();
    match event {
        Event::Mount { ins, animate, entrance } => {
            let look = fx_of(&ins);
            if animate {
                acts.extend([
                    Act::Land(0.0),
                    Act::Run { target: 1.0, anim: entrance },
                    Act::Intent { bits: Intent::TRANSITION | Intent::OPACITY, seconds: entrance.curve.logical_duration() },
                ]);
                *state = Entering { at0: look, at1: FX_ID };
            } else {
                acts.push(Act::LandIfNeeded(1.0));
                *state = Present { at0: look, at1: FX_ID };
            }
        }
        Event::Revive { p, ins, entrance } => {
            let (old0, old1) = state.looks();
            // Keep the current look at p, land exactly on identity at 1. The last lane (drawn
            // shape) is kept as it was, as an entrance never re-solves it.
            let mut at0 = old0;
            if p <= 0.001 {
                at0 = fx_of(&ins);
            } else {
                let cur = lerp_fx(&old0, &old1, p);
                let q = (1.0 - p).max(1e-3);
                for k in 0..8 {
                    at0[k] = (cur[k] - FX_ID[k] * p) / q;
                }
            }
            *generation += 1;
            *state = Entering { at0, at1: FX_ID };
            acts.extend([
                Act::Dirty,
                Act::Run { target: 1.0, anim: entrance },
                Act::Intent { bits: Intent::TRANSITION | Intent::OPACITY, seconds: entrance.curve.logical_duration() },
            ]);
        }
        Event::Land { ins } => {
            *generation += 1;
            *state = Present { at0: fx_of(&ins), at1: FX_ID };
            acts.extend([Act::Anchor(ins.anchor), Act::Reveal(ins.reveal), Act::Land(1.0)]);
        }
        Event::Appear => {
            // Fully present, no presence effect. Only the absent look is replaced: an exit that
            // was under way keeps the look it was leaving from.
            let at1 = state.looks().1;
            *state = Present { at0: FX_ID, at1 };
            acts.push(Act::Land(1.0));
        }
        Event::FadeIn { entrance } => {
            let (at0, at1) = state.looks();
            *state = Entering { at0, at1 };
            acts.extend([Act::Land(0.0), Act::Run { target: 1.0, anim: entrance }]);
        }
        Event::Exit { p, rem, exit } => {
            if state.exiting() {
                return acts;
            }
            let (old0, old1) = state.looks();
            let unplayed = p < 1e-3;
            if unplayed && lerp_fx(&old0, &old1, p)[0] < 1.0 / 512.0 {
                // An entrance that hasn't shown anything yet (a delayed one still waiting) has
                // no exit to play.
                acts.push(Act::Unmount);
                return acts;
            }
            let target = fx_of(&rem);
            let mut at1 = old1;
            if p >= 0.999 {
                at1 = FX_ID;
            } else if unplayed {
                // None of the entrance has played, but its insertion effect shows: leave from
                // there, as a whole exit from it.
                at1 = lerp_fx(&old0, &old1, p);
            } else {
                // Mid-entrance: keep the current look at p, land exactly on the removal effect
                // at 0. The last lane (drawn shape) stays as it was.
                let cur = lerp_fx(&old0, &old1, p);
                for k in 0..8 {
                    at1[k] = target[k] + (cur[k] - target[k]) / p.max(1e-3);
                }
            }
            *generation += 1;
            *state = Exiting { at0: target, at1 };
            if rem.scale != 1.0 {
                acts.push(Act::Anchor(rem.anchor));
            }
            if rem.reveal.is_some() {
                acts.push(Act::Reveal(rem.reveal));
            }
            acts.push(Act::Dirty);
            if unplayed {
                acts.push(Act::Land(1.0));
            } else {
                acts.push(Act::Ensure);
            }
            acts.extend([
                Act::Run { target: 0.0, anim: exit },
                Act::Intent { bits: Intent::TRANSITION | Intent::OPACITY, seconds: exit.curve.logical_duration() },
            ]);
        }
        Event::Settle => match state {
            Present { .. } => {}
            Entering { at0, at1 } => *state = Present { at0: *at0, at1: *at1 },
            Exiting { .. } => acts.push(Act::Retire),
        },
        Event::Faded { p } => {
            // An exit nobody can see is over: presence has faded below one 8-bit step.
            if state.exiting() {
                let (at0, at1) = state.pair();
                if at0[0] + (at1[0] - at0[0]) * p < 1.0 / 255.0 {
                    acts.push(Act::Retire);
                }
            }
        }
    }
    acts
}
