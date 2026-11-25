//! Frame paint pass: `Scene` presented poses, clips, layers and text → GPU `Instance`s.
use super::*;

mod layers;
pub(in crate::runtime) mod pose;
mod primitives;

impl RuntimeCore {
    pub(in crate::runtime) fn build(&mut self) -> bool {
        let (_paint_time, _blend) = (crate::frame::profile::Scope::new(crate::frame::profile::Phase::Paint), crate::theme::scope(self.paint.srgb_blend));
        let s = self.env.scale;
        let snap_px = |v: f32| grid(v, s) * s;
        self.keep_swap();
        self.instances.clear();
        // Quads a text hasn't drawn with for a while go, with the layouts they hold.
        self.paint.text_quads.begin_frame();
        let mut pinned_pages = 0u32;
        for f in self.motion.flights() {
            for q in &f.glyphs {
                if q.params[1] == KIND_GLYPH {
                    pinned_pages |= 1u32 << (q.params[2] as u32).min(31);
                }
            }
        }
        if let Some(after) = self.text.begin_frame_pinned(pinned_pages) {
            self.schedule_wakes([after]);
        }
        self.marks.clear();
        self.paint.icon_aprons.clear();
        self.surfaces.clear();
        self.paint.clear_materials();
        self.hit.hits.clear();
        self.hit.wheel_hits.clear();
        self.clear_help_hits();
        self.clear_drop_hits();
        self.motion.reset_loops_active();
        self.paint.scale_frame = None;
        self.paint.videos_active = false;
        self.paint.animated_icons = false;
        self.paint.animated_icons_shown = false;
        self.paint.node_animated_icon = false;
        self.paint.icon_anim_wake = None;
        let now = crate::time::now();
        let video_clock = match self.wall_start {
            None => crate::video::Clock::Virtual(self.clock),
            Some(_) => crate::video::Clock::Display(self.frame.frame_target().filter(|t| *t > now).unwrap_or(now)),
        };
        let mut video_signature = 0u64;
        self.motion.clear_loop_wake();
        self.hit.overlay_hits.clear();
        let n_nodes = self.tree.nodes.len();
        self.paint.inh_scratch.clear();
        self.paint.inh_cold.clear();
        self.paint.turn_scratch.clear();
        self.paint.unturned_drawn.clear();
        self.paint.inh_scratch.resize(n_nodes, Inh::default());
        self.paint.drawn.clear();
        self.paint.drawn.resize(n_nodes, [0.0; 4]);
        self.paint.drawn_scale.clear();
        self.paint.drawn_scale.resize(n_nodes, 1.0);
        self.paint.seen.clear();
        self.paint.seen.resize(n_nodes, false);
        self.scroll.stuck.clear();
        self.scroll.sticky_bands.clear();
        self.scroll.sticky_frame.clear();
        if self.tree.has_sticky {
            self.scroll.stuck.resize(n_nodes, 0.0);
            self.scroll.sticky_frame.resize(n_nodes, sticky::UNBOUNDED);
        }
        let sel_ranges = self.selection_ranges();
        let mark_ranges = self.mark_ranges();
        self.ime_area = None;
        self.edit.ime_option_as_alt = crate::OptionAsAlt::None;
        let now = self.t;
        let vp = [0.0, 0.0, self.viewport.0 * s, self.viewport.1 * s];
        let mut glyph_count = 0usize;
        let mut ambient: Option<f32> = None;
        // Retain scope capacity across frames. Records preserve absolute instance indices
        // for images, video insertions, retained springs and compositor list ranges.
        let mut layers = std::mem::take(&mut self.paint.layer_scratch);
        layers.clear();

        self.paint.prepare_dissolve(&self.tree, &self.scene, &self.motion, self.clock, self.reduced_motion, self.kbd.focus.keyboard(), self.kbd.focus.path());

        let paint_state_hash = self.prepare_cache_layers(&sel_ranges, &mark_ranges);

        self.place_growth_rows();

        let flying = !self.motion.flying.is_empty();
        let tree_order = self.paint.needs_tree_order(&self.tree, &self.scene, flying);
        let default_ring = self.default_ring_owner();
        self.keep_begin(tree_order, vp, default_ring, &sel_ranges, &mark_ranges);
        let mut active_cache: Option<ActiveCache> = None;
        self.paint.pixel_snap.begin_frame(self.t, [vp[2], vp[3]]);
        // One past the subtree the last one copied from the last paint.
        let mut skip_to = 0;
        for paint_index in 0..n_nodes {
            if paint_index < skip_to { continue; }
            let i = if tree_order { self.paint.layer_paint_order[paint_index] } else { paint_index };
            if let Some(rank) = self.paint.paint_rank.as_mut() {
                rank.resize(n_nodes, 0);
                rank[i] = paint_index as u32;
            }
            let node_id = self.tree.nodes[i].id;
            let node_parent = self.tree.nodes[i].parent;
            // A lifted flight paints outside every ancestor's layer, above the whole window.
            // `El::matched_in_place` keeps the flight in its destination's paint order and layers.
            let is_flight = flying && self.motion.flying.iter().any(|f| f.0 == node_id);
            let flight_in_place = is_flight && self.tree.nodes[i].vis.flight_in_place && !self.motion.launches().launched(node_id);
            let lift = is_flight && !flight_in_place;
            let raised = (self.tree.nodes[i].vis_flags.raise() || (tree_order && self.paint.is_lifted_anchor(i))) && !lift;
            if lift || raised {
                self.paint.close_all_layers(&self.tree.nodes, &mut self.instances, &mut self.surfaces, &mut self.marks, self.track_marks, &mut self.text, self.commit.commit_gen(), &mut layers, &mut active_cache, &mut glyph_count);
            }
            self.keep_close(Some(node_parent), &mut layers, &mut active_cache, &mut glyph_count);
            self.paint.close_layers_to_parent(&self.tree.nodes, &mut self.instances, &mut self.surfaces, &mut self.marks, self.track_marks, &mut self.text, self.commit.commit_gen(), node_parent, &mut layers, &mut active_cache, &mut glyph_count);
            self.keep_open(i, glyph_count, &sel_ranges, &mark_ranges);
            let n = &self.tree.nodes[i];
            let Some(p) = self.scene.presented(n.id) else { continue };
            let mut pinh = n.parent.map_or_else(|| self.root_inh(), |pr| self.paint.inh_scratch[pr]);
            let parent_cold = pinh.cold;
            if pinh.anchor_hidden() || super::super::commit::inline::is_elided_inline_child(&self.tree.nodes, i) {
                pinh.set_anchor_hidden(true);
                self.paint.inh_scratch[i] = pinh;
                self.paint.drawn[i] = [0.0; 4];
                self.paint.seen[i] = false;
                self.keep_painted(i, false, None);
                continue;
            }
            let mut pcold_buf: Option<InhCold> = None;
            // A stuck header's title (its first child) fades as the next header comes up.
            if parent_cold != 0 {
                let first_fade = self.paint.inh_cold_of(&pinh).first_fade;
                if first_fade < 1.0 {
                    if n.parent.is_some_and(|pr| pr + 1 == i) {
                        pinh.opacity *= first_fade;
                        pinh.paint_opacity *= first_fade;
                    }
                    let mut c = *self.paint.inh_cold_of(&pinh);
                    c.first_fade = 1.0;
                    pcold_buf = Some(c);
                }
            }
            // `El::stuck_reveal`: shown as far as its sticky parent has held.
            if let Some(span) = n.sticky.as_ref().and_then(|s| s.reveal)
                && let Some(pr) = n.parent
            {
                let held = if self.tree.has_sticky { self.scroll.stuck.get(pr).map_or(0.0, |s| s.abs()) } else { 0.0 };
                let shown = (held / span).clamp(0.0, 1.0);
                pinh.opacity *= shown;
                pinh.paint_opacity *= shown;
            }
            pinh.dy += self.paint.row_dy[i];
            pinh.grow_rigid(self.paint.row_dy[i]);
            if let Some(a) = self.tree.nodes[i].above.clone() {
                if let Some(j) = anchor_index_live(&self.tree.nodes, i, &a, |idx| self.subtree_exiting(idx)) {
                    let mut pc = pcold_buf.unwrap_or_else(|| *self.paint.inh_cold_of(&pinh));
                    let saved = self.anchored_inh(i, j, &a, &p, &mut pinh, &mut pc);
                    pcold_buf = Some(pc);
                    self.paint.last_anchor_targets.insert(node_id, saved);
                } else if self.subtree_exiting(i)
                    && let Some((target, k, bounds)) = self.paint.last_anchor_targets.get(&node_id).copied()
                {
                    let mut pc = pcold_buf.unwrap_or_else(|| *self.paint.inh_cold_of(&pinh));
                    self.anchored_inh_at(i, &a, target, k, bounds, ScaleMotion::Still, &p, &mut pinh, &mut pc);
                    pcold_buf = Some(pc);
                } else {
                    // Never draw an anchored overlay at (0, 0) when its anchor cannot be resolved.
                    let path = self.tree.nodes[i].path.clone();
                    if self.paint.unresolved_anchor_warnings.insert((path.clone(), a.key.clone())) {
                        eprintln!("rune: anchored overlay {path:?} could not resolve anchor {:?}; suppressed (never drawn at origin)", a.key);
                    }
                    pinh.set_anchor_hidden(true);
                    pinh.opacity = 0.0;
                    pinh.paint_opacity = 0.0;
                    pinh.hit_opacity = 0.0;
                    if let Some(pc) = pcold_buf {
                        pinh.cold = self.paint.store_inh_cold(parent_cold, pc);
                    }
                    self.paint.inh_scratch[i] = pinh;
                    self.paint.drawn[i] = [0.0; 4];
                    self.paint.seen[i] = false;
                    self.keep_painted(i, false, None);
                    continue;
                }
            }
            let kids_fade = if self.tree.has_sticky {
                self.apply_sticky_inh(i, node_parent, &p, &mut pinh, &mut pcold_buf)
            } else {
                1.0
            };
            if let Some(len) = {
                let (cold_ref, is_default) = match &pcold_buf {
                    Some(c) => (c, *c == InhCold::DEFAULT),
                    None => (self.paint.inh_cold_of(&pinh), pinh.cold == 0),
                };
                let cold_copy = *cold_ref;
                self.keep_or_paint(i, &pinh, &cold_copy, is_default, &mut glyph_count)
            } {
                skip_to = paint_index + len;
                continue;
            }
            if raised {
                let mut pc = pcold_buf.unwrap_or_else(|| *self.paint.inh_cold_of(&pinh));
                self.paint.reopen_raised_layers(&self.tree.nodes, &self.scene, &mut self.instances, self.surfaces.len(), node_parent, &mut pinh, &mut pc, &mut layers, s);
                pcold_buf = Some(pc);
            }
            let flight_dest = if is_flight {
                let mut pc = pcold_buf.unwrap_or_else(|| *self.paint.inh_cold_of(&pinh));
                let dest = self.motion.flight_inh(&self.tree.nodes, &self.paint, &self.scene, self.commit.commit_gen(), node_id, node_parent, !flight_in_place, &mut pinh, &mut pc);
                pcold_buf = Some(pc);
                dest
            } else {
                None
            };
            let mut pcold = pcold_buf.unwrap_or_else(|| *self.paint.inh_cold_of(&pinh));
            let n = &self.tree.nodes[i];
            let mark = if n.vis.mark != 0 { n.vis.mark } else { pinh.mark };
            let mut own = p.opacity;
            let mut hit_own = own * group_factor(&self.scene, self.kbd.focus.keyboard(), self.kbd.focus.path(), n, 1.0);
            own *= group_factor(&self.scene, self.kbd.focus.keyboard(), self.kbd.focus.path(), n, group_lit(&self.scene, n, pinh.group));
            let base_opacity = (own * pinh.opacity).clamp(0.0, 1.0);
            let hover_owner = n.vis.tracks_hover_paint() || n.vis_flags.hover_group();
            let group = if hover_owner {
                self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0)
            } else {
                pinh.group
            };
            let (pressed, press_to) = match n.vis.cold.get().and_then(|c| c.press_color) {
                Some(c) => (self.scene.get(n.id, P_PRESS).unwrap_or(0.0).clamp(0.0, 1.0), Some(c)),
                None => (pcold.pressed, pcold.press_to),
            };
            // Loops and reveals run on the node's clock (`El::time_scale`).
            let track = self.motion.loop_track(n.id);
            let lnow = if track.is_some() { self.scene.store().node_time(n.id) } else { self.t };
            let loop_value = track.map(|l| l.sample_at(lnow, self.clock, self.reduced_motion));
            if fades(n.looping.as_deref().copied()) && let Some(v) = loop_value {
                own *= v;
                hit_own *= v;
            }
            let effective_opacity = (own * pinh.opacity).clamp(0.0, 1.0);
            let hit_opacity = (hit_own * pinh.hit_opacity).clamp(0.0, 1.0);
            let next = if tree_order { self.paint.layer_paint_order.get(paint_index + 1).copied() } else { (i + 1 < n_nodes).then_some(i + 1) };
            let dissolve = self.tree.has_dissolve && self.paint.dissolve_scratch[i];
            let additive_parent = self.tree.has_dissolve && n.parent.is_some_and(|p| self.paint.dissolve_scratch[p]);
            // `El::layer_opacity` fades paint only; hit testing and what elements report keep
            // `own`.
            let paint_own = own * n.vis.layer_opacity;
            // A fading node with children, or a shape whose pieces overlap, takes its opacity
            // once as a layer rather than each primitive taking it over the ones beneath. So
            // does a layer opacity over two paints of the node's own (a label on its fill).
            let mut isolate = effective_opacity > 0.001 && (p.blur > 0.0 || dissolve
                || additive_parent || (paint_own > 0.001 && paint_own < 1.0
                    && (next.is_some_and(|j| self.tree.nodes[j].parent == Some(i)) || n.vis.shape.as_ref().is_some_and(|s| s.overlaps())
                        || (n.vis.layer_opacity < 1.0 && own_paints(n) >= 2))));
            let (mut clip, mut fade) = (pinh.paint_clip, pcold.paint_fade);
            let mut fade_origin = pcold.paint_fade_origin;
            let mut rounded = pcold.rounded;
            let has_press_scale = n.handlers.get().is_some_and(|h| h.press_scale.is_some());
            let press_k = if has_press_scale { self.scene.get(n.id, P_PRESS_SCALE).unwrap_or(1.0) } else { 1.0 };
            let has_press_offset = n.handlers.get().is_some_and(|h| h.press_offset.is_some());
            let press_off = if has_press_offset {
                [self.scene.get(n.id, P_PRESS_OX).unwrap_or(0.0), self.scene.get(n.id, P_PRESS_OY).unwrap_or(0.0)]
            } else {
                [0.0; 2]
            };
            let sequence = track
                .filter(|_| matches!(n.looping.as_ref().map(|l| l.kind), Some(rune_motion::LoopKind::Sequence(_))));
            let static_k = if n.scale_track { self.scene.get(n.id, P_SCALE).unwrap_or(n.static_scale) } else { n.static_scale };
            let own_sc_rest = p.transform.scale.x * press_k
                * sequence.and_then(|l| l.pose(lnow, self.reduced_motion)).map_or(1.0, |p| p.scale);
            let own_sc = own_sc_rest * static_k;
            // An `El::scale` or `El::live_scale` an app drives from a slider, wheel or pinch
            // counts as moving until it holds still, then rasterizes crisp at its resting scale.
            // When Rune's own `P_SCALE` track drives the scale (without `live_scale`), the target
            // is known ahead of time: record `Still(target)` while it animates so the landing frame
            // rests immediately without starting a `STATIC_SCALE_SETTLE` tail.
            let has_live_scale = n.live.iter().any(|b| b.property == crate::live::Property::Scale);
            let tracked_scale = if has_live_scale { static_k * p.transform.scale.x } else { static_k };
            let store = self.scene.store();
            let p_scale_anim = n.scale_track && store.is_animating(n.id, P_SCALE);
            let p_scale_target = if !has_live_scale && p_scale_anim {
                Some(store.target(n.id, P_SCALE).unwrap_or(tracked_scale))
            } else {
                None
            };
            let scale_settling = if let Some(target) = p_scale_target {
                self.paint.static_scales.insert(n.id, ScaleSettle::Still(target));
                false
            } else if tracked_scale != 1.0 || has_live_scale || self.paint.static_scales.contains_key(&n.id) {
                pose::step_static_scale(&mut self.paint.static_scales, &mut self.paint.scale_frame, n.id, tracked_scale, self.t, self.settled, has_live_scale)
            } else { false };
            let own_scale_anim = (has_press_scale && store.is_animating(n.id, P_PRESS_SCALE))
                || p_scale_anim
                || sequence.is_some_and(|l| !self.reduced_motion && l.active(lnow));
            let presence_anim = store.is_animating(n.id, prop::PRESENCE);
            let scale_animating = store.is_animating(n.id, prop::SCALE) || presence_anim || own_scale_anim;
            let presence_scales = presence_anim
                && n.transition.as_deref().is_some_and(|t| t.insertion.scale != 1.0 || t.removal.scale != 1.0);
            let own_scale_moving = presence_scales || own_scale_anim;
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
            let p_scale_tracked = !has_live_scale && pinh.scale_motion != ScaleMotion::Live && (p_scale_target.is_some() || pinh.scale_tracked());
            let scale_target = if p_scale_tracked {
                pcold.scale_target * own_sc_rest * p_scale_target.unwrap_or(static_k)
            } else {
                sc
            };
            let press_target = if has_press_offset {
                [store.target(n.id, P_PRESS_OX).unwrap_or(0.0), store.target(n.id, P_PRESS_OY).unwrap_or(0.0)]
            } else {
                [0.0; 2]
            };
            let press_off_anim = has_press_offset
                && (store.is_animating(n.id, P_PRESS_OX) || store.is_animating(n.id, P_PRESS_OY));
            let own_press_by = if press_off_anim {
                [press_off[0] - press_target[0], press_off[1] - press_target[1]]
            } else {
                [0.0; 2]
            };
            let press_off_by = [pcold.press_off_by[0] + own_press_by[0], pcold.press_off_by[1] + own_press_by[1]];
            let hit_press_off = [press_off[0] * pinh.scale, press_off[1] * pinh.scale];
            if press_off != [0.0; 2] {
                pinh.dx += press_off[0];
                pinh.dy += press_off[1];
            }
            if press_target != [0.0; 2] {
                pinh.set_snap_shifted(true);
            }
            let pivot = match n.vis.origin {
                Some([ox, oy]) => [p.rect.x + ox * p.rect.w, p.rect.y + oy * p.rect.h],
                None => [p.anchor.x, p.anchor.y],
            };
            let centre = [pivot[0] + pinh.dx, pivot[1] + pinh.dy];
            let centre_to = [pcold.anchor_to[0] + (centre[0] - pcold.anchor[0]) * pinh.scale,
                pcold.anchor_to[1] + (centre[1] - pcold.anchor[1]) * pinh.scale];
            let (anchor, anchor_to) = if (own_sc - 1.0).abs() > 1e-5 { (centre, centre_to) } else { (pcold.anchor, pcold.anchor_to) };
            // A pressed element still hits at its resting size, so a press near its edge stays inside.
            let press_anchor = centre_to;
            let blur = pcold.blur + p.blur * sc;
            // An effect slot's bleed can reach into view from a rect outside it.
            let filter_margin = blur * s * 3.0 + n.vis.effect.map_or(0.0, |e| e.bleed * sc * s);

            let mut raw_edges = p.rect.edges();
            raw_edges[0] += pinh.dx;
            raw_edges[2] += pinh.dx;
            raw_edges[1] += pinh.dy;
            raw_edges[3] += pinh.dy;
            if self.paint.row_h[i] >= 0.0 {
                raw_edges[3] = raw_edges[1] + self.paint.row_h[i];
            }
            if self.tree.has_sticky {
                pose::update_sticky_frame(&mut self.scroll.sticky_frame, i, n, raw_edges);
            }
            // A product of 1 (0.5 inside 2.0) can still move the box.
            let mut rect_pt = if let Some(dest) = flight_dest {
                pose::flight_frame(&self.scene, n, &p, own_sc, self.paint.row_h[i], dest)
            } else if (sc - 1.0).abs() > 1e-5 || anchor != anchor_to {
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
            let cover = if is_flight && n.local.w > 0.0 && n.local.h > 0.0 {
                pose::flight_cover(self.motion.launches(), node_id, n, rect_pt, sc)
            } else { None };
            let r_exact = rect_pt.map(snap_px);
            // A node the visibility test below culls paints nothing, and any clip it hands down is
            // off screen too, so it keeps its exact edges.
            let rigid = pinh.rigid.map(|v| v * s);
            let can_snap = !pinh.snap_off() && !n.vis_flags.no_pixel_snap() && p.transform.rotate == 0.0
                && !(paint_state::PaintState::slide_glides(&self.tree.nodes, &self.motion, self.reduced_motion, i)
                    && !paint_state::PaintState::slide_pair(&self.tree.nodes, &self.motion, i));
            let snaps = can_snap && (pinh.in_cache() || {
                let m = filter_margin + 2.0;
                r_exact[2].max(r_exact[0]) >= clip[0].max(vp[0]) - m && r_exact[0] <= clip[2].min(vp[2]) + m
                    && r_exact[3].max(r_exact[1]) >= clip[1].max(vp[1]) - m && r_exact[1] <= clip[3].min(vp[3]) + m
            });
            let unscaled = (sc - 1.0).abs() <= 1e-5 && anchor == anchor_to;
            let animating = p.flags & (rune_motion::flags::ANIMATED | rune_motion::flags::ANCESTOR_ANIMATED) != 0;
            let opens_snap_layer = (own_sc - 1.0).abs() > 1e-5 || (n.above.is_some() && !unscaled);
            let snap_layer = if can_snap && opens_snap_layer {
                self.paint.pixel_snap.layer(n.id, (r_exact, raw_edges.map(snap_px)), rigid, pinh.in_cache() || n.vis_flags.cache())
            } else {
                pcold.snap_layer
            };
            let steps = pinh.snap_steps() || n.vis.cold.get().is_some_and(|c| c.motion_snaps);
            let rest = if snaps && animating && !steps { self.scene.rest(n.id) } else { None };
            let press_gliding = snaps && !steps && press_off_by != [0.0; 2];
            let gliding = snaps && (rest.is_some() || press_gliding || !unscaled);
            let (mut r, snap_rest) = if !snaps {
                (r_exact, None)
            } else if rest.is_none() && !press_gliding && unscaled {
                self.paint.pixel_snap.at_rest(n.id, r_exact, rigid, pcold.snap_parent)
            } else {
                let motion = pixel_snap::motion(&p, rest, s, n.above.is_some(), self.paint.row_h[i] >= 0.0);
                let motion = pixel_snap::with_offset_motion(motion, raw_edges, press_off_by, s);
                let raw = if unscaled { r_exact } else { raw_edges.map(snap_px) };
                self.paint.pixel_snap.snapped(n.id, motion, (r_exact, raw), rigid, (snap_layer, sc), pcold.snap_parent)
            };
            let hairline_alpha = if n.vis.cold.get().is_some_and(|c| c.hairline) {
                let (hr, a) = pixel_snap::hairline(r, r_exact);
                r = hr;
                a
            } else {
                1.0
            };
            let (w, h) = ((r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0));
            // `El::rotate`: this subtree draws into a layer turned about its origin.
            // The layer takes the clips from above; the node's own clips apply inside, upright.
            let own_turn = p.transform.rotate;
            let own_turn = (own_turn != 0.0 && own_turn.is_finite() && own_turn.abs() > 1e-6).then_some(own_turn);
            let turn_pivot = pose::turn_pivot(flight_dest, n.vis.origin, rect_pt, centre_to, s);
            let (node_turn, turn_index) = if pinh.turn == 0 && own_turn.is_none() {
                (None, 0)
            } else {
                let mut outer_clip = intersect(pinh.clip, clip);
                if let Some((mask, _)) = rounded { outer_clip = intersect(outer_clip, mask); }
                compose_turn(&mut self.paint.turn_scratch, pinh.turn, own_turn, ([r[0], r[1], w, h], turn_pivot, outer_clip))
            };
            if let Some(angle) = own_turn {
                if !pinh.skip_paint() {
                    open_turned_layer(&mut self.instances, &mut layers, (i, self.surfaces.len()), [r[0], r[1], w, h],
                        (angle, turn_pivot), (&mut clip, &mut fade, &mut fade_origin, &mut rounded));
                } else {
                    (clip, fade, fade_origin, rounded) = (NO_CLIP, [0.0; 4], NO_CLIP, None);
                }
                pinh.clip = NO_CLIP;
                pcold.fade = [0.0; 4];
                pcold.fade_origin = NO_CLIP;
            }
            // Where the node shows on screen, upright: the turned box's bounds.
            let mut screen_r = [r_exact[0], r_exact[1], r_exact[2].max(r_exact[0]), r_exact[3].max(r_exact[1])];
            if let Some(t) = node_turn { screen_r = t.bbox(screen_r); }
            self.paint.drawn[i] = [screen_r[0] / s, screen_r[1] / s, (screen_r[2] - screen_r[0]) / s, (screen_r[3] - screen_r[1]) / s];
            if cover.is_none() { self.paint.drawn_scale[i] = sc; }
            let radii = p.radius.0.map(|v| v * sc * s);

            if !p.mask.is_none() {
                let ins = p.mask.inset.0.map(|v| v * sc * s);
                let fea = p.mask.feather.0.map(|v| (v * sc * s).clamp(-65504.0, 65504.0));
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
                    let vis_clip = if pinh.in_cache() { intersect(pinh.clip, clip) } else { clip };
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
            self.paint.seen[i] = visible;

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
            if self.frame.live.visible() && base_opacity >= 1.0 / 255.0 && !self.reduced_motion && loop_vis {
                self.motion.schedule_loop_wake(&mut self.paint, &self.scene, self.clock, self.time_scale, node_id, lnow, now, &mut ambient);
            }

            let layers::LayerScope {
                o,
                skip_paint,
                in_cache,
                open_cache,
                blur_hole,
                glyphs_clear,
                mut hit_clip,
                mut hit_fade,
                mut hit_origin,
                primitive_start,
                surface_start,
            } = self.open_node_layers(
                i,
                s,
                now,
                &p,
                &pinh,
                &pcold,
                r,
                r_exact,
                radii,
                w,
                h,
                sc,
                p_scale_tracked.then_some(scale_target),
                m,
                effective_opacity,
                paint_own,
                visible,
                dissolve,
                is_flight,
                own_turn,
                node_turn.as_ref(),
                next,
                scale_motion,
                vp,
                paint_state_hash,
                glyph_count,
                &mut clip,
                &mut fade,
                &mut fade_origin,
                &mut rounded,
                &mut isolate,
                &mut layers,
                &mut active_cache,
            );

            let first_instance = self.instances.len();
            let n = &self.tree.nodes[i];
            // Native input readiness follows focus, not entrance opacity. An input
            // mounting at opacity zero still needs its IME area in this frame.
            let input_caret = n.input.as_ref().filter(|input| input.edit.focused && self.can_focus(i) && !self.node_disabled(i))
                .filter(|_| !self.motion.flights().iter().any(|f| n.key.as_deref() == Some(&f.source_key)
                    && f.alphas(f.destination, now, self.reduced_motion).0 > 0.0))
                .and_then(|_| input_caret(n, r, sc, s, gliding, &mut self.edit.input_scroll));
            if let Some((_, area)) = input_caret {
                if n.input.as_ref().is_some_and(|input| !input.edit.is_secure()) && !self.is_node_read_only(i) {
                    self.ime_area = Some(area);
                    self.edit.ime_option_as_alt = crate::OptionAsAlt::None;
                }
            } else if n.ime_cursor && self.ime_area.is_none() && self.allowed(i) && !self.subtree_exiting(i) {
                self.ime_area = Some(app_ime::area(r, s));
                self.edit.ime_option_as_alt = n.ime_option_as_alt;
            }
            let paint_visible = if in_cache {
                r[2] >= clip[0] - filter_margin - 2.0
                    && r[0] <= clip[2] + filter_margin + 2.0
                    && r[3] >= clip[1] - filter_margin - 2.0
                    && r[1] <= clip[3] + filter_margin + 2.0
            } else {
                visible
            };
            let painted = !skip_paint && effective_opacity > 0.001 && o > 0.001 && paint_visible;
            debug_assert!(!pinh.anchor_hidden(), "anchored overlay {:?} reached paint without a resolved anchor", n.path);
            // Unpulled out of sight, a video still counts, so a scroll across a clip edge doesn't commit.
            if !painted && let Some((surface, _)) = &n.vis.surface { video_signature = crate::video::fold_unpulled(video_signature, surface); }
            self.paint.node_animated_icon = false;
            let faded_slot = if painted {
                self.paint_node_box(
                    i,
                    s,
                    now,
                    &pinh,
                    &pcold,
                    r,
                    r_exact,
                    raw_edges,
                    radii,
                    w,
                    h,
                    sc,
                    o * hairline_alpha,
                    effective_opacity,
                    base_opacity,
                    p.draw,
                    m,
                    clip,
                    fade,
                    fade_origin,
                    vp,
                    pressed,
                    press_to,
                    loop_value,
                    blur_hole,
                    cover,
                    scale_motion,
                    snap_rest.as_ref(),
                    next,
                    default_ring,
                    primitive_start,
                    mark,
                    isolate,
                    in_cache,
                    skip_paint,
                    loop_vis,
                    gliding,
                    node_turn.is_some(),
                    input_caret.map(|(caret, _)| caret),
                    video_clock,
                    &sel_ranges,
                    &mark_ranges,
                    &mut video_signature,
                    &mut glyph_count,
                )
            } else {
                None
            };
            let packed_fade = crate::render::pack_fade(fade, fade_origin, clip);
            for instance in &mut self.instances[first_instance..] {
                instance.fade = if instance.clip == clip {
                    packed_fade
                } else {
                    crate::render::pack_fade(fade, fade_origin, instance.clip)
                };
            }
            let text_clip = if self.tree.nodes[i].collapsing && self.tree.nodes[i].vis.clip.is_none() && self.tree.nodes[i].scroll.is_none() {
                intersect(clip, r)
            } else {
                clip
            };
            self.clip_text_instances(i, first_instance, (r, r_exact), sc, text_clip, fade, fade_origin);
            let n = &self.tree.nodes[i];
            if let Some((_, radii)) = rounded {
                for instance in &mut self.instances[primitive_start..] {
                    if glyphs_clear && instance.params[1] == KIND_GLYPH { continue; }
                    instance.round_clip_corners(radii);
                }
                let packed = crate::render::pack_clip_radii(clip, radii);
                for surface in &mut self.surfaces[surface_start..] { surface.fade[3] = packed; }
            }
            // A faded effect slot's layer carries its clip; the slot and the layer's end don't.
            if let Some(at) = faded_slot {
                for instance in &mut self.instances[at..at + 2] { instance.fade = [0; 4]; }
            }
            if self.track_marks {
                self.marks.resize(self.instances.len(), mark);
            }
            // A scroll viewport clips its content with a hard edge unless it sets soft ones.
            if let Some(f) = n.vis.clip.or(n.scroll.map(|_| [0.0; 4])) {
                let outer = [clip, fade, fade_origin];
                // A sliding window a step moves clips from its anchor's pose (`slide_anchor`).
                let rs = self.paint.slide_anchor(&self.tree.nodes, &self.motion, &self.scene, s, self.reduced_motion, i, rect_pt, r_exact, r, if unscaled { 1.0 } else { sc });
                clip = intersect(rs, clip);
                let (own, origin) = self.scroll_edge_fade(i, f, rs, sc * s, p.rect.edges()[3]);
                inherit_fade(&mut fade, &mut fade_origin, origin, own);
                let root_inh = self.root_inh();
                self.paint.note_slide(&self.tree.nodes, &self.motion, &self.scene, self.instances.len(), s, self.viewport, self.reduced_motion, root_inh, i, outer, rect_pt, rs, (if unscaled { 1.0 } else { sc }, sc));
            } else if n.collapsing {
                clip = intersect(r, clip);
            }
            let n = &self.tree.nodes[i];
            let mask = self.paint.row_mask[i];
            if mask >= 0.0 {
                clip[3] = clip[3].min(r[3]);
                // A fold's lifted contents (`GrowTrack::hide_above`) never draw above its top.
                if self.paint.row_lifts.iter().any(|&(ri, _)| ri == i) { clip[1] = clip[1].max(r[1]); }
                inherit_fade(&mut fade, &mut fade_origin, r, [0.0, 0.0, 0.0, self.growth_feather(i)]);
            }
            let mut dy = pinh.dy;
            let snap_off = pinh.snap_off() || n.vis_flags.no_pixel_snap() || turn_index != 0;
            let mut next_rigid = pinh.rigid;
            let mut next_cold = pcold;
            if let Some(o) = n.vis.cold.get().and_then(|c| c.paint_contain) {
                next_cold.contain_at(&mut next_rigid, o);
            }
            if n.scroll.is_some()
                && let Some(st) = self.scroll.scrolls.get(&n.id)
            {
                let offset = self.stuck_offset(i, p.rect.edges()[3]).unwrap_or(st.offset);
                let held = self.bottom_offset(i, p.rect.edges()[3]).is_some_and(|b| (offset - b).abs() < 0.5);
                let exact = offset as f32 + st.slack.height();
                let (drawn, hold) = st.drawn.draw(exact, st.offset as f32, st.pinned(), s);
                dy -= drawn;
                next_rigid[1] -= drawn;
                if let Some(st) = self.scroll.scrolls.get_mut(&self.tree.nodes[i].id) { (st.held, st.drawn) = (held, hold); }
            }
            let (mut scale, mut anchor, mut anchor_to, mut dx) = (sc, anchor, anchor_to, pinh.dx);
            if n.scroll_axes.x() && let Some(st) = self.scroll.scrolls.get_mut(&self.tree.nodes[i].id) {
                st.drawn_x = grid(st.x.shift() as f32, s);
                dx -= st.drawn_x;
                next_rigid[0] -= st.drawn_x;
            }
            if open_cache && sc > 1e-4 {
                dx += (r_exact[0] / s - rect_pt[0]) / sc;
                dy += (r_exact[1] / s - rect_pt[1]) / sc;
            }
            let (mut cover_offset, mut outer) = (pcold.cover, pcold.outer);
            let scale_motion = if cover.is_some() { ScaleMotion::Moving } else { scale_motion };
            if let Some((k, c)) = cover {
                clip = intersect(clip, r);
                hit_clip = intersect(hit_clip, r);
                let plain = [dx, dy];
                dx = rect_pt[0] - p.rect.x + c[0] / k;
                dy += rect_pt[1] - p.rect.y - pinh.dy + c[1] / k;
                (scale, anchor, anchor_to) = (k, [rect_pt[0], rect_pt[1]], [rect_pt[0], rect_pt[1]]);
                cover_offset = [dx - plain[0], dy - plain[1]];
                outer = Some([pinh.clip, pcold.fade, pcold.fade_origin]);
            }
            inherit_fade(&mut hit_fade, &mut hit_origin, fade_origin, fade);
            let full_clip = intersect(hit_clip, clip);
            if node_turn.is_some() {
                if self.paint.unturned_drawn.is_empty() {
                    self.paint.unturned_drawn.resize(n_nodes, ([0.0; 4], NO_CLIP));
                }
                self.paint.unturned_drawn[i] = (r.map(|v| v / s), full_clip);
            }
            let group_rect = if hover_owner {
                if effective_opacity > 0.001 { rect_pt } else { intersect(rect_pt, pcold.group_rect) }
            } else { pcold.group_rect };
            let held_moving = pinh.held_moving() || own_scale_moving || cover.is_some();
            next_cold.first_fade = kids_fade;
            next_cold.fade = hit_fade;
            next_cold.fade_origin = hit_origin;
            next_cold.paint_fade = fade;
            next_cold.paint_fade_origin = fade_origin;
            next_cold.rounded = rounded;
            next_cold.outer = outer;
            next_cold.anchor = anchor;
            next_cold.anchor_to = anchor_to;
            next_cold.blur = blur;
            next_cold.cover = cover_offset;
            next_cold.pressed = pressed;
            next_cold.press_to = press_to;
            next_cold.group_rect = group_rect;
            next_cold.still = if scale_motion == ScaleMotion::Still { (scale, anchor, anchor_to) } else { pcold.still };
            next_cold.held = if !held_moving { (scale, anchor, anchor_to) } else { pcold.held };
            next_cold.scale_target = if cover.is_some() { scale } else { scale_target };
            next_cold.press_off_by = if cover.is_some() { [0.0; 2] } else { press_off_by };
            next_cold.snap_layer = if cover.is_some() { 0 } else { snap_layer };
            next_cold.snap_parent = snap_rest.filter(|_| cover.is_none() && next_rigid == pinh.rigid && pixel_snap::frames(&n.vis));
            let cold_idx = self.paint.store_inh_cold(parent_cold, next_cold);
            self.paint.inh_scratch[i] = Inh {
                opacity: effective_opacity,
                paint_opacity: o,
                hit_opacity: hit_opacity * kids_fade,
                scale,
                dx,
                dy,
                clip: full_clip,
                paint_clip: clip,
                group,
                mark,
                turn: turn_index,
                cold: cold_idx,
                rigid: next_rigid,
                scale_motion,
                flags: Inh::pack_flags(
                    held_moving,
                    p_scale_tracked && cover.is_none(),
                    in_cache,
                    skip_paint,
                    false,
                    snap_off,
                    steps,
                    pinh.snap_shifted() || n.vis.offset != [0.0; 2],
                ),
            };
            // Shown only under its group's hover, it takes presses where the pointer hovers the
            // group: a press in the same frame as the move that reveals it finds it, as a page's
            // `:hover` applies before the press does.
            let (hit_opacity, hit_rect) = if effective_opacity > 0.001 { (effective_opacity, rect_pt) } else { (hit_opacity, intersect(rect_pt, pcold.group_rect)) };
            self.register_node_hits(i, pinh.clip, (hit_opacity, visible), hit_rect, (press_k, press_anchor, hit_press_off));
            if self.tree.has_sticky {
                self.fade_below_header(i, s);
            }
            self.keep_painted(i, !self.paint.node_animated_icon && own_turn.is_none() && !opens_snap_layer && snap_layer == 0 && !in_cache && !skip_paint && cover.is_none() && !is_flight && !raised, Some(paint_keep::ShiftNode { screen_r, r, rect_pt, m, visible, can_snap, gliding, isolate, unmasked: p.mask.is_none(), hover_owner, effective_opacity }));
        }
        if !self.paint.icon_anim_mounts.is_empty() {
            self.paint.icon_anim_mounts.retain(|(id, _), _| self.tree.node_index.contains_key(id));
        }
        if !self.paint.last_anchor_targets.is_empty() {
            self.paint.last_anchor_targets.retain(|id, _| self.tree.node_index.contains_key(id));
        }
        self.keep_close(None, &mut layers, &mut active_cache, &mut glyph_count);
        self.keep_end();
        if self.paint.tracks_native_materials()
            && !self.paint.material_fallbacks.is_empty()
            && !self.paint.material_fallback_warned
        {
            self.paint.material_fallback_warned = true;
            let fallen = self.paint.material_fallbacks.len();
            let total = self.paint.materials.len() + fallen;
            eprintln!("{}", crate::system_material::fallback_notice(fallen, total, self.paint.max_native_materials));
        }
        self.paint.close_all_layers(&self.tree.nodes, &mut self.instances, &mut self.surfaces, &mut self.marks, self.track_marks, &mut self.text, self.commit.commit_gen(), &mut layers, &mut active_cache, &mut glyph_count);
        // A moving chip can enter a stationary pointer. Re-evaluate against this frame's
        // geometry without inventing pointer-move or drag events on animation frames. Tooltips
        // follow the hold this decides.
        let queued = self.frame.queue.len();
        let held = self.pointer.pos.map(|(x, y)| self.refresh_hover(x, y));
        self.frame.queue.observe_since(queued);
        self.update_help();
        self.paint.layer_scratch = layers;
        self.tree.built = true;
        self.land_unseen_entrances();
        self.tree.built_ids.clear();
        self.tree.built_ids.extend(self.tree.nodes.iter().map(|n| n.id));
        if video_signature != self.paint.video_signature {
            // A video loaded, ended or failed: views reading its state re-run next frame.
            self.paint.video_signature = video_signature;
            self.commit.dirty = true;
            self.paint.videos_active = true;
        }
        if self.track_marks { self.marks.resize(self.instances.len(), 0); }
        self.stats.glyphs = glyph_count;
        self.ambient = ambient;
        // A live transform can change hover paint with no handler and no running spring.
        // Retarget from the new geometry and schedule the frame that samples that paint. A
        // commit that moved the indices left keyboard focus hover (`El::hover_group`) to
        // this pass too.
        let interaction = (self.pointer.pos.is_some() || self.kbd.focus.keyboard()) && self.update_interaction(held);
        self.frame.queue.len() > queued || interaction
    }
}
