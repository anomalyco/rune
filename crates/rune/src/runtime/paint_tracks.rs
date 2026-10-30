//! A node's paint: colours, shadow geometry, hover and press. Most of it never changes, so a
//! property gets a track only when it does, starting from what was last drawn.

use super::*;

/// Finds new nodes in the last build. Both trees are mostly in the same order, so it walks
/// them together and builds a map only on the first node out of step.
#[derive(Default)]
pub(super) struct LastBuild {
    next: usize,
    map: Option<HashMap<NodeId, usize>>,
}

impl LastBuild {
    pub(super) fn find(&mut self, old: &[Node], id: NodeId) -> Option<usize> {
        let at = match old.get(self.next) {
            Some(n) if n.id == id => Some(self.next),
            _ => self.map.get_or_insert_with(|| old.iter().enumerate().map(|(i, n)| (n.id, i)).collect()).get(&id).copied(),
        }?;
        self.next = at + 1;
        Some(at)
    }

    pub(super) fn advance(&mut self, next: usize) {
        self.next = next;
    }
}

/// Resolves `old` against `old_nodes`, falling back to `nodes[i]` when `old` was moved out of
/// `old_nodes` by a `same_path` memo reuse (leaving its paint and text fields unchanged on `nodes[i]`).
#[inline]
pub(super) fn prev_node<'a>(old_nodes: &'a [Node], nodes: &'a [Node], old: Option<usize>, i: usize) -> Option<&'a Node> {
    old.map(|o| if old_nodes[o].is_moved_stub() { &nodes[i] } else { &old_nodes[o] })
}

/// The colour a node's text or icon draws in.
pub(super) fn text_color(n: &Node) -> Option<Color> {
    n.text.as_ref().map(|t| t.color).or(n.vis.icon.map(|(_, c)| c))
}

/// Readies `p` (hover, press, press scale), which reads `rest` without a track, to move to
/// `to`: false when it has no track and `to` is `rest`, so there's nothing to do; otherwise it
/// has a track, made at `rest` if it had none.
pub(super) fn ready_from_rest(scene: &mut Scene, id: NodeId, p: Prop<f32>, rest: f32, to: f32) -> bool {
    if scene.store().has(id, p) {
        return true;
    }
    if to == rest {
        return false;
    }
    scene.set(id, p, rest);
    true
}

/// A shadow's geometry as its track holds it.
pub(super) fn shadow_geo(sh: &crate::el::Shadow) -> rune_motion::Rect {
    rune_motion::Rect { x: sh.x, y: sh.y, w: sh.blur, h: sh.spread }
}

impl<S: 'static> Runtime<S> {
    /// Commits `to` on paint property `p` of node `id`, which last drew `from` (None if it's
    /// new). Without a track a node draws its own value (`Runtime::color`, `shadow_at`), so it
    /// needs none until the value changes, and a commit that leaves it changes nothing.
    pub(super) fn commit_paint<T: rune_motion::Animatable + PartialEq>(&mut self, id: NodeId, p: Prop<T>, from: Option<T>, to: T, s: Spring, instant: bool) {
        let tracked = self.scene.store().has(id, p);
        if from == Some(to) && !(instant && tracked) {
            // Its track, if any, already heads to `to`: nothing else moves this property.
            return;
        }
        if !tracked {
            match from {
                Some(from) if !instant => self.scene.set(id, p, from),
                _ => return,
            }
        }
        self.set_or_anim(id, p, to, s, instant);
    }

}

#[cfg(test)]
#[path = "paint_tracks_tests.rs"]
mod tests;
