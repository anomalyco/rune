//! Frame paint pass: `Scene` presented poses, clips, layers and text → GPU `Instance`s.
use super::*;

#[path = "build/layers.rs"]
mod layers;
#[path = "build/pose.rs"]
mod pose;
#[path = "build/primitives.rs"]
mod primitives;

impl RuntimeCore {
    pub(super) fn build(&mut self) -> bool {
        let (_paint_time, _blend) = (crate::frame::profile::Scope::new(crate::frame::profile::Phase::Paint), crate::theme::scope(self.srgb_blend));
        let s = self.env.scale;
        self.keep_swap();
        self.instances.clear();
        self.quads_frame += 1;
        // Quads a text hasn't drawn with for a while go, with the layouts they hold.
        if self.quads_frame % 64 == 0 {
            let frame = self.quads_frame;
            self.text_quads.retain(|_, (_, used)| frame - *used < 64);
        }
        let mut pinned_pages = 0u32;
        for f in &self.flights {
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
        self.icon_aprons.clear();
        self.surfaces.clear();
        self.materials.clear();
        self.hits.clear();
        self.wheel_hits.clear();
        self.clear_help_hits();
        self.clear_drop_hits();
        self.loops_active = false;
        self.scale_frame = None;
        self.videos_active = false;
        let now = crate::time::Instant::now();
        let video_clock = match self.wall_start {
            None => crate::video::Clock::Virtual(self.clock),
            Some(_) => crate::video::Clock::Display(self.frame_target.filter(|t| *t > now).unwrap_or(now)),
        };
        let mut video_signature = 0u64;
        self.loop_wake = None;
        self.overlay_hits.clear();
        let n_nodes = self.nodes.len();
        self.inh_scratch.clear();
        self.turn_scratch.clear();
        self.unturned_drawn.clear();
        self.inh_scratch.resize(n_nodes, Inh::default());
        self.drawn.clear();
        self.drawn.resize(n_nodes, [0.0; 4]);
        self.drawn_scale.clear();
        self.drawn_scale.resize(n_nodes, 1.0);
        self.seen.clear();
        self.seen.resize(n_nodes, false);
        self.stuck.clear();
        self.sticky_bands.clear();
        self.sticky_frame.clear();
        if self.has_sticky {
            self.stuck.resize(n_nodes, 0.0);
            self.sticky_frame.resize(n_nodes, sticky::UNBOUNDED);
        }
        let sel_ranges = self.selection_ranges();
        let mark_ranges = self.mark_ranges();
        self.ime_area = None;
        self.ime_option_as_alt = false;
        let now = self.t;
        let vp = [0.0, 0.0, self.viewport.0 * s, self.viewport.1 * s];
        let mut glyph_count = 0usize;
        let mut backdrop_blurs = 0usize;
        let mut ambient: Option<f32> = None;
        // Retain scope capacity across frames. Records preserve absolute instance indices
        // for images, video insertions, retained springs and compositor list ranges.
        let mut layers = std::mem::take(&mut self.layer_scratch);
        layers.clear();

        self.prepare_dissolve();

        let paint_state_hash = self.prepare_cache_layers(&sel_ranges, &mark_ranges);

        self.place_growth_rows();

        let flying = !self.flying.is_empty();
        let tree_order = self.needs_tree_order(flying);
        let default_ring = self.default_ring_owner();
        self.keep_begin(tree_order, vp, default_ring, &sel_ranges, &mark_ranges);
        let mut active_cache: Option<ActiveCache> = None;
        self.pixel_snap.begin_frame(self.t, [vp[2], vp[3]]);
        // One past the subtree the last one copied from the last paint.
        let mut skip_to = 0;
        for paint_index in 0..n_nodes {
            if paint_index < skip_to { continue; }
            let i = if tree_order { self.layer_paint_order[paint_index] } else { paint_index };
            if let Some(rank) = self.paint_rank.as_mut() {
                rank.resize(n_nodes, 0);
                rank[i] = paint_index as u32;
            }
            let node_id = self.nodes[i].id;
            let node_parent = self.nodes[i].parent;
            // A flight paints outside every ancestor's layer, above the whole window.
            let lift = flying && self.flying.iter().any(|f| f.0 == node_id);
            let raised = self.nodes[i].vis_flags.raise() && !lift;
            if lift || raised {
                self.close_all_layers(&mut layers, &mut active_cache, &mut glyph_count);
            }
            self.keep_close(Some(node_parent), &mut layers, &mut active_cache, &mut glyph_count);
            self.close_layers_to_parent(node_parent, &mut layers, &mut active_cache, &mut glyph_count);
            self.keep_open(i, glyph_count, &sel_ranges, &mark_ranges);
            let n = &self.nodes[i];
            let Some(p) = self.scene.presented(n.id) else { continue };
            let mut pinh = n.parent.map_or_else(|| self.root_inh(), |pr| self.inh_scratch[pr]);
            // A stuck header's title (its first child) fades as the next header comes up.
            if pinh.first_fade < 1.0 && n.parent.is_some_and(|pr| pr + 1 == i) {
                pinh.opacity *= pinh.first_fade;
                pinh.paint_opacity *= pinh.first_fade;
            }
            pinh.first_fade = 1.0;
            // `El::stuck_reveal`: shown as far as its sticky parent has held.
            if let Some(span) = n.sticky.as_ref().and_then(|s| s.reveal)
                && let Some(pr) = n.parent
            {
                let held = if self.has_sticky { self.stuck.get(pr).map_or(0.0, |s| s.abs()) } else { 0.0 };
                let shown = (held / span).clamp(0.0, 1.0);
                pinh.opacity *= shown;
                pinh.paint_opacity *= shown;
            }
            pinh.dy += self.row_dy[i];
            pinh.snap.grow(self.row_dy[i]);
            if let Some(a) = &n.above
                && let Some(j) = anchor_index(&self.nodes, i, a)
            {
                self.anchored_inh(i, j, a, &p, &mut pinh);
            }
            let kids_fade = if self.has_sticky {
                self.apply_sticky_inh(i, node_parent, &p, &mut pinh)
            } else {
                1.0
            };
            if let Some(len) = self.keep_or_paint(i, &pinh, &mut glyph_count) {
                skip_to = paint_index + len;
                continue;
            }
            if raised {
                self.reopen_raised_layers(node_parent, &mut pinh, &mut layers, s);
            }
            if lift {
                self.lift_flight_inh(node_id, node_parent, &mut pinh);
            }
            let mut pose = self.resolve_pose(
                i,
                paint_index,
                tree_order,
                lift,
                &p,
                vp,
                &mut pinh,
                &mut layers,
                &mut ambient,
            );
            let scope = self.open_node_layers(
                i,
                &p,
                &pinh,
                &mut pose,
                vp,
                lift,
                paint_state_hash,
                glyph_count,
                &mut backdrop_blurs,
                &mut layers,
                &mut active_cache,
            );

            let first_instance = self.instances.len();
            let n = &self.nodes[i];
            // Native input readiness follows focus, not entrance opacity. An input
            // mounting at opacity zero still needs its IME area in this frame.
            let input_caret = n.input.as_ref().filter(|input| input.edit.focused && self.can_focus(i))
                .filter(|_| !self.flights.iter().any(|f| n.key.as_deref() == Some(&f.source_key)
                    && f.alphas(f.destination, now, self.reduced_motion).0 > 0.0))
                .and_then(|_| input_caret(n, pose.r, pose.sc, s, pose.gliding, &mut self.input_scroll));
            if let Some((_, area)) = input_caret
                && n.input.as_ref().is_some_and(|input| !input.edit.is_secure()) { self.ime_area = Some(area); }
            else if n.ime_cursor && self.ime_area.is_none() && self.allowed(i) && !self.subtree_exiting(i) { self.ime_area = Some(app_ime::area(pose.r, s)); self.ime_option_as_alt = n.ime_option_as_alt; }
            let paint_visible = if scope.in_cache {
                pose.r[2] >= pose.clip[0] - pose.filter_margin - 2.0
                    && pose.r[0] <= pose.clip[2] + pose.filter_margin + 2.0
                    && pose.r[3] >= pose.clip[1] - pose.filter_margin - 2.0
                    && pose.r[1] <= pose.clip[3] + pose.filter_margin + 2.0
            } else {
                pose.visible
            };
            let painted = !scope.skip_paint && pose.effective_opacity > 0.001 && scope.o > 0.001 && paint_visible;
            // Unpulled out of sight, a video still counts, so a scroll across a clip edge doesn't commit.
            if !painted && let Some((surface, _)) = &n.vis.surface { video_signature = crate::video::fold_unpulled(video_signature, surface); }
            let faded_slot = if painted {
                self.paint_node_box(
                    i,
                    &pinh,
                    &pose,
                    &scope,
                    p.draw,
                    vp,
                    default_ring,
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
            let packed_fade = crate::render::pack_fade(pose.fade, pose.fade_origin, pose.clip);
            for instance in &mut self.instances[first_instance..] {
                instance.fade = packed_fade;
            }
            self.clip_text_instances(i, first_instance, (pose.r, pose.r_exact), pose.sc, pose.clip, pose.fade, pose.fade_origin);
            let n = &self.nodes[i];
            if let Some((_, radii)) = pose.rounded {
                for instance in &mut self.instances[scope.primitive_start..] {
                    if scope.glyphs_clear && instance.params[1] == KIND_GLYPH { continue; }
                    instance.round_clip_corners(radii);
                }
                let packed = crate::render::pack_clip_radii(pose.clip, radii);
                for surface in &mut self.surfaces[scope.surface_start..] { surface.fade[3] = packed; }
            }
            // A faded effect slot's layer carries its clip; the slot and the layer's end don't.
            if let Some(at) = faded_slot {
                for instance in &mut self.instances[at..at + 2] { instance.fade = [0; 4]; }
            }
            if self.track_marks {
                self.marks.resize(self.instances.len(), pose.mark);
            }
            let (mut clip, mut fade, mut fade_origin) = (pose.clip, pose.fade, pose.fade_origin);
            let (mut hit_clip, mut hit_fade, mut hit_origin) = (scope.hit_clip, scope.hit_fade, scope.hit_origin);
            // A scroll viewport clips its content with a hard edge unless it sets soft ones.
            if let Some(f) = n.vis.clip.or(n.scroll.map(|_| [0.0; 4])) {
                let outer = [clip, fade, fade_origin];
                // A sliding window a step moves clips from its anchor's pose (`slide_anchor`).
                let rs = self.slide_anchor(i, pose.rect_pt, pose.r_exact, pose.r, if pose.unscaled { 1.0 } else { pose.sc });
                clip = intersect(rs, clip);
                let (own, origin) = self.scroll_edge_fade(i, f, rs, pose.sc * s, p.rect.edges()[3]);
                inherit_fade(&mut fade, &mut fade_origin, origin, own);
                self.note_slide(i, outer, pose.rect_pt, rs, (if pose.unscaled { 1.0 } else { pose.sc }, pose.sc));
            }
            let n = &self.nodes[i];
            let mask = self.row_mask[i];
            if mask >= 0.0 {
                clip[3] = clip[3].min(pose.r[3]);
                // A fold's lifted contents (`GrowTrack::hide_above`) never draw above its top.
                if self.row_lifts.iter().any(|&(ri, _)| ri == i) { clip[1] = clip[1].max(pose.r[1]); }
                inherit_fade(&mut fade, &mut fade_origin, pose.r, [0.0, 0.0, 0.0, self.growth_feather(i)]);
            }
            let mut dy = pinh.dy;
            let snap_off = pinh.snap.off || n.vis_flags.no_pixel_snap() || pose.turn_index != 0;
            let mut snap = pinh.snap;
            // `El::paint_contained`: its children snap against the containing box's origin,
            // rounded on its own, as a translation that snaps as a whole.
            if let Some(o) = n.vis.cold.get().and_then(|c| c.paint_contain) {
                snap.contain_at(o);
            }
            if n.scroll.is_some()
                && let Some(st) = self.scrolls.get(&n.id)
            {
                let offset = self.stuck_offset(i, p.rect.edges()[3]).unwrap_or(st.offset);
                let held = self.bottom_offset(i, p.rect.edges()[3]).is_some_and(|b| (offset - b).abs() < 0.5);
                let exact = offset as f32 + st.slack.height();
                let (drawn, hold) = st.drawn.draw(exact, st.offset as f32, st.pinned(), s);
                dy -= drawn;
                snap.scroll(1, -drawn);
                if let Some(st) = self.scrolls.get_mut(&self.nodes[i].id) { (st.held, st.drawn) = (held, hold); }
            }
            let (mut scale, mut anchor, mut anchor_to, mut dx) = (pose.sc, pose.anchor, pose.anchor_to, pinh.dx);
            if n.scroll_axes.x() && let Some(st) = self.scrolls.get_mut(&self.nodes[i].id) {
                st.drawn_x = grid(st.x.shift() as f32, s);
                dx -= st.drawn_x;
                snap.scroll(0, -st.drawn_x);
            }
            if scope.open_cache && pose.sc > 1e-4 {
                dx += (pose.r_exact[0] / s - pose.rect_pt[0]) / pose.sc;
                dy += (pose.r_exact[1] / s - pose.rect_pt[1]) / pose.sc;
            }
            let (mut cover_offset, mut outer) = (pinh.cover, pinh.outer);
            let scale_motion = if pose.cover.is_some() { ScaleMotion::Moving } else { pose.scale_motion };
            if let Some((k, c)) = pose.cover {
                // Children ride this node's presented origin at their rest layout. Scale them by
                // `k` about the drawn origin, centred, and clip them to the moving rect.
                clip = intersect(clip, pose.r);
                hit_clip = intersect(hit_clip, pose.r);
                let plain = [dx, dy];
                dx = pose.rect_pt[0] - p.rect.x + c[0] / k;
                dy += pose.rect_pt[1] - p.rect.y - pinh.dy + c[1] / k;
                (scale, anchor, anchor_to) = (k, [pose.rect_pt[0], pose.rect_pt[1]], [pose.rect_pt[0], pose.rect_pt[1]]);
                cover_offset = [dx - plain[0], dy - plain[1]];
                outer = Some([pinh.clip, pinh.fade, pinh.fade_origin]);
            }
            inherit_fade(&mut hit_fade, &mut hit_origin, fade_origin, fade);
            let full_clip = intersect(hit_clip, clip);
            if pose.node_turn.is_some() {
                if self.unturned_drawn.is_empty() {
                    self.unturned_drawn.resize(n_nodes, ([0.0; 4], NO_CLIP));
                }
                self.unturned_drawn[i] = (pose.r.map(|v| v / s), full_clip);
            }
            self.inh_scratch[i] = Inh { opacity: pose.effective_opacity, paint_opacity: scope.o, first_fade: kids_fade,
                clip: full_clip, fade: hit_fade, fade_origin: hit_origin,
                paint_clip: clip, paint_fade: fade, paint_fade_origin: fade_origin, rounded: pose.rounded, scale, anchor, anchor_to, blur: pose.blur, dy, dx,
                cover: cover_offset, outer, mark: pose.mark, group: pose.group, pressed: pose.pressed, press_to: pose.press_to,
                hit_opacity: pose.hit_opacity * kids_fade, group_rect: if pose.hover_owner { pose.rect_pt } else { pinh.group_rect },
                scale_motion, still: if scale_motion == ScaleMotion::Still { (scale, anchor, anchor_to) } else { pinh.still },
                in_cache: scope.in_cache, skip_paint: scope.skip_paint, turn: pose.turn_index,
                snap: pixel_snap::Inherit { off: snap_off, steps: pose.steps, shifted: pinh.snap.shifted || n.vis.offset != [0.0; 2], layer: if pose.cover.is_some() { 0 } else { pose.snap_layer }, parent: pose.snap_rest.filter(|_| pose.cover.is_none() && snap.rigid == pinh.snap.rigid && pixel_snap::frames(&n.vis)), ..snap } };
            // Shown only under its group's hover, it takes presses where the pointer hovers the
            // group: a press in the same frame as the move that reveals it finds it, as a page's
            // `:hover` applies before the press does.
            let (hit_opacity, hit_rect) = if pose.effective_opacity > 0.001 { (pose.effective_opacity, pose.rect_pt) } else { (pose.hit_opacity, intersect(pose.rect_pt, pinh.group_rect)) };
            self.register_node_hits(i, pinh.clip, (hit_opacity, pose.visible), hit_rect, (pose.press_k, pose.press_anchor));
            if self.has_sticky {
                self.fade_below_header(i, s);
            }
            self.keep_painted(i, pose.own_turn.is_none() && !pose.opens_snap_layer && pose.snap_layer == 0 && !scope.in_cache && !scope.skip_paint && pose.cover.is_none() && !lift && !raised);
        }
        self.keep_close(None, &mut layers, &mut active_cache, &mut glyph_count);
        self.keep_end();
        self.close_all_layers(&mut layers, &mut active_cache, &mut glyph_count);
        // A moving chip can enter a stationary pointer. Re-evaluate against this frame's
        // geometry without inventing pointer-move or drag events on animation frames. Tooltips
        // follow the hold this decides.
        let queued = self.queue.len();
        let held = self.pointer.map(|(x, y)| self.refresh_hover(x, y));
        self.queue.observe_since(queued);
        self.update_help();
        self.layer_scratch = layers;
        self.built = true;
        self.land_unseen_entrances();
        self.built_ids.clear();
        self.built_ids.extend(self.nodes.iter().map(|n| n.id));
        if video_signature != self.video_signature {
            // A video loaded, ended or failed: views reading its state re-run next frame.
            self.video_signature = video_signature;
            self.dirty = true;
            self.videos_active = true;
        }
        if self.track_marks { self.marks.resize(self.instances.len(), 0); }
        self.stats.glyphs = glyph_count;
        self.ambient = ambient;
        // A live transform can change hover paint with no handler and no running spring.
        // Retarget from the new geometry and schedule the frame that samples that paint. A
        // commit that moved the indices left keyboard focus hover (`El::hover_group`) to
        // this pass too.
        let interaction = (self.pointer.is_some() || self.focus.keyboard()) && self.update_interaction(held);
        self.queue.len() > queued || interaction
    }
}
