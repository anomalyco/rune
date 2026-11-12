//! COLR v1 colour glyphs (gradients, transforms, layers): Windows 11's Segoe UI Emoji and the
//! COLR v1 Noto Color Emoji many Linux distributions ship. swash draws only COLR v0's flat
//! layers, and nothing for a v1-only font. This walks skrifa's paint graph into premultiplied
//! RGBA at the glyph's pixel size, as Skia does for Chromium: each clip a coverage mask from
//! zeno, each fill a solid colour or gradient evaluated per pixel, layers composited on pop.
//! A glyph is drawn once per size; the image atlas keeps it.

use skrifa::color::{Brush, ColorGlyphFormat, ColorPainter, ColorStop, CompositeMode, Extend, Transform};
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::metrics::BoundingBox;
use skrifa::raw::TableProvider;
use skrifa::{GlyphId, MetadataProvider};
use swash::zeno::{Command, Mask, Vector};

/// Glyph `glyph` of the font as a COLR v1 image at `size` px, with its placement (left, top
/// above the baseline, width, height): None when the font has no v1 paint graph for it.
pub(super) fn rasterize(data: &[u8], index: u32, glyph: u32, size: f32, coords: &[i16]) -> Option<(crate::pixels::Pixels, swash::zeno::Placement)> {
    let font = skrifa::FontRef::from_index(data, index).ok()?;
    let color = font.color_glyphs().get_with_format(GlyphId::new(glyph), ColorGlyphFormat::ColrV1)?;
    let coords: Vec<NormalizedCoord> = coords.iter().map(|&c| NormalizedCoord::from_bits(c)).collect();
    let location = LocationRef::new(&coords);
    let upem = font.head().ok()?.units_per_em() as f32;
    let scale = size / upem;
    let bounds = color.bounding_box(location, Size::new(size)).or_else(|| {
        // No clip box: the em square across the glyph's advance.
        let advance = font.glyph_metrics(Size::new(size), location).advance_width(GlyphId::new(glyph))?;
        Some(BoundingBox { x_min: 0.0, y_min: -0.25 * size, x_max: advance, y_max: size })
    })?;
    let (left, top) = (bounds.x_min.floor() as i32, bounds.y_max.ceil() as i32);
    let (w, h) = ((bounds.x_max.ceil() as i32 - left).max(0) as u32, (top - bounds.y_min.floor() as i32).max(0) as u32);
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }
    let cpal = font.cpal().ok();
    let palette = Palette {
        records: cpal.as_ref().and_then(|c| c.color_records_array()).and_then(Result::ok).unwrap_or(&[]),
        first: cpal.as_ref().and_then(|c| c.color_record_indices().first().map(|f| f.get() as usize)).unwrap_or(0),
    };
    // Font units, y up, to image pixels, y down.
    let base = Transform { xx: scale, yx: 0.0, xy: 0.0, yy: -scale, dx: -left as f32, dy: top as f32 };
    let mut painter = Painter {
        location,
        palette,
        w,
        h,
        transforms: vec![base],
        outlines: font.outline_glyphs(),
        clips: vec![Clip { cover: vec![1.0; (w * h) as usize], bounds: [0, 0, w as usize, h as usize] }],
        layers: vec![(vec![[0.0; 4]; (w * h) as usize], CompositeMode::SrcOver)],
    };
    color.paint(location, &mut painter).ok()?;
    let rgba: Vec<u8> = painter.layers.pop()?.0.iter().flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    let pixels = crate::pixels::Pixels::new();
    pixels.set(w, h, rgba);
    Some((pixels, swash::zeno::Placement { left, top, width: w, height: h }))
}

/// Palette 0, read where a paint asks for a colour: Segoe UI Emoji's has thousands of entries
/// and a glyph uses a handful.
struct Palette<'a> {
    records: &'a [skrifa::raw::tables::cpal::ColorRecord],
    first: usize,
}

impl Palette<'_> {
    /// Straight RGBA in 0..1. 0xFFFF is the text's colour; an emoji's image takes none, so it's black.
    fn color(&self, index: u16, alpha: f32) -> [f32; 4] {
        let c = self.records.get(self.first + index as usize).map_or([0.0, 0.0, 0.0, 1.0], |c| [c.red, c.green, c.blue, c.alpha].map(|v| v as f32 / 255.0));
        [c[0], c[1], c[2], c[3] * alpha]
    }

    /// The colour line at `t`, premultiplied, interpolated premultiplied as Skia does so a
    /// stop going transparent doesn't darken.
    fn stop(&self, stops: &[ColorStop], extend: Extend, t: f32) -> [f32; 4] {
        let t = extend_t(extend, t);
        let at = |s: &ColorStop| premul(self.color(s.palette_index, s.alpha));
        let (Some(first), Some(last)) = (stops.first(), stops.last()) else { return [0.0; 4] };
        if t <= first.offset { return at(first); }
        for pair in stops.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if t <= b.offset {
                let k = (t - a.offset) / (b.offset - a.offset).max(1e-6);
                let (pa, pb) = (at(a), at(b));
                return [0, 1, 2, 3].map(|i| pa[i] + (pb[i] - pa[i]) * k);
            }
        }
        at(last)
    }
}

struct Painter<'a> {
    location: LocationRef<'a>,
    palette: Palette<'a>,
    w: u32,
    h: u32,
    transforms: Vec<Transform>,
    outlines: skrifa::outline::OutlineGlyphCollection<'a>,
    /// Each the intersection of the clips under it.
    clips: Vec<Clip>,
    /// Premultiplied RGBA layers, each with the mode it merges down with.
    layers: Vec<(Vec<[f32; 4]>, CompositeMode)>,
}

/// Coverage 0..1 per pixel, and the rows and columns (x0, y0, x1, y1) where it isn't zero, which
/// a fill visits alone.
struct Clip {
    cover: Vec<f32>,
    bounds: [usize; 4],
}

/// `a` after `b`: `b` applies first.
fn concat(a: &Transform, b: &Transform) -> Transform {
    Transform {
        xx: a.xx * b.xx + a.xy * b.yx,
        yx: a.yx * b.xx + a.yy * b.yx,
        xy: a.xx * b.xy + a.xy * b.yy,
        yy: a.yx * b.xy + a.yy * b.yy,
        dx: a.xx * b.dx + a.xy * b.dy + a.dx,
        dy: a.yx * b.dx + a.yy * b.dy + a.dy,
    }
}

fn apply(t: &Transform, x: f32, y: f32) -> (f32, f32) {
    (t.xx * x + t.xy * y + t.dx, t.yx * x + t.yy * y + t.dy)
}

fn invert(t: &Transform) -> Option<Transform> {
    let det = t.xx * t.yy - t.xy * t.yx;
    if det.abs() < 1e-12 { return None; }
    let (xx, xy, yx, yy) = (t.yy / det, -t.xy / det, -t.yx / det, t.xx / det);
    Some(Transform { xx, xy, yx, yy, dx: -(xx * t.dx + xy * t.dy), dy: -(yx * t.dx + yy * t.dy) })
}

/// An outline in image pixels, through the current transform.
struct Pen<'t> {
    t: &'t Transform,
    path: Vec<Command>,
}

impl Pen<'_> {
    fn v(&self, x: f32, y: f32) -> Vector {
        let (x, y) = apply(self.t, x, y);
        Vector::new(x, y)
    }
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) { let v = self.v(x, y); self.path.push(Command::MoveTo(v)); }
    fn line_to(&mut self, x: f32, y: f32) { let v = self.v(x, y); self.path.push(Command::LineTo(v)); }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) { let (c, v) = (self.v(cx, cy), self.v(x, y)); self.path.push(Command::QuadTo(c, v)); }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let (a, b, v) = (self.v(c0x, c0y), self.v(c1x, c1y), self.v(x, y));
        self.path.push(Command::CurveTo(a, b, v));
    }
    fn close(&mut self) { self.path.push(Command::Close); }
}

impl Painter<'_> {
    fn transform(&self) -> Transform { *self.transforms.last().unwrap_or(&Transform::default()) }

    fn push_mask(&mut self, path: &[Command]) {
        let (w, h) = (self.w as usize, self.h as usize);
        let Some(under) = self.clips.last() else { return };
        // Only the box the path and the clip under it share is rasterised and visited.
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for v in path.iter().flat_map(|c| match *c {
            Command::MoveTo(a) | Command::LineTo(a) => [Some(a), None, None],
            Command::QuadTo(a, b) => [Some(a), Some(b), None],
            Command::CurveTo(a, b, d) => [Some(a), Some(b), Some(d)],
            Command::Close => [None, None, None],
        }).flatten() {
            lo = [lo[0].min(v.x), lo[1].min(v.y)];
            hi = [hi[0].max(v.x), hi[1].max(v.y)];
        }
        let [ux0, uy0, ux1, uy1] = under.bounds;
        let bx0 = (lo[0].floor().max(0.0) as usize).max(ux0);
        let by0 = (lo[1].floor().max(0.0) as usize).max(uy0);
        let bx1 = (hi[0].ceil().max(0.0) as usize).min(w).min(ux1);
        let by1 = (hi[1].ceil().max(0.0) as usize).min(h).min(uy1);
        let mut cover = vec![0.0f32; w * h];
        let mut bounds = [usize::MAX, usize::MAX, 0, 0];
        if bx1 > bx0 && by1 > by0 {
            let (bw, bh) = (bx1 - bx0, by1 - by0);
            let mut mask = vec![0u8; bw * bh];
            Mask::new(path).size(bw as u32, bh as u32).offset(Vector::new(-(bx0 as f32), -(by0 as f32))).render_into(&mut mask, None);
            for y in by0..by1 {
                for x in bx0..bx1 {
                    let (m, k) = (mask[(y - by0) * bw + (x - bx0)], y * w + x);
                    if m == 0 || under.cover[k] <= 0.0 { continue; }
                    cover[k] = m as f32 / 255.0 * under.cover[k];
                    bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x + 1), bounds[3].max(y + 1)];
                }
            }
        }
        if bounds[0] == usize::MAX { bounds = [0, 0, 0, 0]; }
        self.clips.push(Clip { cover, bounds });
    }
}

/// `t` on the colour line's 0..1 as `extend` carries it past the ends.
fn extend_t(extend: Extend, t: f32) -> f32 {
    match extend {
        Extend::Repeat => t - t.floor(),
        Extend::Reflect => { let m = t.rem_euclid(2.0); if m > 1.0 { 2.0 - m } else { m } }
        _ => t.clamp(0.0, 1.0),
    }
}

fn premul(c: [f32; 4]) -> [f32; 4] { [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]] }

/// The parameter of the two-point conical gradient at `p`: the largest `t` with
/// `|p - c(t)| = r(t)` and `r(t) >= 0`.
fn radial_t(p: (f32, f32), c0: (f32, f32), r0: f32, c1: (f32, f32), r1: f32) -> Option<f32> {
    let (cdx, cdy, dr) = (c1.0 - c0.0, c1.1 - c0.1, r1 - r0);
    let (pdx, pdy) = (p.0 - c0.0, p.1 - c0.1);
    let a = cdx * cdx + cdy * cdy - dr * dr;
    let b = pdx * cdx + pdy * cdy + r0 * dr;
    let c = pdx * pdx + pdy * pdy - r0 * r0;
    if a.abs() < 1e-6 {
        if b.abs() < 1e-6 { return None; }
        let t = c / (2.0 * b);
        return (r0 + t * dr >= 0.0).then_some(t);
    }
    let disc = b * b - a * c;
    if disc < 0.0 { return None; }
    let s = disc.sqrt();
    let (t1, t2) = ((b + s) / a, (b - s) / a);
    let (hi, lo) = if t1 > t2 { (t1, t2) } else { (t2, t1) };
    if r0 + hi * dr >= 0.0 { Some(hi) } else if r0 + lo * dr >= 0.0 { Some(lo) } else { None }
}

fn composite(mode: CompositeMode, src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let (sa, da) = (src[3], dst[3]);
    let over = |i: usize| src[i] + dst[i] * (1.0 - sa);
    match mode {
        CompositeMode::Clear => [0.0; 4],
        CompositeMode::Src => src,
        CompositeMode::Dest => dst,
        CompositeMode::DestOver => [0, 1, 2, 3].map(|i| dst[i] + src[i] * (1.0 - da)),
        CompositeMode::SrcIn => src.map(|c| c * da),
        CompositeMode::DestIn => dst.map(|c| c * sa),
        CompositeMode::SrcOut => src.map(|c| c * (1.0 - da)),
        CompositeMode::DestOut => dst.map(|c| c * (1.0 - sa)),
        CompositeMode::SrcAtop => { let mut o = [0, 1, 2].map(|i| src[i] * da + dst[i] * (1.0 - sa)).to_vec(); o.push(da); [o[0], o[1], o[2], o[3]] }
        CompositeMode::DestAtop => { let mut o = [0, 1, 2].map(|i| dst[i] * sa + src[i] * (1.0 - da)).to_vec(); o.push(sa); [o[0], o[1], o[2], o[3]] }
        CompositeMode::Xor => [0, 1, 2, 3].map(|i| src[i] * (1.0 - da) + dst[i] * (1.0 - sa)),
        CompositeMode::Plus => [0, 1, 2, 3].map(|i| (src[i] + dst[i]).min(1.0)),
        CompositeMode::Multiply => {
            let mut o = [0, 1, 2].map(|i| src[i] * dst[i] + src[i] * (1.0 - da) + dst[i] * (1.0 - sa)).to_vec();
            o.push(sa + da - sa * da);
            [o[0], o[1], o[2], o[3]]
        }
        CompositeMode::Screen => [0, 1, 2, 3].map(|i| src[i] + dst[i] - src[i] * dst[i]),
        // The rest of the separable and non-separable blend modes are rare in emoji fonts:
        // drawn over, as a renderer that lacks one does.
        _ => [over(0), over(1), over(2), over(3)],
    }
}

impl ColorPainter for Painter<'_> {
    fn push_transform(&mut self, transform: Transform) {
        let t = concat(&self.transform(), &transform);
        self.transforms.push(t);
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 { self.transforms.pop(); }
    }

    fn push_clip_glyph(&mut self, glyph_id: GlyphId) {
        let t = self.transform();
        let mut pen = Pen { t: &t, path: Vec::new() };
        if let Some(outline) = self.outlines.get(glyph_id) {
            let _ = outline.draw(DrawSettings::unhinted(Size::unscaled(), self.location), &mut pen);
        }
        let path = pen.path;
        self.push_mask(&path);
    }

    fn push_clip_box(&mut self, b: BoundingBox) {
        let t = self.transform();
        let corners = [(b.x_min, b.y_min), (b.x_max, b.y_min), (b.x_max, b.y_max), (b.x_min, b.y_max)];
        let mut path: Vec<Command> = corners.iter().enumerate().map(|(i, &(x, y))| {
            let (x, y) = apply(&t, x, y);
            if i == 0 { Command::MoveTo(Vector::new(x, y)) } else { Command::LineTo(Vector::new(x, y)) }
        }).collect();
        path.push(Command::Close);
        self.push_mask(&path);
    }

    fn pop_clip(&mut self) {
        if self.clips.len() > 1 { self.clips.pop(); }
    }

    fn fill(&mut self, brush: Brush<'_>) {
        let Some(inv) = invert(&self.transform()) else { return };
        let Painter { palette, clips, layers, w, .. } = self;
        let w = *w as usize;
        let (Some(clip), Some((layer, _))) = (clips.last(), layers.last_mut()) else { return };
        let [x0, y0, x1, y1] = clip.bounds;
        // A gradient's colour line, sampled once per fill in 64 steps: finer than one step a
        // pixel at emoji sizes (an emoji is 20-80 px across), and a quarter of 256's setup.
        let line = |stops: &[ColorStop]| std::array::from_fn::<[f32; 4], 64, _>(|i| palette.stop(stops, Extend::Pad, i as f32 / 63.0));
        let pick = |lut: &[[f32; 4]; 64], extend: Extend, t: f32| lut[(extend_t(extend, t) * 63.0 + 0.5) as usize];
        // Each brush's colour at pixel (x, y), decided once per fill rather than per pixel.
        let mut paint = |colour: &mut dyn FnMut(f32, f32) -> [f32; 4]| {
            for y in y0..y1 {
                for x in x0..x1 {
                    let k = y * w + x;
                    let cover = clip.cover[k];
                    if cover <= 0.0 { continue; }
                    let c = colour(x as f32 + 0.5, y as f32 + 0.5);
                    let (s, d) = (c.map(|v| v * cover), layer[k]);
                    layer[k] = [s[0] + d[0] * (1.0 - s[3]), s[1] + d[1] * (1.0 - s[3]), s[2] + d[2] * (1.0 - s[3]), s[3] + d[3] * (1.0 - s[3])];
                }
            }
        };
        match brush {
            Brush::Solid { palette_index, alpha } => {
                let c = premul(palette.color(palette_index, alpha));
                paint(&mut |_, _| c);
            }
            Brush::LinearGradient { p0, p1, color_stops, extend } => {
                // t is affine in the pixel's position: t = a x + b y + c.
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let len = (dx * dx + dy * dy).max(1e-12);
                let (a, b) = ((inv.xx * dx + inv.yx * dy) / len, (inv.xy * dx + inv.yy * dy) / len);
                let c = ((inv.dx - p0.x) * dx + (inv.dy - p0.y) * dy) / len;
                let lut = line(color_stops);
                paint(&mut |x, y| pick(&lut, extend, a * x + b * y + c));
            }
            Brush::RadialGradient { c0, r0, c1, r1, color_stops, extend } => {
                let lut = line(color_stops);
                paint(&mut |x, y| radial_t(apply(&inv, x, y), (c0.x, c0.y), r0, (c1.x, c1.y), r1).map_or([0.0; 4], |t| pick(&lut, extend, t)));
            }
            Brush::SweepGradient { c0, start_angle, end_angle, color_stops, extend } => {
                let lut = line(color_stops);
                let span = end_angle - start_angle;
                paint(&mut |x, y| {
                    let p = apply(&inv, x, y);
                    // Clockwise degrees in font space (y up), as skrifa hands them over.
                    let angle = (-(p.1 - c0.y).atan2(p.0 - c0.x).to_degrees()).rem_euclid(360.0);
                    pick(&lut, extend, if span.abs() > 1e-6 { (angle - start_angle) / span } else { 0.0 })
                });
            }
        }
    }
    fn push_layer(&mut self, composite_mode: CompositeMode) {
        self.layers.push((vec![[0.0; 4]; (self.w * self.h) as usize], composite_mode));
    }

    fn pop_layer(&mut self) {
        if self.layers.len() < 2 { return; }
        let Some((top, mode)) = self.layers.pop() else { return };
        let Some((under, _)) = self.layers.last_mut() else { return };
        for (d, s) in under.iter_mut().zip(&top) {
            *d = composite(mode, *s, *d);
        }
    }
}
