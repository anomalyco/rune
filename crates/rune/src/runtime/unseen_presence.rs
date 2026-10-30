//! Presence nobody would see. A removed node that drew nothing last frame has no exit to show: it
//! faded out already, sits under a parent that hid at once, or lies out of the window or its
//! clips with everything it holds (a filtered list's rows scrolled out of view). It goes at
//! once rather than fading where nothing shows, and what mounts under such a node, still out of
//! sight, enters at once. Likewise a virtual row that a change mounted with
//! its entrance but that landed out of view lands its entrance at once, and one nobody saw that
//! a filter brings into view enters like the rows new to it.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Per node of the last build: neither it nor anything under it drew. Empty before the first
    /// build and after a commit, until the next one draws.
    pub(super) fn drew_nothing(&self) -> Vec<bool> {
        if !self.built || self.seen.len() != self.nodes.len() || self.inh_scratch.len() != self.nodes.len() {
            return Vec::new();
        }
        let mut seen = self.seen.clone();
        mark_ancestors(&self.nodes, &mut seen);
        seen.iter().zip(&self.inh_scratch).map(|(&seen, h)| !seen || h.opacity < 1.0 / 512.0).collect()
    }

    /// How tall growth row `id` can be, in its own points, before its bottom passes the bottom
    /// of its clip (a scroll's viewport, the window), from where and how big it last drew
    /// (`El::fold_glide`); unbounded before it draws.
    pub(super) fn fold_limit(&self, id: NodeId) -> f32 {
        if !self.built || self.drawn.len() != self.nodes.len() { return f32::INFINITY; }
        let Some(i) = self.nodes.iter().position(|n| n.id == id) else { return f32::INFINITY };
        let Some(h) = self.grows.get(&id).map(|g| g.h).filter(|&h| h > 0.5) else { return f32::INFINITY };
        let d = self.drawn[i];
        let k = d[3] / h;
        if !(k > 0.01) { return f32::INFINITY; }
        let bottom = (self.screen_clip_of_parent(self.nodes[i].parent)[3] / self.env.scale).min(self.viewport.1);
        h + (bottom - d[1] - d[3]) / k
    }

    /// Node `i` of the commit mounts out of sight before and after (`unseen_before_and_after`),
    /// so its entrance lands. A matched element still flies to its match.
    pub(super) fn enters_unseen(&self, nodes: &[Node], unseen: &[bool], i: usize) -> bool {
        unseen.get(i).copied().unwrap_or(false) && nodes[i].matched.is_none()
    }

    /// Ends the exits that drew nothing last frame. A flight paints outside its clips, so it stays.
    pub(super) fn end_unseen_exits(&mut self) {
        if !self.built { return; }
        let mut hid: Option<Vec<bool>> = None;
        for i in 0..self.nodes.len() {
            let id = self.nodes[i].id;
            let faded = self.inh_scratch.get(i).is_some_and(|h| h.opacity < 1.0 / 512.0);
            if (!faded && self.seen.get(i).is_none_or(|&s| s)) || !self.is_exiting(id) || self.flying.iter().any(|f| f.0 == id) {
                continue;
            }
            if hid.get_or_insert_with(|| self.drew_nothing()).get(i).copied().unwrap_or(false) {
                self.scene.unmount(id);
            }
        }
    }

    /// Row `i` of the commit's `nodes`, kept by a change that replaced what the reader saw
    /// ([`crate::virtual_list::VirtualList::replaced_view`]) but drawn nowhere before, enters
    /// like the rows new to the view instead of appearing whole.
    pub(super) fn reenter_unseen_row(&mut self, nodes: &[Node], i: usize, unseen: &HashSet<NodeId>) {
        if !unseen.contains(&nodes[i].id) { return; }
        let Some(p) = nodes[i].parent else { return };
        let Some(v) = self.virtuals.get_mut(&nodes[p].path) else { return };
        let Some(key) = nodes[i].path.strip_prefix(nodes[p].path.as_str()).and_then(|k| k.strip_prefix('/')) else { return };
        if !v.replaced_view() { return; }
        v.entering.insert(key.to_owned());
        self.scene.reenter(nodes[i].id);
    }

    /// Virtual rows mounted with their entrance have just drawn once: the ones out of view land.
    /// Its own fade starts a row at no opacity, so only where it drew counts, strictly: a row
    /// touching its viewport's edge shows nothing. Runs after each draw, since a list filling its
    /// window commits again on the frames that follow.
    pub(super) fn land_unseen_entrances(&mut self) {
        if self.virtuals.values().all(|v| v.entering.is_empty()) { return; }
        let s = self.env.scale;
        let (w, h) = self.viewport;
        let mut land = Vec::new();
        for (path, v) in &mut self.virtuals {
            for key in v.entering.drain() {
                let Some(&id) = self.path_ids.get(&format!("{path}/{key}")) else { continue };
                let Some(&i) = self.node_index.get(&id) else { continue };
                let Some(&[x, y, rw, rh]) = self.drawn.get(i) else { continue };
                let clip = screen_clip_of(self.nodes[i].parent, &self.inh_scratch, &self.turn_scratch);
                let shows = (x + rw).min(clip[2] / s).min(w) > x.max(clip[0] / s).max(0.0)
                    && (y + rh).min(clip[3] / s).min(h) > y.max(clip[1] / s).max(0.0);
                if !shows { land.push(id); }
            }
        }
        for id in land { self.scene.enter_now(id); }
    }

    /// Per node of the new tree `nodes`: neither it nor anything under it drew in the last
    /// build, and none of its swept path lands in sight in this layout (in the window, inside
    /// its clips). Empty when nothing non-skipped qualifies.
    pub(super) fn unseen_before_and_after(&self, nodes: &[Node], skip: &[memo_reuse::CleanApplySpan]) -> Vec<bool> {
        if !self.built || self.seen.len() != self.nodes.len() || self.seen.iter().all(|&s| s) {
            return Vec::new();
        }
        let mut seen = self.seen.clone();
        mark_ancestors(&self.nodes, &mut seen);
        let any_active_unseen = self.nodes.iter().zip(&seen).any(|(old, &s)| {
            !s && self.node_index.get(&old.id).is_some_and(|&ni| !memo_reuse::is_in_clean_spans(skip, ni))
        });
        if !any_active_unseen { return Vec::new(); }
        let before: HashSet<NodeId> = self.nodes.iter().zip(&seen).filter(|(_, s)| !**s).map(|(n, _)| n.id).collect();
        if before.is_empty() { return Vec::new(); }
        let old_drawn: HashMap<NodeId, [f32; 4]> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, old)| old.target[2] > old.target[0] || old.target[3] > old.target[1])
            .map(|(old_i, old)| (old.id, self.drawn_edges(old_i)))
            .collect();
        // Room for shadows and blur, which draw past a node's rect.
        const MARGIN: f32 = 32.0;
        let window = [0.0, 0.0, self.viewport.0, self.viewport.1];
        // Per node: the swept clip and scroll-offset range its children are under, in points.
        let mut inner = vec![(window, [0.0f32; 2], [0.0f32; 2]); nodes.len()];
        let mut after = vec![false; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            let (clip, shift_min, shift_max) = match n.parent {
                Some(p) if n.above.is_none() && p < i => inner[p],
                Some(_) if n.above.is_none() => { after[i] = true; (window, [0.0; 2], [0.0; 2]) }
                _ => (window, [0.0; 2], [0.0; 2]),
            };
            let t = n.target;
            let r = [t[0] - shift_max[0], t[1] - shift_max[1], t[2] - shift_min[0], t[3] - shift_min[1]];
            let swept = match old_drawn.get(&n.id) {
                Some(&b) => [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])],
                None => r,
            };
            after[i] |= swept[2] >= clip[0] - MARGIN && swept[0] <= clip[2] + MARGIN
                && swept[3] >= clip[1] - MARGIN && swept[1] <= clip[3] + MARGIN;
            let clip = if n.vis.clip.is_some() || n.scroll.is_some() {
                intersect(clip, swept)
            } else { clip };
            let (shift_min, shift_max) = match (self.scrolls.get(&n.id), n.scroll) {
                (Some(st), Some(sp)) => {
                    let children = || (i + 1..nodes.len()).take_while(|&j| nodes[j].parent >= Some(i)).filter(|&j| nodes[j].parent == Some(i));
                    let max_x = if n.scroll_axes.x() { children().fold(0.0_f32, |m, c| m.max(nodes[c].target[2] - n.target[2])) } else { 0.0 };
                    let max_y = if n.scroll_axes.y() { children().fold(0.0_f32, |m, c| m.max(nodes[c].target[3] - n.target[3])) } else { 0.0 };
                    let ox0 = st.x.offset as f32;
                    let ox1 = ox0.clamp(0.0, max_x);
                    let oy0 = st.offset as f32 + st.slack.height();
                    let oy1 = if st.pinned && sp.stick_bottom { max_y + st.slack.height() } else { oy0.clamp(0.0, max_y + st.slack.height()) };
                    (
                        [shift_min[0] + ox0.min(ox1), shift_min[1] + oy0.min(oy1)],
                        [shift_max[0] + ox0.max(ox1), shift_max[1] + oy0.max(oy1)],
                    )
                }
                _ => (shift_min, shift_max),
            };
            inner[i] = (clip, shift_min, shift_max);
        }
        mark_ancestors(nodes, &mut after);
        // A node new under one nobody saw before or after (a ticking digit in a row scrolled out
        // of view) enters where nothing shows, so it lands at once too.
        let old: HashSet<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        let mut unseen = vec![false; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            let new_under_unseen = !old.contains(&n.id) && n.parent.is_some_and(|p| p < i && unseen[p]);
            unseen[i] = !after[i] && (before.contains(&n.id) || new_under_unseen);
        }
        unseen
    }
}
