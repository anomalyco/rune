//! Rest-time memory release across the runtime's owning subsystems and companion thread-local caches.

use std::cell::RefCell;

use super::*;

thread_local! {
    static REST_HOOKS: RefCell<Vec<fn()>> = const { RefCell::new(Vec::new()) };
}

/// Registers a thread-local cache rest callback (for companion crates such as `rune-kit`).
/// Deduplicated by function pointer; runs once per window rest on this thread.
#[doc(hidden)]
pub fn register_thread_rest_hook(hook: fn()) {
    REST_HOOKS.try_with(|hooks| {
        if let Ok(mut list) = hooks.try_borrow_mut()
            && !list.iter().any(|&f| std::ptr::fn_addr_eq(f, hook))
        {
            list.push(hook);
        }
    }).ok();
}

fn run_thread_rest_hooks() {
    let mut i = 0;
    loop {
        let hook = REST_HOOKS
            .try_with(|hooks| hooks.try_borrow().ok().and_then(|list| list.get(i).copied()))
            .ok()
            .flatten();
        let Some(f) = hook else { break };
        f();
        i += 1;
    }
}

fn shrink_vec<T>(v: &mut Vec<T>, floor: usize) {
    let cap = (v.len().saturating_mul(2)).max(floor);
    if v.capacity() > cap {
        v.shrink_to(cap);
    }
}

fn shrink_map<K: Eq + std::hash::Hash, V, S: std::hash::BuildHasher>(m: &mut std::collections::HashMap<K, V, S>, floor: usize) {
    let cap = (m.len().saturating_mul(2)).max(floor);
    if m.capacity() > cap {
        m.shrink_to(cap);
    }
}

impl types::TreeState {
    pub(super) fn rest(&mut self) {
        let live = self.nodes.len();
        shrink_vec(&mut self.nodes, 64);
        // Keep spare_nodes large enough to ping-pong the current tree without reallocating on
        // the next commit, while releasing excess capacity from an earlier large view.
        self.spare_nodes.clear();
        let spare_cap = live.max(64);
        if self.spare_nodes.capacity() > spare_cap.saturating_mul(2) {
            self.spare_nodes.shrink_to(spare_cap);
        }
        self.node_index.rest(&self.nodes);
        shrink_vec(&mut self.built_ids, 64);
        shrink_map(&mut self.path_ids, 64);
        shrink_vec(&mut self.aspect_snap, 16);
        self.key_index.rest();
    }
}

impl hit::HitState {
    pub(super) fn rest(&mut self, live_nodes: usize) {
        let nf = (live_nodes.saturating_mul(2)).max(64);
        shrink_vec(&mut self.hits, nf);
        shrink_vec(&mut self.wheel_hits, nf);
        shrink_vec(&mut self.overlay_hits, 32);
        shrink_vec(&mut self.help_hits, 32);
        shrink_vec(&mut self.drop_hits, 32);
    }
}

impl requests::ScrollTables {
    pub(super) fn rest(&mut self) {
        shrink_vec(&mut self.stuck, 32);
        shrink_vec(&mut self.sticky_frame, 32);
        shrink_vec(&mut self.sticky_bands, 32);
        shrink_map(&mut self.scrolls, 16);
    }
}

impl RuntimeCore {
    /// Releases cached layouts, dormant scene slots, spare node/paint/memo capacity and companion
    /// thread-local caches not needed to redraw the current view. Called once when a window or
    /// overlay rests (`window::rest`).
    pub(crate) fn rest(&mut self) {
        self.text.rest();
        self.images.rest();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(rewind) = self.motion.rewind() {
            rewind.rest();
        }
        self.prune_path_ids();
        self.tree.rest();
        self.commit.rest();
        let live_nodes = self.tree.nodes.len();
        let live_inst = self.instances.len();
        shrink_vec(&mut self.instances, live_inst.max(128));
        shrink_vec(&mut self.marks, live_inst.max(128));
        shrink_vec(&mut self.surfaces, 16);
        self.hit.rest(live_nodes);
        self.scroll.rest();
        self.paint.rest(live_nodes, live_inst, &self.tree.node_index);
        self.scene.shrink_to_live();
        if let Some(a11y) = self.host.a11y_mut() {
            a11y.rest();
        }
        run_thread_rest_hooks();
    }
}

#[cfg(test)]
#[path = "rest_tests.rs"]
mod tests;
