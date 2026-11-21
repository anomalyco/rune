//! Dropping detached nodes and pruning per-node tables across commits and finished exits.

use super::*;

/// Reveal states kept for texts that left the tree, newest first: a keyed child that comes back
/// under its old path still shows its words (r349). One that never comes back would otherwise
/// keep its entry, and its path, for the window's life, so only this many stay and the oldest
/// leave.
pub(in crate::runtime) const REVEALS_KEPT: usize = 64;

impl RuntimeCore {
    /// Forgets the ids of paths that left with nothing kept under their id, so a session that
    /// mounts many distinct keys (a long virtual list scrolled end to end) doesn't keep them all.
    /// A path whose id still holds scroll, growth or reveal state keeps it, so it comes back
    /// with that state as before.
    pub(in crate::runtime) fn prune_path_ids(&mut self) {
        if self.tree.path_ids.len() <= self.tree.node_index.len() { return; }
        let Self { tree, scroll, motion, .. } = self;
        tree.path_ids.retain(|_, id| {
            tree.node_index.contains_key(id) || scroll.scrolls.contains_key(id) || motion.stacks.contains_key(id) || motion.grows.contains_key(id) || motion.has_reveal(*id)
        });
    }

    /// Recomputes node-derived presence flags in a single pass over `self.nodes`.
    /// Returns whether any node has a loop spec.
    pub(in crate::runtime) fn refresh_node_flags(&mut self) -> bool {
        let (mut dissolve, mut cache, mut pix, mut drawn_ly, mut key_up, mut sticky, mut raise, mut any_loop) =
            (false, false, false, false, false, false, false, false);
        let mut ordered = false;
        for n in &self.tree.nodes {
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
        self.tree.has_dissolve = dissolve;
        self.tree.has_cache = cache;
        self.tree.has_pixel_aspect = pix;
        self.tree.has_drawn_layout = drawn_ly;
        self.tree.has_key_up = key_up;
        self.tree.has_sticky = self.holding() || sticky;
        self.tree.has_raise = raise;
        self.tree.has_paint_order = ordered;
        any_loop
    }

    pub(in crate::runtime) fn prune_detached_tables(&mut self, prune_paths: bool) -> bool {
        let any_loop = self.refresh_node_flags();
        self.paint.prune_caches(self.tree.has_cache, &self.scene, &mut self.text.atlas.resident_caches);
        self.paint.prune_static_scales(&self.scene);
        self.motion.prune_detached(&self.scene, &self.tree.node_index, REVEALS_KEPT);
        if !self.commit.virtuals.is_empty() {
            let Self { commit, tree, .. } = self;
            commit.virtuals.retain(|path, _| tree.path_ids.get(path).is_some_and(|id| tree.node_index.contains_key(id)));
        }
        if prune_paths {
            self.prune_path_ids();
        }
        self.refresh_modal_index();
        any_loop
    }

    pub(in crate::runtime) fn prune_exited(&mut self) {
        // Fills last commit's spare buffer and keeps this one as the next spare: a few thousand
        // nodes would otherwise grow a fresh Vec, megabytes, on every frame a node exits.
        self.nodes_replaced();
        let mut old = std::mem::replace(&mut self.tree.nodes, std::mem::take(&mut self.tree.spare_nodes));
        self.tree.nodes.clear();
        self.tree.nodes.reserve(old.len());
        let mut remap = std::mem::take(&mut self.commit.apply_scratch.remap);
        remap.clear();
        remap.resize(old.len(), None);
        self.tree.node_index.rebuild(old.len());
        for (old_idx, mut n) in old.drain(..).enumerate() {
            if !self.scene.contains(n.id) {
                self.hover_pruned(&n);
                self.motion.remove_reveal(n.id);
                self.scroll.scrolls.remove(&n.id);
                continue;
            }
            n.parent = n.parent.and_then(|p| remap[p]);
            let idx = self.tree.nodes.len();
            remap[old_idx] = Some(idx);
            self.tree.node_index.insert(n.id, idx);
            self.tree.nodes.push(n);
        }
        self.tree.spare_nodes = old;
        self.commit.memo.on_prune(&remap);
        remap.clear();
        self.commit.apply_scratch.remap = remap;
        self.prune_detached_tables(false);
        if !self.paint.layer_paint_order().is_empty() || !self.motion.flying.is_empty() || self.tree.has_sticky || self.tree.has_raise || self.tree.has_paint_order { self.update_layer_paint_order(); }
        // A removed element's live sources (a `Pixels` stream) stop waking this window.
        if self.frame.has_live_subscriptions() {
            let keep = self.frame.live.dirty();
            self.subscribe_live();
            self.frame.live.pruned(keep);
        }
    }
}
