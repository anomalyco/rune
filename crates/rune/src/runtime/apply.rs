//! Applying a completed measurement pass to `Scene`, growth stacks, reveals, scroll viewports,
//! live bindings and retained exits.
use super::*;

impl<S: 'static> Runtime<S> {
    /// The mutation phase cannot accept an El or an unfinished measurement pass.
    pub(super) fn apply(&mut self, measured: Measured, tx: &Tx) {
        let _apply_time = crate::frame_profile::Scope::new(crate::frame_profile::Phase::Apply);
        let Measured { layout: taffy, mut nodes, reduced_motion, time_scale, caret_read, memo_flattened, memo_reused, .. } = measured;
        self.caret_watch = caret_read;
        let reduced_motion_unchanged = reduced_motion == self.reduced_motion;
        if !reduced_motion_unchanged {
            self.reduced_motion = reduced_motion;
            self.scene.set_reduced_motion(reduced_motion);
        }
        self.time_scale = time_scale;
        if tx.settle { self.flights.clear(); }
        let scale = self.env.scale;
        // Culling is not a model removal. Retire offscreen scene/growth state immediately;
        // re-entering historical rows mount settled and don't replay streaming words.
        if !self.virtual_culled.is_empty() {
            let mut culled = vec![false; self.nodes.len()];
            for (i, n) in self.nodes.iter().enumerate() {
                if self.virtual_culled.contains(&n.path) || n.parent.is_some_and(|p| culled[p]) {
                    culled[i] = true;
                    self.scene.unmount(n.id);
                    self.grows.remove(&n.id);
                }
            }
            for path in &self.virtual_culled {
                if let Some(id) = self.path_ids.get(path) {
                    self.scene.unmount(*id);
                    self.grows.remove(id);
                }
            }
        }
        for v in self.virtuals.values_mut() {
            v.shown = v.keys[v.range.clone()].iter().cloned().collect();
        }
        // A row scrolled into view mounts settled with its subtree. Content added later under a
        // row already on screen is a live change and keeps its own entrance.
        // Parents precede children, so a remounting row passes its flag down its whole subtree.
        for i in 0..nodes.len() {
            if let Some(p) = nodes[i].parent {
                nodes[i].virtual_mount |= nodes[p].virtual_mount && !self.scene.contains(nodes[p].id);
            }
        }
        let first = self.commits == 0;
        self.commits += 1;
        self.stats.commits = self.commits;
        // Concealing is a security boundary: old geometry must not disclose the revealed
        // line count through a height spring, displaced siblings, or a lingering scroll glide.
        let revealed: HashSet<_> = self.nodes.iter()
            .filter(|n| {
                n.input.as_ref().is_some_and(|input| input.edit.is_secure() && input.edit.is_revealed())
                    || (n.input.is_none() && n.text.as_ref().is_some_and(|t| t.redacted && t.secure_revealed))
            })
            .map(|n| n.id).collect();
        let mut concealed = Vec::new();
        let concealing = if !revealed.is_empty() {
            concealed.resize(nodes.len(), false);
            for (i, n) in nodes.iter().enumerate() {
                let hidden_now = n.input.as_ref().is_some_and(|input| input.edit.is_hidden_multiline())
                    || (n.input.is_none() && n.text.as_ref().is_some_and(|t| t.redacted && !t.secure_revealed));
                if hidden_now && revealed.contains(&n.id) {
                    concealed[i] = true;
                }
            }
            mark_ancestors(&nodes, &mut concealed);
            concealed.iter().any(|&hidden| hidden)
        } else {
            false
        };
        let instant = tx.instant || first || concealing;
        // An instant change lands what it changes; motion an earlier change set going keeps on.
        self.scene.keep_flights(tx.instant && !tx.settle && !first && !concealing);
        let now = self.t;
        if self.fit_height && let Some(root) = nodes.first() { self.fit_window(root.local.h, tx, first); }

        // What drew nothing last frame has no exit to show (`unseen_presence.rs`).
        let any_gone = self.nodes.iter().any(|o| !self.node_index.contains_key(&o.id));
        let any_replaced = self.virtuals.values().any(crate::virtual_list::VirtualList::replaced_view);
        let drew_nothing = if any_gone || any_replaced { self.drew_nothing() } else { Vec::new() };
        let unseen_ids: HashSet<NodeId> = if self.virtuals.is_empty() || drew_nothing.is_empty() { HashSet::default() } else {
            self.nodes.iter().zip(&drew_nothing).filter(|&(_, &u)| u).map(|(n, _)| n.id).collect()
        };

        // Growth stacks: rows of a stack that existed before grow; a new stack mounts settled.
        for c in 0..nodes.len() {
            if nodes[c].growth.is_none() { continue }
            let cid = nodes[c].id;
            let instant = instant || nodes[c].instant_changes;
            // A reader away from the tail is anchored using measured row heights. Land those
            // heights with the anchor correction, unless `keep_on_screen` or `scroll_into_view`
            // is driving this viewport across the commit. A reveal left from an earlier commit
            // keeps only rows already growing on their spring: new rows land, or under a parent
            // whose size springs too, the two springs carry them past their slots.
            // So does a reveal in this commit for rows new to the stack: rows inserted above an
            // anchored first row and revealed at once land whole instead of growing from zero.
            let revealing = self.virtuals.get(&nodes[c].path)
                .is_some_and(|v| self.scroll_reveals.iter().any(|r| r.viewport == v.scroll_path));
            let revealed_now = self.virtuals.get(&nodes[c].path).is_some_and(|v| {
                tx.scroll_to.iter().any(|(scope, key)| {
                    nodes.iter().enumerate().find(|(_, n)| n.named(key)
                        && n.parent.is_none_or(|p| in_scope(&nodes[p].path, scope.as_deref())))
                        .is_some_and(|(i, _)| {
                            std::iter::successors(nodes[i].parent, |&p| nodes[p].parent)
                                .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y())
                                .is_some_and(|p| nodes[p].path == v.scroll_path)
                        })
                })
            });
            let reader = self.virtuals.get(&nodes[c].path)
                .filter(|v| !self.keeps.iter().any(|k| k.viewport == v.scroll_path))
                .and_then(|v| self.path_ids.get(&v.scroll_path))
                .and_then(|id| self.scrolls.get(id)).is_some_and(|s| !s.pinned);
            let reading = reader && !revealed_now;
            let rows: Vec<usize> = (c + 1..nodes.len()).filter(|&j| nodes[j].parent == Some(c)).collect();
            let existed = self.stacks.contains_key(&cid) && !tx.settle;
            let mut stack = self.stacks.remove(&cid).unwrap_or_default();
            let new_ids: Vec<NodeId> = rows.iter().map(|&j| nodes[j].id).collect();
            let new_set: HashSet<NodeId> = new_ids.iter().copied().collect();
            // Merge: keep leaving rows where they stood. In a virtual list, a row that drew
            // nothing, or any row when the change keeps none the reader could see (a filter),
            // leaves no space to close: the list's anchor already holds what shows (`set_keys`),
            // and the space would only push the rows the change brings in down, out of view.
            let list = self.virtuals.get(&nodes[c].path);
            let is_virtual = list.is_some();
            let replaced = list.is_some_and(crate::virtual_list::VirtualList::replaced_view);
            let mut order = Vec::with_capacity(new_ids.len() + 4);
            let mut ni = 0;
            for &old in &stack.order {
                if new_set.contains(&old) {
                    while ni < new_ids.len() && new_ids[ni] != old {
                        if !order.contains(&new_ids[ni]) {
                            order.push(new_ids[ni]);
                        }
                        ni += 1;
                    }
                    if !order.contains(&old) {
                        order.push(old);
                    }
                    ni += 1;
                } else if self.grows.get(&old).is_some_and(|g| g.h > 0.01) && existed && !instant && !replaced && !(is_virtual && unseen_ids.contains(&old)) {
                    stack.closing.entry(old).or_insert(now + 0.15);
                    order.push(old);
                } else {
                    self.grows.remove(&old);
                    stack.closing.remove(&old);
                }
            }
            for &id in &new_ids[ni.min(new_ids.len())..] {
                if !order.contains(&id) {
                    order.push(id);
                }
            }
            let ctop = nodes[c].target[1];
            let width = nodes[c].local.w;
            let revealing_moving = revealing && rows.iter().any(|&j| {
                let id = nodes[j].id;
                self.grows.get(&id).is_some_and(|g| g.moving()) || stack.pending_heights.contains_key(&id)
            });
            // A person resizing the window holds the content: rewrapped rows land in the frame,
            // so a pinned tail stays where it is instead of sliding back after the drag. A
            // streaming row still waits for the width to settle, then eases.
            let resized = existed && stack.window != self.viewport;
            stack.window = self.viewport;
            if first || !existed || tx.settle || self.reduced_motion || (reading && !revealing_moving) {
                stack.width_hold_until = 0.0;
                stack.width_shrink_until = 0.0;
                stack.pending_heights.clear();
            } else if (width - stack.width).abs() > 0.25 {
                stack.width_hold_until = now + 0.12;
                stack.width_shrink_until = now + 0.4;
            }
            stack.width = width;
            stack.pending_heights.retain(|id, _| new_set.contains(id));
            stack.gap = crate::virtual_list::gap_y(taffy.style(nodes[c].taffy).unwrap());
            // With every row gone, the leaving rows still stand below the stack's top padding.
            let pad_top = rows.first().map_or(stack.pad_top, |&j| nodes[j].target[1] - ctop);
            stack.pad_bottom = rows.last().map_or(nodes[c].target[3] - ctop - pad_top, |&j| nodes[c].target[3] - nodes[j].target[3]);
            stack.pad_top = pad_top;
            for &j in &rows {
                let id = nodes[j].id;
                let mut h = nodes[j].target[3] - nodes[j].target[1];
                stack.closing.remove(&id);
                stack.tops.insert(id, nodes[j].target[1] - ctop);
                let streaming = nodes[j].streaming;
                let whole = nodes[j].whole && !streaming;
                let g_moving = self.grows.get(&id).is_some_and(|g| g.moving()) || stack.pending_heights.contains_key(&id);
                let entering = revealed_now && reader && !self.grows.contains_key(&id);
                let row_reading = (reading && !(revealing && g_moving)) || entering;
                if streaming && existed && !tx.settle && !first && !self.reduced_motion && !row_reading {
                    // Measure the final wrapped text before mutating the growth track. Targets
                    // retain full layout height; only the presented stack extent lags the fill.
                    let mut end = j + 1;
                    while end < nodes.len() && nodes[end].parent.is_some_and(|p| p >= j) { end += 1; }
                    if let Some(n) = nodes[j..end].iter().rev().find(|n| n.text.as_ref().is_some_and(|t| t.wrap && t.reveal >= 0.0))
                        && let Some(l) = &n.text_layout
                    {
                        let width = (n.target[2] - n.target[0] - n.inset[0] - n.inset[2]) * scale;
                        h = (h - crate::text::stream_height_lag(l, width, scale)).max(0.0);
                    }
                }
                let duration = tx.motion.map_or(0.0, |m| m.duration * 1.15);
                let g = self.grows.entry(id).or_insert_with(|| {
                    if existed && !instant && !nodes[j].virtual_mount { GrowTrack { streaming, whole, duration, ..GrowTrack::settled(0.0) } } else { GrowTrack::settled(h) }
                });
                g.streaming = streaming;
                g.whole = whole;
                let snap_change = (instant || resized) && !streaming && (h - g.target).abs() > 1e-4;
                if tx.settle || !existed || first || self.reduced_motion || snap_change || row_reading {
                    *g = GrowTrack { streaming, whole, ..GrowTrack::settled(h) };
                    stack.pending_heights.remove(&id);
                } else {
                    // Only a width/scale rewrap may shrink an active streaming row, including
                    // when fresh text arrives in the same frame as the resize.
                    let t = if streaming && !self.relayout && now >= stack.width_shrink_until { h.max(g.h.min(g.target)) } else { h };
                    if now < stack.width_hold_until {
                        stack.pending_heights.insert(id, t);
                    } else {
                        stack.pending_heights.remove(&id);
                        if (t - g.target).abs() > 1e-4 { g.duration = duration; }
                        g.retarget(t);
                    }
                }
            }
            if let Some(&last) = rows.last()
                && !self.scene.contains(nodes[last].id)
                && let Some(s) = std::iter::successors(nodes[c].parent, |&p| nodes[p].parent)
                    .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y())
                && let Some(r) = self.scroll_reveals.iter_mut().find(|r| r.viewport == nodes[s].path && r.at_end)
            {
                r.scope = None;
                r.key = nodes[last].path.clone();
            }
            stack.growing = !stack.pending_heights.is_empty()
                || order.iter().any(|r| self.grows.get(r).is_some_and(|g| g.target > g.h + 1.0 / 128.0));
            stack.moving = stack.growing
                || order.iter().any(|r| self.grows.get(r).is_some_and(|g| g.moving()));
            // Order holds the rows this commit brought and the ones still closing; no others.
            stack.tops.retain(|id, _| new_set.contains(id) || stack.closing.contains_key(id));
            let animates = existed && !instant && !tx.relayout && !self.reduced_motion;
            stack.reorder.measure(animates, &stack.order, &order, stack.gap, |id| self.grows.get(&id).map(|g| g.h));
            stack.order = order;
            self.stacks.insert(cid, stack);
        }

        // Put nodes on their subtree's clock before this commit animates them.
        let time_scaled = self.time_scaled || nodes.iter().any(|n| n.time_scale != 1.0);
        let mut time_scales = Vec::new();
        if time_scaled { self.sync_time_scales(&nodes, &mut time_scales, false); }

        let can_skip_apply = !first && !tx.settle && !concealing && reduced_motion_unchanged && !self.scene.tracing();
        let mut skip_spans = std::mem::take(&mut self.memo.skip_scratch);
        self.clean_apply_spans(&nodes, &memo_reused, can_skip_apply, &mut skip_spans);
        let unseen = self.unseen_before_and_after(&nodes, &skip_spans);
        // `El::matched` elements of this tree: (id, index, newly mounted).
        let mut arrivals: Vec<(Rc<str>, usize, bool)> = Vec::new();
        let mut mounted: Vec<bool> = Vec::new();
        // Nodes this commit slid from where they were drawn, or under an ancestor that slid.
        let mut slid: Vec<bool> = Vec::new();
        // How far the top of each node's contents moved down in its growth row (drawn less new).
        let mut top_moved: Vec<f32> = Vec::new();
        let mut last = paint_tracks::LastBuild::default();
        let mut skip_cursor = 0usize;
        let mut i = 0usize;
        while i < nodes.len() {
            while skip_cursor < skip_spans.len() && skip_spans[skip_cursor].new_start + skip_spans[skip_cursor].len <= i {
                skip_cursor += 1;
            }
            if let Some(span) = skip_spans.get(skip_cursor).filter(|s| s.new_start == i) {
                for k in 0..span.len {
                    nodes[span.new_start + k].scale_track = self.nodes[span.old_start + k].scale_track;
                }
                last.advance(span.old_start + span.len);
                i += span.len;
                continue;
            }
            let cur_i = i;
            i += 1;
            let i = cur_i;
            let id = nodes[i].id;
            let parent_id = nodes[i].parent.map(|p| nodes[p].id);
            let spring_l = tx.motion.unwrap_or(nodes[i].motion);
            let anim_l: Anim = tx.motion_curve.unwrap_or_else(|| spring_l.into());
            let spring_c = tx.color_motion.unwrap_or(nodes[i].motion);
            // Text leads its background so theme crossfades never dip through zero contrast.
            let spring_tc = Spring::new((spring_c.duration * 0.5).max(0.1), spring_c.bounce);
            let row_like = nodes[i].in_row == RowLayout::Snaps || nodes[i].parent.is_some_and(|p| nodes[p].growth.is_some());
            // An `El::instant_changes` element lands its changes, and those of what it holds;
            // its own entrance and exit, and colours, stay with the commit.
            let enters_at_once = instant || nodes[i].parent.is_some_and(|p| nodes[p].instant_changes);
            let lands = instant || nodes[i].instant_changes;
            let layout_instant = lands || row_like || tx.relayout || self.virtuals.contains_key(&nodes[i].path)
                || self.aspect_snap.contains(&id);
            let mut scene_rect = nodes[i].local;
            if let Some(v) = nodes[i].parent.and_then(|p| self.virtuals.get(&nodes[p].path)) {
                scene_rect.y -= v.before;
            }

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
                    .radius(Corners(nodes[i].vis.radius))
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
                let mount = if enters_at_once || nodes[i].virtual_mount || quiet_row { Mount::Initial } else { Mount::Animated };
                self.scene.insert(id, spec, mount);
            }
            self.scene.set_ride_trailing(id, nodes[i].vis.ride_trailing);
            // How far a growth or virtual row's place jumps in this commit (drawn minus new).
            let (mut row_jump, mut reorder_jump) = (0.0, 0.0);
            if was_present {
                // As in SwiftUI, a node leaves with the transition of its last render.
                if !was_exiting {
                    self.scene.set_transition(id, nodes[i].transition.as_deref().copied().unwrap_or_else(Transition::opacity));
                }
                let stack_row = nodes[i].parent.is_some_and(|p| nodes[p].growth.is_some()) || self.virtuals.contains_key(&nodes[i].path)
                    || (nodes[i].in_row == RowLayout::Horizontal && !self.aspect_snap.contains(&id));
                // A window's own resize step moves content at rest with the window; motion
                // already running retargets on its spring.
                let follows_window = tx.window_step && !self.flying.iter().any(|f| f.0 == id)
                    && !self.scene.store().is_animating(id, prop::FRAME);
                // Out of sight before and after: nobody would see it move.
                let out_of_sight = unseen.get(i).copied().unwrap_or(false) && nodes[i].matched.is_none()
                    && !self.flying.iter().any(|f| f.0 == id);
                let layout_anim = if out_of_sight {
                    LayoutAnim::INSTANT
                } else if stack_row && !lands && !tx.relayout && !follows_window {
                    // Growth and virtual windows own a row's vertical place; its horizontal place
                    // still springs, or a re-centred column jumps against its moving parent. So
                    // do `animate_layout` contents inside the row.
                    // A flying row's path restarts from where it is drawn, with no slide on top.
                    if !self.flying.iter().any(|f| f.0 == id) {
                        if let Some(drawn) = self.scene.get(id, prop::FRAME) { row_jump = drawn.y - scene_rect.y; }
                        reorder_jump = self.reorder_jump(&nodes, i);
                    }
                    let mut la = LayoutAnim::axes(anim_l, Anim::INSTANT);
                    if tx.motion.is_none() && nodes[i].width_leads {
                        la = la.width_leads();
                    }
                    la
                } else if layout_instant || follows_window {
                    LayoutAnim::INSTANT
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
                let flying = if self.flying.is_empty() || lands { None }
                    else { self.flying.iter().position(|f| f.0 == id) };
                // A growth row lays out its contents at once. When the top of a parent's contents
                // moves in it (a padding, gap or margin above its first child changed), they all
                // slide from where they were drawn; a sibling's new size still moves the ones
                // under it at once, as the row's growth reveals it.
                if nodes[i].in_row == RowLayout::Snaps && !stack_row && !lands && !tx.relayout && !follows_window && !out_of_sight
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
                        (row_jump, reorder_jump) = (tm, tm);
                    }
                }
                if let Some(f) = flying {
                    // Keep a matched flight on its one screen path, even where a growth row
                    // owns its place; retarget it only if its layout moved, or an ancestor's
                    // under a slide its path doesn't ride, so an unrelated commit doesn't re-time it.
                    if self.flying[f].1 != nodes[i].local || nodes[i].parent.is_some_and(|p| slid.get(p).copied().unwrap_or(false)) {
                        self.flying[f].1 = nodes[i].local;
                        // A growth row's instant vertical place would land the whole flight.
                        let la = if layout_anim.x == Anim::INSTANT || layout_anim.y == Anim::INSTANT { LayoutAnim::new(spring_l) } else { layout_anim };
                        let [drawn, speed] = self.launches.restart(&self.scene, id);
                        self.scene.retarget_drawn(id, scene_rect, la.screen(), drawn, speed);
                    }
                } else if !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Rect) {
                    self.scene.set_frame(id, scene_rect, layout_anim);
                }
                let quiet = lands || out_of_sight;
                self.set_or_anim(id, prop::RADIUS, Corners(nodes[i].vis.radius), spring_l, quiet);
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
            let shift = nodes[i].vis.offset;
            if !slid.is_empty() && nodes[i].parent.is_some_and(|p| slid[p]) { slid[i] = true; }
            if nodes[i].looping.is_none() && !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Offset)
                && (shift != [0.0; 2] || reorder_jump != 0.0 || self.scene.store().has(id, prop::OFFSET))
            {
                let to = rune_motion::Vec2::new(shift[0], shift[1]);
                if lands || !was_present || was_exiting || self.reduced_motion {
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
                    match nodes[i].vis.offset_motion.as_deref().copied() {
                        Some(anim) => self.scene.animate(id, prop::OFFSET, to, anim),
                        None => self.scene.animate(id, prop::OFFSET, to, spring_l),
                    }
                } else if reorder_jump != 0.0 {
                    self.slide_reordered(id, reorder_jump, to, anim_l);
                    if slid.is_empty() { slid.resize(nodes.len(), false); }
                    slid[i] = true;
                }
            }
            let rest_quiet = lands || !was_present || was_exiting || self.reduced_motion;
            nodes[i].group_fade = self.set_group_rest(id, nodes[i].vis.group_opacity, nodes[i].opacity_motion.as_deref().copied(), spring_l, rest_quiet);
            let k = nodes[i].static_scale;
            nodes[i].scale_track = k != 1.0 || self.scene.store().has(id, P_SCALE);
            if nodes[i].scale_track {
                if lands || !was_present || was_exiting || self.reduced_motion {
                    self.scene.set(id, P_SCALE, k);
                } else if self.scene.store().target(id, P_SCALE) != Some(k) {
                    if !self.scene.store().has(id, P_SCALE) { self.scene.set(id, P_SCALE, 1.0); }
                    match nodes[i].vis.scale_motion.as_deref().copied() {
                        Some(anim) => self.scene.animate(id, P_SCALE, k, anim),
                        None => self.scene.animate(id, P_SCALE, k, spring_l),
                    }
                }
            }
            let turn = nodes[i].vis.rotate;
            if turn != 0.0 || self.scene.store().has(id, prop::ROTATION) {
                if lands || !was_present || was_exiting || self.reduced_motion {
                    self.scene.set(id, prop::ROTATION, turn);
                } else if self.scene.store().target(id, prop::ROTATION) != Some(turn) {
                    if !self.scene.store().has(id, prop::ROTATION) { self.scene.set(id, prop::ROTATION, 0.0); }
                    match nodes[i].vis.rotate_motion.as_deref().copied() {
                        Some(anim) => self.scene.animate(id, prop::ROTATION, turn, anim),
                        None => self.scene.animate(id, prop::ROTATION, turn, spring_l),
                    }
                }
            }
            let vis = &nodes[i].vis;
            if !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Blur)
                && (vis.blur != 0.0 || self.scene.store().has(id, prop::BLUR))
            {
                if !instant && was_present && let Some(&anim) = vis.blur_motion.as_deref() {
                    self.scene.animate(id, prop::BLUR, vis.blur, anim);
                } else { self.set_or_anim(id, prop::BLUR, vis.blur, spring_l, instant || !was_present); }
            }
            let old = last.find(&self.nodes, id);
            if !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Fill) {
                self.commit_paint(id, P_FILL, old.map(|o| self.nodes[o].vis.fill), vis.fill, spring_c, instant);
            }
            if vis.border_width > 0.0 {
                let had_bottom = self.scene.store().has(id, P_BORDER_BOTTOM);
                if vis.border_bottom.is_some() && !had_bottom
                    && let Some(c) = self.scene.get(id, P_BORDER).or(old.map(|o| self.nodes[o].vis.border))
                {
                    // A solid border's bottom springs into the gradient rather than jumping.
                    self.scene.set(id, P_BORDER_BOTTOM, c);
                }
                self.commit_paint(id, P_BORDER, old.map(|o| self.nodes[o].vis.border), vis.border, spring_c, instant);
                if vis.border_bottom.is_some() || had_bottom {
                    self.set_or_anim(id, P_BORDER_BOTTOM, vis.border_bottom.unwrap_or(vis.border), spring_c, instant);
                }
            }
            if let Some(c) = paint_tracks::text_color(&nodes[i]) {
                self.commit_paint(id, P_TEXT, old.and_then(|o| paint_tracks::text_color(&self.nodes[o])), c, spring_tc, instant);
            }
            if nodes[i].vis.icon.is_some() && (nodes[i].vis.icon_turn != 0.0 || self.scene.store().has(id, P_ICON_TURN)) {
                if was_present && !self.scene.store().has(id, P_ICON_TURN) {
                    self.scene.set(id, P_ICON_TURN, 0.0);
                }
                self.set_or_anim(id, P_ICON_TURN, nodes[i].vis.icon_turn, spring_l, instant || !was_present);
            }
            if vis.hover.a > 0.0 || self.scene.store().has(id, P_HOVER_C) {
                self.commit_paint(id, P_HOVER_C, old.map(|o| self.nodes[o].vis.hover), vis.hover, spring_c, instant);
            }
            if vis.press.a > 0.0 || self.scene.store().has(id, P_PRESS_C) {
                self.commit_paint(id, P_PRESS_C, old.map(|o| self.nodes[o].vis.press), vis.press, spring_c, instant);
            }
            for (k, sh) in vis.shadows.iter().take(P_SHADOWS.len()).enumerate() {
                let was = old.and_then(|o| self.nodes[o].vis.shadows.get(k).copied());
                self.commit_paint(id, P_SHADOWS[k], was.map(|w| w.color), sh.color, spring_c, instant);
                self.commit_paint(id, P_SHADOW_GEO[k], was.map(|w| paint_tracks::shadow_geo(&w)), paint_tracks::shadow_geo(sh), spring_l, instant);
            }
            // Only a shadow that was drawn can have a track.
            for k in vis.shadows.len()..old.map_or(0, |o| self.nodes[o].vis.shadows.len()).min(P_SHADOWS.len()) {
                self.scene.store_mut().remove(id, P_SHADOWS[k]);
                self.scene.store_mut().remove(id, P_SHADOW_GEO[k]);
            }
            if let Some(&g) = vis.gradient.as_deref() {
                let was = old.and_then(|o| self.nodes[o].vis.gradient.as_deref().copied());
                self.commit_paint(id, P_GRADIENT[0], was.map(|w| w[0]), g[0], spring_c, instant);
                self.commit_paint(id, P_GRADIENT[1], was.map(|w| w[1]), g[1], spring_c, instant);
            }
            if let Some(rg) = &vis.radial {
                for (k, &(_, c)) in rg.stops.iter().take(P_RADIAL.len()).enumerate() {
                    let was = old.and_then(|o| self.nodes[o].vis.radial.as_ref()?.stops.get(k).map(|s| s.1));
                    self.commit_paint(id, P_RADIAL[k], was, c, spring_c, instant);
                }
            }
            if let Some(ol) = vis.outline {
                self.commit_paint(id, P_OUTLINE, old.and_then(|o| self.nodes[o].vis.outline.map(|w| w.color)), ol.color, spring_c, instant);
            }

            // Streaming word reveal.
            if let Some(spec) = &nodes[i].text
                && spec.reveal >= 0.0
            {
                let content = &spec.content;
                let now = if time_scaled { self.scene.store().node_time(id) } else { now };
                let fade = if self.reduced_motion { 0.15 } else { spec.reveal };
                let mounted_late = !was_present && !instant && !nodes[i].virtual_mount;
                let st = self.reveals.entry(id).or_insert_with(|| RevealState {
                    len: if mounted_late { 0 } else { content.len() },
                    words: Vec::new(),
                    fade,
                });
                st.fade = fade;
                if instant || (!was_present && nodes[i].virtual_mount) || content.len() < st.len || !content.is_char_boundary(st.len) {
                    st.len = content.len();
                    st.words.clear();
                } else if content.len() > st.len && fade > 0.0 {
                    let mut starts = Vec::new();
                    word_starts(content, st.len, &mut starts);
                    // A word continuing across the old end keeps its time.
                    let mut tn = st.words.last().map_or(now, |w| w.1.max(now));
                    let pending = st.words.iter().filter(|w| w.1 > now).count() + starts.len();
                    let stagger = if self.reduced_motion {
                        0.0
                    } else {
                        let k = (pending as f32 / 24.0).clamp(0.0, 1.0);
                        (0.020 + (0.0055 - 0.020) * k) as f64
                    };
                    for b in starts {
                        st.words.push((b, tn));
                        tn += stagger;
                    }
                    st.len = content.len();
                } else {
                    st.len = content.len();
                }
            }
        }

        if !arrivals.is_empty() || !self.matched.is_empty() {
            self.match_elements(&nodes, arrivals, tx, instant);
        }
        if self.launches.pending() {
            self.launch_elements(&nodes, tx, instant);
        }

        // Scroll viewports: a new sticky viewport opens at the end.
        for (i, n) in nodes.iter().enumerate() {
            if let Some(sp) = n.scroll {
                let e = self.scrolls.entry(n.id).or_insert(ScrollState {
                    offset: 0.0,
                    v: 0.0,
                    pinned: sp.stick_bottom,
                    max: 0.0,
                    direct: false,
                    scrolling: false,
                    glide: None,
                    shrink_glide: false,
                    shrink_cut: false,
                    glide_to: None,
                    laid: None,
                    at_bottom: None,
                    far_from_bottom: None,
                    held: false,
                    slack: Default::default(),
                    x: AxisScroll::default(),
                    told: None,
                });
                if concealed.get(i).copied().unwrap_or(false) {
                    e.offset = 0.0;
                    e.v = 0.0;
                    e.max = 0.0;
                    e.glide = None;
                    e.glide_to = None;
                    e.held = false;
                    e.scrolling = false;
                    e.slack = scroll_bottom::Slack::landed(self.viewport);
                    e.x = AxisScroll::default();
                }
                if tx.settle && sp.stick_bottom {
                    e.pinned = true;
                    e.v = 0.0;
                }
                // A window resize, a settle or an instant change lands the slack: the rows go
                // straight to the bottom, not on a glide a moment later.
                if tx.settle || tx.instant || e.slack.window != self.viewport { e.slack = scroll_bottom::Slack::landed(self.viewport); }
                if tx.follow_edge && sp.stick_bottom && !tx.instant { e.slack.follow = true; }
            }
        }

        // Removed nodes stay mounted in `Scene` until their exit transition settles at opacity 0.
        let present_len = nodes.len();
        let mut old_nodes = std::mem::take(&mut self.nodes);
        for old in &old_nodes {
            if old.live.is_empty() { continue; }
            if let Some(&i) = self.node_index.get(&old.id) {
                let mut fill = None;
                for bound in &mut nodes[i].live {
                    if let Some(previous) = old.live.iter().find(|b| b.property == bound.property && Arc::ptr_eq(&b.source, &bound.source)) {
                        bound.seen = if tx.settle { None } else { previous.seen };
                        if bound.property == crate::live::Property::Fill { fill = Some(old.vis.fill); }
                    }
                }
                if let Some(fill) = fill { nodes[i].vis.fill = fill; }
                // Removing a binding restores the normal property, including transform defaults.
                for previous in &old.live {
                    if !nodes[i].live.iter().any(|b| b.property == previous.property) {
                        match previous.property {
                            crate::live::Property::Offset => self.scene.set(old.id, prop::OFFSET, rune_motion::Vec2::ZERO),
                            crate::live::Property::Scale => self.scene.set(old.id, prop::SCALE, 1.0),
                            _ => {}
                        }
                    }
                }
            }
        }
        if !any_gone {
            old_nodes.clear();
        } else {
            let old_ids: Vec<NodeId> = old_nodes.iter().map(|n| n.id).collect();
            // Only the nodes that left are moved out; the rest drop where they are.
            let gone: Vec<usize> = (0..old_nodes.len()).filter(|&oi| !self.node_index.contains_key(&old_ids[oi])).collect();
            let mut at = 0;
            old_nodes.retain(|_| { at += 1; gone.binary_search(&(at - 1)).is_ok() });
            for (oi, mut old) in gone.into_iter().zip(old_nodes.drain(..)) {
                if !self.scene.contains(old.id) {
                    self.hover_pruned(&old);
                    continue;
                }
                let old_parent_id = old.parent.and_then(|p| old_ids.get(p).copied());
                let parent_remained_present =
                    old_parent_id.is_none_or(|pid| self.node_index.get(&pid).is_some_and(|&i| i < present_len));
                if parent_remained_present {
                    let instant = instant || old_parent_id.and_then(|pid| self.node_index.get(&pid)).is_some_and(|&p| nodes[p].instant_changes);
                    if tx.settle || (instant && !self.is_exiting(old.id)) || drew_nothing.get(oi).copied().unwrap_or(false) {
                        self.scene.unmount(old.id);
                    } else {
                        if let Some(t) = old_parent_id.and_then(|pid| self.node_index.get(&pid)).and_then(|&p| nodes[p].vis.child_exit.as_deref().copied()) {
                            self.scene.set_transition(old.id, t);
                        }
                        self.scene.remove(old.id);
                    }
                } else if !tx.settle && self.flying.iter().any(|f| f.0 == old.id) {
                    // A flight paints outside its ancestors, so it can't fade with an exiting one.
                    self.scene.remove(old.id);
                }
                old.parent = old_parent_id.and_then(|pid| self.node_index.get(&pid).copied());
                old.live.clear();
                // Exit visuals outlive their model. Never call an accessor captured by
                // a deleted row while normalizing a later focus change.
                if let Some(input) = &mut old.input {
                    let was_revealed = input.edit.is_secure() && input.edit.is_revealed();
                    input.access = None;
                    input.key = None;
                    input.change = None;
                    input.edit.focused = false;
                    input.edit.clear_preedit();
                    input.edit.conceal();
                    if was_revealed && let Some(spec) = &mut old.text {
                        spec.replace_content(input.edit.display().0);
                        if input.edit.is_hidden_multiline() { spec.wrap = false; }
                        let scale = self.env.scale;
                        let w = (old.local.w - old.inset[0] - old.inset[2]).max(0.0);
                        let wrap = spec.wrap.then_some(w * scale);
                        old.text_layout = Some(self.text.layout_for_spec(spec, scale, wrap, line_alignment(spec)));
                        old.elided = None;
                    }
                } else if let Some(spec) = &mut old.text
                    && spec.redacted
                    && spec.secure_revealed
                {
                    spec.conceal();
                    let scale = self.env.scale;
                    let w = (old.local.w - old.inset[0] - old.inset[2]).max(0.0);
                    let wrap = spec.wrap.then_some(w * scale);
                    let mut layout = self.text.layout_for_spec(spec, scale, wrap, line_alignment(spec));
                    let mut elided = None;
                    if spec.truncate && layout.full_width() > w * scale + 0.5 {
                        let (cut, e) = truncated(&mut self.text, spec, scale, w * scale);
                        (layout, elided) = (cut, Some(e));
                    }
                    old.text_layout = Some(layout);
                    old.elided = elided;
                }
                // A resize that laid the tree out again took the text layouts before the view ran.
                if old.text_layout.is_none() { relayout::lay_out_text_in_place(&mut self.text, &mut old, self.env.scale); }
                self.node_index.insert(old.id, nodes.len());
                nodes.push(old);
            }
        }
        self.spare_nodes = old_nodes;

        if time_scaled {
            // Exits keep following their present ancestors' scale, so one under a frozen subtree
            // finishes once it resumes.
            self.sync_time_scales(&nodes, &mut time_scales, true);
            self.time_scaled = time_scales.iter().any(|&k| k != 1.0);
        }
        self.nodes = nodes;
        self.memo.commit_flattened(&self.nodes, present_len, memo_flattened, memo_reused);
        skip_spans.clear();
        self.memo.skip_scratch = skip_spans;
        self.find_app_root();
        self.commit_gen = self.commit_gen.wrapping_add(1);
        let (mut dissolve, mut cache, mut pix, mut drawn_ly, mut key_up, mut sticky, mut raise, mut any_loop) =
            (false, false, false, false, false, false, false, false);
        for n in &self.nodes {
            dissolve |= n.vis.blend == crate::render::Blend::PlusLighter;
            cache |= n.vis.cache;
            pix |= n.pixel_aspect.is_some();
            drawn_ly |= n.on_drawn_layout.is_some();
            key_up |= n.focus.key_up.is_some();
            sticky |= n.sticky.as_ref().is_some_and(|s| s.edge.is_some());
            raise |= n.vis.raise;
            any_loop |= n.looping.is_some();
        }
        self.has_dissolve = dissolve;
        self.has_cache = cache;
        if !self.has_cache {
            if !self.node_caches.is_empty() { self.node_caches.clear(); }
            if !self.text.atlas.resident_caches.is_empty() { self.text.atlas.resident_caches.clear(); }
        } else if !self.node_caches.is_empty() {
            self.node_caches.retain(|id, _| self.scene.contains(*id));
            self.text.atlas.resident_caches.retain(|id, _| self.scene.contains(NodeId(*id)));
        }
        self.has_pixel_aspect = pix;
        self.has_drawn_layout = drawn_ly;
        self.has_key_up = key_up;
        self.has_sticky = self.holding() || sticky;
        self.has_raise = raise;
        // A flight that left the tree stays lifted and covered while it exits, so it doesn't pop.
        self.flying.retain(|(id, _)| self.node_index.contains_key(id));
        self.launches.keep(&self.flying);
        if self.nodes.len() > present_len || !self.flying.is_empty() || self.has_sticky || self.has_raise { self.update_layer_paint_order(); }
        else { self.layer_paint_order.clear(); }
        self.built = false;
        self.virtuals.retain(|path, _| self.nodes.iter().any(|n| n.path == *path));
        self.prune_path_ids();
        self.subscribe_live();
        self.refresh_modal_index();
        self.sync_modals();
        self.sync_follows();
        self.loops.retain(|id, _| self.scene.contains(*id));
        self.catch_loops_up();
        self.static_scales.retain(|id, _| self.scene.contains(*id));
        if any_loop || !self.loops.is_empty() {
            for n in &self.nodes {
                let t = if time_scaled { self.scene.store().node_time(n.id) } else { self.t };
                if let Some(&spec) = n.looping.as_deref() {
                    let start = self.loop_start(n.id, t);
                    let track = self.loops.entry(n.id).and_modify(|l| { l.retime(spec, t); l.enabled(true, t); })
                        .or_insert_with(|| rune_motion::LoopTrack::new(spec, start));
                    if tx.settle || self.reduced_motion { track.finish(); }
                } else if let Some(l) = self.loops.get_mut(&n.id) {
                    l.enabled(false, t);
                }
            }
        }
        self.layout = taffy;
        self.reuse.built(self.env.scale, self.srgb_blend);
        self.text.end_commit();
        self.dirty = false;
        self.relayout = false;
        self.aspect_snap.clear();
        self.viewport_stepped = false;
        self.settle = false;
    }
}
