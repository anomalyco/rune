//! Per-node pose helpers extracted from `build`: sticky and flight inheritance, static-scale
//! settling, sticky frame bounds, matched-flight cover, and loop wake scheduling.
use super::*;

#[inline(always)]
pub(super) fn step_static_scale(
    static_scales: &mut HashMap<NodeId, ScaleSettle>,
    scale_frame: &mut Option<AnotherFrame>,
    node_id: NodeId,
    tracked_scale: f32,
    t: f64,
    settled: bool,
    has_live_scale: bool,
) -> bool {
    let seen = static_scales.entry(node_id).or_insert(ScaleSettle::Still(tracked_scale));
    let moving = matches!(seen.step(tracked_scale, t, settled, 1e-5, true), crate::machine::Effects::One(_));
    if !has_live_scale && moving {
        *scale_frame = Some(AnotherFrame);
    }
    moving
}

#[inline(always)]
pub(super) fn update_sticky_frame(sticky_frame: &mut [[f32; 8]], i: usize, n: &Node, raw_edges: [f32; 4]) {
    let up = n.parent.map_or(sticky::UNBOUNDED, |pr| sticky_frame[pr]);
    // Content scrolls without end on a viewport's scrolling axes; non-scrolling axes
    // bound sticky children to this node's content box.
    let sc_y = n.scroll.is_some() && n.scroll_axes.y();
    let sc_x = n.scroll.is_some() && n.scroll_axes.x();
    let v = if sc_y {
        [raw_edges[1], raw_edges[3], f32::NEG_INFINITY, f32::INFINITY]
    } else {
        [up[0], up[1], raw_edges[1] + n.inset[1], raw_edges[3] - n.inset[3]]
    };
    let x = if sc_x {
        [raw_edges[0], raw_edges[2], f32::NEG_INFINITY, f32::INFINITY]
    } else {
        [up[4], up[5], raw_edges[0] + n.inset[0], raw_edges[2] - n.inset[2]]
    };
    sticky_frame[i] = [v[0], v[1], v[2], v[3], x[0], x[1], x[2], x[3]];
}

#[inline(always)]
pub(super) fn flight_cover(
    launches: &flights::Launches,
    node_id: NodeId,
    n: &Node,
    rect_pt: [f32; 4],
) -> Option<(f32, [f32; 2])> {
    if launches.launched(node_id) {
        return None;
    }
    let (pw, ph) = (rect_pt[2] - rect_pt[0], rect_pt[3] - rect_pt[1]);
    if n.vis.cold.get().is_some_and(|c| c.flight_crop) {
        // `El::matched_crop`: at its own size, from the leading edge.
        Some((1.0, [0.0, (ph - n.local.h) * 0.5]))
    } else {
        let k = (pw / n.local.w).max(ph / n.local.h).max(1e-3);
        Some((k, [(pw - n.local.w * k) * 0.5, (ph - n.local.h * k) * 0.5]))
    }
}

impl RuntimeCore {
    #[inline(always)]
    pub(super) fn apply_sticky_inh(
        &mut self,
        i: usize,
        node_parent: Option<usize>,
        p: &rune_motion::Presented,
        pinh: &mut Inh,
    ) -> f32 {
        // `El::stuck_fade`: how much of a stuck header's title shows.
        let mut kids_fade = 1.0f32;
        if let Some((edge, inset, tail)) = self.sticky_of(i) {
            let n = &self.tree.nodes[i];
            // Worked out from this frame's offsets, which the viewport above already applied.
            // Exclude this node's own `prop::OFFSET` (`live_offset`) so it translates from the
            // stuck place rather than being clamped away by the sticky edge.
            let (frame, viewport) = self.sticky_frame_of(i, edge);
            let own_off = self.scene.get(n.id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
            let (top, left) = (p.rect.y - own_off.y + pinh.dy, p.rect.x - own_off.x + pinh.dx);
            let h = if self.paint.row_h[i] >= 0.0 { self.paint.row_h[i] } else { p.rect.h };
            let shift = sticky::shift(edge, if tail { inset - h } else { inset }, [top, top + h, left, left + p.rect.w], frame);
            self.scroll.stuck[i] = shift;
            if edge == crate::el::StickyEdge::Top { kids_fade = self.stuck_fade(i, top + shift, pinh.dy); }
            // A header whose section's rows fade under it keeps the clip they lost.
            if let Some(&(_, top)) = self.scroll.sticky_bands.iter().find(|b| Some(b.0) == node_parent) {
                pinh.paint_clip[1] = top;
            }
            if edge == crate::el::StickyEdge::Left { pinh.dx += shift } else { pinh.dy += shift }
            pinh.snap.scroll((edge != crate::el::StickyEdge::Left) as usize, shift);
            // A sticky element stays crisp against the viewport edge it sticks to while rows
            // scrolling under its fill fade out.
            let slot = match edge {
                crate::el::StickyEdge::Left => 0,
                crate::el::StickyEdge::Top => 1,
                crate::el::StickyEdge::Bottom => 3,
            };
            if let Some(viewport) = viewport.filter(|&v| self.tree.nodes[v].scroll_edges.fade > 0.0) {
                let (width, origin) = self.sticky_fade(n.parent.unwrap(), viewport, slot, false);
                pinh.fade[slot] = width;
                pinh.fade_origin[slot] = origin;
                let (width, origin) = self.sticky_fade(n.parent.unwrap(), viewport, slot, true);
                pinh.paint_fade[slot] = width;
                pinh.paint_fade_origin[slot] = origin;
            }
        }
        kids_fade
    }
}

impl crate::runtime::tracks::MotionState {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lift_flight_inh(
        &mut self,
        paint: &paint_state::PaintState,
        scene: &Scene,
        commit_gen: u64,
        node_id: NodeId,
        node_parent: Option<usize>,
        pinh: &mut Inh,
    ) {
        // Keep the destination's clips (consumed ones included) and offsets. Drop its
        // ancestors' opacity, scale and blur: an entering parent's fade or scale would
        // otherwise hide the flight or jump it off the rect it was seen at. The scales
        // holding still above that parent (an app-wide zoom) stay, so it flies at the size
        // it lands at. Inside another flight, also drop that flight's cover offset and clip.
        let [clip, fade, fade_origin] = if self.launches().flying(node_id) {
            // A launch leaves where its clips don't reach.
            [crate::render::NO_CLIP, [0.0; 4], pinh.fade_origin]
        } else {
            pinh.outer.unwrap_or_else(|| {
                let clip = if pinh.turn != 0 { screen_clip_of(node_parent, &paint.inh_scratch, &paint.turn_scratch) } else { pinh.clip };
                [clip, pinh.fade, pinh.fade_origin]
            })
        };
        let (scale, anchor, anchor_to) = pinh.still;
        *pinh = Inh { opacity: 1.0, paint_opacity: 1.0, hit_opacity: 1.0, clip, fade, fade_origin, paint_clip: clip, paint_fade: fade,
            paint_fade_origin: fade_origin, rounded: None, scale, anchor, anchor_to, blur: 0.0,
            dx: pinh.dx - pinh.cover[0], dy: pinh.dy - pinh.cover[1], cover: [0.0; 2], outer: None,
            scale_motion: ScaleMotion::Still, in_cache: false, skip_paint: false, turn: 0, snap: pixel_snap::Inherit { layer: 0, parent: None, ..pinh.snap }, ..*pinh };
        let fix = self.launches_mut().offset_fix(scene, node_id, [pinh.dx, pinh.dy], commit_gen);
        (pinh.dx, pinh.dy) = (pinh.dx + fix[0], pinh.dy + fix[1]);
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn schedule_loop_wake(
        &mut self,
        paint: &mut paint_state::PaintState,
        scene: &Scene,
        clock: f64,
        time_scale: f32,
        node_id: NodeId,
        lnow: f64,
        now: f64,
        ambient: &mut Option<f32>,
    ) {
        if let Some(track) = self.loop_track(node_id).copied() {
            let node_k = scene.store().node_time_scale(node_id) as f64;
            if let Some(dt) = track.frame_interval().filter(|_| track.active(lnow)) {
                *ambient = Some(ambient.map_or(dt as f32, |a| a.min(dt as f32)));
                let wake = clock + dt;
                self.arm_loop_wake(wake, now + dt * time_scale as f64);
            } else if time_scale > 0.0 && node_k > 0.0 {
                self.mark_loops_active(track.active(lnow));
            }
            if time_scale > 0.0 && node_k > 0.0 && let Some(start) = track.pending_start(lnow) {
                let window_start = now + (start - lnow) / node_k;
                paint.note_cycle_other_deadline();
                let wake = clock + (window_start - now) / time_scale as f64;
                self.arm_loop_wake(wake, window_start);
            }
        }
    }
}
