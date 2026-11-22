//! Where a frame presents each growth row: its top offset, its height and its bottom feather
//! mask, per node index, from its stack's tracks.
use super::*;

impl RuntimeCore {
    pub(in crate::runtime) fn growth_feather(&self, i: usize) -> f32 {
        let width = (self.paint.row_mask[i] * self.env.scale).clamp(0.0, 65504.0);
        if self.motion.grows.get(&self.tree.nodes[i].id).is_some_and(|g| g.linear_edge) { -width } else { width }
    }

    /// Fills `row_dy`, `row_h` and `row_mask` for this frame's nodes.
    pub(in crate::runtime) fn place_growth_rows(&mut self) {
        let (s, n_nodes) = (self.env.scale, self.tree.nodes.len());
        self.paint.row_dy.clear();
        self.paint.row_dy.resize(n_nodes, 0.0);
        self.paint.row_h.clear();
        self.paint.row_h.resize(n_nodes, -1.0);
        self.paint.row_mask.clear();
        self.paint.row_mask.resize(n_nodes, -1.0);
        self.paint.clear_row_lifts();
        let mut emptied = Vec::new();
        for (cid, stack) in &self.motion.stacks {
            let Some(&ci) = self.tree.node_index.get(cid) else { continue };
            let Some(spec) = self.tree.nodes[ci].growth.as_deref().copied() else { continue };
            let origin = self.commit.virtuals.get(&self.tree.nodes[ci].path).map_or(0.0, |v| v.before);
            self.paint.row_dy[ci] = grid(origin, s);
            let mut y = stack.pad_top - origin;
            let mut first_row = true;
            for id in &stack.order {
                let Some(g) = self.motion.grows.get(id) else { continue };
                if !first_row { y += stack.gap; }
                first_row = false;
                let top_rel = stack.tops.get(id).copied().unwrap_or(0.0) - origin;
                let cur = y;
                if let Some(&ri) = self.tree.node_index.get(id) {
                    self.paint.row_dy[ri] = cur - top_rel;
                    self.paint.row_h[ri] = g.h;
                    let withheld = if g.streaming { (self.tree.nodes[ri].local.h - g.target).max(0.0) } else { 0.0 };
                    let rewrap = stack.pending_heights.contains_key(id) && self.tree.nodes[ri].local.h > g.h + 0.01;
                    let overflows = spec.edge.overflow && (g.streaming || g.paced());
                    // A fold that landed shut keeps what it folded away where it drew, lifted
                    // and clipped, until that has left: let go as it lands, the contents would
                    // drop back to their layout for the frames their exit has left.
                    let landed = g.shut() && !g.moving();
                    let folded = landed && self.holds_exiting(ri);
                    if landed && !folded { emptied.push(*id); }
                    if spec.edge.clip && !overflows && (g.h < g.target - 0.01 || g.mask > 0.01 || withheld > 0.01 || rewrap || g.shutting_fold() || folded) {
                        self.paint.row_mask[ri] = g.mask.max(withheld.min(spec.edge.floor)).max(if rewrap { spec.edge.floor } else { 0.0 });
                    }
                    let lift = if landed && !folded { 0.0 } else { g.lift() };
                    if lift > 0.0 {
                        self.paint.push_row_lift(ri, lift);
                    }
                }
                y = cur + g.h;
            }
            // The stack itself presents the rows' heights, which scrolling follows.
            self.paint.row_h[ci] = y + stack.pad_bottom;
            for (id, dy) in stack.slides.shifts() {
                if let Some(&i) = self.tree.node_index.get(&id) { self.paint.row_dy[i] += dy; }
            }
        }
        for id in emptied {
            if let Some(g) = self.motion.grows.get_mut(&id) { g.forget_folded(); }
        }
        // A fold's lifted contents (`GrowTrack::hide_above`) draw that far up inside it, all but
        // the header it shuts under.
        for idx in 0..self.paint.row_lifts().len() {
            let (ri, lift) = self.paint.row_lifts()[idx];
            let mut k = ri + 1;
            while k < n_nodes && self.tree.nodes[k].parent.is_some_and(|p| p >= ri) {
                if self.tree.nodes[k].parent == Some(ri) { self.paint.row_dy[k] -= lift; }
                k += 1;
            }
            let header = self.motion.fold_headers.get(&self.tree.nodes[ri].id).and_then(|p| self.tree.path_ids.get(p)).and_then(|id| self.tree.node_index.get(id));
            if let Some(&h) = header.filter(|&&h| h > ri && h < k) { self.paint.row_dy[h] += lift; }
        }
        self.place_joined_rows();
        if !self.motion.fold_headers.is_empty() {
            let lifted: Vec<NodeId> = self.paint.row_lifts().iter().map(|&(ri, _)| self.tree.nodes[ri].id).collect();
            self.motion.fold_headers.retain(|id, _| lifted.contains(id));
        }
    }

    /// Something inside row `ri` is leaving. Leaving nodes follow the ones that stay, so they
    /// needn't sit in one run after it.
    fn holds_exiting(&self, ri: usize) -> bool {
        let inside = |mut i: usize| loop {
            match self.tree.nodes[i].parent {
                Some(p) if p == ri => return true,
                Some(p) if p > ri => i = p,
                _ => return false,
            }
        };
        (ri + 1..self.tree.nodes.len()).any(|i| inside(i) && self.is_exiting(self.tree.nodes[i].id))
    }
}
