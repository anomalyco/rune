//! `El::on_edge_drag`: a finger that lands in a zone along a window edge and first moves away
//! from it drives the innermost such ancestor on its hit path, before anything under the finger
//! (a child's `on_drag`, a scroll viewport's pan, a press) takes the touch. `touch_machine.rs`
//! owns the finger's mode; this resolves per-edge candidates via `chain_start`, builds `Drag`
//! events, and tracks the active fallback handler so unmount, inert, Escape, focus loss, modals
//! and `cancel_pointer` always deliver a cancelled `Phase::Up`.
use crate::SwipeEdge;

use super::*;

/// Innermost matching `on_edge_drag` node per physical [`SwipeEdge`], indexed by `SwipeEdge::index`.
pub(in crate::runtime) type EdgeCandidates = [Option<NodeId>; 4];

/// An active `El::on_edge_drag` gesture's live target and fallback handler, kept in lockstep
/// with `Mode::Edge` so cancellation always delivers a cancelled `Phase::Up`.
pub(in crate::runtime) struct ActiveEdge {
    pub(in crate::runtime) node: NodeId,
    pub(in crate::runtime) path: String,
    pub(in crate::runtime) edge: SwipeEdge,
    pub(in crate::runtime) origin: (f32, f32),
    pub(in crate::runtime) pos: (f32, f32),
    pub(in crate::runtime) last_drag: Drag,
    pub(in crate::runtime) handler: crate::el::DragFn,
}

/// Whether `candidates` holds at least one armed edge candidate.
#[inline]
pub(in crate::runtime) fn any_candidate(candidates: EdgeCandidates) -> bool {
    candidates.iter().any(Option::is_some)
}

/// Which window edge a first move of `(dx, dy)` goes mostly away from (±45° inward cone),
/// or `None` on an exact 45° diagonal or zero move.
pub(in crate::runtime) fn moves_away_edge(dx: f32, dy: f32) -> Option<SwipeEdge> {
    if dx > dy.abs() {
        Some(SwipeEdge::Left)
    } else if -dx > dy.abs() {
        Some(SwipeEdge::Right)
    } else if dy > dx.abs() {
        Some(SwipeEdge::Top)
    } else if -dy > dx.abs() {
        Some(SwipeEdge::Bottom)
    } else {
        None
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
    /// For each physical [`SwipeEdge`], the innermost live, allowed `on_edge_drag` ancestor on
    /// the `chain_start(x, y)` hit path whose zone and layout bounds hold `(x, y)`. Respects
    /// paint order, clipping, opacity, raised overlays, and open modals.
    pub(in crate::runtime) fn edge_drag_candidates(&self, x: f32, y: f32) -> EdgeCandidates {
        let mut candidates: EdgeCandidates = [None; 4];
        let (viewport, safe) = (self.viewport, self.safe_area());
        let mut at = self.chain_start(x, y);
        while let Some(i) = at {
            if self.subtree_exiting(i) || !self.allowed(i) {
                break;
            }
            let n = &self.tree.nodes[i];
            if let Some(drags) = n.handlers.edge_drags.as_deref() {
                let t = n.target;
                let [lx, ly] = self.to_local(i, x, y);
                if (0.0..=t[2] - t[0]).contains(&lx) && (0.0..=t[3] - t[1]).contains(&ly) {
                    for edge in SwipeEdge::ALL {
                        let slot = edge.index();
                        if candidates[slot].is_none()
                            && let Some((width, _)) = drags.get(edge)
                            && in_zone(edge, *width, viewport, safe, x, y)
                        {
                            candidates[slot] = Some(n.id);
                        }
                    }
                }
            }
            at = n.parent;
        }
        candidates
    }

    fn build_edge_drag(&self, i: usize, edge: SwipeEdge, phase: Phase, origin: (f32, f32), (x, y): (f32, f32), cancelled: bool) -> Drag {
        let n = &self.tree.nodes[i];
        let r = n.target;
        let [lx, ly] = self.to_local(i, x, y);
        let [sx, sy] = self.to_local(i, origin.0, origin.1);
        Drag {
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
            edge: Some(edge),
        }
    }

    /// Queues `phase` of the edge drag on node `id` for `edge` from `origin` to `pos`.
    pub(in crate::runtime) fn queue_edge_drag(
        &mut self,
        id: NodeId,
        edge: SwipeEdge,
        phase: Phase,
        origin: (f32, f32),
        pos: (f32, f32),
        cancelled: bool,
    ) {
        match phase {
            Phase::Down => {
                let Some(&i) = self.tree.node_index.get(&id) else { return };
                if self.subtree_exiting(i) || !self.allowed(i) { return; }
                let Some((_, handler)) = self.tree.nodes[i].handlers.edge_drags.as_deref().and_then(|e| e.get(edge)) else { return };
                let handler = handler.clone();
                let path = self.tree.nodes[i].path.clone();
                let d = self.build_edge_drag(i, edge, Phase::Down, origin, pos, false);
                self.pointer.touch.active_edge = Some(ActiveEdge {
                    node: id,
                    path: path.clone(),
                    edge,
                    origin,
                    pos,
                    last_drag: d,
                    handler,
                });
                self.frame.queue.push(Action::EdgeDrag(path, d));
            }
            Phase::Move => {
                let Some(path) = self.pointer.touch.active_edge.as_ref().map(|a| a.path.clone()) else { return };
                let live = self.live_index(&path).filter(|&i| {
                    self.allowed(i) && self.tree.nodes[i].handlers.edge_drags.as_deref().is_some_and(|e| e.get(edge).is_some())
                });
                let Some(i) = live else {
                    self.cancel_edge_drag();
                    return;
                };
                let d = self.build_edge_drag(i, edge, Phase::Move, origin, pos, false);
                if let Some(active) = self.pointer.touch.active_edge.as_mut() {
                    active.pos = pos;
                    active.last_drag = d;
                }
                self.frame.queue.push(Action::EdgeDrag(path, d));
            }
            Phase::Up => {
                let Some(active) = self.pointer.touch.active_edge.take() else { return };
                let live = self.live_index(&active.path).filter(|&i| {
                    self.allowed(i) && self.tree.nodes[i].handlers.edge_drags.as_deref().is_some_and(|e| e.get(active.edge).is_some())
                });
                let d = match live {
                    Some(i) => self.build_edge_drag(i, active.edge, Phase::Up, active.origin, pos, cancelled),
                    None => {
                        let (dx, dy) = (pos.0 - active.origin.0, pos.1 - active.origin.1);
                        Drag {
                            phase: Phase::Up,
                            x: active.last_drag.start_x + dx,
                            y: active.last_drag.start_y + dy,
                            dx,
                            dy,
                            cancelled: true,
                            window: [pos.0, pos.1],
                            screen: self.screen_at(pos.0, pos.1),
                            ..active.last_drag
                        }
                    }
                };
                if d.cancelled {
                    let handler = live
                        .and_then(|i| self.tree.nodes[i].handlers.edge_drags.as_deref()?.get(active.edge).map(|(_, f)| f.clone()))
                        .unwrap_or(active.handler);
                    self.pointer.touch.push_edge_gone(active.path.clone(), handler);
                }
                self.frame.queue.push(Action::EdgeDrag(active.path, d));
            }
        }
    }

    /// Cancels an active `El::on_edge_drag` gesture, queuing a cancelled `Phase::Up` via its
    /// fallback handler and moving the active touch into `Mode::Cancelled`. Returns `false` when
    /// no edge drag is active.
    pub(in crate::runtime) fn cancel_edge_drag(&mut self) -> bool {
        let Some(active) = self.pointer.touch.active_edge.take() else { return false };
        let _ = super::touch_machine::step_touch(self.pointer.touch.state_mut(), super::touch_machine::TouchInput::CancelEdge);
        let live = self.live_index(&active.path).filter(|&i| {
            self.allowed(i) && self.tree.nodes[i].handlers.edge_drags.as_deref().is_some_and(|e| e.get(active.edge).is_some())
        });
        let handler = live
            .and_then(|i| self.tree.nodes[i].handlers.edge_drags.as_deref()?.get(active.edge).map(|(_, f)| f.clone()))
            .unwrap_or(active.handler);
        let pos = active.pos;
        let d = match live {
            Some(i) if self.tree.built || self.tree.built_ids.get(i) == Some(&self.tree.nodes[i].id) => {
                self.build_edge_drag(i, active.edge, Phase::Up, active.origin, pos, true)
            }
            _ => {
                let (dx, dy) = (pos.0 - active.origin.0, pos.1 - active.origin.1);
                Drag {
                    phase: Phase::Up,
                    x: active.last_drag.start_x + dx,
                    y: active.last_drag.start_y + dy,
                    dx,
                    dy,
                    cancelled: true,
                    window: [pos.0, pos.1],
                    screen: self.screen_at(pos.0, pos.1),
                    ..active.last_drag
                }
            }
        };
        self.pointer.touch.push_edge_gone(active.path.clone(), handler);
        self.frame.queue.push(Action::EdgeDrag(active.path, d));
        true
    }

    /// After a commit: an active `on_edge_drag` whose element left the tree, started exiting,
    /// turned inert or dropped its edge handler ends with a cancelled `Phase::Up`; one that
    /// stayed live refreshes its fallback handler from the new view.
    pub(in crate::runtime) fn sync_held_edge_drag(&mut self) {
        let Some((path, edge)) = self.pointer.touch.active_edge.as_ref().map(|a| (a.path.clone(), a.edge)) else { return };
        if let Some(i) = self.live_index(&path)
            && self.allowed(i)
            && let Some((_, fresh)) = self.tree.nodes[i].handlers.edge_drags.as_deref().and_then(|e| e.get(edge))
        {
            let fresh = fresh.clone();
            let id = self.tree.nodes[i].id;
            if let Some(active) = self.pointer.touch.active_edge.as_mut() {
                active.handler = fresh;
                active.node = id;
            }
            return;
        }
        self.cancel_edge_drag();
    }
}

#[cfg(test)]
#[path = "edge_drag_tests.rs"]
mod tests;
