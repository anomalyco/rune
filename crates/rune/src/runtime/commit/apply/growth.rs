//! Growth stack reconciliation during `RuntimeCore::apply`: merging row order, retargeting
//! `GrowTrack`s, closing/reopening folds, reorder jumps and live-growth row tracking.
use super::*;

impl RuntimeCore {
    pub(super) fn apply_growth_stacks(
        &mut self,
        start: usize,
        nodes: &[Node],
        taffy: &LayoutEngine,
        tx: &Tx,
        scratch: &mut apply_scratch::ApplyScratch,
        held: &[bool],
        unseen_ids: &HashSet<NodeId>,
        first: bool,
        concealing: bool,
        instant: bool,
        cut: bool,
        now: f64,
        scale: f32,
        live_by_id: &mut HashMap<NodeId, row_reorder::LiveRow>,
    ) {
        for c in start..nodes.len() {
            if nodes[c].cold.get().is_none_or(|nd| nd.growth.is_none()) { continue }
            self.apply_growth_stack(
                c, nodes, taffy, tx, scratch, held, unseen_ids,
                first, concealing, instant, cut, now, scale, live_by_id,
            );
        }
    }

    fn apply_growth_stack(
        &mut self,
        c: usize,
        nodes: &[Node],
        taffy: &LayoutEngine,
        tx: &Tx,
        scratch: &mut apply_scratch::ApplyScratch,
        held: &[bool],
        unseen_ids: &HashSet<NodeId>,
        first: bool,
        concealing: bool,
        instant: bool,
        cut: bool,
        now: f64,
        scale: f32,
        live_by_id: &mut HashMap<NodeId, row_reorder::LiveRow>,
    ) {
        let cid = nodes[c].id;
        let instant = instant || nodes[c].instant_changes;
        let cut = cut || nodes[c].instant_changes;
        // A reader away from the tail is anchored using measured row heights. Land those
        // heights with the anchor correction, unless `keep_on_screen` or `scroll_into_view`
        // is driving this viewport across the commit. A reveal left from an earlier commit
        // keeps only rows already growing on their spring: new rows land, or under a parent
        // whose size springs too, the two springs carry them past their slots.
        // So does a reveal in this commit for rows new to the stack: rows inserted above an
        // anchored first row and revealed at once land whole instead of growing from zero.
        let revealing = self.commit.virtuals.get(&nodes[c].path)
            .is_some_and(|v| self.scroll.requests.reveals().any(|r| r.viewport == v.scroll_path));
        let revealed_now = self.commit.virtuals.get(&nodes[c].path).is_some_and(|v| {
            tx.scroll_to.iter().any(|r| {
                nodes.iter().enumerate().find(|(_, n)| n.named(&r.key)
                    && n.parent.is_none_or(|p| in_scope(&nodes[p].path, r.scope.as_deref())))
                    .is_some_and(|(i, _)| {
                        std::iter::successors(nodes[i].parent, |&p| nodes[p].parent)
                            .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y())
                            .is_some_and(|p| nodes[p].path == v.scroll_path)
                    })
            })
        });
        let reader = self.commit.virtuals.get(&nodes[c].path)
            .filter(|v| !self.scroll.requests.keeps().any(|k| k.viewport == v.scroll_path))
            .and_then(|v| self.tree.path_ids.get(&v.scroll_path))
            .and_then(|id| self.scroll.scrolls.get(id)).is_some_and(|s| !s.pinned());
        let reading = reader && !revealed_now;
        scratch.rows.clear();
        scratch.rows.extend((c + 1..nodes.len()).filter(|&j| nodes[j].parent == Some(c)));
        let rows = &scratch.rows;
        let existed = self.motion.stacks.contains_key(&cid) && !tx.commit.lands();
        let mut stack = self.motion.stacks.remove(&cid).unwrap_or_default();
        scratch.new_ids.clear();
        scratch.new_ids.extend(rows.iter().map(|&j| nodes[j].id));
        let new_ids = &scratch.new_ids;
        scratch.new_set.clear();
        scratch.new_set.extend(new_ids.iter().copied());
        let new_set = &scratch.new_set;
        // Merge: keep leaving rows where they stood. In a virtual list, a row that drew
        // nothing, or any row when the change keeps none the reader could see (a filter),
        // leaves no space to close: the list's anchor already holds what shows (`set_keys`),
        // and the space would only push the rows the change brings in down, out of view.
        let list = self.commit.virtuals.get(&nodes[c].path);
        let is_virtual = list.is_some();
        let replaced = list.is_some_and(crate::el::virtual_list::VirtualList::replaced_view);
        let (mut opened, mut shut) = (Vec::new(), Vec::new());
        let mut order = std::mem::take(&mut scratch.order);
        order.clear();
        order.reserve(new_ids.len() + 4);
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
            } else if let Some(g) = self.motion.grows.get(&old).filter(|g| g.h > 0.01) && existed && !cut && !replaced && !(is_virtual && unseen_ids.contains(&old)) {
                // A row fades in place before its space closes, except a fold, which shuts
                // at once on its own spring under its feather, as it would fold shut in place.
                let hold = if g.glide().is_some() { 0.0 } else { 0.15 };
                if g.glide().is_some() && !stack.closing.contains_key(&old) { shut.push(old); }
                stack.closing.entry(old).or_insert(now + hold);
                order.push(old);
            } else {
                self.motion.grows.remove(&old);
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
            self.motion.grows.get(&id).is_some_and(|g| g.moving()) || stack.pending_heights.contains_key(&id)
        });
        // A person resizing the window holds the content: rewrapped rows land in the frame,
        // so a pinned tail stays where it is instead of sliding back after the drag. A
        // streaming row still waits for the width to settle, then eases. A stack held at its
        // landing width (`El::hold_landing_width`) lands its rewrapped rows the same way.
        let resized = existed && (stack.window != self.viewport || held.get(c).copied().unwrap_or(false));
        stack.window = self.viewport;
        if first || !existed || tx.commit.lands() || self.reduced_motion || (reading && !revealing_moving) {
            stack.width_hold = WidthHold::Off;
            stack.pending_heights.clear();
        } else if (width - stack.width).abs() > 0.25 {
            stack.width_hold = WidthHold::rewrap(now);
        }
        let live_rows = if existed && !first && !resized && !concealing && !self.reduced_motion && !tx.live_growth.is_empty() && !stack.width_hold.defers(now) {
            self.live_tail(nodes, c, tx, &mut stack)
        } else { None };
        stack.viewport_height = list.map_or(0.0, |v| v.viewport);
        stack.width = width;
        stack.pending_heights.retain(|id, _| new_set.contains(id));
        stack.gap = crate::el::virtual_list::gap_y(taffy.style(nodes[c].taffy));
        // With every row gone, the leaving rows still stand below the stack's top padding.
        let pad_top = rows.first().map_or(stack.pad_top, |&j| nodes[j].target[1] - ctop);
        stack.pad_bottom = rows.last().map_or(nodes[c].target[3] - ctop - pad_top, |&j| nodes[c].target[3] - nodes[j].target[3]);
        stack.pad_top = pad_top;
        let (mut hidden_cut, mut reopened, mut held_folds) = (0.0, Vec::new(), Vec::new());
        for &j in rows {
            let id = nodes[j].id;
            let mut h = nodes[j].target[3] - nodes[j].target[1];
            stack.closing.remove(&id);
            stack.tops.insert(id, nodes[j].target[1] - ctop);
            let streaming = nodes[j].streaming;
            let whole = nodes[j].whole && !streaming;
            let g_moving = self.motion.grows.get(&id).is_some_and(|g| g.moving()) || stack.pending_heights.contains_key(&id);
            let entering = revealed_now && reader && !self.motion.grows.contains_key(&id);
            let row_reading = (reading && !(revealing && g_moving)) || entering;
            let live = !row_reading && live_rows.as_ref().is_some_and(|range| {
                nodes[j].key.as_deref().and_then(|key| list?.index_of(key)).is_some_and(|index| range.contains(&index))
            });
            if streaming && existed && !tx.commit.lands() && !first && !self.reduced_motion && !row_reading {
                // Measure the final wrapped text before mutating the growth track. Targets
                // retain full layout height; only the presented stack extent lags the fill.
                let mut end = j + 1;
                while end < nodes.len() && nodes[end].parent.is_some_and(|p| p >= j) { end += 1; }
                if let Some(n) = nodes[j..end].iter().rev().find(|n| n.text.as_ref().is_some_and(|t| t.wrap && t.reveal >= 0.0))
                    && let Some(laid) = &n.text_layout
                {
                    let width = (n.target[2] - n.target[0] - n.inset[0] - n.inset[2]) * scale;
                    h = (h - laid.view().stream_height_lag(width, scale)).max(0.0);
                }
            }
            let (duration, glide) = (tx.motion.map_or(0.0, |m| m.spring().duration * 1.15), nodes[j].glide.map(|s| (s, self.fold_limit(id))));
            let hidden = if glide.is_some() { self.fold_hidden_above(nodes, j, h) } else { None };
            if existed && !cut && !nodes[j].virtual_mount && nodes[j].glide.is_some() && !self.motion.grows.contains_key(&id) { opened.push(id); }
            let g = self.motion.grows.entry(id).or_insert_with(|| {
                if existed && (!cut || live) && !nodes[j].virtual_mount { GrowTrack::settled(0.0).with_duration(duration) } else { GrowTrack::settled(h) }
            });
            g.set_kind(streaming, whole, glide);
            let snap_change = ((instant && !live) || resized) && !streaming && (h - g.target).abs() > 1e-4;
            if tx.commit.lands() || !existed || first || self.reduced_motion || snap_change || row_reading {
                *g = g.restarted(h);
                stack.pending_heights.remove(&id);
                stack.slides.land(id);
            } else {
                // Only a width/scale rewrap may shrink an active streaming row, including
                // when fresh text arrives in the same frame as the resize.
                let t = if streaming && !self.commit.relayout && stack.width_hold.may_shrink(now) { h.max(g.h.min(g.target)) } else { h };
                if stack.width_hold.defers(now) {
                    stack.pending_heights.insert(id, t);
                } else {
                    stack.pending_heights.remove(&id);
                    if (t - g.target).abs() > 1e-4 { g.duration = duration; g.linear_edge = live; }
                    let shutting = t < g.target - 1e-4;
                    // A fold opening again while its contents draw lifted, or opening from
                    // rest (not one mounting): `ride_from_end` grows it from its bottom where
                    // its viewport follows its end, and leaves the rest alone.
                    if t > g.target + 1e-4 && (g.lift() > 0.0 || (g.glide().is_some() && g.target > 1e-4)) { reopened.push((j, g.give.is_some())); }
                    let before = g.h;
                    g.retarget(t);
                    if let Some((above, header)) = hidden.filter(|_| shutting && g.shutting_fold()) {
                        held_folds.push((id, before - g.h));
                        hidden_cut += g.hide_above(above);
                        self.motion.insert_fold_header(id, header);
                    }
                }
            }
        }
        if !tx.commit.lands() && !first && !self.reduced_motion { self.motion.link_growth_runs(&mut stack, &order, &opened, &shut); } else { stack.runs.clear(); }
        if hidden_cut > 0.0 { self.scroll_by_cut(nodes, c, hidden_cut); }
        if !held_folds.is_empty() { self.give_way(nodes, c, &std::mem::take(&mut held_folds)); }
        for (j, giving) in reopened.drain(..) { self.ride_from_end(nodes, c, j, giving); }
        if let Some(&last) = rows.last()
            && !self.scene.contains(nodes[last].id)
            && let Some(s) = std::iter::successors(nodes[c].parent, |&p| nodes[p].parent)
                .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y())
        {
            self.scroll.requests.step(requests::Event::NewTail { viewport: &nodes[s].path, key: &nodes[last].path });
        }
        stack.growing = !stack.pending_heights.is_empty()
            || order.iter().any(|r| self.motion.grows.get(r).is_some_and(|g| g.target > g.h + 1.0 / 128.0));
        stack.moving = stack.growing
            || order.iter().any(|r| self.motion.grows.get(r).is_some_and(|g| g.moving()));
        // Order holds the rows this commit brought and the ones still closing; no others.
        stack.tops.retain(|id, _| new_set.contains(id) || stack.closing.contains_key(id));
        let animates = existed && (!instant || live_rows.is_some()) && !tx.relayout && !self.reduced_motion;
        stack.reorder.measure(animates, &stack.order, &order, stack.gap, |id| self.motion.grows.get(&id).map(|g| g.h));
        if first || self.reduced_motion || concealing || resized || reader || nodes[c].instant_changes || tx.commit.lands() || (stack.order != order && live_rows.is_none()) { stack.seams.clear(); }
        if let Some(range) = live_rows && let Some(v) = self.commit.virtuals.get(&nodes[c].path) && stack.order != order {
            self.join_live_rows(&mut stack, nodes, rows, v, &range);
            let ink = tx.live_growth.iter().any(|(key, ink)| *ink && nodes[c].named(key));
            live_by_id.extend(rows.iter().enumerate().filter_map(|(k, &j)| {
                let moves = |&n: &usize| stack.reorder.jump(nodes[n].id) != 0.0;
                let joins = ink && (rows.get(k.wrapping_sub(1)).is_some_and(moves) || rows.get(k + 1).is_some_and(moves));
                self.live_row(&nodes[j], &stack.reorder, ink, joins, v, &range)
            }));
        }
        scratch.order = std::mem::replace(&mut stack.order, order);
        self.motion.stacks.insert(cid, stack);
    }
}
