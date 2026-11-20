//! What a live-edit reload reports about a running window: its scroll offsets and moving frames.
use super::*;

impl types::TreeState {
    pub(super) fn hot_scrolls(&self, scrolls: &HashMap<NodeId, ScrollState>) -> Vec<(String, f64)> {
        self.nodes.iter().filter_map(|n| scrolls.get(&n.id).map(|s| (n.path.clone(), s.offset))).collect()
    }

    pub(crate) fn hot_motion(&self, scene: &Scene) -> Vec<(String, [f32; 4], [f32; 4])> {
        let store = scene.store();
        self.nodes.iter().filter_map(|n| {
            if !store.is_animating(n.id, prop::FRAME) { return None; }
            Some((n.path.clone(), store.sample(n.id, prop::FRAME)?.edges(), store.sample_velocity(n.id, prop::FRAME)?.edges()))
        }).collect()
    }
}

impl RuntimeCore {
    #[inline]
    pub(crate) fn hot_scrolls(&self) -> Vec<(String, f64)> {
        self.tree.hot_scrolls(&self.scroll.scrolls)
    }
}
