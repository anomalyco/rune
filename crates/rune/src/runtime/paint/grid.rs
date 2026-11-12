//! Painting a terminal's cell grid (`El::grid`): each row as the elements it replaces drew it.
//!
//! A row was a box of the grid's width and one cell tall holding its background boxes, a
//! one-line `rich` text with a zero-width truncate fade, a `text` per pinned cell, and a
//! `canvas` over them, all at the row's top-left. Each part here is placed where layout and the
//! pixel snap put that element (edges rounded to the device grid from their 1/64 px exact
//! positions), and draws through the same calls: the fill's rect instance, the text's
//! decorations and glyph quads from the shared atlas, the shape's instances. So the pixels are
//! the same, from one node instead of a few per row.
//!
//! A row draws only when it changed: its instances are kept with a key of all they depend on
//! (where it sits, the clip, opacity, how glyphs rasterize and the atlas's clock), and a frame
//! where the key holds copies them, keeping the atlas pages they sample resident. A row with a
//! colour glyph or an outline glyph (another atlas, or a record drawn per frame) always draws.

use std::rc::Rc;

use super::*;
use crate::el::grid::{DrawnRow, Grid, GridRow, LaidRow};

impl<S: 'static> Runtime<S> {
    /// Paints node `i`'s grid inside its snapped box `r` (exact `r_exact`, device px edges).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_grid(&mut self, i: usize, g: &Grid, r: [f32; 4], r_exact: [f32; 4], clip: [f32; 4], fade: [f32; 4], o: f32, glyph_count: &mut usize) {
        let s = self.env.scale;
        let srgb = self.srgb_blend;
        let flow = self.nodes[i].flow;
        let node = self.nodes[i].id;
        // The device grid an element's edges land on, and how far the box's own snap moved it
        // past plain rounding (a translation that snaps as a whole).
        let round = |v: f32| (v + 0.5).floor();
        let shift = [r[0] - round(r_exact[0]), r[1] - round(r_exact[1])];
        let origin_pt = [r_exact[0] / s, r_exact[1] / s];
        let px = |pt: f32| rune_motion::snap::grid(pt, s) * s;
        let [cw, ch] = g.cell;
        let _ = self.text.begin_text(node.0, false);
        let cols = f32::from(g.cols);
        for (k, row) in g.rows.iter().enumerate() {
            let y0 = px(origin_pt[1] + k as f32 * ch);
            let y1 = px(origin_pt[1] + (k + 1) as f32 * ch);
            // Rows wholly outside the clip draw nothing.
            if round(y0) + shift[1] > clip[3] || round(y1) + shift[1] < clip[1] {
                continue;
            }
            let x0 = px(origin_pt[0]);
            let x1 = px(origin_pt[0] + cols * cw);
            let edge = |e: [f32; 4]| [round(e[0]) + shift[0], round(e[1]) + shift[1], round(e[2]).max(round(e[0]) + 1.0) + shift[0], round(e[3]).max(round(e[1]) + 1.0) + shift[1]];
            let row_r = edge([x0, y0, x1, y1]);
            let key = |emit: u64| {
                use std::hash::{Hash, Hasher};
                let mut h = rune_motion::Fx::default();
                (row_r.map(f32::to_bits), clip.map(f32::to_bits), fade.map(f32::to_bits), o.to_bits(), s.to_bits(), srgb, emit).hash(&mut h);
                h.finish()
            };
            if let Some(d) = row.drawn.borrow().as_ref().filter(|d| d.key == key(self.text.emit_key())) {
                self.instances.extend_from_slice(&d.instances);
                d.pages.iter().for_each(|&(page, texels)| self.text.touch_quad(page, texels));
                *glyph_count += d.glyphs;
                continue;
            }
            let (start, glyphs) = (self.instances.len(), *glyph_count);
            let mut pages = Some(Vec::new());
            let laid = self.laid_row(row, cols * cw, flow);

            // Background and selection runs: a filled box each, under the text.
            for f in &row.fills {
                let fx0 = px(origin_pt[0] + f32::from(f.start) * cw);
                let fx1 = px(origin_pt[0] + f32::from(f.end) * cw);
                let b = edge([fx0, y0, fx1, y1]);
                let fill = premul(f.color, o, srgb);
                if fill[3] > 0.0005 {
                    self.instances.push(Instance {
                        rect: [b[0], b[1], b[2] - b[0], b[3] - b[1]],
                        radii: [0.0; 4],
                        fill,
                        border: [0.0; 4],
                        params: [0.0, KIND_RECT, 0.0, 1.0],
                        uv: [0.0; 4],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            }

            // The line: a `rich` text clipped to its row (its truncate fade).
            let line = &laid.line;
            let line_flow = crate::el::Flow::of_layout(line);
            let origin = [text_x(&row.line, row_r[0], row_r[2], line.full_width().ceil(), line_flow), row_r[1]];
            let exact_w = x1 - x0;
            let overflows = line.full_width() > cols * cw * s + 0.5 && line.full_width() > (row_r[2] - row_r[0]).max(exact_w) + 0.5;
            let mut bounds = row_r;
            if !overflows {
                match line_flow {
                    crate::el::Flow::Ltr => bounds[2] = bounds[2].max(bounds[0] + exact_w.max(line.full_width())),
                    crate::el::Flow::Rtl => bounds[0] = bounds[0].min(bounds[2] - exact_w.max(line.full_width())),
                }
            }
            let text_clip = intersect(clip, bounds);
            self.paint_grid_text(&row.line, line, origin, clip, Some(text_clip), fade, o, glyph_count, &mut pages);

            // Pinned cells: a `text` each, at its column's left edge and the row's top.
            for (cell, layout) in row.cells.iter().zip(laid.cells.iter()) {
                let layout: &Rc<parley::Layout<crate::text::Ink>> = layout;
                let cx = round(px(origin_pt[0] + f32::from(cell.col) * cw)) + shift[0];
                let cell_flow = crate::el::Flow::of_layout(layout);
                let w = layout.full_width().ceil();
                let at = [text_x(&cell.spec, cx, cx + w, w, cell_flow), row_r[1]];
                self.paint_grid_text(&cell.spec, layout, at, clip, None, fade, o, glyph_count, &mut pages);
            }

            // The vector layer over the text, from its box's origin rounded to the device grid.
            if let Some(shape) = &row.over {
                let at = [round(x0) + shift[0], round(y0) + shift[1]];
                push_shape(&mut self.instances, shape, at, [s, s], s, o, srgb, clip, fade);
            }
            *row.drawn.borrow_mut() = pages.map(|pages| DrawnRow {
                key: key(self.text.emit_key()),
                instances: self.instances[start..].to_vec(),
                pages,
                glyphs: *glyph_count - glyphs,
            });
        }
        self.text.set_emit(crate::text::outline::Emit::Atlas);
    }

    /// Row `row`'s shaped line and cells at this scale and width, shaped on its first paint.
    fn laid_row(&mut self, row: &GridRow, width_pt: f32, flow: crate::el::Flow) -> LaidRowRef {
        let s = self.env.scale;
        let fits = |l: &LaidRow| l.scale == s.to_bits() && l.width == width_pt.to_bits();
        if let Some(l) = row.laid.borrow().as_ref().filter(|l| fits(l)) {
            return LaidRowRef { line: l.line.clone(), cells: l.cells.clone() };
        }
        // As commit lays a text node out: its direction from the element's flow, and its layout
        // at its box's width when it wraps. A row's texts start left to right, so most need no
        // copy; the layout's key is the one commit's shape key gives.
        let mut shape = |spec: &crate::el::TextSpec, wrap: Option<f32>| {
            let dir = crate::el::step_text_dir(spec.dir, crate::el::TextDirEvent::Flatten(Some(flow)));
            if spec.dir == dir && spec.shape.is_none() {
                return self.text.layout_for_spec(spec, s, wrap, line_alignment(spec));
            }
            let mut spec = spec.clone();
            spec.dir = dir;
            spec.shape = None;
            self.text.layout_for_spec(&spec, s, wrap, line_alignment(&spec))
        };
        let line = shape(&row.line, row.line.wrap.then_some(width_pt * s));
        let cells: Vec<_> = row.cells.iter().map(|c| shape(&c.spec, None)).collect();
        *row.laid.borrow_mut() = Some(LaidRow { scale: s.to_bits(), width: width_pt.to_bits(), line: line.clone(), cells: cells.clone() });
        LaidRowRef { line, cells }
    }

    /// One text's decorations and glyphs from `origin`, as a still, unscaled text node paints
    /// them; `text_clip` is the clip its glyphs take when it has one of its own. Adds the atlas
    /// pages its glyphs sample to `pages`, or empties it for a glyph a copy can't draw again.
    #[allow(clippy::too_many_arguments)]
    fn paint_grid_text(&mut self, spec: &crate::el::TextSpec, layout: &Rc<parley::Layout<crate::text::Ink>>, origin: [f32; 2], clip: [f32; 4], text_clip: Option<[f32; 4]>, fade: [f32; 4], o: f32, glyph_count: &mut usize, pages: &mut Option<Vec<(u16, u32)>>) {
        let s = self.env.scale;
        let srgb = self.srgb_blend;
        self.deco_scratch.clear();
        self.text.decorations_with_rings(layout, &spec.spans, &[], spec.cold.span_radius, s, origin, &mut self.deco_scratch);
        for d in &self.deco_scratch {
            self.instances.push(rect_inst(d.rect, premul(d.color, o, srgb), d.radius, clip, fade));
        }
        self.glyph_scratch.clear();
        self.text.set_glyph_inks(spec.color, &spec.spans);
        self.text.set_glide([0.0; 2]);
        let em = spec.spans.iter().filter_map(|sp| sp.size).fold(spec.style.size.max(spec.style.line_px()), f32::max);
        let pad = 2.0 * em * s + 2.0;
        let band = (clip[1] - origin[1]) - pad..(clip[3] - origin[1]) + pad;
        self.text.glyphs_in_band_cached(layout, origin, 1.0, false, band, &mut self.glyph_scratch);
        let base = premul(spec.color, o, srgb);
        let glyph_clip = text_clip.unwrap_or(clip);
        for q in &self.glyph_scratch {
            let [x0, y0] = [q.rect[0], q.rect[1]];
            let (x1, y1) = (x0 + q.rect[2], y0 + q.rect[3]);
            if y0 > clip[3] || y1 < clip[1] || x0 > clip[2] || x1 < clip[0] {
                continue;
            }
            let f = if q.ink > 0 && let Some(c) = spec.spans.get(q.ink as usize - 1).and_then(|sp| sp.color) { premul(c, o, srgb) } else { base };
            self.instances.extend(glyph_or_image(q, &self.text, &mut self.images, f, glyph_clip, fade));
            *glyph_count += 1;
            if q.color != 0 || q.outline.is_some() {
                *pages = None;
            } else if let Some(pages) = pages.as_mut() {
                let (page, texels) = (q.atlas_page() as u16, (q.rect[2] * if q.lcd { 3.0 } else { 1.0 } * q.rect[3]) as u32);
                match pages.iter_mut().find(|p| p.0 == page) {
                    Some(p) => p.1 += texels,
                    None => pages.push((page, texels)),
                }
            }
        }
    }
}

/// A row's laid-out line and cells, for one paint.
struct LaidRowRef {
    line: Rc<parley::Layout<crate::text::Ink>>,
    cells: Vec<Rc<parley::Layout<crate::text::Ink>>>,
}
