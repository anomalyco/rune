use std::collections::HashMap;

use skrifa::color::{
    Brush, ColorGlyphFormat, ColorPainter, ColorStop, CompositeMode, Extend, Transform as Affine,
};
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::{DrawSettings, OutlineGlyphCollection, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::raw::types::BoundingBox;
use skrifa::{GlyphId, MetadataProvider};
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, Mask, Path, PathBuilder, Pixmap, PixmapPaint, Rect,
    Transform,
};

use crate::color::{Color, srgb_decode, srgb_encode};
use crate::runtime::HashSet as FxHashSet;

use super::atlas::GlyphKey;

const MAX_SIDE: f32 = 4096.0;
const RAMP: usize = 512;
const FOREGROUND: u16 = 0xFFFF;

pub(super) struct Image {
    pub(super) rgba: Vec<u8>,
    pub(super) placement: swash::zeno::Placement,
    pub(super) foreground: bool,
}

#[derive(Default)]
pub(super) struct Foreground {
    inks: Vec<u32>,
    glyphs: FxHashSet<GlyphKey>,
}

impl Foreground {
    pub(super) fn set_inks(&mut self, element: Color, spans: &[super::Span]) {
        self.inks.clear();
        self.inks.push(pack(element));
        self.inks
            .extend(spans.iter().map(|s| pack(s.color.unwrap_or(element))));
    }

    pub(super) fn ink(&self, ink: u16) -> u32 {
        self.inks
            .get(ink as usize)
            .or(self.inks.first())
            .copied()
            .unwrap_or(pack(Color::BLACK))
    }

    pub(super) fn key(&self, key: GlyphKey, ink: u16) -> GlyphKey {
        if self.glyphs.contains(&key) {
            GlyphKey {
                ink: self.ink(ink),
                ..key
            }
        } else {
            key
        }
    }

    pub(super) fn learn(&mut self, key: GlyphKey, ink: u16) -> GlyphKey {
        self.glyphs.insert(key);
        GlyphKey {
            ink: self.ink(ink),
            ..key
        }
    }

    pub(super) fn retain<'k>(&mut self, kept: impl Iterator<Item = &'k GlyphKey>) {
        self.glyphs = kept
            .filter(|k| k.ink != 0)
            .map(|k| GlyphKey { ink: 0, ..*k })
            .collect();
    }
}

fn pack(color: Color) -> u32 {
    let [r, g, b] = [color.r, color.g, color.b]
        .map(|c| (srgb_encode(c.clamp(0.0, 1.0)) * 255.0).round() as u32);
    r << 24 | g << 16 | b << 8 | 0xFF
}

pub(super) fn rasterize(
    data: &[u8],
    index: u32,
    glyph: u32,
    ppem: f32,
    coords: &[i16],
    ink: u32,
) -> Option<Image> {
    let font = skrifa::FontRef::from_index(data, index).ok()?;
    let color = font
        .color_glyphs()
        .get_with_format(GlyphId::new(glyph), ColorGlyphFormat::ColrV1)?;
    let upem = font.head().ok()?.units_per_em();
    if upem == 0 || ppem.is_nan() || ppem <= 0.0 {
        return None;
    }
    let coords: Vec<NormalizedCoord> = coords
        .iter()
        .map(|&c| NormalizedCoord::from_bits(c))
        .collect();
    let location = LocationRef::new(&coords);
    let mut outlines = Outlines {
        glyphs: font.outline_glyphs(),
        location,
        paths: HashMap::new(),
    };
    let mut bounds = Bounds {
        outlines: &mut outlines,
        transforms: vec![Transform::identity()],
        clips: Vec::new(),
        ink: None,
    };
    color.paint(location, &mut bounds).ok()?;
    let ink_box = bounds.ink?;
    let scale = ppem / upem as f32;
    let (left, right) = ((ink_box[0] * scale).floor(), (ink_box[2] * scale).ceil());
    let (bottom, top) = ((ink_box[1] * scale).floor(), (ink_box[3] * scale).ceil());
    let (w, h) = (right - left, top - bottom);
    if !(1.0..=MAX_SIDE).contains(&w) || !(1.0..=MAX_SIDE).contains(&h) {
        return None;
    }
    let (w, h) = (w as u32, h as u32);
    let palette = font
        .color_palettes()
        .get(0)
        .map(|p| {
            p.colors()
                .iter()
                .map(|c| [c.red, c.green, c.blue, c.alpha])
                .collect()
        })
        .unwrap_or_default();
    let [r, g, b, _] = ink.to_be_bytes();
    let mut painter = Painter {
        outlines: &mut outlines,
        palette,
        foreground: [r, g, b, 255],
        used_foreground: false,
        transforms: vec![Transform::from_row(scale, 0.0, 0.0, -scale, -left, top)],
        clips: Vec::new(),
        layers: vec![Pixmap::new(w, h)?],
        size: (w, h),
    };
    color.paint(location, &mut painter).ok()?;
    let layer = painter.layers.first()?;
    let mut rgba = Vec::with_capacity(layer.data().len());
    for px in layer.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let placement = swash::zeno::Placement {
        left: left as i32,
        top: top as i32,
        width: w,
        height: h,
    };
    Some(Image {
        rgba,
        placement,
        foreground: painter.used_foreground,
    })
}

struct Outlines<'a> {
    glyphs: OutlineGlyphCollection<'a>,
    location: LocationRef<'a>,
    paths: HashMap<u32, Option<Path>>,
}

impl Outlines<'_> {
    fn path(&mut self, glyph: GlyphId) -> Option<&Path> {
        let (glyphs, location) = (&self.glyphs, self.location);
        self.paths
            .entry(glyph.to_u32())
            .or_insert_with(|| {
                let mut pen = Pen(PathBuilder::new());
                glyphs
                    .get(glyph)?
                    .draw(DrawSettings::unhinted(Size::unscaled(), location), &mut pen)
                    .ok()?;
                pen.0.finish()
            })
            .as_ref()
    }
}

struct Pen(PathBuilder);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y)
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y)
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to(cx0, cy0, x, y)
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.cubic_to(cx0, cy0, cx1, cy1, x, y)
    }
    fn close(&mut self) {
        self.0.close()
    }
}

fn affine(t: Affine) -> Transform {
    Transform::from_row(t.xx, t.yx, t.xy, t.yy, t.dx, t.dy)
}

fn box_path(clip: BoundingBox<f32>) -> Option<Path> {
    Some(PathBuilder::from_rect(Rect::from_ltrb(
        clip.x_min, clip.y_min, clip.x_max, clip.y_max,
    )?))
}

const UNBOUNDED: [f32; 4] = [
    f32::NEG_INFINITY,
    f32::NEG_INFINITY,
    f32::INFINITY,
    f32::INFINITY,
];

struct Bounds<'o, 'a> {
    outlines: &'o mut Outlines<'a>,
    transforms: Vec<Transform>,
    clips: Vec<[f32; 4]>,
    ink: Option<[f32; 4]>,
}

impl Bounds<'_, '_> {
    fn push(&mut self, path: Option<Path>) {
        let ts = *self.transforms.last().unwrap_or(&Transform::identity());
        let [l, b, r, t] = *self.clips.last().unwrap_or(&UNBOUNDED);
        let area = path
            .and_then(|p| p.transform(ts))
            .map(|p| p.bounds())
            .map_or([0.0; 4], |a| {
                [
                    a.left().max(l),
                    a.top().max(b),
                    a.right().min(r),
                    a.bottom().min(t),
                ]
            });
        self.clips.push(area);
    }
}

impl ColorPainter for Bounds<'_, '_> {
    fn push_transform(&mut self, transform: Affine) {
        let ts = *self.transforms.last().unwrap_or(&Transform::identity());
        self.transforms.push(ts.pre_concat(affine(transform)));
    }

    fn pop_transform(&mut self) {
        self.transforms.pop();
    }

    fn push_clip_glyph(&mut self, glyph: GlyphId) {
        let path = self.outlines.path(glyph).cloned();
        self.push(path);
    }

    fn push_clip_box(&mut self, clip: BoundingBox<f32>) {
        self.push(box_path(clip));
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn fill(&mut self, _brush: Brush<'_>) {
        let Some(&[l, b, r, t]) = self.clips.last() else {
            return;
        };
        if !(l < r && b < t && l.is_finite() && b.is_finite() && r.is_finite() && t.is_finite()) {
            return;
        }
        self.ink = Some(self.ink.map_or([l, b, r, t], |[il, ib, ir, it]| {
            [il.min(l), ib.min(b), ir.max(r), it.max(t)]
        }));
    }

    fn push_layer(&mut self, _mode: CompositeMode) {}
}

struct Painter<'o, 'a> {
    outlines: &'o mut Outlines<'a>,
    palette: Vec<[u8; 4]>,
    foreground: [u8; 4],
    used_foreground: bool,
    transforms: Vec<Transform>,
    clips: Vec<Option<Mask>>,
    layers: Vec<Pixmap>,
    size: (u32, u32),
}

impl Painter<'_, '_> {
    fn transform(&self) -> Transform {
        *self.transforms.last().unwrap_or(&Transform::identity())
    }

    fn push(&mut self, path: Option<Path>) {
        let (w, h) = self.size;
        let mut mask = match self.clips.last().and_then(Option::as_ref) {
            Some(mask) => mask.clone(),
            None => {
                let Some(mut mask) = Mask::new(w, h) else {
                    return self.clips.push(None);
                };
                mask.data_mut().fill(255);
                mask
            }
        };
        match path {
            Some(path) => mask.intersect_path(&path, FillRule::Winding, true, self.transform()),
            None => mask.clear(),
        }
        self.clips.push(Some(mask));
    }

    fn color(&mut self, index: u16, alpha: f32) -> [f32; 4] {
        let [r, g, b, a] = if index == FOREGROUND {
            self.used_foreground = true;
            self.foreground
        } else {
            self.palette.get(index as usize).copied().unwrap_or([0; 4])
        };
        [
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0 * alpha.clamp(0.0, 1.0),
        ]
    }

    fn ramp(&mut self, stops: &[ColorStop]) -> Vec<[u8; 4]> {
        let linear: Vec<(f32, [f32; 4])> = stops
            .iter()
            .map(|s| {
                let [r, g, b, a] = self.color(s.palette_index, s.alpha);
                (
                    s.offset,
                    [
                        srgb_decode(r) * a,
                        srgb_decode(g) * a,
                        srgb_decode(b) * a,
                        a,
                    ],
                )
            })
            .collect();
        (0..RAMP)
            .map(|i| {
                let t = i as f32 / (RAMP - 1) as f32;
                let next = linear.partition_point(|(o, _)| *o <= t);
                let c = match (next.checked_sub(1).map(|i| linear[i]), linear.get(next)) {
                    (Some((o0, c0)), Some(&(o1, c1))) if o1 > o0 => {
                        let f = (t - o0) / (o1 - o0);
                        std::array::from_fn(|k| c0[k] + (c1[k] - c0[k]) * f)
                    }
                    (Some((_, c)), _) | (None, Some(&(_, c))) => c,
                    (None, None) => [0.0; 4],
                };
                premultiplied_srgb(c)
            })
            .collect()
    }
}

fn premultiplied_srgb([r, g, b, a]: [f32; 4]) -> [u8; 4] {
    if a <= 0.0 {
        return [0; 4];
    }
    let a = a.min(1.0);
    let encode = |c: f32| (srgb_encode((c / a).clamp(0.0, 1.0)) * a * 255.0).round() as u8;
    [encode(r), encode(g), encode(b), (a * 255.0).round() as u8]
}

fn extend(t: f32, mode: Extend) -> f32 {
    match mode {
        Extend::Repeat => t - t.floor(),
        Extend::Reflect => {
            let m = t.rem_euclid(2.0);
            if m > 1.0 { 2.0 - m } else { m }
        }
        _ => t.clamp(0.0, 1.0),
    }
}

enum Shade {
    Solid([u8; 4]),
    Linear {
        p0: [f32; 2],
        d: [f32; 2],
        len2: f32,
    },
    Radial {
        c0: [f32; 2],
        r0: f32,
        cd: [f32; 2],
        dr: f32,
        a: f32,
    },
    Sweep {
        c: [f32; 2],
        start: f32,
        end: f32,
    },
}

impl Shade {
    fn at(&self, [x, y]: [f32; 2]) -> Option<f32> {
        match *self {
            Shade::Solid(_) => Some(0.0),
            Shade::Linear { p0, d, len2 } => Some(((x - p0[0]) * d[0] + (y - p0[1]) * d[1]) / len2),
            Shade::Radial { c0, r0, cd, dr, a } => {
                let (px, py) = (x - c0[0], y - c0[1]);
                let b = px * cd[0] + py * cd[1] + r0 * dr;
                let c = px * px + py * py - r0 * r0;
                let valid = |t: f32| (r0 + t * dr >= 0.0).then_some(t);
                if a.abs() < 1e-6 {
                    if b.abs() < 1e-6 {
                        None
                    } else {
                        valid(c / (2.0 * b))
                    }
                } else {
                    let disc = b * b - a * c;
                    if disc < 0.0 {
                        return None;
                    }
                    let root = disc.sqrt();
                    let (t1, t2) = ((b + root) / a, (b - root) / a);
                    valid(t1.max(t2)).or_else(|| valid(t1.min(t2)))
                }
            }
            Shade::Sweep { c, start, end } => {
                let angle = (-(y - c[1])).atan2(x - c[0]).to_degrees().rem_euclid(360.0);
                if end > start {
                    Some((angle - start) / (end - start))
                } else {
                    Some(if angle < start { 0.0 } else { 1.0 })
                }
            }
        }
    }
}

impl ColorPainter for Painter<'_, '_> {
    fn push_transform(&mut self, transform: Affine) {
        let ts = self.transform().pre_concat(affine(transform));
        self.transforms.push(ts);
    }

    fn pop_transform(&mut self) {
        self.transforms.pop();
    }

    fn push_clip_glyph(&mut self, glyph: GlyphId) {
        let path = self.outlines.path(glyph).cloned();
        self.push(path);
    }

    fn push_clip_box(&mut self, clip: BoundingBox<f32>) {
        self.push(box_path(clip));
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn fill(&mut self, brush: Brush<'_>) {
        let Some(inverse) = self.transform().invert() else {
            return;
        };
        let (shade, mode, ramp) = match brush {
            Brush::Solid {
                palette_index,
                alpha,
            } => {
                let [r, g, b, a] = self.color(palette_index, alpha);
                let premultiply = |c: f32| (c * a * 255.0).round() as u8;
                (
                    Shade::Solid([
                        premultiply(r),
                        premultiply(g),
                        premultiply(b),
                        premultiply(1.0),
                    ]),
                    Extend::Pad,
                    Vec::new(),
                )
            }
            Brush::LinearGradient {
                p0,
                p1,
                color_stops,
                extend,
            } => {
                let d = [p1.x - p0.x, p1.y - p0.y];
                let len2 = d[0] * d[0] + d[1] * d[1];
                if len2 == 0.0 {
                    return;
                }
                (
                    Shade::Linear {
                        p0: [p0.x, p0.y],
                        d,
                        len2,
                    },
                    extend,
                    self.ramp(color_stops),
                )
            }
            Brush::RadialGradient {
                c0,
                r0,
                c1,
                r1,
                color_stops,
                extend,
            } => {
                let cd = [c1.x - c0.x, c1.y - c0.y];
                let dr = r1 - r0;
                if cd == [0.0, 0.0] && dr == 0.0 {
                    return;
                }
                let a = cd[0] * cd[0] + cd[1] * cd[1] - dr * dr;
                (
                    Shade::Radial {
                        c0: [c0.x, c0.y],
                        r0,
                        cd,
                        dr,
                        a,
                    },
                    extend,
                    self.ramp(color_stops),
                )
            }
            Brush::SweepGradient {
                c0,
                start_angle,
                end_angle,
                color_stops,
                extend,
            } => (
                Shade::Sweep {
                    c: [c0.x, c0.y],
                    start: start_angle,
                    end: end_angle,
                },
                extend,
                self.ramp(color_stops),
            ),
        };
        let Some(layer) = self.layers.last_mut() else {
            return;
        };
        let clip = self.clips.last().and_then(Option::as_ref).map(Mask::data);
        let w = layer.width() as usize;
        for (i, px) in layer
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            let cover = clip.map_or(255, |c| c[i]) as u32;
            if cover == 0 {
                continue;
            }
            let src = match &shade {
                Shade::Solid(color) => *color,
                gradient => {
                    let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                    let local = [
                        inverse.sx * x + inverse.kx * y + inverse.tx,
                        inverse.ky * x + inverse.sy * y + inverse.ty,
                    ];
                    let Some(t) = gradient.at(local).filter(|t| t.is_finite()) else {
                        continue;
                    };
                    ramp[(extend(t, mode) * (RAMP - 1) as f32).round() as usize]
                }
            };
            let scale = |v: u8| (v as u32 * cover + 127) / 255;
            let alpha = scale(src[3]);
            for k in 0..4 {
                px[k] = (scale(src[k]) + (px[k] as u32 * (255 - alpha) + 127) / 255).min(255) as u8;
            }
        }
    }

    fn push_layer(&mut self, _mode: CompositeMode) {
        let (w, h) = self.size;
        self.layers.extend(Pixmap::new(w, h));
        self.clips.push(None);
    }

    fn pop_layer_with_mode(&mut self, mode: CompositeMode) {
        self.clips.pop();
        if self.layers.len() < 2 {
            return;
        }
        let Some(top) = self.layers.pop() else { return };
        let Some(below) = self.layers.last_mut() else {
            return;
        };
        let paint = PixmapPaint {
            opacity: 1.0,
            blend_mode: blend(mode),
            quality: FilterQuality::Nearest,
        };
        below.draw_pixmap(
            0,
            0,
            top.as_ref(),
            &paint,
            Transform::identity(),
            self.clips.last().and_then(Option::as_ref),
        );
    }
}

fn blend(mode: CompositeMode) -> BlendMode {
    match mode {
        CompositeMode::Clear => BlendMode::Clear,
        CompositeMode::Src => BlendMode::Source,
        CompositeMode::Dest => BlendMode::Destination,
        CompositeMode::DestOver => BlendMode::DestinationOver,
        CompositeMode::SrcIn => BlendMode::SourceIn,
        CompositeMode::DestIn => BlendMode::DestinationIn,
        CompositeMode::SrcOut => BlendMode::SourceOut,
        CompositeMode::DestOut => BlendMode::DestinationOut,
        CompositeMode::SrcAtop => BlendMode::SourceAtop,
        CompositeMode::DestAtop => BlendMode::DestinationAtop,
        CompositeMode::Xor => BlendMode::Xor,
        CompositeMode::Plus => BlendMode::Plus,
        CompositeMode::Screen => BlendMode::Screen,
        CompositeMode::Overlay => BlendMode::Overlay,
        CompositeMode::Darken => BlendMode::Darken,
        CompositeMode::Lighten => BlendMode::Lighten,
        CompositeMode::ColorDodge => BlendMode::ColorDodge,
        CompositeMode::ColorBurn => BlendMode::ColorBurn,
        CompositeMode::HardLight => BlendMode::HardLight,
        CompositeMode::SoftLight => BlendMode::SoftLight,
        CompositeMode::Difference => BlendMode::Difference,
        CompositeMode::Exclusion => BlendMode::Exclusion,
        CompositeMode::Multiply => BlendMode::Multiply,
        CompositeMode::HslHue => BlendMode::Hue,
        CompositeMode::HslSaturation => BlendMode::Saturation,
        CompositeMode::HslColor => BlendMode::Color,
        CompositeMode::HslLuminosity => BlendMode::Luminosity,
        _ => BlendMode::SourceOver,
    }
}

#[cfg(test)]
#[path = "colr_tests.rs"]
mod tests;
