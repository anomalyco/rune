//! `El::hold_landing_width`: a column whose width springs (a side panel opening beside it) takes
//! its landing layout at once, so its text wraps once, and slides as one from where its first
//! child in view was drawn. The commit lands the held subtree's layout and gives the holding
//! element an offset of that child's jump, which springs back to its own offset.
use super::*;

/// The nodes the last commit held at their landing width, kept for the frame that draws them:
/// motion checks take that frame's re-layout as designed.
#[derive(Debug, Default)]
pub(in crate::runtime) struct LandingHolds(HashSet<NodeId>);

impl LandingHolds {
    pub(in crate::runtime) fn clear(&mut self) {
        self.0.clear();
    }

    pub(in crate::runtime) fn holds(&self, id: NodeId) -> bool {
        !self.0.is_empty() && self.0.contains(&id)
    }
}

impl tracks::MotionState {
    /// Marks in `held`, per node of `nodes`, those this commit lands under an
    /// `El::hold_landing_width` element whose width changes, and returns each such element's
    /// index with how far it shifts so its anchor child stays where it was drawn. `held` stays
    /// empty when nothing holds.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::runtime) fn landing_holds(
        &mut self,
        nodes: &[Node],
        old_nodes: &[Node],
        seen: &[bool],
        scene: &Scene,
        animated: bool,
        reduced_motion: bool,
        held: &mut Vec<bool>,
    ) -> Vec<(usize, f32)> {
        held.clear();
        let mut out = Vec::new();
        if !animated || reduced_motion {
            return out;
        }
        let mut last = paint_tracks::Finder::default();
        for (i, n) in nodes.iter().enumerate() {
            if !n.vis.cold.get().is_some_and(|c| c.hold_landing) || !scene.contains(n.id) {
                continue;
            }
            let Some(drawn) = scene.get(n.id, prop::FRAME) else { continue };
            let was = scene.store().target(n.id, prop::FRAME).unwrap_or(drawn);
            if (was.w - n.local.w).abs() < 0.5 {
                continue;
            }
            let end = subtree_end(nodes, i);
            let present: Vec<usize> = (i + 1..end)
                .filter(|&c| {
                    let cid = nodes[c].id;
                    nodes[c].parent == Some(i)
                        && scene.contains(cid)
                        && !matches!(scene.phase(cid), Some(PresencePhase::Exiting { .. }))
                })
                .collect();
            let anchor = present.iter().copied()
                .find(|&c| last.find(old_nodes, nodes[c].id).is_some_and(|o| Self::drew(old_nodes, seen, o)))
                .or(present.first().copied());
            let jump = anchor.and_then(|c| scene.get(nodes[c].id, prop::FRAME).map(|r| r.x - nodes[c].local.x)).unwrap_or(0.0);
            held.resize(nodes.len(), false);
            for (j, h) in held.iter_mut().enumerate().take(end).skip(i) {
                *h = true;
                self.landing_holds.0.insert(nodes[j].id);
            }
            out.push((i, drawn.x - n.local.x + jump));
        }
        out
    }

    /// Whether old node `o`, or anything under it, drew in the last build.
    fn drew(old_nodes: &[Node], seen: &[bool], o: usize) -> bool {
        seen.len() == old_nodes.len() && (o..subtree_end(old_nodes, o)).any(|j| seen[j])
    }
}

/// One past the last node of `i`'s subtree.
fn subtree_end(nodes: &[Node], i: usize) -> usize {
    (i + 1..nodes.len()).find(|&j| nodes[j].parent < Some(i)).unwrap_or(nodes.len())
}
