//! Screen travel: a node starts at a rect in root space and follows one straight path to its own
//! layout (matched geometry, launches).
use super::*;

impl Scene {
    /// Hands one node's presented geometry, corner radius and fill to another, which then morphs
    /// to its own layout (SwiftUI `matchedGeometryEffect`, Motion `layoutId`). `from` disappears
    /// at once; `to` starts exactly where `from` was seen, with its velocity, and travels a
    /// straight root-space path. Under Reduce Motion it's a crossfade in place.
    pub fn match_geometry(&mut self, from: NodeId, to: NodeId, anim: impl Into<LayoutAnim>) {
        self.match_geometry_offset(from, to, Vec2::ZERO, anim);
    }

    /// [`match_geometry`](Self::match_geometry) for a `from` that was seen `offset` away from its
    /// scene rect, measured in `to`'s root space: a scroll offset the renderer applies outside the
    /// scene, which differs between the two places.
    pub fn match_geometry_offset(&mut self, from: NodeId, to: NodeId, offset: Vec2, anim: impl Into<LayoutAnim>) {
        let (Some(a), Some(b)) = (self.slot(from), self.slot(to)) else { return };
        let anim = anim.into();
        if self.store.reduced_motion() {
            self.remove(from);
            let enter = self.nodes[b].transition.springs.enter;
            self.track_set(b, prop::PRESENCE, 0.0);
            self.track_animate(b, prop::PRESENCE, 1.0, Anim::from(enter));
            self.nodes[b].phase = Phase::Entering;
            return;
        }
        let (mut start, vel) = self.exact_world(a);
        for (k, edge) in start.iter_mut().enumerate() {
            *edge += if k % 2 == 0 { offset.x } else { offset.y };
        }
        let radius = self.current::<Corners>(a, prop::RADIUS).unwrap_or(self.nodes[a].radius);
        let fill = self.current::<Color>(a, prop::FILL).unwrap_or(self.nodes[a].fill);
        let (to_radius, to_fill) = (self.nodes[b].radius, self.nodes[b].fill);
        // `to` is the one surface from now on: fully present, no presence effect.
        self.track_set(b, prop::PRESENCE, 1.0);
        self.nodes[b].phase = Phase::Present;
        self.nodes[b].fx_a = FX_ID;
        self.start_path(b, start, vel, anim);
        self.track_set(b, prop::RADIUS, radius);
        self.track_animate(b, prop::RADIUS, to_radius, anim.x);
        self.track_set(b, prop::FILL, fill);
        self.track_animate(b, prop::FILL, to_fill, anim.y);
        self.intent(to, Intent::POSITION | Intent::SIZE | Intent::SHAPE, anim.y.curve.logical_duration());
        self.remove_now(a);
    }

    /// Starts `id`, fully present, at `start` (root space) on one straight root-space path to
    /// its own layout: [`match_geometry`](Self::match_geometry) from a place no node was.
    /// Under Reduce Motion it keeps its own entrance.
    pub fn launch(&mut self, id: NodeId, start: Rect, anim: impl Into<LayoutAnim>) {
        let Some(i) = self.slot(id) else { return };
        if self.store.reduced_motion() { return; }
        let anim = anim.into();
        self.track_set(i, prop::PRESENCE, 1.0);
        self.nodes[i].phase = Phase::Present;
        self.nodes[i].fx_a = FX_ID;
        self.start_path(i, start.edges(), [0.0; 4], anim);
        self.intent(id, Intent::POSITION, anim.y.curve.logical_duration());
    }

    /// [`set_frame`](Self::set_frame) with screen travel for a node the renderer draws `offset`
    /// away from its scene rect, moving `velocity` faster (root space), by offsets it applies
    /// outside the scene: the new path starts where the node is drawn, at the speed it's drawn
    /// moving, so a re-aim keeps its speed. The renderer then stops adding them.
    pub fn retarget_drawn(&mut self, id: NodeId, frame: Rect, anim: impl Into<LayoutAnim>, offset: Vec2, velocity: Vec2) {
        self.set_frame_drawn(id, frame, anim.into(), [offset, velocity]);
    }

    pub(super) fn start_path(&mut self, i: usize, start: [f32; 4], vel: [f32; 4], anim: LayoutAnim) {
        let end_local = self.nodes[i].frame;
        let po = self.parent_target_origin(i);
        let end = end_local.translated(po).edges();
        // Project the current edge velocity onto each axis' path to keep it moving the same way.
        let px = axis_progress_velocity(start[0], start[2], end[0], end[2], vel[0], vel[2]);
        let py = axis_progress_velocity(start[1], start[3], end[1], end[3], vel[1], vel[3]);
        self.nodes[i].path = Some(Path { start: Rect::from_edges(start[0], start[1], start[2], start[3]), end_local, origin: po });
        if !self.travelling.contains(&(i as u32)) {
            self.travelling.push(i as u32);
        }
        self.track_set(i, prop::TRAVEL, Vec2::ZERO);
        let t = self.nodes[i].tracks[prop::TRAVEL.id.0 as usize].unwrap().index_usize();
        self.store.animate_raw(t, &[1.0, 0.0], anim.x, Some(&[px, 0.0]));
        self.store.animate_raw(t, &[1.0, 1.0], anim.y, Some(&[px, py]));
        if self.nodes[i].tracks[prop::FRAME.id.0 as usize].is_some() {
            self.track_set(i, prop::FRAME, end_local);
        }
        self.nodes[i].dirty = true;
    }

    pub(super) fn end_path(&mut self, i: usize) {
        self.nodes[i].path = None;
        self.travelling.retain(|&x| x as usize != i);
        let id = self.nodes[i].id;
        self.store.remove(id, prop::TRAVEL);
        self.nodes[i].tracks[prop::TRAVEL.id.0 as usize] = None;
    }
}

/// The progress velocity that best continues edge velocities `(v0, v1)` along the path from
/// `(s0, s1)` to `(e0, e1)`.
fn axis_progress_velocity(s0: f32, s1: f32, e0: f32, e1: f32, v0: f32, v1: f32) -> f32 {
    let (d0, d1) = (e0 - s0, e1 - s1);
    let len2 = d0 * d0 + d1 * d1;
    if len2 < 1e-6 { 0.0 } else { (v0 * d0 + v1 * d1) / len2 }
}
