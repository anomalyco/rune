//! Leaf content painting inside a box: vector shapes, images, icons and rich text.
use super::*;

pub(in crate::runtime) struct LeafBox {
    pub r: [f32; 4],
    pub r_exact: [f32; 4],
    /// Where its exact box rests against where its snapped box rests (device px), its paint
    /// containment's rounding in: Blink's text lays out from the exact place.
    pub exact_off: [f32; 2],
    pub raw_size: [f32; 2],
    /// The paint containment's origin above (`El::paint_contained`), device px: what draws
    /// from its exact box's origin rounds against it, as Blink rounds inside it.
    pub contain: [f32; 2],
    pub radii: [f32; 4],
    pub sc: f32,
    pub o: f32,
    pub draw: f32,
    pub clip: [f32; 4],
    pub fade: [f32; 4],
    pub group: f32,
    /// The nearest `El::press_color` (here or above) and how far a press on it is held.
    pub pressed: f32,
    pub press_to: Option<Color>,
    pub loop_value: Option<f32>,
    pub cover: Option<(f32, [f32; 2])>,
    pub scale_motion: ScaleMotion,
    /// The scale of the nearest still ancestor, which moving text rasterizes at.
    pub rest: f32,
    pub gliding: bool,
    pub caret: Option<[f32; 4]>,
    /// The rows of the screen, in pixels, where what it draws can show: its clip's, cut to the
    /// window's unless it draws into a layer turned or cached first. A long text draws its
    /// lines there and no others.
    pub shown: [f32; 2],
    /// The horizontal columns of the screen, in pixels, matching `shown`.
    pub shown_x: [f32; 2],
}

/// `v` (device px) on the device grid inside a paint containment at `c`: the containment's
/// origin and the offset in it each round half up, as Blink rounds them.
fn on_grid(v: f32, c: f32) -> f32 {
    let round = |x: f32| (x + 0.5).floor();
    round(v - c) + round(c)
}

impl RuntimeCore {
    pub(in crate::runtime) fn paint_content(
        &mut self,
        i: usize,
        b: LeafBox,
        sel_ranges: &[(usize, usize, usize)],
        mark_ranges: &[(usize, usize, usize)],
        glyph_count: &mut usize,
    ) {
        let s = self.env.scale;
        let srgb = self.paint.srgb_blend;
        let now = self.t;
        let LeafBox {
            r,
            r_exact,
            exact_off,
            raw_size,
            contain,
            radii,
            sc,
            o,
            draw,
            clip,
            fade,
            group,
            pressed,
            press_to,
            loop_value,
            cover,
            scale_motion,
            rest,
            gliding,
            caret: input_caret,
            shown,
            shown_x,
        } = b;
        let (w, h) = ((r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0));
        let (w_exact, h_exact) = ((r_exact[2] - r_exact[0]).max(0.0), (r_exact[3] - r_exact[1]).max(0.0));
        let k_px = sc * s;
        let link_fx = self.tree.nodes[i].text.is_some().then(|| self.link_fx(i));
        let node_id = self.tree.nodes[i].id;
        let text_color = self.tree.nodes[i].vis.icon.map(|(_, c)| c)
            .or_else(|| self.tree.nodes[i].text.as_ref().map(|sp| sp.color))
            .map(|c| self.color(node_id, P_TEXT, c));
        // A terminal's cell grid paints its rows itself (`grid.rs`).
        if let Some(g) = self.tree.nodes[i].vis.grid.clone() {
            self.paint.paint_grid(node_id, self.tree.nodes[i].flow, s, &g, r, r_exact, clip, fade, o, glyph_count, &mut self.text, &mut self.images, &mut self.instances);
        }
        self.fill_visible_by_line(i, r, exact_off, sc, cover, scale_motion, rest, gliding, shown);
        let n = &self.tree.nodes[i];
        let text_tint = |c: Color| paint_tracks::press_tint(paint_tracks::group_tint(n, c, group), press_to, pressed);
        if let Some(shape) = &n.vis.shape {
            // At rest a shape draws from its exact box's origin rounded to the device grid, at its
            // exact scale, as Chromium draws a canvas or an inline SVG: what it holds keeps its
            // geometry against the grid, so a sheet's 1 px header line covers the same device rows
            // (one whole and half the next at 1.5x) wherever the sheet sits. Moving or scaled, it
            // stays centred in its snapped box, sharing the centre fills, borders and icons use, so
            // a glide or a swap to an icon doesn't wobble. A view-boxed shape stretches to the box.
            let still = cover.is_none() && scale_motion == ScaleMotion::Still && !gliding && (sc - 1.0).abs() < 1e-5;
            let at = if still {
                [on_grid(r_exact[0], contain[0]), on_grid(r_exact[1], contain[1])]
            } else {
                [r[0] + (w - w_exact) * 0.5, r[1] + (h - h_exact) * 0.5]
            };
            let (origin, k) = match shape.view_box {
                Some([vw, vh]) => ([r[0], r[1]], [w / vw, h / vh]),
                None => (at, [k_px, k_px]),
            };
            if let Some(t) = self.scene.get(node_id, P_TRIM) {
                // A presence draw traces the trimmed span on from its start.
                let end = t.x + (t.y - t.x) * draw;
                push_shape(&mut self.instances, &shape.trimmed(t.x, end), origin, k, k_px, o, srgb, clip, fade);
            } else if draw < 1.0 {
                push_shape(&mut self.instances, &shape.drawn(draw), origin, k, k_px, o, srgb, clip, fade);
            } else {
                push_shape(&mut self.instances, shape, origin, k, k_px, o, srgb, clip, fade);
            }
        }
        if w > 0.0 && h > 0.0 && let Some(pixels) = &n.vis.pixels {
            let snap_u = if cover.is_none() { (1.0 - (sc - 1.0).abs() * 32.0).clamp(0.0, 1.0) } else { 0.0 };
            let (rect, crop) = crate::pixels::fit_in(n.vis.pixels_fit, n.vis.surface_place, [r[0], r[1], w, h], pixels.size(), snap_u);
            let span = [crop[2] - crop[0], crop[3] - crop[1]];
            if rect[2] > 0.0 && rect[3] > 0.0 && span[0] > 0.0 && span[1] > 0.0 {
                // The mip covers the whole image's drawn size, not just its visible window.
                if let Some((image, next)) = self.images.place_at(pixels, [rect[2] / span[0], rect[3] / span[1]], Some(now)) {
                    if let Some(at) = next {
                        arm_image_wake(&mut self.paint, &mut self.motion, self.frame.live.visible(), self.clock, self.time_scale, now, at);
                    }
                    let [x, y, iw, ih] = image.rect.map(|v| v as f32);
                    // A letterboxed image rounds its own, smaller corners.
                    let half = rect[2].min(rect[3]) * 0.5;
                    self.instances.push(Instance {
                        rect,
                        radii: if n.vis.pixels_fit.is_some() { radii.map(|v| v.min(half)) } else { radii },
                        fill: [0.0, 0.0, 0.0, o],
                        border: [0.0; 4],
                        params: [image.page as f32, KIND_IMAGE, image.premultiplied as u8 as f32, 1.0],
                        // The shader samples from a whole-texel origin; the span may be fractional.
                        uv: [x + (crop[0] * iw).round(), y + (crop[1] * ih).round(), span[0] * iw, span[1] * ih],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            }
        }
        if let Some((ic, _)) = n.vis.icon {
            let color = text_tint(text_color.unwrap());
            let looped = if matches!(n.looping.as_ref().map(|l| l.kind), Some(rune_motion::LoopKind::Rotation)) { loop_value.unwrap_or(0.0) } else { 0.0 };
            let turn = looped + if self.scene.store().has(n.id, P_ICON_TURN) {
                self.scene.get(n.id, P_ICON_TURN).unwrap_or(n.vis.icon_turn)
            } else { n.vis.icon_turn };
            // `El::mirror_in_rtl`: the drawing mirrors (its turn already runs the other way).
            let facing = match (n.flow, n.vis.mirror_rtl) {
                (crate::el::Flow::Rtl, true) => crate::text::Facing::Mirrored,
                _ => crate::text::Facing::Upright,
            };
            let tk = if cover.is_some() { 1.0 } else { sc };
            let raster = if cover.is_some() { 1.0 } else { scale_motion.raster(sc, rest) };
            if cover.is_none() && scale_motion == ScaleMotion::Live {
                self.paint.scale_frame = Some(AnotherFrame);
            }
            let zoom = tk / raster.max(1e-4);
            let exact_side = w_exact.min(h_exact);
            let raw_side = raw_size[0].min(raw_size[1]).max(0.0) * s;
            let snap_u = if cover.is_none() && (raw_side - raw_side.round()).abs() < 1.0 / 512.0 {
                (1.0 - (sc - 1.0).abs() * 32.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let drawn = exact_side + (w.min(h) - exact_side) * snap_u;
            let px = if cover.is_none() && scale_motion != ScaleMotion::Still {
                (raw_side * raster).round().max(1.0) as u32
            } else {
                (drawn / zoom.max(1e-4)).round().max(1.0) as u32
            };
            let pos = [(r[0] + r[2] - drawn) * 0.5, (r[1] + r[3] - drawn) * 0.5];
            // At rest, a monochrome icon draws as Chromium paints an inline SVG: its box's own
            // origin rounded to the device grid (not the box a centring snap shrank by a pixel,
            // which redrew it at another scale), and the drawing at its exact scale on a canvas
            // the size rounded up, 1:1 on the grid. A 1 pt stroke at 1.875x covers two pixels
            // at 94% instead of one whole and two at 40%. Moving, turned or scaled icons keep
            // the sub-pixel path below.
            let resting = cover.is_none() && scale_motion == ScaleMotion::Still && !gliding && turn == 0.0 && (sc - 1.0).abs() < 1e-5;
            let exact = w_exact.min(h_exact);
            let kind = self.text.icon_kind(ic);
            if kind == crate::text::IconKind::AnimatedMono && !self.reduced_motion && drawn > 1e-4 {
                let (canvas, scale_side, at, draw_side) = if resting && exact > 1e-4 {
                    let canvas = (exact - 1.0 / 64.0).ceil().max(1.0) as u32;
                    let at = [on_grid(r_exact[0] + (w_exact - exact) * 0.5, contain[0]), on_grid(r_exact[1] + (h_exact - exact) * 0.5, contain[1])];
                    (canvas, exact, at, canvas as f32)
                } else {
                    (px, px as f32, pos, drawn)
                };
                if !paint_animated_icon(
                    &mut self.text,
                    &mut self.paint,
                    &mut self.instances,
                    self.frame.live.visible(),
                    self.t,
                    self.time_scale,
                    n.id,
                    ic,
                    (canvas, scale_side),
                    at,
                    draw_side,
                    turn,
                    facing,
                    color,
                    o,
                    srgb,
                    clip,
                    fade,
                ) && let Some(q) = self.text.icon_quad_sized(ic, (canvas, scale_side), at, facing)
                {
                    self.push_icon(icon_inst(&q, at, draw_side, turn, premul(color, o, srgb), clip, fade));
                }
            } else if resting && exact > 1e-4 && kind != crate::text::IconKind::Color {
                let canvas = (exact - 1.0 / 64.0).ceil().max(1.0) as u32;
                let at = [on_grid(r_exact[0] + (w_exact - exact) * 0.5, contain[0]), on_grid(r_exact[1] + (h_exact - exact) * 0.5, contain[1])];
                if let Some(q) = self.text.icon_quad_sized(ic, (canvas, exact), at, facing) {
                    self.push_icon(icon_inst(&q, at, canvas as f32, 0.0, premul(color, o, srgb), clip, fade));
                }
            // An icon that paints its own colours draws as its image (or its silhouette in
            // `color`, `El::icon_one_color`), upright, faded only by the
            // element's opacity and its colour's alpha.
            } else if kind == crate::text::IconKind::Color && drawn > 1e-4 && let Some(pixels) = self.text.color_icon(ic, px, color, n.vis.icon_one_color) {
                if let Some(image) = self.images.place(pixels, [drawn, drawn]) {
                    self.instances.push(Instance {
                        rect: [pos[0], pos[1], drawn, drawn],
                        radii: [0.0; 4],
                        fill: [0.0, 0.0, 0.0, premul(color, o, srgb)[3]],
                        border: [0.0; 4],
                        params: [image.page as f32, KIND_IMAGE, image.premultiplied as u8 as f32, 1.0],
                        uv: image.rect.map(|v| v as f32),
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            } else if drawn > 1e-4 && let Some(q) = self.text.icon_quad_facing(ic, px, pos, facing) {
                self.push_icon(icon_inst(&q, pos, drawn, turn, premul(color, o, srgb), clip, fade));
            }
        } else if let (Some(spec), Some(laid)) = (&n.text, &n.text_layout) {
            let flow = laid.view();
            let link_fx = link_fx.unwrap();
            let color = text_tint(text_color.unwrap());
            // A matched flight places its own text in its scaled content box. Its glyphs keep
            // their rest size so a label doesn't pop at the swap.
            let (zk, zr) = match cover {
                Some((k, c)) => (k, [r[0] + c[0] * s, r[1] + c[1] * s,
                    r[0] + (c[0] + n.local.w * k) * s, r[1] + (c[1] + n.local.h * k) * s]),
                None => (sc, r),
            };
            // Everywhere else text scales with its box (`El::scale`, `live_scale`,
            // `press_scale`, transitions): `tk` about the text's origin. It rasterizes at
            // `raster` and draws filtered by the rest, `tk / raster`.
            let tk = if cover.is_some() { 1.0 } else { sc };
            let raster = if cover.is_some() { 1.0 } else { scale_motion.raster(sc, rest) };
            if cover.is_none() && scale_motion == ScaleMotion::Live {
                self.paint.scale_frame = Some(AnotherFrame);
            }
            // Text whose scale moves draws from its outlines, magnified to `tk`: crisp all the way,
            // and the same outline on the frame it lands as the atlas mask it rests as.
            let place_masks = self.text.begin_text(node_id.0, cover.is_none() && scale_motion != ScaleMotion::Still)
                .into_iter().any(|e| e == crate::text::outline::PathEffect::PlaceMasks);
            // The frame it lands draws outlines while its masks are placed; one more frame draws
            // the masks, so text never rests on its landing frame.
            if place_masks {
                self.paint.scale_frame = Some(AnotherFrame);
            }
            let direct = self.text.emit() == crate::text::outline::Emit::Outline;
            let ins = n.inset.map(|v| v * zk * s);
            // Text lays out from its box's place moved by how far its exact resting box sits from its
            // snapped one (inside its paint containment), as Blink paints a text fragment from its
            // exact place: its glyphs then round as Skia rounds them (the baseline half up, pens to
            // a quarter pixel) where Chromium has them, rather than from a box already snapped. A
            // gliding box carries its rest's offset, so the frame a glide ends or
            // a flight lands places glyphs exactly as the resting frame after it. A field keeps its
            // snapped box, which its caret and selection share.
            let zr = if scale_motion == ScaleMotion::Still && (sc - 1.0).abs() < 1e-5 && n.input.is_none() {
                let by = exact_off;
                [zr[0] + by[0], zr[1] + by[1], zr[2] + by[0], zr[3] + by[1]]
            } else {
                zr
            };
            let (x0, x1) = (zr[0] + ins[0], zr[2] - ins[2]);
            let mut origin = [text_x(spec, x0, x1, text_width(flow.full_width() * tk), flow.flow()), zr[1] + ins[1]];
            // A gliding box's glyphs and decorations lay out from its nearest whole pixel (plus its rest's
            // fraction, above) and move the rest. Within a 64th of a pixel they don't move, but still lay
            // out from that pixel, as at rest.
            let (glide, off) = if gliding && cover.is_none() && scale_motion == ScaleMotion::Still {
                ([pixel_snap::text_glide(r[0]), pixel_snap::text_glide(r[1])], [r[0] - r[0].round(), r[1] - r[1].round()])
            } else {
                ([0.0; 2], [0.0; 2])
            };
            let mut clip = clip;
            if let Some(inp) = n.input.as_ref().filter(|i| i.single_line) && let Some(layout) = laid.single() {
                let scroll = single_line_scroll(&mut self.edit.input_scroll, &n.path, inp, &spec.content, layout, s, (x1 - x0) / tk.max(1e-4)) * tk;
                origin[0] -= scroll;
                if layout.full_width() * tk > x1 - x0 {
                    clip = [clip[0].max(x0), clip[1], clip[2].min(x1), clip[3]];
                }
            }
            let rest_origin = [origin[0] - off[0], origin[1] - off[1]];
            // Paragraph block backgrounds, quote bars and list markers.
            if spec.cold.blocks.iter().any(|&b| b != crate::edit::Block::Paragraph) {
                emit_block_decorations(
                    &mut self.instances,
                    &mut self.text,
                    &mut self.images,
                    &mut self.paint.glyph_scratch,
                    spec,
                    flow,
                    rest_origin,
                    (x0 - glide[0], x1 - glide[0]),
                    s,
                    tk,
                    raster,
                    scale_motion,
                    glide,
                    color,
                    o,
                    srgb,
                    clip,
                    fade,
                );
            }
            // Text fields: selection under the text, placeholder, caret over it.
            let mut caret = None;
            // The selected display bytes and the ink their `El::selection_style` gives them.
            let mut selected_ink: Option<(usize, usize, [f32; 4])> = None;
            if let Some(inp) = &n.input {
                let e = &inp.edit;
                // The bytes of the paragraphs the clip shows, which a long draft's selection and
                // spelling marks are drawn in; the newline after the last one included.
                let (top, bottom) = if tk > 1e-4 { ((shown[0] - origin[1]) / tk, (shown[1] - origin[1]) / tk) } else { (f32::NEG_INFINITY, f32::INFINITY) };
                let (shown_a, shown_b) = flow.pieces_in(top, bottom).fold((usize::MAX, 0), |(lo, hi), p| (lo.min(p.start), hi.max(p.end.saturating_add(1))));
                let departing = self.motion.flights().iter().any(|f| f.departs(&n.path, n.key.as_deref())
                    && f.alphas(f.destination, now, self.reduced_motion).0 > 0.0);
                if e.text.is_empty() && e.preedit().is_none() && !inp.placeholder.is_empty() && !departing {
                    let pl_indent = spec.cold.blocks.first().map_or(0.0, |b| (b.indent_pt(spec.style.size) * s).round() * tk);
                    // A one-line field's placeholder stays on its line, cut by the field's clip, as its
                    // text does; a multi-line field's wraps at its width.
                    let wrap = (!inp.single_line).then(|| (x1 - x0 - pl_indent).max(1.0) / tk.max(1e-4));
                    // A by-paragraph field's placeholder is a `dir="auto"` text of its own, as
                    // Desktop's composer draws it: in its own direction, its field's padding with it.
                    let own = spec.by_paragraph(true);
                    let options = crate::text::RichLayoutOptions::new().flow(match spec.dir {
                        crate::el::TextDir::Fixed(f) if !own => Some(f),
                        _ => None,
                    });
                    let pl = self.text.layout_rich_with(&inp.placeholder, &[], inp.placeholder_style.unwrap_or(spec.style), s, wrap, options);
                    let pl_flow = crate::el::Flow::of_layout(&pl);
                    let start = if own && pl_flow != n.flow { rest_origin[0] + ins[2] - ins[0] } else { rest_origin[0] };
                    // Right to left it ends at the field's start edge, its right.
                    let pl_x = match (pl_flow, wrap) {
                        (crate::el::Flow::Ltr, _) => start + pl_indent,
                        (crate::el::Flow::Rtl, Some(_)) => start,
                        (crate::el::Flow::Rtl, None) => start - pl_indent - text_width(pl.full_width() * tk),
                    };
                    let pl_w = pl.full_width() * tk;
                    let pl_clip = if inp.single_line && (pl_x + off[0] < x0 || pl_x + off[0] + pl_w > x1) {
                        [clip[0].max(x0), clip[1], clip[2].min(x1), clip[3]]
                    } else {
                        clip
                    };
                    let pl_origin = [pl_x, rest_origin[1]];
                    let pl_r_origin = if scale_motion == ScaleMotion::Still { pl_origin } else { [0.0, 0.0] };
                    self.paint.glyph_scratch.clear();
                    self.text.set_glyph_inks(inp.placeholder_color, &[]);
                    self.text.set_glide(glide);
                    self.text.glyphs_for_layout_scaled(&pl, pl_r_origin, raster, false, &mut self.paint.glyph_scratch);
                    self.text.set_glide([0.0; 2]);
                    let pc = premul(inp.placeholder_color, o, srgb);
                    let zoom = tk / raster;
                    for q in &self.paint.glyph_scratch {
                        let [gx0, gy0] = if zoom == 1.0 && pl_origin == pl_r_origin { [q.rect[0], q.rect[1]] } else {
                            std::array::from_fn(|k| pl_origin[k] + (q.rect[k] - pl_r_origin[k]) * zoom)
                        };
                        if gy0 > pl_clip[3] || gy0 + q.rect[3] * zoom < pl_clip[1]
                            || gx0 > pl_clip[2] || gx0 + q.rect[2] * zoom < pl_clip[0]
                        {
                            continue;
                        }
                        self.instances.extend(glyph_or_image(q, &self.text, &mut self.images, pc, pl_clip, fade)
                            .map(|g| zoom_glyph(g, pl_origin, pl_r_origin, zoom)));
                    }
                }
                if e.focused && !departing {
                    let (a, b) = e.range();
                    if a != b && e.preedit().is_none() {
                        let (da, db) = e.display_selection_range(a, b);
                        // An inactive window paints a field's selection in its inactive pair, as a browser does.
                        let (highlight, ink) = inp.selection_style.map_or((inp.selection, None), |st| st.pick(self.host.window_focused != Some(false)));
                        selectable::push_field_selection(&mut self.instances, flow, da.max(shown_a), db.min(shown_b),
                            origin, tk, premul(highlight, o, srgb), s, clip, fade);
                        selected_ink = ink.map(|c| (da, db, premul(c, o, srgb)));
                    }
                    // A range selection hides the caret, as a browser's field and a native one do.
                    if a == b || e.is_composing() {
                        caret = input_caret;
                    }
                    // The composition's underline: thin under what's composed, thick under the
                    // clause being converted, each clause a point short at both ends so clauses
                    // read apart, as Chromium draws them (`CompositionMarkerPainter`).
                    if e.preedit().is_some() {
                        let pa = e.range().0;
                        for clause in e.preedit_clauses() {
                            let (a, b, thick) = (pa + clause.range().start, pa + clause.range().end, if clause.is_target() { 2.0 } else { 1.0 });
                            for (bb, _) in flow.range_boxes(a, b, false) {
                                let (x0, x1) = (origin[0] + bb.x0 as f32 * tk, origin[0] + bb.x1 as f32 * tk);
                                let inset = if x1 - x0 > 4.0 * s { s } else { 0.0 };
                                let h = thick * s;
                                self.instances.push(rect_inst(
                                    [x0 + inset, origin[1] + bb.y1 as f32 * tk - h, x1 - x0 - 2.0 * inset, h],
                                    premul(color, o, srgb),
                                    0.0,
                                    clip,
                                    fade,
                                ));
                            }
                        }
                    }
                }
                // Misspelled words (`El::spellcheck`), but the one the caret is typing.
                if inp.spellcheck && !e.is_secure() && e.preedit().is_none() && !departing {
                    // Windows and the web check word by word, so the paragraphs shown are checked
                    // alone: a 20,000-line draft hashes and walks only what's on screen. macOS
                    // finds each text's language from the whole of it.
                    let (from, to) = (shown_a.min(e.text.len()), shown_b.min(e.text.len()));
                    let found = if cfg!(any(target_os = "windows", target_arch = "wasm32")) && (from > 0 || to < e.text.len()) && e.text.is_char_boundary(from) && e.text.is_char_boundary(to) {
                        std::rc::Rc::new(crate::spell::misspelled(&e.text[from..to], Some(self.frame.live.waker())).iter().map(|&(a, b)| (a + from, b + from)).collect::<Vec<_>>())
                    } else {
                        crate::spell::misspelled(&e.text, Some(self.frame.live.waker()))
                    };
                    // The word typing left the caret in waits unmarked until the pause ends (`spell::pause`).
                    // Typing shows in the paragraphs shown, the caret's among them.
                    let watched = if e.text.is_char_boundary(from) && e.text.is_char_boundary(to) { &e.text[from..to] } else { e.text.as_str() };
                    let caret = if e.focused { crate::spell::pause::observe(&mut self.edit.spell_watch, &n.path, watched, e.range(), self.clock) } else { None };
                    let color = premul(SPELLING, o, srgb);
                    for &(a, b) in found.iter().filter(|&&(a, b)| caret.is_none_or(|c| c < a || c > b)) {
                        let (da, db) = e.display_selection_range(a, b);
                        if db < shown_a || da > shown_b { continue; }
                        for (bb, baseline) in flow.range_boxes(da, db, false) {
                            // Its top a point under the baseline, as Chromium's.
                            let top = origin[1] + (baseline + s) * tk;
                            let (x0, x1) = (origin[0] + bb.x0 as f32 * tk, origin[0] + bb.x1 as f32 * tk);
                            push_spelling_marker(&mut self.instances, x0, x1, top, s * tk, color, clip, fade);
                        }
                    }
                }
            }
            // Each paragraph a by-paragraph field or by-line block shows (or its one layout), in
            // the rows the clip shows with two ems either side for ink past the line box.
            let rows = crate::runtime::text_fill::visible_text_rows(spec, rest_origin[1], shown, tk, raster, s);
            if let crate::text::TextLayout::Paragraphs(p) = laid && p.waiting() {
                self.paint.text_rows.insert(n.id, (rows.start, rows.end));
            }
            let (base_origin, base_rest) = (origin, rest_origin);
            let base_d_origin = if scale_motion == ScaleMotion::Still && tk == 1.0 { base_rest } else { [0.0, 0.0] };
            let reveal = if spec.cold.advance_in.is_none() { self.motion.reveal_fade(n.id) } else { None };
            let drawn_len = n.elided.map_or(spec.content.len(), |e| e.drawn_len(spec.content.len()));
            self.paint.deco_scratch.clear();
            let mut pill_icons = Vec::new();
            let mut bare_all = Vec::new();
            let pill_bg = spec.cold.pill_colors.map_or(spec.rich_style().link.alpha(0.2), |(bg, _)| bg);
            for piece in flow.pieces_in(rows.start, rows.end) {
                let layout = piece.layout;
                let d_origin = [base_d_origin[0], base_d_origin[1] + piece.y];
                let (spans_here, pills) = (piece.spans(&spec.spans), piece.pills(&spec.cold.pills));
                let from = self.paint.deco_scratch.len();
                crate::text::pill_chips(layout, &pills, spec.style.size, s, d_origin, pill_bg, &mut self.paint.deco_scratch, &mut pill_icons);
                // Elided text draws its decorations over the drawn string's offsets.
                let drawn_spans = n.elided.map(|e| e.spans(&spans_here));
                let spans = drawn_spans.as_deref().unwrap_or(&spans_here);
                let drawn_rings = n.elided.map(|e| e.rings(&spec.cold.rings));
                self.text.decorations_with_rings(layout, spans, drawn_rings.as_deref().unwrap_or(&spec.cold.rings), spec.cold.span_radius, s, d_origin, &mut self.paint.deco_scratch);
                if !piece.is_whole() {
                    for d in &mut self.paint.deco_scratch[from..] { d.byte = piece.content(d.byte as usize) as u32; }
                }
                if !pills.is_empty() {
                    let mut bare = Vec::new();
                    crate::text::bare_icons(layout, &pills, s, d_origin, &mut bare);
                    bare_all.extend(bare.into_iter().map(|(k, at, px)| (pills[k].clone(), at, px)));
                }
            }
            if !spec.cold.highlights.is_empty() {
                let drawn = n.elided.map(|e| e.highlights(&spec.cold.highlights));
                let marks = drawn.as_deref().unwrap_or(&spec.cold.highlights);
                self.text.highlight_decorations_flow(flow, marks, drawn_len, s, base_d_origin, rows.clone(), &mut self.paint.deco_scratch);
            }
            for piece in flow.pieces_in(rows.start, rows.end) {
                let d_origin = [base_d_origin[0], base_d_origin[1] + piece.y];
                let strikes = piece.strikes(&spec.cold.strikes);
                let from = self.paint.deco_scratch.len();
                if !strikes.is_empty() {
                    let spans_here = piece.spans(&spec.spans);
                    let drawn_spans = n.elided.map(|e| e.spans(&spans_here));
                    let spans = drawn_spans.as_deref().unwrap_or(&spans_here);
                    let drawn = n.elided.map(|e| e.strikes(&strikes));
                    self.text.strike_decorations(piece.layout, spans, drawn.as_deref().unwrap_or(&strikes), color, s, d_origin, &mut self.paint.deco_scratch);
                }
                link_fx.decos_piece(&piece, drawn_len, s, d_origin, &mut self.paint.deco_scratch);
                if !piece.is_whole() {
                    for d in &mut self.paint.deco_scratch[from..] { d.byte = piece.content(d.byte as usize) as u32; }
                }
            }
            if !spec.cold.rings.is_empty() || spec.cold.highlights.iter().any(|h| h.ring.is_some()) {
                // Adjacent backgrounds overlap by bg_pad. Paint ringed chips after
                // plain fills so a neighboring fill cannot erase an inset edge;
                // keep underlines above both. No sorting on the ringless path.
                self.paint.deco_scratch.sort_by_key(|d| if d.radius == 0.0 { 2 } else { u8::from(d.ring.is_some()) });
            }
            if base_d_origin != base_rest || tk != 1.0 {
                for d in &mut self.paint.deco_scratch {
                    d.rect = scale_about(d.rect, base_rest, base_d_origin, tk);
                    d.radius *= tk;
                    if let Some((_, rw)) = &mut d.ring { *rw *= tk; }
                }
            }
            if glide != [0.0; 2] {
                for d in &mut self.paint.deco_scratch { d.rect[0] += glide[0]; d.rect[1] += glide[1]; }
            }
            for d in &self.paint.deco_scratch {
                let alpha = if d.ring.is_some() {
                    reveal.map_or(1.0, |fade| fade.alpha(d.byte, self.scene.store().node_time(n.id)))
                } else { 1.0 };
                let c = premul(d.color, o * alpha, srgb);
                let mut inst = rect_inst(d.rect, c, d.radius, clip, fade);
                if let Some((rc, rw)) = d.ring {
                    inst.border = premul(rc, o * alpha, srgb);
                    inst.params[0] = rw;
                    // The opacity the colours carry, so the ring hides the chip's fill by its own alpha.
                    inst.params[3] = o * alpha;
                }
                self.instances.push(inst);
            }
            let pill_fg = spec.cold.pill_colors.and_then(|(_, fg)| fg).unwrap_or(color);
            for (icon, px, at, fg) in pill_icons {
                let pill_fg = fg.unwrap_or(pill_fg);
                let at = [
                    base_rest[0] + (at[0] - base_d_origin[0]) * tk + glide[0],
                    base_rest[1] + (at[1] - base_d_origin[1]) * tk + glide[1],
                ];
                let drawn = px as f32 * tk;
                let px_r = (px as f32 * raster).round().max(1.0) as u32;
                paint_inline_icon(
                    &mut self.text,
                    &mut self.paint,
                    &mut self.instances,
                    self.reduced_motion,
                    self.frame.live.visible(),
                    self.t,
                    self.time_scale,
                    n.id,
                    icon,
                    px_r,
                    drawn,
                    at,
                    pill_fg,
                    o,
                    srgb,
                    clip,
                    fade,
                );
            }
            let mut inline_image_wake: Option<f64> = None;
            for (pill, at, px) in bare_all {
                let Some(b) = pill.bare.as_deref() else { continue };
                let mut at = [
                    base_rest[0] + (at[0] - base_d_origin[0]) * tk + glide[0],
                    base_rest[1] + (at[1] - base_d_origin[1]) * tk + glide[1],
                ];
                let mut drawn = [px[0] * tk, px[1] * tk];
                // A replacing image's clear margins draw past its box.
                if let Some(bleed) = b.replace.map(|r| r.bleed * s * tk).filter(|&v| v > 0.0) {
                    at = [at[0] - bleed, at[1] - bleed];
                    drawn = [drawn[0] + 2.0 * bleed, drawn[1] + 2.0 * bleed];
                }
                // An image fits its box, keeping its shape; a glyph takes the text's colour.
                if let Some(pixels) = &b.image
                    && let (w, h) = pixels.size()
                    && w > 0
                    && h > 0
                {
                    let (w, h) = (w as f32, h as f32);
                    let k = (drawn[0] / w).min(drawn[1] / h);
                    let rect = [at[0] + (drawn[0] - w * k) * 0.5, at[1] + (drawn[1] - h * k) * 0.5, w * k, h * k];
                    // An inline image outside its clip or viewport draws nothing and wakes nothing.
                    let visible = shown_x[1] > shown_x[0] && shown[1] > shown[0]
                        && rect[2] > 0.0 && rect[3] > 0.0
                        && rect[0] + rect[2] > shown_x[0] && rect[0] < shown_x[1]
                        && rect[1] + rect[3] > shown[0] && rect[1] < shown[1];
                    if visible && let Some((image, next)) = self.images.place_at(pixels, [rect[2], rect[3]], Some(now)) {
                        if let Some(at) = next {
                            inline_image_wake = Some(inline_image_wake.map_or(at, |prev| prev.min(at)));
                        }
                        self.instances.push(Instance {
                            rect,
                            radii: [(b.radius * s * tk).min(rect[2].min(rect[3]) * 0.5); 4],
                            fill: [0.0, 0.0, 0.0, o],
                            border: [0.0; 4],
                            params: [image.page as f32, KIND_IMAGE, image.premultiplied as u8 as f32, 1.0],
                            uv: image.rect.map(|v| v as f32),
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                    continue;
                }
                let Some(icon) = pill.icon else { continue };
                let px_r = (px[0] * raster).round().max(1.0) as u32;
                paint_inline_icon(
                    &mut self.text,
                    &mut self.paint,
                    &mut self.instances,
                    self.reduced_motion,
                    self.frame.live.visible(),
                    self.t,
                    self.time_scale,
                    n.id,
                    icon,
                    px_r,
                    drawn[0],
                    at,
                    pill.fg.unwrap_or(color),
                    o,
                    srgb,
                    clip,
                    fade,
                );
            }
            if spec.selectable && n.input.is_none() {
                // A mark keeps the active selection's highlight, whatever the window's focus, and
                // the glyphs their own ink, as a custom highlight's background does.
                let highlight = spec.cold.selection_style.map_or(spec.cold.selection, |st| st.pick(true).0);
                let fill = premul(highlight, o, srgb);
                for &(_, a, b) in mark_ranges.iter().filter(|m| m.0 == i) {
                    let map = |a, b| n.elided.map_or((a, b), |e| e.range(a, b));
                    text_marks::push_mark(&mut self.instances, flow, &spec.content, (a, b), map, base_origin, tk, fill, clip, fade);
                }
                if let Some((a, b)) = selectable::range_in(sel_ranges, i) {
                    let (a, b) = n.elided.map_or((a, b), |e| e.range(a, b));
                    // `::selection` with a colour recolours the selected glyphs, as a field's does.
                    let (highlight, ink) = spec.cold.selection_style.map_or((spec.cold.selection, None), |st| st.pick(self.host.window_focused != Some(false)));
                    selectable::push_selection(&mut self.instances, flow, a, b, base_origin, tk, premul(highlight, o, srgb), s, clip, fade);
                    if let Some(c) = ink {
                        selected_ink = Some((a, b, premul(c, o, srgb)));
                    }
                }
            }
            for piece in flow.pieces_in(rows.start, rows.end) {
                let layout = piece.layout;
                let spans_here = piece.spans(&spec.spans);
                let drawn_spans = n.elided.map(|e| e.spans(&spans_here));
                let spans = drawn_spans.as_deref().unwrap_or(&spans_here);
                self.paint.glyph_scratch.clear();
                self.text.set_glyph_inks(color, spans);
                let origin = [base_rest[0], base_rest[1] + piece.y * tk];
                self.text.set_glide(glide);
                let r_origin = if scale_motion == ScaleMotion::Still { origin } else { [0.0, 0.0] };
                let has_hl_fg = spec.cold.highlights.iter().any(|h| h.fg.is_some());
                // A cell of a longer run draws its slice of the run's glyphs, shaped in context, at the
                // same raster factor as a paragraph: under a zoom its masks are made at the zoomed size
                // rather than magnified, and the frame it comes to rest places them.
                // A still text whose quads this frame would draw as the last one did copies them,
                // as `El::grid` rows do: `quads` is the entry to record them under otherwise.
                let mut copied = false;
                let mut quads: Option<((NodeId, crate::text::LayoutId), u64, usize, usize)> = None;
                let mut pages: Option<Vec<(u16, u32)>> = None;
                let zoom = if let (Some(r), Some((_, at))) = (&n.run_layout, &spec.cold.advance_in)
                    && let Some(x) = r.x(*at)
                {
                    let cell = [r_origin[0] - x * raster, r_origin[1]];
                    self.text.glyphs_in_scaled(r, *at, cell, raster, &mut self.paint.glyph_scratch);
                    if place_masks {
                        let mut masks = Vec::new();
                        self.text.set_emit(crate::text::outline::Emit::Atlas);
                        self.text.glyphs_in_scaled(r, *at, cell, raster, &mut masks);
                        self.text.set_emit(crate::text::outline::Emit::Outline);
                    }
                    tk / raster
                } else {
                    // Only the lines the clip shows, with two ems either side for ink past
                    // the line box; the per-glyph test below still decides what draws.
                    let band = if tk > 1e-4 {
                        let em = spec.spans.iter().filter_map(|sp| sp.size).fold(spec.style.size.max(spec.style.line_px()), f32::max);
                        let pad = 2.0 * em * s + 2.0 / raster.max(1e-4);
                        (shown[0] - origin[1]) / tk - pad..(shown[1] - origin[1]) / tk + pad
                    } else {
                        f32::NEG_INFINITY..f32::INFINITY
                    };
                    let bytes = reveal.is_some() || !link_fx.is_empty() || has_hl_fg || selected_ink.is_some();
                    // A secret's glyphs aren't kept past the frame that draws them.
                    let secret = spec.redacted || n.input.as_ref().is_some_and(|i| i.edit.is_secure());
                    let shared = piece.shared.or(laid.single());
                    let still = scale_motion == ScaleMotion::Still && glide == [0.0; 2] && !bytes && !place_masks
                        && !self.motion.flights().iter().any(|f| f.lands(&n.path, n.key.as_deref()));
                    // A layout whose stamp the id table dropped draws from its glyphs, uncopied.
                    if let Some(l) = shared.filter(|_| still && !direct && !secret)
                        && let Some(layout_id) = self.text.layout_id(l)
                    {
                        use std::hash::{Hash, Hasher};
                        let mut h = rune_motion::Fx::default();
                        (origin.map(f32::to_bits), clip.map(f32::to_bits), fade.map(f32::to_bits), shown.map(f32::to_bits), [tk, raster, s].map(f32::to_bits)).hash(&mut h);
                        (premul(color, o, srgb).map(f32::to_bits), srgb, self.text.emit_key()).hash(&mut h);
                        spans.len().hash(&mut h);
                        for sp in spans.iter() {
                            (sp.start, sp.end, sp.color.map(|c| premul(c, o, srgb).map(f32::to_bits))).hash(&mut h);
                        }
                        let (id, key) = ((n.id, layout_id), h.finish());
                        match self.paint.text_quads.get_mut(&id).filter(|(d, _)| d.key == key) {
                            Some((d, used)) => {
                                *used = self.paint.quads_frame;
                                self.instances.extend_from_slice(&d.instances);
                                d.pages.iter().for_each(|&(page, texels)| self.text.touch_quad(page, texels));
                                *glyph_count += d.glyphs;
                                copied = true;
                                #[cfg(test)]
                                {
                                    self.paint.quads_copied += 1;
                                }
                            }
                            None => {
                                quads = Some((id, key, self.instances.len(), *glyph_count));
                                pages = Some(Vec::new());
                            }
                        }
                    }
                    match shared.filter(|_| !direct && !secret) {
                        _ if copied => {}
                        Some(l) => self.text.glyphs_in_band_cached(l, r_origin, raster, bytes, band.clone(), &mut self.paint.glyph_scratch),
                        None => self.text.glyphs_in_band(layout, r_origin, raster, bytes, band.clone(), &mut self.paint.glyph_scratch),
                    }
                    if place_masks && !secret && let Some(layout) = shared {
                        // Landing: the masks the next frame draws, placed now and not drawn.
                        let mut masks = Vec::new();
                        self.text.set_emit(crate::text::outline::Emit::Atlas);
                        self.text.glyphs_in_band_cached(layout, r_origin, raster, bytes, band, &mut masks);
                        self.text.set_emit(crate::text::outline::Emit::Outline);
                    }
                    tk / raster
                };
                self.text.set_glide([0.0; 2]);
                let base = premul(color, o, srgb);
                let lands = self.motion.flights().iter().any(|f| f.lands(&n.path, n.key.as_deref()));
                for q in if lands { &[][..] } else { &self.paint.glyph_scratch[..] } {
                    let [x0, y0] = if zoom == 1.0 && origin == r_origin { [q.rect[0], q.rect[1]] } else {
                        std::array::from_fn(|k| origin[k] + (q.rect[k] - r_origin[k]) * zoom)
                    };
                    let (x1, y1) = (x0 + q.rect[2] * zoom, y0 + q.rect[3] * zoom);
                    // Wholly outside the clip, a glyph draws nothing: a long line in an
                    // `El::scroll_x` skips the glyphs scrolled out of sight.
                    let out = y0 > clip[3] || y1 < clip[1] || x0 > clip[2] || x1 < clip[0];
                    self.paint.keep.cycles.glyph([x0, y0, x1, y1], !out);
                    if out { continue; }
                    let mut f = base;
                    if q.ink > 0
                        && let Some(c) = spans.get(q.ink as usize - 1).and_then(|sp| sp.color)
                    {
                        f = premul(c, o, srgb);
                    }
                    let cbyte = piece.content(q.byte as usize);
                    if let Some((a, b, c)) = selected_ink
                        && (a..b).contains(&cbyte)
                    {
                        f = c;
                    }
                    if has_hl_fg
                        && let Some(c) = spec.cold.highlights.iter().rev().find_map(|h| {
                            let (a, b) = n.elided.map_or((h.start as usize, h.end as usize), |e| e.mark(h.start as usize, h.end as usize, false));
                            (a..b).contains(&cbyte).then_some(h.fg).flatten()
                        })
                    {
                        f = premul(c, o, srgb);
                    }
                    if !link_fx.is_empty() && let Some(c) = link_fx.glyph_color(cbyte as u32) {
                        f = premul(c, o, srgb);
                    }
                    if let Some(fade) = reveal {
                        let a = fade.alpha(cbyte as u32, self.scene.store().node_time(n.id));
                        if a <= 0.0 {
                            continue;
                        }
                        f = f.map(|v| v * a);
                    }
                    self.instances.extend(glyph_or_image(q, &self.text, &mut self.images, f, clip, fade)
                        .map(|g| zoom_glyph(g, origin, r_origin, zoom)));
                    *glyph_count += 1;
                    // A colour or outline glyph draws per frame, from another atlas or a record.
                    if q.color != 0 || q.outline.is_some() {
                        pages = None;
                    } else if let Some(pages) = pages.as_mut() {
                        let (page, texels) = (q.atlas_page() as u16, (q.rect[2] * if q.lcd { 3.0 } else { 1.0 } * q.rect[3]) as u32);
                        match pages.iter_mut().find(|p| p.0 == page) {
                            Some(p) => p.1 += texels,
                            None => pages.push((page, texels)),
                        }
                    }
                }
                if let (Some((id, key, start, glyphs)), Some(pages)) = (quads, pages) {
                    let d = crate::el::grid::DrawnRow { key, instances: self.instances[start..].to_vec(), pages, glyphs: *glyph_count - glyphs };
                    self.paint.text_quads.insert(id, (d, self.paint.quads_frame));
                }
            }
            if let (Some(c), Some(inp)) = (caret, &n.input) {
                self.instances.push(rect_inst(c, premul(inp.caret, o, srgb), 0.0, clip, fade));
            }
            if let Some(at) = inline_image_wake {
                arm_image_wake(&mut self.paint, &mut self.motion, self.frame.live.visible(), self.clock, self.time_scale, now, at);
            }
            self.text.set_emit(crate::text::outline::Emit::Atlas);
        }
    }

    fn push_icon(&mut self, inst: Instance) {
        if self.paint.paint_rank.is_some() && inst.uv[2] > 0.0 { self.paint.icon_aprons.push(apron_key(&inst)); }
        self.instances.push(inst);
    }

    /// The rectangle a live or truncated text clips its glyphs to (edges, device pixels), its
    /// box, and whether it runs past that box; `None` for other text.
    pub(in crate::runtime) fn text_clip(&self, i: usize, (r, r_exact): ([f32; 4], [f32; 4]), sc: f32, clip: [f32; 4]) -> Option<([f32; 4], [f32; 4], bool)> {
        let s = self.env.scale;
        let n = &self.tree.nodes[i];
        let (Some(spec), Some(layout)) = (&n.text, n.text_layout.as_ref().and_then(TextLayout::single)) else { return None };
        if !(spec.truncate_fade || spec.cold.live.is_some()) { return None; }
        let ins = n.inset.map(|v| v * sc * s);
        let mut bounds = [r[0] + ins[0], r[1] + ins[1], r[2] - ins[2], r[3] - ins[3]];
        // Layout width already includes display scale `s`; bounds also include the
        // element/ancestor transform `sc`. Compare both after that transform. A text fades
        // only when it runs past its laid-out box and the box drawn now: snapping each edge
        // to a whole pixel can leave the drawn box a sliver short, and a box springing to a
        // new layout (`animate_layout`, a flight) is short until it lands. A text that fits
        // isn't clipped by either.
        let exact = r_exact[2] - r_exact[0] - ins[0] - ins[2];
        let laid_out = (n.local.w - n.inset[0] - n.inset[2]).max(0.0) * s;
        let overflows = layout.full_width() > laid_out + 0.5
            && layout.full_width() * sc > (bounds[2] - bounds[0]).max(exact) + 0.5 * sc;
        // A right-to-left paragraph runs out past its left edge.
        match (overflows, crate::el::Flow::of_layout(layout)) {
            (true, _) => {}
            (false, crate::el::Flow::Ltr) => bounds[2] = bounds[2].max(bounds[0] + exact.max(layout.full_width() * sc)),
            (false, crate::el::Flow::Rtl) => bounds[0] = bounds[0].min(bounds[2] - exact.max(layout.full_width() * sc)),
        }
        Some((intersect(clip, bounds), bounds, overflows))
    }

    /// Text-only clipping leaves the box's fill/border and descendants untouched.
    pub(in crate::runtime) fn clip_text_instances(
        &mut self,
        i: usize,
        first_instance: usize,
        (r, r_exact): ([f32; 4], [f32; 4]),
        sc: f32,
        clip: [f32; 4],
        fade: [f32; 4],
        fade_origin: [f32; 4],
    ) {
        let s = self.env.scale;
        if let Some((text_clip, bounds, overflows)) = self.text_clip(i, (r, r_exact), sc, clip)
            && let Some(spec) = &self.tree.nodes[i].text
        {
            let mut widths = fade;
            let mut origins = fade_origin;
            if spec.truncate_fade && overflows {
                let width = (spec.style.size * spec.fade_em * sc * s).min((bounds[2] - bounds[0]).max(0.0)).clamp(0.0, 65504.0);
                // The fade is where the text is cut: the end of its own paragraph, which runs out
                // past the box on that side. An Arabic title in a left-to-right row fades on the
                // left and keeps its first words whole; a Latin one in a right-to-left row on the
                // right.
                let line = self.tree.nodes[i].text_layout.as_ref().and_then(TextLayout::single).map_or(crate::el::Flow::Ltr, |l| crate::el::Flow::of_layout(l));
                let own = if (spec.align == Align::End) != (line == crate::el::Flow::Rtl) { [width, 0.0, 0.0, 0.0] }
                    else { [0.0, 0.0, width, 0.0] };
                inherit_fade(&mut widths, &mut origins, bounds, own);
            }
            for instance in &mut self.instances[first_instance..] {
                if instance.params[1] == KIND_GLYPH {
                    instance.clip = text_clip;
                    instance.fade = crate::render::pack_fade(widths, origins, text_clip);
                }
            }
        }
    }
}

/// Wakes a shown window for an animated image's next frame at animation time `at`.
fn arm_image_wake(
    paint: &mut state::PaintState,
    motion: &mut crate::runtime::tracks::MotionState,
    live_visible: bool,
    clock: f64,
    time_scale: f32,
    now: f64,
    at: f64,
) {
    if live_visible && time_scale > 0.0 {
        let wake = clock + (at - now).max(0.0) / time_scale as f64;
        paint.keep.cycles.other_deadline();
        motion.arm_loop_wake(wake, at);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_inline_icon(
    text: &mut TextSystem,
    paint: &mut state::PaintState,
    instances: &mut Vec<Instance>,
    reduced_motion: bool,
    live_visible: bool,
    t: f64,
    time_scale: f32,
    node_id: NodeId,
    icon: &'static crate::text::Icon,
    px: u32,
    drawn: f32,
    at: [f32; 2],
    fg: Color,
    o: f32,
    srgb: bool,
    clip: [f32; 4],
    fade: [f32; 4],
) {
    if !reduced_motion
        && text.icon_kind(icon) == crate::text::IconKind::AnimatedMono
        && paint_animated_icon(text, paint, instances, live_visible, t, time_scale, node_id, icon, (px, px as f32), at, drawn, 0.0, crate::text::Facing::Upright, fg, o, srgb, clip, fade)
    {
        return;
    }
    if let Some(q) = text.icon_quad(icon, px, at) {
        let inst = icon_inst(&q, at, drawn, 0.0, premul(fg, o, srgb), clip, fade);
        if paint.paint_rank.is_some() && inst.uv[2] > 0.0 { paint.icon_aprons.push(apron_key(&inst)); }
        instances.push(inst);
    }
}

/// Paints a monochrome animated icon via its double-buffered glyph-atlas tile pair, gating wakes
/// on visibility (`live_visible`), non-zero alpha (`> 1/255`), and active state. Returns `false`
/// when the caller should fall back to the static `t = 0` quad.
#[allow(clippy::too_many_arguments)]
fn paint_animated_icon(
    text: &mut TextSystem,
    paint: &mut state::PaintState,
    instances: &mut Vec<Instance>,
    live_visible: bool,
    t: f64,
    time_scale: f32,
    node_id: NodeId,
    ic: &'static crate::text::Icon,
    sized: (u32, f32),
    at: [f32; 2],
    draw_side: f32,
    turn: f32,
    facing: crate::text::Facing,
    color: Color,
    o: f32,
    srgb: bool,
    clip: [f32; 4],
    fade: [f32; 4],
) -> bool {
    const ALPHA_MIN: f32 = 1.0 / 255.0;
    if o <= ALPHA_MIN || color.a <= ALPHA_MIN {
        return false;
    }
    let mount_t = if text.icon_anim_is_finite(ic) {
        paint.get_or_insert_icon_mount(node_id, ic as *const crate::text::Icon as usize, t)
    } else {
        0.0
    };
    // Occluded, minimised, or hidden windows must not rasterize or keep the frame loop awake
    // (`loop_sleep.rs:153`), but still report `animated_icons_shown` so Windows occlusion polling
    // (`look.rs:88`) wakes when uncovered.
    if !live_visible {
        if text.icon_anim_live(ic, t, mount_t) {
            paint.animated_icons_shown = true;
            paint.node_animated_icon = true;
        }
        return false;
    }
    let Some(anim) = text.animated_icon_quad(ic, sized, at, facing, t, mount_t, node_id.0) else {
        return false;
    };
    match anim.state {
        crate::text::AnimState::Moving => {
            paint.animated_icons_shown = true;
            paint.node_animated_icon = true;
            if time_scale > 0.0 {
                paint.animated_icons = true;
            }
        }
        crate::text::AnimState::WaitingUntil(wake_at) => {
            paint.animated_icons_shown = true;
            paint.node_animated_icon = true;
            if time_scale > 0.0 {
                paint.note_icon_anim_wake(wake_at);
            }
        }
        crate::text::AnimState::Finished => {}
    }
    let inst = icon_inst(&anim.quad, at, draw_side, turn, premul(color, o, srgb), clip, fade);
    let slot_index = instances.len();
    if paint.paint_rank.is_some() && inst.uv[2] > 0.0 { paint.icon_aprons.push(apron_key(&inst)); }
    if !matches!(anim.state, crate::text::AnimState::Finished) {
        paint.keep.cycles.record_icon(super::keep::cycle::IconSlot {
            at: slot_index,
            node_id,
            ic,
            sized,
            origin: at,
            draw_side,
            turn,
            facing,
            rect: inst.rect,
        });
    }
    instances.push(inst);
    true
}
