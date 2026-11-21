//! The viewport and the panel that gave it room share the incoming row's actual height
//! track. Only local vertical edges that changed at their seam need a presentation delta.
use super::*;

#[derive(Clone, Debug, Default)]
pub(in crate::runtime) enum Handoff {
    #[default]
    Idle,
    Growing { row: NodeId, viewport: NodeId, frames: Vec<(NodeId, rune_motion::Rect, f32, f32)>, rows: Vec<(NodeId, f32, f32)> },
}

impl types::TreeState {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn tail_handoff(
        &self,
        scene: &Scene,
        grows: &HashMap<NodeId, GrowTrack>,
        nodes: &[Node],
        stack: usize,
        viewport: usize,
        row: &str,
        room: &str,
    ) -> Option<Handoff> {
        let (r, panel) = nodes.iter().enumerate().find(|(_, n)| n.named(room))?;
        let view = &nodes[viewport];
        let old = |id| self.nodes.iter().find(|n| n.id == id);
        let (a, b) = (old(view.id)?.target, old(panel.id)?.target);
        let close = |x: f32, y: f32| (x - y).abs() < 1.0 / 64.0;
        if panel.target[1] <= b[1] || !close(a[3], b[1]) || !close(view.target[3], panel.target[1])
            || !close(panel.target[3], b[3]) || a[..3] != view.target[..3] { return None; }
        let row = nodes.iter().find(|n| n.parent == Some(stack) && n.key.as_deref() == Some(row) && !n.virtual_mount && !scene.contains(n.id))?;
        let ancestors = |k| std::iter::successors(Some(k), |&j: &usize| nodes[j].parent);
        let mut chain: HashSet<usize> = ancestors(viewport).collect();
        chain.extend(ancestors(r));
        let end = r + 1 + nodes[r+1..].iter().take_while(|n| n.parent >= Some(r)).count();
        let frames = nodes.iter().enumerate().filter(|(k,n)| chain.contains(k) || (r..end).contains(k)
            || (n.parent == panel.parent && old(n.id).is_some_and(|o| o.target == n.target)))
            .filter_map(|(_,n)| {
                let was = scene.get(n.id, prop::FRAME)?;
                let (dy, dh) = (was.y - n.local.y, was.h - n.local.h);
                (dy != 0.0 || dh != 0.0).then_some((n.id, n.local, dy, dh))
            }).collect();
        // A kept row can lose its footer to the arriving row. Tie that height to the same
        // progress too: independent whole-row springs land at different absolute tolerances.
        let rows = nodes.iter().filter(|n| n.parent == Some(stack) && n.whole && !n.streaming && n.id != row.id)
            .filter_map(|n| { let from = grows.get(&n.id)?.h;
                (from != n.local.h).then_some((n.id, from, n.local.h)) }).collect();
        Some(Handoff::Growing { row: row.id, viewport: view.id, frames, rows })
    }
}

impl Handoff {
    pub(in crate::runtime) fn place(&mut self, grows: &mut HashMap<NodeId, GrowTrack>, scene: &mut rune_motion::Scene, nodes: &[Node], index: &NodeIndex, scrolls: &mut HashMap<NodeId, ScrollState>) -> bool {
        let Self::Growing { row, viewport, frames, rows } = self else { return false };
        let (left, rate) = grows.get(row).filter(|g| g.target > 0.0).map_or((0.0, 0.0), |g| ((1.0 - g.h / g.target).clamp(0.0, 1.0), g.v / g.target));
        for &(id, from, to) in rows.iter() {
            if let Some(g) = grows.get_mut(&id) && g.target == to { g.h = to + (from - to) * left; g.v = (to - from) * rate; }
        }
        // The edge was already paid for by the row. Don't add the same space to slack.
        if let Some(s) = scrolls.get_mut(viewport) { s.slack.follow = true; }
        for &(id, target, dy, dh) in frames.iter() {
            let Some(n) = index.get(&id).map(|&k| &nodes[k]) else { continue };
            // A later measurement (e.g. typing or a resize) owns its new geometry.
            if n.local.y != target.y || n.local.h != target.h { continue; }
            scene.set(id, prop::FRAME, rune_motion::Rect { y: target.y + dy * left, h: target.h + dh * left, ..n.local });
        }
        if left == 0.0 { *self = Self::Idle; }
        true
    }
}
