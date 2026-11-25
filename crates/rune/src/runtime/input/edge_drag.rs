//! `El::on_edge_drag`: a finger that lands in a zone along a window edge and first moves away
//! from it drives the innermost such ancestor on its hit path, before anything under the finger
//! (a child's `on_drag`, a scroll viewport's pan, a press) takes the touch. `touch.rs` holds the
//! finger's mode; this finds the candidate when it lands and builds what its handler hears.
use crate::SwipeEdge;

use super::*;

/// Whether a first move of `(dx, dy)` from the press goes mostly away from `edge`.
pub(in crate::runtime) fn moves_away(edge: SwipeEdge, dx: f32, dy: f32) -> bool {
    match edge {
        SwipeEdge::Left => dx > dy.abs(),
        SwipeEdge::Right => -dx > dy.abs(),
        SwipeEdge::Top => dy > dx.abs(),
        SwipeEdge::Bottom => -dy > dx.abs(),
    }
}

/// Whether window point (x, y) lies within `width` points of `edge` of a `viewport` whose safe
/// area insets are `safe` (top, right, bottom, left).
pub(in crate::runtime) fn in_zone(edge: SwipeEdge, width: f32, viewport: (f32, f32), safe: [f32; 4], x: f32, y: f32) -> bool {
    match edge {
        SwipeEdge::Left => x <= safe[3] + width,
        SwipeEdge::Right => x >= viewport.0 - safe[1] - width,
        SwipeEdge::Top => y <= safe[0] + width,
        SwipeEdge::Bottom => y >= viewport.1 - safe[2] - width,
    }
}

impl RuntimeCore {
    /// The innermost live, allowed `on_edge_drag` element whose zone holds a finger landing at
    /// (x, y), that holds the point and lies on the hit path of what's under it.
    pub(in crate::runtime) fn edge_drag_candidate(&self, x: f32, y: f32) -> Option<(NodeId, SwipeEdge)> {
        if !self.tree.nodes.iter().any(|n| n.handlers.edge_drag.is_some()) { return None; }
        let (viewport, safe) = (self.viewport, crate::runtime::cx::safe_area_insets());
        let top = self.top_target(x, y);
        let depth = |i: usize| std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent).count();
        self.tree.nodes.iter().enumerate()
            .filter_map(|(i, n)| n.handlers.edge_drag.as_ref().map(|&(edge, width, _)| (i, edge, width)))
            .filter(|&(i, edge, width)| {
                let t = self.tree.nodes[i].target;
                let [lx, ly] = self.to_local(i, x, y);
                in_zone(edge, width, viewport, safe, x, y)
                    && (0.0..=t[2] - t[0]).contains(&lx) && (0.0..=t[3] - t[1]).contains(&ly)
                    && top.is_none_or(|t| self.node_holds(i, t))
                    && !self.subtree_exiting(i) && !self.subtree_unmounted(i) && self.allowed(i)
            })
            .max_by_key(|&(i, ..)| depth(i))
            .map(|(i, edge, _)| (self.tree.nodes[i].id, edge))
    }

    /// Queues `phase` of the edge drag on node `id` for a finger that landed at `origin` and
    /// is now at (x, y). Nothing when the element has left.
    pub(in crate::runtime) fn queue_edge_drag(&mut self, id: NodeId, phase: Phase, origin: (f32, f32), (x, y): (f32, f32), cancelled: bool) {
        let Some(&i) = self.tree.node_index.get(&id) else { return };
        let n = &self.tree.nodes[i];
        let r = n.target;
        let [lx, ly] = self.to_local(i, x, y);
        let [sx, sy] = self.to_local(i, origin.0, origin.1);
        let d = Drag {
            phase,
            x: lx,
            y: ly,
            w: r[2] - r[0],
            h: r[3] - r[1],
            start_x: sx,
            start_y: sy,
            dx: x - origin.0,
            dy: y - origin.1,
            data: 0.0,
            clicks: 1,
            cancelled,
            rtl: n.flow == crate::el::Flow::Rtl,
            window: [x, y],
            screen: self.screen_at(x, y),
        };
        self.frame.queue.push(Action::EdgeDrag(n.path.clone(), d));
    }
}

#[cfg(test)]
#[path = "edge_drag_tests.rs"]
mod tests;
