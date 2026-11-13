//! Dropping nodes whose exit has finished from the flat node list.

use super::*;

/// Reveal states kept for texts that left the tree, newest first: a keyed child that comes back
/// under its old path still shows its words (r349). One that never comes back would otherwise
/// keep its entry, and its path, for the window's life, so only this many stay and the oldest
/// leave.
pub(super) const REVEALS_KEPT: usize = 64;

impl<S: 'static> Runtime<S> {
    pub(super) fn prune_exited(&mut self) {
        // Fills last commit's spare buffer and keeps this one as the next spare: a few thousand
        // nodes would otherwise grow a fresh Vec, megabytes, on every frame a node exits.
        self.nodes_replaced();
        let mut old = std::mem::replace(&mut self.nodes, std::mem::take(&mut self.spare_nodes));
        self.nodes.clear();
        self.nodes.reserve(old.len());
        self.has_dissolve = false;
        self.has_cache = false;
        let mut remap = std::mem::take(&mut self.apply_scratch.remap);
        remap.clear();
        remap.resize(old.len(), None);
        self.node_index.clear();
        for (old_idx, mut n) in old.drain(..).enumerate() {
            if !self.scene.contains(n.id) {
                self.hover_pruned(&n);
                self.reveals.remove(&n.id);
                self.scrolls.remove(&n.id);
                self.loops.remove(&n.id);
                if !self.node_caches.is_empty() {
                    self.node_caches.remove(&n.id);
                    self.text.atlas.resident_caches.remove(&n.id.0);
                }
                continue;
            }
            n.parent = n.parent.and_then(|p| remap[p]);
            let idx = self.nodes.len();
            remap[old_idx] = Some(idx);
            self.node_index.insert(n.id, idx);
            self.has_dissolve |= n.vis_flags.blend() == crate::render::Blend::PlusLighter;
            self.has_cache |= n.vis_flags.cache();
            self.nodes.push(n);
        }
        self.spare_nodes = old;
        // A text's reveal state and the id its path maps to outlive the node, so a keyed child
        // that comes back under its old path keeps its words (r349). Nodes that leave the commit
        // never reach the loop above, so nothing else would ever drop those entries: keep the
        // newest `REVEALS_KEPT` that left and drop the rest, the oldest first. Their paths leave
        // with the next commit's `prune_path_ids`, which keeps a path only while it holds state.
        let mut kept = std::mem::take(&mut self.reveals_kept);
        kept.retain(|id| self.reveals.contains_key(id) && !self.node_index.contains_key(id));
        let mut gone: Vec<NodeId> = self.reveals.keys().copied()
            .filter(|id| !self.node_index.contains_key(id) && !kept.contains(id))
            .collect();
        gone.sort_unstable_by(|a, b| b.cmp(a));
        gone.append(&mut kept);
        if gone.len() > REVEALS_KEPT {
            for id in gone.split_off(REVEALS_KEPT) { self.reveals.remove(&id); }
        }
        self.reveals_kept = gone;
        self.memo.on_prune(&remap);
        remap.clear();
        self.apply_scratch.remap = remap;
        self.virtuals.retain(|path, _| self.nodes.iter().any(|n| n.path == *path));
        self.refresh_modal_index();
        self.flying.retain(|(id, _)| self.node_index.contains_key(id));
        self.has_raise = self.nodes.iter().any(|n| n.vis_flags.raise());
        self.has_paint_order = self.nodes.iter().any(|n| n.vis.paint_order != 0);
        if !self.layer_paint_order.is_empty() || !self.flying.is_empty() || self.has_raise || self.has_paint_order { self.update_layer_paint_order(); }
        // A removed element's live sources (a `Pixels` stream) stop waking this window.
        if !self.live_subscriptions.is_empty() {
            let keep = self.live.dirty();
            self.subscribe_live();
            self.live.pruned(keep);
        }
    }
}
