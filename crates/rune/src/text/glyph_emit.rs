//! Glyph emission: walking a laid-out paragraph's glyphs into atlas quads.
use super::*;

impl TextSystem {
    /// Glyph quads for single-line `text` with its layout box's top-left at `origin` (device px).
    pub fn glyphs(&mut self, text: &str, style: TextStyle, scale: f32, origin: [f32; 2], out: &mut Vec<GlyphQuad>) {
        let l = self.layout_rich(text, &[], style, scale, None);
        self.glyphs_for_layout(&l, scale, origin, false, out);
    }

    /// Glyph quads for a cached [`Layout`] with its top-left at `origin` (device px). When
    /// `bytes` is set, each quad carries the byte offset of its cluster. The layout already
    /// holds its scale, so `_scale` is unused; it stays for source compatibility.
    pub fn glyphs_for_layout(
        &mut self,
        layout: &Layout<Ink>,
        _scale: f32,
        origin: [f32; 2],
        bytes: bool,
        out: &mut Vec<GlyphQuad>,
    ) {
        self.glyphs_for_layout_scaled(layout, origin, 1.0, bytes, out);
    }

    /// [`Self::glyphs_for_layout`] with the layout magnified `k` times about `origin`: pen
    /// positions scale and each glyph rasterizes at `k` times its size, so it stays crisp.
    pub(crate) fn glyphs_for_layout_scaled(
        &mut self,
        layout: &Layout<Ink>,
        origin: [f32; 2],
        k: f32,
        bytes: bool,
        out: &mut Vec<GlyphQuad>,
    ) {
        self.glyphs_in_band(layout, origin, k, bytes, f32::NEG_INFINITY..f32::INFINITY, out);
    }

    /// [`Self::glyphs_for_layout_scaled`] for only the lines whose box meets `band`, a span of
    /// the layout's own y (device px, before `k`). Lines are found by binary search, so a long
    /// text in a scroll view costs its visible lines, not all of them.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn glyphs_in_band(
        &mut self,
        layout: &Layout<Ink>,
        origin: [f32; 2],
        k: f32,
        bytes: bool,
        band: std::ops::Range<f32>,
        out: &mut Vec<GlyphQuad>,
    ) {
        let mut table = std::mem::take(&mut self.cluster_scratch);
        let mut coords = std::mem::take(&mut self.coords_scratch);
        let mut lo = 0;
        if band.start > 0.0 {
            let mut hi = layout.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if layout.get(mid).is_some_and(|l| l.metrics().block_max_coord < band.start) { lo = mid + 1 } else { hi = mid }
            }
        }
        let lines = (lo..layout.len()).map_while(|i| layout.get(i).filter(|l| l.metrics().block_min_coord <= band.end));
        for line in lines {
            if bytes {
                table.clear();
                let mut x = line.metrics().offset;
                for run in line.runs() {
                    for cl in run.visual_clusters() {
                        table.push((x, cl.text_range().start as u32));
                        x += cl.advance();
                    }
                }
            }
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
                let face = Face::of(&glyph_run, &mut coords);
                let baseline = (origin[1] + face.baseline * k).round();
                let mut rel = glyph_run.offset();
                for g in glyph_run.glyphs() {
                    let byte = if bytes {
                        let i = table.partition_point(|&(x0, _)| x0 <= rel + 0.01);
                        table.get(i.saturating_sub(1)).map_or(0, |e| e.1)
                    } else {
                        0
                    };
                    let at = if k == 1.0 { [origin[0] + rel + g.x, baseline + g.y] }
                        else { [origin[0] + (rel + g.x) * k, baseline + g.y * k] };
                    self.push_glyph(&face, &coords, g.id, at, k, byte, out);
                    rel += g.advance;
                }
            }
        }
        self.cluster_scratch = table;
        self.coords_scratch = coords;
    }

    /// Glyph quads for the grapheme at byte `at` of a shaped run, with the run's top-left at
    /// `origin` (device px). Only that grapheme's glyphs are visited. `_scale` is unused (the
    /// run holds its scale); it stays for source compatibility.
    pub fn glyphs_in(&mut self, run: &RunLayout, at: usize, _scale: f32, origin: [f32; 2], out: &mut Vec<GlyphQuad>) {
        let Some(i) = run.grapheme(at) else { return };
        let glyphs = run.graphemes[i].2 as usize..run.graphemes[i + 1].2 as usize;
        for g in &run.glyphs[glyphs] {
            let face = &run.faces[g.face as usize];
            let baseline = (origin[1] + face.face.baseline).round();
            self.push_glyph(&face.face, &face.coords, g.id, [origin[0] + g.x, baseline + g.y], 1.0, at as u32, out);
        }
    }

    /// Rasterizes (once) and appends one glyph at device position `at`, `k` times its size.
    #[allow(clippy::too_many_arguments)]
    fn push_glyph(&mut self, face: &Face, coords: &[i16], glyph: u32, at: [f32; 2], k: f32, byte: u32, out: &mut Vec<GlyphQuad>) {
        let [gx, gy] = at;
        let (font, font_size, ink, skew) = (&face.font, face.size * k, face.ink, face.skew);
        let radius = self.apple.radius(ink, font_size);
        let (fx, subpx) = coretext_match::glyph_x(gx, font_size);
        let key = GlyphKey {
            font: font.data.id(),
            index: font.index,
            glyph,
            size: font_size.to_bits(),
            coords: face.coords_key,
            subpx,
            dilation: (radius as f32).to_bits(),
            apple: self.apple.key(),
            skew,
        };
        let entry = match self.atlas.map.get(&key) {
            Some(&e) => {
                if let Some(a) = e {
                    self.atlas.touch(a.page, a.w * a.h);
                }
                e
            }
            None => {
                // Colour glyphs sit on whole pixels and ignore emboldening and obliques.
                let color_key = GlyphKey { subpx: 0, dilation: 0, apple: 0, skew: 0, ..key };
                if let Some(c) = self.color_glyphs.get_mut(&color_key) {
                    c.used = true;
                    out.push(c.quad(gx, gy, ink, byte));
                    return;
                }
                let Some(font_ref) = FontRef::from_index(font.data.as_ref(), font.index as usize) else {
                    return;
                };
                let mut scaler = self
                    .scale_cx
                    .builder(font_ref)
                    .size(font_size)
                    .hint(false)
                    .normalized_coords(coords.iter().copied())
                    .build();
                if let Some((pixels, at)) = rasterize_color(&mut scaler, glyph as u16) {
                    self.color_bytes = self.color_bytes.saturating_add((at.width as usize) * (at.height as usize) * 4);
                    self.color_pixels.push(pixels);
                    let index = self.color_pixels.len() as u32;
                    let c = ColorGlyph { index, used: true, left: at.left, top: at.top, w: at.width, h: at.height };
                    self.color_glyphs.insert(color_key, c);
                    out.push(c.quad(gx, gy, ink, byte));
                    return;
                }
                let placed = self.apple.rasterize(&mut self.scale_cx, &mut self.atlas, macos_smoothing::Job {
                    font: font_ref, key: (key.font, key.index, key.coords), coords, glyph: glyph as u16, ppem: font_size,
                    phase_x: subpx as f32 / coretext_match::PHASES, radius, skew,
                });
                match placed {
                    Rasterized::Placed(g) => {
                        self.atlas.map.insert(key, Some(g));
                        Some(g)
                    }
                    Rasterized::Empty => {
                        self.atlas.map.insert(key, None);
                        None
                    }
                    Rasterized::Unplaced => None,
                }
            }
        };
        if let Some(a) = entry {
            out.push(GlyphQuad {
                rect: [fx + a.left as f32, gy.round() - a.top as f32, a.w as f32, a.h as f32],
                uv: [a.x as f32, (a.page as u32 * ATLAS_SIZE + a.y) as f32],
                ink,
                byte,
                color: 0,
            });
        }
    }
}
