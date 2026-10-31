//! Free paint helpers for rich-text block decorations, field carets, glyphs and rects.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_block_decorations(
    instances: &mut Vec<Instance>,
    text_sys: &mut TextSystem,
    images: &mut crate::pixels::ImageAtlas,
    glyph_scratch: &mut Vec<crate::text::GlyphQuad>,
    spec: &TextSpec,
    layout: &Layout<Ink>,
    origin: [f32; 2],
    (x0, x1): (f32, f32),
    s: f32,
    tk: f32,
    raster: f32,
    text_color: Color,
    o: f32,
    srgb: bool,
    clip: [f32; 4],
    fade: [f32; 4],
) {
    use crate::edit::{Block, MAX_LIST_LEVEL};
    let line_h_default = spec.style.line_px() * s * tk;
    let box_w = (x1 - x0).max(layout.full_width() * tk).max(40.0 * s * tk);
    let mut para_idx = 0usize;
    let mut first_line_of_para = true;
    let mut list_counts = [0u32; (MAX_LIST_LEVEL as usize) + 1];
    let mut code_run: Option<(f32, f32)> = None;
    let mut quote_run: Option<(f32, f32)> = None;
    let mut markers: Vec<(f32, f32, Block, u32)> = Vec::new();

    let flush_code = |instances: &mut Vec<Instance>, run: &mut Option<(f32, f32)>| {
        if let Some((top, bot)) = run.take() {
            let pad_y = (2.0 * s * tk).round();
            instances.push(rect_inst(
                [x0, top - pad_y, box_w, (bot - top) + 2.0 * pad_y],
                premul(spec.rich_style.code_block_bg, o, srgb),
                6.0 * s * tk,
                clip,
                fade,
            ));
        }
    };
    let flush_quote = |instances: &mut Vec<Instance>, run: &mut Option<(f32, f32)>| {
        if let Some((top, bot)) = run.take() {
            let bar_w = (3.0 * s * tk).round().max(2.0);
            let bar_x = (origin[0] + 2.0 * s * tk).round();
            let inset_y = (2.0 * s * tk).round();
            instances.push(rect_inst(
                [bar_x, top + inset_y, bar_w, (bot - top - 2.0 * inset_y).max(4.0 * s * tk)],
                premul(spec.rich_style.quote_bar, o, srgb),
                bar_w * 0.5,
                clip,
                fade,
            ));
        }
    };

    let mut had_lines = false;
    for line in layout.lines() {
        had_lines = true;
        let m = line.metrics();
        let blk = spec.blocks.get(para_idx).copied().unwrap_or(Block::Paragraph);
        let is_soft = spec.soft_breaks.binary_search(&(para_idx as u32)).is_ok();
        let top = origin[1] + m.block_min_coord * tk;
        let bot = if m.block_max_coord > m.block_min_coord {
            origin[1] + m.block_max_coord * tk
        } else {
            top + line_h_default
        };
        let baseline = origin[1] + m.baseline * tk;

        if blk == Block::Code {
            flush_quote(instances, &mut quote_run);
            match &mut code_run {
                Some((_, b)) => *b = bot,
                None => code_run = Some((top, bot)),
            }
        } else if blk == Block::Quote {
            flush_code(instances, &mut code_run);
            match &mut quote_run {
                Some((_, b)) => *b = bot,
                None => quote_run = Some((top, bot)),
            }
        } else {
            flush_code(instances, &mut code_run);
            flush_quote(instances, &mut quote_run);
        }

        if first_line_of_para {
            match blk {
                Block::Bullet(lvl) => {
                    let l = (lvl.min(MAX_LIST_LEVEL)) as usize;
                    for c in &mut list_counts[l + 1..] {
                        *c = 0;
                    }
                    list_counts[l] = 0;
                    if !is_soft {
                        let indent = (blk.indent_pt(spec.style.size) * s).round();
                        markers.push((indent, baseline, blk, 0));
                    }
                }
                Block::Numbered(lvl) => {
                    let l = (lvl.min(MAX_LIST_LEVEL)) as usize;
                    for c in &mut list_counts[l + 1..] {
                        *c = 0;
                    }
                    if !is_soft {
                        list_counts[l] += 1;
                        let indent = (blk.indent_pt(spec.style.size) * s).round();
                        markers.push((indent, baseline, blk, list_counts[l]));
                    }
                }
                Block::Paragraph | Block::Quote | Block::Code => {
                    if !is_soft {
                        list_counts = [0; (MAX_LIST_LEVEL as usize) + 1];
                    }
                }
            }
        }

        if line.break_reason() == parley::BreakReason::Explicit {
            para_idx += 1;
            first_line_of_para = true;
        } else {
            first_line_of_para = false;
        }
    }

    if !had_lines
        && let Some(&blk) = spec.blocks.first()
    {
        let top = origin[1];
        let bot = top + line_h_default;
        let baseline = top + spec.style.size * 0.8 * s * tk;
        match blk {
            Block::Code => code_run = Some((top, bot)),
            Block::Quote => quote_run = Some((top, bot)),
            Block::Bullet(_) => {
                let indent = (blk.indent_pt(spec.style.size) * s).round();
                markers.push((indent, baseline, blk, 0));
            }
            Block::Numbered(_) => {
                let indent = (blk.indent_pt(spec.style.size) * s).round();
                markers.push((indent, baseline, blk, 1));
            }
            Block::Paragraph => {}
        }
    }

    flush_code(instances, &mut code_run);
    flush_quote(instances, &mut quote_run);

    if !markers.is_empty() {
        let marker_color = spec.rich_style.marker.unwrap_or_else(|| text_color.alpha(0.68));
        let mc = premul(marker_color, o, srgb);
        for (indent_px, baseline_y, blk, num) in markers {
            match blk {
                Block::Bullet(lvl) => {
                    let d = (4.5 * s * tk).round().max(3.0);
                    let cx = origin[0] + (indent_px - (7.0 * s).round()) * tk - d;
                    let cy = (baseline_y - 0.34 * spec.style.size * s * tk - d * 0.5).round();
                    let radius = if lvl >= 2 { 1.0 * s * tk } else { d * 0.5 };
                    let c = if lvl == 1 { premul(marker_color.alpha(marker_color.a * 0.65), o, srgb) } else { mc };
                    instances.push(rect_inst([cx.round(), cy, d, d], c, radius, clip, fade));
                }
                Block::Numbered(_) => {
                    let label = format!("{num}.");
                    let ml = text_sys.layout_rich(&label, &[], spec.style, s, None);
                    let mw = ml.full_width();
                    let m_base = ml.lines().next().map_or(spec.style.size * 0.8 * s, |l| l.metrics().baseline);
                    let mx = origin[0] + (indent_px - (5.0 * s).round() - mw) * tk;
                    let my = baseline_y - m_base * tk;
                    let m_origin = [mx.round(), my.round()];
                    glyph_scratch.clear();
                    text_sys.set_glyph_inks(marker_color, &[]);
                    text_sys.glyphs_for_layout_scaled(&ml, m_origin, raster, false, glyph_scratch);
                    for q in &*glyph_scratch {
                        instances.extend(
                            glyph_or_image(q, text_sys, images, mc, clip, fade)
                                .map(|g| zoom_glyph(g, m_origin, m_origin, tk / raster)),
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// Pixel caret and logical-point IME area use the same shaped cursor geometry.
pub(super) fn input_caret(n: &Node, r: [f32; 4], scale: f32, s: f32, scrolls: &mut HashMap<String, f32>) -> Option<([f32; 4], [f32; 4])> {
    let (spec, layout, input) = (n.text.as_ref()?, n.text_layout.as_ref()?, n.input.as_ref()?);
    let e = &input.edit;
    let ins = n.inset.map(|v| v * scale * s);
    let (x0, x1) = (r[0] + ins[0], r[2] - ins[2]);
    // The text scales with its box, so the caret does too.
    let mut x = text_x(spec, x0, x1, (layout.full_width() * scale).ceil());
    if input.single_line { x -= single_line_scroll(scrolls, &n.path, input, &spec.content, layout, s, (x1 - x0) / scale.max(1e-4)) * scale; }
    let ci = caret_index(e, &spec.content);
    let cur = Cursor::from_byte_index(layout, ci, Affinity::Downstream);
    let bb = crate::text::cursor_geometry(&cur, layout, 1.5 * s);
    let (cx, cy) = (x + bb.x0 as f32 * scale, r[1] + ins[1] + bb.y0 as f32 * scale);
    let h = (if bb.y1 > bb.y0 { (bb.y1 - bb.y0) as f32 } else { spec.style.line_px() * s }) * scale;
    Some(([cx.round(), cy + h * 0.1, (1.5 * s * scale).round().max(1.0), h * 0.8], [cx / s, cy / s, 2.0 * scale, h / s]))
}

/// Whether `looping`'s sampled value multiplies the element's opacity.
pub(super) fn fades(looping: Option<rune_motion::LoopSpec>) -> bool {
    matches!(looping.map(|l| l.kind), Some(rune_motion::LoopKind::Opacity { .. } | rune_motion::LoopKind::OpacityCycle(_) | rune_motion::LoopKind::Sequence(_)))
}

/// `rect` (x, y, w, h) magnified `k` times about `origin`.
pub(super) fn scale_about(rect: [f32; 4], origin: [f32; 2], k: f32) -> [f32; 4] {
    [origin[0] + (rect[0] - origin[0]) * k, origin[1] + (rect[1] - origin[1]) * k, rect[2] * k, rect[3] * k]
}

/// A glyph instance drawn `k` times its rasterized size about `origin`. An alpha glyph keeps
/// its atlas footprint in `uv.zw`, so the shader samples it filtered.
pub(super) fn zoom_glyph(mut g: Instance, origin: [f32; 2], r_origin: [f32; 2], k: f32) -> Instance {
    if k == 1.0 && origin == r_origin { return g; }
    if g.params[1] == KIND_GLYPH && g.uv[2] == 0.0 { (g.uv[2], g.uv[3]) = (g.rect[2], g.rect[3]); }
    g.rect = [
        origin[0] + (g.rect[0] - r_origin[0]) * k,
        origin[1] + (g.rect[1] - r_origin[1]) * k,
        g.rect[2] * k,
        g.rect[3] * k,
    ];
    g
}

/// A text glyph's instance: alpha-atlas coverage tinted by `fill`, or a colour glyph's image
/// at `fill`'s alpha. None when the image atlas can't place it.
pub(super) fn glyph_or_image(q: &GlyphQuad, text: &TextSystem, images: &mut crate::pixels::ImageAtlas, fill: [f32; 4], clip: [f32; 4], fade: [f32; 4]) -> Option<Instance> {
    if q.color == 0 {
        return Some(glyph(q, fill, clip, fade));
    }
    let image = images.place(text.color_glyph(q.color), [q.rect[2], q.rect[3]])?;
    Some(Instance {
        rect: q.rect,
        radii: [0.0; 4],
        fill: [0.0, 0.0, 0.0, fill[3]],
        border: [0.0; 4],
        params: [image.page as f32, KIND_IMAGE, image.premultiplied as u8 as f32, 1.0],
        uv: image.rect.map(|v| v as f32),
        clip,
        fade: crate::render::pack_fade(fade, clip, clip),
    })
}

pub(super) fn rect_inst(rect: [f32; 4], fill: [f32; 4], radius: f32, clip: [f32; 4], fade: [f32; 4]) -> Instance {
    Instance {
        rect,
        radii: [radius; 4],
        fill,
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip,
        fade: crate::render::pack_fade(fade, clip, clip),
    }
}
