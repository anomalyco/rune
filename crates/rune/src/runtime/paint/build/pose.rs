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

/// Destination transform a lifted matched flight blends into as `prop::TRAVEL` runs from 0 to 1.
#[derive(Clone, Copy)]
pub(super) struct FlightDest {
    held: (f32, [f32; 2], [f32; 2]),
    dxy: [f32; 2],
    anc_off: [f32; 2],
    travel: [f32; 2],
}

/// Sum of `prop::OFFSET` (`live_offset` and `El::offset`) across a node's ancestors in `scene`.
#[inline]
pub(in crate::runtime) fn ancestor_scene_offset(nodes: &[Node], scene: &Scene, parent: Option<usize>) -> [f32; 2] {
    let mut off = [0.0f32; 2];
    let mut cur = parent;
    while let Some(pi) = cur {
        if let Some(o) = scene.get(nodes[pi].id, prop::OFFSET) {
            off[0] += o.x;
            off[1] += o.y;
        }
        cur = nodes[pi].parent;
    }
    off
}

/// Edge delta `rest - end` between where a matched flight lands on screen (under `held` zoom,
/// ancestor scene offsets `anc_off` and scroll/growth `dxy`) and where its scene path ends.
#[inline]
pub(in crate::runtime) fn flight_rest_delta(
    scene: &Scene,
    n: &Node,
    p_rect: rune_motion::Rect,
    own_sc: f32,
    row_h: f32,
    held: (f32, [f32; 2], [f32; 2]),
    dxy: [f32; 2],
    anc_off: [f32; 2],
) -> [f32; 4] {
    let (k_d, a_d, a_to) = held;
    if (k_d - 1.0).abs() <= 1e-5
        && a_d == a_to
        && dxy == [0.0; 2]
        && anc_off == [0.0; 2]
        && (own_sc - 1.0).abs() <= 1e-5
    {
        return [0.0; 4];
    }
    let own_off = scene.get(n.id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
    let (end_rect, anc_off) = match scene.path_end(n.id) {
        Some(r) => (r.translated(own_off), anc_off),
        // Path ended while exiting: `p_rect` already rides `parent_origin` (includes `anc_off`).
        None => (p_rect, [0.0; 2]),
    };
    let mut end = end_rect.edges();
    if row_h >= 0.0 {
        end[3] = end[1] + row_h;
    }
    let rest_raw = [
        end[0] + anc_off[0] + dxy[0],
        end[1] + anc_off[1] + dxy[1],
        end[2] + anc_off[0] + dxy[0],
        end[3] + anc_off[1] + dxy[1],
    ];
    let rest = if (own_sc - 1.0).abs() > 1e-5 {
        let pivot = match n.vis.origin {
            Some([ox, oy]) => [end_rect.x + anc_off[0] + ox * end_rect.w, end_rect.y + anc_off[1] + oy * end_rect.h],
            None => [end_rect.x + anc_off[0] + 0.5 * end_rect.w, end_rect.y + anc_off[1] + 0.5 * end_rect.h],
        };
        let centre = [pivot[0] + dxy[0], pivot[1] + dxy[1]];
        let centre_to = [a_to[0] + (centre[0] - a_d[0]) * k_d, a_to[1] + (centre[1] - a_d[1]) * k_d];
        let full = k_d * own_sc;
        [
            centre_to[0] + (rest_raw[0] - centre[0]) * full,
            centre_to[1] + (rest_raw[1] - centre[1]) * full,
            centre_to[0] + (rest_raw[2] - centre[0]) * full,
            centre_to[1] + (rest_raw[3] - centre[1]) * full,
        ]
    } else if (k_d - 1.0).abs() > 1e-5 || a_d != a_to {
        [
            a_to[0] + (rest_raw[0] - a_d[0]) * k_d,
            a_to[1] + (rest_raw[1] - a_d[1]) * k_d,
            a_to[0] + (rest_raw[2] - a_d[0]) * k_d,
            a_to[1] + (rest_raw[3] - a_d[1]) * k_d,
        ]
    } else {
        rest_raw
    };
    [rest[0] - end[0], rest[1] - end[1], rest[2] - end[2], rest[3] - end[3]]
}

/// Screen-space edges of a lifted matched flight, blending straight from its take-off rect at
/// `travel = 0` to its resting rect under `dest` at `travel = 1`.
#[inline(always)]
pub(super) fn flight_frame(
    scene: &Scene,
    n: &Node,
    p: &rune_motion::Presented,
    own_sc: f32,
    row_h: f32,
    dest: FlightDest,
) -> [f32; 4] {
    let mut cur = p.rect.edges();
    if row_h >= 0.0 {
        cur[3] = cur[1] + row_h;
    }
    let delta = flight_rest_delta(scene, n, p.rect, own_sc, row_h, dest.held, dest.dxy, dest.anc_off);
    if delta == [0.0; 4] {
        return cur;
    }
    [
        cur[0] + delta[0] * dest.travel[0],
        cur[1] + delta[1] * dest.travel[1],
        cur[2] + delta[2] * dest.travel[0],
        cur[3] + delta[3] * dest.travel[1],
    ]
}

#[inline(always)]
pub(super) fn flight_cover(
    launches: &flights::Launches,
    node_id: NodeId,
    n: &Node,
    rect_pt: [f32; 4],
    sc: f32,
) -> Option<(f32, [f32; 2])> {
    if launches.launched(node_id) {
        return None;
    }
    let (pw, ph) = (rect_pt[2] - rect_pt[0], rect_pt[3] - rect_pt[1]);
    if n.vis.cold.get().is_some_and(|c| c.flight_crop) {
        // `El::matched_crop`: at its own size, from the leading edge.
        let k = sc.max(1e-3);
        Some((k, [0.0, (ph - n.local.h * k) * 0.5]))
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
        nodes: &[Node],
        paint: &paint_state::PaintState,
        scene: &Scene,
        commit_gen: u64,
        node_id: NodeId,
        node_parent: Option<usize>,
        pinh: &mut Inh,
    ) -> Option<FlightDest> {
        // Keep the destination's clips (consumed ones included) and offsets. Drop its
        // ancestors' opacity, scale and blur: an entering parent's fade or scale would
        // otherwise hide the flight or jump it off the rect it was seen at. Inside another
        // flight, also drop that flight's cover offset and clip.
        let [clip, fade, fade_origin] = if self.launches().flying(node_id) {
            // A launch leaves where its clips don't reach.
            [crate::render::NO_CLIP, [0.0; 4], pinh.fade_origin]
        } else {
            pinh.outer.unwrap_or_else(|| {
                let clip = if pinh.turn != 0 { screen_clip_of(node_parent, &paint.inh_scratch, &paint.turn_scratch) } else { pinh.clip };
                [clip, pinh.fade, pinh.fade_origin]
            })
        };
        let dxy = [pinh.dx - pinh.cover[0], pinh.dy - pinh.cover[1]];
        if self.launches().launched(node_id) {
            // Launches map their start through the still zoom at take-off and keep it throughout.
            let (scale, anchor, anchor_to) = pinh.still;
            *pinh = Inh { opacity: 1.0, paint_opacity: 1.0, hit_opacity: 1.0, clip, fade, fade_origin, paint_clip: clip, paint_fade: fade,
                paint_fade_origin: fade_origin, rounded: None, scale, anchor, anchor_to, blur: 0.0,
                dx: dxy[0], dy: dxy[1], cover: [0.0; 2], outer: None,
                scale_motion: ScaleMotion::Still, in_cache: false, skip_paint: false, turn: 0, snap: pixel_snap::Inherit { layer: 0, parent: None, ..pinh.snap }, ..*pinh };
            let fix = self.launches_mut().offset_fix(scene, node_id, [pinh.dx, pinh.dy], commit_gen);
            (pinh.dx, pinh.dy) = (pinh.dx + fix[0], pinh.dy + fix[1]);
            return None;
        }
        // Matched flights leave from the source's drawn screen rect (scale 1, no destination
        // offsets) and blend straight into the destination's held zoom and live/scroll offsets.
        let held = pinh.held;
        let travel = scene.get(node_id, prop::TRAVEL).map_or([1.0, 1.0], |v| [v.x, v.y]);
        let k_t = (0.5 * (travel[0] + travel[1])).clamp(0.0, 1.0);
        let scale = 1.0 + (held.0 - 1.0) * k_t;
        let anc_off = ancestor_scene_offset(nodes, scene, node_parent);
        *pinh = Inh { opacity: 1.0, paint_opacity: 1.0, hit_opacity: 1.0, clip, fade, fade_origin, paint_clip: clip, paint_fade: fade,
            paint_fade_origin: fade_origin, rounded: None, scale, anchor: [0.0; 2], anchor_to: [0.0; 2], blur: 0.0,
            dx: dxy[0], dy: dxy[1], cover: [0.0; 2], outer: None,
            scale_motion: ScaleMotion::Still, in_cache: false, skip_paint: false, turn: 0, snap: pixel_snap::Inherit { layer: 0, parent: None, ..pinh.snap }, ..*pinh };
        Some(FlightDest { held, dxy, anc_off, travel })
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
