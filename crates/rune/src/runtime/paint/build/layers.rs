//! Paint layer scoping: dissolve pre-pass, tree-order check, layer stack closing, raised ancestor
//! reopening, `El::cache` eligibility/opening, rounded clips, backdrop blur, cutouts and isolation.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct LayerScope {
    pub(super) o: f32,
    pub(super) skip_paint: bool,
    pub(super) in_cache: bool,
    pub(super) open_cache: bool,
    pub(super) blur_hole: Option<f32>,
    pub(super) glyphs_clear: bool,
    pub(super) hit_clip: [f32; 4],
    pub(super) hit_fade: [f32; 4],
    pub(super) hit_origin: [f32; 4],
    pub(super) primitive_start: usize,
    pub(super) surface_start: usize,
}

#[inline(always)]
fn strip_round_mask(clip: &mut [f32; 4], round: &mut Option<([f32; 4], [f32; 4])>) {
    if let Some((mask, radii)) = *round {
        let e = radii.map(crate::render::corner_extent);
        let (left, right, top, bottom) = (e[0].max(e[3]), e[1].max(e[2]), e[0].max(e[1]), e[2].max(e[3]));
        let cut = intersect(*clip, mask);
        if cut[2] <= cut[0] || cut[3] <= cut[1]
            || (cut[0] >= mask[0] + left && cut[2] <= mask[2] - right)
            || (cut[1] >= mask[1] + top && cut[3] <= mask[3] - bottom)
        {
            *clip = cut;
            *round = None;
        }
    }
}

impl paint_state::PaintState {
    // Phases of `build`'s single paint loop; keep them inlined into it.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_dissolve(
        &mut self,
        tree: &types::TreeState,
        scene: &Scene,
        motion: &crate::runtime::tracks::MotionState,
        clock: f64,
        reduced_motion: bool,
        focus_keyboard: bool,
        focus_path: Option<&str>,
    ) {
        let n_nodes = tree.nodes.len();
        // One linear pass, including retained exits, Live opacity, presence and loops. Reuse
        // inherited scratch for hover groups so activation samples exactly the paint values.
        self.dissolve_scratch.clear();
        if tree.has_dissolve { self.dissolve_scratch.resize(n_nodes, false); }
        for index in 0..if tree.has_dissolve { n_nodes } else { 0 } {
            let i = self.layer_paint_order.get(index).copied().unwrap_or(index);
            let n = &tree.nodes[i];
            let inherited_group = n.parent.map_or(0.0, |p| self.inh_scratch[p].group);
            self.inh_scratch[i].group = if n.vis.hover().a > 0.0 || n.vis_flags.hover_group() {
                scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0)
            } else { inherited_group };
            let Some(parent) = n.parent.filter(|&p| tree.nodes[p].vis_flags.blend() == crate::render::Blend::PlusLighter) else { continue };
            let Some(p) = scene.presented(n.id) else { continue };
            let mut opacity = p.opacity * n.vis.layer_opacity;
            opacity *= group_factor(scene, focus_keyboard, focus_path, n, inherited_group);
            if fades(n.looping.as_deref().copied()) && let Some(track) = motion.loop_track(n.id) {
                opacity *= track.sample_at(scene.store().node_time(n.id), clock, reduced_motion);
            }
            self.dissolve_scratch[parent] |= opacity > 0.001 && opacity < 1.0;
        }
    }

    #[inline(always)]
    pub(super) fn needs_tree_order(&self, tree: &types::TreeState, scene: &Scene, flying: bool) -> bool {
        // Exiting descendants are appended to storage. Bring them back into their subtree
        // only when isolation requires it; the ordinary painter stream stays byte-identical.
        // Matched flights paint last (see `update_layer_paint_order`).
        ((flying || tree.has_sticky || tree.has_raise || tree.has_paint_order || !self.lifted_anchors.is_empty()) && !self.layer_paint_order.is_empty()) || self.layer_paint_order.windows(2).any(|pair| {
            let n = &tree.nodes[pair[0]];
            tree.nodes[pair[1]].parent == Some(pair[0]) && ((tree.has_dissolve && self.dissolve_scratch[pair[0]]) || n.vis_flags.cache() || n.vis.clip_radius.is_some() || n.vis.group_opacity.is_some() || n.group_fade
                || n.vis.layer_opacity < 1.0 || fades(n.looping.as_deref().copied())
                || scene.presented(n.id).is_some_and(|p| (p.opacity > 0.0 && p.opacity < 1.0) || p.blur > 0.0 || p.transform.rotate.abs() > 1e-6))
        })
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn close_all_layers(
        &mut self,
        nodes: &[Node],
        instances: &mut Vec<Instance>,
        surfaces: &mut [crate::native_surface::SurfaceDraw],
        marks: &mut Vec<u32>,
        track_marks: bool,
        text: &mut TextSystem,
        commit_gen: u64,
        layers: &mut Vec<(usize, usize, usize)>,
        active_cache: &mut Option<ActiveCache>,
        glyph_count: &mut usize,
    ) {
        while let Some((_, start, surface_start)) = layers.pop() {
            if active_cache.is_some_and(|ac| ac.instance_start == start) {
                let ac = active_cache.take().unwrap();
                self.close_cached_layer(nodes, instances, surfaces, marks, track_marks, text, commit_gen, ac, start, surface_start, glyph_count);
            } else {
                crate::render::end_layer(instances, surfaces, marks, start, surface_start);
            }
        }
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn close_layers_to_parent(
        &mut self,
        nodes: &[Node],
        instances: &mut Vec<Instance>,
        surfaces: &mut [crate::native_surface::SurfaceDraw],
        marks: &mut Vec<u32>,
        track_marks: bool,
        text: &mut TextSystem,
        commit_gen: u64,
        node_parent: Option<usize>,
        layers: &mut Vec<(usize, usize, usize)>,
        active_cache: &mut Option<ActiveCache>,
        glyph_count: &mut usize,
    ) {
        while let Some(&(ancestor, start, surface_start)) = layers.last() {
            let mut parent = node_parent;
            while parent.is_some_and(|p| p != ancestor) { parent = nodes[parent.unwrap()].parent; }
            if parent == Some(ancestor) { break; }
            if active_cache.is_some_and(|ac| ac.instance_start == start) {
                let ac = active_cache.take().unwrap();
                self.close_cached_layer(nodes, instances, surfaces, marks, track_marks, text, commit_gen, ac, start, surface_start, glyph_count);
            } else {
                crate::render::end_layer(instances, surfaces, marks, start, surface_start);
            }
            layers.pop();
        }
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn reopen_raised_layers(
        &self,
        nodes: &[Node],
        scene: &Scene,
        instances: &mut Vec<Instance>,
        surfaces_len: usize,
        node_parent: Option<usize>,
        pinh: &mut Inh,
        layers: &mut Vec<(usize, usize, usize)>,
        s: f32,
    ) {
        // Out of its ancestors' layers: their opacity applies directly instead.
        pinh.paint_opacity = pinh.opacity;
        pinh.paint_clip = pinh.clip;
        pinh.paint_fade = pinh.fade;
        pinh.paint_fade_origin = pinh.fade_origin;
        pinh.in_cache = false;
        pinh.skip_paint = false;
        // Rounded clips and turns consumed by an ancestor's layer must be reapplied
        // outside that layer too. Reopen only the masks and turns, not opacity or blur.
        pinh.rounded = None;
        let mut chain = Vec::new();
        let mut parent = node_parent;
        while let Some(p) = parent {
            chain.push(p);
            parent = nodes[p].parent;
        }
        for &p in chain.iter().rev() {
            let ancestor = &nodes[p];
            let k = self.inh_scratch[p].turn;
            if k != 0 && ancestor.parent.map_or(0, |pp| self.inh_scratch[pp].turn) != k {
                let t = self.turn_scratch[k as usize - 1];
                let start = instances.len();
                let marker = Instance::begin_layer(t.rect, t.outer_clip, Default::default()).turned(t.angle, t.pivot);
                instances.push(marker);
                layers.push((p, start, surfaces_len));
            }
            if let Some(radius) = ancestor.vis.clip_radius {
                let [x, y, w, h] = self.unturned_drawn.get(p).filter(|_| k != 0)
                    .map_or_else(|| self.drawn[p].map(|v| v * s), |&(b, _)| [b[0] * s, b[1] * s, (b[2] - b[0]) * s, (b[3] - b[1]) * s]);
                let radii = ancestor.vis.clip_corners.unwrap_or_else(|| {
                    if ancestor.vis.radii() != [0.0; 4] {
                        scene.presented(ancestor.id).map_or(ancestor.vis.radii(), |p| p.radius.0)
                    } else { [radius; 4] }
                }).map(|radius| {
                    let radius = (radius.max(0.0) * self.inh_scratch[p].scale * s).min(w.min(h) * 0.5);
                    if ancestor.vis_flags.continuous_corners() { -radius } else { radius }
                });
                let start = instances.len();
                let mut marker = Instance::begin_layer([x, y, w, h], [x, y, x + w, y + h], Default::default());
                marker.round_clip_corners(radii);
                instances.push(marker);
                layers.push((p, start, surfaces_len));
            }
        }
    }
}

impl RuntimeCore {

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_node_layers(
        &mut self,
        i: usize,
        s: f32,
        now: f64,
        p: &rune_motion::Presented,
        pinh: &Inh,
        r: [f32; 4],
        r_exact: [f32; 4],
        radii: [f32; 4],
        w: f32,
        h: f32,
        sc: f32,
        m: f32,
        effective_opacity: f32,
        paint_own: f32,
        visible: bool,
        dissolve: bool,
        lift: bool,
        own_turn: Option<f32>,
        node_turn: Option<&Turn>,
        next: Option<usize>,
        scale_motion: ScaleMotion,
        vp: [f32; 4],
        paint_state_hash: u64,
        glyph_count: usize,
        clip: &mut [f32; 4],
        fade: &mut [f32; 4],
        fade_origin: &mut [f32; 4],
        rounded: &mut Option<([f32; 4], [f32; 4])>,
        isolate: &mut bool,
        backdrop_blurs: &mut usize,
        layers: &mut Vec<(usize, usize, usize)>,
        active_cache: &mut Option<ActiveCache>,
    ) -> LayerScope {
        let n = &self.tree.nodes[i];
        if n.vis_flags.cache() && self.paint.cache_live_scratch[i] {
            if let Some(entry) = self.paint.node_caches.get_mut(&n.id) {
                entry.phase = RasterPhase::Live;
            }
        }
        let mut skip_paint = pinh.skip_paint;
        let mut in_cache = pinh.in_cache;
        let mut open_cache = false;
        // A scale changed by view commits may accompany arbitrary content edits. Draw
        // live until it rests instead of trusting a partial content hash or allocating a
        // new raster on each commit. Live-only transforms still composite the old raster.
        let mut committed_scale_moving = false;
        if n.vis_flags.cache() && let Some(entry) = self.paint.node_caches.get_mut(&n.id)
            && entry.commit_gen != self.commit.commit_gen()
            && let Some(frame) = entry.committed_scale(sc * s, now)
        {
            committed_scale_moving = true;
            self.paint.scale_frame = Some(frame);
        }
        if n.vis_flags.cache() && active_cache.is_none() && !lift && !skip_paint && !self.paint.cache_live_scratch[i]
            && !committed_scale_moving && own_turn.is_none()
            && effective_opacity > 0.001 && w > 0.0 && h > 0.0
        {
            let eff_scale = sc * s;
            let est_bounds = if let Some(entry) = self.paint.node_caches.get(&n.id) {
                let k = eff_scale / entry.raster_scale.max(1e-4);
                [
                    r[0] + entry.rel_bounds[0] * k,
                    r[1] + entry.rel_bounds[1] * k,
                    r[0] + entry.rel_bounds[2] * k,
                    r[1] + entry.rel_bounds[3] * k,
                ]
            } else {
                let sh = n.vis.shadows.iter().enumerate().fold(0.0f32, |m, (k, sh)| {
                    let sh = self.shadow_at(n.id, k, sh);
                    m.max((sh.x.abs() + sh.y.abs() + sh.spread.max(0.0) + sh.blur * 3.0) * eff_scale)
                });
                [r[0] - sh, r[1] - sh, r[2] + sh, r[3] + sh]
            };
            let cache_vis = match node_turn {
                None => est_bounds[2] >= clip[0].max(vp[0]) - m
                    && est_bounds[0] <= clip[2].min(vp[2]) + m
                    && est_bounds[3] >= clip[1].max(vp[1]) - m
                    && est_bounds[1] <= clip[3].min(vp[3]) + m,
                Some(t) => {
                    let lc = intersect(pinh.clip, *clip);
                    let sb = t.bbox(est_bounds);
                    let oc = t.outer_clip;
                    est_bounds[2] >= lc[0] - m && est_bounds[0] <= lc[2] + m
                        && est_bounds[3] >= lc[1] - m && est_bounds[1] <= lc[3] + m
                        && sb[2] >= oc[0].max(vp[0]) - m && sb[0] <= oc[2].min(vp[2]) + m
                        && sb[3] >= oc[1].max(vp[1]) - m && sb[1] <= oc[3].min(vp[3]) + m
                }
            };
            if !cache_vis {
                skip_paint = true;
            } else {
                let est_w = (est_bounds[2] - est_bounds[0]).ceil();
                let est_h = (est_bounds[3] - est_bounds[1]).ceil();
                if est_w <= 4096.0 && est_h <= 4096.0 {
                    open_cache = true;
                }
            }
        }
        // A negative clip radius rounds with continuous corners (`render::corner_extent`).
        let mut own_round = n.vis.clip_corners.or_else(|| {
            n.vis.clip_radius.map(|radius| {
                if n.vis.radii() != [0.0; 4] {
                    p.radius.0
                } else {
                    [radius; 4]
                }
            })
        }).map(|radii| {
            let half = w.min(h) * 0.5;
            let radii = radii.map(|radius| {
                let r = (radius.max(0.0) * sc * s).min(half);
                if n.vis_flags.continuous_corners() && r > 0.0 { -r } else { r }
            });
            (r, radii)
        });
        let mut hit_clip = intersect(pinh.clip, *clip);
        if let Some((rect, _)) = own_round { hit_clip = intersect(hit_clip, rect); }
        let (mut hit_fade, mut hit_origin) = (pinh.fade, pinh.fade_origin);
        inherit_fade(&mut hit_fade, &mut hit_origin, *fade_origin, *fade);
        // A rectangle wholly in either central strip misses all four corner squares.
        // Its hard edges already exclude everything the rounded mask would remove.
        strip_round_mask(clip, rounded);
        if !open_cache {
            strip_round_mask(clip, &mut own_round);
        }
        // A backdrop blur samples what is drawn before it, so it goes ahead of any layer this
        // element opens (a fade still blurs the window) and of its shadows, which skip it.
        let blur_hole = (n.vis.backdrop_blur > 0.0 && !skip_paint && visible && *backdrop_blurs < crate::MAX_BACKDROP_BLURS
            && effective_opacity > 0.001 && paint_own * pinh.paint_opacity * n.vis.paint_alpha > 0.001)
            .then(|| paint_slots::push_backdrop_blur(&mut self.instances, n.vis.backdrop_blur, ([r[0], r[1], w, h], radii, n.vis_flags.continuous_corners()),
                (*clip, *fade, *fade_origin), *rounded, sc * s, (paint_own * pinh.paint_opacity * n.vis.paint_alpha).clamp(0.0, 1.0)));
        *backdrop_blurs += blur_hole.is_some() as usize;
        // The packed ABI carries one clip rectangle. Preserve a rectangular cut through
        // a rounded corner on a separate outer scope, never recenter the rounded mask.
        if !skip_paint && let Some((mask, _)) = rounded.or(own_round)
            && intersect(*clip, mask) != mask {
            let mut marker = Instance::begin_layer([r[0], r[1], w, h], *clip, Default::default());
            marker.fade = crate::render::pack_fade(*fade, *fade_origin, *clip);
            layers.push((i, self.instances.len(), self.surfaces.len()));
            self.instances.push(marker);
            *clip = NO_CLIP; *fade = [0.0; 4]; *fade_origin = NO_CLIP;
        }
        // Two rounded rectangles cannot generally be represented by one rounded rectangle.
        // Consume the ancestor on an outer composite, retaining both original corner centres.
        if !skip_paint && rounded.is_some() && own_round.is_some() && *rounded != own_round {
            let (mask, radii) = rounded.take().unwrap();
            let mut marker = Instance::begin_layer([r[0], r[1], w, h], mask, Default::default());
            marker.fade = crate::render::pack_fade(*fade, *fade_origin, mask);
            marker.round_clip_corners(radii);
            layers.push((i, self.instances.len(), self.surfaces.len()));
            self.instances.push(marker);
            *clip = NO_CLIP; *fade = [0.0; 4]; *fade_origin = NO_CLIP;
        }
        if let Some(mask) = own_round { *rounded = Some(mask); }
        if let Some((mask, _)) = *rounded {
            *clip = intersect(*clip, mask);
        }
        // `El::cutout_circle`: the subtree draws in a layer whose composite cuts the hole,
        // after the clips from above, which it takes.
        if !skip_paint && let Some(c) = n.vis.cutout {
            let k = sc * s;
            let mut marker = Instance::begin_layer([r[0], r[1], w, h], *clip, Default::default());
            marker.fade = crate::render::pack_fade(*fade, *fade_origin, *clip);
            if let Some((_, radii)) = *rounded { marker.round_clip_corners(radii); }
            marker.radii = [r[0] + c[0] * k, r[1] + c[1] * k, c[2] * k, c[3] * k];
            layers.push((i, self.instances.len(), self.surfaces.len()));
            self.instances.push(marker);
            *clip = NO_CLIP; *fade = [0.0; 4]; *fade_origin = NO_CLIP; *rounded = None;
        }
        // Live/truncated text adds a glyph-only rectangle below. Consume the inherited
        // rounded mask first so the text clip cannot move its corner centres, unless that
        // rectangle sits clear of the corners and the glyphs can skip the mask.
        let glyphs_clear = rounded.is_some_and(|(mask, radii)| {
            self.text_clip(i, (r, r_exact), sc, *clip).is_some_and(|(text, ..)| inside_rounded(text, mask, radii))
        });
        *isolate |= open_cache || (rounded.is_some() && (n.text.as_ref().is_some_and(|t| (t.truncate_fade || t.cold.live.is_some()) && !glyphs_clear)
            || n.input.as_ref().is_some_and(|i| i.single_line)));
        // A soft clip fades each primitive on its own, so where one paints over another (an
        // image over its placeholder fill, a label over its bubble) the one beneath shows
        // through the fade, and the stack brightens or darkens towards the edge. A stack that
        // reaches into a soft edge composites as one layer, which takes the fade once.
        if !*isolate && visible && effective_opacity > 0.001 && fade.iter().any(|&f| f != 0.0)
            && in_fade([r[0], r[1], r[0] + w, r[1] + h], *clip, *fade, *fade_origin)
        {
            let paints = own_paints(n);
            let parent = next.is_some_and(|j| self.tree.nodes[j].parent == Some(i));
            *isolate = paints >= 2 || (paints == 1 && parent);
        }
        let o = if *isolate { 1.0 } else { (paint_own * pinh.paint_opacity).clamp(0.0, 1.0) };
        if !skip_paint && *isolate {
            let start_idx = self.instances.len();
            layers.push((i, start_idx, self.surfaces.len()));
            let mut marker = Instance::begin_layer([r[0], r[1], w, h], *clip,
                crate::render::LayerSpec { opacity: paint_own * pinh.paint_opacity,
                    blend: if dissolve { crate::render::Blend::PlusLighter } else { crate::render::Blend::SourceOver },
                    blur: p.blur * sc * s, clip_radius: 0.0 });
            marker.fade = crate::render::pack_fade(*fade, *fade_origin, *clip);
            if let Some((_, radii)) = *rounded { marker.round_clip_corners(radii); }
            self.instances.push(marker);
            // Clip the completed layer, never its inputs (including off-wall blur taps).
            *clip = NO_CLIP; *fade = [0.0; 4]; *fade_origin = NO_CLIP; *rounded = None;
            if open_cache {
                in_cache = true;
                let eff_scale = sc * s;
                let paint_hash = {
                    use std::hash::Hasher;
                    let mut h = rune_motion::Fx::default();
                    h.write_u64(paint_state_hash);
                    h.write_u64(self.paint.cache_paint_scratch[i]);
                    h.write_u32(pinh.group.to_bits());
                    h.finish()
                };
                let (scale, pass, frame) =
                    if let Some(entry) = self.paint.node_caches.get(&n.id) {
                        let is_resident = self.text.atlas.resident_caches.get(&n.id.0) == Some(&entry.signature);
                        entry.open(eff_scale, s, self.settled, now, self.commit.commit_gen(), is_resident,
                            scale_motion, paint_hash, || self.paint.cache_check_scratch[i], glyph_count)
                    } else {
                        (ScaleSettle::Still(eff_scale), RasterPass::Paint { glyph_start: glyph_count }, None)
                    };
                if frame.is_some() { self.paint.scale_frame = frame; }
                if matches!(pass, RasterPass::Elide { .. }) {
                    skip_paint = true;
                }
                *active_cache = Some(ActiveCache {
                    node_index: i,
                    node_id: n.id,
                    instance_start: start_idx,
                    origin: [r[0], r[1]],
                    size: [w, h],
                    cur_scale: eff_scale,
                    scale,
                    env_scale: s,
                    paint_hash,
                    pass,
                });
            }
        }
        let primitive_start = self.instances.len();
        let surface_start = self.surfaces.len();
        // Include native surface geometry, which lives beside the primitive stream.
        if !skip_paint && n.vis.surface.is_some() {
            for &(_, at, _) in layers.iter() {
                let b = &mut self.instances[at].rect;
                let hi = [(b[0] + b[2]).max(r[2].min(clip[2])), (b[1] + b[3]).max(r[3].min(clip[3]))];
                b[0] = b[0].min(r[0].max(clip[0])); b[1] = b[1].min(r[1].max(clip[1]));
                b[2] = hi[0] - b[0]; b[3] = hi[1] - b[1];
            }
        }
        LayerScope {
            o,
            skip_paint,
            in_cache,
            open_cache,
            blur_hole,
            glyphs_clear,
            hit_clip,
            hit_fade,
            hit_origin,
            primitive_start,
            surface_start,
        }
    }
}
