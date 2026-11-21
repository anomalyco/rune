use super::Painter;
use super::super::shade::Clip;
use crate::render::Instance;
use crate::render::instance::shader_consts;
use crate::render::variant::LCD_MEAN_EPSILON;

/// `pow(cov / 255, e)` for each coverage byte, per exponent: Skia's text gamma keys a handful of
/// exponents by the text's luminance, and a frame's text alternates between them (syntax colours),
/// so the last `POW_TABLES` stay.
#[derive(Default)]
pub(super) struct Pow {
    tables: Vec<(u32, [f32; 256])>,
}

const POW_TABLES: usize = 16;

impl Pow {
    fn index(&mut self, e: f32) -> usize {
        let bits = e.to_bits();
        let at = match self.tables.iter().position(|(k, _)| *k == bits) {
            Some(at) => at,
            None => {
                if self.tables.len() == POW_TABLES { self.tables.remove(0); }
                self.tables.push((bits, std::array::from_fn(|c| if e == 1.0 { c as f32 / 255.0 } else { (c as f32 / 255.0).powf(e) })));
                self.tables.len() - 1
            }
        };
        at
    }
}

impl Painter<'_, '_> {
    /// A `KIND_GLYPH` from its atlas mask: whole texels at rest, bilinear when its quad is
    /// scaled (`uv.z > 0`) or turned (`params.x`), a subpixel mask's three texels a pixel
    /// (`radii.x`).
    pub(super) fn glyph(&mut self, i: &Instance, r: [u32; 4], clip: &Clip) {
        if i.radii[3] > 0.0 { return self.outline(i, r, clip); }
        let Some(&mask) = self.scene.glyph_pages.get(i.params[2] as usize) else { return };
        let side = crate::text::ATLAS_SIZE as i32;
        let load = |x: i32, y: i32| -> u8 { if x < 0 || y < 0 || x >= side || y >= side { 0 } else { mask[(y * side + x) as usize] } };
        let e = i.params[3];
        let turn = i.params[0];
        let rotated = turn != 0.0;
        let filtered = i.uv[2] > 0.0 || rotated;
        let lcd = i.radii[0] > shader_consts::FLAG_ON_THRESHOLD && !filtered;
        // Subpixel text blends by channel over the window; inside layers it draws grayscale.
        let by_channel = lcd && self.scene.lcd && self.depth == 0;
        let (sin, cos) = turn.sin_cos();
        let centre = [i.rect[0] + i.rect[2] * 0.5, i.rect[1] + i.rect[3] * 0.5];
        let span = if i.uv[2] > 0.0 { [i.uv[2], i.uv[3]] } else { [i.rect[2], i.rect[3]] };
        let table = self.pow.index(e);
        let mut count = 0;
        let segs = self.segments(r, None, clip);
        let mut k = 0;
        for y in r[1]..r[3] {
            let py = y as f32 + 0.5;
            while segs[k].y1 <= y { k += 1; }
            let (whole, vis) = (segs[k].whole, segs[k].vis);
            let ty = i.uv[1] as i32 + (py - i.rect[1]).floor() as i32;
            if !lcd && !filtered {
                // A mask at rest: one texel a pixel, a row of them per row.
                if ty < 0 || ty >= side { continue; }
                let row = &mask[(ty * side) as usize..((ty + 1) * side) as usize];
                let t0 = i.uv[0] as i32 + (r[0] as f32 + 0.5 - i.rect[0]).floor() as i32 - r[0] as i32;
                for x in vis.iter().flat_map(|(a, b)| a..b) {
                    let Some(&v) = row.get((t0 + x as i32).max(0) as usize).filter(|_| t0 + x as i32 >= 0) else { continue };
                    if v == 0 { continue; }
                    count += 1;
                    let px = x as f32 + 0.5;
                    let a = if px >= whole.0 && px <= whole.1 { 1.0 } else { clip.alpha([px, py]) };
                    let cov = self.pow.tables[table].1[v as usize];
                    self.put(x, y, i.fill.map(|f| f * cov * a));
                }
                continue;
            }
            for x in vis.iter().flat_map(|(a, b)| a..b) {
                let px = x as f32 + 0.5;
                let alpha = || if px >= whole.0 && px <= whole.1 { 1.0 } else { clip.alpha([px, py]) };
                if lcd {
                    let tx = i.uv[0] as i32 + (px - i.rect[0]).floor() as i32 * shader_consts::OUTLINE_SUBPIXEL_STRIDE as i32;
                    let c3 = [load(tx, ty), load(tx + 1, ty), load(tx + 2, ty)];
                    if c3 == [0; 3] { continue; }
                    count += 1;
                    let a = alpha();
                    let mean = ((c3[0] as f32 + c3[1] as f32 + c3[2] as f32) / shader_consts::OUTLINE_SUBPIXEL_STRIDE / 255.0).powf(e);
                    if by_channel {
                        if mean < LCD_MEAN_EPSILON { continue; }
                        let kc = c3.map(|v| self.pow.tables[table].1[v as usize]);
                        let f = i.fill.map(|v| v * a);
                        self.surface.blend_lcd(x, y, [f[0] * kc[0], f[1] * kc[1], f[2] * kc[2], f[3] * mean], kc.map(|v| f[3] * v));
                    } else {
                        self.put(x, y, i.fill.map(|v| v * mean * a));
                    }
                    continue;
                }
                let cov = if filtered {
                    // A turned icon samples at its unturned point; outside its quad is outside.
                    let p = if rotated {
                        let d = [px - centre[0], py - centre[1]];
                        [centre[0] + cos * d[0] + sin * d[1], centre[1] - sin * d[0] + cos * d[1]]
                    } else { [px, py] };
                    if rotated && (p[0] < i.rect[0] || p[1] < i.rect[1] || p[0] >= i.rect[0] + i.rect[2] || p[1] >= i.rect[1] + i.rect[3]) { continue; }
                    let local = [(p[0] - i.rect[0]) * span[0] / i.rect[2] - 0.5, (p[1] - i.rect[1]) * span[1] / i.rect[3] - 0.5];
                    let base = [local[0].floor(), local[1].floor()];
                    let f = [local[0] - base[0], local[1] - base[1]];
                    let t = [(i.uv[0] + base[0]) as i32, (i.uv[1] + base[1]) as i32];
                    let s = |dx: i32, dy: i32| load(t[0] + dx, t[1] + dy) as f32 / 255.0;
                    let mix = |a: f32, b: f32, t: f32| a * (1.0 - t) + b * t;
                    let c = mix(mix(s(0, 0), s(1, 0), f[0]), mix(s(0, 1), s(1, 1), f[0]), f[1]);
                    if c <= 0.0 { continue; }
                    if e == 1.0 { c } else { c.powf(e) }
                } else {
                    let v = load(i.uv[0] as i32 + (px - i.rect[0]).floor() as i32, ty);
                    if v == 0 { continue; }
                    self.pow.tables[table].1[v as usize]
                };
                count += 1;
                let a = alpha();
                self.put(x, y, i.fill.map(|v| v * cov * a));
            }
        }
        self.stats.glyph_px += count;
        self.segs = segs;
    }

    /// A glyph drawn from its outline (text whose scale moves): outline.wgsl's `outline_glyph`,
    /// Windfoil's box (coverage 0), or one box about each subpixel (2), blended by channel
    /// over the window where the frame's text is subpixel.
    fn outline(&mut self, i: &Instance, r: [u32; 4], clip: &Clip) {
        let Some(store) = self.scene.outlines else { return };
        let z = i.radii[3];
        let (rec, hint, radius, mode) = (i.border[0] as u32, i.border[1] as u32, i.border[2], (i.border[3] + 0.5) as u32);
        let hw = 0.5 / z;
        let across = if mode == shader_consts::OUTLINE_MODE_SUBPIXEL_MASK { shader_consts::OUTLINE_SUBPIXEL_STRIDE } else { 1.0 };
        let e = i.params[3];
        let rgb = mode == shader_consts::OUTLINE_MODE_RGB && self.scene.lcd && self.depth == 0;
        let cover = |local: [f32; 2]| crate::text::outline::coverage(store, rec, hint, local, i.radii[2], radius, hw);
        let segs = self.segments(r, None, clip);
        let mut k = 0;
        for y in r[1]..r[3] {
            let py = y as f32 + 0.5;
            while segs[k].y1 <= y { k += 1; }
            let (whole, vis) = (segs[k].whole, segs[k].vis);
            for x in vis.iter().flat_map(|(a, b)| a..b) {
                let px = x as f32 + 0.5;
                let local = [(px - i.radii[0]) / across / z, (i.radii[1] - py) / z];
                let a = if px >= whole.0 && px <= whole.1 { 1.0 } else { clip.alpha([px, py]) };
                if rgb {
                    let third = 2.0 * hw / shader_consts::OUTLINE_SUBPIXEL_STRIDE;
                    let c3 = [cover([local[0] - third, local[1]]), cover(local), cover([local[0] + third, local[1]])];
                    let mean = ((c3[0] + c3[1] + c3[2]) / shader_consts::OUTLINE_SUBPIXEL_STRIDE).powf(e);
                    if mean < LCD_MEAN_EPSILON { continue; }
                    let kc = c3.map(|v| v.powf(e));
                    let f = i.fill.map(|v| v * a);
                    self.surface.blend_lcd(x, y, [f[0] * kc[0], f[1] * kc[1], f[2] * kc[2], f[3] * mean], kc.map(|v| f[3] * v));
                    continue;
                }
                let cov = cover(local);
                if cov <= 0.0 { continue; }
                self.stats.glyph_px += 1;
                self.put(x, y, i.fill.map(|v| v * cov.powf(e) * a));
            }
        }
        self.segs = segs;
    }
}
