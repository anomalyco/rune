//! Structure and presence: nodes mount, enter, leave and revive, and the presence progress
//! that blends a node's transition effect toward identity.
use super::*;

impl Scene {
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
        // A child rides a parent that is still mostly to come. Past half its entrance (and in
        // the ~0.7 s presence tail after it has visibly arrived), riding would show the child at
        // once at most of its opacity, so it enters on its own instead of popping in.
        let rides_parent = parent != NONE && match self.nodes[parent as usize].phase {
            Phase::Present => false,
            Phase::Entering => self.presence_now(parent as usize) < 0.5,
            Phase::Exiting { .. } => true,
        };
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
            ride_trailing: false,
            tracks: [None; B],
            dirty: true,
            world: Rect::ZERO,
            origin: Vec2::ZERO,
            last: None,
        };
        let i = match self.free.pop() {
            Some(i) => {
                self.nodes[i as usize] = node;
                self.rests[i as usize] = REST_ZERO;
                i
            }
            None => {
                self.nodes.push(node);
                self.rests.push(REST_ZERO);
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
        } else if !self.at_base(i as usize, prop::PRESENCE, 1.0) {
            self.track_set(i as usize, prop::PRESENCE, 1.0);
        }
    }

    /// A present node's transition from its latest render: the removal effect and exit spring it
    /// leaves with, and how it enters if it revives. An entrance already running keeps its look;
    /// an exiting node keeps the transition it is leaving with.
    pub fn set_transition(&mut self, id: NodeId, transition: Transition) {
        if let Some(i) = self.slot(id)
            && !matches!(self.nodes[i].phase, Phase::Exiting { .. })
        {
            self.nodes[i].transition = transition;
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
        self.ensure_prop(i, prop::PRESENCE);
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

    /// An entering node is present at once, its entrance landed: one nobody saw run.
    pub fn enter_now(&mut self, id: NodeId) {
        if let Some(i) = self.slot(id)
            && self.nodes[i].phase == Phase::Entering
        {
            self.present_now(i);
        }
    }

    /// A present node enters again from nothing, as if just mounted: one nobody saw that a
    /// change brings into view.
    pub fn reenter(&mut self, id: NodeId) {
        if let Some(i) = self.slot(id)
            && self.nodes[i].phase == Phase::Present
        {
            self.track_set(i, prop::PRESENCE, 0.0);
            self.revive(i);
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
        self.nodes[i].tracks[prop::PRESENCE.id.0 as usize].and_then(|t| self.sample(t)).map_or(1.0, |(x, _)| x[0])
    }
}
