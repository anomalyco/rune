//! Pure-Rust LaTeX display math for closed ```` ```latex ```` and ```` ```math ```` fences in
//! [`crate::Markdown`] (`anomalyco/opencode#64380`), backed by RaTeX (`ratex-parser`,
//! `ratex-layout`, `ratex-types`, `ratex-katex-fonts`), `skrifa`, and `tiny-skia`.
//!
//! Zero JS or WASM runtime; works on both native and `wasm32-unknown-unknown`. Hard bounds on
//! source length, nesting depth, macro expansion, display-list item count, and canvas pixel area
//! guard against adversarial inputs; any parse error, unsupported construct, or exceeded cap
//! returns `None` so [`crate::Markdown`] falls back cleanly to a highlighted code block.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::LazyLock;

use ratex_font::{FontId, katex_ttf_glyph_char};
use ratex_layout::LayoutOptions;
use ratex_types::color::Color as RatexColor;
use ratex_types::display_item::{DisplayItem, DisplayList};
use ratex_types::path_command::PathCommand;
use rune::{Color, Direction, El, Pixels, col, el, row};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::markdown::MarkdownStyle;

/// KaTeX's `.katex` default CSS scale relative to surrounding body text (`1.21em`).
const KATEX_FONT_SCALE: f32 = 1.21;
/// Hard cap on raw LaTeX source length accepted for math rendering (16 KiB).
const MAX_INPUT_BYTES: usize = 16 * 1024;
/// Hard pre-parse cap on `{...}` / `\left...\right` / `\begin...\end` nesting depth.
const MAX_NESTING_DEPTH: usize = 32;
/// Hard cap on emitted `DisplayItem`s before rasterisation.
const MAX_DISPLAY_ITEMS: usize = 4_096;
/// Hard caps on logical equation dimensions in points.
const MAX_LOGICAL_WIDTH: f32 = 4_096.0;
const MAX_LOGICAL_HEIGHT: f32 = 1_024.0;
/// Hard cap on rasterised pixel count (~16 MiB RGBA max per equation).
const MAX_CANVAS_PIXELS: u64 = 4_000_000;
/// Bounded per-thread cache limits for rendered equation bitmaps.
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
/// Vertical padding (points) around a display equation (`katex-display`).
const DISPLAY_PAD_Y: f32 = 6.0;
/// Safety margin (points) around the rasterised ink bounds for antialiasing / curve overshoot.
const RASTER_MARGIN_PT: f32 = 2.0;

/// Whether a code fence's language (first word of its info string) names display math
/// (`isMathFence` in `packages/ui/src/context/marked-parser.tsx`).
pub fn is_math_fence(lang: Option<&str>) -> bool {
    lang.is_some_and(|l| l.eq_ignore_ascii_case("latex") || l.eq_ignore_ascii_case("math"))
}

/// Renders `src` as display math styled from `style`, returning `None` when `src` is empty,
/// exceeds hard safety caps, or fails to parse/layout so the caller can fall back to code.
pub fn render(src: &str, style: &MarkdownStyle) -> Option<El> {
    let (_, _, _, text_width) = style.math_params();
    render_with_avail(src, style, text_width)
}

pub(crate) fn render_with_avail(src: &str, style: &MarkdownStyle, avail_width: Option<f32>) -> Option<El> {
    let trimmed = src.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_INPUT_BYTES || !within_nesting_cap(trimmed) {
        return None;
    }
    let (body_size, ink, math_scale, _) = style.math_params();
    let em_pt = (body_size * KATEX_FONT_SCALE).clamp(8.0, 48.0);
    let scale = math_scale.clamp(1.0, 3.0);
    let cached = render_cached(trimmed, em_pt, ink, scale)?;

    let img = el()
        .key("math")
        .size(cached.width, cached.height)
        .pixels(&cached.pixels)
        .role("img")
        .label(trimmed.to_owned());

    let fits = avail_width.is_some_and(|w| w > 0.0 && cached.width <= w);
    if fits {
        return Some(
            row()
                .key("math-block")
                .dir(Direction::Ltr)
                .w_full()
                .center()
                .py(DISPLAY_PAD_Y)
                .child(img),
        );
    }

    let inner = row().w_full().min_w(cached.width).center().py(DISPLAY_PAD_Y).child(img);
    let viewport = col().key("scroll").w_full().scroll_x().child(inner);
    Some(col().key("math-block").dir(Direction::Ltr).w_full().child(viewport))
}

/// Fast pre-parse scan guarding against deeply nested braces or environments before entering the
/// recursive descent parser.
fn within_nesting_cap(src: &str) -> bool {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                // Skip escaped single char or inspect `\left` / `\right` / `\begin` / `\end`.
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                if !bytes[i].is_ascii_alphabetic() {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                match &bytes[start..i] {
                    b"left" | b"begin" => {
                        depth += 1;
                        if depth > MAX_NESTING_DEPTH {
                            return false;
                        }
                    }
                    b"right" | b"end" => {
                        depth = depth.saturating_sub(1);
                    }
                    _ => {}
                }
            }
            b'{' => {
                depth += 1;
                if depth > MAX_NESTING_DEPTH {
                    return false;
                }
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'%' => {
                // Skip TeX line comment to newline.
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    true
}

#[derive(Clone)]
struct CachedMath {
    pixels: Pixels,
    width: f32,
    height: f32,
    bytes: usize,
}

struct CacheSlot {
    math: Option<CachedMath>,
    bytes: usize,
    tick: u64,
}

#[derive(Default)]
struct MathCache {
    entries: HashMap<u64, CacheSlot>,
    total_bytes: usize,
    tick: u64,
}

thread_local! {
    static CACHE: RefCell<MathCache> = RefCell::new(MathCache::default());
}

fn cache_key(src: &str, em_pt: f32, ink: Color, scale: f32) -> u64 {
    let mut h = DefaultHasher::new();
    (src, em_pt.to_bits(), color_key(ink), scale.to_bits()).hash(&mut h);
    h.finish()
}

fn color_key(c: Color) -> [u8; 4] {
    [
        (c.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.a.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

fn render_cached(src: &str, em_pt: f32, ink: Color, scale: f32) -> Option<CachedMath> {
    let key = cache_key(src, em_pt, ink, scale);
    let cached = CACHE.with_borrow_mut(|c| {
        c.tick += 1;
        let tick = c.tick;
        c.entries.get_mut(&key).map(|slot| {
            slot.tick = tick;
            slot.math.clone()
        })
    });
    if let Some(result) = cached {
        return result;
    }

    let built = build_math(src, em_pt, ink, scale);
    CACHE.with_borrow_mut(|c| {
        c.tick += 1;
        let bytes = built.as_ref().map_or(0, |m| m.bytes);
        let tick = c.tick;
        c.total_bytes += bytes;
        c.entries.insert(key, CacheSlot { math: built.clone(), bytes, tick });
        while c.entries.len() > MAX_CACHE_ENTRIES || c.total_bytes > MAX_CACHE_BYTES {
            let Some(&victim) = c.entries.iter().min_by_key(|(_, v)| v.tick).map(|(k, _)| k) else {
                break;
            };
            if let Some(removed) = c.entries.remove(&victim) {
                c.total_bytes = c.total_bytes.saturating_sub(removed.bytes);
            }
        }
    });
    built
}

fn build_math(src: &str, em_pt: f32, ink: Color, scale: f32) -> Option<CachedMath> {
    let ast = ratex_parser::parse(src).ok()?;
    let opts = LayoutOptions::default().with_color(RatexColor::new(ink.r, ink.g, ink.b, ink.a));
    let lbox = ratex_layout::layout(&ast, &opts);
    ratex_layout::validate_layout(&lbox).ok()?;
    let dl = ratex_layout::to_display_list(&lbox);
    if dl.items.is_empty() || dl.items.len() > MAX_DISPLAY_ITEMS {
        return None;
    }
    if !dl.width.is_finite() || !dl.height.is_finite() || !dl.depth.is_finite() {
        return None;
    }
    let total_h_em = dl.height + dl.depth;
    if dl.width <= 0.0 || total_h_em <= 0.0 {
        return None;
    }

    let logical_w = (dl.width as f32 * em_pt + 2.0 * RASTER_MARGIN_PT).ceil();
    let logical_h = (total_h_em as f32 * em_pt + 2.0 * RASTER_MARGIN_PT).ceil();
    if logical_w <= 0.0
        || logical_h <= 0.0
        || logical_w > MAX_LOGICAL_WIDTH
        || logical_h > MAX_LOGICAL_HEIGHT
    {
        return None;
    }

    let px_w = (logical_w * scale).ceil().max(1.0) as u32;
    let px_h = (logical_h * scale).ceil().max(1.0) as u32;
    if u64::from(px_w) * u64::from(px_h) > MAX_CANVAS_PIXELS {
        return None;
    }

    let rgba = rasterize(&dl, em_pt * scale, RASTER_MARGIN_PT * scale, px_w, px_h)?;
    let bytes = rgba.len();
    let pixels = Pixels::new();
    pixels.set(px_w, px_h, rgba);
    Some(CachedMath { pixels, width: logical_w, height: logical_h, bytes })
}

// ---------------------------------------------------------------------------------------------
// KaTeX font cache & DisplayList rasterisation (`skrifa` + `tiny-skia`)

const KATEX_FONT_FILES: [(&str, &str); 19] = [
    ("AMS-Regular", "KaTeX_AMS-Regular.ttf"),
    ("Caligraphic-Regular", "KaTeX_Caligraphic-Regular.ttf"),
    ("Caligraphic-Bold", "KaTeX_Caligraphic-Bold.ttf"),
    ("Fraktur-Regular", "KaTeX_Fraktur-Regular.ttf"),
    ("Fraktur-Bold", "KaTeX_Fraktur-Bold.ttf"),
    ("Main-Bold", "KaTeX_Main-Bold.ttf"),
    ("Main-BoldItalic", "KaTeX_Main-BoldItalic.ttf"),
    ("Main-Italic", "KaTeX_Main-Italic.ttf"),
    ("Main-Regular", "KaTeX_Main-Regular.ttf"),
    ("Math-BoldItalic", "KaTeX_Math-BoldItalic.ttf"),
    ("Math-Italic", "KaTeX_Math-Italic.ttf"),
    ("SansSerif-Bold", "KaTeX_SansSerif-Bold.ttf"),
    ("SansSerif-Italic", "KaTeX_SansSerif-Italic.ttf"),
    ("SansSerif-Regular", "KaTeX_SansSerif-Regular.ttf"),
    ("Script-Regular", "KaTeX_Script-Regular.ttf"),
    ("Size1-Regular", "KaTeX_Size1-Regular.ttf"),
    ("Size2-Regular", "KaTeX_Size2-Regular.ttf"),
    ("Size3-Regular", "KaTeX_Size3-Regular.ttf"),
    ("Size4-Regular", "KaTeX_Size4-Regular.ttf"),
];

const TYPEWRITER_FONT: (&str, &str) = ("Typewriter-Regular", "KaTeX_Typewriter-Regular.ttf");

struct FontSlot {
    name: &'static str,
    font: Option<FontRef<'static>>,
}

static KATEX_FONTS: LazyLock<Vec<FontSlot>> = LazyLock::new(|| {
    KATEX_FONT_FILES
        .iter()
        .chain(std::iter::once(&TYPEWRITER_FONT))
        .map(|&(name, file)| {
            let font = match ratex_katex_fonts::ttf_bytes(file) {
                Some(Cow::Borrowed(bytes)) => FontRef::new(bytes).ok(),
                _ => None,
            };
            FontSlot { name, font }
        })
        .collect()
});

fn katex_font(name: &str) -> Option<&'static FontRef<'static>> {
    let fonts = &*KATEX_FONTS;
    fonts
        .iter()
        .find(|slot| slot.name == name)
        .and_then(|slot| slot.font.as_ref())
        .or_else(|| fonts.iter().find(|slot| slot.name == "Main-Regular").and_then(|slot| slot.font.as_ref()))
}

struct SkiaPen {
    pb: PathBuilder,
    ox: f32,
    oy: f32,
}

impl OutlinePen for SkiaPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(self.ox + x, self.oy - y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(self.ox + x, self.oy - y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.pb.quad_to(self.ox + cx0, self.oy - cy0, self.ox + x, self.oy - y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.pb.cubic_to(
            self.ox + cx0,
            self.oy - cy0,
            self.ox + cx1,
            self.oy - cy1,
            self.ox + x,
            self.oy - y,
        );
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

fn paint_of(c: RatexColor) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(
        (c.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.a.clamp(0.0, 1.0) * 255.0).round() as u8,
    );
    paint
}

fn rasterize(dl: &DisplayList, em_px: f32, margin_px: f32, px_w: u32, px_h: u32) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(px_w, px_h)?;
    let em = f64::from(em_px);
    let pad = f64::from(margin_px);

    for item in &dl.items {
        match item {
            DisplayItem::GlyphPath { x, y, scale, font, char_code, color } => {
                let Some(ch) = char::from_u32(*char_code) else { continue };
                if ch.is_whitespace() {
                    continue;
                }
                let mapped = FontId::parse(font)
                    .map(|fid| katex_ttf_glyph_char(fid, *char_code))
                    .unwrap_or(ch);
                let Some(font_ref) = katex_font(font) else { continue };
                let (font_ref, gid) = match font_ref.charmap().map(mapped) {
                    Some(g) => (font_ref, g),
                    None => {
                        let Some(main) = katex_font("Main-Regular") else { continue };
                        let Some(g) = main.charmap().map(mapped).or_else(|| main.charmap().map(ch)) else {
                            continue;
                        };
                        (main, g)
                    }
                };
                let ppem = (*scale * em) as f32;
                if !ppem.is_finite() || ppem <= 0.1 {
                    continue;
                }
                let Some(outline) = font_ref.outline_glyphs().get(gid) else { continue };
                let mut pen = SkiaPen {
                    pb: PathBuilder::new(),
                    ox: (*x * em + pad) as f32,
                    oy: (*y * em + pad) as f32,
                };
                if outline
                    .draw(DrawSettings::unhinted(Size::new(ppem), LocationRef::default()), &mut pen)
                    .is_err()
                {
                    continue;
                }
                if let Some(path) = pen.pb.finish() {
                    let paint = paint_of(*color);
                    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                }
            }
            DisplayItem::Line { x, y, width, thickness, color, dashed } => {
                let lx = (*x * em + pad) as f32;
                let lw = (*width * em) as f32;
                let th = ((*thickness * em) as f32).max(1.0);
                let ly = (*y * em + pad) as f32 - th * 0.5;
                if !lx.is_finite() || !ly.is_finite() || !lw.is_finite() || lw <= 0.0 {
                    continue;
                }
                let paint = paint_of(*color);
                if *dashed {
                    let dash = (4.0 * th).max(2.0);
                    let period = 2.0 * dash;
                    let mut cx = 0.0f32;
                    while cx < lw {
                        let seg = dash.min(lw - cx);
                        if let Some(rect) = Rect::from_xywh(lx + cx, ly, seg, th) {
                            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                        }
                        cx += period;
                    }
                } else if let Some(rect) = Rect::from_xywh(lx, ly, lw, th) {
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
            DisplayItem::Rect { x, y, width, height, color } => {
                let rx = (*x * em + pad) as f32;
                let ry = (*y * em + pad) as f32;
                let rw = (*width * em) as f32;
                let rh = (*height * em) as f32;
                if let Some(rect) = Rect::from_xywh(rx, ry, rw, rh) {
                    let paint = paint_of(*color);
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
            DisplayItem::Path { x, y, commands, fill, color } => {
                let ox = *x * em + pad;
                let oy = *y * em + pad;
                let mut pb = PathBuilder::new();
                for cmd in commands {
                    match *cmd {
                        PathCommand::MoveTo { x: px, y: py } => pb.move_to((ox + px * em) as f32, (oy + py * em) as f32),
                        PathCommand::LineTo { x: px, y: py } => pb.line_to((ox + px * em) as f32, (oy + py * em) as f32),
                        PathCommand::QuadTo { x1, y1, x: px, y: py } => pb.quad_to(
                            (ox + x1 * em) as f32,
                            (oy + y1 * em) as f32,
                            (ox + px * em) as f32,
                            (oy + py * em) as f32,
                        ),
                        PathCommand::CubicTo { x1, y1, x2, y2, x: px, y: py } => pb.cubic_to(
                            (ox + x1 * em) as f32,
                            (oy + y1 * em) as f32,
                            (ox + x2 * em) as f32,
                            (oy + y2 * em) as f32,
                            (ox + px * em) as f32,
                            (oy + py * em) as f32,
                        ),
                        PathCommand::Close => pb.close(),
                    }
                }
                if let Some(path) = pb.finish() {
                    let paint = paint_of(*color);
                    if *fill {
                        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                    } else {
                        let stroke = Stroke { width: (0.04 * em_px).max(1.0), ..Stroke::default() };
                        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                    }
                }
            }
        }
    }

    // `tiny-skia::Pixmap` stores premultiplied sRGB RGBA; `rune::Pixels::set` takes straight-alpha sRGB RGBA.
    let src = pixmap.data();
    let mut out = vec![0u8; src.len()];
    for (dst_px, src_px) in out.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let a = src_px[3];
        if a == 0 {
            continue;
        }
        if a == 255 {
            dst_px.copy_from_slice(src_px);
            continue;
        }
        let a16 = u16::from(a);
        let half = a16 / 2;
        dst_px[0] = ((u16::from(src_px[0]) * 255 + half) / a16).min(255) as u8;
        dst_px[1] = ((u16::from(src_px[1]) * 255 + half) / a16).min(255) as u8;
        dst_px[2] = ((u16::from(src_px[2]) * 255 + half) / a16).min(255) as u8;
        dst_px[3] = a;
    }
    Some(out)
}
