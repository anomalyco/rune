//! Per-node paint pose: sticky and flight inheritance, opacity, scale, pixel snapping, rotation,
//! masks, visibility and loop wake scheduling.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct NodePose {
    pub(super) mark: u32,
    pub(super) base_opacity: f32,
    pub(super) hover_owner: bool,
    pub(super) group: f32,
    pub(super) pressed: f32,
    pub(super) press_to: Option<Color>,
    pub(super) loop_value: Option<f32>,
    pub(super) effective_opacity: f32,
    pub(super) hit_opacity: f32,
    pub(super) next: Option<usize>,
    pub(super) dissolve: bool,
    pub(super) paint_own: f32,
    pub(super) isolate: bool,
    pub(super) clip: [f32; 4],
    pub(super) fade: [f32; 4],
    pub(super) fade_origin: [f32; 4],
    pub(super) rounded: Option<([f32; 4], [f32; 4])>,
    pub(super) press_k: f32,
    pub(super) scale_motion: ScaleMotion,
    pub(super) sc: f32,
    pub(super) anchor: [f32; 2],
    pub(super) anchor_to: [f32; 2],
    pub(super) press_anchor: [f32; 2],
    pub(super) blur: f32,
    pub(super) filter_margin: f32,
    pub(super) raw_edges: [f32; 4],
    pub(super) rect_pt: [f32; 4],
    pub(super) cover: Option<(f32, [f32; 2])>,
    pub(super) r_exact: [f32; 4],
    pub(super) unscaled: bool,
    pub(super) opens_snap_layer: bool,
    pub(super) snap_layer: u32,
    pub(super) steps: bool,
    pub(super) gliding: bool,
    pub(super) r: [f32; 4],
    pub(super) snap_rest: Option<pixel_snap::Parent>,
    pub(super) w: f32,
    pub(super) h: f32,
    pub(super) own_turn: Option<f32>,
    pub(super) node_turn: Option<Turn>,
    pub(super) turn_index: u32,
    pub(super) radii: [f32; 4],
    pub(super) m: f32,
    pub(super) visible: bool,
    pub(super) loop_vis: bool,
}

impl RuntimeCore {
    #[inline]
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
            let n = &self.nodes[i];
            // Worked out from this frame's offsets, which the viewport above already applied.
            // Exclude this node's own `prop::OFFSET` (`live_offset`) so it translates from the
            // stuck place rather than being clamped away by the sticky edge.
            let (frame, viewport) = self.sticky_frame_of(i, edge);
            let own_off = self.scene.get(n.id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
            let (top, left) = (p.rect.y - own_off.y + pinh.dy, p.rect.x - own_off.x + pinh.dx);
            let h = if self.row_h[i] >= 0.0 { self.row_h[i] } else { p.rect.h };
            let shift = sticky::shift(edge, if tail { inset - h } else { inset }, [top, top + h, left, left + p.rect.w], frame);
            self.stuck[i] = shift;
            if edge == crate::el::StickyEdge::Top { kids_fade = self.stuck_fade(i, top + shift, pinh.dy); }
            // A header whose section's rows fade under it keeps the clip they lost.
            if let Some(&(_, top)) = self.sticky_bands.iter().find(|b| Some(b.0) == node_parent) {
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
            if let Some(viewport) = viewport.filter(|&v| self.nodes[v].scroll_edges.fade > 0.0) {
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

    #[inline]
    pub(super) fn lift_flight_inh(&mut self, node_id: NodeId, node_parent: Option<usize>, pinh: &mut Inh) {
        // Keep the destination's clips (consumed ones included) and offsets. Drop its
        // ancestors' opacity, scale and blur: an entering parent's fade or scale would
        // otherwise hide the flight or jump it off the rect it was seen at. The scales
        // holding still above that parent (an app-wide zoom) stay, so it flies at the size
        // it lands at. Inside another flight, also drop that flight's cover offset and clip.
        let [clip, fade, fade_origin] = if self.launches.flying(node_id) {
            // A launch leaves where its clips don't reach.
            [crate::render::NO_CLIP, [0.0; 4], pinh.fade_origin]
        } else {
            pinh.outer.unwrap_or_else(|| {
                let clip = if pinh.turn != 0 { self.screen_clip_of_parent(node_parent) } else { pinh.clip };
                [clip, pinh.fade, pinh.fade_origin]
            })
        };
        let (scale, anchor, anchor_to) = pinh.still;
        *pinh = Inh { opacity: 1.0, paint_opacity: 1.0, hit_opacity: 1.0, clip, fade, fade_origin, paint_clip: clip, paint_fade: fade,
            paint_fade_origin: fade_origin, rounded: None, scale, anchor, anchor_to, blur: 0.0,
            dx: pinh.dx - pinh.cover[0], dy: pinh.dy - pinh.cover[1], cover: [0.0; 2], outer: None,
            scale_motion: ScaleMotion::Still, in_cache: false, skip_paint: false, turn: 0, snap: pixel_snap::Inherit { layer: 0, parent: None, ..pinh.snap }, ..*pinh };
        let fix = self.launches.offset_fix(&self.scene, node_id, [pinh.dx, pinh.dy], self.commit_gen);
        (pinh.dx, pinh.dy) = (pinh.dx + fix[0], pinh.dy + fix[1]);
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn resolve_pose(
        &mut self,
        i: usize,
        paint_index: usize,
        tree_order: bool,
        lift: bool,
        p: &rune_motion::Presented,
        vp: [f32; 4],
        pinh: &mut Inh,
        layers: &mut Vec<(usize, usize, usize)>,
        ambient: &mut Option<f32>,
    ) -> NodePose {
        let s = self.env.scale;
        let snap_px = |v: f32| grid(v, s) * s;
        let now = self.t;
        let n_nodes = self.nodes.len();
        let n = &self.nodes[i];
        let node_id = n.id;
        let mark = if n.vis.mark != 0 { n.vis.mark } else { pinh.mark };
        let mut own = p.opacity;
        let mut hit_own = own * self.group_factor(n, 1.0);
        own *= self.group_factor(n, pinh.group);
        let base_opacity = (own * pinh.opacity).clamp(0.0, 1.0);
        let hover_owner = n.vis.hover().a > 0.0 || n.vis_flags.hover_group();
        let group = if hover_owner {
            self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0)
        } else {
            pinh.group
        };
        let (pressed, press_to) = match n.vis.cold.get().and_then(|c| c.press_color) {
            Some(c) => (self.scene.get(n.id, P_PRESS).unwrap_or(0.0).clamp(0.0, 1.0), Some(c)),
            None => (pinh.pressed, pinh.press_to),
        };
        // Loops and reveals run on the node's clock (`El::time_scale`).
        let track = self.loops.get(&n.id);
        let lnow = if track.is_some() { self.scene.store().node_time(n.id) } else { self.t };
        let loop_value = track.map(|l| l.sample_at(lnow, self.clock, self.reduced_motion));
        if fades(n.looping.as_deref().copied()) && let Some(v) = loop_value {
            own *= v;
            hit_own *= v;
        }
        let effective_opacity = (own * pinh.opacity).clamp(0.0, 1.0);
        let hit_opacity = (hit_own * pinh.hit_opacity).clamp(0.0, 1.0);
        let next = if tree_order { self.layer_paint_order.get(paint_index + 1).copied() } else { (i + 1 < n_nodes).then_some(i + 1) };
        let dissolve = self.has_dissolve && self.dissolve_scratch[i];
        let additive_parent = self.has_dissolve && n.parent.is_some_and(|p| self.dissolve_scratch[p]);
        // `El::layer_opacity` fades paint only; hit testing and what elements report keep
        // `own`.
        let paint_own = own * n.vis.layer_opacity;
        // A fading node with children, or a shape whose pieces overlap, takes its opacity
        // once as a layer rather than each primitive taking it over the ones beneath. So
        // does a layer opacity over two paints of the node's own (a label on its fill).
        let isolate = effective_opacity > 0.001 && (p.blur > 0.0 || dissolve
            || additive_parent || (paint_own > 0.001 && paint_own < 1.0
                && (next.is_some_and(|j| self.nodes[j].parent == Some(i)) || n.vis.shape.as_ref().is_some_and(|s| s.overlaps())
                    || (n.vis.layer_opacity < 1.0 && own_paints(n) >= 2))));
        let (mut clip, mut fade) = (pinh.paint_clip, pinh.paint_fade);
        let mut fade_origin = pinh.paint_fade_origin;
        let mut rounded = pinh.rounded;
        let has_press_scale = n.handlers.get().is_some_and(|h| h.press_scale.is_some());
        let press_k = if has_press_scale { self.scene.get(n.id, P_PRESS_SCALE).unwrap_or(1.0) } else { 1.0 };
        let sequence = track
            .filter(|_| matches!(n.looping.as_ref().map(|l| l.kind), Some(rune_motion::LoopKind::Sequence(_))));
        let static_k = if n.scale_track { self.scene.get(n.id, P_SCALE).unwrap_or(n.static_scale) } else { n.static_scale };
        let own_sc = p.transform.scale.x * press_k * static_k
            * sequence.and_then(|l| l.pose(lnow, self.reduced_motion)).map_or(1.0, |p| p.scale);
        // An `El::scale` or `El::live_scale` an app drives from a slider, wheel or pinch
        // counts as moving until it holds still, then rasterizes crisp at its resting scale.
        let has_live_scale = n.live.iter().any(|b| b.property == crate::live::Property::Scale);
        let tracked_scale = if has_live_scale { static_k * p.transform.scale.x } else { static_k };
        let scale_settling = if tracked_scale != 1.0 || has_live_scale || self.static_scales.contains_key(&n.id) {
            let t = self.t;
            let seen = self.static_scales.entry(n.id).or_insert(ScaleSettle::Still(tracked_scale));
            let moving = matches!(seen.step(tracked_scale, t, self.settled, 1e-5, true), crate::machine::Effects::One(_));
            if !has_live_scale && moving {
                self.scale_frame = Some(AnotherFrame);
            }
            moving
        } else { false };
        let store = self.scene.store();
        let scale_animating = store.is_animating(n.id, prop::SCALE) || store.is_animating(n.id, prop::PRESENCE)
            || (has_press_scale && store.is_animating(n.id, P_PRESS_SCALE))
            || (n.scale_track && store.is_animating(n.id, P_SCALE))
            || sequence.is_some_and(|l| !self.reduced_motion && l.active(lnow));
        let scale_motion = pinh.scale_motion.max(
            if !has_live_scale && (scale_animating || scale_settling) {
                ScaleMotion::Moving
            } else if has_live_scale && (scale_settling || scale_animating) {
                ScaleMotion::Live
            } else {
                ScaleMotion::Still
            });
        // Scale about this node's centre where its ancestors' scales put it, so a scaled child
        // of a scaled parent stays in place.
        let sc = pinh.scale * own_sc;
        let pivot = match n.vis.origin {
            Some([ox, oy]) => [p.rect.x + ox * p.rect.w, p.rect.y + oy * p.rect.h],
            None => [p.anchor.x, p.anchor.y],
        };
        let centre = [pivot[0] + pinh.dx, pivot[1] + pinh.dy];
        let centre_to = [pinh.anchor_to[0] + (centre[0] - pinh.anchor[0]) * pinh.scale,
            pinh.anchor_to[1] + (centre[1] - pinh.anchor[1]) * pinh.scale];
        let (anchor, anchor_to) = if (own_sc - 1.0).abs() > 1e-5 { (centre, centre_to) } else { (pinh.anchor, pinh.anchor_to) };
        // A pressed element still hits at its resting size, so a press near its edge stays inside.
        let press_anchor = centre_to;
        let blur = pinh.blur + p.blur * sc;
        // An effect slot's bleed can reach into view from a rect outside it.
        let filter_margin = blur * s * 3.0 + n.vis.effect.map_or(0.0, |e| e.bleed * sc * s);

        let mut raw_edges = p.rect.edges();
        raw_edges[0] += pinh.dx;
        raw_edges[2] += pinh.dx;
        raw_edges[1] += pinh.dy;
        raw_edges[3] += pinh.dy;
        if self.row_h[i] >= 0.0 {
            raw_edges[3] = raw_edges[1] + self.row_h[i];
        }
        if self.has_sticky {
            let up = n.parent.map_or(sticky::UNBOUNDED, |pr| self.sticky_frame[pr]);
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
            self.sticky_frame[i] = [v[0], v[1], v[2], v[3], x[0], x[1], x[2], x[3]];
        }
        // A product of 1 (0.5 inside 2.0) can still move the box.
        let mut rect_pt = if (sc - 1.0).abs() > 1e-5 || anchor != anchor_to {
            [
                anchor_to[0] + (raw_edges[0] - anchor[0]) * sc,
                anchor_to[1] + (raw_edges[1] - anchor[1]) * sc,
                anchor_to[0] + (raw_edges[2] - anchor[0]) * sc,
                anchor_to[1] + (raw_edges[3] - anchor[1]) * sc,
            ]
        } else {
            raw_edges
        };
        // Land a sub-quantum size residual before snapping both edges. Otherwise a
        // translating/scaling box can alternate between neighbouring widths at rest.
        for (axis, target) in [n.local.w, n.local.h].into_iter().enumerate() {
            let target = grid(target, s);
            if ((rect_pt[axis + 2] - rect_pt[axis]) - target).abs() * s < 1.0 / 32.0 {
                rect_pt[axis + 2] = rect_pt[axis] + target;
            }
        }
        // A matched flight scales its laid-out content uniformly to cover the moving rect,
        // centred: `k` and the centring offset in points. A launch flies at its own size.
        let cover = if lift && n.local.w > 0.0 && n.local.h > 0.0 && !self.launches.launched(node_id) {
            let (pw, ph) = (rect_pt[2] - rect_pt[0], rect_pt[3] - rect_pt[1]);
            if n.vis.cold.get().is_some_and(|c| c.flight_crop) {
                // `El::matched_crop`: at its own size, from the leading edge.
                Some((1.0, [0.0, (ph - n.local.h) * 0.5]))
            } else {
                let k = (pw / n.local.w).max(ph / n.local.h).max(1e-3);
                Some((k, [(pw - n.local.w * k) * 0.5, (ph - n.local.h * k) * 0.5]))
            }
        } else { None };
        let r_exact = rect_pt.map(snap_px);
        // A node the visibility test below culls paints nothing, and any clip it hands down is
        // off screen too, so it keeps its exact edges.
        let rigid = pinh.snap.rigid.map(|v| v * s);
        // What a stepped window holds glides with its slide; the window itself snaps as a still box,
        // and its clip glides along the slide only (`slide_anchor`).
        let can_snap = !pinh.snap.off && !n.vis_flags.no_pixel_snap() && p.transform.rotate == 0.0 && !(self.slide_glides(i) && !self.slide_pair(i));
        let snaps = can_snap && (pinh.in_cache || {
            let m = filter_margin + 2.0;
            r_exact[2].max(r_exact[0]) >= clip[0].max(vp[0]) - m && r_exact[0] <= clip[2].min(vp[2]) + m
                && r_exact[3].max(r_exact[1]) >= clip[1].max(vp[1]) - m && r_exact[1] <= clip[3].min(vp[3]) + m
        });
        let unscaled = (sc - 1.0).abs() <= 1e-5 && anchor == anchor_to;
        let animating = p.flags & (rune_motion::flags::ANIMATED | rune_motion::flags::ANCESTOR_ANIMATED) != 0;
        // A node with its own scale, or an anchored overlay inheriting its anchor's zoom after
        // `anchored_inh` cleared `snap.layer`, heads a scaled snap layer so its subtree snaps
        // onto whole pixels once still.
        let opens_snap_layer = (own_sc - 1.0).abs() > 1e-5 || (n.above.is_some() && !unscaled);
        let snap_layer = if can_snap && opens_snap_layer {
            self.pixel_snap.layer(n.id, (r_exact, raw_edges.map(snap_px)), rigid, pinh.in_cache || n.vis_flags.cache())
        } else {
            pinh.snap.layer
        };
        let steps = pinh.snap.steps || n.vis.cold.get().is_some_and(|c| c.motion_snaps);
        let rest = if snaps && animating && !steps { self.scene.rest(n.id) } else { None };
        let gliding = snaps && (rest.is_some() || !unscaled);
        let (r, snap_rest) = if !snaps {
            (r_exact, None)
        } else if rest.is_none() && unscaled {
            self.pixel_snap.at_rest(n.id, r_exact, rigid, pinh.snap.parent)
        } else {
            let motion = pixel_snap::motion(p, rest, s, n.above.is_some(), self.row_h[i] >= 0.0);
            let raw = if unscaled { r_exact } else { raw_edges.map(snap_px) };
            self.pixel_snap.snapped(n.id, motion, (r_exact, raw), rigid, (snap_layer, sc), pinh.snap.parent)
        };
        let (w, h) = ((r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0));
        // `El::rotate`: this subtree draws into a layer turned about its origin.
        // The layer takes the clips from above; the node's own clips apply inside, upright.
        let own_turn = p.transform.rotate;
        let own_turn = (own_turn != 0.0 && own_turn.is_finite() && own_turn.abs() > 1e-6 && !lift).then_some(own_turn);
        let turn_pivot = [centre_to[0] * s, centre_to[1] * s];
        let (node_turn, turn_index) = if pinh.turn == 0 && own_turn.is_none() {
            (None, 0)
        } else {
            let mut outer_clip = intersect(pinh.clip, clip);
            if let Some((mask, _)) = rounded { outer_clip = intersect(outer_clip, mask); }
            compose_turn(&mut self.turn_scratch, pinh.turn, own_turn, ([r[0], r[1], w, h], turn_pivot, outer_clip))
        };
        if let Some(angle) = own_turn {
            if !pinh.skip_paint {
                open_turned_layer(&mut self.instances, layers, (i, self.surfaces.len()), [r[0], r[1], w, h],
                    (angle, turn_pivot), (&mut clip, &mut fade, &mut fade_origin, &mut rounded));
            } else {
                (clip, fade, fade_origin, rounded) = (NO_CLIP, [0.0; 4], NO_CLIP, None);
            }
            pinh.clip = NO_CLIP;
            pinh.fade = [0.0; 4];
            pinh.fade_origin = NO_CLIP;
        }
        // Where the node shows on screen, upright: the turned box's bounds.
        let mut screen_r = [r_exact[0], r_exact[1], r_exact[2].max(r_exact[0]), r_exact[3].max(r_exact[1])];
        if let Some(t) = node_turn { screen_r = t.bbox(screen_r); }
        self.drawn[i] = [screen_r[0] / s, screen_r[1] / s, (screen_r[2] - screen_r[0]) / s, (screen_r[3] - screen_r[1]) / s];
        if cover.is_none() { self.drawn_scale[i] = sc; }
        let radii = p.radius.0.map(|v| v * sc * s);

        if !p.mask.is_none() {
            let ins = p.mask.inset.0.map(|v| v * sc * s);
            let fea = p.mask.feather.0.map(|v| v * sc * s);
            clip = [
                (r[0] + ins[0]).max(clip[0]),
                (r[1] + ins[1]).max(clip[1]),
                (r[2] - ins[2]).min(clip[2]),
                (r[3] - ins[3]).min(clip[3]),
            ];
            inherit_fade(&mut fade, &mut fade_origin, [r[0] + ins[0], r[1] + ins[1], r[2] - ins[2], r[3] - ins[3]], fea);
        }
        let m = filter_margin + 2.0;
        let visible = match node_turn {
            None => {
                let vis_clip = if pinh.in_cache { intersect(pinh.clip, clip) } else { clip };
                // Under a clip with no area nothing draws, however near its edge.
                vis_clip[2] > vis_clip[0] && vis_clip[3] > vis_clip[1]
                    && screen_r[2] >= vis_clip[0].max(vp[0]) - m
                    && screen_r[0] <= vis_clip[2].min(vp[2]) + m
                    && screen_r[3] >= vis_clip[1].max(vp[1]) - m
                    && screen_r[1] <= vis_clip[3].min(vp[3]) + m
            }
            Some(t) => {
                let lc = intersect(pinh.clip, clip);
                let oc = t.outer_clip;
                r[2] >= lc[0] - m && r[0] <= lc[2] + m && r[3] >= lc[1] - m && r[1] <= lc[3] + m
                    && screen_r[2] >= oc[0].max(vp[0]) - m
                    && screen_r[0] <= oc[2].min(vp[2]) + m
                    && screen_r[3] >= oc[1].max(vp[1]) - m
                    && screen_r[1] <= oc[3].min(vp[3]) + m
            }
        };
        self.seen[i] = visible;

        // A slide may sweep wholly outside its clip; judge visibility at its resting place.
        let home = match n.looping.as_ref().map(|l| l.kind) {
            Some(rune_motion::LoopKind::Slide { .. } | rune_motion::LoopKind::SlideEased { .. }) => {
                let o = self.scene.get(n.id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
                [r[0] - o.x * s, r[1] - o.y * s, r[2] - o.x * s, r[3] - o.y * s]
            }
            _ => r,
        };
        // A loop at its brightest under one 8-bit step can't be seen: let it sleep.
        let loop_vis = match node_turn {
            None => home[2] > clip[0].max(vp[0] - filter_margin) && home[0] < clip[2].min(vp[2] + filter_margin)
                && home[3] > clip[1].max(vp[1] - filter_margin) && home[1] < clip[3].min(vp[3] + filter_margin),
            Some(t) => {
                let lc = intersect(pinh.clip, clip);
                let sh = t.bbox(home);
                let oc = t.outer_clip;
                home[2] > lc[0] - filter_margin && home[0] < lc[2] + filter_margin
                    && home[3] > lc[1] - filter_margin && home[1] < lc[3] + filter_margin
                    && sh[2] > oc[0].max(vp[0] - filter_margin) && sh[0] < oc[2].min(vp[2] + filter_margin)
                    && sh[3] > oc[1].max(vp[1] - filter_margin) && sh[1] < oc[3].min(vp[3] + filter_margin)
            }
        };
        if self.live.visible() && base_opacity >= 1.0 / 255.0 && !self.reduced_motion
            && loop_vis
            && let Some(track) = self.loops.get(&n.id)
        {
            let node_k = self.scene.store().node_time_scale(n.id) as f64;
            if let Some(dt) = track.frame_interval().filter(|_| track.active(lnow)) {
                *ambient = Some(ambient.map_or(dt as f32, |a| a.min(dt as f32)));
                let wake = self.clock + dt;
                if self.loop_wake.is_none_or(|w| wake < w.0) {
                    self.loop_wake = Some((wake, now + dt * self.time_scale as f64));
                }
            } else if self.time_scale > 0.0 && node_k > 0.0 {
                self.loops_active |= track.active(lnow);
            }
            if self.time_scale > 0.0 && node_k > 0.0 && let Some(start) = track.pending_start(lnow) {
                let window_start = now + (start - lnow) / node_k;
                self.paint_keep.cycles.other_deadline();
                let wake = self.clock + (window_start - now) / self.time_scale as f64;
                if self.loop_wake.is_none_or(|w| wake < w.0) { self.loop_wake = Some((wake, window_start)); }
            }
        }

        NodePose {
            mark,
            base_opacity,
            hover_owner,
            group,
            pressed,
            press_to,
            loop_value,
            effective_opacity,
            hit_opacity,
            next,
            dissolve,
            paint_own,
            isolate,
            clip,
            fade,
            fade_origin,
            rounded,
            press_k,
            scale_motion,
            sc,
            anchor,
            anchor_to,
            press_anchor,
            blur,
            filter_margin,
            raw_edges,
            rect_pt,
            cover,
            r_exact,
            unscaled,
            opens_snap_layer,
            snap_layer,
            steps,
            gliding,
            r,
            snap_rest,
            w,
            h,
            own_turn,
            node_turn,
            turn_index,
            radii,
            m,
            visible,
            loop_vis,
        }
    }
}
