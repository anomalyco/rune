//! Per-frame stepping, presence retirement, screen-travel handoff, and [`Presented`] output.

use super::*;

impl Scene {
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
        self.exited.begin();
        self.to_remove.clear();

        // Presence settles: finish entrances, retire an exit that is still the node's own (a
        // node that revived and left again is a different exit, and its settle retires it then).
        for ev in self.store.events() {
            if let MotionEvent::Settled { node, prop: p, .. } = *ev {
                if p != prop::PRESENCE.id {
                    continue;
                }
                let Some(&i) = self.index.get(&node.0) else { continue };
                let acts = self.nodes[i as usize].phase_step(&mut self.exiting, PhaseEvent::Settle);
                for act in acts {
                    if let Act::Retire = act {
                        self.to_remove.push(i);
                    }
                }
            }
        }
        // An exit nobody can see is over. Presence settles at 1e-5, ~40 frames after the node
        // fades below one 8-bit step; retire it there so whatever waits on the unmount (focus,
        // hit-testing, the next layer) doesn't wait on an invisible tail.
        let mut retire = core::mem::take(&mut self.to_remove);
        for (t, node, p) in self.store.touched_tracks() {
            if p != prop::PRESENCE.id {
                continue;
            }
            let Some(&i) = self.index.get(&node.0) else { continue };
            if !self.nodes[i as usize].presence.exiting() || retire.contains(&i) {
                continue;
            }
            let Some(&[presence, ..]) = self.store.values_of(t) else { continue };
            let acts = self.nodes[i as usize].phase_step(&mut self.exiting, PhaseEvent::Faded { p: presence });
            for act in acts {
                if let Act::Retire = act {
                    retire.push(i);
                }
            }
        }
        self.to_remove = retire;
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
        // Screen travel ends once it has arrived and no ancestor is still moving or transitioning.
        // `travelling` is the order the paths started in, not the tree's: a child whose path
        // started first is checked before its parent, as dev checked it.
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
            let po = self.parent_target_origin(i);
            if let Some(path) = &mut self.nodes[i].path {
                path.origin = po;
            }
            k += 1;
        }
        if self.order_dirty {
            self.rebuild_order();
        }
        self.out.clear();
        // Written first, so this frame's own values for the same nodes win.
        self.out.append(&mut self.late);
        self.present_dirty();
        for k in 0..self.to_remove.len() {
            let i = self.to_remove[k] as usize;
            if self.nodes[i].alive {
                self.remove_now(i);
            }
        }
        self.exited.end();
        // A node still travelling waits on an active track of its own or an ancestor's, so this
        // covers it, and a frozen one doesn't keep the frames coming.
        let animating = self.store.running();
        if let Some(trace) = self.trace.as_mut() {
            trace.frame(self.store.now(), &self.out, self.exited.as_slice(), self.store.events(), &self.intents);
            self.intents.clear();
        }
        FrameOutput { changed: &self.out, exited: self.exited.as_slice(), events: self.store.events(), animating }
    }

    /// What [`frame`](Self::frame) and any [`present`](Self::present) since presented.
    pub fn changed(&self) -> &[Presented] {
        &self.out
    }

    /// Presents what was set or animated since [`frame`](Self::frame), without stepping time,
    /// so a value worked out from something that frame moved afterwards (a scroll offset)
    /// shows in the same frame. The motion trace, already written for that frame, records them
    /// with the next.
    pub fn present(&mut self) {
        if self.order_dirty {
            self.rebuild_order();
        }
        let from = self.out.len();
        self.present_dirty();
        if self.trace.is_some() {
            self.late.extend_from_slice(&self.out[from..]);
        }
    }

    /// Recomputes the presented values of dirty nodes and their subtrees, into `out`.
    fn present_dirty(&mut self) {
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
    }

    /// An ancestor still moving or changing how it draws its subtree: a node ending screen travel
    /// under it would jump onto its mid-transition offset, scale, opacity or blur.
    fn ancestor_moving(&self, i: usize) -> bool {
        let mut p = self.nodes[i].parent;
        while p != NONE {
            let n = &self.nodes[p as usize];
            for prop in [prop::FRAME.id, prop::TRAVEL.id, prop::OFFSET.id, prop::PRESENCE.id, prop::SCALE.id, prop::OPACITY.id, prop::BLUR.id] {
                if n.tracks[prop.0 as usize].is_some_and(|t| self.store.is_active(t)) {
                    return true;
                }
            }
            p = n.parent;
        }
        false
    }

    pub(super) fn values(&self, i: usize, p: PropId) -> Option<&[f32]> {
        self.nodes[i].tracks[p.0 as usize].and_then(|t| self.store.values_of(t))
    }

    fn targets<const N: usize>(&self, i: usize, p: PropId) -> Option<[f32; N]> {
        let mut out = [0.0; N];
        let t = self.nodes[i].tracks[p.0 as usize]?;
        self.store.targets_of(t, &mut out).then_some(out)
    }

    /// Where `compute` would put node `i` with every track at its target. It repeats the same
    /// arithmetic so that, at rest, the two rects are bit-identical.
    fn rest_of(&self, i: usize, reduced: bool) -> Rect {
        let n = &self.nodes[i];
        let parent = n.parent;
        let world = match n.path {
            Some(path) => {
                let p = self.targets::<2>(i, prop::TRAVEL.id).unwrap_or([1.0, 1.0]);
                let s = path.start.edges();
                let e = path.end_local.translated(self.parent_target_origin(i)).edges();
                let ed: [f32; 4] = core::array::from_fn(|k| s[k] + (e[k] - s[k]) * p[k % 2]);
                Rect::from_edges(ed[0], ed[1], ed[2], ed[3])
            }
            None => {
                let origin = if parent == NONE { Vec2::ZERO } else { self.rests[parent as usize].origin };
                let local = self.targets::<4>(i, prop::FRAME.id).map_or(n.frame, |v| Rect::read_lanes(&v));
                local.translated(origin)
            }
        };
        let p = self.targets::<1>(i, prop::PRESENCE.id).map_or(1.0, |v| v[0]);
        let (at0, at1) = n.presence.pair();
        let s = lerp_fx(at0, at1, p);
        let s = if reduced { [s[0], 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] } else { s };
        let own = self.targets::<2>(i, prop::OFFSET.id).map_or(Vec2::ZERO, |v| Vec2::read_lanes(&v));
        world.translated(Vec2::new(own.x + s[2] + s[4] * world.w, own.y + s[3] + s[5] * world.h))
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
                let mut world = local.translated(parent_origin);
                if n.ride_trailing && parent != NONE {
                    let p = &self.nodes[parent as usize];
                    world.x += p.world.w - p.frame.w;
                }
                world
            }
        };
        let p = self.values(i, prop::PRESENCE.id).map_or(1.0, |v| v[0]);
        let (at0, at1) = n.presence.pair();
        let s = lerp_fx(at0, at1, p);
        let s = if reduced { [s[0], 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] } else { s };
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
        let in_flow = !n.presence.exiting() || !n.transition.pop_layout;
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
            phase: n.presence.public(n.generation),
            in_flow,
            flags: fl,
            draw: s[8].clamp(0.0, 1.0),
        };
        // Where it is is where it rests while nothing that places it is in flight. A node riding
        // its parent's trailing edge also moves with the parent's width.
        let still = !self.nodes[i].ride_trailing
            && (parent == NONE || self.rests[parent as usize].origin == self.nodes[parent as usize].origin) && [prop::TRAVEL.id, prop::FRAME.id, prop::PRESENCE.id, prop::OFFSET.id]
            .iter()
            .all(|p| self.nodes[i].tracks[p.0 as usize].is_none_or(|t| self.store.is_settled(t)));
        let rest = if still { shown } else { self.rest_of(i, reduced) };
        let n = &mut self.nodes[i];
        n.world = world;
        n.origin = Vec2::new(shown.x, shown.y);
        self.rests[i] = Rest { rect: rest, origin: Vec2::new(rest.x, rest.y), moving: rest != shown };
        out
    }
}
