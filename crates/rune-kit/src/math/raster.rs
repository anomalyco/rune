//! Host-side KaTeX font rasteriser (`skrifa` + `tiny-skia`) for validated `RM01` display lists.
//!
//! - Zero work at startup: the deflated 20-font KaTeX pack inflates once on the first math
//!   rasterisation, and each individual TTF is parsed with `skrifa::FontRef` lazily on first use.
//! - Hard texture and memory bounds: `px_w <= 4096`, `px_h <= 4096` (safe on both `ImageAtlas`
//!   8192 and WebGL2 `max_texture_dimension_2d = 4096`), `px_w * px_h <= 1_048_576` (4 MiB max
//!   RGBA against an 8 MiB LRU cache, preventing self-eviction / whole-cache wipes).
//! - All geometry and dashes are clipped to canvas bounds with a hard dash cap (`MAX_DASHES`).

use std::sync::OnceLock;

use rune::{Color, Pixels};
use skrifa::outline::DrawSettings;
use skrifa::prelude::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use super::wire::{DisplayItem, DisplayList, PathCmd};

/// Maximum bitmap width or height in physical pixels (safe on both native 8192 atlas and WebGL2 4096).
pub(super) const MAX_TEXTURE_DIM: u32 = 4096;
/// Maximum total pixels in one math bitmap (1 Mi px = 4 MiB RGBA <= half of the 8 MiB cache).
pub(super) const MAX_CANVAS_PIXELS: u64 = 1_048_576;
/// Maximum logical bounds in points before scaling.
const MAX_LOGICAL_WIDTH: f32 = 4096.0;
const MAX_LOGICAL_HEIGHT: f32 = 1024.0;
/// Maximum dashed segments drawn for any single `\hdashline` / proof-tree rule.
const MAX_DASHES: usize = 2048;
/// Safety padding around rasterised outlines in logical points.
const RASTER_MARGIN_PT: f32 = 2.0;
/// KaTeX display math uses `1.21em` relative to surrounding body font (`katex.css`).
const KATEX_EM_RATIO: f32 = 1.21;
/// Hard ceiling on inflating `katex-fonts.pack.deflate` (actual pack is 514,049 bytes).
const MAX_FONT_PACK_BYTES: usize = 1024 * 1024;

static KATEX_FONTS_PACK_DEFLATE: &[u8] = include_bytes!("../../assets/math/katex-fonts.pack.deflate");

const FONT_NAMES: [&str; 20] = [
    "AMS-Regular",
    "Caligraphic-Bold",
    "Caligraphic-Regular",
    "Fraktur-Bold",
    "Fraktur-Regular",
    "Main-Bold",
    "Main-BoldItalic",
    "Main-Italic",
    "Main-Regular",
    "Math-BoldItalic",
    "Math-Italic",
    "SansSerif-Bold",
    "SansSerif-Italic",
    "SansSerif-Regular",
    "Script-Regular",
    "Size1-Regular",
    "Size2-Regular",
    "Size3-Regular",
    "Size4-Regular",
    "Typewriter-Regular",
];

const MAIN_REGULAR_IDX: usize = 8;

struct FontStore {
    pack: Vec<u8>,
    ranges: [Option<(usize, usize)>; 20],
    parsed: [OnceLock<Option<FontRef<'static>>>; 20],
}

static FONTS: OnceLock<Option<FontStore>> = OnceLock::new();

fn font_index(name: &str) -> Option<usize> {
    let stripped = name.strip_prefix("KaTeX_").unwrap_or(name);
    let stripped = stripped.strip_suffix(".ttf").unwrap_or(stripped);
    FONT_NAMES.iter().position(|&n| n == stripped)
}

fn load_font_store() -> Option<FontStore> {
    let pack = miniz_oxide::inflate::decompress_to_vec_with_limit(
        KATEX_FONTS_PACK_DEFLATE,
        MAX_FONT_PACK_BYTES,
    )
    .ok()?;
    if pack.len() < 8 || &pack[..4] != b"KF01" {
        return None;
    }
    let count = u32::from_le_bytes([pack[4], pack[5], pack[6], pack[7]]) as usize;
    if count != FONT_NAMES.len() {
        return None;
    }
    let mut pos = 8usize;
    let mut ranges = [None; 20];
    for _ in 0..count {
        let name_len = *pack.get(pos)? as usize;
        pos = pos.checked_add(1)?;
        let name_end = pos.checked_add(name_len)?;
        let name = std::str::from_utf8(pack.get(pos..name_end)?).ok()?;
        pos = name_end;
        let len_end = pos.checked_add(4)?;
        let lb = pack.get(pos..len_end)?;
        let data_len = u32::from_le_bytes([lb[0], lb[1], lb[2], lb[3]]) as usize;
        pos = len_end;
        let data_end = pos.checked_add(data_len)?;
        if data_end > pack.len() {
            return None;
        }
        if let Some(idx) = font_index(name) {
            ranges[idx] = Some((pos, data_end));
        }
        pos = data_end;
    }
    Some(FontStore {
        pack,
        ranges,
        parsed: std::array::from_fn(|_| OnceLock::new()),
    })
}

fn font_store() -> Option<&'static FontStore> {
    FONTS.get_or_init(load_font_store).as_ref()
}

fn font_at(store: &'static FontStore, idx: usize) -> Option<&'static FontRef<'static>> {
    store.parsed[idx]
        .get_or_init(|| {
            let (start, end) = store.ranges[idx]?;
            FontRef::new(&store.pack[start..end]).ok()
        })
        .as_ref()
}

fn resolve_font(store: &'static FontStore, name: &str) -> Option<&'static FontRef<'static>> {
    if let Some(idx) = font_index(name)
        && let Some(f) = font_at(store, idx)
    {
        return Some(f);
    }
    font_at(store, MAIN_REGULAR_IDX)
}

struct TinyPen {
    builder: PathBuilder,
    ox: f32,
    oy: f32,
}

impl skrifa::outline::OutlinePen for TinyPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(self.ox + x, self.oy - y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(self.ox + x, self.oy - y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder.quad_to(self.ox + cx0, self.oy - cy0, self.ox + x, self.oy - y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder.cubic_to(
            self.ox + cx0,
            self.oy - cy0,
            self.ox + cx1,
            self.oy - cy1,
            self.ox + x,
            self.oy - y,
        );
    }
    fn close(&mut self) {
        self.builder.close();
    }
}

fn to_paint(item_color: [u8; 4], default_ink: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.anti_alias = true;
    // RaTeX leaves unstyled items at opaque black (0, 0, 0, 255); recolour those to theme `ink`
    // while preserving explicit `\color{...}` colours.
    let [r, g, b, a] = if item_color == [0, 0, 0, 255] {
        [
            (default_ink.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (default_ink.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (default_ink.b.clamp(0.0, 1.0) * 255.0).round() as u8,
            (default_ink.a.clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    } else {
        item_color
    };
    paint.set_color_rgba8(r, g, b, a);
    paint
}

/// Rasterised equation ready for GPU upload, with exact `1 / scale` logical dimensions.
#[derive(Clone)]
pub(super) struct Rasterized {
    pub(super) pixels: Pixels,
    pub(super) width_pt: f32,
    pub(super) height_pt: f32,
    pub(super) byte_len: usize,
}

/// Rasterises a validated `DisplayList` into straight-alpha RGBA `Pixels`.
/// Returns `None` if dimensions exceed host texture/pixel caps or a printable glyph is unsupported.
pub(super) fn rasterize(dl: &DisplayList, body_pt: f32, ink: Color, scale: f32) -> Option<Rasterized> {
    let em_pt = (body_pt * KATEX_EM_RATIO).clamp(8.0, 72.0);
    let scale = scale.clamp(0.5, 3.0);
    let logical_w = dl.width * em_pt + 2.0 * RASTER_MARGIN_PT;
    let logical_h = (dl.height + dl.depth) * em_pt + 2.0 * RASTER_MARGIN_PT;
    if !logical_w.is_finite()
        || !logical_h.is_finite()
        || logical_w <= 0.0
        || logical_h <= 0.0
        || logical_w > MAX_LOGICAL_WIDTH
        || logical_h > MAX_LOGICAL_HEIGHT
    {
        return None;
    }
    let px_w = (logical_w * scale).ceil() as u32;
    let px_h = (logical_h * scale).ceil() as u32;
    if px_w == 0
        || px_h == 0
        || px_w > MAX_TEXTURE_DIM
        || px_h > MAX_TEXTURE_DIM
        || (px_w as u64).saturating_mul(px_h as u64) > MAX_CANVAS_PIXELS
    {
        return None;
    }

    let store = font_store()?;
    let mut pixmap = Pixmap::new(px_w, px_h)?;
    let em_px = em_pt * scale;
    let margin_px = RASTER_MARGIN_PT * scale;
    let baseline_y = margin_px + dl.height * em_px;
    let canvas_w = px_w as f32;
    let canvas_h = px_h as f32;

    for item in &dl.items {
        match item {
            DisplayItem::Glyph { x, y, scale: g_scale, char_code, color, font } => {
                let ch = char::from_u32(*char_code)?;
                // Skip whitespace codepoints; fail cleanly on unmapped printable glyphs (e.g. CJK/emoji)
                // so the block falls back to highlighted code instead of leaving blank holes.
                if ch.is_whitespace() {
                    continue;
                }
                let primary = resolve_font(store, font)?;
                let (face, gid) = match primary.charmap().map(*char_code) {
                    Some(g) => (primary, g),
                    None => {
                        let fallback = font_at(store, MAIN_REGULAR_IDX)?;
                        let g = fallback.charmap().map(*char_code)?;
                        (fallback, g)
                    }
                };
                let gx = margin_px + x * em_px;
                let gy = baseline_y + y * em_px;
                if gx < -em_px * 4.0 || gx > canvas_w + em_px * 4.0 || gy < -em_px * 4.0 || gy > canvas_h + em_px * 4.0 {
                    continue;
                }
                let font_px = (g_scale * em_px).clamp(1.0, 512.0);
                let Some(outline) = face.outline_glyphs().get(gid) else {
                    continue;
                };
                let mut pen = TinyPen { builder: PathBuilder::new(), ox: gx, oy: gy };
                let settings = DrawSettings::unhinted(Size::new(font_px), LocationRef::default());
                if outline.draw(settings, &mut pen).is_err() {
                    continue;
                }
                if let Some(path) = pen.builder.finish() {
                    let paint = to_paint(*color, ink);
                    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                }
            }
            DisplayItem::Line { x, y, width, thickness, color, dashed } => {
                let lx = margin_px + x * em_px;
                let lw = width * em_px;
                let lh = (thickness * em_px).max(1.0);
                let ly = baseline_y + y * em_px - lh * 0.5;
                // Clip strictly to canvas bounds before iterating dashes (prevents off-canvas
                // and f32-ULP infinite loops on huge `\hdashline` / `\rule` widths).
                let x0 = lx.max(0.0);
                let x1 = (lx + lw).min(canvas_w);
                let y0 = ly.max(0.0);
                let y1 = (ly + lh).min(canvas_h);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let paint = to_paint(*color, ink);
                if *dashed {
                    let dash = (0.18 * em_px).max(2.0);
                    let gap = (0.12 * em_px).max(1.5);
                    let period = dash + gap;
                    let start_k = ((x0 - lx) / period).floor().max(0.0);
                    let mut cx = lx + start_k * period;
                    for _ in 0..MAX_DASHES {
                        if cx >= x1 {
                            break;
                        }
                        let seg0 = cx.max(x0);
                        let seg1 = (cx + dash).min(x1);
                        if seg1 > seg0
                            && let Some(rect) = Rect::from_xywh(seg0, y0, seg1 - seg0, y1 - y0)
                        {
                            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                        }
                        cx += period;
                    }
                } else if let Some(rect) = Rect::from_xywh(x0, y0, x1 - x0, y1 - y0) {
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
            DisplayItem::Rect { x, y, width, height, color } => {
                let rx = margin_px + x * em_px;
                let ry = baseline_y + y * em_px;
                let rw = width * em_px;
                let rh = height * em_px;
                let x0 = rx.max(0.0);
                let y0 = ry.max(0.0);
                let x1 = (rx + rw).min(canvas_w);
                let y1 = (ry + rh).min(canvas_h);
                if x1 > x0
                    && y1 > y0
                    && let Some(rect) = Rect::from_xywh(x0, y0, x1 - x0, y1 - y0)
                {
                    let paint = to_paint(*color, ink);
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
            DisplayItem::Path { x, y, fill, color, commands } => {
                let ox = margin_px + x * em_px;
                let oy = baseline_y + y * em_px;
                let mut pb = PathBuilder::new();
                for &cmd in commands {
                    match cmd {
                        PathCmd::MoveTo(px, py) => pb.move_to(ox + px * em_px, oy + py * em_px),
                        PathCmd::LineTo(px, py) => pb.line_to(ox + px * em_px, oy + py * em_px),
                        PathCmd::QuadTo(x1, y1, px, py) => pb.quad_to(
                            ox + x1 * em_px,
                            oy + y1 * em_px,
                            ox + px * em_px,
                            oy + py * em_px,
                        ),
                        PathCmd::CubicTo(x1, y1, x2, y2, px, py) => pb.cubic_to(
                            ox + x1 * em_px,
                            oy + y1 * em_px,
                            ox + x2 * em_px,
                            oy + y2 * em_px,
                            ox + px * em_px,
                            oy + py * em_px,
                        ),
                        PathCmd::Close => pb.close(),
                    }
                }
                if let Some(path) = pb.finish() {
                    let paint = to_paint(*color, ink);
                    if *fill {
                        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                    } else {
                        let stroke = Stroke {
                            width: (0.04 * em_px).max(1.0),
                            line_cap: LineCap::Butt,
                            line_join: LineJoin::Miter,
                            ..Stroke::default()
                        };
                        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                    }
                }
            }
        }
    }

    // Unpremultiply tiny-skia's premultiplied RGBA into straight RGBA8 expected by `rune::Pixels`.
    let src = pixmap.data();
    let byte_len = src.len();
    let mut out = vec![0u8; byte_len];
    for (s, d) in src.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let a = s[3];
        if a == 0 {
            continue;
        }
        if a == 255 {
            d.copy_from_slice(s);
        } else {
            let af = f32::from(a);
            d[0] = ((f32::from(s[0]) * 255.0 / af).round() as u32).min(255) as u8;
            d[1] = ((f32::from(s[1]) * 255.0 / af).round() as u32).min(255) as u8;
            d[2] = ((f32::from(s[2]) * 255.0 / af).round() as u32).min(255) as u8;
            d[3] = a;
        }
    }

    let pixels = Pixels::new();
    pixels.set(px_w, px_h, out);
    // Exact `1 / scale` logical dimensions guarantee a 1:1 texel-to-device-pixel draw at 1x/1.5x/2x/3x.
    Some(Rasterized {
        pixels,
        width_pt: px_w as f32 / scale,
        height_pt: px_h as f32 / scale,
        byte_len,
    })
}
