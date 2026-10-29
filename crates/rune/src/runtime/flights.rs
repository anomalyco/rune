//! Elements that fly to their place on one screen path: `El::matched` pairs and `Tx::launch`.
use super::*;

/// `Tx::launch` flights: destinations waiting for the next commit, each with the screen point
/// its source's text started at, and the ones in the air.
#[derive(Default)]
pub(super) struct Launches {
    pending: Vec<(String, [f32; 2])>,
    flying: Vec<Launched>,
}

/// A launch in the air, the offsets outside the scene it was started against, and the ones
/// its last frame drew it under.
struct Launched {
    id: NodeId,
    shift: [f32; 2],
    last: Option<[f32; 2]>,
}

impl Launches {
    pub(super) fn pending(&self) -> bool { !self.pending.is_empty() }

    /// Draws outside every clip, at its own size, until it lands.
    pub(super) fn flying(&self, id: NodeId) -> bool { self.flying.iter().any(|l| l.id == id) }

    /// Before its destination's layout moves mid-flight and its path restarts from where it is:
    /// it starts again under the offsets it was last drawn under, so the restart doesn't jump.
    pub(super) fn restart(&mut self, scene: &Scene, id: NodeId) {
        let Some(l) = self.flying.iter_mut().find(|l| l.id == id) else { return };
        let Some(last) = l.last else { return };
        let travel = scene.get(id, prop::TRAVEL).unwrap_or(rune_motion::Vec2::new(1.0, 1.0));
        l.shift = [last[0] + (l.shift[0] - last[0]) * (1.0 - travel.x), last[1] + (l.shift[1] - last[1]) * (1.0 - travel.y)];
    }

    /// Drops launches that landed (left `flying`).
    pub(super) fn keep(&mut self, flying: &[(NodeId, MRect)]) {
        self.flying.retain(|l| flying.iter().any(|f| f.0 == l.id));
    }

    /// A launch leaves from where its source was seen and flies to where its destination is
    /// now: it starts under the offsets outside the scene it launched against and ends under
    /// this frame's `offset`, blending by its travel. A growth row above still resizing, a
    /// moved virtual window, or a row whose slot moves under an offset that hides the move
    /// (an inserted row's growth) shifts where it lands, never where it is. What to add to it.
    pub(super) fn offset_fix(&mut self, scene: &Scene, id: NodeId, offset: [f32; 2]) -> [f32; 2] {
        let Some(l) = self.flying.iter_mut().find(|l| l.id == id) else { return [0.0; 2] };
        l.last = Some(offset);
        let travel = scene.get(id, prop::TRAVEL).unwrap_or(rune_motion::Vec2::new(1.0, 1.0));
        [(l.shift[0] - offset[0]) * (1.0 - travel.x), (l.shift[1] - offset[1]) * (1.0 - travel.y)]
    }
}

impl<S: 'static> Runtime<S> {
    /// Shared-element motion (`El::matched`): each of the last commit's matched elements that
    /// left the tree hands its geometry, radius, fill and opacity to the element with its id that
    /// just mounted. Costs nothing without matched elements; else O(matched), plus one pass over
    /// the old nodes when a pair matches.
    pub(super) fn match_elements(&mut self, nodes: &[Node], arrivals: Vec<(Rc<str>, usize, bool)>, tx: &Tx, instant: bool) {
        let mut by_id: HashMap<Rc<str>, (usize, bool)> = HashMap::with_capacity(arrivals.len());
        for (key, i, arriving) in arrivals {
            if by_id.insert(key.clone(), (i, arriving)).is_some() { self.warn_duplicate_match(&key); }
        }
        let mut current: Vec<(Rc<str>, NodeId)> = by_id.iter().map(|(key, &(i, _))| (key.clone(), nodes[i].id)).collect();
        current.sort_unstable_by_key(|(key, _)| by_id[key].0);
        let old = std::mem::replace(&mut self.matched, current);
        // Under Reduce Motion the old one exits and the new one enters, each by its own
        // transition (a crossfade in place) and with the parents it leaves or enters with.
        if instant || tx.settle || self.reduced_motion { return; }
        let mut old_index: Option<HashMap<NodeId, usize>> = None;
        // Descendants first: matching an element unmounts its old subtree, matched ones included.
        for (key, from) in old.into_iter().rev() {
            let Some(&(i, true)) = by_id.get(&key) else { continue };
            let to = nodes[i].id;
            if from == to || self.node_index.contains_key(&from) || !self.scene.contains(from) || self.is_exiting(from) { continue; }
            let index = old_index.get_or_insert_with(|| self.nodes.iter().enumerate().map(|(k, n)| (n.id, k)).collect());
            let Some(&from_i) = index.get(&from) else { continue };
            // Scroll offsets live outside the scene: start where `from` was seen on screen.
            let mut offset = rune_motion::Vec2::ZERO;
            if self.built {
                // Flights drop the cover offsets of flights around them.
                let shift = |k: usize| self.inh_scratch.get(k).map_or([0.0; 2], |h| [h.dx - h.cover[0], h.dy - h.cover[1]]);
                let source = self.nodes[from_i].parent.map_or([0.0; 2], shift);
                let row = self.row_dy.get(from_i).copied().unwrap_or(0.0);
                let dest = std::iter::successors(nodes[i].parent, |&p| nodes[p].parent)
                    .find_map(|p| index.get(&nodes[p].id).copied()).map_or([0.0; 2], shift);
                offset = rune_motion::Vec2::new(source[0] - dest[0], source[1] + row - dest[1]);
            }
            let spring = tx.motion.or(nodes[i].matched.as_ref().and_then(|m| m.1)).unwrap_or(motion::REFLOW);
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
            self.flying.retain(|f| f.0 != to);
            self.flying.push((to, nodes[i].local));
        }
    }

    /// `Tx::launch`, before the commit: where the source's text starts on screen.
    pub(super) fn capture_launch(&mut self, source: String, target: String) {
        let Some((i, n)) = self.nodes.iter().enumerate().find(|(_, n)| crate::flight::selects(&source, &n.path, n.key.as_deref())) else { return };
        let (Some(spec), Some(layout), Some(r)) = (&n.text, &n.text_layout, self.drawn.get(i)) else { return };
        let (_, origin) = text_extent(n, spec, layout, *r, self.env.scale);
        self.launches.pending.push((target, origin));
    }

    /// `Tx::launch`, in the commit: each destination that just mounted starts with its first
    /// text where its source's text was seen, at its own size, and flies to its place unclipped.
    pub(super) fn launch_elements(&mut self, nodes: &[Node], tx: &Tx, instant: bool) {
        let launches = std::mem::take(&mut self.launches.pending);
        if instant || tx.settle || self.reduced_motion { return; }
        let index: HashMap<NodeId, usize> = self.nodes.iter().enumerate().map(|(k, n)| (n.id, k)).collect();
        let built: HashMap<NodeId, usize> = self.built_ids.iter().enumerate().map(|(k, &id)| (id, k)).collect();
        let scale = self.env.scale;
        for (target, from) in launches {
            let Some(i) = nodes.iter().position(|n| crate::flight::selects(&target, &n.path, n.key.as_deref())) else { continue };
            let to = nodes[i].id;
            if index.contains_key(&to) && !self.is_exiting(to) { continue; }
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
                let (Some(spec), Some(layout)) = (&nodes[j].text, &nodes[j].text_layout) else { return None };
                let r = nodes[j].target;
                let (_, origin) = text_extent(&nodes[j], spec, layout, [r[0], r[1], r[2] - r[0], r[3] - r[1]], scale);
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
            let spring = tx.motion.unwrap_or(motion::REFLOW);
            self.scene.launch(to, start, LayoutAnim::new(spring).screen());
            self.flying.retain(|f| f.0 != to);
            self.flying.push((to, nodes[i].local));
            self.launches.flying.retain(|l| l.id != to);
            self.launches.flying.push(Launched { id: to, shift, last: None });
        }
    }

    fn warn_duplicate_match(&mut self, key: &Rc<str>) {
        let message = || format!("duplicate matched id {key:?}: the last element in tree order is matched; give each element its own id");
        if let Some(warnings) = self.key_warnings.as_mut() { warnings.push(message()); }
        #[cfg(debug_assertions)]
        if self.matched_warned.insert(key.clone()) { eprintln!("rune: {}", message()); }
    }
}
