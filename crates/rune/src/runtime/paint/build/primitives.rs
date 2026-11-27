//! Per-node GPU primitive emission: native materials, outer and inset shadows, fills, gradients,
//! video/native surfaces, borders, effect slots, focus rings/outlines and leaf content.
use super::*;

impl RuntimeCore {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_node_box(
        &mut self,
        i: usize,
        s: f32,
        now: f64,
        pinh: &Inh,
        pcold: &InhCold,
        r: [f32; 4],
        r_exact: [f32; 4],
        raw_edges: [f32; 4],
        radii: [f32; 4],
        w: f32,
        h: f32,
        sc: f32,
        o: f32,
        effective_opacity: f32,
        base_opacity: f32,
        draw: f32,
        m: f32,
        clip: [f32; 4],
        fade: [f32; 4],
        fade_origin: [f32; 4],
        vp: [f32; 4],
        pressed: f32,
        press_to: Option<Color>,
        loop_value: Option<f32>,
        blur_hole: Option<f32>,
        cover: Option<(f32, [f32; 2])>,
        scale_motion: ScaleMotion,
        snap_rest: Option<&pixel_snap::Parent>,
        next: Option<usize>,
        default_ring: Option<usize>,
        primitive_start: usize,
        mark: u32,
        isolate: bool,
        in_cache: bool,
        skip_paint: bool,
        loop_vis: bool,
        gliding: bool,
        turn_index: u32,
        caret: Option<[f32; 4]>,
        video_clock: crate::video::Clock,
        sel_ranges: &[(usize, usize, usize)],
        mark_ranges: &[(usize, usize, usize)],
        video_signature: &mut u64,
        glyph_count: &mut usize,
    ) -> (Option<usize>, Option<usize>) {
        let turned = turn_index != 0;
        let n = &self.tree.nodes[i];
        let mut faded_slot = None;
        // Disabled styling is distributed to each authored paint leaf at build time.
        // Keep this local to emission: it neither propagates to children nor opens scopes.
        let o = o * n.vis.paint_alpha;
        let srgb = self.paint.srgb_blend;
        // Subtree blur runs once after rasterization, never on each primitive as well.
        let blur = 0.0;
        let k_px = sc * s;
        let (native_material, wayland_candidate, material_draw) = match n.material {
            // A dimmed-out element takes no native view: its fallback draws nothing and does not
            // count as a cap fallback.
            Some((material, params)) if effective_opacity * n.vis.paint_alpha > 0.001 => {
                if self.paint.materials.len() < self.paint.max_native_materials {
                    let radius = crate::system_material::clamp_radius(radii, w, h);
                    let continuous = n.vis_flags.continuous_corners();
                    let material_opacity = effective_opacity * n.vis.paint_alpha;
                    let draw = crate::system_material::MaterialDraw {
                        id: n.id, material, rect: [r[0], r[1], w, h], radius, continuous,
                        opacity: material_opacity, params,
                    };
                    self.paint.materials.push(draw);
                    let hole_r = if continuous { -radius } else { radius };
                    let native = self.paint.native_materials.then_some(hole_r);
                    let wayland = (!self.paint.native_materials && self.paint.wayland_blur_materials)
                        .then_some((radius, hole_r, material_opacity));
                    (native, wayland, Some((draw, hole_r)))
                } else {
                    self.paint.material_fallbacks.push(n.id);
                    (None, None, None)
                }
            }
            _ => (None, None, None),
        };
        #[cfg(not(any(test, all(target_os = "windows", feature = "windows-system-material"))))]
        let _ = material_draw;
        let stuck_k = self.stuck_shadow_alpha(i);
        let stuck_shadows: (f32, &[crate::el::Shadow]) = match &n.sticky {
            Some(s) if stuck_k > 0.0 => (stuck_k, &s.shadows),
            _ => (0.0, &[]),
        };
        // `El::focus_within_flat`: focus inside drops its own shadows and hover fill.
        let focus_flat = n.focus.flat_within && self.focus_within(i);
        let hov_v = if !focus_flat && (n.vis.tracks_hover_paint() || self.scene.store().is_animating(n.id, P_HOVER_C)) {
            self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let prs_v = if n.vis.tracks_press() || self.scene.store().is_animating(n.id, P_PRESS_C) {
            self.scene.get(n.id, P_PRESS).unwrap_or(0.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut wayland_blur_hole = None;
        let mut windows_blur_hole = None;
        #[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
        let windows_candidate = (!self.paint.native_materials
            && !self.paint.wayland_blur_materials
            && self.paint.windows_acrylic_materials)
            .then_some(material_draw)
            .flatten();
        #[cfg(not(any(test, all(target_os = "windows", feature = "windows-system-material"))))]
        let windows_candidate: Option<(crate::system_material::MaterialDraw, f32)> = None;
        let (resolved_fill, fill_replace_w) = if native_material.is_some() {
            (Color::TRANSPARENT, 0.0)
        } else if let Some((radius, hole_r, material_opacity)) = wayland_candidate {
            let rest_fill = self.color(n.id, P_FILL, n.vis.fill);
            let (resolved, w_rep) = paint_tracks::replace_color(
                rest_fill, if focus_flat { None } else { n.vis.hover_fill }, hov_v, n.vis.press_fill, prs_v,
            );
            if resolved.a * o < 1.0 {
                // Translucent tint or missing fill: punch the outer-shadow interior hole and
                // add to Wayland's blur region while opacity stays at or above the threshold.
                wayland_blur_hole = Some(hole_r);
                if material_opacity >= crate::system_material::WAYLAND_BLUR_MIN_OPACITY {
                    self.paint.wayland_material_blurs.push(
                        crate::system_material::WaylandMaterialBlur {
                            rect: [r[0], r[1], w, h], radius, clip, opacity: material_opacity,
                        },
                    );
                }
            }
            (resolved, w_rep)
        } else if let Some((draw, hole_r)) = windows_candidate {
            #[cfg(not(any(test, all(target_os = "windows", feature = "windows-system-material"))))]
            let _ = draw;
            let rest_fill = self.color(n.id, P_FILL, n.vis.fill);
            let (resolved, w_rep) = paint_tracks::replace_color(
                rest_fill, if focus_flat { None } else { n.vis.hover_fill }, hov_v, n.vis.press_fill, prs_v,
            );
            if resolved.a * o < 1.0 {
                // Wayland-aligned fill rules for Windows 11 Acrylic: opaque fills stay opaque and
                // unblurred; translucent fills tint above Acrylic; empty fills paint transparent so
                // the Acrylic brush supplies the theme-aware tint.
                windows_blur_hole = Some(hole_r);
                #[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
                self.paint.windows_material_draws.push(draw);
            }
            (resolved, w_rep)
        } else {
            #[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
            if let Some((draw, _)) = material_draw {
                // Even before Windows Acrylic activates (Frame 1 or while effects are off), record
                // non-opaque material elements so opaque-only windows stay Idle with zero DWM/WinRT
                // calls while non-opaque materials can activate on Frame 1 or when effects return.
                let authored = self.color(n.id, P_FILL, n.vis.fill);
                let (authored_resolved, _) = paint_tracks::replace_color(
                    authored, if focus_flat { None } else { n.vis.hover_fill }, hov_v, n.vis.press_fill, prs_v,
                );
                if authored_resolved.a * o < 1.0 {
                    self.paint.windows_material_draws.push(draw);
                }
            }
            let rest_fill = match n.material {
                Some(_) => self.color(n.id, P_FILL, crate::system_material::fallback_fill(n.vis.fill, self.env.system_dark)),
                None => self.color(n.id, P_FILL, n.vis.fill),
            };
            paint_tracks::replace_color(rest_fill, if focus_flat { None } else { n.vis.hover_fill }, hov_v, n.vis.press_fill, prs_v)
        };
        let casts = !n.vis.shadows.is_empty() && native_material.is_none() && wayland_blur_hole.is_none() && windows_blur_hole.is_none() && blur_hole.is_none() && n.material.is_none();
        let fill_a = if casts { resolved_fill.a } else { 0.0 };
        let solid = casts && fill_a >= 1.0;
        // A box that paints itself solid hides the middle of its shadows (bands.rs). Dimmed
        // (`El::dim`), or filled with a translucent colour, it leaves that middle clear, as
        // CSS clips box-shadow to outside the box: a 6% white button shows the surface
        // under it, not its own shadows.
        let opaque = solid && o >= 1.0 && n.vis.gradient.is_none() && n.vis.radial.is_none() && n.vis.surface.is_none();
        let see_through = casts && fill_a > 0.0 && fill_a < 1.0;
        let dim_hole = ((solid && o < 1.0) || see_through).then(|| crate::system_material::clamp_radius(radii, w, h) * if n.vis_flags.continuous_corners() { -1.0 } else { 1.0 });
        let cast = shadow_paint::Caster {
            rect: [r[0], r[1], w, h], radii, shape: if n.vis_flags.continuous_corners() { radii.map(|v| -v) } else { radii },
            k_px, scale: s, opacity: o, srgb, clip, fade, hole: native_material.or(wayland_blur_hole).or(windows_blur_hole).or(blur_hole).or(dim_hole), opaque,
        };
        for (k, sh) in n.vis.shadows.iter().chain(stuck_shadows.1).enumerate() {
            let c = if k >= n.vis.shadows.len() { sh.color.alpha(sh.color.a * stuck_shadows.0) }
                else if k < P_SHADOWS.len() { self.color(n.id, P_SHADOWS[k], sh.color) } else { sh.color };
            if c.a <= 0.0 || sh.inset || (focus_flat && k < n.vis.shadows.len()) {
                continue;
            }
            let sh = &if k < n.vis.shadows.len() { self.shadow_at(n.id, k, sh) } else { *sh };
            shadow_paint::push_shadow(&mut self.instances, sh, c, &cast);
        }
        // Shadows keep circular corners; the element's own shapes take the sign, and
        // negative radii draw continuous corners (shader.wgsl `sd_rounded`).
        let radii = if n.vis_flags.continuous_corners() { radii.map(|v| -v) } else { radii };
        let mut overlay = [0.0f32; 4];
        if let Some(c) = n.focus.highlight && self.highlight_visible(i) {
            overlay = over(premul(c, 1.0, srgb), overlay);
        }
        if let Some(c) = n.focus.fill && self.focus_ring_visible(i) {
            overlay = over(premul(c, 1.0, srgb), overlay);
        }
        if !focus_flat && (n.vis.hover().a > 0.0 || self.scene.store().is_animating(n.id, P_HOVER_C)) && hov_v > 0.0 {
            overlay = over(premul(self.color(n.id, P_HOVER_C, n.vis.hover()), hov_v, srgb), overlay);
        }
        if (n.vis.press().a > 0.0 || self.scene.store().is_animating(n.id, P_PRESS_C)) && prs_v > 0.0 {
            overlay = over(premul(self.color(n.id, P_PRESS_C, n.vis.press()), prs_v, srgb), overlay);
        }
        let layered = n.vis.gradient.is_some() || n.vis.radial.is_some() || n.vis.surface.is_some();
        let base_color = |c: Color| blend::flat_base(c, srgb, !layered && n.vis.border_width() == 0.0 && c.a * o < 1.0);
        let base = if native_material.is_some() { [0.0; 4] } else { base_color(resolved_fill) };
        let fill = if layered { base } else { over(overlay, base) }.map(|c| c * o);
        let bw = pixel_snap::line_width(n.vis.border_width() * s) * sc;
        let focus_border = n.focus.border.filter(|_| self.focus_ring_visible(i));
        let hover_border = if focus_flat { None } else { n.vis.hover_border };
        let press_border = n.vis.press_border;
        let (resolved_border, border_replace_w) = if let Some(fb) = focus_border {
            (fb, 1.0)
        } else if bw > 0.0 {
            paint_tracks::replace_color(self.color(n.id, P_BORDER, n.vis.border()), hover_border, hov_v, press_border, prs_v)
        } else {
            (Color::TRANSPARENT, 0.0)
        };
        let border = if bw > 0.0 { premul(resolved_border, o, srgb) } else { [0.0; 4] };
        // A gradient border draws as KIND_GRADIENT with a width: `border` top, `uv` bottom.
        // After a switch back to `border`, it stays a gradient until both stops meet.
        let store = self.scene.store();
        let springing = store.has(n.id, P_BORDER_BOTTOM)
            && (store.is_animating(n.id, P_BORDER_BOTTOM) || store.is_animating(n.id, P_BORDER));
        let border_bottom = (bw > 0.0 && focus_border.is_none() && (n.vis.border_bottom.is_some() || springing))
            .then(|| {
                let rest_bot = self.color(n.id, P_BORDER_BOTTOM, n.vis.border_bottom.unwrap_or(n.vis.border()));
                let (resolved_bot, _) = paint_tracks::replace_color(rest_bot, hover_border, hov_v, press_border, prs_v);
                premul(resolved_bot, o, srgb)
            })
            .filter(|b| (n.vis.border_bottom.is_some() && border_replace_w < 1.0) || *b != border);
        let (border_kind, border_uv) = border_bottom.map_or((KIND_RECT, [0.0; 4]), |b| (KIND_GRADIENT, b));
        let border_a = border[3].max(border_uv[3]);
        // A box with zero width or height has no area to fill or border; emitting it lets
        // `sd_rounded` shade a one-pixel line at up to half strength on fractional device pixels.
        let has_area = w > 0.0 && h > 0.0;
        // A dashed ring is its own instance over the fill, which then draws without a ring.
        let dashed = n.vis.border_dash.filter(|_| has_area && bw > 0.0 && border_a > 0.0005).map(|[dash, gap]| Instance {
            rect: [r[0], r[1], w, h],
            radii,
            fill: border,
            border: border_bottom.unwrap_or(border),
            params: [bw, KIND_DASHED, blur * s, 1.0],
            uv: [dash * k_px, gap * k_px, n.vis.border_dash_phase * k_px, 0.0],
            clip,
            fade: crate::render::pack_fade(fade, clip, clip),
        });
        // Per-side widths (`El::border_sides`), each snapped as `bw` is; the widest is `bw`.
        let sides = n.vis.border_sides.filter(|_| bw > 0.0).map(|ws| ws.map(|v| pixel_snap::line_width(v * s) * sc));
        let padding = sides.unwrap_or([bw; 4]);
        let (bw, border, border_kind, border_uv, border_a) =
            if n.vis.border_dash.is_some() { (0.0, [0.0; 4], KIND_RECT, [0.0; 4], 0.0) } else { (bw, border, border_kind, border_uv, border_a) };
        // The border's own instance, `o` in `params.w` so the shader knows how much it
        // hides the fill under it (shader.wgsl `border_over`). With inset shadows it draws
        // apart, over them, as CSS layers an inset box-shadow between background and border.
        let ring = Instance {
            rect: [r[0], r[1], w, h],
            radii,
            fill: [0.0; 4],
            border,
            params: [bw, border_kind, blur * s, o],
            uv: if border_kind == KIND_RECT { sides.unwrap_or([0.0; 4]) } else { border_uv },
            clip,
            fade: crate::render::pack_fade(fade, clip, clip),
        };
        let apart = has_area && bw > 0.0 && border_a > 0.0005 && n.vis.shadows.iter().chain(stuck_shadows.1).any(|sh| sh.inset);
        // Dimmed, an opaque fill and its gradient draw as one opaque gradient, so they fade
        // together as a layer at that opacity would (CSS `opacity`), not one through the other.
        let flat = layered && o < 1.0 && base[3] >= 1.0 && n.vis.gradient.is_some();
        if layered {
            // Fill, gradient and overlays first, then the border on top, as in CSS.
            if has_area && fill[3] > 0.0005 && !flat {
                self.instances.push(Instance {
                    rect: [r[0], r[1], w, h],
                    radii,
                    fill,
                    border: [0.0; 4],
                    params: [0.0, KIND_RECT, blur * s, 1.0],
                    uv: [0.0; 4],
                    clip,
                    fade: crate::render::pack_fade(fade, clip, clip),
                });
            }
            let grad_keep = (1.0 - fill_replace_w).clamp(0.0, 1.0);
            let g = n.vis.gradient.as_deref().copied().unwrap_or_default();
            let stop = |c| if flat { over(premul(c, grad_keep, srgb), base).map(|v| v * o) } else { premul(c, o * grad_keep, srgb) };
            let top = stop(self.color(n.id, P_GRADIENT[0], g[0]));
            let bottom = stop(self.color(n.id, P_GRADIENT[1], g[1]));
            if has_area && (top[3] > 0.0005 || bottom[3] > 0.0005) {
                self.instances.push(Instance {
                    rect: [r[0], r[1], w, h],
                    radii,
                    fill: top,
                    border: [0.0; 4],
                    params: [0.0, KIND_GRADIENT, 0.0, 1.0],
                    uv: bottom,
                    clip,
                    fade: crate::render::pack_fade(fade, clip, clip),
                });
            }
            if has_area && grad_keep > 0.0005 && let Some(rg) = &n.vis.radial {
                let segs = rg.stops.len().saturating_sub(1);
                let rmax = radii[0].max(radii[1]).max(radii[2]).max(radii[3]).min(w * 0.5).min(h * 0.5);
                let pad = 1.0 + blur * s * 2.0;
                for seg in 0..segs {
                    let c0 = rg.stops[seg].1;
                    let c0 = if seg < P_RADIAL.len() { self.color(n.id, P_RADIAL[seg], c0) } else { c0 };
                    let c1 = rg.stops[seg + 1].1;
                    let c1 = if seg + 1 < P_RADIAL.len() { self.color(n.id, P_RADIAL[seg + 1], c1) } else { c1 };
                    let c0 = premul(c0, o * grad_keep, srgb);
                    let c1 = premul(c1, o * grad_keep, srgb);
                    if c0[3] <= 0.0005 && c1[3] <= 0.0005 {
                        continue;
                    }
                    let t0_val = rg.stops[seg].0;
                    let t1_val = rg.stops[seg + 1].0;
                    let (seg_rect, seg_radii, uv) = match rg.geometry {
                        crate::el::GradientGeometry::Radial { center, radius } => {
                            let cx = center[0] * w;
                            let cy = center[1] * h;
                            let rx = (radius[0] * k_px).max(1e-3);
                            let ry = (radius[1] * k_px).max(1e-3);
                            if seg + 1 < segs {
                                let mut lx0 = (cx - t1_val * rx - pad).floor().max(0.0);
                                let mut ly0 = (cy - t1_val * ry - pad).floor().max(0.0);
                                let mut lx1 = (cx + t1_val * rx + pad).ceil().min(w);
                                let mut ly1 = (cy + t1_val * ry + pad).ceil().min(h);
                                if rmax > 0.0 {
                                    if lx0 < rmax && (ly0 < rmax || ly1 > h - rmax) { lx0 = 0.0; }
                                    if lx1 > w - rmax && (ly0 < rmax || ly1 > h - rmax) { lx1 = w; }
                                    if ly0 < rmax && (lx0 < rmax || lx1 > w - rmax) { ly0 = 0.0; }
                                    if ly1 > h - rmax && (lx0 < rmax || lx1 > w - rmax) { ly1 = h; }
                                    if lx0 == 0.0 && (ly0 == 0.0 || ly1 == h) { lx1 = lx1.max((2.0 * rmax).min(w)); }
                                    if lx1 == w && (ly0 == 0.0 || ly1 == h) { lx0 = lx0.min((w - 2.0 * rmax).max(0.0)); }
                                    if ly0 == 0.0 && (lx0 == 0.0 || lx1 == w) { ly1 = ly1.max((2.0 * rmax).min(h)); }
                                    if ly1 == h && (lx0 == 0.0 || lx1 == w) { ly0 = ly0.min((h - 2.0 * rmax).max(0.0)); }
                                }
                                if lx1 <= lx0 || ly1 <= ly0 {
                                    continue;
                                }
                                let br = [
                                    if lx0 == 0.0 && ly0 == 0.0 { radii[0] } else { 0.0 },
                                    if lx1 == w && ly0 == 0.0 { radii[1] } else { 0.0 },
                                    if lx1 == w && ly1 == h { radii[2] } else { 0.0 },
                                    if lx0 == 0.0 && ly1 == h { radii[3] } else { 0.0 },
                                ];
                                ([r[0] + lx0, r[1] + ly0, lx1 - lx0, ly1 - ly0], br, [cx - lx0, cy - ly0, rx, ry])
                            } else {
                                ([r[0], r[1], w, h], radii, [cx, cy, rx, ry])
                            }
                        }
                        crate::el::GradientGeometry::Linear { angle } => {
                            let (sin, cos) = angle.sin_cos();
                            let (dx, dy) = (sin, -cos);
                            let span = (dx.abs() * w + dy.abs() * h).max(1e-3);
                            ([r[0], r[1], w, h], radii, [dx / span, dy / span, 0.0, 0.0])
                        }
                    };
                    let t0 = t0_val + if seg > 0 { 2.0 } else { 0.0 };
                    let t1 = t1_val + if seg + 1 < segs { 2.0 } else { 0.0 };
                    self.instances.push(Instance {
                        rect: seg_rect,
                        radii: seg_radii,
                        fill: c0,
                        border: c1,
                        params: [t0, KIND_RADIAL, blur * s, t1],
                        uv,
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            }
            if let Some((surface, fit)) = &n.vis.surface {
                if let Some(pulled) = surface.pull_video(video_clock, self.frame.live.waker()) {
                    // A clip whose next picture isn't due for a while wakes the window
                    // then, instead of drawing every refresh in between.
                    let wait = pulled.due.map(|due| due.saturating_duration_since(crate::time::now()).as_secs_f64());
                    match wait {
                        Some(wait) if pulled.active && wait > crate::video::WAKE_MIN => {
                            let wake = self.clock + wait;
                            self.paint.keep.cycles.other_deadline();
                            self.motion.arm_loop_wake(wake, now + wait * self.time_scale as f64);
                        }
                        _ => self.paint.videos_active |= pulled.active,
                    }
                    *video_signature = crate::video::fold(*video_signature, pulled.generation, surface.key());
                }
                self.surfaces.push(crate::native_surface::SurfaceDraw {
                    surface: surface.placed(n.vis.surface_place), fit: *fit, before_instance: self.instances.len() as u32,
                    rect: [r[0], r[1], w, h], radii, clip, fade: crate::render::pack_fade(fade, fade_origin, clip), opacity: o,
                });
            }
            let ov = overlay.map(|c| c * o);
            if apart && ov[3] > 0.0005 {
                let under = border_paint::fill_under(Instance { fill: ov, ..ring }, false);
                if under.rect[2] > 0.0 && under.rect[3] > 0.0 { self.instances.push(under); }
            } else if has_area && !apart && (ov[3] > 0.0005 || border_a > 0.0005) {
                self.instances.push(Instance { fill: ov, ..ring });
            }
        } else if apart {
            if fill[3] > 0.0005 {
                let under = border_paint::fill_under(Instance { fill, ..ring }, true);
                if under.rect[2] > 0.0 && under.rect[3] > 0.0 { self.instances.push(under); }
            }
        } else if has_area && (fill[3] > 0.0005 || border_a > 0.0005) {
            if fill == [0.0; 4] && border_kind == KIND_RECT && bw > 0.0 && sides.is_none() {
                bands::push_ring(&mut self.instances, ring);
            } else {
                if let Some(rune_motion::LoopKind::OpacityCycle(curve)) = n.looping.as_ref().map(|l| l.kind)
                    && n.a11y.hidden && !next.is_some_and(|j| self.tree.nodes[j].parent == Some(i))
                    && !isolate && !in_cache && !skip_paint && loop_vis && base_opacity > 0.0
                    && pinh.paint_opacity > 0.0 && loop_value.is_some_and(|v| v > 0.0) && n.vis.layer_opacity == 1.0
                    && n.vis.paint_alpha == 1.0 && bw == 0.0 && overlay == [0.0; 4]
                    && !n.vis.tracks_hover_paint() && !n.vis.tracks_press()
                    && self.instances.len() == primitive_start && n.text.is_none() && n.input.is_none()
                    && n.vis.shape.is_none() && n.vis.pixels.is_none() && n.vis.web_image.is_none() && n.vis.effect.is_none()
                    && n.vis.shadows.is_empty() && n.handlers.get().is_none() && n.focus.get().is_none()
                    && !n.vis_flags.hit() && !turned && cover.is_none() && n.help.is_none()
                    && curve.stops.iter().all(|&(_, a)| a.is_finite() && a > 0.001 && a <= 1.0 && base[3] * a > 0.0005)
                {
                    self.paint.keep.cycles.record(i, self.instances.len(), curve, (base, o / loop_value.unwrap_or(1.0)), ring.rect);
                }
                self.instances.push(Instance { fill, ..ring });
            }
        }
        // Inset shadows draw inside the border (CSS's padding box), under it.
        let (inner, inner_radii) = border_paint::inset_box([r[0], r[1], w, h], radii, padding);
        if inner[2] > 0.0 && inner[3] > 0.0 {
            for (k, sh) in n.vis.shadows.iter().chain(stuck_shadows.1).enumerate() {
                if !sh.inset || (focus_flat && k < n.vis.shadows.len()) {
                    continue;
                }
                let c = if k >= n.vis.shadows.len() { sh.color.alpha(sh.color.a * stuck_shadows.0) }
                    else if k < P_SHADOWS.len() { self.color(n.id, P_SHADOWS[k], sh.color) } else { sh.color };
                if c.a <= 0.0 {
                    continue;
                }
                let sh = &if k < n.vis.shadows.len() { self.shadow_at(n.id, k, sh) } else { *sh };
                self.instances.push(border_paint::inset_shadow(sh, premul(c, o, srgb), (inner, inner_radii), (k_px, s), clip, fade));
            }
        }
        if apart {
            if sides.is_none() && border_kind == KIND_RECT { bands::push_ring(&mut self.instances, ring); } else { self.instances.push(ring); }
        }
        self.instances.extend(dashed);
        if let Some(effect) = n.vis.effect {
            let fade = crate::render::pack_fade(fade, clip, clip);
            faded_slot = paint_slots::push_effect(&mut self.instances, (effect, mark), ([r[0], r[1], w, h], radii, clip, fade), k_px, o);
        }
        let focus_ring = if default_ring == Some(i) || self.forced_default_ring(i) { Some(self.default_focus_ring()) }
            else if let Some(key) = n.focus.ring_for.as_deref() {
                n.focus.ring.filter(|_| self.kbd.focus.keyboard() && self.kbd.focus.path().is_some_and(|p| p.rsplit('/').next().is_some_and(|seg| element_path::segment_matches_key(seg, key))))
            }
            else if n.focus.within { n.focus.ring.filter(|_| self.focus_within(i) && (!n.focus.within_keyboard || self.kbd.focus.entered_by_keys())) }
            else { self.focus_ring_visible(i).then_some(n.focus.ring).flatten() };
        if let Some(ol) = focus_ring.or(n.vis.outline) {
            let c = if focus_ring.is_some() { ol.color } else { self.color(n.id, P_OUTLINE, ol.color) };
            let ow = pixel_snap::line_width(ol.width * s) * sc;
            let g = pixel_snap::line_width(ol.offset * s) * sc + ow;
            let ring_radii = match (focus_ring.is_some(), n.focus.ring_radius) {
                (true, Some(rr)) => {
                    let rr = rr * s * sc;
                    [if n.vis_flags.continuous_corners() && rr > 0.0 { -rr } else { rr }; 4]
                }
                _ => radii.map(|v| if v != 0.0 { v + g.copysign(v) } else { 0.0 }),
            };
            if c.a > 0.0 && ol.width > 0.0 {
                self.instances.push(Instance {
                    rect: [r[0] - g, r[1] - g, w + 2.0 * g, h + 2.0 * g],
                    radii: ring_radii,
                    fill: [0.0; 4],
                    border: premul(c, o, srgb),
                    params: [ow, KIND_RECT, 0.0, 1.0],
                    uv: [0.0; 4],
                    clip,
                    fade: crate::render::pack_fade(fade, clip, clip),
                });
            }
        }
        let content_clip = if n.collapsing && n.vis.clip.is_none() && n.scroll.is_none() {
            intersect(clip, r)
        } else {
            clip
        };
        let has_web_image = n.vis.cold.get().is_some_and(|c| c.web_image.is_some());
        let web_border_inset = if border_a > 0.0005 || dashed.is_some() { padding } else { [0.0; 4] };
        let text_start = self.paint_content(
            i,
            paint_content::LeafBox {
                r,
                r_exact,
                // Its exact resting place against its snapped one, inside its containment's
                // own rounding (`LeafBox::exact_off`).
                // Under an offset (`El::offset`, a translation as CSS transforms are) it moves
                // with its box, as it did before this: a glide toward a moving offset keeps
                // the text on its card, and lands as a card that never moved draws.
                exact_off: if pinh.snap_shifted() || n.vis.offset != [0.0; 2] { [0.0; 2] } else {
                    let o = snap_rest.map_or([0.0; 2], |p| p.exact_off(pcold.snap_steady.map(|v| v * s)));
                    let c = pcold.snap_contain.map(|v| v * s);
                    [o[0] + (c[0] + 0.5).floor() - c[0], o[1] + (c[1] + 0.5).floor() - c[1]]
                },
                raw_size: [raw_edges[2] - raw_edges[0], raw_edges[3] - raw_edges[1]],
                contain: pcold.snap_contain.map(|v| v * s),
                radii,
                sc,
                o,
                draw,
                clip: content_clip,
                fade,
                group: pinh.group,
                pressed,
                press_to,
                loop_value,
                cover,
                scale_motion,
                rest: pcold.still.0,
                gliding,
                caret,
                shown: if !turned && !pinh.in_cache() { [content_clip[1].max(vp[1] - m), content_clip[3].min(vp[3] + m)] } else { [content_clip[1], content_clip[3]] },
                shown_x: if !turned && !pinh.in_cache() { [content_clip[0].max(vp[0] - m), content_clip[2].min(vp[2] + m)] } else { [content_clip[0], content_clip[2]] },
            },
            sel_ranges,
            mark_ranges,
            glyph_count,
        );
        if has_web_image {
            self.record_web_image(i, r, radii, web_border_inset, effective_opacity, pinh.clip, content_clip, turn_index);
        }
        (faded_slot, text_start)
    }
}

#[cfg(test)]
#[path = "primitives_tests.rs"]
mod tests;
