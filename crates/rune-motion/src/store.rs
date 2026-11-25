//! The animation store: every animated property of every node, in flat arrays the render thread
//! samples once per frame.
//!
//! - **Struct of arrays.** Lane targets, values, velocities and segments live in parallel `Vec`s;
//!   a track (one property of one node) is a contiguous run of lanes.
//! - **Only active tracks cost anything.** `tick` walks a dense list of active track indices.
//!   Settled tracks snap to `(target, 0)`, leave the list and cost nothing until retargeted.
//! - **No allocation per frame.** Pools for additive layers and delayed events use intrusive free
//!   lists, and the event and touched buffers keep their capacity.
//! - **Ids, not references.** `NodeId` + `Prop<T>` or a generational `TrackId`.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::carry::Carry;
use crate::curve::{Anim, Blend, Curve, Decay, Velocity};
use crate::layer::{Kind, Layer};
use crate::props::{NodeId, Prop, PropClass, PropId};
use crate::spring::Spring;
use crate::value::{Animatable, MAX_LANES, lanes};

mod clock;
mod compact;
mod lanes;
mod started;
mod tracks;
mod typed;

use self::tracks::Mode;

const NONE: u32 = u32::MAX;

/// A generational handle to one track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackId {
    index: u32,
    generation: u32,
}

impl TrackId {
    pub(crate) fn index_usize(self) -> usize {
        self.index as usize
    }

    pub(crate) fn set_index(&mut self, index: u32) {
        self.index = index;
    }
}

/// What happened during a tick (or since the last one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionEvent {
    /// The track reached `(target, 0)` and went dormant.
    Settled { track: TrackId, node: NodeId, prop: PropId },
    /// An animation started with `.completion(token)` finished (`finished`), or was replaced by a
    /// newer animation before it did (`!finished`).
    Completed { token: u64, node: NodeId, prop: PropId, finished: bool },
}

/// How a released gesture continues.
#[derive(Clone, Copy, Debug)]
pub enum Release<T> {
    /// Spring to a target, starting from the release velocity.
    Spring { target: T, spring: Spring },
    /// Coast with exponential decay to the projected rest point.
    Decay(Decay),
    /// Coast, but if the projection leaves `[min, max]`, spring onto the bound with the velocity
    /// the value has when it reaches it (scroll views). If it is already out of bounds
    /// (rubber-banded), spring straight back.
    DecayClamped { decay: Decay, min: T, max: T, spring: Spring },
}

#[derive(Clone, Copy)]
struct Extra {
    layer: Layer,
    next: u32,
}

#[derive(Clone, Copy)]
struct Pending {
    at: f64,
    target: f32,
    /// NaN: inherit.
    velocity: f32,
    curve: Curve,
    blend: Blend,
    next: u32,
}

#[derive(Clone, Copy)]
struct Meta {
    node: NodeId,
    prop: PropId,
    class: PropClass,
    generation: u32,
    alive: bool,
    epoch: u32,
    token: u64,
    logical_at: f64,
    node_next: u32,
}

/// A fast, unkeyed hasher for small fixed-size keys (ids, glyph keys) that no input chooses.
#[derive(Default)]
pub struct Fx(u64);
impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(b as u64);
        }
    }
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64)
    }
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64)
    }
    fn write_u64(&mut self, i: u64) {
        self.add(i)
    }
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64)
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
impl Fx {
    #[inline]
    fn add(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}
pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

/// A clock some nodes' tracks run on instead of the store's: it advances by `dt × scale` each
/// time the store's time moves. Nodes at the same scale share one; `nodes == 0` marks it free.
#[derive(Clone, Copy)]
struct SubClock {
    time: f64,
    scale: f32,
    nodes: u32,
}

/// Optional diagnostics for inherited spatial spring retargets (points per second).
#[derive(Clone, Copy, Debug, Default)]
pub struct RetargetAudit {
    pub lanes: u64,
    pub max_velocity_error: f32,
}

/// All animated properties, sampled together at one presentation time.
pub struct AnimationStore {
    now: f64,
    pixel_scale: f32,
    reduced_motion: bool,

    l_target: Vec<f32>,
    l_value: Vec<f32>,
    l_vel: Vec<f32>,
    l_layer: Vec<Layer>,
    l_more: Vec<u32>,
    l_pending: Vec<u32>,
    l_curve: Vec<Curve>,
    lane_free: Vec<Vec<u32>>,

    t_lane0: Vec<u32>,
    t_n: Vec<u8>,
    t_settle_at: Vec<f64>,
    t_next_due: Vec<f64>,
    t_slot: Vec<u32>,
    t_meta: Vec<Meta>,
    /// The track's clock: 0 is the store's own, otherwise an index into `clocks`.
    t_clock: Vec<u32>,
    /// Sampled at its held pose by the last tick (in whichever [`Mode`] it holds), so `tick`
    /// can skip it until [`mode`](Self::mode) says it moves. [`resample`](Self::resample)
    /// clears it wherever a track's timing or pose changes.
    t_held: Vec<bool>,
    track_free: Vec<u32>,

    /// Index 0 stands for the store's own clock and is never read.
    clocks: Vec<SubClock>,
    clock_free: Vec<u32>,
    node_clock: FxMap<u64, u32>,

    active: Vec<u32>,
    /// With the generation they had, so a track freed (or its slot reused) since drops out
    /// where these are read, and freeing a track never scans them.
    touched: Vec<TrackId>,
    set_touched: Vec<TrackId>,
    /// Tracks started or retargeted since the last tick, and between the two before (`started`).
    starts: Vec<TrackId>,
    ticked_starts: Vec<TrackId>,

    extra: Vec<Extra>,
    extra_free: u32,
    pending: Vec<Pending>,
    pending_free: u32,

    index: FxMap<(u64, u16), u32>,
    node_head: FxMap<u64, u32>,

    events: Carry<MotionEvent>,
    retarget_audit: Option<RetargetAudit>,
}

impl Default for AnimationStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimationStore {
    pub fn new() -> Self {
        AnimationStore {
            now: 0.0,
            pixel_scale: 2.0,
            reduced_motion: false,
            l_target: Vec::new(),
            l_value: Vec::new(),
            l_vel: Vec::new(),
            l_layer: Vec::new(),
            l_more: Vec::new(),
            l_pending: Vec::new(),
            l_curve: Vec::new(),
            lane_free: (0..=MAX_LANES).map(|_| Vec::new()).collect(),
            t_lane0: Vec::new(),
            t_n: Vec::new(),
            t_settle_at: Vec::new(),
            t_next_due: Vec::new(),
            t_slot: Vec::new(),
            t_meta: Vec::new(),
            t_clock: Vec::new(),
            t_held: Vec::new(),
            track_free: Vec::new(),
            clocks: Vec::new(),
            clock_free: Vec::new(),
            node_clock: FxMap::default(),
            active: Vec::new(),
            touched: Vec::new(),
            set_touched: Vec::new(),
            starts: Vec::new(),
            ticked_starts: Vec::new(),
            extra: Vec::new(),
            extra_free: NONE,
            pending: Vec::new(),
            pending_free: NONE,
            index: FxMap::default(),
            node_head: FxMap::default(),
            events: Carry::new(),
            retarget_audit: None,
        }
    }

    /// Pre-sizes for `tracks` tracks of about `lanes_per_track` lanes, so even the first frames
    /// don't allocate.
    pub fn with_capacity(tracks: usize, lanes_per_track: usize) -> Self {
        let mut s = Self::new();
        let lanes = tracks * lanes_per_track;
        s.l_target.reserve(lanes);
        s.l_value.reserve(lanes);
        s.l_vel.reserve(lanes);
        s.l_layer.reserve(lanes);
        s.l_more.reserve(lanes);
        s.l_pending.reserve(lanes);
        s.l_curve.reserve(lanes);
        s.t_lane0.reserve(tracks);
        s.t_n.reserve(tracks);
        s.t_settle_at.reserve(tracks);
        s.t_next_due.reserve(tracks);
        s.t_slot.reserve(tracks);
        s.t_meta.reserve(tracks);
        s.t_clock.reserve(tracks);
        s.t_held.reserve(tracks);
        s.active.reserve(tracks);
        s.touched.reserve(tracks);
        s.events.reserve(tracks * 2);
        s.index.reserve(tracks);
        s.node_head.reserve(tracks);
        s
    }

    // ------------------------------------------------------------------------------------------
    // Time and policy

    /// The presentation time animations are applied at and sampled for (seconds, from a [`crate::Clock`]).
    pub fn now(&self) -> f64 {
        self.now
    }

    /// Opt-in trace diagnostics; disabled in normal rendering. Measures inherited spatial
    /// spring velocity at actual retarget boundaries, excluding deliberate instant cuts.
    pub fn audit_retargets(&mut self, enabled: bool) {
        self.retarget_audit = enabled.then(RetargetAudit::default);
    }

    pub fn take_retarget_audit(&mut self) -> RetargetAudit {
        self.retarget_audit.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Sets the frame's target presentation time. Apply this frame's commits after this, then `tick`.
    pub fn set_time(&mut self, t: f64) {
        let dt = t - self.now;
        self.now = t;
        for c in self.clocks.iter_mut().skip(1).filter(|c| c.nodes > 0 && c.scale != 0.0) {
            c.time += dt * c.scale as f64;
        }
    }

    /// `set_time(t)` then `tick()`.
    pub fn tick_at(&mut self, t: f64) {
        self.set_time(t);
        self.tick();
    }

    /// Device pixels per point; sets settle thresholds below the 1/64 px snap quantum.
    pub fn set_pixel_scale(&mut self, scale: f32) {
        self.pixel_scale = scale.max(1.0);
    }

    pub fn pixel_scale(&self) -> f32 {
        self.pixel_scale
    }

    /// The OS Reduce Motion setting. Applied per property class to every new animation.
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.reduced_motion = on;
    }

    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    // ------------------------------------------------------------------------------------------
    // The frame

    /// Samples every active track at `now()`, applies due delayed animations, settles finished
    /// tracks. Allocation-free once buffers have grown to the working-set size.
    pub fn tick(&mut self) {
        self.events.begin();
        self.touched.clear();
        let meta = &self.t_meta;
        let live = |id: &&TrackId| meta[id.index as usize].alive && meta[id.index as usize].generation == id.generation;
        self.touched.extend(self.set_touched.iter().filter(live));
        self.set_touched.clear();
        std::mem::swap(&mut self.starts, &mut self.ticked_starts);
        self.starts.clear();
        let clocked = !self.node_clock.is_empty();
        let mut i = 0;
        while i < self.active.len() {
            let t = self.active[i] as usize;
            let now = if clocked { self.time_of(t) } else { self.now };
            // Nothing moves on a stopped clock, or on a track that only waits for a delayed
            // start: once sampled where it holds, it isn't touched again until it moves.
            let mode = self.mode(t);
            if mode.holds() && self.t_held[t] {
                i += 1;
                continue;
            }
            self.touched.push(TrackId { index: t as u32, generation: self.t_meta[t].generation });
            if self.t_next_due[t] <= now {
                self.apply_due(t, now);
            }
            self.t_held[t] = mode.holds();
            let l0 = self.t_lane0[t] as usize;
            let n = self.t_n[t] as usize;
            let range = l0..l0 + n;
            let mut stacked = false;
            {
                // One bounds check per track, then straight-line lane sampling.
                let layers = &self.l_layer[range.clone()];
                let more = &self.l_more[range.clone()];
                let targets = &self.l_target[range.clone()];
                let values = &mut self.l_value[range.clone()];
                let vels = &mut self.l_vel[range.clone()];
                for k in 0..n {
                    let (o, v) = layers[k].sample(now);
                    values[k] = targets[k] + o;
                    vels[k] = v;
                    stacked |= more[k] != NONE;
                }
            }
            if stacked {
                for l in range {
                    if self.l_more[l] != NONE {
                        let (o2, v2) = self.sample_extra(l, now);
                        self.l_value[l] += o2;
                        self.l_vel[l] += v2;
                    }
                }
            }
            if now >= self.t_settle_at[t] && self.t_next_due[t] == f64::INFINITY {
                self.settle(t);
                self.deactivate(t, i);
                continue;
            }
            i += 1;
        }
        self.events.end();
    }

    /// Events since the previous tick (settles, completions).
    pub fn events(&self) -> &[MotionEvent] {
        self.events.as_slice()
    }

    /// The values presented at the last tick for a track, as raw lanes.
    pub fn values_of(&self, id: TrackId) -> Option<&[f32]> {
        self.contains(id).then(|| self.lane_values(id.index as usize))
    }

    /// Where each lane of a track comes to rest (its latest target, delayed ones included),
    /// into `out`. False when the track is gone.
    pub(crate) fn targets_of(&self, id: TrackId, out: &mut [f32]) -> bool {
        if !self.contains(id) {
            return false;
        }
        let t = id.index as usize;
        let l0 = self.t_lane0[t] as usize;
        for (k, slot) in out.iter_mut().enumerate().take(self.t_n[t] as usize) {
            *slot = self.final_target(l0 + k);
        }
        true
    }

    /// Is the track currently animating?
    pub fn is_active(&self, id: TrackId) -> bool {
        self.contains(id) && self.t_slot[id.index as usize] != NONE
    }

    /// Exact `(value, velocity)` lanes of a track at `now()`, into `x` and `v`.
    pub(crate) fn sample_exact(&self, id: TrackId, x: &mut [f32], v: &mut [f32]) -> bool {
        if !self.contains(id) {
            return false;
        }
        let t = id.index as usize;
        let l0 = self.t_lane0[t] as usize;
        let now = self.time_of(t);
        for i in 0..self.t_n[t] as usize {
            let (a, b) = self.sample_lane(l0 + i, now);
            x[i] = a;
            v[i] = b;
        }
        true
    }

    pub fn active_len(&self) -> usize {
        self.active.len()
    }

    /// Some track is animating on a clock that moves. Tracks held by a stopped clock don't count.
    pub fn running(&self) -> bool {
        if self.node_clock.is_empty() {
            return !self.active.is_empty();
        }
        self.active.iter().any(|&t| !matches!(self.mode(t as usize), Mode::Frozen))
    }

    pub fn track_len(&self) -> usize {
        self.t_meta.iter().filter(|m| m.alive).count()
    }

    // ------------------------------------------------------------------------------------------
    // Untyped core (used by the scene, keyframes and choreography)

    pub(crate) fn ensure(&mut self, node: NodeId, prop: PropId, class: PropClass, init: &[f32]) -> u32 {
        if let Some(&t) = self.index.get(&(node.0, prop.0)) {
            self.check_lanes(t as usize, init.len());
            return t;
        }
        let n = init.len();
        assert!(n > 0 && n <= MAX_LANES);
        let l0 = self.lane_free[n].pop().unwrap_or_else(|| {
            let l0 = self.l_target.len();
            let end = l0 + n;
            self.l_target.resize(end, 0.0);
            self.l_value.resize(end, 0.0);
            self.l_vel.resize(end, 0.0);
            self.l_layer.resize(end, Layer::REST);
            self.l_more.resize(end, NONE);
            self.l_pending.resize(end, NONE);
            self.l_curve.resize(end, Curve::Instant);
            l0 as u32
        });
        for (i, &x) in init.iter().enumerate() {
            let l = l0 as usize + i;
            self.l_target[l] = x;
            self.l_value[l] = x;
            self.l_vel[l] = 0.0;
            self.l_layer[l] = Layer::REST;
            self.l_more[l] = NONE;
            self.l_pending[l] = NONE;
            self.l_curve[l] = Curve::Instant;
        }
        let head = self.node_head.get(&node.0).copied().unwrap_or(NONE);
        let clock = if self.node_clock.is_empty() { 0 } else { self.node_clock.get(&node.0).copied().unwrap_or(0) };
        let t = match self.track_free.pop() {
            Some(t) => {
                let t = t as usize;
                let generation = self.t_meta[t].generation.wrapping_add(1);
                self.t_meta[t] = Meta::new(node, prop, class, generation, head);
                self.t_lane0[t] = l0;
                self.t_n[t] = n as u8;
                self.t_settle_at[t] = f64::NEG_INFINITY;
                self.t_next_due[t] = f64::INFINITY;
                self.t_slot[t] = NONE;
                self.t_clock[t] = clock;
                self.resample(t);
                t as u32
            }
            None => {
                self.t_meta.push(Meta::new(node, prop, class, 0, head));
                self.t_lane0.push(l0);
                self.t_n.push(n as u8);
                self.t_settle_at.push(f64::NEG_INFINITY);
                self.t_next_due.push(f64::INFINITY);
                self.t_slot.push(NONE);
                self.t_clock.push(clock);
                self.t_held.push(false);
                (self.t_meta.len() - 1) as u32
            }
        };
        self.index.insert((node.0, prop.0), t);
        self.node_head.insert(node.0, t);
        t
    }

    pub(crate) fn id_of(&self, t: usize) -> TrackId {
        TrackId { index: t as u32, generation: self.t_meta[t].generation }
    }

    fn check_lanes(&self, t: usize, n: usize) {
        assert_eq!(
            self.t_n[t] as usize, n,
            "property {:?} of {:?} was created with {} lanes, used with {}",
            self.t_meta[t].prop, self.t_meta[t].node, self.t_n[t], n
        );
    }

    pub(crate) fn lane_targets_4(&self, t: usize) -> [f32; 4] {
        let l0 = self.t_lane0[t] as usize;
        core::array::from_fn(|i| self.final_target(l0 + i))
    }

    fn lane_values(&self, t: usize) -> &[f32] {
        let l0 = self.t_lane0[t] as usize;
        &self.l_value[l0..l0 + self.t_n[t] as usize]
    }

    pub(crate) fn set_raw(&mut self, t: u32, values: &[f32]) {
        let t = t as usize;
        let l0 = self.t_lane0[t] as usize;
        for (i, &x) in values.iter().enumerate() {
            let l = l0 + i;
            self.cancel_pending(l);
            self.free_extra(l);
            self.l_layer[l] = Layer::REST;
            self.l_target[l] = x;
            self.l_value[l] = x;
            self.l_vel[l] = 0.0;
            self.l_curve[l] = Curve::Instant;
        }
        self.t_next_due[t] = f64::INFINITY;
        self.t_settle_at[t] = f64::NEG_INFINITY;
        self.take_token(t, false);
        self.t_meta[t].epoch = self.t_meta[t].epoch.wrapping_add(1);
        if self.t_slot[t] != NONE {
            let slot = self.t_slot[t] as usize;
            self.deactivate(t, slot);
        }
        self.set_touched.push(TrackId { index: t as u32, generation: self.t_meta[t].generation });
    }

    /// Animates lanes of track `t` toward `targets`. `lane_vel` overrides per-lane start velocity.
    /// False when no lane's target changed, so nothing moves.
    pub(crate) fn animate_raw(&mut self, t: usize, targets: &[f32], mut anim: Anim, lane_vel: Option<&[f32]>) -> bool {
        let now = self.time_of(t);
        if self.t_next_due[t] <= now {
            self.apply_due(t, now);
        }
        let class = self.t_meta[t].class;
        if self.reduced_motion {
            anim.curve = class.reduce(anim.curve);
        }
        let eps = class.eps(self.pixel_scale);
        let blend = anim.resolved_blend();
        let l0 = self.t_lane0[t] as usize;
        let mut changed = false;
        for (i, &target) in targets.iter().enumerate() {
            let l = l0 + i;
            let vel = match (lane_vel, anim.velocity) {
                (Some(v), _) => Some(v[i]),
                (None, Velocity::Inherit) => None,
                (None, Velocity::Zero) => Some(0.0),
                (None, Velocity::Uniform(u)) => Some(u),
            };
            let decay = matches!(anim.curve, Curve::Decay(_));
            if vel.is_none() && !decay && self.final_target(l) == target {
                continue; // unchanged target: keep the running trajectory and any pending due time
            }
            changed = true;
            self.cancel_pending(l);
            if anim.delay > 0.0 {
                self.push_pending(l, Pending {
                    at: now + anim.delay as f64,
                    target,
                    velocity: vel.unwrap_or(f32::NAN),
                    curve: anim.curve,
                    blend,
                    next: NONE,
                });
            } else {
                let before = (self.retarget_audit.is_some() && class == PropClass::Spatial
                    && matches!(anim.curve, Curve::Spring(_)) && vel.is_none()
                    && self.l_layer[l].end > now).then(|| self.sample_lane(l, now).1);
                self.start_lane(l, now, target, anim.curve, blend, vel, eps);
                if let Some(before) = before {
                    let error = (self.sample_lane(l, now).1 - before).abs();
                    let audit = self.retarget_audit.as_mut().unwrap();
                    audit.lanes += 1;
                    audit.max_velocity_error = audit.max_velocity_error.max(error);
                }
            }
        }
        if changed {
            let end = now + anim.delay as f64 + anim.curve.logical_duration() as f64;
            self.commit_scheduled(t, end, anim.token);
        } else if anim.token != 0 {
            if self.t_slot[t] == NONE {
                let m = self.t_meta[t];
                self.events.push(MotionEvent::Completed { token: anim.token, node: m.node, prop: m.prop, finished: true });
            } else {
                self.take_token(t, false);
                self.t_meta[t].token = anim.token;
            }
        }
        changed
    }

    /// Animates a 2-lane track (`prop::TRAVEL`) with independent per-axis curves and starting
    /// velocities in one pass, avoiding double-starting lane 0 when `anim_x` and `anim_y` are
    /// additive tweens or differ by axis.
    pub(crate) fn animate_axes_2(
        &mut self,
        t: usize,
        targets: [f32; 2],
        anims: [Anim; 2],
        vels: [f32; 2],
    ) {
        let now = self.time_of(t);
        if self.t_next_due[t] <= now {
            self.apply_due(t, now);
        }
        let class = self.t_meta[t].class;
        let eps = class.eps(self.pixel_scale);
        let l0 = self.t_lane0[t] as usize;
        let mut end = now;
        for (i, (&target, &vel)) in targets.iter().zip(&vels).enumerate() {
            let mut anim = anims[i];
            if self.reduced_motion {
                anim.curve = class.reduce(anim.curve);
            }
            let blend = anim.resolved_blend();
            let l = l0 + i;
            self.cancel_pending(l);
            if anim.delay > 0.0 {
                self.push_pending(l, Pending {
                    at: now + anim.delay as f64,
                    target,
                    velocity: vel,
                    curve: anim.curve,
                    blend,
                    next: NONE,
                });
            } else {
                self.start_lane(l, now, target, anim.curve, blend, Some(vel), eps);
            }
            end = end.max(now + anim.delay as f64 + anim.curve.logical_duration() as f64);
        }
        let token = if anims[1].token != 0 { anims[1].token } else { anims[0].token };
        self.commit_scheduled(t, end, token);
    }
}

impl Meta {
    fn new(node: NodeId, prop: PropId, class: PropClass, generation: u32, node_next: u32) -> Meta {
        Meta { node, prop, class, generation, alive: true, epoch: 0, token: 0, logical_at: f64::NEG_INFINITY, node_next }
    }
}
