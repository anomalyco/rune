//! Every sequence of presence events, walked against dev's own logic.
//!
//! The reference below is a literal transcription of the code this branch replaces: the fields
//! `Node` used to carry (`phase`, `generation`, `fx_a`, `fx_b`, `anchor`, `reveal`, `transition`)
//! and every branch in `insert_delayed`, `remove`, `revive`, `present_now`, `enter_now`,
//! `land_waiting` and `reenter` that wrote them. It never calls the scene's presence code.
//!
//! The side under test drives the real [`Scene`]; the reference is fed the facts it read out of
//! the store (the presence value, the settle event), and the two are compared after every event
//! and after every frame: the phase, whether the node is still in the scene, the exit the frame
//! reports, and the whole presented look (opacity, scale, rect, blur, draw, mask, anchor and
//! `in_flow`). A reordered write, a dropped early return or a look that jumps fails here.

use rune_motion::prelude::*;
use rune_motion::{Effect, MotionEvent, Phase, Reveal, prop};

const ID: NodeId = NodeId(1);
const DT: f64 = 1.0 / 120.0;

fn scene_frame() -> Rect {
    Rect::new(10.0, 20.0, 100.0, 40.0)
}

fn spec(transition: Transition) -> NodeSpec {
    NodeSpec::new(scene_frame()).transition(transition)
}

fn slow() -> SpringPair {
    SpringPair::new(Spring::new(0.12, 0.0), Spring::new(0.12, 0.0))
}

/// Covers opacity, scale, blur and the reveal mask.
fn blur_reveal(springs: SpringPair) -> Transition {
    Transition::blur_replace().combined(Transition::reveal(Edge::Leading)).springs(springs)
}

/// Covers opacity, the size-fraction offset and the drawn-shape lane.
fn push_draw(springs: SpringPair) -> Transition {
    Transition::push(Edge::Bottom).combined(Transition::draw()).springs(springs)
}

/// An entrance that slides in without fading: its insertion look is still opaque at presence 0.
fn slide(springs: SpringPair) -> Transition {
    Transition::move_edge(Edge::Leading).springs(springs)
}

/// A removal that scales about its top-left corner and wipes, so it pivots and hides somewhere
/// else than the insertion the node arrived on.
fn corner_scale(springs: SpringPair) -> Transition {
    let removal = Transition::scale_anchor(0.9, Vec2::new(0.0, 0.0)).combined(Transition::reveal(Edge::Top));
    Transition::asymmetric(Transition::move_edge(Edge::Leading), removal).springs(springs)
}

/// The two transitions a walk inserts and retransitions with, and the springs they asked for.
#[derive(Clone, Copy)]
struct Cfg {
    mount: Transition,
    retarget: Transition,
    /// The literal pair the transitions were built from. The reference runs its own presence on
    /// it, so a scene that reads a different duration from the same transition fails the walk.
    springs: SpringPair,
}

// ---------------------------------------------------------------------------------------------
// The reference: dev's fields and branches.

const IDENTITY: [f32; 9] = [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// dev's `Phase`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum DevPhase {
    Entering,
    Present,
    Exiting { generation: u32 },
}

fn lanes(e: &Effect) -> [f32; 9] {
    let hidden = if e.reveal.is_some() { 1.0 } else { 0.0 };
    [e.opacity, e.scale, e.offset.x, e.offset.y, e.offset_fraction.x, e.offset_fraction.y, e.blur, hidden, e.draw]
}

fn lerp(a: &[f32; 9], b: &[f32; 9], p: f32) -> [f32; 9] {
    core::array::from_fn(|k| a[k] * (1.0 - p) + b[k] * p)
}

/// The presence value at the scene's clock, which is what dev read with `presence_now`.
fn presence(scene: &Scene, node: NodeId) -> f32 {
    scene.store().sample(node, prop::PRESENCE).unwrap_or(1.0)
}

struct Dev {
    phase: DevPhase,
    generation: u32,
    fx_a: [f32; 9],
    fx_b: [f32; 9],
    anchor: Vec2,
    reveal: Option<Reveal>,
    transition: Transition,
    alive: bool,
    /// Removed, waiting for the next frame to report it in `exited`.
    pending_exit: bool,
    /// The presence track dev's own writes drive, on the same clock as the scene: the expected
    /// progress, settle and exit frames. A separate store, so a scene that runs the track on a
    /// different spring or for a different duration fails here instead of agreeing with itself.
    ref_store: AnimationStore,
    /// The springs a walk configured, which the reference's entrance and exit run on.
    springs: SpringPair,
}

impl Dev {
    fn new(transition: Transition, springs: SpringPair) -> Dev {
        let mut ref_store = AnimationStore::new();
        ref_store.set(ID, prop::PRESENCE, 1.0);
        Dev {
            phase: DevPhase::Present,
            generation: 0,
            fx_a: IDENTITY,
            fx_b: IDENTITY,
            anchor: Vec2::new(0.5, 0.5),
            reveal: None,
            transition,
            alive: false,
            pending_exit: false,
            ref_store,
            springs,
        }
    }

    /// The presence the reference's own track holds at the scene's clock.
    fn ref_presence(&self) -> f32 {
        self.ref_store.sample(ID, prop::PRESENCE).unwrap_or(1.0)
    }

    /// `insert_delayed`, with the node's frame unchanged: a new node, a revive, or a landing.
    fn insert(&mut self, transition: Transition, mount: Mount, delay: f32, p: f32) {
        self.transition = transition;
        if self.alive {
            let exiting = matches!(self.phase, DevPhase::Exiting { .. });
            if exiting && mount == Mount::Initial {
                self.present_now();
                return;
            }
            if exiting {
                self.revive(p);
            }
            return;
        }
        let ins = transition.insertion.kit_rules(false);
        // A root node has no parent to ride, so `rides_parent` is false.
        let animate = mount == Mount::Animated && !ins.shows_nothing();
        self.phase = if animate { DevPhase::Entering } else { DevPhase::Present };
        self.generation = 0;
        self.fx_a = lanes(&ins);
        self.fx_b = IDENTITY;
        self.anchor = ins.anchor;
        self.reveal = ins.reveal;
        self.alive = true;
        if animate {
            self.land(0.0);
            self.run(1.0, Anim { delay, ..Anim::from(self.springs.enter) });
        } else {
            self.land(1.0);
        }
    }

    fn set_transition(&mut self, transition: Transition) {
        if !matches!(self.phase, DevPhase::Exiting { .. }) {
            self.transition = transition;
        }
    }

    fn remove(&mut self, p: f32) {
        if !self.alive || matches!(self.phase, DevPhase::Exiting { .. }) {
            return;
        }
        let unplayed = p < 1e-3;
        if unplayed && lerp(&self.fx_a, &self.fx_b, p)[0] < 1.0 / 512.0 {
            // An entrance that hasn't shown anything yet has no exit to play.
            self.retire();
            return;
        }
        let rem = self.transition.removal.kit_rules(true);
        let target = lanes(&rem);
        self.generation += 1;
        self.phase = DevPhase::Exiting { generation: self.generation };
        if p >= 0.999 {
            self.fx_a = target;
            self.fx_b = IDENTITY;
        } else if unplayed {
            self.fx_b = lerp(&self.fx_a, &self.fx_b, p);
            self.fx_a = target;
        } else {
            let cur = lerp(&self.fx_a, &self.fx_b, p);
            self.fx_a = target;
            for k in 0..8 {
                self.fx_b[k] = target[k] + (cur[k] - target[k]) / p.max(1e-3);
            }
        }
        if rem.scale != 1.0 {
            self.anchor = rem.anchor;
        }
        if rem.reveal.is_some() {
            self.reveal = rem.reveal;
        }
        if unplayed {
            self.land(1.0);
        }
        self.run(0.0, Anim::from(self.springs.exit));
    }

    fn unmount(&mut self) {
        if self.alive {
            self.retire();
        }
    }

    fn enter_now(&mut self) {
        if self.phase == DevPhase::Entering {
            self.present_now();
        }
    }

    fn land_waiting(&mut self, p: f32) {
        if self.phase == DevPhase::Entering && p < 1e-3 {
            self.present_now();
        }
    }

    fn reenter(&mut self) {
        if self.alive && self.phase == DevPhase::Present {
            // `reenter` lands presence at 0, then revives from there.
            self.land(0.0);
            self.revive(0.0);
        }
    }

    fn revive(&mut self, p: f32) {
        self.generation += 1;
        self.phase = DevPhase::Entering;
        if p <= 0.001 {
            let ins = self.transition.insertion.kit_rules(false);
            self.fx_a = lanes(&ins);
        } else {
            // Keep the current look at p, land exactly on identity at 1. The last lane (drawn
            // shape) is kept: an entrance never re-solves it.
            let cur = lerp(&self.fx_a, &self.fx_b, p);
            let q = (1.0 - p).max(1e-3);
            for k in 0..8 {
                self.fx_a[k] = (cur[k] - IDENTITY[k] * p) / q;
            }
        }
        self.fx_b = IDENTITY;
        // A revive runs the entrance with no delay, whatever the mount was held for.
        self.run(1.0, Anim::from(self.springs.enter));
    }

    fn present_now(&mut self) {
        self.generation += 1;
        self.phase = DevPhase::Present;
        let ins = self.transition.insertion.kit_rules(false);
        self.fx_a = lanes(&ins);
        self.fx_b = IDENTITY;
        self.anchor = ins.anchor;
        self.reveal = ins.reveal;
        self.land(1.0);
    }

    /// The reference's `Land`: presence jumps, whatever it was doing.
    fn land(&mut self, v: f32) {
        self.ref_store.set(ID, prop::PRESENCE, v);
    }

    /// The reference's `Run`: presence animates from where the reference's own track is.
    fn run(&mut self, target: f32, anim: Anim) {
        self.ref_store.animate(ID, prop::PRESENCE, target, anim);
    }

    /// Moves the reference's store to `t` and samples it: the presence and the settle event the
    /// walk expects the scene to have this frame.
    fn reference_frame(&mut self, t: f64) -> (f32, bool) {
        self.ref_store.set_time(t);
        let p = self.ref_presence();
        self.ref_store.tick();
        let settled = self.ref_store.events().iter().any(|e| {
            matches!(*e, MotionEvent::Settled { node: n, prop: q, .. } if n == ID && q == prop::PRESENCE.id)
        });
        (p, settled)
    }

    fn retire(&mut self) {
        self.alive = false;
        self.pending_exit = true;
    }

    /// `match_geometry`'s `to`: present at once, with only the absent look replaced.
    fn matched(&mut self) {
        self.phase = DevPhase::Present;
        self.fx_a = IDENTITY;
        self.land(1.0);
    }

    fn looks(&self) -> ([f32; 9], [f32; 9]) {
        (self.fx_a, self.fx_b)
    }

    /// One frame's worth of dev's `Scene::frame`: presence settles, then an exit whose presence
    /// faded below one 8-bit step retires. `p` is the presence the frame's tick had, which is the
    /// value dev read from the store. Returns whether it reports the node in `exited`.
    fn frame(&mut self, settled: bool, p: f32) -> bool {
        let mut retired = false;
        if settled {
            match self.phase {
                DevPhase::Entering => self.phase = DevPhase::Present,
                DevPhase::Exiting { generation } if generation == self.generation => {
                    self.retire();
                    retired = true;
                }
                _ => {}
            }
        }
        if self.alive && matches!(self.phase, DevPhase::Exiting { .. }) {
            if self.fx_a[0] + (self.fx_b[0] - self.fx_a[0]) * p < 1.0 / 255.0 {
                self.retire();
                retired = true;
            }
        }
        let reported = retired || self.pending_exit;
        self.pending_exit = false;
        reported
    }
}

// ---------------------------------------------------------------------------------------------
// The walk.

#[derive(Clone, Copy, Debug)]
enum Step {
    Insert { mount: Mount, delay: f32 },
    Retransition,
    Remove,
    Reenter,
    EnterNow,
    LandWaiting,
    Unmount,
    Tick,
}

fn run(cfg: Cfg, steps: &[Step]) {
    let mut scene = Scene::new();
    let mut dev = Dev::new(cfg.mount, cfg.springs);
    let mut t = 0.0;
    for step in steps {
        // The presence the store has shown so far, which is what dev read before changing
        // anything: read before the call, for the same reason.
        let p = presence(&scene, ID);
        match *step {
            Step::Insert { mount, delay } => {
                if delay > 0.0 {
                    scene.insert_delayed(ID, spec(cfg.mount), mount, delay);
                } else {
                    scene.insert(ID, spec(cfg.mount), mount);
                }
                dev.insert(cfg.mount, mount, delay, p);
            }
            Step::Retransition => {
                scene.set_transition(ID, cfg.retarget);
                dev.set_transition(cfg.retarget);
            }
            Step::Remove => {
                scene.remove(ID);
                dev.remove(p);
            }
            Step::Reenter => {
                scene.reenter(ID);
                dev.reenter();
            }
            Step::EnterNow => {
                scene.enter_now(ID);
                dev.enter_now();
            }
            Step::LandWaiting => {
                scene.land_waiting(ID);
                dev.land_waiting(p);
            }
            Step::Unmount => {
                scene.unmount(ID);
                dev.unmount();
            }
            Step::Tick => {
                for _ in 0..24 {
                    tick(&mut scene, &mut dev, ID, scene_frame(), &mut t, true, steps);
                }
            }
        }
        check_state(&scene, &dev, steps);
    }
}

fn check_state(scene: &Scene, dev: &Dev, steps: &[Step]) {
    assert_eq!(scene.contains(ID), dev.alive, "in the scene after {steps:?}");
    if !dev.alive {
        assert_eq!(scene.phase(ID), None, "phase after {steps:?}");
        return;
    }
    let want = match dev.phase {
        DevPhase::Entering => Phase::Entering,
        DevPhase::Present => Phase::Present,
        DevPhase::Exiting { generation } => Phase::Exiting { generation },
    };
    assert_eq!(scene.phase(ID), Some(want), "phase after {steps:?}");
}

fn check_presented(p: &Presented, dev: &Dev, presence: f32, frame: Rect, steps: &[Step]) {
    let look = lerp(&dev.fx_a, &dev.fx_b, presence);
    let shown = Vec2::new(
        frame.x + look[2] + look[4] * frame.w,
        frame.y + look[3] + look[5] * frame.h,
    );
    let close = |got: f32, want: f32, tol: f32, what: &str| {
        assert!((got - want).abs() <= tol, "{what} {got} vs {want} after {steps:?}");
    };
    close(p.opacity, look[0].clamp(0.0, 1.0), 1e-6, "opacity");
    close(p.transform.scale.x, look[1], 1e-6, "scale");
    // Presented rects snap to 1/64 device px.
    close(p.rect.x, shown.x, 2.0 / 64.0, "rect.x");
    close(p.rect.y, shown.y, 2.0 / 64.0, "rect.y");
    let blur = if look[6].max(0.0) < 1.0 / 64.0 { 0.0 } else { look[6].max(0.0) };
    close(p.blur, blur, 1e-6, "blur");
    close(p.draw, look[8].clamp(0.0, 1.0), 1e-6, "draw");
    close(p.anchor.x, shown.x + dev.anchor.x * frame.w, 2.0 / 64.0, "anchor.x");
    close(p.anchor.y, shown.y + dev.anchor.y * frame.h, 2.0 / 64.0, "anchor.y");
    let (mut inset, mut feather) = ([0.0f32; 4], [0.0f32; 4]);
    if let Some(rv) = dev.reveal {
        let hidden = look[7].clamp(0.0, 1.0);
        if hidden > 0.0 {
            let (side, len) = match rv.edge {
                Edge::Leading => (0, frame.w),
                Edge::Top => (1, frame.h),
                Edge::Trailing => (2, frame.w),
                Edge::Bottom => (3, frame.h),
            };
            let opposite = (side + 2) % 4;
            inset[opposite] += hidden * len;
            feather[opposite] = rv.feather * (hidden * len / rv.feather.max(1e-3)).min(1.0);
        }
    }
    for k in 0..4 {
        close(p.mask.inset.0[k], inset[k], 1e-4, "mask inset");
        close(p.mask.feather.0[k], feather[k], 1e-4, "mask feather");
    }
    assert_eq!(
        p.in_flow,
        !matches!(dev.phase, DevPhase::Exiting { .. }) || !dev.transition.pop_layout,
        "in_flow after {steps:?}"
    );
}

fn enumerate(cfg: Cfg, alphabet: &[Step], depth: usize, path: &mut Vec<Step>) {
    if !path.is_empty() {
        run(cfg, path);
    }
    if path.len() == depth {
        return;
    }
    for step in alphabet {
        path.push(*step);
        enumerate(cfg, alphabet, depth, path);
        path.pop();
    }
}

/// Mounts, landings, retransitions, exits and revivals, every sequence up to five long.
#[test]
fn walk_entrances_and_exits() {
    let cfg = Cfg { mount: blur_reveal(slow()), retarget: push_draw(slow()), springs: slow() };
    let alphabet = [
        Step::Insert { mount: Mount::Animated, delay: 0.0 },
        Step::Insert { mount: Mount::Initial, delay: 0.0 },
        Step::Retransition,
        Step::Remove,
        Step::Reenter,
        Step::Tick,
    ];
    enumerate(cfg, &alphabet, 5, &mut Vec::new());
}

/// A held entrance, landed either way, and the paths that unmount without an exit.
#[test]
fn walk_held_entrances() {
    let cfg = Cfg { mount: blur_reveal(slow()), retarget: push_draw(slow()), springs: slow() };
    let alphabet = [
        Step::Insert { mount: Mount::Animated, delay: 0.05 },
        Step::EnterNow,
        Step::LandWaiting,
        Step::Remove,
        Step::Unmount,
        Step::Tick,
    ];
    enumerate(cfg, &alphabet, 5, &mut Vec::new());
}

/// A slide that doesn't fade: removed before it shows, it leaves from its insertion look instead
/// of unmounting unseen. Its retarget pivots the exit somewhere else.
#[test]
fn walk_visible_entrances_that_never_played() {
    let cfg = Cfg { mount: slide(slow()), retarget: corner_scale(slow()), springs: slow() };
    let alphabet = [
        Step::Insert { mount: Mount::Animated, delay: 0.0 },
        Step::Insert { mount: Mount::Initial, delay: 0.0 },
        Step::Retransition,
        Step::Remove,
        Step::Reenter,
        Step::Tick,
    ];
    enumerate(cfg, &alphabet, 5, &mut Vec::new());
}

/// Presence that lands at once, so an exit settles on the frame it starts.
#[test]
fn walk_instant_presence() {
    let cfg = Cfg { mount: blur_reveal(SpringPair::NONE), retarget: push_draw(SpringPair::NONE), springs: SpringPair::NONE };
    let alphabet = [
        Step::Insert { mount: Mount::Animated, delay: 0.0 },
        Step::Retransition,
        Step::Remove,
        Step::Reenter,
        Step::EnterNow,
        Step::Tick,
    ];
    enumerate(cfg, &alphabet, 5, &mut Vec::new());
}

/// `match_geometry` onto a node that is exiting keeps the look that exit was leaving from: dev
/// replaced only the absent look, so the exit's presence-1 look still shows at presence 1.
#[test]
fn matched_geometry_onto_an_exit_keeps_the_look_it_was_leaving_from() {
    let (from, to) = (NodeId(30), NodeId(1));
    let transition = push_draw(slow());
    let matched = |exit_first: bool| {
        let to_spec = NodeSpec::new(Rect::new(40.0, 20.0, 100.0, 40.0)).transition(transition);
        let mut scene = Scene::new();
        let mut dev = Dev::new(transition, slow());
        scene.insert(from, NodeSpec::new(Rect::new(0.0, 0.0, 20.0, 20.0)), Mount::Initial);
        scene.insert(to, to_spec, Mount::Animated);
        dev.insert(transition, Mount::Animated, 0.0, 1.0);
        let mut t = 0.0;
        advance(&mut scene, &mut dev, to, to_spec.frame, &mut t, 4, true);
        if exit_first {
            scene.remove(to);
            dev.remove(presence(&scene, to));
            advance(&mut scene, &mut dev, to, to_spec.frame, &mut t, 3, true);
        }
        scene.match_geometry(from, to, slow());
        dev.matched();
        // A screen path now places the node, so only the presence's own look is compared.
        advance(&mut scene, &mut dev, to, to_spec.frame, &mut t, 1, false);
        (scene.presented(to).unwrap(), dev)
    };
    let (kept, dev) = matched(true);
    let (plain, _) = matched(false);
    assert_eq!((kept.phase, plain.phase), (Phase::Present, Phase::Present));
    let (_, at1) = dev.looks();
    assert!(at1[5].abs() > 1.0, "the exit's solved look keeps an offset of its own ({})", at1[5]);
    // The offset is a fraction of the height it is drawn at, which the screen path is still
    // carrying from the node it matched from.
    assert!(
        ((kept.rect.y - plain.rect.y) - at1[5] * kept.rect.h).abs() <= 2.0 / 64.0,
        "{} vs {} for an offset of {} of {}",
        kept.rect.y,
        plain.rect.y,
        at1[5],
        kept.rect.h
    );
}

/// One frame of `node`: time moves, the scene frames at it, and the reference alone says whether
/// dev would report the node as exited this frame.
///
/// The reference's own presence track runs the walk's configured springs on the same clock, and
/// the scene's presence is compared against it every frame, so a scene that runs an exit for a
/// different duration than the transition asked for fails here rather than agreeing with itself.
fn tick(scene: &mut Scene, dev: &mut Dev, node: NodeId, frame: Rect, t: &mut f64, compare: bool, steps: &[Step]) {
    *t += DT;
    scene.set_time(*t);
    let p = presence(scene, node);
    let (settled, exited, changed) = {
        let out = scene.frame();
        let settled = out.events.iter().any(|e| {
            matches!(*e, MotionEvent::Settled { node: n, prop: q, .. } if n == node && q == prop::PRESENCE.id)
        });
        (settled, out.exited.contains(&node), out.changed.iter().find(|p| p.node == node).copied())
    };
    let (ref_p, ref_settled) = dev.reference_frame(*t);
    if dev.alive {
        assert!((p - ref_p).abs() <= 1e-6, "presence {p} vs {ref_p} after {steps:?}");
        assert_eq!(settled, ref_settled, "presence settles after {steps:?}");
    }
    let reported = dev.frame(dev.alive && ref_settled, ref_p);
    assert_eq!(exited, reported, "exited on this frame after {steps:?}");
    if compare && let Some(shown) = changed {
        // A delayed start only gets its layer on the tick that starts it, so a node still in the
        // scene is asked again after the frame; one that was unmounted keeps the value above.
        let shown_at = if scene.contains(node) { presence(scene, node) } else { p };
        check_presented(&shown, dev, shown_at, frame, steps);
    }
}

/// Runs `frames` frames of `node`, comparing what it presents against `dev` unless a screen path
/// places it.
fn advance(scene: &mut Scene, dev: &mut Dev, node: NodeId, frame: Rect, t: &mut f64, frames: usize, compare: bool) {
    for _ in 0..frames {
        tick(scene, dev, node, frame, t, compare, &[]);
    }
}


/// A child whose path started before its parent's is checked first, as dev's travelling list
/// checked it: its arrival waits on the parent's own frame animation, which landing the parent
/// ends, so it lands the frame after the parent. Scanning the tree would land the parent first
/// and let the child land on that same frame, one early.
#[test]
fn a_child_that_started_travelling_first_lands_the_frame_after_its_parent() {
    let (p, c) = (NodeId(10), NodeId(11));
    let child_frame = Rect::new(10.0, 10.0, 50.0, 50.0);
    let mut s = Scene::new();
    s.insert(p, NodeSpec::new(Rect::new(0.0, 0.0, 400.0, 400.0)), Mount::Initial);
    s.insert(c, NodeSpec::new(child_frame).parent(p), Mount::Initial);
    s.frame_at(0.0);
    let travel = LayoutAnim::new(Spring::new(0.3, 0.0));
    s.launch(c, Rect::new(300.0, 300.0, 50.0, 50.0), travel);
    s.launch(p, Rect::new(100.0, 100.0, 400.0, 400.0), travel);
    // The parent keeps animating its own frame while it travels, so its travel track arriving is
    // not what stops a child from landing on it.
    s.animate(p, prop::FRAME, Rect::new(20.0, 20.0, 400.0, 400.0), Spring::new(2.0, 0.0));
    let (mut parent, mut child) = (None, None);
    for k in 1..600 {
        s.frame_at(k as f64 / 120.0);
        parent = parent.or_else(|| s.is_resting_at(p, Rect::new(0.0, 0.0, 400.0, 400.0)).then_some(k));
        child = child.or_else(|| s.is_resting_at(c, child_frame).then_some(k));
        if parent.is_some() && child.is_some() {
            break;
        }
    }
    let (parent, child) = (parent.expect("the parent lands"), child.expect("the child lands"));
    assert_eq!(child, parent + 1, "parent landed on {parent}, child on {child}");
}
