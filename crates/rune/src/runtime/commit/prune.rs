//! Dropping detached nodes and pruning per-node tables across commits and finished exits.

use super::*;

/// Reveal states kept for texts that left the tree, newest first: a keyed child that comes back
/// under its old path still shows its words (r349). One that never comes back would otherwise
/// keep its entry, and its path, for the window's life, so only this many stay and the oldest
/// leave.
pub(super) const REVEALS_KEPT: usize = 64;

impl RuntimeCore {
    /// Forgets the ids of paths that left with nothing kept under their id, so a session that
    /// mounts many distinct keys (a long virtual list scrolled end to end) doesn't keep them all.
    /// A path whose id still holds scroll, growth or reveal state keeps it, so it comes back
    /// with that state as before.
    pub(super) fn prune_path_ids(&mut self) {
        if self.path_ids.len() <= self.node_index.len() { return; }
        let Self { path_ids, node_index, scrolls, stacks, grows, reveals, .. } = self;
        path_ids.retain(|_, id| {
            node_index.contains_key(id) || scrolls.contains_key(id) || stacks.contains_key(id) || grows.contains_key(id) || reveals.contains_key(id)
        });
    }

    /// Recomputes node-derived presence flags in a single pass over `self.nodes`.
    /// Returns whether any node has a loop spec.
    pub(super) fn refresh_node_flags(&mut self) -> bool {
        let (mut dissolve, mut cache, mut pix, mut drawn_ly, mut key_up, mut sticky, mut raise, mut any_loop) =
            (false, false, false, false, false, false, false, false);
        let mut ordered = false;
        for n in &self.nodes {
            dissolve |= n.vis_flags.blend() == crate::render::Blend::PlusLighter;
            cache |= n.vis_flags.cache();
            if let Some(c) = n.cold.get() {
                pix |= c.pixel_aspect.is_some();
                sticky |= c.sticky.as_ref().is_some_and(|s| s.edge.is_some());
                any_loop |= c.looping.is_some();
            }
            drawn_ly |= n.handlers.get().is_some_and(|h| h.on_drawn_layout.is_some() || h.on_visible.is_some() || h.drawn_rect.is_some());
            key_up |= n.focus.get().is_some_and(|f| f.key_up.is_some());
            raise |= n.vis_flags.raise();
            ordered |= n.vis.paint_order != 0;
        }
        self.has_dissolve = dissolve;
        self.has_cache = cache;
        self.has_pixel_aspect = pix;
        self.has_drawn_layout = drawn_ly;
        self.has_key_up = key_up;
        self.has_sticky = self.holding() || sticky;
        self.has_raise = raise;
        self.has_paint_order = ordered;
        any_loop
    }

    /// Keeps at most the newest `REVEALS_KEPT` reveal states for texts that have left `self.nodes`,
    /// dropping the oldest first.
    fn cap_reveals(&mut self) {
        if self.reveals.is_empty() {
            self.reveals_kept.clear();
            return;
        }
        // A text's reveal state and the id its path maps to outlive the node, so a keyed child
        // that comes back under its old path keeps its words (r349). Nodes dropped during an
        // instant parent unmount leave their reveal behind, so bound those orphans here.
        let mut kept = std::mem::take(&mut self.reveals_kept);
        kept.retain(|id| self.reveals.contains_key(id) && !self.node_index.contains_key(id));
        let mut gone: Vec<NodeId> = self.reveals.keys().copied()
            .filter(|id| !self.node_index.contains_key(id) && !kept.contains(id))
            .collect();
        if !gone.is_empty() {
            gone.sort_unstable_by(|a, b| b.cmp(a));
            gone.append(&mut kept);
            kept = gone;
        }
        if kept.len() > REVEALS_KEPT {
            for id in kept.split_off(REVEALS_KEPT) { self.reveals.remove(&id); }
        }
        self.reveals_kept = kept;
    }

    /// Recomputes node-derived flags and prunes tables keyed by detached nodes.
    /// Shared by commit apply (`prune_paths = true`) and finished-exit pruning (`prune_paths = false`).
    ///
    /// Returns whether any node in `self.nodes` carries a loop spec.
    ///
    /// Finished-exit pruning leaves `path_ids` reclamation to the next commit: `path_ids` cannot
    /// grow without a commit, and keeping a directly-unmounted path until a subsequent commit lets
    /// an immediate remount reclaim its id.
    pub(super) fn prune_detached_tables(&mut self, prune_paths: bool) -> bool {
        let any_loop = self.refresh_node_flags();
        if !self.has_cache {
            if !self.node_caches.is_empty() { self.node_caches.clear(); }
            if !self.text.atlas.resident_caches.is_empty() { self.text.atlas.resident_caches.clear(); }
        } else {
            if !self.node_caches.is_empty() {
                self.node_caches.retain(|id, _| self.scene.contains(*id));
            }
            if !self.text.atlas.resident_caches.is_empty() {
                self.text.atlas.resident_caches.retain(|id, _| self.scene.contains(NodeId(*id)));
            }
        }
        if !self.loops.is_empty() {
            self.loops.retain(|id, _| self.scene.contains(*id));
        }
        if !self.static_scales.is_empty() {
            self.static_scales.retain(|id, _| self.scene.contains(*id));
        }
        self.cap_reveals();
        // A flight that left the tree stays lifted and covered while it exits, so it doesn't pop.
        self.flying.retain(|(id, _)| self.node_index.contains_key(id));
        self.launches.keep(&self.flying);
        if !self.virtuals.is_empty() {
            let Self { virtuals, path_ids, node_index, .. } = self;
            virtuals.retain(|path, _| path_ids.get(path).is_some_and(|id| node_index.contains_key(id)));
        }
        if prune_paths {
            self.prune_path_ids();
        }
        self.refresh_modal_index();
        any_loop
    }

    pub(super) fn prune_exited(&mut self) {
        // Fills last commit's spare buffer and keeps this one as the next spare: a few thousand
        // nodes would otherwise grow a fresh Vec, megabytes, on every frame a node exits.
        self.nodes_replaced();
        let mut old = std::mem::replace(&mut self.nodes, std::mem::take(&mut self.spare_nodes));
        self.nodes.clear();
        self.nodes.reserve(old.len());
        let mut remap = std::mem::take(&mut self.apply_scratch.remap);
        remap.clear();
        remap.resize(old.len(), None);
        self.node_index.clear();
        for (old_idx, mut n) in old.drain(..).enumerate() {
            if !self.scene.contains(n.id) {
                self.hover_pruned(&n);
                self.reveals.remove(&n.id);
                self.scrolls.remove(&n.id);
                continue;
            }
            n.parent = n.parent.and_then(|p| remap[p]);
            let idx = self.nodes.len();
            remap[old_idx] = Some(idx);
            self.node_index.insert(n.id, idx);
            self.nodes.push(n);
        }
        self.spare_nodes = old;
        self.memo.on_prune(&remap);
        remap.clear();
        self.apply_scratch.remap = remap;
        self.prune_detached_tables(false);
        if !self.layer_paint_order.is_empty() || !self.flying.is_empty() || self.has_sticky || self.has_raise || self.has_paint_order { self.update_layer_paint_order(); }
        // A removed element's live sources (a `Pixels` stream) stop waking this window.
        if !self.live_subscriptions.is_empty() {
            let keep = self.live.dirty();
            self.subscribe_live();
            self.live.pruned(keep);
        }
    }
}

#[cfg(test)]
mod prune_tests;
