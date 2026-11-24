//! The presented tree the render thread owns: presence (exits that stay mounted), layout animation
//! by edges, screen-space travel, matched geometry, and the per-frame [`Presented`] output.

use crate::carry::Carry;
use crate::curve::{Anim, Curve};
use crate::props::{NodeId, Prop, PropId, prop};
use crate::snap::snap_rect;
use crate::spring::{Spring, SpringPair};
use crate::store::{AnimationStore, FxMap, MotionEvent, TrackId};
use crate::trace::{Intent, MotionTrace, flags};
use crate::transition::{Effect, Reveal, Transition};
use crate::value::{Animatable, Color, Corners, MAX_LANES, Rect, Sides, Transform, Vec2, lanes};

mod frame;
mod layout;
mod phase;
mod presence;
mod props;
mod trailing;
mod travel;

use layout::D;
use phase::{Act, Event as PhaseEvent, Presence, lerp_fx};

const NONE: u32 = u32::MAX;
const B: usize = prop::BUILTIN_COUNT;

/// Width-leads ratio for the X axis spring (native morph: `width_vd = 0.78 · vd`).
pub const WIDTH_LEAD: f32 = 0.78;

/// Where a node is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Entering,
    Present,
    /// Removed by the app; still drawn until its exit settles. `generation` guards against stale exits.
    Exiting { generation: u32 },
}

/// Whether an insert is part of the first build (no animation, Kit rule 6) or a later change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mount {
    Initial,
    Animated,
}

/// The coordinate space layout motion is tracked in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Travel {
    /// Edges animate in parent space; an unchanged child rides its parent.
    #[default]
    Parent,
    /// The node travels one straight screen-space path from where it was seen to where it lands,
    /// whatever its ancestors do (Kit rule 12). Interpolates start and end root-space rects with
    /// one progress per axis.
    Screen,
}

/// How a frame change animates: per-axis curves, directional edge lead, and travel space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutAnim {
    pub x: Anim,
    pub y: Anim,
    pub lead: Option<(Anim, Anim)>,
    pub travel: Travel,
}

impl LayoutAnim {
    pub fn new(anim: impl Into<Anim>) -> LayoutAnim {
        let a = anim.into();
        LayoutAnim { x: a, y: a, lead: None, travel: Travel::Parent }
    }
    pub fn axes(x: impl Into<Anim>, y: impl Into<Anim>) -> LayoutAnim {
        LayoutAnim { x: x.into(), y: y.into(), lead: None, travel: Travel::Parent }
    }
    /// The X axis springs faster (duration × 0.78), so a widening surface fits its content first
    /// (Kit rule 9: "the width should animate much faster").
    pub fn width_leads(mut self) -> LayoutAnim {
        if let Curve::Spring(s) = self.x.curve {
            self.x.curve = Curve::Spring(s.with_duration(s.duration * WIDTH_LEAD));
        }
        self
    }
    /// When an axis travels, its leading edge in the direction of travel uses `lead` and its
    /// trailing edge uses `trail` (width leads along the travel direction).
    pub fn lead(mut self, lead: impl Into<Anim>, trail: impl Into<Anim>) -> LayoutAnim {
        self.lead = Some((lead.into(), trail.into()));
        self
    }
    pub fn screen(mut self) -> LayoutAnim {
        self.travel = Travel::Screen;
        self
    }
    pub const INSTANT: LayoutAnim =
        LayoutAnim { x: Anim::INSTANT, y: Anim::INSTANT, lead: None, travel: Travel::Parent };

    pub(crate) fn is_instant(&self) -> bool {
        self.x.is_instant()
            && self.y.is_instant()
            && self.lead.is_none_or(|(lead, trail)| lead.is_instant() && trail.is_instant())
    }
}

impl From<Spring> for LayoutAnim {
    fn from(s: Spring) -> Self {
        LayoutAnim::new(s)
    }
}
impl From<Anim> for LayoutAnim {
    fn from(a: Anim) -> Self {
        LayoutAnim::new(a)
    }
}
impl From<SpringPair> for LayoutAnim {
    fn from(p: SpringPair) -> Self {
        LayoutAnim::new(p.enter)
    }
}

/// What the app side sends when a keyed node appears.
#[derive(Clone, Copy, Debug)]
pub struct NodeSpec {
    pub parent: Option<NodeId>,
    /// Layout frame in parent space, at the target state.
    pub frame: Rect,
    pub opacity: f32,
    pub radius: Corners,
    pub fill: Color,
    pub transition: Transition,
}

impl NodeSpec {
    pub fn new(frame: Rect) -> NodeSpec {
        NodeSpec {
            parent: None,
            frame,
            opacity: 1.0,
            radius: Corners::ZERO,
            fill: Color::TRANSPARENT,
            transition: Transition::opacity(),
        }
    }
    pub fn parent(mut self, parent: NodeId) -> Self {
        self.parent = Some(parent);
        self
    }
    pub fn opacity(mut self, o: f32) -> Self {
        self.opacity = o;
        self
    }
    pub fn radius(mut self, r: impl Into<Corners>) -> Self {
        self.radius = r.into();
        self
    }
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = c;
        self
    }
    pub fn transition(mut self, t: Transition) -> Self {
        self.transition = t;
        self
    }
}

impl From<f32> for Corners {
    fn from(r: f32) -> Corners {
        Corners::all(r)
    }
}

/// A soft clip in the node's own coordinates: how far each side is pulled in, and how wide the
/// gradient ramp on that side is. The renderer evaluates it as an SDF ramp (Kit rule 11).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mask {
    pub inset: Sides,
    pub feather: Sides,
}

impl Mask {
    pub fn is_none(&self) -> bool {
        self.inset == Sides::ZERO && self.feather == Sides::ZERO
    }
}

/// Everything the renderer needs to draw one node this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presented {
    pub node: NodeId,
    pub parent: Option<NodeId>,
    /// Root-space rect including every ancestor's (and this node's) translation, both edges
    /// snapped to 1/64 device px. Shapes, borders and shadows are drawn at this size.
    pub rect: Rect,
    /// This node's own opacity (presence × property). Ancestors apply theirs as group opacity.
    pub opacity: f32,
    /// Scale and rotation about `anchor` (translation is already in `rect`). Ancestors' scale and
    /// rotation compose down the tree, like `CALayer`.
    pub transform: Transform,
    /// Root-space pivot for `transform`.
    pub anchor: Vec2,
    /// Own plus presence Gaussian blur sigma in points. Zero means "attach no filter".
    pub blur: f32,
    pub radius: Corners,
    pub fill: Color,
    pub mask: Mask,
    pub phase: Phase,
    /// False while exiting with `pop_layout`: the app's layout should no longer reserve space.
    pub in_flow: bool,
    /// motion-lint flags (animated, ancestorAnimated, clipped, filtered) and intent bits
    /// (own `<< 8`, ancestor `<< 16`).
    pub flags: u32,
    /// How much of this node's own shape is drawn, 0 to 1 ([`crate::Transition::draw`]).
    pub draw: f32,
}

impl Presented {
    /// The 2×3 affine for `transform` about `anchor`.
    pub fn affine(&self) -> [f32; 6] {
        self.transform.to_affine(self.anchor)
    }
}

/// One matched-geometry handover completed by [`Scene::match_geometry`] and its siblings:
/// `from` left the scene immediately and `to` took over from `start` toward `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handover {
    pub from: NodeId,
    pub to: NodeId,
    pub start: Rect,
    pub end: Rect,
}

/// One frame's output.
pub struct FrameOutput<'a> {
    /// Nodes whose presented values changed, parents before children.
    pub changed: &'a [Presented],
    /// Nodes whose exit settled this frame: draw nothing for them from now on.
    pub exited: &'a [NodeId],
    pub events: &'a [MotionEvent],
    /// Anything still moving? If not, the display link can pause.
    pub animating: bool,
}

#[derive(Clone, Copy)]
struct Path {
    start: Rect,
    end_local: Rect,
    /// Its parent's root-space target origin as the last frame drew it.
    origin: Vec2,
}

#[derive(Clone)]
struct Node {
    id: NodeId,
    parent: u32,
    first_child: u32,
    last_child: u32,
    next_sibling: u32,
    prev_sibling: u32,
    alive: bool,
    frame: Rect,
    opacity: f32,
    radius: Corners,
    fill: Color,
    transition: Transition,
    presence: Presence,
    /// How many exits this node has been through, reported with an exiting phase.
    generation: u32,
    /// Pivot for the presented transform. Set by the effect each phase entered from, and kept:
    /// a removal that scales moves it, and a static node still reports it.
    anchor: Vec2,
    /// Soft clip a reveal hides behind, or none. Kept across phases for the same reason.
    reveal: Option<Reveal>,
    path: Option<Path>,
    /// `set_ride_trailing`: x is kept against the parent's trailing edge while its width moves.
    ride_trailing: bool,
    tracks: [Option<TrackId>; B],
    dirty: bool,
    /// Root-space layout rect this frame (unsnapped) and the origin children ride (with offsets).
    world: Rect,
    origin: Vec2,
    last: Option<Presented>,
}

impl Node {
    /// Moves this node's presence one event on, updating `exiting`, and asks for what the scene
    /// should do about it.
    fn phase_step(&mut self, exiting: &mut Vec<NodeId>, event: PhaseEvent) -> phase::Acts {
        let was_exiting = self.presence.exiting();
        let acts = phase::step(&mut self.presence, &mut self.generation, event);
        let now_exiting = self.presence.exiting();
        if !was_exiting && now_exiting {
            exiting.push(self.id);
        } else if was_exiting && !now_exiting
            && let Some(k) = exiting.iter().position(|&x| x == self.id)
        {
            exiting.swap_remove(k);
        }
        acts
    }
}

/// Where a node comes to rest (see [`Scene::rest`]), beside `Scene::nodes` so the nodes the
/// renderer reads every frame stay the same size.
#[derive(Clone, Copy)]
struct Rest {
    /// Root-space presented rect with every track at its target, and the origin children rest on.
    rect: Rect,
    origin: Vec2,
    /// `rect` differs from where the node is this frame.
    moving: bool,
}

const REST_ZERO: Rest = Rest { rect: Rect::ZERO, origin: Vec2::ZERO, moving: false };

/// The render thread's animated tree.
pub struct Scene {
    store: AnimationStore,
    nodes: Vec<Node>,
    rests: Vec<Rest>,
    free: Vec<u32>,
    index: FxMap<u64, u32>,
    root_head: u32,
    root_tail: u32,
    order: Vec<u32>,
    order_dirty: bool,
    out: Vec<Presented>,
    /// What [`Scene::present`] presented after the motion trace wrote its frame, for the next.
    late: Vec<Presented>,
    /// Nodes currently in [`Phase::Exiting`], in no fixed order.
    exiting: Vec<NodeId>,
    exited: Carry<NodeId>,
    to_remove: Vec<u32>,
    /// Who is on a screen path, in the order the paths started. It only says who travels and in
    /// what order; the path itself is the node's.
    travelling: Vec<u32>,
    trace: Option<MotionTrace>,
    intents: Vec<(NodeId, u32, f32)>,
    /// Matched-geometry handovers completed since the last [`Scene::take_handovers`].
    handovers: Vec<Handover>,
    /// Per-node enter/exit curves overriding `Transition::springs` (`set_transition_motion`).
    transition_motion: FxMap<u64, (Anim, Anim)>,
    /// `keep_flights`: landings leave motion already on its way to the same value.
    keep: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    pub fn new() -> Scene {
        Scene {
            store: AnimationStore::new(),
            nodes: Vec::new(),
            rests: Vec::new(),
            free: Vec::new(),
            index: FxMap::default(),
            root_head: NONE,
            root_tail: NONE,
            order: Vec::new(),
            order_dirty: false,
            out: Vec::new(),
            late: Vec::new(),
            exiting: Vec::new(),
            exited: Carry::new(),
            to_remove: Vec::new(),
            travelling: Vec::new(),
            trace: None,
            intents: Vec::new(),
            handovers: Vec::new(),
            transition_motion: FxMap::default(),
            keep: false,
        }
    }

    /// While on, landing a property ([`Scene::set`], or [`Scene::set_frame`] with
    /// [`LayoutAnim::INSTANT`]) on the value it's already moving to leaves that motion running:
    /// an instant change lands what it changes, not what an earlier change set going. Off, a
    /// landing ends any motion, as settling needs.
    pub fn keep_flights(&mut self, on: bool) {
        self.keep = on;
    }

    /// `prop` is moving and its target is `value`, so a landing under `keep_flights` leaves it.
    fn in_flight_to<T: Animatable>(&self, id: NodeId, prop: Prop<T>, value: T) -> bool {
        if !self.keep || !self.store.is_animating(id, prop) {
            return false;
        }
        let Some(to) = self.store.target(id, prop) else { return false };
        let (mut a, mut b) = ([0.0; MAX_LANES], [0.0; MAX_LANES]);
        to.write_lanes(&mut a[..T::LANES]);
        value.write_lanes(&mut b[..T::LANES]);
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 1e-3)
    }

    pub fn store(&self) -> &AnimationStore {
        &self.store
    }
    /// For choreography and custom properties. Tracks of scene nodes are picked up automatically.
    pub fn store_mut(&mut self) -> &mut AnimationStore {
        &mut self.store
    }

    pub fn set_time(&mut self, t: f64) {
        self.store.set_time(t);
    }
    /// Runs the node's animations at `scale` times the scene's speed; see
    /// [`AnimationStore::set_node_time_scale`].
    pub fn set_time_scale(&mut self, id: NodeId, scale: f32) {
        self.store.set_node_time_scale(id, scale);
    }
    pub fn now(&self) -> f64 {
        self.store.now()
    }
    pub fn set_pixel_scale(&mut self, s: f32) {
        let previous = self.store.pixel_scale();
        self.store.set_pixel_scale(s);
        if self.store.pixel_scale() != previous {
            for node in self.nodes.iter_mut().filter(|n| n.alive) { node.dirty = true; }
        }
    }
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.store.set_reduced_motion(on);
        for n in self.nodes.iter_mut().filter(|n| n.alive) {
            n.dirty = true;
        }
    }

    /// Records every frame as motion-lint JSONL.
    pub fn set_trace(&mut self, trace: Option<MotionTrace>) {
        self.trace = trace;
        for node in self.nodes.iter_mut().filter(|n| n.alive) { node.dirty = true; }
    }
    pub fn tracing(&self) -> bool { self.trace.is_some() }
    pub fn trace_mut(&mut self) -> Option<&mut MotionTrace> { self.trace.as_mut() }
    /// Updates metadata even if the node's geometry has not changed.
    pub fn trace_name(&mut self, id: NodeId, name: Option<&'static str>) {
        if self.trace.as_mut().is_some_and(|trace| trace.name(id, name)) {
            if let Some(i) = self.slot(id) { self.nodes[i].dirty = true; }
        }
    }
    pub fn trace_namespace(&mut self, id: NodeId, namespace: Option<(&'static str, &'static str)>) {
        if self.trace.as_mut().is_some_and(|trace| trace.namespace(id, namespace)) {
            if let Some(i) = self.slot(id) { self.nodes[i].dirty = true; }
        }
    }
    pub fn take_trace(&mut self) -> Option<MotionTrace> {
        self.trace.take()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.index.contains_key(&id.0)
    }
    pub fn phase(&self, id: NodeId) -> Option<Phase> {
        let i = self.slot(id)?;
        Some(self.nodes[i].presence.public(self.nodes[i].generation))
    }
    /// Nodes currently in [`Phase::Exiting`], in no fixed order.
    pub fn exiting(&self) -> &[NodeId] {
        &self.exiting
    }
    /// The last presented snapshot (hit-test against what the user sees). Not tracked.
    pub fn presented(&self, id: NodeId) -> Option<Presented> {
        self.slot(id).and_then(|i| self.nodes[i].last)
    }
    /// The rect the node comes to rest at once every track (its own and its ancestors') reaches
    /// its target, snapped like [`Presented::rect`], while that differs from where it is: the
    /// difference from [`presented`](Self::presented) is the motion still in flight. Only a
    /// node whose [`Presented::flags`] has `ANIMATED` or `ANCESTOR_ANIMATED` can be moving.
    pub fn rest(&self, id: NodeId) -> Option<Rect> {
        let r = &self.rests[self.slot(id)?];
        r.moving.then(|| snap_rect(r.rect, self.store.pixel_scale()))
    }
    /// Root-space layout rect this frame (without this node's own offset or presence offset),
    /// snapped to the same 1/64 device-px grid as [`Presented::rect`].
    pub fn world(&self, id: NodeId) -> Option<Rect> {
        self.slot(id).map(|i| snap_rect(self.nodes[i].world, self.store.pixel_scale()))
    }
    pub fn len(&self) -> usize {
        self.index.len()
    }
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    fn slot(&self, id: NodeId) -> Option<usize> {
        self.index.get(&id.0).map(|&i| i as usize)
    }

    /// Lane values and velocities of track `t`, sampled exactly at `now()`.
    fn sample(&self, t: TrackId) -> Option<([f32; MAX_LANES], [f32; MAX_LANES])> {
        let mut x = [0.0; MAX_LANES];
        let mut v = [0.0; MAX_LANES];
        self.store.sample_exact(t, &mut x, &mut v).then_some((x, v))
    }

    /// Unmounts a node and its subtree now (no exit), reporting them in `exited`.
    fn remove_now(&mut self, i: usize) {
        // Children first.
        let mut c = self.nodes[i].first_child;
        while c != NONE {
            let next = self.nodes[c as usize].next_sibling;
            self.remove_now(c as usize);
            c = next;
        }
        let id = self.nodes[i].id;
        // Unlink from the parent's (or root) child list.
        let parent = self.nodes[i].parent;
        let next = self.nodes[i].next_sibling;
        let prev = self.nodes[i].prev_sibling;
        if prev == NONE {
            if parent == NONE {
                self.root_head = next;
            } else {
                self.nodes[parent as usize].first_child = next;
            }
        } else {
            self.nodes[prev as usize].next_sibling = next;
        }
        if next == NONE {
            if parent == NONE { self.root_tail = prev; } else { self.nodes[parent as usize].last_child = prev; }
        } else {
            self.nodes[next as usize].prev_sibling = prev;
        }
        if self.nodes[i].presence.exiting()
            && let Some(k) = self.exiting.iter().position(|&x| x == id)
        {
            self.exiting.swap_remove(k);
        }
        self.travelling.retain(|&x| x as usize != i);
        self.store.remove_node(id);
        if !self.transition_motion.is_empty() {
            self.transition_motion.remove(&id.0);
        }
        self.index.remove(&id.0);
        let n = &mut self.nodes[i];
        n.presence = Presence::LANDED;
        n.alive = false;
        n.dirty = false;
        n.first_child = NONE;
        n.last_child = NONE;
        n.next_sibling = NONE;
        n.prev_sibling = NONE;
        n.last = None;
        self.free.push(i as u32);
        self.exited.push(id);
        self.order_dirty = true;
    }

    fn rebuild_order(&mut self) {
        self.order.clear();
        let mut r = self.root_head;
        while r != NONE {
            self.order.push(r);
            r = self.nodes[r as usize].next_sibling;
        }
        let mut k = 0;
        while k < self.order.len() {
            let mut c = self.nodes[self.order[k] as usize].first_child;
            while c != NONE {
                self.order.push(c);
                c = self.nodes[c as usize].next_sibling;
            }
            k += 1;
        }
        self.order_dirty = false;
    }
}

