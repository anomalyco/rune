//! Backgrounds, borders and per-corner radii for padded [`SpanBox`] ranges.
use crate::*;

/// Emits one [`PaintedDeco`] per line fragment of each painted [`SpanBoxRun`] in `layout`
/// (device px, layout top-left at `origin`). Distinct boxes are never merged across adjacent
/// ranges, and horizontal edges snap cleanly so adjacent chips (such as `from:` + `ada`) meet
/// without overlapping or leaving a gap.
#[doc(hidden)]
pub fn span_box_decorations(
    layout: &Layout<Ink>,
    span_boxes: &[SpanBoxRun],
    size: f32,
    scale: f32,
    origin: [f32; 2],
    out: &mut Vec<PaintedDeco>,
) {
    if span_boxes.is_empty() {
        return;
    }
    for (i, sb) in span_boxes.iter().enumerate() {
        if !sb.style.is_painted() {
            continue;
        }
        let (a, b) = (sb.start as usize, sb.end as usize);
        if a >= b {
            continue;
        }
        let boxes = range_boxes(layout, a, b, false);
        if boxes.is_empty() {
            continue;
        }
        let lead_id = SPAN_BOX | (i as u64) << 1;
        let trail_id = lead_id | 1;
        let fill = sb.style.bg_color().unwrap_or(Color::TRANSPARENT);
        let ring = sb.style.border_ring().map(|(c, w)| (c, w * scale));
        let raw_corners = sb.style.corner_radii().map(|r| r * scale);
        for (bb, line_idx) in boxes {
            let Some(line) = layout.get(line_idx) else { continue };
            let (gx0, gx1) = (bb.x0 as f32, bb.x1 as f32);
            if gx1 <= gx0 {
                continue;
            }
            let (mut x0, mut x1) = (gx0, gx1);
            let mut lead_box = None;
            let mut trail_box = None;
            let (mut asc, mut desc) = (0.0f32, 0.0f32);
            for item in line.items() {
                match item {
                    PositionedLayoutItem::InlineBox(ib) if ib.id == lead_id => lead_box = Some(ib),
                    PositionedLayoutItem::InlineBox(ib) if ib.id == trail_id => trail_box = Some(ib),
                    PositionedLayoutItem::GlyphRun(gr) => {
                        let r = gr.run().text_range();
                        if r.start < b && a < r.end {
                            let fm = gr.run().font_metrics();
                            (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                        }
                    }
                    _ => {}
                }
            }
            if let Some(ref lb) = lead_box {
                if (lb.x + lb.width - gx0).abs() < 1.5 {
                    x0 = x0.min(lb.x);
                } else if (lb.x - gx1).abs() < 1.5 {
                    x1 = x1.max(lb.x + lb.width);
                }
            }
            if let Some(tb) = trail_box {
                if (tb.x - gx1).abs() < 1.5 {
                    x1 = x1.max(tb.x + tb.width);
                } else if (tb.x + tb.width - gx0).abs() < 1.5 {
                    x0 = x0.min(tb.x);
                }
            }
            if asc == 0.0 {
                (asc, desc) = (size * scale * 0.95, size * scale * 0.25);
            }
            let m = line.metrics();
            let base = origin[1] + m.baseline;
            let top = (base - asc - 1.0 * scale).round();
            let bot = (base + desc + 1.0 * scale).round();
            let l = (origin[0] + x0).round();
            let r = (origin[0] + x1).round();
            let rect = [l, top, (r - l).max(0.0), (bot - top).max(0.0)];
            if rect[2] <= 0.0 || rect[3] <= 0.0 {
                continue;
            }
            let lr = line.text_range();
            let has_start = lr.start <= a && a < lr.end;
            let has_end = lr.start < b && b <= lr.end;
            let rtl = layout.is_rtl();
            let (keep_left, keep_right) = if has_start && has_end {
                (true, true)
            } else if rtl {
                (has_end, has_start)
            } else {
                (has_start, has_end)
            };
            let half = rect[2].min(rect[3]) * 0.5;
            let corners = [
                if keep_left { raw_corners[0].min(half) } else { 0.0 },
                if keep_right { raw_corners[1].min(half) } else { 0.0 },
                if keep_right { raw_corners[2].min(half) } else { 0.0 },
                if keep_left { raw_corners[3].min(half) } else { 0.0 },
            ];
            let byte = lr.end.min(b).saturating_sub(1) as u32;
            out.push(PaintedDeco::with_corners(
                Deco { rect, color: fill, radius: 0.0 },
                ring,
                corners,
                byte,
            ));
        }
    }
}

#[cfg(test)]
#[path = "box_decos_tests.rs"]
mod tests;
