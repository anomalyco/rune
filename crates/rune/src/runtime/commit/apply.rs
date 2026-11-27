//! Applying a completed measurement pass to `Scene`, growth stacks, reveals, scroll viewports,
//! live bindings and retained exits.
use super::*;

mod exits;
mod growth;

impl RuntimeCore {
    /// The mutation phase cannot accept an El or an unfinished measurement pass.
    /// Returns whether `sync_modals` or `sync_follows` ran a focus-change callback that dirtied
    /// state (or latched a follow-up focus request), so `commit` can reflect it in the same frame.
    pub(in crate::runtime) fn apply(&mut self, app: &mut dyn AppDriver, measured: Measured, tx: &Tx) -> bool {
        let _apply_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Apply);
        let Measured { layout: mut taffy, mut nodes, reduced_motion, time_scale, defaults, titlebar, caret_read, wm_state_read, primary_pointer_read, memo_flattened, memo_reused, taken_paths, above_fit_styles, .. } = measured;
        let rebound_press = self.rebound_press_path(&nodes);
        self.commit.defaults = defaults;
        if crate::tooling::interaction::active() { self.interaction_reused(&memo_reused); }
        let mut scratch = std::mem::take(&mut self.commit.apply_scratch);
        self.edit.caret_watch = caret_read;
        if let Some(read) = wm_state_read { self.host.wm_state_read = read; }
        if let Some(read) = primary_pointer_read { self.host.primary_pointer_read = read; }
        let reduced_motion_unchanged = reduced_motion == self.reduced_motion;
        if !reduced_motion_unchanged {
            self.reduced_motion = reduced_motion;
            self.scene.set_reduced_motion(reduced_motion);
        }
        self.time_scale = time_scale;
        if tx.commit.lands() { self.motion.clear_flights(); }
        let scale = self.env.scale;
        // Culling is not a model removal. Retire offscreen scene/growth state immediately;
        // re-entering historical rows mount settled and don't replay streaming words.
        if !self.commit.virtual_culled.is_empty() {
            scratch.culled.clear();
            scratch.culled.resize(self.tree.nodes.len(), false);
            for (i, n) in self.tree.nodes.iter().enumerate() {
                if self.commit.virtual_culled.contains(&n.path) || n.parent.is_some_and(|p| scratch.culled[p]) {
                    scratch.culled[i] = true;
                    self.scene.unmount(n.id);
                    self.motion.grows.remove(&n.id);
                }
            }
            for path in &self.commit.virtual_culled {
                if let Some(id) = self.tree.path_ids.get(path) {
                    self.scene.unmount(*id);
                    self.motion.grows.remove(id);
                }
            }
        }
        // A path reused by a non-virtual element is no longer a virtual list; drop its stale
        // metadata before `apply` shifts child frames or skips spans against `v.before`.
        // Lists removed from `nodes` stay until their exiting node prunes.
        if !self.commit.virtuals.is_empty() {
            let Self { commit, tree, .. } = self;
            commit.virtuals.retain(|path, _| {
                !tree.path_ids.get(path)
                    .and_then(|id| tree.node_index.get(id))
                    .and_then(|&i| nodes.get(i))
                    .is_some_and(|n| !n.virtual_list)
            });
        }
        for v in self.commit.virtuals.values_mut() {
            v.shown = v.keys[v.range.clone()].iter().cloned().collect();
            v.mounted = v.indexed.then(|| crate::el::virtual_list::Mounted::of(v));
        }
        self.commit.virtual_items.commit_staged();
        // A row scrolled into view mounts settled with its subtree. Content added later under a
        // row already on screen is a live change and keeps its own entrance.
        // Parents precede children, so a remounting row passes its flag down its whole subtree.
        for i in 0..nodes.len() {
            if let Some(p) = nodes[i].parent {
                nodes[i].virtual_mount |= nodes[p].virtual_mount && !self.scene.contains(nodes[p].id);
            }
        }
        let first = self.commit.commits == 0;
        // The first frame's own reads, for the next launch's first view (`layout_seed`).
        if first { self.commit.layout_first = self.layout_seed(); }
        self.commit.commits += 1;
        if first || self.commit.watch.cause != CommitWhy::VIRTUAL {
            self.commit.non_virtual_commits += 1;
        }
        self.stats.commits = self.commit.commits;
        // Concealing is a security boundary: old geometry must not disclose the revealed
        // line count through a height spring, displaced siblings, or a lingering scroll glide.
        let revealed: HashSet<_> = self.tree.nodes.iter()
            .filter(|n| {
                n.input.as_ref().is_some_and(|input| input.edit.masking() == crate::edit::Masking::Shown)
                    || (n.input.is_none() && n.text.as_ref().is_some_and(|t| t.redacted && t.cold.secure_revealed))
            })
            .map(|n| n.id).collect();
        let mut concealed = std::mem::take(&mut scratch.concealed);
        concealed.clear();
        let concealing = if !revealed.is_empty() {
            concealed.resize(nodes.len(), false);
            for (i, n) in nodes.iter().enumerate() {
                let hidden_now = n.input.as_ref().is_some_and(|input| input.edit.is_hidden_multiline())
                    || (n.input.is_none() && n.text.as_ref().is_some_and(|t| t.redacted && !t.cold.secure_revealed));
                if hidden_now && revealed.contains(&n.id) {
                    concealed[i] = true;
                }
            }
            mark_ancestors(&nodes, &mut concealed);
            concealed.iter().any(|&hidden| hidden)
        } else {
            false
        };
        let instant = tx.commit.snaps() || first || concealing;
        // What this commit mounts or unmounts, and rows new to a growth stack, appear and go at
        // once when no input caused it (`Tx::unprompted`); what stays keeps its motion.
        let cut = instant || tx.unprompted;
        if tx.commit.snaps() && !first { self.commit.commit_why |= crate::runtime::CommitWhy::INSTANT; }
        // An instant change lands what it changes; motion an earlier change set going keeps on.
        let keeps_flights = tx.commit.snaps() && !tx.commit.lands() && !first && !concealing;
        self.scene.keep_flights(keeps_flights);
        let now = self.t;
        if self.host.content_sizing.fits_any() && let Some(root) = nodes.first() { self.fit_window((root.local.w, root.local.h), tx, first); }
        let mut held = std::mem::take(&mut scratch.held);
        let holds = self.motion.landing_holds(&nodes, &self.tree.nodes, self.paint.seen(), &self.scene, !instant && !tx.relayout && !tx.window_step, self.reduced_motion, &mut held);

        // What drew nothing last frame has no exit to show (`unseen_presence.rs`).
        let any_gone = self.tree.nodes.iter().any(|o| !self.tree.node_index.contains_key(&o.id));
        let any_replaced = self.commit.virtuals.values().any(crate::el::virtual_list::VirtualList::replaced_view);
        let drew_nothing = if any_gone || any_replaced { self.drew_nothing() } else { Vec::new() };
        let unseen_ids: HashSet<NodeId> = if self.commit.virtuals.is_empty() || drew_nothing.is_empty() { HashSet::default() } else {
            self.tree.nodes.iter().zip(&drew_nothing).filter(|&(_, &u)| u).map(|(n, _)| n.id).collect()
        };

        let mut live_by_id = HashMap::default();
        // Growth stacks: rows of a stack that existed before grow; a new stack mounts settled.
        if let Some(start) = nodes.iter().position(|n| n.cold.get().is_some_and(|nd| nd.growth.is_some())) {
            self.apply_growth_stacks(
                start, &nodes, &taffy, tx, &mut scratch, &held, &unseen_ids,
                first, concealing, instant, cut, now, scale, &mut live_by_id,
            );
        }

        // Put nodes on their subtree's clock before this commit animates them.
        let time_scaled = self.motion.time_scaled() || nodes.iter().any(|n| n.time_scale != 1.0);
        let mut time_scales = std::mem::take(&mut scratch.time_scales);
        time_scales.clear();
        if time_scaled { self.motion.sync_time_scales(&mut self.scene, &nodes, &mut time_scales, false); }

        let can_skip_apply = !first && !tx.commit.lands() && !concealing && reduced_motion_unchanged && !self.scene.tracing();
        let mut skip_spans = std::mem::take(&mut self.commit.memo.skip_scratch);
        self.commit.clean_apply_spans(&self.tree, &self.scene, &self.motion, &nodes, &memo_reused, can_skip_apply, |i| self.reorder_jump(&nodes, i), &mut skip_spans);
        let unseen = self.unseen_before_and_after(&nodes, &skip_spans);
        // `El::matched` elements of this tree: (id, index, newly mounted).
        let mut arrivals = std::mem::take(&mut scratch.arrivals);
        arrivals.clear();
        let mut mounted = std::mem::take(&mut scratch.mounted);
        mounted.clear();
        // A flight this commit would re-aim that has as good as landed lands instead and rides
        // its parent, rather than set off again on a path that doesn't.
        let landing = if self.motion.flying.is_empty() { Vec::new() } else { self.motion.landing_flights(&self.scene, self.env.scale) };
        let mut landed = false;
        // Nodes this commit slid from where they were drawn, or under an ancestor that slid.
        let mut slid = std::mem::take(&mut scratch.slid);
        slid.clear();
        // How far the top of each node's contents moved down in its growth row (drawn less new).
        let mut top_moved = std::mem::take(&mut scratch.top_moved);
        top_moved.clear();
        let has_virtuals = !self.commit.virtuals.is_empty();
        let mut last = paint_tracks::Finder::default();
        let mut skip_cursor = 0usize;
        // A crossing carries its paint and exits; a resizing row carries its inset. Its
        // stationary neighbour only rounds a changed corner, not its opacity or contents.
        let mut live_row = None;
        let mut exit_parents = HashSet::default();
        let mut i = 0usize;
        while i < nodes.len() {
            while skip_cursor < skip_spans.len() && skip_spans[skip_cursor].new_start + skip_spans[skip_cursor].len <= i {
                skip_cursor += 1;
            }
            if let Some(span) = skip_spans.get(skip_cursor).filter(|s| s.new_start == i) {
                for k in 0..span.len {
                    nodes[span.new_start + k].scale_track = self.tree.nodes[span.old_start + k].scale_track;
                }
                last.advance(span.old_start + span.len);
                i += span.len;
                continue;
            }
            let cur_i = i;
            i += 1;
            let i = cur_i;
            if live_row.is_some_and(|(root, _)| nodes[i].parent.is_none_or(|p| p < root)) { live_row = None; }
            let id = nodes[i].id;
            let mut scene_rect = nodes[i].local;
            let parent_virtual = if has_virtuals {
                nodes[i].parent.and_then(|p| self.commit.virtuals.get(&nodes[p].path))
            } else {
                None
            };
            if let Some(v) = parent_virtual {
                scene_rect.y -= v.before;
            }
            if !live_by_id.is_empty() && let Some(&row) = live_by_id.get(&id) { live_row = Some((i, row)); }
            let shift = live_row.and_then(|(_, row)| row.shift);
            let riding = live_row.is_some_and(|(_, row)| row.ink && row.shift.is_some_and(|jump| jump != 0.0)) && !nodes[i].instant_changes;
            if riding && nodes[i].vis.child_exit.is_some() { exit_parents.insert(id); }
            let old = last.find(&self.tree.nodes, id);
            if can_skip_apply
                && slid.is_empty()
                && top_moved.is_empty()
                && paint_tracks::unchanged_node_resting(&self.tree, &self.scene, &self.motion, &nodes, i, old, scene_rect, parent_virtual, || self.reorder_jump(&nodes, i))
            {
                continue;
            }
            let is_virtual = has_virtuals && self.commit.virtuals.contains_key(&nodes[i].path);
            let parent_id = nodes[i].parent.map(|p| nodes[p].id);
            let (spring_l, anim_l) = tx.motion.map_or_else(
                || { let spring = nodes[i].motion; (spring, spring.into()) },
                |m| (m.spring(), m.anim()),
            );
            let spring_c = tx.color_motion.unwrap_or(nodes[i].motion);
            // Text leads its background so theme crossfades never dip through zero contrast; a
            // colour change asked to land at once lands at once on text too.
            let spring_tc = if spring_c == Spring::NONE { spring_c } else { Spring::new((spring_c.duration * 0.5).max(0.1), spring_c.bounce) };
            let row_like = nodes[i].in_row == RowLayout::Snaps || nodes[i].parent.is_some_and(|p| nodes[p].growth.is_some());
            // An `El::instant_changes` element lands its changes, and those of what it holds;
            // its own entrance and exit, and colours, stay with the commit, as do the entrances of
            // an `El::animated_changes` subtree inside it. `El::enter_in_instant` lets a new
            // element's entrance play during a `Tx::instant` commit while layout and exits still
            // snap, and first frames, `Tx::settle`, concealing and unprompted commits still land.
            let cut_for_enter = if nodes[i].enter_in_instant {
                first || concealing || tx.commit.lands() || tx.unprompted
            } else {
                cut
            };
            let enters_at_once = cut_for_enter || (nodes[i].parent.is_some_and(|p| nodes[p].instant_changes) && nodes[i].instant_changes);
            let lands = instant || nodes[i].instant_changes;
            let layout_instant = lands || row_like || tx.relayout || is_virtual
                || (!self.tree.aspect_snap.is_empty() && self.tree.aspect_snap.contains(&id));

            let was_present = self.scene.contains(id);
            let was_exiting = self.is_exiting(id);
            if was_present && !was_exiting && !enters_at_once && !unseen_ids.is_empty() { self.reenter_unseen_row(&nodes, i, &unseen_ids); }
            if let Some((key, _)) = &nodes[i].matched {
                arrivals.push((key.clone(), i, !was_present || was_exiting));
            }
            if !was_present || was_exiting {
                // Presence belongs to the transition. Layout tuning must not replace
                // the caller's enter/exit springs when this node mounts or revives.
                let tr = nodes[i].transition.as_deref().copied().unwrap_or_else(Transition::opacity);
                // Revive presence first, then let this transaction own the layout retarget.
                // Scene::insert otherwise starts its default .4 s reflow before our .25 s
                // menu curve; the equal target makes the intended retarget a no-op.
                let mount_rect = if was_exiting {
                    self.scene.store().target(id, prop::FRAME).unwrap_or(scene_rect)
                } else { scene_rect };
                let mut spec = NodeSpec::new(mount_rect)
                    .opacity(nodes[i].vis.opacity)
                    .radius(Corners(nodes[i].vis.radii()))
                    .transition(tr);
                if let Some(pid) = parent_id {
                    spec = spec.parent(pid);
                }
                // Growth carries a row's entrance; an explicit transition adds to it. Under
                // `animate_layout`, contents new to a row already in place enter on their own.
                if !was_present {
                    if mounted.is_empty() { mounted.resize(nodes.len(), false); }
                    mounted[i] = true;
                }
                let rides_row = row_like || (nodes[i].in_row == RowLayout::Horizontal && nodes[i].parent.is_some_and(|p| mounted.get(p).copied().unwrap_or(false)));
                let quiet_row = rides_row
                    && nodes[i].transition.as_ref().is_none_or(|t| t.insertion == rune_motion::Effect::IDENTITY)
                    && !was_exiting
                    && !self.reduced_motion;
                let mount = if enters_at_once || nodes[i].virtual_mount || quiet_row || self.enters_unseen(&nodes, &unseen, i) { Mount::Initial } else { Mount::Animated };
                match nodes[i].transition_motion.as_deref() {
                    Some(&(enter, exit)) => self.scene.set_transition_motion(id, enter, exit),
                    None => self.scene.clear_transition_motion(id),
                }
                self.scene.insert_delayed(id, spec, mount, nodes[i].vis.enter_delay);
            } else if tx.commit.lands() { self.scene.land_waiting(id); }
            self.scene.set_ride_trailing(id, nodes[i].vis_flags.ride_trailing());
            // How far a growth or virtual row's place jumps in this commit (drawn minus new).
            let (mut row_jump, mut reorder_jump) = (0.0, 0.0);
            if was_present && !was_exiting && !lands
                && landing.contains(&id)
                && self.motion.flying.iter().any(|f| f.0 == id && (f.1 != nodes[i].local || nodes[i].parent.is_some_and(|p| slid.get(p).copied().unwrap_or(false))))
            {
                self.motion.land_flight(&mut self.scene, id);
                landed = true;
            }
            if was_present {
                // As in SwiftUI, a node leaves with the transition of its last render.
                if !was_exiting {
                    self.scene.set_transition(id, nodes[i].transition.as_deref().copied().unwrap_or_else(Transition::opacity));
                    match nodes[i].transition_motion.as_deref() {
                        Some(&(enter, exit)) => self.scene.set_transition_motion(id, enter, exit),
                        None => self.scene.clear_transition_motion(id),
                    }
                }
                let stack_row = nodes[i].parent.is_some_and(|p| nodes[p].growth.is_some()) || is_virtual
                    || (nodes[i].in_row == RowLayout::Horizontal && (self.tree.aspect_snap.is_empty() || !self.tree.aspect_snap.contains(&id)));
                // A window's own resize step moves content at rest with the window; motion
                // already running retargets on its spring.
                let follows_window = tx.window_step && !self.motion.flying.iter().any(|f| f.0 == id)
                    && !self.scene.store().is_animating(id, prop::FRAME);
                // Out of sight before and after: nobody would see it move.
                let out_of_sight = unseen.get(i).copied().unwrap_or(false) && nodes[i].matched.is_none()
                    && !self.motion.flying.iter().any(|f| f.0 == id);
                if !out_of_sight && !nodes[i].instant_changes && live_row.is_some_and(|(root, _)| root == i) {
                    reorder_jump = shift.unwrap_or(0.0);
                }
                let frame_motion = nodes[i].frame_motion.as_deref().copied();
                let layout_anim = if out_of_sight || held.get(i).copied().unwrap_or(false) {
                    LayoutAnim::INSTANT
                } else if stack_row && !lands && !tx.relayout && !follows_window {
                    // Growth and virtual windows own a row's vertical place; its horizontal place
                    // still springs, or a re-centred column jumps against its moving parent. So
                    // do `animate_layout` contents inside the row.
                    // A flying row's path restarts from where it is drawn, with no slide on top.
                    if !self.motion.flying.iter().any(|f| f.0 == id) {
                        if let Some(drawn) = self.scene.get(id, prop::FRAME) { row_jump = drawn.y - scene_rect.y; }
                        reorder_jump = self.reorder_jump(&nodes, i);
                    }
                    if let Some(anim) = frame_motion {
                        LayoutAnim::axes(anim, Anim::INSTANT)
                    } else {
                        let mut la = LayoutAnim::axes(anim_l, Anim::INSTANT);
                        if tx.motion.is_none() && nodes[i].width_leads {
                            la = la.width_leads();
                        }
                        la
                    }
                } else if layout_instant || follows_window {
                    LayoutAnim::INSTANT
                } else if let Some(anim) = frame_motion {
                    LayoutAnim::new(anim)
                } else {
                    let mut la = LayoutAnim::new(anim_l);
                    if tx.motion.is_none() {
                        if let Some((lead, trail)) = nodes[i].lead {
                            la = la.lead(lead, trail);
                        } else if nodes[i].width_leads {
                            la = la.width_leads();
                        }
                    }
                    la
                };
                // An instant change that leaves a flying node's own layout as it was (a click
                // placing a caret elsewhere, say) keeps its flight on its path, as it keeps
                // other motion an earlier change set going.
                let flying = if self.motion.flying.is_empty() { None }
                    else if lands { self.motion.flying.iter().position(|f| f.0 == id && f.1 == nodes[i].local).filter(|_| keeps_flights && !nodes[i].instant_changes) }
                    else { self.motion.flying.iter().position(|f| f.0 == id) };
                // A growth row lays out its contents at once. When the top of a parent's contents
                // moves in it (a padding, gap or margin above its first child changed), they all
                // slide from where they were drawn; a sibling's new size still moves the ones
                // under it at once, as the row's growth reveals it.
                if nodes[i].in_row == RowLayout::Snaps && !stack_row && (!lands || (shift.is_some() && !nodes[i].instant_changes)) && !tx.relayout && !follows_window && !out_of_sight
                    && let Some(p) = nodes[i].parent
                {
                    if i == p + 1 && flying.is_none() && let Some(drawn) = self.scene.get(id, prop::FRAME) {
                        let dy = drawn.y - scene_rect.y;
                        if dy.abs() > 1.0 / 64.0 {
                            if top_moved.is_empty() { top_moved.resize(nodes.len(), 0.0); }
                            top_moved[p] = dy;
                        }
                    }
                    let tm = top_moved.get(p).copied().unwrap_or(0.0);
                    if tm.abs() > 1.0 / 64.0 && flying.is_none() {
                        let left = self.motion.slide_with_row(&nodes, p, id, tm);
                        (row_jump, reorder_jump) = (left, left);
                        // Sliding with its row, it still slid: a flight under it lands or re-aims.
                        if left == 0.0 {
                            if slid.is_empty() { slid.resize(nodes.len(), false); }
                            slid[i] = true;
                        }
                    }
                }
                if let Some(f) = flying {
                    // Keep a matched flight on its one screen path, even where a growth row
                    // owns its place; retarget it only if its layout moved, or an ancestor's
                    // under a slide its path doesn't ride, so an unrelated commit doesn't re-time it.
                    if self.motion.flying[f].1 != nodes[i].local || nodes[i].parent.is_some_and(|p| slid.get(p).copied().unwrap_or(false)) {
                        self.motion.flying[f].1 = nodes[i].local;
                        // A growth row's instant vertical place would land the whole flight.
                        let la = if let Some(&anim) = nodes[i].matched_motion.as_deref() {
                            LayoutAnim::new(anim)
                        } else if layout_anim.x == Anim::INSTANT || layout_anim.y == Anim::INSTANT {
                            LayoutAnim::new(anim_l)
                        } else {
                            layout_anim
                        };
                        let [drawn, speed] = self.motion.launches_mut().restart(&self.scene, id);
                        self.scene.retarget_drawn(id, scene_rect, la.screen(), drawn, speed);
                    }
                } else if !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Rect) {
                    // Springing to a new place, it slides from where it was drawn: a flight
                    // under it, headed for its target, lands or re-aims.
                    let was = self.scene.store().target(id, prop::FRAME).or_else(|| self.scene.get(id, prop::FRAME));
                    if was.is_some_and(|r| (layout_anim.x != Anim::INSTANT && r.x != scene_rect.x) || (layout_anim.y != Anim::INSTANT && r.y != scene_rect.y)) {
                        if slid.is_empty() { slid.resize(nodes.len(), false); }
                        slid[i] = true;
                    }
                    self.scene.set_frame(id, scene_rect, layout_anim);
                }
                let quiet = (lands && !riding) || out_of_sight;
                let joins = live_row.is_some_and(|(_, row)| row.joins) && !nodes[i].instant_changes;
                self.set_or_anim(id, prop::RADIUS, Corners(nodes[i].vis.radii()), spring_l, (lands && !riding && !joins) || out_of_sight);
                if nodes[i].live.iter().any(|b| b.property == crate::live::Property::Opacity) {
                    // The live source owns this property, including across unrelated commits.
                } else if !quiet && let Some(&anim) = nodes[i].opacity_motion.as_deref() {
                    // Only a new target: an anim with its own velocity would restart each commit,
                    // and would run on an element that mounted where it rests.
                    let rest = self.scene.store().target(id, prop::OPACITY).or_else(|| self.scene.get(id, prop::OPACITY));
                    if rest != Some(nodes[i].vis.opacity) {
                        self.scene.animate(id, prop::OPACITY, nodes[i].vis.opacity, anim);
                    }
                } else {
                    self.set_or_anim(id, prop::OPACITY, nodes[i].vis.opacity, spring_l, quiet);
                }
            }
            let rest_quiet = (lands && !riding) || !was_present || was_exiting || self.reduced_motion;
            let shift = nodes[i].vis.offset;
            if !slid.is_empty() && nodes[i].parent.is_some_and(|p| slid[p]) { slid[i] = true; }
            // A moving sequence plays on top of `El::offset` and `El::rotate`; between plays they
            // aim at its resting pose so a change still springs (`step_loops_into_scene`). While
            // the sequence actively plays, `step_loops_into_scene` owns both channels so an
            // unrelated commit doesn't start a spring back toward `rest`.
            let sequence_rest = match nodes[i].looping.as_deref().map(|l| l.kind) {
                Some(rune_motion::LoopKind::Sequence(q)) if q.moves() => Some(q.rest()),
                _ => None,
            };
            let sequence_playing = sequence_rest.is_some()
                && !self.reduced_motion
                && self.motion.loop_active(id, self.scene.store().node_time(id));
            if (nodes[i].looping.is_none() || (sequence_rest.is_some() && !sequence_playing)) && !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Offset)
                && (shift != [0.0; 2] || reorder_jump != 0.0 || self.scene.store().has(id, prop::OFFSET))
            {
                let lift = sequence_rest.map_or([0.0; 2], |b| b.translate);
                let to = rune_motion::Vec2::new(shift[0] + lift[0], shift[1] + lift[1]);
                if rest_quiet && reorder_jump == 0.0 {
                    self.scene.set(id, prop::OFFSET, to);
                } else if self.scene.store().target(id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO) != to {
                    if row_jump != 0.0 {
                        // The row starts from where it was drawn: a reordered row that drops its
                        // offset slides into its new place.
                        let now = self.scene.get(id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
                        self.scene.set(id, prop::OFFSET, rune_motion::Vec2::new(now.x, now.y + row_jump));
                        if slid.is_empty() { slid.resize(nodes.len(), false); }
                        slid[i] = true;
                    }
                    self.scene.animate(id, prop::OFFSET, to, nodes[i].vis.offset_motion.as_deref().copied().unwrap_or_else(|| spring_l.into()));
                } else if reorder_jump != 0.0 {
                    self.slide_reordered(id, reorder_jump, to, anim_l);
                    if slid.is_empty() { slid.resize(nodes.len(), false); }
                    slid[i] = true;
                }
            }
            // Held at its landing width, it starts where its anchor child was drawn.
            if let Some(&(_, dx)) = holds.iter().find(|h| h.0 == i)
                && nodes[i].looping.is_none()
                && !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Offset)
            {
                let now = self.scene.get(id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
                self.scene.set(id, prop::OFFSET, rune_motion::Vec2::new(now.x + dx, now.y));
                self.scene.animate(id, prop::OFFSET, rune_motion::Vec2::new(shift[0], shift[1]), nodes[i].vis.offset_motion.as_deref().copied().unwrap_or_else(|| spring_l.into()));
                if slid.is_empty() { slid.resize(nodes.len(), false); }
                slid[i] = true;
            }
            nodes[i].group_fade = set_group_rest(&mut self.scene, id, nodes[i].vis.group_opacity, nodes[i].opacity_motion.as_deref().copied(), spring_l, rest_quiet);
            let k = nodes[i].static_scale;
            nodes[i].scale_track = k != 1.0 || self.scene.store().has(id, P_SCALE);
            if nodes[i].scale_track {
                let motion = nodes[i].vis.scale_motion.as_deref().copied();
                let anim = motion.unwrap_or_else(|| spring_l.into());
                let starting = !rest_quiet && anim.curve != rune_motion::Curve::Instant
                    && self.scene.store().target(id, P_SCALE) != Some(k);
                self.snap_or_animate(id, P_SCALE, k, rest_quiet, 1.0, motion, spring_l);
                if starting && !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Scale) {
                    let target = self.scene.store().target(id, P_SCALE).unwrap_or(k);
                    self.paint.record_static_scale_target(id, target);
                }
            }
            let turn = nodes[i].vis.rotate + sequence_rest.map_or(0.0, |b| b.rotate);
            if !sequence_playing
                && !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Rotation)
                && (turn != 0.0 || self.scene.store().has(id, prop::ROTATION))
            {
                self.snap_or_animate(id, prop::ROTATION, turn, rest_quiet, 0.0, nodes[i].vis.rotate_motion.as_deref().copied(), spring_l);
            }
            let (instant, vis) = ((instant && !riding) || self.enters_unseen(&nodes, &unseen, i), &nodes[i].vis);
            if !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Blur)
                && (vis.blur != 0.0 || self.scene.store().has(id, prop::BLUR))
            {
                if !instant && was_present && let Some(&anim) = vis.blur_motion.as_deref() {
                    self.scene.animate(id, prop::BLUR, vis.blur, anim);
                } else { self.set_or_anim(id, prop::BLUR, vis.blur, spring_l, instant || !was_present); }
            }
            let live_colors = paint_tracks::live_colors(&nodes[i]);
            if !live_colors.fill {
                paint_tracks::commit_paint(&mut self.scene, id, P_FILL, paint_tracks::prev_node(&self.tree.nodes, &nodes, old, i).map(|o| o.vis.fill), vis.fill, spring_c, instant);
            }
            if vis.border_width() > 0.0 {
                let had_bottom = self.scene.store().has(id, P_BORDER_BOTTOM);
                if vis.border_bottom.is_some() && !had_bottom
                    && let Some(c) = self.scene.get(id, P_BORDER).or(paint_tracks::prev_node(&self.tree.nodes, &nodes, old, i).map(|o| o.vis.border()))
                {
                    // A solid border's bottom springs into the gradient rather than jumping.
                    self.scene.set(id, P_BORDER_BOTTOM, c);
                }
                if !live_colors.border {
                    paint_tracks::commit_paint(&mut self.scene, id, P_BORDER, paint_tracks::prev_node(&self.tree.nodes, &nodes, old, i).map(|o| o.vis.border()), vis.border(), spring_c, instant);
                }
                if vis.border_bottom.is_some() || had_bottom {
                    self.set_or_anim(id, P_BORDER_BOTTOM, vis.border_bottom.unwrap_or(vis.border()), spring_c, instant);
                }
            }
            if !live_colors.text && let Some(c) = paint_tracks::text_color(&nodes[i]) {
                paint_tracks::commit_paint(&mut self.scene, id, P_TEXT, paint_tracks::prev_node(&self.tree.nodes, &nodes, old, i).and_then(paint_tracks::text_color), c, spring_tc, instant);
            }
            if nodes[i].vis.icon.is_some() && (nodes[i].vis.icon_turn != 0.0 || self.scene.store().has(id, P_ICON_TURN)) {
                if was_present && !self.scene.store().has(id, P_ICON_TURN) {
                    self.scene.set(id, P_ICON_TURN, 0.0);
                }
                self.set_or_anim(id, P_ICON_TURN, nodes[i].vis.icon_turn, spring_l, instant || !was_present);
            }
            if vis.hover().a > 0.0 || self.scene.store().has(id, P_HOVER_C) {
                paint_tracks::commit_paint(&mut self.scene, id, P_HOVER_C, paint_tracks::prev_node(&self.tree.nodes, &nodes, old, i).map(|o| o.vis.hover()), vis.hover(), spring_c, instant);
            }
            if vis.press().a > 0.0 || self.scene.store().has(id, P_PRESS_C) {
                paint_tracks::commit_paint(&mut self.scene, id, P_PRESS_C, paint_tracks::prev_node(&self.tree.nodes, &nodes, old, i).map(|o| o.vis.press()), vis.press(), spring_c, instant);
            }
            if !vis.cold.is_empty() || paint_tracks::prev_node(&self.tree.nodes, &nodes, old, i).is_some_and(|o| !o.vis.cold.is_empty()) {
                paint_tracks::commit_cold_paint(&mut self.scene, &self.tree.nodes, id, vis, old, live_colors, spring_c, spring_l, instant);
            }

            // Streaming word reveal.
            if let Some(spec) = &nodes[i].text
                && spec.reveal >= 0.0
            {
                let content = spec.content.as_str();
                let now = if time_scaled { self.scene.store().node_time(id) } else { now };
                let fade = if self.reduced_motion { spec.reveal.min(0.15) } else { spec.reveal };
                self.motion.commit_reveal(id, content, fade, now, time_scaled, was_present, instant, tx.unprompted, nodes[i].virtual_mount, self.reduced_motion);
            }
        }

        if !arrivals.is_empty() || self.motion.has_matched() {
            self.motion.match_elements(&self.tree, &self.paint, &mut self.scene, &mut self.commit, &nodes, &arrivals, tx, instant, self.reduced_motion);
        }
        arrivals.clear();
        if self.motion.launches().pending() {
            self.motion.launch_elements(&self.tree, &self.paint, &mut self.scene, &nodes, tx, instant, self.env.scale, self.reduced_motion);
        }

        // Scroll viewports: a new sticky viewport opens at the end.
        for (i, n) in nodes.iter().enumerate() {
            if let Some(sp) = n.cold.get().and_then(|c| c.scroll) {
                let e = self.scroll.scrolls.entry(n.id).or_insert(ScrollState { mode: if sp.stick_bottom { ScrollMode::following() } else { ScrollMode::rest() }, ..ScrollState::default() });
                e.x.flow = n.flow;
                if concealed.get(i).copied().unwrap_or(false) {
                    e.offset = 0.0;
                    e.halt();
                    e.max = 0.0;
                    e.held = false;
                    e.still();
                    e.slack = scroll_bottom::Slack::landed(self.viewport);
                    e.x = AxisScroll::default();
                }
                if tx.commit.lands() && sp.stick_bottom {
                    e.pin();
                }
                // A window resize, a settle or an instant change lands the slack: the rows go
                // straight to the bottom, not on a glide a moment later.
                if tx.commit.lands() || tx.commit.snaps() || e.slack.window != self.viewport { e.slack = scroll_bottom::Slack::landed(self.viewport); }
                if tx.follow_edge && sp.stick_bottom && !tx.commit.snaps() { e.slack.follow = true; }
            }
        }

        // Removed nodes stay mounted in `Scene` until their exit transition settles at opacity 0.
        let present_len = nodes.len();
        let mut old_nodes = std::mem::take(&mut self.tree.nodes);
        exits::carry_live_bindings(&self.tree.node_index, &mut self.scene, &mut nodes, &old_nodes, tx);
        if !any_gone {
            old_nodes = apply_scratch::clear_old_nodes(old_nodes, &memo_reused, &mut scratch.moved_spans);
        } else {
            let mut pruned = Vec::new();
            exits::retire_removed_nodes(
                &mut self.tree.node_index, &mut self.scene, &taffy, &self.commit.virtuals, &self.motion.flying,
                &mut self.text, self.env.scale, self.reduced_motion,
                &mut nodes, &mut old_nodes, present_len, tx, instant, &drew_nothing, &exit_parents, &mut scratch, &mut pruned,
            );
            for old in &pruned { self.hover_pruned(old); }
        }
        self.tree.spare_nodes = old_nodes;

        if time_scaled {
            // Exits keep following their present ancestors' scale, so one under a frozen subtree
            // finishes once it resumes.
            self.motion.sync_time_scales(&mut self.scene, &nodes, &mut time_scales, true);
            self.motion.set_time_scaled(time_scales.iter().any(|&k| k != 1.0));
        }
        scratch.concealed = concealed;
        scratch.time_scales = time_scales;
        scratch.arrivals = arrivals;
        scratch.mounted = mounted;
        scratch.slid = slid;
        scratch.top_moved = top_moved;
        scratch.held = held;
        self.commit.apply_scratch = scratch;
        self.tree.nodes = nodes;
        self.commit.memo.commit_flattened(&self.tree.nodes, present_len, memo_flattened, memo_reused, taken_paths);
        skip_spans.clear();
        self.commit.memo.skip_scratch = skip_spans;
        self.find_app_root();
        self.commit.commit_gen = self.commit.commit_gen.wrapping_add(1);
        let any_loop = self.prune_detached_tables(true);
        if self.tree.nodes.len() > present_len || !self.motion.flying.is_empty() || landed || self.tree.has_sticky || self.tree.has_raise || self.tree.has_paint_order || self.tree.has_above { self.update_layer_paint_order(); }
        else { self.paint.clear_layer_paint_order(); }
        self.tree.built = false;
        self.subscribe_live();
        if let Some(path) = rebound_press { self.pointer.rebind_press_path(&path); }
        let modal_cb = self.sync_modals(app);
        self.sync_held_drag();
        let follows_cb = self.sync_follows(app);
        self.motion.sync_loops(&self.tree.nodes, &self.scene, self.t, self.clock, self.time_scale, time_scaled, any_loop, tx.commit.lands() || self.reduced_motion);
        taffy.compact_if_sparse(|me, slot| self.tree.nodes[me as usize].taffy = slot);
        self.commit.layout = taffy;
        self.commit.reuse.built(self.env.scale, self.paint.srgb_blend(), above_fit_styles);
        self.text.end_commit();
        self.commit.dirty = false;
        self.commit.relayout = false;
        self.clear_a11y_activation_build();
        self.tree.aspect_snap.clear();
        self.commit.viewport_stepped = false;
        self.commit.settle = false;
        // After the commit: a new height changes the insets, which the next frame's view reads.
        self.set_titlebar(titlebar);
        modal_cb || follows_cb || self.kbd.focus_request.is_some()
    }

    /// Sets `prop` to `to` when `quiet`, or animates it from `identity` when its target changed.
    fn snap_or_animate<T: rune_motion::Animatable + PartialEq>(
        &mut self, id: NodeId, prop: Prop<T>, to: T, quiet: bool, identity: T, motion: Option<Anim>, spring: Spring,
    ) {
        if quiet {
            self.scene.set(id, prop, to);
        } else if self.scene.store().target(id, prop) != Some(to) {
            if !self.scene.store().has(id, prop) { self.scene.set(id, prop, identity); }
            self.scene.animate(id, prop, to, motion.unwrap_or_else(|| spring.into()));
        }
    }
}
