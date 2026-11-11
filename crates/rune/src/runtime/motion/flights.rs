//! Elements that fly to their place on one screen path: `El::matched` pairs and `Tx::launch`.
use crate::machine::{Attempt, Effects, Seq};
use super::*;

/// `Tx::launch` and `El::matched` flights: each one from the frame it is asked for until it
/// lands, and the launches still waiting for their destination to mount.
#[derive(Default)]
pub(super) struct Launches {
    seq: Seq,
    launches: Vec<Launch>,
}

/// One launch or matched flight, with the attempt that names it while it has no node to fly to
/// yet.
struct Launch {
    attempt: Attempt,
    phase: LaunchPhase,
}

/// Where a launch is between being asked for and landing.
enum LaunchPhase {
    /// `Tx::launch`: its source's text was at `from` on screen, and it waits for the commit
    /// that mounts `target`. Its answer carries the attempt back.
    Waiting { target: String, from: [f32; 2] },
    /// In the air, at the node it flies to.
    Airborne(Airborne),
}

/// A flight in the air: a `Tx::launch`, which flies a path of its own, or an `El::matched`
/// flight started under `Tx::unclipped_flights`, which flies the scene's and draws outside
/// every clip until it lands.
enum Airborne {
    Launched(Launched),
    Unclipped { id: NodeId },
}

/// What moves a launch on, from being asked for to landing.
enum LaunchEvent<'a> {
    /// `Tx::launch` asked for `target` this frame, whose source's text was at `from` on screen.
    Ask { target: String, from: [f32; 2] },
    /// A commit builds: hand out the launches waiting for a destination to mount.
    Commit,
    /// The destination the launch `attempt` waited for mounted, and its flight starts there.
    TakeOff { attempt: Attempt, flight: Launched },
    /// It didn't mount, so the launch is over.
    Missed { attempt: Attempt },
    /// An `El::matched` flight on `id` starts (`on`) or stops drawing outside every clip.
    Unclip { id: NodeId, on: bool },
    /// The window's flights are `flying`: the ones in the air that left them landed.
    Flights { flying: &'a [(NodeId, MRect)] },
}

/// What a transition asks its caller to do: find `target`'s node in the commit's tree and start
/// its path from `from`, then feed the flight back as `TakeOff`, or `Missed` when it didn't
/// mount. A launch keeps its attempt while it waits, so a late answer changes nothing.
enum LaunchEffect {
    Mount { attempt: Attempt, target: String, from: [f32; 2] },
}

impl Launch {
    /// The node it flies to, once it has one.
    fn airborne_id(&self) -> Option<NodeId> {
        match &self.phase {
            LaunchPhase::Waiting { .. } => None,
            LaunchPhase::Airborne(Airborne::Launched(flight)) => Some(flight.id),
            LaunchPhase::Airborne(Airborne::Unclipped { id }) => Some(*id),
        }
    }
}

/// A launch in the air, the offsets outside the scene it was started against, the ones its
/// last frame drew it under (none before its first frame), and how fast they last moved
/// between frames with no commit. Its re-aims folded `folded` of what its flight added to them
/// into its path, when `1 - travel` was `folded_at`.
struct Launched {
    id: NodeId,
    shift: [f32; 2],
    drawn: Option<Drawn>,
    drift: [f32; 2],
    folded: [f32; 2],
    folded_at: [f32; 2],
}

/// The offsets outside the scene a launch's frame drew it under, at scene time `at`, after
/// commit `commit`.
#[derive(Clone, Copy)]
struct Drawn {
    offset: [f32; 2],
    at: f64,
    commit: u64,
}

impl Launches {
    pub(super) fn pending(&self) -> bool { self.launches.iter().any(|l| matches!(l.phase, LaunchPhase::Waiting { .. })) }

    /// `Tx::launch` asked for `target` this frame, whose source's text was at `from` on screen.
    pub(super) fn wait(&mut self, target: String, from: [f32; 2]) {
        self.step(LaunchEvent::Ask { target, from });
    }

    /// What this commit mounts, in the order they were asked for. Each launch waits for its
    /// answer (`take_off` or `miss`) before it is over.
    pub(super) fn commit(&mut self) -> Vec<LaunchEffect> { self.step(LaunchEvent::Commit).into_iter().collect() }

    /// The destination `attempt` waited for mounted, and `flight` starts at its node.
    pub(super) fn take_off(&mut self, attempt: Attempt, flight: Launched) {
        self.step(LaunchEvent::TakeOff { attempt, flight });
    }

    /// The destination `attempt` waited for didn't mount, so its launch is over.
    pub(super) fn miss(&mut self, attempt: Attempt) { self.step(LaunchEvent::Missed { attempt }); }

    /// Starts (`on`) or drops the entry for `id`'s `El::matched` flight, which draws it outside
    /// every clip until it lands. A `Tx::launch` on the same node draws it so already.
    pub(super) fn unclip(&mut self, id: NodeId, on: bool) { self.step(LaunchEvent::Unclip { id, on }); }

    /// Drops the flights in the air that landed (left `flying`). Launches still waiting for
    /// their destination stay for the commit that mounts it.
    pub(super) fn keep(&mut self, flying: &[(NodeId, MRect)]) { self.step(LaunchEvent::Flights { flying }); }

    /// One transition: the phase each event leaves a launch in, and the mount attempts a commit
    /// hands out. The caller finds the node and starts its path, then feeds the answer back.
    fn step(&mut self, event: LaunchEvent) -> Effects<LaunchEffect> {
        match event {
            LaunchEvent::Ask { target, from } => {
                let attempt = self.seq.next();
                self.launches.push(Launch { attempt, phase: LaunchPhase::Waiting { target, from } });
                Effects::none()
            }
            LaunchEvent::Commit => {
                let mut mounts = Effects::none();
                for launch in &self.launches {
                    if let LaunchPhase::Waiting { target, from } = &launch.phase {
                        mounts.push(LaunchEffect::Mount { attempt: launch.attempt, target: target.clone(), from: *from });
                    }
                }
                mounts
            }
            LaunchEvent::TakeOff { attempt, flight } => {
                let waiting = self.launches.iter().position(|l| l.attempt == attempt && matches!(l.phase, LaunchPhase::Waiting { .. }));
                // An answer whose launch is no longer waiting changes nothing.
                if let Some(at) = waiting {
                    let to = flight.id;
                    self.launches.remove(at);
                    // It takes over the flight its node already had: a matched one, unclipped.
                    self.launches.retain(|l| l.airborne_id() != Some(to));
                    self.launches.push(Launch { attempt, phase: LaunchPhase::Airborne(Airborne::Launched(flight)) });
                }
                Effects::none()
            }
            LaunchEvent::Missed { attempt } => {
                self.launches.retain(|l| !(l.attempt == attempt && matches!(l.phase, LaunchPhase::Waiting { .. })));
                Effects::none()
            }
            LaunchEvent::Unclip { id, on } => {
                self.launches.retain(|l| !matches!(&l.phase, LaunchPhase::Airborne(Airborne::Unclipped { id: other }) if *other == id));
                if on && !self.flying(id) {
                    let attempt = self.seq.next();
                    self.launches.push(Launch { attempt, phase: LaunchPhase::Airborne(Airborne::Unclipped { id }) });
                }
                Effects::none()
            }
            LaunchEvent::Flights { flying } => {
                self.launches.retain(|l| l.airborne_id().is_none_or(|id| flying.iter().any(|f| f.0 == id)));
                Effects::none()
            }
        }
    }

    /// Draws outside every clip until it lands: a launch, at its own size, or an unclipped
    /// matched flight.
    pub(super) fn flying(&self, id: NodeId) -> bool { self.launches.iter().any(|l| l.airborne_id() == Some(id)) }

    /// A launch in the air, which flies at its own size.
    pub(super) fn launched(&self, id: NodeId) -> bool { self.flight(id).is_some() }

    /// The launch in the air at `id`, whose own path is what makes it a launch.
    fn flight(&self, id: NodeId) -> Option<&Launched> {
        self.launches.iter().find_map(|l| match &l.phase {
            LaunchPhase::Airborne(Airborne::Launched(f)) if f.id == id => Some(f),
            _ => None,
        })
    }

    fn flight_mut(&mut self, id: NodeId) -> Option<&mut Launched> {
        self.launches.iter_mut().find_map(|l| match &mut l.phase {
            LaunchPhase::Airborne(Airborne::Launched(f)) if f.id == id => Some(f),
            _ => None,
        })
    }

    /// Before its destination's layout moves mid-flight and its path restarts from where it is
    /// drawn: the offset and velocity outside the scene its last frame drew it with, which the
    /// new path takes over, so the restart keeps its place and speed. It then flies on under
    /// the offsets it was last drawn under.
    pub(super) fn restart(&mut self, scene: &Scene, id: NodeId) -> [rune_motion::Vec2; 2] {
        let Some(l) = self.flight_mut(id) else { return [rune_motion::Vec2::ZERO; 2] };
        let ([ox, oy], [vx, vy]) = l.blend(scene);
        if let Some(drawn) = l.drawn {
            let carried = l.carried(scene);
            let travel = scene.get(id, prop::TRAVEL).unwrap_or(rune_motion::Vec2::new(1.0, 1.0));
            (l.folded, l.folded_at) = (carried, [1.0 - travel.x, 1.0 - travel.y]);
            l.shift = drawn.offset;
        }
        [rune_motion::Vec2::new(ox, oy), rune_motion::Vec2::new(vx, vy)]
    }

    /// How much of the scroll under it launch `id` doesn't ride yet: what its flight adds to the
    /// offsets outside the scene, and what its re-aims folded into its path, let go as it
    /// travels. Without a launch, nothing.
    pub(super) fn carried(&self, scene: &Scene, id: NodeId) -> [f32; 2] {
        self.flight(id).map_or([0.0; 2], |l| l.carried(scene))
    }

    /// What launch `id`'s flight adds to the offsets outside the scene it is drawn under, and
    /// how much faster that moves it than they do.
    pub(super) fn lead(&self, scene: &Scene, id: NodeId) -> [rune_motion::Vec2; 2] {
        let Some(l) = self.flight(id) else { return [rune_motion::Vec2::ZERO; 2] };
        let ([ox, oy], [vx, vy]) = l.blend(scene);
        [rune_motion::Vec2::new(ox, oy), rune_motion::Vec2::new(vx - l.drift[0], vy - l.drift[1])]
    }

    /// A launch leaves from where its source was seen and flies to where its destination is
    /// now: it starts under the offsets outside the scene it launched against and ends under
    /// this frame's `offset`, blending by its travel. A growth row above still resizing, a
    /// moved virtual window, or a row whose slot moves under an offset that hides the move
    /// (an inserted row's growth) shifts where it lands, never where it is. What to add to it.
    /// A commit's offset step comes with a layout move that hides it, so only frames with
    /// no commit between them measure the offsets' drift.
    pub(super) fn offset_fix(&mut self, scene: &Scene, id: NodeId, offset: [f32; 2], commit: u64) -> [f32; 2] {
        let Some(l) = self.flight_mut(id) else { return [0.0; 2] };
        let now = scene.now();
        if let Some(last) = l.drawn && last.commit == commit && now > last.at {
            let dt = (now - last.at) as f32;
            l.drift = [(offset[0] - last.offset[0]) / dt, (offset[1] - last.offset[1]) / dt];
        }
        l.drawn = Some(Drawn { offset, at: now, commit });
        let travel = scene.get(id, prop::TRAVEL).unwrap_or(rune_motion::Vec2::new(1.0, 1.0));
        [(l.shift[0] - offset[0]) * (1.0 - travel.x), (l.shift[1] - offset[1]) * (1.0 - travel.y)]
    }
}

impl Launched {
    fn carried(&self, scene: &Scene) -> [f32; 2] {
        let travel = scene.get(self.id, prop::TRAVEL).unwrap_or(rune_motion::Vec2::new(1.0, 1.0));
        let (blend, _) = self.blend(scene);
        let left = [1.0 - travel.x, 1.0 - travel.y];
        [0, 1].map(|k| blend[k] + if self.folded_at[k] > 1e-4 { self.folded[k] * (left[k] / self.folded_at[k]).clamp(0.0, 1.0) } else { 0.0 })
    }

    /// What its flight adds to the offsets outside the scene it is drawn under, and how fast
    /// that moves it. Drawn under `offset + (shift - offset) * (1 - travel)`: its rate is the
    /// offset's drift times travel plus `(offset - shift) * rate`.
    fn blend(&self, scene: &Scene) -> ([f32; 2], [f32; 2]) {
        let Some(Drawn { offset: last, .. }) = self.drawn else { return ([0.0; 2], [0.0; 2]) };
        let travel = scene.get(self.id, prop::TRAVEL).unwrap_or(rune_motion::Vec2::new(1.0, 1.0));
        let rate = scene.store().sample_velocity(self.id, prop::TRAVEL).unwrap_or(rune_motion::Vec2::ZERO);
        let (t, r) = ([travel.x, travel.y], [rate.x, rate.y]);
        (
            [0, 1].map(|k| (self.shift[k] - last[k]) * (1.0 - t[k])),
            [0, 1].map(|k| self.drift[k] * t[k] + (last[k] - self.shift[k]) * r[k]),
        )
    }
}

impl<S: 'static> Runtime<S> {
    /// Starts the text flights and launches handlers asked for this frame.
    pub(super) fn capture_flights(&mut self, tx: &mut Tx) {
        for (source, target) in std::mem::take(&mut tx.text_flights) {
            self.capture_text_flight(source, target);
        }
        for (source, target) in std::mem::take(&mut tx.launches) {
            self.capture_launch(source, target);
        }
    }

    /// Flights drawn within a quarter of a device pixel of where their layout would show them,
    /// and moving under that a frame (60 Hz) faster than it: as good as landed, kept in the
    /// air only by their springs' settle tails. They draw on the same pixels landed.
    pub(super) fn landing_flights(&self) -> Vec<NodeId> {
        let near = 0.25 / self.env.scale.max(1.0);
        let within = |v: rune_motion::Vec2| v.x.abs().max(v.y.abs()) <= near;
        self.flying.iter().map(|f| f.0).filter(|&id| {
            let Some([p, v]) = self.scene.path_lead(id) else { return false };
            let [bp, bv] = self.launches.lead(&self.scene, id);
            let add = |a: rune_motion::Vec2, b: rune_motion::Vec2| rune_motion::Vec2::new(a.x + b.x, a.y + b.y);
            let v = add(v, bv);
            within(add(p, bp)) && within(rune_motion::Vec2::new(v.x / 60.0, v.y / 60.0))
        }).collect()
    }

    /// Lands flight `id` at its layout, as if it had landed: it rides its parent from here.
    pub(super) fn land_flight(&mut self, id: NodeId) {
        self.flying.retain(|f| f.0 != id);
        self.launches.keep(&self.flying);
        self.scene.land_path(id);
    }

    /// Shared-element motion (`El::matched`): each of the last commit's matched elements that
    /// left the tree hands its geometry, radius, fill and opacity to the element with its id that
    /// just mounted. Costs nothing without matched elements; else O(matched), plus one pass over
    /// the old nodes when a pair matches.
    pub(super) fn match_elements(&mut self, nodes: &[Node], arrivals: &[(Rc<str>, usize, bool)], tx: &Tx, instant: bool) {
        let mut by_id: HashMap<Rc<str>, (usize, bool)> = HashMap::with_capacity_and_hasher(arrivals.len(), Default::default());
        for (key, i, arriving) in arrivals {
            if by_id.insert(key.clone(), (*i, *arriving)).is_some() { self.warn_duplicate_match(key); }
        }
        let mut current: Vec<(Rc<str>, NodeId)> = by_id.iter().map(|(key, &(i, _))| (key.clone(), nodes[i].id)).collect();
        current.sort_unstable_by_key(|(key, _)| by_id[key].0);
        let old = std::mem::replace(&mut self.matched, current);
        // Under Reduce Motion the old one exits and the new one enters, each by its own
        // transition (a crossfade in place) and with the parents it leaves or enters with.
        if instant || tx.commit.lands() || self.reduced_motion { return; }
        let mut old_index: Option<HashMap<NodeId, usize>> = None;
        let mut built_index: Option<HashMap<NodeId, usize>> = None;
        // Descendants first: matching an element unmounts its old subtree, matched ones included.
        for (key, from) in old.into_iter().rev() {
            let Some(&(i, true)) = by_id.get(&key) else { continue };
            let to = nodes[i].id;
            if from == to || self.node_index.contains_key(&from) || !self.scene.contains(from) || self.is_exiting(from) { continue; }
            let index = old_index.get_or_insert_with(|| self.nodes.iter().enumerate().map(|(k, n)| (n.id, k)).collect());
            let Some(&from_i) = index.get(&from) else { continue };
            // Scroll offsets live outside the scene: start where `from` was seen on screen. They
            // come from the last build, which a commit earlier in the same frame (a glide's poll
            // before the key that closes it) leaves in place, so look nodes up by id.
            let built = &*built_index.get_or_insert_with(|| self.built_ids.iter().enumerate().map(|(k, &id)| (id, k)).collect());
            // Flights drop the cover offsets of flights around them.
            let shift = |id: NodeId| built.get(&id).and_then(|&k| self.inh_scratch.get(k)).map_or([0.0; 2], |h| [h.dx - h.cover[0], h.dy - h.cover[1]]);
            let source = self.nodes[from_i].parent.map_or([0.0; 2], |p| shift(self.nodes[p].id));
            let row = built.get(&from).and_then(|&k| self.row_dy.get(k)).copied().unwrap_or(0.0);
            let dest = std::iter::successors(nodes[i].parent, |&p| nodes[p].parent)
                .map(|p| nodes[p].id).find(|id| built.contains_key(id)).map_or([0.0; 2], shift);
            let offset = rune_motion::Vec2::new(source[0] - dest[0], source[1] + row - dest[1]);
            let spring = tx.motion.map(MotionSource::spring).or(nodes[i].matched.as_ref().and_then(|m| m.1)).unwrap_or(motion::REFLOW);
            let fill = self.color(from, P_FILL, self.nodes[from_i].vis.fill);
            let opacity = self.scene.get(from, prop::OPACITY);
            self.scene.match_geometry_offset(from, to, offset, LayoutAnim::new(spring).screen());
            let live = |p: crate::live::Property| nodes[i].live.iter().any(|b| b.property == p);
            if fill != nodes[i].vis.fill && !live(crate::live::Property::Fill) {
                self.scene.set(to, P_FILL, fill);
                self.scene.animate(to, P_FILL, nodes[i].vis.fill, spring);
            }
            if let Some(o) = opacity.filter(|&o| o != nodes[i].vis.opacity) && !live(crate::live::Property::Opacity) {
                self.scene.set(to, prop::OPACITY, o);
                self.scene.animate(to, prop::OPACITY, nodes[i].vis.opacity, spring);
            }
            // Its content mounted with it and flies as part of what was seen, so none of it fades
            // in on its own (each fresh level under a present parent otherwise would).
            for j in i + 1..nodes.len() {
                if !std::iter::successors(nodes[j].parent, |&p| nodes[p].parent).any(|p| p == i) { break; }
                self.scene.enter_now(nodes[j].id);
            }
            self.flying.retain(|f| f.0 != to);
            self.flying.push((to, nodes[i].local));
            self.launches.unclip(to, tx.unclipped_flights);
        }
    }

    pub(super) fn capture_text_flight(&mut self, source: String, target: String) {
        let Some((i, n)) = self.nodes.iter().enumerate().find(|(_, n)| crate::flight::selects(&source, &n.path, n.key.as_deref())) else { return };
        let (Some(spec), Some(laid), Some(r)) = (&n.text, &n.text_layout, self.drawn.get(i)) else { return };
        if spec.redacted || spec.content.is_empty() { return; }
        let scale = self.env.scale;
        let (rect, origin) = text_extent(n, spec, laid.view(), *r, scale);
        let clip = self.screen_clip_of_parent(n.parent).map(|v| v / scale);
        let clip = [rect.x.max(clip[0]).max(0.0), rect.y.max(clip[1]).max(0.0),
            (rect.x + rect.w).min(clip[2]).min(self.viewport.0), (rect.y + rect.h).min(clip[3]).min(self.viewport.1)];
        if rect.w <= 0.0 || clip[2] <= clip[0] || clip[3] <= clip[1] { return; }
        self.glyph_scratch.clear();
        self.text.set_glyph_inks(spec.color, &[]);
        // From outlines where text draws them, which a flight's zoom magnifies without blurring.
        self.text.set_emit(self.text.moving_emit());
        for p in laid.view().pieces() {
            self.text.glyphs_for_layout(p.layout, scale, [origin[0] * scale, origin[1] * scale + p.y], false, &mut self.glyph_scratch);
        }
        self.text.set_emit(crate::text::outline::Emit::Atlas);
        let color = spec.color;
        let srgb = self.srgb_blend;
        // Colour glyphs live in the image atlas, which may move them before the flight ends.
        let glyphs = self.glyph_scratch.iter().filter(|q| q.color == 0 && q.rect[1] < clip[3] * scale && q.rect[1] + q.rect[3] > clip[1] * scale)
            .map(|q| glyph(q, premul(color, 1.0, srgb), clip.map(|v| v * scale), [0.0; 4])).collect();
        self.flights.push(TextFlight::new(source, target, rect, clip, glyphs, scale, self.t));
    }

    /// `Tx::launch`, before the commit: where the source's text starts on screen.
    pub(super) fn capture_launch(&mut self, source: String, target: String) {
        let Some((i, n)) = self.nodes.iter().enumerate().find(|(_, n)| crate::flight::selects(&source, &n.path, n.key.as_deref())) else { return };
        let (Some(spec), Some(laid), Some(r)) = (&n.text, &n.text_layout, self.drawn.get(i)) else { return };
        let (_, origin) = text_extent(n, spec, laid.view(), *r, self.env.scale);
        self.launches.wait(target, origin);
    }

    /// `Tx::launch`, in the commit: each destination that just mounted starts with its first
    /// text where its source's text was seen, at its own size, and flies to its place unclipped.
    pub(super) fn launch_elements(&mut self, nodes: &[Node], tx: &Tx, instant: bool) {
        let mounts = self.launches.commit();
        if instant || tx.commit.lands() || self.reduced_motion {
            for LaunchEffect::Mount { attempt, .. } in mounts { self.launches.miss(attempt); }
            return;
        }
        let index: HashMap<NodeId, usize> = self.nodes.iter().enumerate().map(|(k, n)| (n.id, k)).collect();
        let built: HashMap<NodeId, usize> = self.built_ids.iter().enumerate().map(|(k, &id)| (id, k)).collect();
        let scale = self.env.scale;
        for mount in mounts {
            let LaunchEffect::Mount { attempt, target, from } = mount;
            let Some(i) = nodes.iter().position(|n| crate::flight::selects(&target, &n.path, n.key.as_deref())) else {
                self.launches.miss(attempt);
                continue;
            };
            let to = nodes[i].id;
            if index.contains_key(&to) && !self.is_exiting(to) {
                self.launches.miss(attempt);
                continue;
            }
            let within = |mut j: usize| loop {
                match nodes[j].parent {
                    Some(p) if p == i => break true,
                    Some(p) if p > i => j = p,
                    _ => break false,
                }
            };
            // Its first text, or its own top-left.
            let t = nodes[i].target;
            let rel = (i..nodes.len()).take_while(|&j| j == i || within(j)).find_map(|j| {
                let (Some(spec), Some(laid)) = (&nodes[j].text, &nodes[j].text_layout) else { return None };
                let r = nodes[j].target;
                let (_, origin) = text_extent(&nodes[j], spec, laid.view(), [r[0], r[1], r[2] - r[0], r[3] - r[1]], scale);
                Some([origin[0] - t[0], origin[1] - t[1]])
            }).unwrap_or([0.0; 2]);
            // Scroll offsets live outside the scene: subtract the destination's. Under a still
            // scale, its screen point maps back through it to where it is laid out. Both come
            // from the last build, which a commit earlier in the same frame leaves in place.
            let (mut shift, mut zoom) = ([0.0; 2], (1.0, [0.0; 2], [0.0; 2]));
            if let Some(h) = std::iter::successors(nodes[i].parent, |&p| nodes[p].parent)
                .find_map(|p| built.get(&nodes[p].id).copied())
                .and_then(|k| self.inh_scratch.get(k))
            {
                shift = [h.dx - h.cover[0], h.dy - h.cover[1]];
                zoom = h.still;
            }
            let (k, anchor, anchor_to) = zoom;
            let at = |axis: usize| anchor[axis] + (from[axis] - anchor_to[axis]) / k - rel[axis] - shift[axis];
            let (w, h) = (t[2] - t[0], t[3] - t[1]);
            let start = MRect::new(at(0), at(1), w, h);
            let spring = tx.motion.map_or(motion::REFLOW, MotionSource::spring);
            self.scene.launch(to, start, LayoutAnim::new(spring).screen());
            self.flying.retain(|f| f.0 != to);
            self.flying.push((to, nodes[i].local));
            self.launches.take_off(attempt, Launched { id: to, shift, drawn: None, drift: [0.0; 2], folded: [0.0; 2], folded_at: [0.0; 2] });
        }
    }

    fn warn_duplicate_match(&mut self, key: &Rc<str>) {
        let message = || format!("duplicate matched id {key:?}: the last element in tree order is matched; give each element its own id");
        if let Some(warnings) = self.key_warnings.as_mut() { warnings.push(message()); }
        #[cfg(debug_assertions)]
        if self.matched_warned.insert(key.clone()) { eprintln!("rune: {}", message()); }
    }
}

#[cfg(test)]
#[path = "flights_tests.rs"]
mod tests;
