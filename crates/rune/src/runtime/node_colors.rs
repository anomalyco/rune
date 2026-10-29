//! A node's fill and text colours. Most never change, so their tracks are made only when one
//! does, and start from the colour last drawn.

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
}

/// The colour a node's text or icon draws in.
pub(super) fn text_color(n: &Node) -> Option<Color> {
    n.text.as_ref().map(|t| t.color).or(n.vis.icon.map(|(_, c)| c))
}

impl<S: 'static> Runtime<S> {
    /// Commits colour `to` on `p` of node `id`, which last drew `from` (None if it's new). A
    /// node without a track draws its own colour (`Runtime::color`), so it needs none until the
    /// colour changes, and a commit that leaves it changes nothing.
    pub(super) fn commit_color(&mut self, id: NodeId, p: Prop<Color>, from: Option<Color>, to: Color, s: Spring, instant: bool) {
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
#[path = "node_colors_tests.rs"]
mod tests;
