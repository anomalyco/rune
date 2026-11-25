//! Rest-time compaction of dormant scene node slots and scratch buffers.

use super::{FxMap, NONE, Scene};

fn shrink_vec<T>(v: &mut Vec<T>, floor: usize) {
    let cap = (v.len().saturating_mul(2)).max(floor);
    if v.capacity() > cap {
        v.shrink_to(cap);
    }
}

impl Scene {
    /// Compacts dead node slots, animation tracks and scratch buffers to match live nodes when
    /// idle (nothing exiting, travelling or animating). Idempotent: a second call allocates and
    /// moves nothing.
    pub fn shrink_to_live(&mut self) {
        if !self.exiting.is_empty() || !self.travelling.is_empty() || self.store.is_animating_any() {
            return;
        }
        let live = self.index.len();
        let node_floor = (live.saturating_mul(2)).max(64);
        let scene_sparse = self.nodes.capacity() > node_floor
            || self.rests.capacity() > node_floor
            || self.free.capacity() > 64
            || self.index.capacity() > node_floor
            || self.order.capacity() > node_floor
            || self.out.capacity() > node_floor
            || self.late.capacity() > 64
            || self.exiting.capacity() > 64
            || self.to_remove.capacity() > 64
            || self.travelling.capacity() > 64
            || self.intents.capacity() > 64
            || self.handovers.capacity() > 64
            || self.transition_motion.capacity() > (self.transition_motion.len().saturating_mul(2)).max(64);

        if scene_sparse {
            let old_len = self.nodes.len();
            let mut remap = vec![NONE; old_len];
            let mut new_nodes = Vec::with_capacity(live);
            let mut new_rests = Vec::with_capacity(live);
            for (old_idx, (node, rest)) in self.nodes.drain(..).zip(self.rests.drain(..)).enumerate() {
                if !node.alive {
                    continue;
                }
                let new_idx = new_nodes.len() as u32;
                remap[old_idx] = new_idx;
                new_nodes.push(node);
                new_rests.push(rest);
            }
            let map_slot = |slot: u32| -> u32 {
                if slot == NONE { NONE } else { remap[slot as usize] }
            };
            let mut new_index = FxMap::with_capacity_and_hasher(live, Default::default());
            for (i, node) in new_nodes.iter_mut().enumerate() {
                node.parent = map_slot(node.parent);
                node.first_child = map_slot(node.first_child);
                node.last_child = map_slot(node.last_child);
                node.next_sibling = map_slot(node.next_sibling);
                node.prev_sibling = map_slot(node.prev_sibling);
                new_index.insert(node.id.0, i as u32);
            }
            self.root_head = map_slot(self.root_head);
            self.root_tail = map_slot(self.root_tail);
            self.nodes = new_nodes;
            self.rests = new_rests;
            self.index = new_index;
            self.free = Vec::new();
            self.order.clear();
            shrink_vec(&mut self.order, live.max(16));
            self.order_dirty = true;
            shrink_vec(&mut self.out, live.max(16));
            shrink_vec(&mut self.late, 16);
            shrink_vec(&mut self.exiting, 16);
            self.exited.shrink();
            shrink_vec(&mut self.to_remove, 16);
            shrink_vec(&mut self.travelling, 16);
            shrink_vec(&mut self.intents, 16);
            shrink_vec(&mut self.handovers, 16);
            let tm_cap = (self.transition_motion.len().saturating_mul(2)).max(16);
            if self.transition_motion.capacity() > tm_cap {
                self.transition_motion.shrink_to(tm_cap);
            }
        }

        if let Some(track_remap) = self.store.compact_with_remap() {
            for node in &mut self.nodes {
                for slot in &mut node.tracks {
                    if let Some(id) = slot {
                        if let Some(&new_idx) = track_remap.get(id.index_usize()) && new_idx != NONE {
                            id.set_index(new_idx);
                        } else {
                            *slot = None;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
