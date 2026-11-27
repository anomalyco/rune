//! `El::cache` offscreen raster caching: subtree eligibility and state hashing before paint,
//! and bounds/signature sealing when a cached layer closes.
use super::*;
use crate::machine::Effects;

#[cfg(test)]
#[path = "cache_layer_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) enum ScaleSettle {
    Still(f32),
    Settling { scale: f32, since: f64 },
}

#[derive(Clone, Copy)]
pub(in crate::runtime) struct AnotherFrame;

impl ScaleSettle {
    /// Paint samples the animation clock, not a wall-clock deadline. Continue to
    /// request frames during the interval, including when the clock is slowed.
    #[inline]
    pub(in crate::runtime) fn step(&mut self, scale: f32, now: f64, reset: bool, epsilon: f32, poll: bool) -> Effects<AnotherFrame> {
        let (seen, since) = match *self {
            Self::Still(seen) => (seen, f64::NEG_INFINITY),
            Self::Settling { scale, since } => (scale, since),
        };
        // A real change is a strict comparison, as dev's was: a NaN or infinite scale
        // equals neither, so it never restarts the window.
        let changed = !reset && (scale - seen).abs() > epsilon;
        let since = if reset {
            *self = Self::Still(scale);
            f64::NEG_INFINITY
        } else if changed {
            *self = Self::Settling { scale, since: now };
            now
        } else {
            since
        };
        // Frames follow elapsed time since that change, so nothing compared per frame can
        // extend the window past `STATIC_SCALE_SETTLE`.
        if poll && now - since < STATIC_SCALE_SETTLE { Effects::One(AnotherFrame) } else { Effects::None }
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::runtime) enum RasterPhase {
    Live,
    Raster { paint_hash: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) enum RasterPass {
    Paint { glyph_start: usize },
    Elide { glyphs: usize },
}

/// One `MaterialDraw` recorded relative to a cached layer's origin, `raster_scale`, and outer
/// `effective_opacity`, replayed when the layer is reused (`RasterPass::Elide`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) struct CachedMaterial {
    pub(in crate::runtime) id: NodeId,
    pub(in crate::runtime) material: crate::SystemMaterial,
    pub(in crate::runtime) params: crate::MaterialParams,
    /// `[x - origin_x, y - origin_y, width, height]` at `raster_scale`.
    pub(in crate::runtime) rel_rect: [f32; 4],
    /// Clamped corner radius at `raster_scale`.
    pub(in crate::runtime) radius: f32,
    pub(in crate::runtime) continuous: bool,
    /// Opacity relative to the cached layer's outer `effective_opacity`.
    pub(in crate::runtime) rel_opacity: f32,
    /// Whether this material had a non-opaque authored fill eligible for Windows Acrylic visuals.
    #[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
    pub(in crate::runtime) windows_backdrop: bool,
}

#[derive(Clone, Debug)]
pub(in crate::runtime) struct NodeCache {
    pub(in crate::runtime) signature: u64,
    pub(in crate::runtime) raster_scale: f32,
    pub(in crate::runtime) scale: ScaleSettle,
    pub(in crate::runtime) env_scale: f32,
    pub(in crate::runtime) phase: RasterPhase,
    pub(in crate::runtime) rel_bounds: [f32; 4],
    pub(in crate::runtime) glyphs: usize,
    pub(in crate::runtime) commit_gen: u64,
    /// `El::system_material` draws recorded while painting this cached layer; empty (zero heap
    /// allocation) for cached subtrees with no material elements.
    pub(in crate::runtime) materials: Vec<CachedMaterial>,
    /// Visible material nodes inside this cached layer that overflowed `max_native_materials`.
    pub(in crate::runtime) material_fallbacks: Vec<NodeId>,
}

#[derive(Clone, Copy)]
pub(in crate::runtime) struct ActiveCache {
    pub(in crate::runtime) node_index: usize,
    pub(in crate::runtime) node_id: NodeId,
    pub(in crate::runtime) instance_start: usize,
    pub(in crate::runtime) origin: [f32; 2],
    pub(in crate::runtime) size: [f32; 2],
    pub(in crate::runtime) cur_scale: f32,
    pub(in crate::runtime) scale: ScaleSettle,
    pub(in crate::runtime) env_scale: f32,
    pub(in crate::runtime) effective_opacity: f32,
    pub(in crate::runtime) material_start: usize,
    pub(in crate::runtime) fallback_start: usize,
    /// Everything outside commits that paints the subtree: hover, press, links, an outside
    /// hover group and the window-wide state in `build`'s paint hash.
    pub(in crate::runtime) paint_hash: u64,
    pub(in crate::runtime) pass: RasterPass,
}

impl NodeCache {
    #[inline]
    pub(in crate::runtime) fn committed_scale(&mut self, scale: f32, now: f64) -> Option<AnotherFrame> {
        if (scale - self.raster_scale).abs() > 1e-4
            && matches!(self.scale.step(scale, now, false, 1e-4, true), Effects::One(_))
        {
            self.phase = RasterPhase::Live;
            Some(AnotherFrame)
        } else { None }
    }

    #[inline]
    pub(in crate::runtime) fn open(&self, eff_scale: f32, s: f32, settled: bool, now: f64, commit_gen: u64,
        is_resident: bool, scale_motion: ScaleMotion, paint_hash: u64, check: impl FnOnce() -> bool,
        glyph_start: usize) -> (ScaleSettle, RasterPass, Option<AnotherFrame>) {
        let env_same = (s - self.env_scale).abs() <= 1e-4 && !settled;
        let mut scale = self.scale;
        let scale_diff = (eff_scale - self.raster_scale).abs() > 1e-4;
        let same_commit = commit_gen == self.commit_gen;
        let eligible = same_commit && scale_diff && env_same && is_resident && matches!(self.phase, RasterPhase::Raster { .. });
        let pending = scale.step(eff_scale, now, !env_same, 1e-4, eligible);
        let scale_settling = eligible && (matches!(pending, Effects::One(_)) || scale_motion == ScaleMotion::Moving);
        // A commit can change any paint property. Never use a partial
        // model hash to skip checking the actual emitted draw output.
        let content_same = same_commit && (!scale_diff || scale_settling);
        let can_elide = is_resident
            && matches!(self.phase, RasterPhase::Raster { .. })
            && content_same
            && matches!(self.phase, RasterPhase::Raster { paint_hash: hash } if hash == paint_hash)
            && !check();
        let pass = if can_elide { RasterPass::Elide { glyphs: self.glyphs } } else { RasterPass::Paint { glyph_start } };
        (scale, pass, scale_settling.then_some(AnotherFrame))
    }
}

impl RuntimeCore {
    pub(in crate::runtime) fn prepare_cache_layers(&mut self, sel_ranges: &[(usize, usize, usize)], mark_ranges: &[(usize, usize, usize)]) -> u64 {
        let (s, n_nodes) = (self.env.scale, self.tree.nodes.len());
        self.paint.cache_live_scratch.clear();
        self.paint.cache_check_scratch.clear();
        self.paint.cache_paint_scratch.clear();
        self.paint.cache_inside_scratch.clear();
        let mut paint_state_hash = 0u64;
        if self.tree.has_cache {
            use std::hash::{Hash, Hasher};
            let mut h = rune_motion::Fx::default();
            self.pointer.hovered.hash(&mut h);
            self.pointer.hover_within.hash(&mut h);
            self.kbd.focus.path().hash(&mut h);
            self.kbd.focus.keyboard().hash(&mut h);
            self.kbd.focus.hovered().hash(&mut h);
            self.commit.defaults.force_focus_rings.hash(&mut h);
            if let Some(ring) = self.commit.defaults.focus_ring {
                let c = ring.color;
                [ring.width, ring.offset, c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut h);
            }
            (self.paint.srgb_blend, self.text.smoothing, self.text.families(), crate::text::icon::swap_id(), self.host.system_reduced_transparency).hash(&mut h);
            h.write_u32(s.to_bits());
            sel_ranges.hash(&mut h);
            self.caret_browse_focus_in(&self.tree.nodes)
                .map(|(fi, b, aff)| (fi, b, matches!(aff, parley::Affinity::Upstream)))
                .hash(&mut h);
            // A selection's colours follow the window's focus (`El::selection_style`).
            if !sel_ranges.is_empty() {
                self.host.window_focused.hash(&mut h);
            }
            mark_ranges.hash(&mut h);
            let mut scroll_mix = 0u64;
            for (&id, st) in &self.scroll.scrolls {
                let mut sh = rune_motion::Fx::default();
                sh.write_u64(id.0);
                sh.write_u64(st.offset.to_bits());
                sh.write_u64(st.x.offset.to_bits());
                sh.write_u32(st.held as u32);
                sh.write_u32(st.slack.height().to_bits());
                scroll_mix = scroll_mix.wrapping_add(sh.finish());
            }
            for (path, &off) in &self.edit.input_scroll {
                let mut sh = rune_motion::Fx::default();
                path.hash(&mut sh);
                sh.write_u32(off.to_bits());
                scroll_mix = scroll_mix.wrapping_add(sh.finish());
            }
            h.write_u64(scroll_mix);
            paint_state_hash = h.finish();

            self.paint.cache_live_scratch.resize(n_nodes, false);
            self.paint.cache_check_scratch.resize(n_nodes, false);
            self.paint.cache_paint_scratch.resize(n_nodes, 0);
            // Only cached subtrees read these, so skip everything outside them.
            self.paint.cache_inside_scratch.resize(n_nodes, false);
            for i in 0..n_nodes {
                let n = &self.tree.nodes[i];
                self.paint.cache_inside_scratch[i] = n.vis_flags.cache() || n.parent.is_some_and(|p| self.paint.cache_inside_scratch[p]);
            }
            let store = self.scene.store();
            for i in (0..n_nodes).rev() {
                if !self.paint.cache_inside_scratch[i] { continue; }
                let n = &self.tree.nodes[i];
                let live = store.is_node_animating(n.id)
                    // Sticky and anchored descendants move within a raster without motion tracks.
                    || self.sticky_of(i).is_some()
                    || n.above.is_some()
                    || (!self.reduced_motion && self.motion.loop_active(n.id, store.node_time(n.id)))
                    || (!self.reduced_motion
                        && self.text.has_animated_icons()
                        && (n.vis.icon.is_some_and(|(ic, _)| {
                            let mount_t = self.paint.icon_mount_time(n.id, ic as *const crate::text::Icon as usize, self.t);
                            self.text.icon_anim_live(ic, self.t, mount_t)
                        }) || n.text.as_ref().is_some_and(|t| {
                            t.cold.pills.iter().any(|p| {
                                p.icon.is_some_and(|ic| {
                                    let mount_t = self.paint.icon_mount_time(n.id, ic as *const crate::text::Icon as usize, self.t);
                                    self.text.icon_anim_live(ic, self.t, mount_t)
                                })
                            })
                        })))
                    || self.motion.grows.get(&n.id).is_some_and(|g| g.moving())
                    || self.scroll.scrolls.get(&n.id).is_some_and(|st| st.slack.height() > 0.0)
                    || self.motion.reveal_fading(n.id)
                    || self.subtree_exiting(i)
                    || (!self.motion.flying.is_empty() && self.motion.flying.iter().any(|f| f.0 == n.id))
                    || (!self.motion.flights().is_empty() && self.motion.flights().iter().any(|f| f.departs(&n.path, n.key.as_deref()) || f.lands(&n.path, n.key.as_deref())))
                    || n.vis.effect.is_some()
                    // A publication or animation frame can replace texels without moving the
                    // atlas slot, so identical image primitives do not imply identical raster contents.
                    || n.vis.pixels.is_some()
                    || n.vis.web_image.is_some()
                    || n.text.as_ref().is_some_and(|t| t.cold.pills.iter().any(crate::text::PillChip::has_image))
                    || n.vis.surface.is_some()
                    // Native material views (macOS, opt-in Windows) and Wayland compositor blur
                    // regions alter fills, punch shadow holes, and read live ancestor clips, so
                    // draw live while active; fallback-only materials stay cached and replay their
                    // recorded rects on elision.
                    || (n.material.is_some() && self.paint.tracks_native_materials())
                    // Never retain a raster of a secret, even when the field is unfocused.
                    || n.text.as_ref().is_some_and(|t| t.redacted)
                    || n.input.as_ref().is_some_and(|inp| inp.edit.focused || inp.edit.is_secure())
                    || n.text.as_ref().is_some_and(|t| t.cold.advance_in.is_some());
                let check = n.text.as_ref().is_some_and(|t| t.cold.live.is_some())
                    || n.live.iter().any(|b| !n.vis_flags.cache() || !matches!(b.property, crate::live::Property::Offset | crate::live::Property::Scale));
                self.paint.cache_live_scratch[i] |= live;
                self.paint.cache_check_scratch[i] |= check;
                // Hover, press and link paint land without a commit (an instant track or a
                // pointer move inside one text), so hash what this frame samples.
                let links = n.text.as_ref().is_some_and(|t| !t.cold.links.is_empty());
                if n.vis.tracks_hover_paint() || n.vis_flags.hover_group() || n.vis.tracks_press() || n.handlers.get().is_some_and(|h| h.press_scale.is_some() || h.press_offset.is_some()) || links {
                    let mut ph = rune_motion::Fx::default();
                    ph.write_u64(n.id.0);
                    for prop in [P_HOVER, P_PRESS, P_PRESS_SCALE, P_PRESS_OX, P_PRESS_OY] {
                        ph.write_u32(self.scene.get(n.id, prop).unwrap_or(0.0).to_bits());
                    }
                    if links { self.link_states(i).hash(&mut ph); }
                    self.paint.cache_paint_scratch[i] = self.paint.cache_paint_scratch[i].wrapping_add(ph.finish());
                }
                if let Some(p) = n.parent {
                    self.paint.cache_paint_scratch[p] = self.paint.cache_paint_scratch[p].wrapping_add(self.paint.cache_paint_scratch[i]);
                    // A lifted descendant draws outside this raster; don't elide its inputs.
                    self.paint.cache_live_scratch[p] |= self.paint.cache_live_scratch[i] || n.vis_flags.raise();
                    self.paint.cache_check_scratch[p] |= self.paint.cache_check_scratch[i] || !n.live.is_empty();
                }
            }
        }
        paint_state_hash
    }
}

impl state::PaintState {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::runtime) fn close_cached_layer(
        &mut self,
        nodes: &[Node],
        instances: &mut Vec<Instance>,
        surfaces: &mut [crate::native_surface::SurfaceDraw],
        marks: &mut Vec<u32>,
        track_marks: bool,
        text: &mut TextSystem,
        commit_gen: u64,
        ac: ActiveCache,
        start: usize,
        surface_start: usize,
        glyph_count: &mut usize,
    ) {
        use std::hash::Hasher;
        let layer_grow = (3.0 * crate::render::layer_blur::sigma(instances[start].params[2])).ceil();
        let glyph_start = match ac.pass {
            RasterPass::Paint { glyph_start } => glyph_start,
            RasterPass::Elide { glyphs } => {
                let entry = self.node_caches.get_mut(&ac.node_id).expect("elided cache entry");
                entry.scale = ac.scale;
                let k = if (ac.cur_scale - entry.raster_scale).abs() <= 1e-4 {
                    1.0
                } else {
                    ac.cur_scale / entry.raster_scale.max(1e-4)
                };
                let b0 = (ac.origin[0] + entry.rel_bounds[0] * k).floor();
                let b1 = (ac.origin[1] + entry.rel_bounds[1] * k).floor();
                let b2 = (ac.origin[0] + entry.rel_bounds[2] * k).ceil();
                let b3 = (ac.origin[1] + entry.rel_bounds[3] * k).ceil();
                let sig = entry.signature;
                // Replay recorded material draws translated to `ac.origin`, scaled by `k`, and
                // scaled by outer opacity so `Runtime::system_materials()` and `on_materials` stay
                // complete with zero heap allocations while reusing the cached raster.
                for m in &entry.materials {
                    let opacity = (m.rel_opacity * ac.effective_opacity).clamp(0.0, 1.0);
                    if opacity <= 0.001 {
                        continue;
                    }
                    if self.materials.len() < self.max_native_materials {
                        let replayed = crate::system_material::MaterialDraw {
                            id: m.id,
                            material: m.material,
                            rect: [
                                ac.origin[0] + m.rel_rect[0] * k,
                                ac.origin[1] + m.rel_rect[1] * k,
                                m.rel_rect[2] * k,
                                m.rel_rect[3] * k,
                            ],
                            radius: m.radius * k,
                            continuous: m.continuous,
                            opacity,
                            params: m.params,
                        };
                        self.materials.push(replayed);
                        #[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
                        if m.windows_backdrop {
                            self.windows_material_draws.push(replayed);
                        }
                    } else {
                        self.material_fallbacks.push(m.id);
                    }
                }
                for &id in &entry.material_fallbacks {
                    self.material_fallbacks.push(id);
                }
                Self::seal_cache_marker(instances, marks, track_marks, start, [b0, b1, (b2 - b0).max(0.0), (b3 - b1).max(0.0)], ac.node_id, sig);
                *glyph_count += glyphs;
                return;
            }
        };

        if instances.len() == start + 1 && surfaces.len() == surface_start {
            instances.pop();
            if track_marks {
                marks.truncate(instances.len());
            }
            self.evict_cache(text, ac.node_id);
            return;
        }

        let mut bounds = [ac.origin[0], ac.origin[1], ac.origin[0] + ac.size[0], ac.origin[1] + ac.size[1]];
        let mut h = rune_motion::Fx::default();
        h.write_u64(ac.node_id.0);
        // Blur is baked into the target, unlike opacity and clipping on the composite.
        h.write_u32(instances[start].params[2].to_bits());
        h.write_u8(self.srgb_blend as u8 | (text.smoothing as u8) << 1);
        h.write_u32(ac.cur_scale.to_bits());
        h.write_u32(ac.size[0].to_bits());
        h.write_u32(ac.size[1].to_bits());
        if let Some(key) = nodes[ac.node_index].vis.cache_key {
            h.write_u8(1);
            h.write_u64(key);
        } else {
            h.write_u8(0);
        }
        let q = |v: f32| (v * 64.0).round() as i64;
        let mut turn_stack: Vec<Option<Turn>> = Vec::new();
        for inst in &instances[start + 1..] {
            if inst.params[1] == crate::render::KIND_LAYER_END {
                turn_stack.pop();
            } else {
                let active_turn = turn_stack.last().copied().flatten();
                let (mut c0, mut c1, mut c2, mut c3) = if inst.params[1] == crate::render::KIND_LAYER {
                    let next = if inst.border[1] != 0.0 {
                        let own = Turn::about(inst.border[1], [inst.border[2], inst.border[3]], inst.rect, NO_CLIP);
                        Some(active_turn.map_or(own, |t| t.after(own)))
                    } else {
                        active_turn
                    };
                    turn_stack.push(next);
                    let g = (3.0 * crate::render::layer_blur::sigma(inst.params[2])).ceil();
                    let mut r = [inst.rect[0] - g, inst.rect[1] - g, inst.rect[0] + inst.rect[2] + g, inst.rect[1] + inst.rect[3] + g];
                    if inst.border[1] != 0.0 {
                        r = Turn::about(inst.border[1], [inst.border[2], inst.border[3]], inst.rect, NO_CLIP).bbox(r);
                    }
                    (
                        r[0].max(inst.clip[0]),
                        r[1].max(inst.clip[1]),
                        r[2].min(inst.clip[2]),
                        r[3].min(inst.clip[3]),
                    )
                } else {
                    let mut rect = inst.rect;
                    if inst.params[1] == KIND_GLYPH && inst.params[0] != 0.0 {
                        let (sin, cos) = inst.params[0].sin_cos();
                        let width = cos.abs() * rect[2] + sin.abs() * rect[3];
                        let height = sin.abs() * rect[2] + cos.abs() * rect[3];
                        rect = [rect[0] + (rect[2] - width) * 0.5, rect[1] + (rect[3] - height) * 0.5, width, height];
                    }
                    let grow = match inst.params[1] {
                        KIND_GLYPH => 0.0,
                        KIND_SHADOW => inst.params[2] * 3.0 + 1.0,
                        KIND_EFFECT => inst.params[2] + 1.0,
                        KIND_RECT | KIND_GRADIENT | KIND_DASHED | KIND_RADIAL => 1.0 + inst.params[2] * 2.0,
                        _ => 1.0,
                    };
                    (
                        (rect[0] - grow).max(inst.clip[0]),
                        (rect[1] - grow).max(inst.clip[1]),
                        (rect[0] + rect[2] + grow).min(inst.clip[2]),
                        (rect[1] + rect[3] + grow).min(inst.clip[3]),
                    )
                };
                if c2 > c0 && c3 > c1 {
                    if let Some(t) = active_turn {
                        [c0, c1, c2, c3] = t.bbox([c0, c1, c2, c3]);
                    }
                    bounds[0] = bounds[0].min(c0);
                    bounds[1] = bounds[1].min(c1);
                    bounds[2] = bounds[2].max(c2);
                    bounds[3] = bounds[3].max(c3);
                }
            }
            h.write_i64(q(inst.rect[0] - ac.origin[0]));
            h.write_i64(q(inst.rect[1] - ac.origin[1]));
            h.write_i64(q(inst.rect[2]));
            h.write_i64(q(inst.rect[3]));
            for r in inst.radii { h.write_u32(r.to_bits()); }
            for f in inst.fill { h.write_u32(f.to_bits()); }
            for b in inst.border { h.write_u32(b.to_bits()); }
            for p in inst.params { h.write_u32(p.to_bits()); }
            for u in inst.uv { h.write_u32(u.to_bits()); }
            for k in 0..4 {
                let cv = inst.clip[k];
                if cv.abs() < 1e6 {
                    h.write_i64(q(cv - ac.origin[k & 1]));
                } else {
                    h.write_u32(cv.to_bits());
                }
            }
            for fd in inst.fade { h.write_u32(fd); }
        }
        let b0 = (bounds[0] - layer_grow).floor();
        let b1 = (bounds[1] - layer_grow).floor();
        let b2 = (bounds[2] + layer_grow).ceil();
        let b3 = (bounds[3] + layer_grow).ceil();
        let bw = (b2 - b0).max(0.0);
        let bh = (b3 - b1).max(0.0);
        if bw > 4096.0 || bh > 4096.0 {
            self.evict_cache(text, ac.node_id);
            crate::render::end_layer(instances, surfaces, marks, start, surface_start);
            return;
        }
        let mut sig = h.finish();
        if sig == 0 { sig = 1; }
        let rel_bounds = [b0 - ac.origin[0], b1 - ac.origin[1], b2 - ac.origin[0], b3 - ac.origin[1]];
        let glyphs = *glyph_count - glyph_start;
        let is_resident = text.atlas.resident_caches.get(&ac.node_id.0) == Some(&sig);
        let (mut cached_materials, mut cached_fallbacks) = match self.node_caches.get_mut(&ac.node_id) {
            Some(prev) => (
                std::mem::take(&mut prev.materials),
                std::mem::take(&mut prev.material_fallbacks),
            ),
            None => (Vec::new(), Vec::new()),
        };
        cached_materials.clear();
        cached_fallbacks.clear();
        let inv_opacity = if ac.effective_opacity > 1e-6 { 1.0 / ac.effective_opacity } else { 0.0 };
        for draw in &self.materials[ac.material_start..] {
            cached_materials.push(CachedMaterial {
                id: draw.id,
                material: draw.material,
                params: draw.params,
                rel_rect: [
                    draw.rect[0] - ac.origin[0],
                    draw.rect[1] - ac.origin[1],
                    draw.rect[2],
                    draw.rect[3],
                ],
                radius: draw.radius,
                continuous: draw.continuous,
                rel_opacity: (draw.opacity * inv_opacity).clamp(0.0, 1.0),
                #[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
                windows_backdrop: self.windows_material_draws.iter().any(|w| w.id == draw.id),
            });
        }
        cached_fallbacks.extend_from_slice(&self.material_fallbacks[ac.fallback_start..]);
        self.node_caches.insert(ac.node_id, NodeCache {
            signature: sig,
            raster_scale: ac.cur_scale,
            scale: ac.scale,
            env_scale: ac.env_scale,
            phase: RasterPhase::Raster { paint_hash: ac.paint_hash },
            rel_bounds,
            glyphs,
            commit_gen,
            materials: cached_materials,
            material_fallbacks: cached_fallbacks,
        });
        if is_resident && surfaces.len() == surface_start {
            instances.truncate(start + 1);
        }
        Self::seal_cache_marker(instances, marks, track_marks, start, [b0, b1, bw, bh], ac.node_id, sig);
    }

    /// Finishes a cached layer's marker at `rect`, naming its cache entry and signature, and
    /// closes the layer.
    fn seal_cache_marker(instances: &mut Vec<Instance>, marks: &mut Vec<u32>, track_marks: bool, start: usize, rect: [f32; 4], id: NodeId, sig: u64) {
        let marker = &mut instances[start];
        marker.rect = rect;
        marker.params[3] = 1.0;
        marker.radii = crate::render::pack_u64_f32(id.0);
        marker.uv = crate::render::pack_u64_f32(sig);
        instances.push(Instance::end_layer());
        if track_marks {
            let mark = marks.get(start).copied().unwrap_or(0);
            marks.resize(instances.len(), mark);
        }
    }

    fn evict_cache(&mut self, text: &mut TextSystem, id: NodeId) {
        self.node_caches.remove(&id);
        text.atlas.resident_caches.remove(&id.0);
    }
}
