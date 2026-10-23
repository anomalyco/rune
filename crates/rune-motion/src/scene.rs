//! The presented tree the render thread owns: presence (exits that stay mounted), layout animation
//! by edges, screen-space travel, matched geometry, and the per-frame [`Presented`] output.

use crate::curve::{Anim, Curve};
use crate::props::{NodeId, Prop, PropId, prop};
use crate::snap::snap_rect;
use crate::spring::{Spring, SpringPair};
use crate::store::{AnimationStore, FxMap, MotionEvent, TrackId};
use crate::trace::{Intent, MotionTrace, flags};
use crate::transition::{Effect, Reveal, Transition};
use crate::value::{Animatable, Color, Corners, MAX_LANES, Rect, Sides, Transform, Vec2, lanes};

const NONE: u32 = u32::MAX;
const B: usize = prop::BUILTIN_COUNT;

/// Width-leads ratio for the X axis spring (Flit morph: `width_vd = 0.78 · vd`).
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
    /// Gaussian blur radius in points. 0 means "attach no filter". Enter/exit only.
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
}

impl Presented {
    /// The 2×3 affine for `transform` about `anchor`.
    pub fn affine(&self) -> [f32; 6] {
        self.transform.to_affine(self.anchor)
    }
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
}

// Effect state lanes: opacity, scale, offset x/y, size-fraction offset x/y, blur, hidden reveal.
type Fx = [f32; 8];
const FX_ID: Fx = [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

fn fx_of(e: &Effect) -> Fx {
    let hidden = if e.reveal.is_some() { 1.0 } else { 0.0 };
    [e.opacity, e.scale, e.offset.x, e.offset.y, e.offset_fraction.x, e.offset_fraction.y, e.blur, hidden]
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
    phase: Phase,
    generation: u32,
    /// Presence effect at p = 0 and p = 1 (normally the effect and identity; re-solved on a
    /// mid-flight reversal so nothing jumps).
    fx_a: Fx,
    fx_b: Fx,
    anchor: Vec2,
    reveal: Option<Reveal>,
    path: Option<Path>,
    tracks: [Option<TrackId>; B],
    dirty: bool,
    /// Root-space layout rect this frame (unsnapped) and the origin children ride (with offsets).
    world: Rect,
    origin: Vec2,
    last: Option<Presented>,
}

/// A small dual number: value and time derivative, for exact velocity of composed geometry.
#[derive(Clone, Copy, Default)]
struct D {
    x: f32,
    v: f32,
}
impl D {
    fn c(x: f32) -> D {
        D { x, v: 0.0 }
    }
    fn add(self, o: D) -> D {
        D { x: self.x + o.x, v: self.v + o.v }
    }
    fn sub(self, o: D) -> D {
        D { x: self.x - o.x, v: self.v - o.v }
    }
    fn mul(self, o: D) -> D {
        D { x: self.x * o.x, v: self.v * o.x + self.x * o.v }
    }
}

/// The render thread's animated tree.
pub struct Scene {
    store: AnimationStore,
    nodes: Vec<Node>,
    free: Vec<u32>,
    index: FxMap<u64, u32>,
    root_head: u32,
    root_tail: u32,
    order: Vec<u32>,
    order_dirty: bool,
    out: Vec<Presented>,
    exited: Vec<NodeId>,
    exited_seen: usize,
    to_remove: Vec<u32>,
    travelling: Vec<u32>,
    trace: Option<MotionTrace>,
    intents: Vec<(NodeId, u32, f32)>,
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
            free: Vec::new(),
            index: FxMap::default(),
            root_head: NONE,
            root_tail: NONE,
            order: Vec::new(),
            order_dirty: false,
            out: Vec::new(),
            exited: Vec::new(),
            exited_seen: 0,
            to_remove: Vec::new(),
            travelling: Vec::new(),
            trace: None,
            intents: Vec::new(),
        }
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
    pub fn now(&self) -> f64 {
        self.store.now()
    }
    pub fn set_pixel_scale(&mut self, s: f32) {
        self.store.set_pixel_scale(s);
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
    }
    pub fn take_trace(&mut self) -> Option<MotionTrace> {
        self.trace.take()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.index.contains_key(&id.0)
    }
    pub fn phase(&self, id: NodeId) -> Option<Phase> {
        self.slot(id).map(|i| self.nodes[i].phase)
    }
    /// The last presented snapshot (hit-test against what the user sees). Not tracked.
    pub fn presented(&self, id: NodeId) -> Option<Presented> {
        self.slot(id).and_then(|i| self.nodes[i].last)
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

    // ------------------------------------------------------------------------------------------
    // Structure and presence

    /// A keyed node appears. On `Mount::Animated` it enters with its transition's insertion
    /// effect; on `Mount::Initial` (or under an entering parent, which it rides) it just appears.
    /// If the key is still exiting, `Mount::Animated` revives it from exactly where it is and
    /// `Mount::Initial` lands it present, at rest on `spec.frame`.
    pub fn insert(&mut self, id: NodeId, spec: NodeSpec, mount: Mount) {
        if let Some(i) = self.slot(id) {
            self.nodes[i].transition = spec.transition;
            let exiting = matches!(self.nodes[i].phase, Phase::Exiting { .. });
            if exiting && mount == Mount::Initial {
                self.present_now(i);
                // Also lands motion toward an unchanged frame and ends screen travel.
                self.set_frame(id, spec.frame, LayoutAnim::INSTANT);
                return;
            }
            if exiting {
                self.revive(i);
            }
            if self.nodes[i].frame != spec.frame && self.nodes[i].path.is_none() {
                self.set_frame(id, spec.frame, LayoutAnim::new(crate::tokens::REFLOW));
            }
            return;
        }
        let parent = match spec.parent {
            Some(p) => self.slot(p).unwrap_or_else(|| panic!("insert {id:?}: parent {p:?} isn't in the scene")) as u32,
            None => NONE,
        };
        let rides_parent = parent != NONE && self.nodes[parent as usize].phase != Phase::Present;
        let animate = mount == Mount::Animated && !rides_parent;
        let ins = spec.transition.insertion.kit_rules(false);
        let node = Node {
            id,
            parent,
            first_child: NONE,
            last_child: NONE,
            next_sibling: NONE,
            prev_sibling: NONE,
            alive: true,
            frame: spec.frame,
            opacity: spec.opacity,
            radius: spec.radius,
            fill: spec.fill,
            transition: spec.transition,
            phase: if animate { Phase::Entering } else { Phase::Present },
            generation: 0,
            fx_a: fx_of(&ins),
            fx_b: FX_ID,
            anchor: ins.anchor,
            reveal: ins.reveal,
            path: None,
            tracks: [None; B],
            dirty: true,
            world: Rect::ZERO,
            origin: Vec2::ZERO,
            last: None,
        };
        let i = match self.free.pop() {
            Some(i) => {
                self.nodes[i as usize] = node;
                i
            }
            None => {
                self.nodes.push(node);
                (self.nodes.len() - 1) as u32
            }
        };
        // Link as the last child (siblings keep insertion order).
        let tail = if parent == NONE { self.root_tail } else { self.nodes[parent as usize].last_child };
        self.nodes[i as usize].prev_sibling = tail;
        if tail == NONE {
            if parent == NONE { self.root_head = i; } else { self.nodes[parent as usize].first_child = i; }
        } else {
            self.nodes[tail as usize].next_sibling = i;
        }
        if parent == NONE { self.root_tail = i; } else { self.nodes[parent as usize].last_child = i; }
        self.index.insert(id.0, i);
        self.order_dirty = true;
        let enter = spec.transition.springs.enter;
        if animate {
            self.track_set(i as usize, prop::PRESENCE, 0.0);
            self.track_animate(i as usize, prop::PRESENCE, 1.0, Anim::from(enter));
            self.intent(id, Intent::TRANSITION | Intent::OPACITY, enter.duration);
        } else {
            self.track_set(i as usize, prop::PRESENCE, 1.0);
        }
    }

    /// The app removed a keyed node. It stays drawn, plays its removal effect on the exit spring,
    /// and is reported in `exited` only once its presence has settled at 0 (opacity 0).
    pub fn remove(&mut self, id: NodeId) {
        let Some(i) = self.slot(id) else { return };
        if matches!(self.nodes[i].phase, Phase::Exiting { .. }) {
            return;
        }
        let p = self.presence_now(i);
        let rem = self.nodes[i].transition.removal.kit_rules(true);
        let target = fx_of(&rem);
        let n = &mut self.nodes[i];
        n.generation += 1;
        n.phase = Phase::Exiting { generation: n.generation };
        if p >= 0.999 {
            n.fx_a = target;
            n.fx_b = FX_ID;
        } else {
            // Mid-entrance: keep the current look at p, land exactly on the removal effect at 0.
            let cur = lerp_fx(&n.fx_a, &n.fx_b, p);
            n.fx_a = target;
            for k in 0..8 {
                n.fx_b[k] = target[k] + (cur[k] - target[k]) / p.max(1e-3);
            }
        }
        if rem.scale != 1.0 {
            n.anchor = rem.anchor;
        }
        if rem.reveal.is_some() {
            n.reveal = rem.reveal;
        }
        n.dirty = true;
        let exit = n.transition.springs.exit;
        self.track_animate(i, prop::PRESENCE, 0.0, Anim::from(exit));
        self.intent(id, Intent::TRANSITION | Intent::OPACITY, exit.duration);
    }

    /// The app removed a keyed node with no exit: it and its subtree unmount now and are reported
    /// in the next frame's `exited`.
    pub fn unmount(&mut self, id: NodeId) {
        if let Some(i) = self.slot(id) {
            self.remove_now(i);
        }
    }

    fn revive(&mut self, i: usize) {
        let p = self.presence_now(i);
        let n = &mut self.nodes[i];
        n.generation += 1;
        n.phase = Phase::Entering;
        if p <= 0.001 {
            let ins = n.transition.insertion.kit_rules(false);
            n.fx_a = fx_of(&ins);
        } else {
            // Keep the current look at p, land exactly on identity at 1.
            let cur = lerp_fx(&n.fx_a, &n.fx_b, p);
            let q = (1.0 - p).max(1e-3);
            for k in 0..8 {
                n.fx_a[k] = (cur[k] - FX_ID[k] * p) / q;
            }
        }
        n.fx_b = FX_ID;
        n.dirty = true;
        let enter = n.transition.springs.enter;
        let id = n.id;
        self.track_animate(i, prop::PRESENCE, 1.0, Anim::from(enter));
        self.intent(id, Intent::TRANSITION | Intent::OPACITY, enter.duration);
    }

    /// An exiting node is present again at once: no presence effect, nothing left to settle.
    fn present_now(&mut self, i: usize) {
        let n = &mut self.nodes[i];
        n.generation += 1;
        n.phase = Phase::Present;
        let ins = n.transition.insertion.kit_rules(false);
        n.fx_a = fx_of(&ins);
        n.fx_b = FX_ID;
        n.anchor = ins.anchor;
        n.reveal = ins.reveal;
        self.track_set(i, prop::PRESENCE, 1.0);
    }

    fn presence_now(&self, i: usize) -> f32 {
        let mut x = [0.0; MAX_LANES];
        let mut v = [0.0; MAX_LANES];
        match self.nodes[i].tracks[prop::PRESENCE.id.0 as usize] {
            Some(t) if self.store.sample_exact(t, &mut x, &mut v) => x[0],
            _ => 1.0,
        }
    }

    // ------------------------------------------------------------------------------------------
    // Layout

    /// The node's layout frame (parent space) changed. Each moving edge animates; a stationary
    /// edge gets no motion at all. With `Travel::Screen` the node takes one straight root-space
    /// path instead, whatever its ancestors do.
    pub fn set_frame(&mut self, id: NodeId, frame: Rect, anim: impl Into<LayoutAnim>) {
        let Some(i) = self.slot(id) else { return };
        let anim = anim.into();
        let old = self.nodes[i].frame;
        let had_path = self.nodes[i].path.is_some();
        #[cfg(all(feature = "dev", debug_assertions))]
        if self.store.retune_springs && !had_path && let Some(track) = self.nodes[i].tracks[prop::FRAME.id.0 as usize] {
            self.store.retune_raw(track.index_usize(), &[anim.x.curve, anim.y.curve, anim.x.curve, anim.y.curve]);
        }
        // An explicit landing also cancels motion toward an unchanged target. Comparing only
        // target frames here used to leave a spring running after Runtime::settle_all().
        if anim == LayoutAnim::INSTANT && (had_path || self.store.is_animating(id, prop::FRAME)) {
            self.end_path(i);
            self.nodes[i].frame = frame;
            self.track_set(i, prop::FRAME, frame);
            self.nodes[i].dirty = true;
            return;
        }
        if old == frame && !had_path {
            return;
        }
        let ms = match anim.x.curve {
            Curve::Spring(s) => s.duration,
            c => c.logical_duration(),
        };
        self.intent(id, Intent::POSITION | Intent::SIZE, ms);
        if anim.travel == Travel::Screen && !self.store.reduced_motion() {
            let (start, vel) = self.exact_world(i);
            self.nodes[i].frame = frame;
            self.start_path(i, start, vel, anim);
            return;
        }
        if had_path {
            // Leave screen travel: continue in parent space from the exact current place.
            let (w, wv) = self.exact_world(i);
            let (po, pv) = self.parent_origin_exact(i);
            let local = Rect::from_edges(w[0] - po.x, w[1] - po.y, w[2] - po.x, w[3] - po.y);
            let lv = [wv[0] - pv.x, wv[1] - pv.y, wv[2] - pv.x, wv[3] - pv.y];
            self.end_path(i);
            self.track_set(i, prop::FRAME, local);
            let t = self.nodes[i].tracks[prop::FRAME.id.0 as usize].unwrap();
            // Give the lanes their velocity, then retarget per axis.
            let tid = t.index_usize();
            self.store.animate_raw(tid, &local.edges(), Anim::new(Curve::Spring(Spring::SMOOTH)), Some(&lv));
        } else if self.nodes[i].tracks[prop::FRAME.id.0 as usize].is_none() {
            self.track_set(i, prop::FRAME, old);
        }
        self.nodes[i].frame = frame;
        let t = self.nodes[i].tracks[prop::FRAME.id.0 as usize].unwrap().index_usize();
        let mut cur = self.store.lane_targets_4(t);
        let e = frame.edges();
        if let Some((lead, trail)) = anim.lead {
            for axis in 0..2 {
                let (a, b) = (axis, axis + 2);
                let (sa, sb) = if e[a] + e[b] > cur[a] + cur[b] { (trail, lead) } else { (lead, trail) };
                let mut tg = cur;
                tg[a] = e[a];
                self.store.animate_raw(t, &tg, sa, None);
                cur[a] = e[a];
                let mut tg = cur;
                tg[b] = e[b];
                self.store.animate_raw(t, &tg, sb, None);
                cur[b] = e[b];
            }
        } else {
            // X edges on the X curve, Y edges on the Y curve; unchanged lanes are skipped.
            self.store.animate_raw(t, &[e[0], cur[1], e[2], cur[3]], anim.x, None);
            self.store.animate_raw(t, &e, anim.y, None);
        }
        self.nodes[i].dirty = true;
    }

    /// Hands one node's presented geometry, corner radius and fill to another, which then morphs
    /// to its own layout (SwiftUI `matchedGeometryEffect`, Motion `layoutId`). `from` disappears
    /// at once; `to` starts exactly where `from` was seen, with its velocity, and travels a
    /// straight root-space path. Under Reduce Motion it's a crossfade in place.
    pub fn match_geometry(&mut self, from: NodeId, to: NodeId, anim: impl Into<LayoutAnim>) {
        let (Some(a), Some(b)) = (self.slot(from), self.slot(to)) else { return };
        let anim = anim.into();
        if self.store.reduced_motion() {
            self.remove(from);
            let enter = self.nodes[b].transition.springs.enter;
            self.track_set(b, prop::PRESENCE, 0.0);
            self.track_animate(b, prop::PRESENCE, 1.0, Anim::from(enter));
            self.nodes[b].phase = Phase::Entering;
            return;
        }
        let (start, vel) = self.exact_world(a);
        let radius = self.current::<Corners>(a, prop::RADIUS).unwrap_or(self.nodes[a].radius);
        let fill = self.current::<Color>(a, prop::FILL).unwrap_or(self.nodes[a].fill);
        let (to_radius, to_fill) = (self.nodes[b].radius, self.nodes[b].fill);
        // `to` is the one surface from now on: fully present, no presence effect.
        self.track_set(b, prop::PRESENCE, 1.0);
        self.nodes[b].phase = Phase::Present;
        self.nodes[b].fx_a = FX_ID;
        self.start_path(b, start, vel, anim);
        self.track_set(b, prop::RADIUS, radius);
        self.track_animate(b, prop::RADIUS, to_radius, anim.x);
        self.track_set(b, prop::FILL, fill);
        self.track_animate(b, prop::FILL, to_fill, anim.y);
        self.intent(to, Intent::POSITION | Intent::SIZE | Intent::SHAPE, anim.y.curve.logical_duration());
        self.remove_now(a);
    }

    fn start_path(&mut self, i: usize, start: [f32; 4], vel: [f32; 4], anim: LayoutAnim) {
        let end_local = self.nodes[i].frame;
        let po = self.parent_target_origin(i);
        let end = end_local.translated(po).edges();
        // Project the current edge velocity onto each axis' path to keep it moving the same way.
        let px = axis_progress_velocity(start[0], start[2], end[0], end[2], vel[0], vel[2]);
        let py = axis_progress_velocity(start[1], start[3], end[1], end[3], vel[1], vel[3]);
        self.nodes[i].path = Some(Path { start: Rect::from_edges(start[0], start[1], start[2], start[3]), end_local });
        if !self.travelling.contains(&(i as u32)) {
            self.travelling.push(i as u32);
        }
        self.track_set(i, prop::TRAVEL, Vec2::ZERO);
        let t = self.nodes[i].tracks[prop::TRAVEL.id.0 as usize].unwrap().index_usize();
        self.store.animate_raw(t, &[1.0, 0.0], anim.x, Some(&[px, 0.0]));
        self.store.animate_raw(t, &[1.0, 1.0], anim.y, Some(&[px, py]));
        if self.nodes[i].tracks[prop::FRAME.id.0 as usize].is_some() {
            self.track_set(i, prop::FRAME, end_local);
        }
        self.nodes[i].dirty = true;
    }

    fn end_path(&mut self, i: usize) {
        self.nodes[i].path = None;
        self.travelling.retain(|&x| x as usize != i);
        let id = self.nodes[i].id;
        self.store.remove(id, prop::TRAVEL);
        self.nodes[i].tracks[prop::TRAVEL.id.0 as usize] = None;
    }

    fn parent_target_origin(&self, i: usize) -> Vec2 {
        let mut o = Vec2::ZERO;
        let mut p = self.nodes[i].parent;
        while p != NONE {
            let f = self.nodes[p as usize].frame;
            o.x += f.x;
            o.y += f.y;
            p = self.nodes[p as usize].parent;
        }
        o
    }

    /// Root-space layout edges of node `i` at `now()`, with velocities, sampled exactly.
    fn exact_world(&self, i: usize) -> ([f32; 4], [f32; 4]) {
        let e = self.world_dual(i);
        (e.map(|d| d.x), e.map(|d| d.v))
    }

    /// Trace-only check of presented scene edges against unsnapped analytic tracks at `now`.
    /// Returns (edge samples, maximum device-pixel error). Renderer-owned growth/scroll and
    /// post-layout scale transforms are outside this metric.
    pub fn sampling_error(&self) -> (usize, f32) {
        let mut count = 0;
        let mut error = 0.0f32;
        for &slot in &self.order {
            let i = slot as usize;
            let Some(p) = self.nodes[i].last else { continue };
            if !self.nodes[i].alive { continue; }
            let w = self.world_dual(i);
            let o = self.origin_dual(i);
            let ideal = [o[0].x, o[1].x, o[0].x + w[2].x - w[0].x, o[1].x + w[3].x - w[1].x];
            for (shown, exact) in p.rect.edges().into_iter().zip(ideal) {
                error = error.max((shown - exact).abs() * self.store.pixel_scale());
                count += 1;
            }
        }
        (count, error)
    }

    fn parent_origin_exact(&self, i: usize) -> (Vec2, Vec2) {
        let p = self.nodes[i].parent;
        if p == NONE {
            return (Vec2::ZERO, Vec2::ZERO);
        }
        let o = self.origin_dual(p as usize);
        (Vec2::new(o[0].x, o[1].x), Vec2::new(o[0].v, o[1].v))
    }

    fn lanes_dual(&self, i: usize, p: PropId, out: &mut [D]) -> bool {
        let Some(t) = self.nodes[i].tracks[p.0 as usize] else { return false };
        let mut x = [0.0; MAX_LANES];
        let mut v = [0.0; MAX_LANES];
        if !self.store.sample_exact(t, &mut x, &mut v) {
            return false;
        }
        for (k, o) in out.iter_mut().enumerate() {
            *o = D { x: x[k], v: v[k] };
        }
        true
    }

    fn world_dual(&self, i: usize) -> [D; 4] {
        let n = &self.nodes[i];
        if let Some(path) = n.path {
            let mut p = [D::c(1.0); 2];
            self.lanes_dual(i, prop::TRAVEL.id, &mut p);
            let s = path.start.edges();
            let end = path.end_local.translated(self.parent_target_origin(i)).edges();
            return core::array::from_fn(|k| {
                let a = D::c(s[k]);
                a.add(D::c(end[k] - s[k]).mul(p[k % 2]))
            });
        }
        let mut local = n.frame.edges().map(D::c);
        self.lanes_dual(i, prop::FRAME.id, &mut local);
        if n.parent == NONE {
            return local;
        }
        let o = self.origin_dual(n.parent as usize);
        core::array::from_fn(|k| local[k].add(o[k % 2]))
    }

    /// The origin children of `i` ride: layout origin plus offsets (own and presence).
    fn origin_dual(&self, i: usize) -> [D; 2] {
        let w = self.world_dual(i);
        let mut off = [D::c(0.0); 2];
        self.lanes_dual(i, prop::OFFSET.id, &mut off);
        let mut p = [D::c(1.0)];
        self.lanes_dual(i, prop::PRESENCE.id, &mut p);
        let n = &self.nodes[i];
        let fx = |k: usize| D::c(n.fx_a[k]).mul(D::c(1.0).sub(p[0])).add(D::c(n.fx_b[k]).mul(p[0]));
        let size = [w[2].sub(w[0]), w[3].sub(w[1])];
        let reduced = self.store.reduced_motion();
        core::array::from_fn(|a| {
            let mut o = w[a].add(off[a]);
            if !reduced {
                o = o.add(fx(2 + a)).add(fx(4 + a).mul(size[a]));
            }
            o
        })
    }

    // ------------------------------------------------------------------------------------------
    // Node properties

    /// Animates one of the node's properties (`prop::OPACITY`, `prop::RADIUS`, …, or a custom one).
    /// The first animation starts from the node's base value, so `.opacity(1)` → `animate(0)`
    /// fades.
    pub fn animate<T: Animatable>(&mut self, id: NodeId, prop: Prop<T>, target: T, anim: impl Into<Anim>) {
        let Some(i) = self.slot(id) else { return };
        let anim = anim.into();
        self.ensure_prop(i, prop);
        #[cfg(all(feature = "dev", debug_assertions))]
        if self.store.retune_springs && let Some(track) = self.nodes[i].tracks.get(prop.id.0 as usize).copied().flatten() {
            self.store.retune_raw(track.index_usize(), &[anim.curve; MAX_LANES][..T::LANES]);
        }
        self.track_animate(i, prop, target, anim);
        let bits = Intent::for_prop(prop.id);
        self.intent(id, bits, anim.curve.logical_duration());
    }

    /// Sets a property immediately (direct manipulation).
    pub fn set<T: Animatable>(&mut self, id: NodeId, prop: Prop<T>, value: T) {
        if let Some(i) = self.slot(id) {
            self.track_set(i, prop, value);
        }
    }

    /// The property's presented value (last frame), falling back to the node's base value.
    pub fn get<T: Animatable>(&self, id: NodeId, prop: Prop<T>) -> Option<T> {
        let i = self.slot(id)?;
        self.store.get(id, prop).or_else(|| self.base::<T>(i, prop.id))
    }

    fn ensure_prop<T: Animatable>(&mut self, i: usize, p: Prop<T>) {
        if self.nodes[i].tracks.get(p.id.0 as usize).copied().flatten().is_none()
            && !self.store.has(self.nodes[i].id, p)
            && let Some(base) = self.base::<T>(i, p.id)
        {
            self.track_set(i, p, base);
        }
    }

    fn base<T: Animatable>(&self, i: usize, p: PropId) -> Option<T> {
        let n = &self.nodes[i];
        let mut l = [0.0; MAX_LANES];
        match p {
            x if x == prop::OPACITY.id => l[0] = n.opacity,
            x if x == prop::SCALE.id => l[0] = 1.0,
            x if x == prop::FRAME.id => l[..4].copy_from_slice(&n.frame.edges()),
            x if x == prop::RADIUS.id => l[..4].copy_from_slice(&n.radius.0),
            x if x == prop::FILL.id => l = lanes(&n.fill),
            x if x == prop::OFFSET.id || x == prop::ROTATION.id || x == prop::BLUR.id || x == prop::MASK.id => {}
            _ => return None,
        }
        Some(T::read_lanes(&l[..T::LANES]))
    }

    fn current<T: Animatable>(&self, i: usize, p: Prop<T>) -> Option<T> {
        let t = self.nodes[i].tracks.get(p.id.0 as usize).copied().flatten()?;
        let mut x = [0.0; MAX_LANES];
        let mut v = [0.0; MAX_LANES];
        self.store.sample_exact(t, &mut x, &mut v).then(|| T::read_lanes(&x[..T::LANES]))
    }

    fn track_set<T: Animatable>(&mut self, i: usize, p: Prop<T>, value: T) {
        let id = self.store.set(self.nodes[i].id, p, value);
        if let Some(slot) = self.nodes[i].tracks.get_mut(p.id.0 as usize) {
            *slot = Some(id);
        }
        self.nodes[i].dirty = true;
    }

    fn track_animate<T: Animatable>(&mut self, i: usize, p: Prop<T>, target: T, anim: Anim) {
        let id = self.store.animate(self.nodes[i].id, p, target, anim);
        if let Some(slot) = self.nodes[i].tracks.get_mut(p.id.0 as usize) {
            *slot = Some(id);
        }
        self.nodes[i].dirty = true;
    }

    fn intent(&mut self, id: NodeId, bits: u32, seconds: f32) {
        if self.trace.is_some() {
            self.intents.push((id, bits, seconds));
        }
    }

    // ------------------------------------------------------------------------------------------
    // The frame

    /// `set_time(t)`, then [`frame`](Self::frame).
    pub fn frame_at(&mut self, t: f64) -> FrameOutput<'_> {
        self.set_time(t);
        self.frame()
    }

    /// Samples every active track at `now()`, finishes exits, and returns the presented values of
    /// every node that changed. Apply this frame's commits (insert/remove/set_frame/animate)
    /// between `set_time` and `frame`.
    pub fn frame(&mut self) -> FrameOutput<'_> {
        self.store.tick();
        // Keep removals made since the last frame (matched geometry), drop last frame's.
        self.exited.drain(..self.exited_seen);
        self.to_remove.clear();

        // Presence settles: finish entrances, retire exits whose generation is current.
        for ev in self.store.events() {
            if let MotionEvent::Settled { node, prop: p, .. } = *ev {
                if p != prop::PRESENCE.id {
                    continue;
                }
                let Some(&i) = self.index.get(&node.0) else { continue };
                let n = &mut self.nodes[i as usize];
                match n.phase {
                    Phase::Entering => n.phase = Phase::Present,
                    Phase::Exiting { generation } if generation == n.generation => self.to_remove.push(i),
                    _ => {}
                }
            }
        }
        // Mark what moved; refresh the track cache from the store.
        for (tid, node, p) in self.store.touched_tracks() {
            if let Some(&i) = self.index.get(&node.0) {
                let n = &mut self.nodes[i as usize];
                n.dirty = true;
                if (p.0 as usize) < B {
                    n.tracks[p.0 as usize] = Some(tid);
                }
            }
        }
        // Screen travel ends once it has arrived and no ancestor is still moving.
        let mut k = 0;
        while k < self.travelling.len() {
            let i = self.travelling[k] as usize;
            let travel = self.nodes[i].tracks[prop::TRAVEL.id.0 as usize];
            let arrived = travel.is_none_or(|t| !self.store.is_active(t));
            if arrived && !self.ancestor_moving(i) {
                let end_local = self.nodes[i].path.map(|p| p.end_local).unwrap_or(self.nodes[i].frame);
                self.end_path(i);
                self.track_set(i, prop::FRAME, end_local);
                continue;
            }
            k += 1;
        }
        if self.order_dirty {
            self.rebuild_order();
        }
        self.out.clear();
        let scale = self.store.pixel_scale();
        let reduced = self.store.reduced_motion();
        for k in 0..self.order.len() {
            let i = self.order[k] as usize;
            let parent = self.nodes[i].parent;
            let parent_dirty = parent != NONE && self.nodes[parent as usize].dirty;
            let own = self.nodes[i].dirty;
            if parent_dirty {
                self.nodes[i].dirty = true;
            }
            if self.nodes[i].dirty {
                let p = self.compute(i, scale, reduced, own, parent_dirty);
                self.nodes[i].last = Some(p);
                self.out.push(p);
            }
        }
        for k in 0..self.order.len() {
            let i = self.order[k] as usize;
            self.nodes[i].dirty = false;
        }
        for k in 0..self.to_remove.len() {
            let i = self.to_remove[k] as usize;
            if self.nodes[i].alive {
                self.remove_now(i);
            }
        }
        self.exited_seen = self.exited.len();
        let animating = self.store.active_len() > 0 || !self.travelling.is_empty();
        if let Some(trace) = self.trace.as_mut() {
            trace.frame(self.store.now(), &self.out, &self.exited, self.store.events(), &self.intents);
            self.intents.clear();
        }
        FrameOutput { changed: &self.out, exited: &self.exited, events: self.store.events(), animating }
    }

    fn ancestor_moving(&self, i: usize) -> bool {
        let mut p = self.nodes[i].parent;
        while p != NONE {
            let n = &self.nodes[p as usize];
            for prop in [prop::FRAME.id, prop::TRAVEL.id, prop::OFFSET.id] {
                if n.tracks[prop.0 as usize].is_some_and(|t| self.store.is_active(t)) {
                    return true;
                }
            }
            p = n.parent;
        }
        false
    }

    fn values(&self, i: usize, p: PropId) -> Option<&[f32]> {
        self.nodes[i].tracks[p.0 as usize].and_then(|t| self.store.values_of(t))
    }

    fn compute(&mut self, i: usize, scale: f32, reduced: bool, own_dirty: bool, parent_dirty: bool) -> Presented {
        let parent = self.nodes[i].parent;
        let parent_origin = if parent == NONE { Vec2::ZERO } else { self.nodes[parent as usize].origin };
        let n = &self.nodes[i];
        let world = match n.path {
            Some(path) => {
                let p = self.values(i, prop::TRAVEL.id).map_or([1.0, 1.0], |v| [v[0], v[1]]);
                let s = path.start.edges();
                let e = path.end_local.translated(self.parent_target_origin(i)).edges();
                let ed: [f32; 4] = core::array::from_fn(|k| s[k] + (e[k] - s[k]) * p[k % 2]);
                Rect::from_edges(ed[0], ed[1], ed[2], ed[3])
            }
            None => {
                let local = self.values(i, prop::FRAME.id).map_or(n.frame, Rect::read_lanes);
                local.translated(parent_origin)
            }
        };
        let p = self.values(i, prop::PRESENCE.id).map_or(1.0, |v| v[0]);
        let s = lerp_fx(&n.fx_a, &n.fx_b, p);
        let s = if reduced { [s[0], 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] } else { s };
        let own_opacity = self.values(i, prop::OPACITY.id).map_or(n.opacity, |v| v[0]);
        let own_scale = self.values(i, prop::SCALE.id).map_or(1.0, |v| v[0]);
        let own_offset = self.values(i, prop::OFFSET.id).map_or(Vec2::ZERO, Vec2::read_lanes);
        let rotation = self.values(i, prop::ROTATION.id).map_or(0.0, |v| v[0]);
        let own_blur = self.values(i, prop::BLUR.id).map_or(0.0, |v| v[0]);
        let radius = self.values(i, prop::RADIUS.id).map_or(n.radius, Corners::read_lanes);
        let fill = self.values(i, prop::FILL.id).map_or(n.fill, Color::read_lanes);
        let own_mask = self.values(i, prop::MASK.id).map_or(Sides::ZERO, Sides::read_lanes);

        let offset = Vec2::new(
            own_offset.x + s[2] + s[4] * world.w,
            own_offset.y + s[3] + s[5] * world.h,
        );
        let shown = world.translated(offset);
        let sc = (own_scale * s[1]).max(0.0);
        let mut blur = (own_blur + s[6]).max(0.0);
        if blur < 1.0 / 64.0 {
            blur = 0.0;
        }
        let mut mask = Mask { inset: own_mask, feather: Sides::ZERO };
        if let Some(rv) = n.reveal {
            let hidden = s[7].clamp(0.0, 1.0);
            if hidden > 0.0 {
                let (side, len) = match rv.edge {
                    crate::Edge::Leading => (0, shown.w),
                    crate::Edge::Top => (1, shown.h),
                    crate::Edge::Trailing => (2, shown.w),
                    crate::Edge::Bottom => (3, shown.h),
                };
                // The content is hidden behind `edge`: the opposite side's inset grows.
                let opposite = (side + 2) % 4;
                mask.inset.0[opposite] += hidden * len;
                // The ramp fades in with the wipe instead of popping (min(1, travel / feather)).
                mask.feather.0[opposite] = rv.feather * (hidden * len / rv.feather.max(1e-3)).min(1.0);
            }
        }
        let anchor = Vec2::new(shown.x + n.anchor.x * shown.w, shown.y + n.anchor.y * shown.h);
        let mut fl = 0u32;
        let mut own_bits = 0u32;
        for (k, t) in n.tracks.iter().enumerate() {
            if t.is_some_and(|t| self.store.is_active(t)) {
                own_bits |= Intent::for_prop(PropId(k as u16));
            }
        }
        if n.path.is_some() {
            own_bits |= Intent::POSITION | Intent::SIZE;
        }
        if own_bits != 0 || (own_dirty && !parent_dirty) {
            fl |= flags::ANIMATED;
        }
        if parent_dirty {
            fl |= flags::ANCESTOR_ANIMATED;
            let pb = self.nodes[parent as usize].last.map_or(0, |p| (p.flags >> 8) & 0xff | (p.flags >> 16) & 0xff);
            fl |= pb << 16;
        }
        fl |= own_bits << 8;
        if !mask.is_none() {
            fl |= flags::CLIPPED;
        }
        if blur > 0.0 {
            fl |= flags::FILTERED;
        }
        let in_flow = !matches!(n.phase, Phase::Exiting { .. }) || !n.transition.pop_layout;
        let out = Presented {
            node: n.id,
            parent: (parent != NONE).then(|| self.nodes[parent as usize].id),
            rect: snap_rect(shown, scale),
            opacity: (own_opacity * s[0]).clamp(0.0, 1.0),
            transform: Transform { translate: Vec2::ZERO, scale: Vec2::new(sc, sc), rotate: rotation },
            anchor,
            blur,
            radius,
            fill,
            mask,
            phase: n.phase,
            in_flow,
            flags: fl,
        };
        let n = &mut self.nodes[i];
        n.world = world;
        n.origin = Vec2::new(shown.x, shown.y);
        out
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
        self.travelling.retain(|&x| x as usize != i);
        self.store.remove_node(id);
        self.index.remove(&id.0);
        let n = &mut self.nodes[i];
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

/// Exact at both ends: `a` at p = 0, `b` at p = 1.
fn lerp_fx(a: &Fx, b: &Fx, p: f32) -> Fx {
    core::array::from_fn(|k| a[k] * (1.0 - p) + b[k] * p)
}

/// The progress velocity that best continues edge velocities `(v0, v1)` along the path from
/// `(s0, s1)` to `(e0, e1)`.
fn axis_progress_velocity(s0: f32, s1: f32, e0: f32, e1: f32, v0: f32, v1: f32) -> f32 {
    let (d0, d1) = (e0 - s0, e1 - s1);
    let len2 = d0 * d0 + d1 * d1;
    if len2 < 1e-6 { 0.0 } else { (v0 * d0 + v1 * d1) / len2 }
}
