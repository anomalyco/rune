//! Structure and presence: nodes mount, enter, leave and revive, and the presence progress
//! that blends a node's transition effect toward identity. Each change is one `phase::step`,
//! carried out here in the order it asks for.
use super::*;

impl Scene {
    /// A keyed node appears. On `Mount::Animated` it enters with its transition's insertion
    /// effect; on `Mount::Initial` (or under an entering parent, which it rides) it just appears.
    /// If the key is still exiting, `Mount::Animated` revives it from exactly where it is and
    /// `Mount::Initial` lands it present, at rest on `spec.frame`.
    pub fn insert(&mut self, id: NodeId, spec: NodeSpec, mount: Mount) {
        self.insert_delayed(id, spec, mount, 0.0);
    }

    /// [`insert`](Self::insert), with a new node's entrance held `delay` seconds of its own
    /// clock: it waits whole at its insertion effect (offset, scale, blur, opacity and reveal
    /// together), then plays the entrance on the transition's enter spring. A node that mounts
    /// at once, rides an entering parent or revives mid-exit ignores the delay, and
    /// [`land_waiting`](Self::land_waiting) lands it. Removed while it waits, it unmounts at
    /// once if its insertion effect is invisible, or else leaves from that effect.
    pub fn insert_delayed(&mut self, id: NodeId, spec: NodeSpec, mount: Mount, delay: f32) {
        if let Some(i) = self.slot(id) {
            self.nodes[i].transition = spec.transition;
            let exiting = self.nodes[i].presence.exiting();
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
        // A child rides an ancestor that is still mostly to come. Past half its entrance (and in
        // the ~0.7 s presence tail after it has visibly arrived), riding would show the child at
        // once at most of its opacity, so it enters on its own instead of popping in. A parent
        // that rode in itself is present at once, so the entering ancestor above it is the one
        // to ask for a plain fade: otherwise each level of a subtree mounting together fades on
        // its own, and the fades multiply until its deep rows show long after it has arrived.
        // An entrance of its own (anything beyond the plain fade, or a delay) asks only its parent,
        // so a list staggering its rows in under an entering surface keeps its stagger.
        let base_enter = if !self.transition_motion.is_empty()
            && let Some(&(enter, _)) = self.transition_motion.get(&id.0)
        {
            enter
        } else {
            Anim::from(spec.transition.springs.enter)
        };
        let base_delay = if base_enter.delay.is_finite() { base_enter.delay.max(0.0) } else { 0.0 };
        let extra_delay = if delay.is_finite() { delay.max(0.0) } else { 0.0 };
        let total_delay = base_delay + extra_delay;
        let plain = total_delay <= 0.0 && spec.transition.insertion == Transition::opacity().insertion;
        let rides_parent = {
            let mut a = parent;
            loop {
                if a == NONE { break false; }
                match self.nodes[a as usize].presence {
                    Presence::Present { .. } if plain => a = self.nodes[a as usize].parent,
                    Presence::Present { .. } => break false,
                    Presence::Entering { .. } => break self.presence_now(a as usize) < 0.5,
                    Presence::Exiting { .. } => break true,
                }
            }
        };
        let ins = spec.transition.insertion.kit_rules(false);
        // An entrance that changes nothing on screen (an identity insertion) has nothing to play.
        let animate = mount == Mount::Animated && !rides_parent && !ins.shows_nothing();
        let entrance = Anim { delay: total_delay, ..base_enter };
        let mut generation = 0;
        let mut presence = Presence::LANDED;
        let acts = phase::step(&mut presence, &mut generation, PhaseEvent::Mount { ins, animate, entrance });
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
            presence,
            generation,
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
        self.apply(i as usize, acts);
    }

    /// A present node's transition from its latest render: the removal effect and exit spring it
    /// leaves with, and how it enters if it revives. An entrance already running keeps its look;
    /// an exiting node keeps the transition it is leaving with.
    pub fn set_transition(&mut self, id: NodeId, transition: Transition) {
        if let Some(i) = self.slot(id)
            && !self.nodes[i].presence.exiting()
        {
            self.nodes[i].transition = transition;
        }
    }

    /// Overrides the enter and exit curves for `id`'s presence transitions, accepting any
    /// [`Anim`] (including timed curves) in place of [`Transition::springs`].
    ///
    /// Call before [`insert`](Self::insert) / [`insert_delayed`](Self::insert_delayed) so the
    /// initial entrance uses `enter`, or on a present node so its next exit or revive uses them.
    /// An entrance or exit already running is left undisturbed. Reduce Motion continues to apply
    /// to presence tracks as usual.
    ///
    /// ```
    /// use rune_motion::{Curve, Easing, Mount, NodeId, NodeSpec, Rect, Scene, Transition};
    ///
    /// let mut scene = Scene::new();
    /// let id = NodeId(1);
    /// scene.set_transition_motion(
    ///     id,
    ///     Curve::tween(0.16, Easing::EASE_OUT),
    ///     Curve::tween(0.11, Easing::EASE_IN),
    /// );
    /// scene.insert(id, NodeSpec::new(Rect::new(0.0, 0.0, 100.0, 40.0)).transition(Transition::opacity()), Mount::Animated);
    /// ```
    pub fn set_transition_motion(&mut self, id: NodeId, enter: impl Into<Anim>, exit: impl Into<Anim>) {
        self.transition_motion.insert(id.0, (enter.into(), exit.into()));
    }

    /// Removes any curve override installed by [`set_transition_motion`](Self::set_transition_motion)
    /// for `id`, falling back to [`Transition::springs`].
    pub fn clear_transition_motion(&mut self, id: NodeId) {
        if !self.transition_motion.is_empty() {
            self.transition_motion.remove(&id.0);
        }
    }

    pub(super) fn enter_anim(&self, i: usize) -> Anim {
        if !self.transition_motion.is_empty()
            && let Some(&(enter, _)) = self.transition_motion.get(&self.nodes[i].id.0)
        {
            enter
        } else {
            Anim::from(self.nodes[i].transition.springs.enter)
        }
    }

    pub(super) fn exit_anim(&self, i: usize) -> Anim {
        if !self.transition_motion.is_empty()
            && let Some(&(_, exit)) = self.transition_motion.get(&self.nodes[i].id.0)
        {
            exit
        } else {
            Anim::from(self.nodes[i].transition.springs.exit)
        }
    }

    /// The app removed a keyed node. It stays drawn, plays its removal effect on the exit curve,
    /// and is reported in `exited` only once its presence has settled at 0 (opacity 0).
    pub fn remove(&mut self, id: NodeId) {
        let Some(i) = self.slot(id) else { return };
        if self.nodes[i].presence.exiting() {
            return;
        }
        let p = self.presence_now(i);
        let rem = self.nodes[i].transition.removal.kit_rules(true);
        let exit = self.exit_anim(i);
        let acts = self.nodes[i].phase_step(&mut self.exiting, PhaseEvent::Exit { p, rem, exit });
        self.apply(i, acts);
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
            && self.nodes[i].presence.entering()
        {
            self.present_now(i);
        }
    }

    /// An entering node that hasn't started to show (a delayed entrance still waiting) is
    /// present at once. One already under way keeps going.
    pub fn land_waiting(&mut self, id: NodeId) {
        if let Some(i) = self.slot(id)
            && self.nodes[i].presence.entering()
            && self.presence_now(i) < 1e-3
        {
            self.present_now(i);
        }
    }

    /// A present node enters again from nothing, as if just mounted: one nobody saw that a
    /// change brings into view.
    pub fn reenter(&mut self, id: NodeId) {
        if let Some(i) = self.slot(id)
            && self.nodes[i].presence.resting()
        {
            self.track_set(i, prop::PRESENCE, 0.0);
            self.revive(i);
        }
    }

    fn revive(&mut self, i: usize) {
        let p = self.presence_now(i);
        let ins = self.nodes[i].transition.insertion.kit_rules(false);
        let entrance = self.enter_anim(i);
        let acts = self.nodes[i].phase_step(&mut self.exiting, PhaseEvent::Revive { p, ins, entrance });
        self.apply(i, acts);
    }

    /// An exiting node is present again at once: no presence effect, nothing left to settle.
    fn present_now(&mut self, i: usize) {
        let ins = self.nodes[i].transition.insertion.kit_rules(false);
        let acts = self.nodes[i].phase_step(&mut self.exiting, PhaseEvent::Land { ins });
        self.apply(i, acts);
    }

    fn presence_now(&self, i: usize) -> f32 {
        self.nodes[i].tracks[prop::PRESENCE.id.0 as usize].and_then(|t| self.sample(t)).map_or(1.0, |(x, _)| x[0])
    }

    /// Carries out what a phase change asked for, in order.
    pub(super) fn apply(&mut self, i: usize, acts: phase::Acts) {
        let id = self.nodes[i].id;
        for act in acts {
            match act {
                Act::Land(v) => self.track_set(i, prop::PRESENCE, v),
                Act::LandIfNeeded(v) => {
                    if !self.at_base(i, prop::PRESENCE, v) {
                        self.track_set(i, prop::PRESENCE, v);
                    }
                }
                Act::Ensure => self.ensure_prop(i, prop::PRESENCE),
                Act::Run { target, anim } => self.track_animate(i, prop::PRESENCE, target, anim),
                Act::Dirty => self.nodes[i].dirty = true,
                Act::Anchor(a) => self.nodes[i].anchor = a,
                Act::Reveal(r) => self.nodes[i].reveal = r,
                Act::Retire => {
                    if !self.to_remove.contains(&(i as u32)) {
                        self.to_remove.push(i as u32);
                    }
                }
                Act::Unmount => self.remove_now(i),
                Act::Intent { bits, seconds } => self.intent(id, bits, seconds),
            }
        }
    }
}
