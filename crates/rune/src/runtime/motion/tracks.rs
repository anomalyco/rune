//! Render-side growth, scroll and streaming word-reveal tracks, plus animated paint samples.
use super::*;
use crate::runtime::scroll_mode::{self, Event as ModeEvent};

/// Scroll momentum decay (iOS normal: 0.998 per ms).
pub(in crate::runtime) const SCROLL_DECAY: f32 = 2.002;

/// One `El::matched` handover recorded during a frame, before the frame's paint pass computes
/// `to_id`'s drawn rect.
pub(in crate::runtime) struct RecordedHandover {
    pub(in crate::runtime) from_path: String,
    pub(in crate::runtime) to_path: String,
    pub(in crate::runtime) to_id: NodeId,
    pub(in crate::runtime) from_rect: [f32; 4],
    pub(in crate::runtime) start_rect: [f32; 4],
}

/// Retained render-side motion state: growth stacks, streaming word reveals, matched/launch/text
/// flights, loops and rewind capture. Ordered hot-first under `#[repr(C)]` so `flying`, `flights`,
/// frame flags and `loops` read during the paint walk sit together and first.
#[repr(C)]
pub(crate) struct MotionState {
    /// Elements flying from a matched one, with their scene target. They paint last.
    pub(in crate::runtime) flying: Vec<(NodeId, MRect)>,
    pub(super) flights: Vec<TextFlight>,
    caught_up: f32,
    slack_dt: f32,
    /// Opt-in wake-rate cap (Hz) when `El::reveal` word fades are the only active animation (`0.0` = display rate).
    reveal_fps: f32,
    /// Why the last frame was animating, as [`AnimSource`] bits, for traces.
    pub(in crate::runtime) anim_sources: u16,
    loops_active: bool,
    /// Some node ran on a scaled clock (`El::time_scale`) at the last commit.
    time_scaled: bool,
    /// Motion has a cause (`Tx::unprompted`): a commit no input in this window caused mounts and
    /// unmounts at once. On for every window an app opens (`tooling::runtime_for`); a bare
    /// `Runtime` keeps every commit's motion.
    pub(in crate::runtime) motion_needs_cause: bool,
    pub(super) loops: HashMap<NodeId, rune_motion::LoopTrack>,
    /// Shared host deadline: unscaled wake time and corresponding animation time.
    loop_wake: Option<(f64, f64)>,
    /// `Tx::launch` flights, waiting for their commit or in the air.
    pub(super) launches: flights::Launches,
    pub(in crate::runtime) grows: HashMap<NodeId, GrowTrack>,
    pub(in crate::runtime) stacks: HashMap<NodeId, Stack>,
    /// The header each lifted fold shuts under, by the fold's id, which draws where it's laid out.
    pub(super) fold_headers: HashMap<NodeId, String>,
    /// What this frame's commits held at their landing width (`El::hold_landing_width`).
    pub(super) landing_holds: landing_hold::LandingHolds,
    pub(super) reveals: HashMap<NodeId, RevealPhase>,
    /// The reveal states `reveals` keeps for texts that have left the tree, newest first, at
    /// most `prune::REVEALS_KEPT` of them (see `prune_exited`).
    reveals_kept: Vec<NodeId>,
    /// The newest streamed word of any text, which the next text's words queue behind.
    reveal_chain: RevealChain,
    /// The last commit's `El::matched` ids, one present element each (the last in tree order).
    pub(super) matched: Vec<(Rc<str>, NodeId)>,
    /// `El::matched` handovers completed during the current frame, cleared when the next frame begins.
    handovers: Vec<RecordedHandover>,
    pub(super) loop_clock: loop_clock::LoopClock,
    /// `motion_elements`' per-node scratch, kept so a frame doesn't allocate.
    pub(in crate::runtime) motion_scratch: std::cell::RefCell<Vec<(u32, u16, bool)>>,
    /// The recorder `Tx::record_rewind` attached (`runtime/motion/rewind.rs`).
    pub(super) rewind: Option<crate::rewind::Rewind>,
    #[cfg(debug_assertions)]
    pub(super) matched_warned: HashSet<Rc<str>>,
    /// Candidate nodes inspected by `end_unseen_exits`, for tests.
    #[cfg(test)]
    pub(in crate::runtime) unseen_exit_checks: u64,
}

impl MotionState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            flying: Vec::new(),
            flights: Vec::new(),
            caught_up: 0.0,
            slack_dt: 0.0,
            reveal_fps: 0.0,
            anim_sources: 0,
            loops_active: false,
            time_scaled: false,
            motion_needs_cause: false,
            loops: HashMap::default(),
            loop_wake: None,
            launches: Default::default(),
            grows: HashMap::default(),
            stacks: HashMap::default(),
            fold_headers: HashMap::default(),
            landing_holds: Default::default(),
            reveals: HashMap::default(),
            reveals_kept: Vec::new(),
            reveal_chain: RevealChain::default(),
            matched: Vec::new(),
            handovers: Vec::new(),
            loop_clock: Default::default(),
            motion_scratch: Default::default(),
            rewind: None,
            #[cfg(debug_assertions)]
            matched_warned: HashSet::default(),
            #[cfg(test)]
            unseen_exit_checks: 0,
        }
    }

    #[inline]
    pub(in crate::runtime) fn flights(&self) -> &[TextFlight] {
        &self.flights
    }

    #[inline]
    pub(in crate::runtime) fn flights_mut(&mut self) -> &mut Vec<TextFlight> {
        &mut self.flights
    }

    #[inline]
    pub(in crate::runtime) fn clear_flights(&mut self) {
        self.flights.clear();
    }

    #[inline]
    pub(in crate::runtime) fn add_caught_up(&mut self, step: f32) {
        self.caught_up += step;
    }

    #[inline]
    pub(in crate::runtime) fn begin_slack_step(&mut self, step: f32) {
        self.slack_dt = std::mem::take(&mut self.caught_up) + step;
    }

    #[inline]
    pub(crate) fn anim_sources(&self) -> u16 {
        self.anim_sources
    }

    #[inline]
    pub(in crate::runtime) fn set_anim_sources(&mut self, sources: u16) {
        self.anim_sources = sources;
    }

    #[inline]
    pub(in crate::runtime) fn add_anim_sources(&mut self, bits: u16) {
        self.anim_sources |= bits;
    }

    #[inline]
    pub(in crate::runtime) fn loops_active(&self) -> bool {
        self.loops_active
    }

    #[inline]
    pub(in crate::runtime) fn reset_loops_active(&mut self) {
        self.loops_active = false;
    }

    #[inline]
    pub(in crate::runtime) fn mark_loops_active(&mut self, active: bool) {
        self.loops_active |= active;
    }

    #[inline]
    pub(in crate::runtime) fn time_scaled(&self) -> bool {
        self.time_scaled
    }

    #[inline]
    pub(in crate::runtime) fn set_time_scaled(&mut self, scaled: bool) {
        self.time_scaled = scaled;
    }

    #[inline]
    pub(crate) fn enable_motion_needs_cause(&mut self) {
        self.motion_needs_cause = true;
    }

    #[inline]
    pub(in crate::runtime) fn motion_needs_cause(&self) -> bool {
        self.motion_needs_cause
    }

    #[inline]
    pub(in crate::runtime) fn loop_track(&self, id: NodeId) -> Option<&rune_motion::LoopTrack> {
        self.loops.get(&id)
    }

    #[inline]
    pub(in crate::runtime) fn has_loop(&self, id: NodeId) -> bool {
        self.loops.contains_key(&id)
    }

    #[inline]
    pub(in crate::runtime) fn loops_is_empty(&self) -> bool {
        self.loops.is_empty()
    }

    #[inline]
    pub(in crate::runtime) fn loops_len(&self) -> usize {
        self.loops.len()
    }

    #[inline]
    pub(in crate::runtime) fn loop_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.loops.keys().copied()
    }

    #[inline]
    pub(in crate::runtime) fn loop_active(&self, id: NodeId, node_time: f64) -> bool {
        self.loops.get(&id).is_some_and(|l| l.active(node_time))
    }

    #[inline]
    pub(in crate::runtime) fn loop_offset(&self, id: NodeId, time: f64, reduced_motion: bool) -> Option<[f32; 2]> {
        self.loops.get(&id)?.offset(time, reduced_motion)
    }

    #[inline]
    pub(in crate::runtime) fn loop_wake(&self) -> Option<(f64, f64)> {
        self.loop_wake
    }

    #[inline]
    pub(in crate::runtime) fn clear_loop_wake(&mut self) {
        self.loop_wake = None;
    }

    #[inline]
    pub(in crate::runtime) fn set_loop_wake(&mut self, wake: (f64, f64)) {
        self.loop_wake = Some(wake);
    }

    #[inline]
    pub(in crate::runtime) fn arm_loop_wake(&mut self, wake: f64, at: f64) {
        if self.loop_wake.is_none_or(|w| wake < w.0) {
            self.loop_wake = Some((wake, at));
        }
    }

    #[inline]
    pub(in crate::runtime) fn launches(&self) -> &flights::Launches {
        &self.launches
    }

    #[inline]
    pub(in crate::runtime) fn launches_mut(&mut self) -> &mut flights::Launches {
        &mut self.launches
    }

    #[inline]
    pub(in crate::runtime) fn insert_fold_header(&mut self, id: NodeId, header: String) {
        self.fold_headers.insert(id, header);
    }

    #[inline]
    pub(in crate::runtime) fn clear_landing_holds(&mut self) {
        self.landing_holds.clear();
    }

    #[inline]
    pub(in crate::runtime) fn holds_landing(&self, id: NodeId) -> bool {
        self.landing_holds.holds(id)
    }

    #[inline]
    pub(in crate::runtime) fn has_reveal(&self, id: NodeId) -> bool {
        !self.reveals.is_empty() && self.reveals.contains_key(&id)
    }

    #[inline]
    pub(in crate::runtime) fn reveal_fading(&self, id: NodeId) -> bool {
        self.reveals.get(&id).is_some_and(RevealPhase::fading)
    }

    #[inline]
    pub(in crate::runtime) fn reveal_caught_up(&self, id: NodeId, len: usize) -> bool {
        self.reveals.get(&id).is_some_and(|r| r.caught_up(len))
    }

    #[inline]
    pub(in crate::runtime) fn reveal_fade(&self, id: NodeId) -> Option<reveal_phase::Fade<'_>> {
        self.reveals.get(&id).and_then(RevealPhase::fade)
    }

    #[inline]
    pub(in crate::runtime) fn remove_reveal(&mut self, id: NodeId) {
        self.reveals.remove(&id);
    }

    #[inline]
    pub(in crate::runtime) fn has_matched(&self) -> bool {
        !self.matched.is_empty()
    }

    #[inline]
    pub(in crate::runtime) fn clear_handovers(&mut self) {
        self.handovers.clear();
    }

    #[inline]
    pub(in crate::runtime) fn handovers(&self) -> &[RecordedHandover] {
        &self.handovers
    }

    /// Records a matched handover for the current frame, collapsing chained commits (`A -> B -> C`
    /// into `A -> C`, or dropping `A -> B -> A`) within the same frame.
    pub(in crate::runtime) fn record_handover(&mut self, h: RecordedHandover) {
        if let Some(pos) = self.handovers.iter().position(|prev| prev.to_path == h.from_path) {
            if self.handovers[pos].from_path == h.to_path {
                self.handovers.remove(pos);
            } else {
                self.handovers[pos].to_path = h.to_path;
                self.handovers[pos].to_id = h.to_id;
                self.handovers[pos].start_rect = h.start_rect;
            }
            return;
        }
        self.handovers.push(h);
    }

    #[inline]
    pub(in crate::runtime) fn set_rewind(&mut self, rewind: Option<crate::rewind::Rewind>) {
        self.rewind = rewind;
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn reveals_map(&self) -> &HashMap<NodeId, RevealPhase> {
        &self.reveals
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn clear_reveals_for_test(&mut self) {
        self.reveals.clear();
        self.reveal_chain = RevealChain::default();
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn reveals_kept_len(&self) -> usize {
        self.reveals_kept.len()
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn reveal_chain_until(&self) -> f64 {
        self.reveal_chain.until()
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn matched_ids(&self) -> Vec<NodeId> {
        self.matched.iter().map(|m| m.1).collect()
    }

    /// Commits streaming word reveal state for node `id` during `apply`.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::runtime) fn commit_reveal(
        &mut self,
        id: NodeId,
        content: &str,
        fade: f32,
        now: f64,
        time_scaled: bool,
        was_present: bool,
        instant: bool,
        unprompted: bool,
        virtual_mount: bool,
        reduced_motion: bool,
    ) {
        use std::collections::hash_map::Entry;
        // A text that has never faded here starts level with its content, or at none when
        // it is entering. A state that outlived its node (an instant parent unmount keeps
        // the reveal, and its path, for the child's return) keeps its words.
        let mounted_late = !was_present && !instant && !unprompted && !virtual_mount;
        let (st, vacant) = match self.reveals.entry(id) {
            Entry::Occupied(st) => (st.into_mut(), false),
            Entry::Vacant(st) => {
                let shown = if mounted_late { 0 } else { content.len() };
                (st.insert(RevealPhase::Idle { shown }), true)
            }
        };
        let arrival = if instant || (!was_present && virtual_mount) || content.len() < st.shown() || !content.is_char_boundary(st.shown()) {
            Arrival::Replaced
        } else if !vacant {
            Arrival::Kept
        } else if mounted_late {
            Arrival::Entering
        } else {
            Arrival::Landed
        };
        let chain = if time_scaled { None } else { Some(&self.reveal_chain) };
        let effects = st.step(RevealEvent::Commit {
            content,
            fade,
            arrival,
            chain,
            now,
            reduced_motion,
        });
        for effect in effects {
            match effect {
                RevealEffect::Started(at) => self.reveal_chain.started(at),
            }
        }
    }

    /// Reconciles loop tracks after a commit, catching running loops up on the app clock first.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::runtime) fn sync_loops(
        &mut self,
        nodes: &[Node],
        scene: &Scene,
        t: f64,
        clock: f64,
        time_scale: f32,
        time_scaled: bool,
        any_loop: bool,
        land_loops: bool,
    ) {
        self.catch_loops_up(scene, t, clock, time_scale);
        if any_loop || !self.loops.is_empty() {
            for n in nodes {
                let t = if time_scaled { scene.store().node_time(n.id) } else { t };
                if let Some(&spec) = n.looping.as_deref() {
                    let times = n.cold.get().and_then(|c| c.loop_times);
                    let start = self.loop_start(scene, n.id, t);
                    let track = self.loops.entry(n.id).and_modify(|l| { l.retime_with_times(spec, times, t); l.enabled(true, t); })
                        .or_insert_with(|| match times {
                            Some(count) => rune_motion::LoopTrack::with_times(spec, count, start),
                            None => rune_motion::LoopTrack::new(spec, start),
                        });
                    if land_loops { track.finish(); }
                } else if let Some(l) = self.loops.get_mut(&n.id) {
                    l.enabled(false, t);
                }
            }
        }
    }

    /// Writes each active slide loop's current offset, and each moving sequence's translate
    /// and rotate (on top of `El::offset` and `El::rotate`), into `scene` before stepping tracks.
    ///
    /// A moving sequence writes while it plays. When it isn't playing it writes only when the
    /// channel's target isn't the pose it holds: a waiting one-shot holds its first beat (so a
    /// change lands there at once), and a finished one rests on its last beat, which is where
    /// commit aims `El::offset` and `El::rotate` (`Sequence::rest`), so later changes spring. A
    /// repeating sequence that hasn't started holds its first beat, its rest too. A dropped
    /// sequence writes nothing, and commit springs the element home. A channel a live binding
    /// owns (`El::live_offset`, `El::live_rotation`) is left alone.
    pub(in crate::runtime) fn step_loops_into_scene(
        &self,
        nodes: &[Node],
        node_index: &NodeIndex,
        scene: &mut Scene,
        reduced_motion: bool,
    ) {
        for (id, track) in &self.loops {
            let now = scene.store().node_time(*id);
            if let Some([x, y]) = track.offset(now, reduced_motion)
                && let Some(n) = node_index.get(id).map(|&i| &nodes[i])
            {
                let offset = rune_motion::Vec2::new(x * n.local.w, y * n.local.h);
                if scene.get(*id, prop::OFFSET) != Some(offset) {
                    scene.set(*id, prop::OFFSET, offset);
                }
            } else if let Some(([x, y], turn)) = track.motion(now, reduced_motion)
                && let Some(n) = node_index.get(id).map(|&i| &nodes[i])
            {
                let playing = track.active(now);
                let live = |p: crate::live::Property| n.live.iter().any(|b| b.property == p);
                if !live(crate::live::Property::Offset) {
                    let offset = rune_motion::Vec2::new(n.vis.offset[0] + x, n.vis.offset[1] + y);
                    let due = if playing { (!scene.store().is_animating(*id, prop::OFFSET)).then(|| scene.get(*id, prop::OFFSET)).flatten() } else { scene.store().target(*id, prop::OFFSET) };
                    if due != Some(offset) {
                        scene.set(*id, prop::OFFSET, offset);
                    }
                }
                if !live(crate::live::Property::Rotation) {
                    let turn = n.vis.rotate + turn;
                    let due = if playing { (!scene.store().is_animating(*id, prop::ROTATION)).then(|| scene.get(*id, prop::ROTATION)).flatten() } else { scene.store().target(*id, prop::ROTATION) };
                    if due != Some(turn) {
                        scene.set(*id, prop::ROTATION, turn);
                    }
                }
            }
        }
    }

    /// Drops landed matched/launch flights from `flying` and `launches`; returns whether any left.
    pub(in crate::runtime) fn prune_landed_flights(&mut self, scene: &Scene) -> bool {
        if self.flying.is_empty() {
            return false;
        }
        let before = self.flying.len();
        // Screen travel ends once it lands and no ancestor still moves. An exiting flight
        // stays lifted until it's gone.
        self.flying.retain(|(id, _)| scene.store().has(*id, prop::TRAVEL)
            || matches!(scene.phase(*id), Some(PresencePhase::Exiting { .. })));
        if self.flying.len() != before {
            self.launches.keep(&self.flying);
            true
        } else {
            false
        }
    }

    /// Keeps at most the newest `limit` reveal states for texts that have left `node_index`,
    /// dropping the oldest first.
    pub(in crate::runtime) fn cap_reveals(&mut self, node_index: &NodeIndex, limit: usize) {
        if self.reveals.is_empty() {
            self.reveals_kept.clear();
            return;
        }
        // A text's reveal state and the id its path maps to outlive the node, so a keyed child
        // that comes back under its old path keeps its words (r349). Nodes dropped during an
        // instant parent unmount leave their reveal behind, so bound those orphans here.
        let mut kept = std::mem::take(&mut self.reveals_kept);
        kept.retain(|id| self.reveals.contains_key(id) && !node_index.contains_key(id));
        let mut gone: Vec<NodeId> = self.reveals.keys().copied()
            .filter(|id| !node_index.contains_key(id) && !kept.contains(id))
            .collect();
        if !gone.is_empty() {
            gone.sort_unstable_by(|a, b| b.cmp(a));
            gone.append(&mut kept);
            kept = gone;
        }
        if kept.len() > limit {
            for id in kept.split_off(limit) { self.reveals.remove(&id); }
        }
        self.reveals_kept = kept;
    }

    /// Prunes loop, reveal, flying and launch entries for detached nodes.
    pub(in crate::runtime) fn prune_detached(
        &mut self,
        scene: &Scene,
        node_index: &NodeIndex,
        reveals_limit: usize,
    ) {
        if !self.loops.is_empty() {
            self.loops.retain(|id, _| scene.contains(*id));
        }
        self.cap_reveals(node_index, reveals_limit);
        // A flight that left the tree stays lifted and covered while it exits, so it doesn't pop.
        self.flying.retain(|(id, _)| node_index.contains_key(id));
        self.launches.keep(&self.flying);
    }

    #[inline]
    pub(in crate::runtime) fn reveal_fps(&self) -> f32 {
        self.reveal_fps
    }

    #[inline]
    pub(in crate::runtime) fn set_reveal_fps(&mut self, fps: f32) {
        self.reveal_fps = crate::app::RuntimeConfig::<()>::normalize_reveal_fps(fps);
    }

    /// Whether any reveal still has fading words on a running clock.
    #[inline]
    pub(in crate::runtime) fn has_fading_reveals(&self, scene: &Scene) -> bool {
        if self.reveals.is_empty() {
            return false;
        }
        let store = scene.store();
        self.reveals.iter().any(|(id, st)| st.fading() && store.node_time_scale(*id) > 0.0)
    }

    /// Advances all reveal phases against `scene`'s node clocks; returns whether any are still fading.
    pub(in crate::runtime) fn reveals_active(&mut self, scene: &Scene) -> bool {
        let store = scene.store();
        let mut any = false;
        for (id, st) in self.reveals.iter_mut() {
            let now = store.node_time(*id);
            st.step(RevealEvent::Tick { now });
            any |= st.fading() && store.node_time_scale(*id) > 0.0;
        }
        any
    }

    /// Presented content extent of node `c` if it is a growth stack (points).
    #[inline]
    pub(in crate::runtime) fn stack_extent(&self, nodes: &[Node], c: usize) -> Option<f64> {
        let n = &nodes[c];
        n.growth.as_ref()?;
        let stack = self.stacks.get(&n.id)?;
        let mut count = 0usize;
        let rows: f64 = stack.order.iter().filter_map(|id| self.grows.get(id)).map(|g| { count += 1; f64::from(g.h) }).sum();
        let gaps = f64::from(stack.gap) * count.saturating_sub(1) as f64;
        Some(f64::from(stack.pad_top) + rows + gaps + f64::from(stack.pad_bottom))
    }
}

/// The (x, y) scroll offsets of node `i`'s scroll viewports, innermost first.
pub(in crate::runtime) fn scroll_offsets<'a>(scrolls: &'a HashMap<NodeId, ScrollState>, nodes: &'a [Node], i: usize) -> impl Iterator<Item = [f32; 2]> + 'a {
    std::iter::successors(Some(i), |&p| nodes[p].parent.filter(|_| nodes[p].above.is_none()))
        .skip(1)
        .filter_map(|p| scrolls.get(&nodes[p].id).map(|st| [st.x.shift() as f32, st.offset as f32]))
}

/// Marks every ancestor of a marked node.
pub(in crate::runtime) fn mark_ancestors(nodes: &[Node], marked: &mut [bool]) {
    for i in 0..nodes.len() {
        if !marked[i] { continue; }
        let mut p = nodes[i].parent;
        while let Some(j) = p {
            if marked[j] { break; }
            marked[j] = true;
            p = nodes[j].parent;
        }
    }
}

impl RuntimeCore {
    pub(in crate::runtime) fn set_or_anim<T: rune_motion::Animatable>(&mut self, id: NodeId, p: Prop<T>, val: T, s: Spring, instant: bool) {
        if instant {
            self.scene.set(id, p, val);
        } else {
            self.scene.animate(id, p, val, s);
        }
    }

    pub(in crate::runtime) fn color(&self, id: NodeId, prop: Prop<Color>, rest: Color) -> Color {
        if self.scene.store().is_animating(id, prop) { self.scene.get(id, prop).unwrap_or(rest) } else { rest }
    }

    /// Shadow `k` of node `id` with its geometry where its spring has it.
    pub(in crate::runtime) fn shadow_at(&self, id: NodeId, k: usize, sh: &crate::el::Shadow) -> crate::el::Shadow {
        match P_SHADOW_GEO.get(k) {
            Some(&prop) if self.scene.store().active_len() > 0 && self.scene.store().is_animating(id, prop) => match self.scene.get(id, prop) {
                Some(g) => crate::el::Shadow { x: g.x, y: g.y, blur: g.w.max(0.0), spread: g.h, ..*sh },
                None => *sh,
            },
            _ => *sh,
        }
    }

    pub(in crate::runtime) fn step_growth(&mut self, dt: f32) -> bool {
        if self.motion.stacks.is_empty() {
            return false;
        }
        // Frame time, stepped in whole substeps and the rest (`growth::substeps`).
        let steps = crate::growth::substeps(dt);
        let now = self.t;
        let mut moving = false;
        let mut placed = false;
        let (mut restored, mut gave) = (Vec::new(), Vec::new());
        for (cid, stack) in self.motion.stacks.iter_mut() {
            let Some(&ci) = self.tree.node_index.get(cid) else { continue };
            let Some(spec) = self.tree.nodes[ci].growth.as_deref().copied() else { continue };
            let n_rows = stack.order.len();
            let mut st_growing = !stack.pending_heights.is_empty();
            let mut st_moving = st_growing;
            let (in_runs, run_grown) = MotionState::step_growth_runs(stack, &mut self.motion.grows, steps, spec);
            stack.grown += run_grown;
            for (k, id) in stack.order.iter().enumerate() {
                let Some(g) = self.motion.grows.get_mut(id) else { continue };
                let h0 = g.h;
                let in_run = in_runs.contains(id);
                if let Some(&at) = stack.closing.get(id)
                    && now >= at
                {
                    if self.reduced_motion {
                        *g = GrowTrack::settled(0.0);
                    } else {
                        g.retarget(0.0);
                    }
                }
                // A row above the tail (the last row) carries it.
                let carrying = k + 2 == n_rows;
                let mut left = dt.min(1.0);
                for step in steps.filter(|_| !in_run) {
                    if !stack.pending_heights.is_empty() {
                        let at = now - f64::from(left);
                        if !stack.width_hold.defers(at) && let Some(height) = stack.pending_heights.remove(id) {
                            g.retarget(height);
                        }
                    }
                    g.step_by(step, spec.spring, &spec.edge, carrying);
                    left -= step;
                }
                // A row nobody saw last frame draws no feather: it has none to ease, and grows one
                // again as it comes into view.
                if g.mask > 0.0 && self.tree.built
                    && self.tree.node_index.get(id).is_some_and(|&i| !self.paint.was_seen(i, true))
                {
                    g.mask = 0.0;
                }
                let r = std::mem::take(&mut g.restore);
                if r > 0.0 { restored.push((ci, *id, r)); }
                if let Some(share) = g.give { gave.push((*id, (h0 - g.h) * share)); }
                if !g.moving() { g.give = None; }
                stack.grown += (g.h - h0).max(0.0);
                st_growing |= g.target > g.h + 1.0 / 128.0;
                st_moving |= g.moving() || (g.h - h0).abs() > 1e-5;
                st_moving |= stack.slides.step(*id, g, spec.spring, steps);
            }
            stack.order.retain(|id| {
                let keep = !stack.closing.contains_key(id) || self.motion.grows.get(id).is_some_and(|g| g.h > 0.01 || g.target > 0.0);
                if !keep {
                    self.motion.grows.remove(id);
                }
                keep
            });
            stack.closing.retain(|id, _| stack.order.contains(id));
            stack.slides.keep(&stack.order);
            stack.growing = st_growing;
            stack.moving = st_moving;
            placed |= stack.handoff.place(&mut self.motion.grows, &mut self.scene, &self.tree.nodes, &self.tree.node_index, &mut self.scroll.scrolls);
            // Keep the shared clock awake through the hold. No timer thread or remeasurement.
            moving |= st_moving;
        }
        // Growth steps after Scene::frame. Scroll follow and painting must see this step's
        // shared edge, even in a viewport with no scroll-offset callback to present it.
        if placed { self.scene.present(); }
        for (ci, id, r) in restored { self.restore_above(ci, id, r); }
        for (id, d) in gave { self.lower_hold(id, d); }
        moving
    }

    /// Presented content extent of node `c` if it is a growth stack (points).
    #[inline]
    pub(in crate::runtime) fn stack_extent(&self, c: usize) -> Option<f64> {
        self.motion.stack_extent(&self.tree.nodes, c)
    }

    /// How far scroll viewport `i`'s content reaches past its bottom, negative while it fits.
    pub(in crate::runtime) fn scroll_overflow(&self, i: usize) -> f64 {
        let view = self.tree.nodes[i].target;
        let content_h = self.scroll_children(i).map(|c| {
            let n = &self.tree.nodes[c];
            f64::from(n.target[1] - view[1]) + self.stack_extent(c).unwrap_or(f64::from(n.target[3] - n.target[1]))
        }).reduce(f64::max).unwrap_or(0.0);
        content_h - f64::from(view[3] - view[1])
    }

    pub(in crate::runtime) fn step_scroll(&mut self, dt: f32) -> bool { self.step_scroll_by(dt, false) }

    /// Steps every scroll viewport `dt` seconds on: a frame's own step, or (`catch_up`) the time
    /// it charges to motion that was already running (`frame::clock::split`).
    pub(in crate::runtime) fn step_scroll_by(&mut self, dt: f32, catch_up: bool) -> bool {
        let slack_dt = std::mem::take(&mut self.motion.slack_dt);
        let scale = f64::from(self.env.scale * self.zoom());
        // Motion reduced mid-ease: a wheel's ease lands where it was going, at once.
        let land_wheels = !self.wheel_glides();
        let mut moving = false;
        for i in 0..self.tree.nodes.len() {
            let Some(sp) = self.tree.nodes[i].scroll else { continue };
            if self.subtree_exiting(i) { continue; }
            let id = self.tree.nodes[i].id;
            let axes = self.tree.nodes[i].scroll_axes;
            if axes.x() { moving |= self.step_scroll_x(i, dt, catch_up); }
            // A horizontal viewport doesn't move vertically, even with content taller than it.
            let overflow = if axes.y() { self.scroll_overflow(i) } else { 0.0 };
            if let Some(active) = self.step_touch_scroll(id, overflow, dt, false) {
                moving |= active;
                continue;
            }
            let held = !self.scroll.requests.is_empty() && {
                let path = &self.tree.nodes[i].path;
                self.scroll.requests.owns(path)
                    && self.scroll_stack(i).and_then(|c| self.motion.stacks.get(&self.tree.nodes[c].id)).is_some_and(|s| s.growing)
            };
            let laid = if axes.y() { self.ease_to_shrunk_end(i, overflow) } else { None };
            let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
            let _ = scroll_mode::step(&mut st.mode, ModeEvent::Step { x_v: st.x.coast() });
            st.max = overflow.max(0.0);
            match st.mode {
                ScrollMode::Pinned(_) if sp.stick_bottom => {
                    let before = st.offset;
                    // A reader stuck on a shrinking stack's end doesn't rise with its rows' last fraction.
                    st.offset = laid.filter(|&end| st.offset <= end + 1e-6).map_or(st.max, |end| st.max.min(end));
                    if (st.offset - before).abs() > 1e-6 { st.note_source(crate::ScrollSource::Programmatic); }
                }
                ScrollMode::Glide { .. } => {
                    let before = st.offset;
                    moving |= scroll_bottom::glide(st, dt);
                    if (st.offset - before).abs() > 1e-6 { st.note_source(crate::ScrollSource::Programmatic); }
                }
                ScrollMode::Momentum { v, .. } if v.abs() > 1.0 => {
                    let before = st.offset;
                    // Exact for exponential decay, so a long frame carries the same distance as
                    // the short frames it stands for.
                    let decay = (-SCROLL_DECAY * dt).exp();
                    st.offset += f64::from(v * (1.0 - decay) / SCROLL_DECAY);
                    let edge = st.offset <= 0.0 || st.offset >= st.max;
                    st.mode = if edge { st.mode.to_rest() } else { st.mode.to_momentum(v * decay) };
                    if (st.offset - before).abs() > 1e-6 { st.note_source(crate::ScrollSource::Momentum); }
                    moving = true;
                }
                ScrollMode::Momentum { .. } => st.mode = st.mode.to_rest(),
                ScrollMode::Wheel { mut curve, .. } => {
                    let before = st.offset;
                    curve.follow(st.offset);
                    // An end that shrank under the ease takes its target with it; one that grew does
                    // too when the wheel was sent to the end of a viewport that follows its bottom
                    // (a reply streaming in), so it lands there and pins.
                    if curve.target() > st.max || (sp.stick_bottom && curve.to_end() && curve.target() < st.max) { curve.retarget(st.max, scale); }
                    if land_wheels { curve.advance(f64::INFINITY, false) } else { curve.advance(f64::from(dt), catch_up) }
                    // On whole device pixels, as Chromium's scroll offsets are: everything inside
                    // moves by the same whole pixels each frame and draws as it did, rather than
                    // landing at a new fraction of a pixel every frame.
                    st.offset = scroll_wheel::on_grid_or_end(curve.value(), st.max, scale);
                    curve.applied(st.offset);
                    // The hand moved it: the app hears a wheel's ease as the wheel.
                    st.moved();
                    if (st.offset - before).abs() > 1e-6 { st.note_source(crate::ScrollSource::Direct); }
                    // An ease sent to a following viewport's end pins the frame it gets there, though
                    // the end growing under it restarts its segment every frame a reply grows.
                    let pins = sp.stick_bottom && curve.to_end() && st.offset == st.max;
                    if curve.done() || pins { st.land(sp.stick_bottom) } else { st.mode = st.mode.to_wheel(curve); moving = true; }
                }
                ScrollMode::Pinned(_) | ScrollMode::Rest(_) => {}
            }
            let clamped = st.offset.clamp(0.0, st.max);
            if (clamped - st.offset).abs() > 1e-6 {
                st.offset = clamped;
                st.note_source(crate::ScrollSource::Programmatic);
            }
            // Landed on a shrinking stack's new end: stick once its rows are within half a point
            // of it, not while they're still short of it. A glide that reaches the end hands it
            // to the pin.
            let shrinking = laid.is_some_and(|end| end < st.max - 0.5 && st.offset < st.max - 1e-3);
            if sp.stick_bottom && !held && !shrinking && st.glide().is_none() && st.mode.wheel().is_none() && st.offset >= st.max - 0.5 && st.coast().abs() <= 1.0 {
                st.pin();
            }
            let _ = scroll_mode::step(&mut st.mode, ModeEvent::Laid(laid));
            if sp.stick_bottom && axes.y() && slack_dt > 0.0 { moving |= self.step_slack(i, slack_dt); }
        }
        moving
    }
}

#[cfg(test)]
#[path = "tracks_tests.rs"]
mod tests;
