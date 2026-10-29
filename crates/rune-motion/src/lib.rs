//! # rune-motion
//!
//! Rune's animation runtime: pure math and data structures the render thread drives once per
//! frame. App code states targets; this crate turns them into presented values with no app work
//! per frame (PLAN.md §3, §4.9; research 06).
//!
//! - [`Spring`]: SwiftUI's `Spring(duration:bounce:)`, closed form for every damping regime.
//! - [`Curve`]: springs, CSS tweens ([`Easing`]), decay (fling), keyframes.
//! - [`Animatable`]: f32, [`Vec2`], [`Rect`] (as edges), [`Color`] (premultiplied OKLab),
//!   [`Corners`], [`Transform`], [`Blur`], tuples, arrays.
//! - [`AnimationStore`]: struct-of-arrays tracks, retargeting that keeps position and velocity,
//!   merge or additive blending, delays, settle events, zero allocation per frame.
//! - [`Scene`]: presence (exits that stay mounted), [`Transition`]s incl. blur-replace, layout
//!   animation by edges, screen-space travel, matched geometry, per-frame [`Presented`] output.
//! - [`Choreography`], [`Stagger`], [`PhaseAnimator`], [`Keyframes`]; [`Clock`]; motion-lint
//!   [`MotionTrace`]s.
//!
//! ```
//! use rune_motion::prelude::*;
//!
//! let mut store = AnimationStore::new();
//! let card = NodeId(1);
//! store.set(card, prop::OPACITY, 0.0);
//! store.animate(card, prop::OPACITY, 1.0, motion::SETTLE);
//! store.tick_at(0.15);
//! let o = store.get(card, prop::OPACITY).unwrap();
//! assert!(o > 0.5 && o < 1.0);
//! ```
#![deny(unsafe_code)]

mod animated;
mod choreo;
mod clock;
mod curve;
mod cycle;
mod easing;
mod gesture;
mod keyframes;
mod layer;
mod looping;
mod sequence;
pub mod math;
mod props;
mod scene;
pub mod snap;
mod spring;
mod store;
pub mod tokens;
mod trace;
mod transition;
mod value;

pub use animated::Animated;
pub use choreo::{Choreography, Cue, Ops, PhaseAnimator, Stagger, StaggerFrom, StepId, When, stagger};
pub use clock::Clock;
pub use curve::{Anim, AnimExt, Blend, Curve, Decay, Velocity};
pub use cycle::Cycle;
pub use easing::{CubicBezier, Easing, StepPosition};
pub use gesture::{VelocityTracker, deceleration, project, rubber_band, rubber_band_clamp, rubber_band_with};
pub use keyframes::{Keyframe, Keyframes};
pub use layer::Eps;
pub use looping::{LoopKind, LoopSpec, LoopTrack};
pub use sequence::{Beat, Sequence};
pub use props::{NodeId, Prop, PropClass, PropId, REDUCED_FADE, prop};
pub use scene::{FrameOutput, LayoutAnim, Mask, Mount, NodeSpec, Phase, Presented, Scene, Travel, WIDTH_LEAD};
pub use spring::{Spring, SpringPair};
#[doc(hidden)]
pub use store::Fx;
pub use store::{AnimationStore, MotionEvent, Release, RetargetAudit, TrackId};
pub use tokens as motion;
pub use trace::{Intent, MotionTrace, SharedBuffer, TraceChannels, TraceHeader, flags};
pub use transition::{Edge, Effect, Reveal, SMALL_SCALE_FLOOR, Transition};
pub use value::{
    Animatable, Blur, Color, Corners, MAX_LANES, Rect, Sides, Transform, Vec2, linear_to_oklab, oklab_to_linear,
    srgb_decode, srgb_encode,
};

/// Everything an animation call site needs.
pub mod prelude {
    pub use crate::{
        Anim, AnimExt, Animatable, Animated, AnimationStore, Blend, Clock, Color, Corners, Cue, Curve, Decay, Easing,
        Edge, Keyframes, LayoutAnim, Mount, NodeId, NodeSpec, Presented, Rect, Release, Scene, Spring, SpringPair,
        Stagger, StaggerFrom, Transition, Travel, Vec2, Velocity, motion, prop,
    };
}
