use super::*;

fn expected_strike(line: &parley::Line<'_, Ink>, run_idx: usize, origin_y: f32, scale: f32) -> (f32, f32) {
    let gr = line
        .items()
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(gr) => Some(gr),
            PositionedLayoutItem::InlineBox(_) => None,
        })
        .nth(run_idx)
        .expect("expected glyph run");
    let fm = gr.run().font_metrics();
    let off = if fm.strikethrough_offset != 0.0 {
        fm.strikethrough_offset
    } else {
        fm.ascent * 0.32
    };
    let y = (origin_y + line.metrics().baseline - off).round();
    let h = fm.strikethrough_size.max(scale).round().max(1.0);
    (y, h)
}

#[test]
fn strike_decorations_uses_struck_runs_metrics_not_first_run() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [10.0, 20.0];

    // Large leading run (40 pt) followed by body-size (14 pt) struck text.
    let text = "Big then struck text";
    let spans = [Span::new(0, 3).size(40.0)];
    let layout = ts.layout_rich(text, &spans, TextStyle::new(14.0, 400.0), scale, None);
    let line = layout.get(0).expect("single line");
    let (big_y, big_h) = expected_strike(&line, 0, origin[1], scale);
    let (body_y, body_h) = expected_strike(&line, 1, origin[1], scale);
    assert_ne!(big_y, body_y, "40 pt and 14 pt runs should have distinct strike y");
    assert_ne!(big_h, body_h, "40 pt and 14 pt runs should have distinct strike thickness");

    let mut out = Vec::new();
    ts.strike_decorations(&layout, &spans, &[(4, text.len() as u32, None)], Color::WHITE, scale, origin, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].rect[1], body_y, "strike y should follow the struck 14 pt run, not the 40 pt leading run");
    assert_eq!(out[0].rect[3], body_h, "strike thickness should follow the struck 14 pt run, not the 40 pt leading run");

    // Tiny leading run (6 pt) followed by larger (32 pt) struck text.
    let text2 = "tiny BIG struck";
    let spans2 = [Span::new(0, 4).size(6.0), Span::new(5, text2.len()).size(32.0)];
    let layout2 = ts.layout_rich(text2, &spans2, TextStyle::new(14.0, 400.0), scale, None);
    let line2 = layout2.get(0).expect("single line");
    let (tiny_y, tiny_h) = expected_strike(&line2, 0, origin[1], scale);
    let struck_idx = line2
        .items()
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(gr) => Some(gr),
            PositionedLayoutItem::InlineBox(_) => None,
        })
        .position(|gr| {
            let r = gr.run().text_range();
            r.start < text2.len() && 5 < r.end
        })
        .expect("run overlapping struck range");
    let (large_y, large_h) = expected_strike(&line2, struck_idx, origin[1], scale);
    assert_ne!(tiny_y, large_y, "6 pt and 32 pt runs should have distinct strike y");
    assert_ne!(tiny_h, large_h, "6 pt and 32 pt runs should have distinct strike thickness");

    out.clear();
    ts.strike_decorations(&layout2, &spans2, &[(5, text2.len() as u32, None)], Color::WHITE, scale, origin, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].rect[1], large_y, "strike y should follow the struck 32 pt run, not the 6 pt leading run");
    assert_eq!(out[0].rect[3], large_h, "strike thickness should follow the struck 32 pt run, not the 6 pt leading run");
}

#[test]
fn strike_decorations_per_box_metrics_when_pill_splits_a_line() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [0.0, 0.0];
    let text = "Big @tag small";
    let spans = [Span::new(0, 3).size(40.0)];
    let pills = [PillChip { start: 4, end: 8, icon: None, bg: None, fg: None, bare: None }];
    let layout = ts.layout_blocks(text, &spans, &[], &pills, TextStyle::new(14.0, 400.0), scale, None, Alignment::Start);
    let line = layout.get(0).expect("single line");
    let (big_y, big_h) = expected_strike(&line, 0, origin[1], scale);
    let small_idx = line
        .items()
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(gr) => Some(gr),
            PositionedLayoutItem::InlineBox(_) => None,
        })
        .position(|gr| gr.run().text_range().end == text.len())
        .expect("trailing 14 pt run");
    let (small_y, small_h) = expected_strike(&line, small_idx, origin[1], scale);
    assert_ne!(big_y, small_y);

    let mut out = Vec::new();
    ts.strike_decorations(&layout, &spans, &[(0, text.len() as u32, None)], Color::WHITE, scale, origin, &mut out);
    assert!(out.len() >= 2, "pill inline boxes split the struck line into separate boxes");
    assert_eq!((out[0].rect[1], out[0].rect[3]), (big_y, big_h));
    let last = out.last().unwrap();
    assert_eq!((last.rect[1], last.rect[3]), (small_y, small_h));
}

#[test]
fn underlined_long_line_scans_items_once_and_matches_underline_edges() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [12.5, 24.0];
    let src = "A🦀".repeat(400);
    let spans = [Span::new(0, src.len()).underline(Color::BLACK)];
    let layout = ts.layout_rich(&src, &spans, TextStyle::new(14.0, 400.0), scale, None);
    let line = layout.get(0).expect("single unwrapped line");
    let run_count = line
        .items()
        .filter(|item| matches!(item, PositionedLayoutItem::GlyphRun(_)))
        .count();
    assert!(run_count >= 400, "alternating Latin and emoji should split into many runs (got {run_count})");

    let _ = super::super::underline::take_line_end_item_visits();
    let mut out = Vec::new();
    ts.decorations(&layout, &spans, scale, origin, &mut out);
    let visits = super::super::underline::take_line_end_item_visits();

    assert_eq!(out.len(), run_count, "each underlined run emits one underline rect");
    assert!(
        visits <= run_count,
        "decorations should scan line items once for line end, not per run (got {visits} visits for {run_count} runs)"
    );

    // Every underline rect must match the per-run `underline_edges` calculation exactly.
    let y = (origin[1] + line.metrics().baseline + 2.0 * scale).round();
    let h = scale.max(1.0);
    let mut expected = Vec::new();
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
        let (l, r) = underline_edges(&line, origin[0], gr.offset(), gr.offset() + gr.advance());
        expected.push(([l, y, r - l, h], Color::BLACK, 0.0f32));
    }
    let actual: Vec<_> = out.iter().map(|d| (d.rect, d.color, d.radius)).collect();
    assert_eq!(actual, expected, "underline rects must match underline_edges identically");

    // A background-only span on the same long line does not scan line items for underline_end at all.
    let bg_spans = [Span::new(0, src.len()).bg(Color::BLACK, 2.0)];
    let bg_layout = ts.layout_rich(&src, &bg_spans, TextStyle::new(14.0, 400.0), scale, None);
    let _ = super::super::underline::take_line_end_item_visits();
    out.clear();
    ts.decorations(&bg_layout, &bg_spans, scale, origin, &mut out);
    assert_eq!(super::super::underline::take_line_end_item_visits(), 0);
}

#[test]
fn underline_and_strikethrough_preserve_positions_and_thickness() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [7.25, 19.5];
    let text = "Big 🦀 middle small";
    let spans = [
        Span::new(0, 3).size(36.0).underline(Color::BLACK),
        Span::new(4, text.len()).underline(Color::WHITE),
    ];
    let layout = ts.layout_rich(text, &spans, TextStyle::new(14.0, 400.0), scale, None);
    let line = layout.get(0).expect("single line");
    let y_ul = (origin[1] + line.metrics().baseline + 2.0 * scale).round();

    let mut decos = Vec::new();
    ts.decorations(&layout, &spans, scale, origin, &mut decos);
    let mut expected_ul = Vec::new();
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
        let ink = gr.style().brush.0 & !crate::raster::synthetic_bold::SYNTHETIC_BOLD;
        if ink == 0 {
            continue;
        }
        let s = &spans[ink as usize - 1];
        if let Some(uc) = s.underline {
            let (l, r) = underline_edges(&line, origin[0], gr.offset(), gr.offset() + gr.advance());
            expected_ul.push(([l, y_ul, r - l, scale.max(1.0)], uc));
        }
    }
    let actual_ul: Vec<_> = decos.iter().map(|d| (d.rect, d.color)).collect();
    assert_eq!(actual_ul, expected_ul);

    let mut strikes = Vec::new();
    ts.strike_decorations(&layout, &spans, &[(0, 3, None), (4, text.len() as u32, None)], Color::BLACK, scale, origin, &mut strikes);
    assert_eq!(strikes.len(), 2);
    let (big_y, big_h) = expected_strike(&line, 0, origin[1], scale);
    assert_eq!((strikes[0].rect[1], strikes[0].rect[3]), (big_y, big_h));
    let tail_idx = line
        .items()
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(gr) => Some(gr),
            PositionedLayoutItem::InlineBox(_) => None,
        })
        .position(|gr| gr.run().text_range().end == text.len())
        .expect("trailing 14 pt run");
    let (tail_y, tail_h) = expected_strike(&line, tail_idx, origin[1], scale);
    assert_eq!((strikes[1].rect[1], strikes[1].rect[3]), (tail_y, tail_h));
}

#[test]
fn layout_span_indexes_do_not_collide_at_32768_or_overflow_at_65536() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let bg_color = Color { r: 0.2, g: 0.4, b: 0.6, a: 1.0 };
    let ul_color = Color { r: 0.8, g: 0.1, b: 0.2, a: 1.0 };

    // 32,767 preceding layout-affecting spans put the next two at ordinals 0x8000 and 0x8001,
    // where a 16-bit brush collides with SYNTHETIC_BOLD (0x8000).
    let mut spans = vec![Span::new(100, 101).underline(Color::BLACK); 32_767];
    spans.push(Span::new(0, 1).bg(bg_color, 2.0));
    spans.push(Span::new(1, 2).underline(ul_color));
    let layout = ts.layout_rich("AB", &spans, style, 1.0, None);

    // Neither span asks for bold; Face::of must not read ordinal 0x8000 as synthetic bold.
    let mut coords = Vec::new();
    let line = layout.get(0).expect("single line");
    for item in line.items() {
        if let PositionedLayoutItem::GlyphRun(gr) = item {
            let face = Face::of(&gr, &mut coords);
            assert!(!face.bold, "ordinal 32,768 must not set synthetic bold on a non-bold run");
        }
    }

    let mut decos = Vec::new();
    ts.decorations(&layout, &spans, 1.0, [0.0, 0.0], &mut decos);
    assert_eq!(decos.len(), 2, "both spans at ordinals 32,768 and 32,769 must emit decorations");
    assert_eq!(decos[0].color, bg_color, "span 32,768 must keep its own background colour");
    assert_eq!(decos[1].color, ul_color, "span 32,769 must keep its own underline colour");

    // synthetic_bold::inks must also keep ordinals 32,768 and 32,769 intact alongside the flag.
    let bold_inks = crate::raster::synthetic_bold::inks(&[0..2], &spans);
    assert_eq!(bold_inks.len(), 2);
    assert_eq!(bold_inks[0].1 & crate::raster::synthetic_bold::SYNTHETIC_BOLD, crate::raster::synthetic_bold::SYNTHETIC_BOLD);
    assert_eq!((bold_inks[0].1 & !crate::raster::synthetic_bold::SYNTHETIC_BOLD) as usize, 32_768);
    assert_eq!((bold_inks[1].1 & !crate::raster::synthetic_bold::SYNTHETIC_BOLD) as usize, 32_769);

    // Past 65,535 layout-affecting spans must neither overflow nor wrap.
    let mut spans64k = vec![Span::new(100, 101).underline(Color::BLACK); 65_535];
    spans64k.push(Span::new(0, 1).bg(bg_color, 2.0));
    spans64k.push(Span::new(1, 2).underline(ul_color));
    let layout64k = ts.layout_rich("AB", &spans64k, style, 1.0, None);
    decos.clear();
    ts.decorations(&layout64k, &spans64k, 1.0, [0.0, 0.0], &mut decos);
    assert_eq!(decos.len(), 2, "both spans at ordinals 65,536 and 65,537 must emit decorations");
    assert_eq!(decos[0].color, bg_color);
    assert_eq!(decos[1].color, ul_color);

    let bold_inks64k = crate::raster::synthetic_bold::inks(&[0..2], &spans64k);
    assert_eq!(bold_inks64k.len(), 2);
    assert_eq!((bold_inks64k[0].1 & !crate::raster::synthetic_bold::SYNTHETIC_BOLD) as usize, 65_536);
    assert_eq!((bold_inks64k[1].1 & !crate::raster::synthetic_bold::SYNTHETIC_BOLD) as usize, 65_537);
}

fn deco_snapshot(out: &[PaintedDeco]) -> Vec<([f32; 4], Color, f32, Option<(Color, f32)>, u32)> {
    out.iter().map(|d| (d.rect, d.color, d.radius, d.ring, d.byte)).collect()
}

fn reference_decorations_with_rings(
    layout: &Layout<Ink>,
    spans: &[Span],
    rings: &[(u32, u32, Color, f32)],
    radius: f32,
    scale: f32,
    origin: [f32; 2],
    out: &mut Vec<PaintedDeco>,
) {
    if spans.iter().all(|s| s.bg.is_none() && s.underline.is_none()) {
        return;
    }
    let layout_spans: Vec<&Span> = super::super::span::layout_spans(spans).collect();
    let radius = radius * scale;
    let mut last_bg = None;
    for line in layout.lines() {
        let m = line.metrics();
        let mut line_end = None;
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
            let ink = gr.style().brush.0 & !crate::raster::synthetic_bold::SYNTHETIC_BOLD;
            if ink == 0 {
                continue;
            }
            let Some(&s) = layout_spans.get(ink as usize - 1) else { continue };
            let x0 = origin[0] + gr.offset();
            let w = gr.advance();
            if let Some(bg) = s.bg {
                let ring = rings
                    .iter()
                    .rev()
                    .find(|&&(ra, rb, _, rw)| rw > 0.0 && ra < s.end && s.start < rb)
                    .map(|&(_, _, c, rw)| (c, rw * scale));
                let pad = s.bg_pad * scale;
                let fm = gr.run().font_metrics();
                let cap = fm.cap_height.unwrap_or(gr.run().font_size() * 0.7);
                let base = origin[1] + m.baseline;
                let (top, bot) = if s.mono {
                    (base.round() - fm.ascent.round(), base.round() + fm.descent.round())
                } else {
                    ((base - cap * 1.35).round(), (base + cap * 0.55).round())
                };
                let (l, r) = ((x0 - pad).round(), (x0 + w + pad).round());
                let rect = [l, top, r - l, bot - top];
                let byte = (gr.run().text_range().end as u32).min(s.end).saturating_sub(1);
                let merge = last_bg.is_some_and(|i: usize| {
                    let (d, old_ring) = out[i].paint();
                    d.color == bg
                        && old_ring == ring
                        && (d.radius - radius).abs() < 1e-3
                        && d.rect[1] < bot
                        && top < d.rect[1] + d.rect[3]
                        && (d.rect[0] + d.rect[2] - rect[0] - 2.0 * pad).abs() < 1.5
                });
                if merge {
                    let painted = &mut out[last_bg.unwrap()];
                    let (d, _) = painted.paint();
                    if rect[2] > d.rect[2] {
                        (d.rect[1], d.rect[3]) = (rect[1], rect[3]);
                    }
                    d.rect[2] = rect[0] + rect[2] - d.rect[0];
                    painted.extend_to(byte);
                } else {
                    last_bg = Some(out.len());
                    out.push(PaintedDeco::new(Deco { rect, color: bg, radius }, ring, byte));
                }
            }
            if let Some(uc) = s.underline.filter(|c| c.a > 0.0) {
                let y = (origin[1] + m.baseline + 2.0 * scale).round();
                let end = *line_end.get_or_insert_with(|| super::super::underline::line_end(&line));
                let (l, r) = super::super::underline::underline_edges_with_end(end, origin[0], gr.offset(), gr.offset() + w);
                out.push(PaintedDeco::new(Deco { rect: [l, y, r - l, scale.max(1.0)], color: uc, radius: 0.0 }, None, 0));
            }
        }
    }
}

fn reference_strike_decorations(
    layout: &Layout<Ink>,
    spans: &[Span],
    strikes: &[(u32, u32, Option<Color>)],
    default_color: Color,
    scale: f32,
    origin: [f32; 2],
    out: &mut Vec<PaintedDeco>,
) {
    for &(s_start, s_end, sc) in strikes {
        let (a, b) = (s_start as usize, s_end as usize);
        if a >= b {
            continue;
        }
        let span_color = sc.or_else(|| {
            spans
                .iter()
                .find(|sp| (sp.start as usize) <= a && b <= (sp.end as usize) && sp.color.is_some())
                .and_then(|sp| sp.color)
        });
        let color = span_color.unwrap_or(default_color);
        for (bb, line_idx) in range_boxes(layout, a, b, false) {
            let w = (bb.x1 - bb.x0) as f32;
            if w <= 0.0 {
                continue;
            }
            let Some(line) = layout.get(line_idx) else { continue };
            let m = line.metrics();
            let mut strike_off = m.line_height * 0.25;
            let mut h = scale.round().max(1.0);
            let (bx0, bx1) = (bb.x0 as f32, bb.x1 as f32);
            let mut best_overlap: Option<f32> = None;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
                let run = gr.run().text_range();
                if run.end <= a || b <= run.start {
                    continue;
                }
                let overlap = (gr.offset() + gr.advance()).min(bx1) - gr.offset().max(bx0);
                if best_overlap.is_none_or(|prev| overlap > prev) {
                    best_overlap = Some(overlap);
                    let fm = gr.run().font_metrics();
                    strike_off = if fm.strikethrough_offset != 0.0 {
                        fm.strikethrough_offset
                    } else {
                        fm.ascent * 0.32
                    };
                    h = fm.strikethrough_size.max(scale).round().max(1.0);
                }
            }
            let x0 = origin[0] + bb.x0 as f32;
            let y = (origin[1] + m.baseline - strike_off).round();
            out.push(PaintedDeco::new(Deco { rect: [x0, y, w, h], color, radius: 0.0 }, None, 0));
        }
    }
}

fn reference_push_highlight_boxes(
    layout: &Layout<Ink>,
    boxes: Vec<(parley::BoundingBox, usize)>,
    range: std::ops::Range<usize>,
    origin: [f32; 2],
    pad: f32,
    vpad: f32,
    radius: f32,
    color: Color,
    ring: Option<(Color, f32)>,
    byte: u32,
    blank: Option<[f32; 3]>,
    out: &mut Vec<PaintedDeco>,
) {
    let mut last: Option<(usize, usize)> = None;
    for (bb, line_idx) in boxes {
        let w = (bb.x1 - bb.x0) as f32;
        if w <= 0.0 {
            continue;
        }
        let x0 = origin[0] + bb.x0 as f32;
        let (mut top, mut bot) = ((origin[1] + bb.y0 as f32).round(), (origin[1] + bb.y1 as f32).round());
        if let Some(l) = layout.get(line_idx) {
            let base = origin[1] + l.metrics().baseline;
            let (mut asc, mut desc) = (0.0f32, 0.0f32);
            for run in l.runs() {
                let r = run.text_range();
                if (r.start < range.end && range.start < r.end) || (range.is_empty() && r.end == range.end) {
                    let fm = run.font_metrics();
                    (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                }
            }
            if asc > 0.0 || desc > 0.0 {
                top = (base - asc - vpad).round();
                bot = (base + desc + vpad).round();
            } else if let Some(b) = blank {
                let bbase = origin[1] + b[0];
                top = (bbase - b[1] - vpad).round();
                bot = (bbase + b[2] + vpad).round();
            }
        } else if let Some(b) = blank {
            let bbase = origin[1] + b[0];
            top = (bbase - b[1] - vpad).round();
            bot = (bbase + b[2] + vpad).round();
        }
        let rect = [x0 - pad, top, w + 2.0 * pad, (bot - top).max(0.0)];
        let merge = last.filter(|&(li, _)| li == line_idx).is_some_and(|(_, idx)| {
            let (d, _) = out[idx].paint();
            (d.rect[0] + d.rect[2] - rect[0] - 2.0 * pad).abs() < 1.0
        });
        if merge {
            let (d, _) = out[last.unwrap().1].paint();
            if rect[3] > d.rect[3] {
                (d.rect[1], d.rect[3]) = (rect[1], rect[3]);
            }
            d.rect[2] = rect[0] + rect[2] - d.rect[0];
        } else {
            last = Some((line_idx, out.len()));
            out.push(PaintedDeco::new(Deco { rect, color, radius }, ring, byte));
        }
    }
}

fn reference_highlight_decorations(
    layout: &Layout<Ink>,
    highlights: &[TextHighlight],
    len: usize,
    scale: f32,
    origin: [f32; 2],
    out: &mut Vec<PaintedDeco>,
) {
    for h in highlights {
        let (a, b) = ((h.start as usize).min(len), (h.end as usize).min(len));
        if a >= b || (h.bg.a <= 0.0 && h.ring.is_none()) {
            continue;
        }
        let pad = h.pad * scale;
        let radius = h.radius * scale;
        let vpad = h.vpad * scale;
        let ring = h.ring.map(|(c, rw)| (c, rw * scale));
        let byte = (b as u32).saturating_sub(1);
        let boxes = range_boxes(layout, a, b, false);
        reference_push_highlight_boxes(layout, boxes, a..b, origin, pad, vpad, radius, h.bg, ring, byte, None, out);
    }
}

#[test]
fn decorations_ring_lookup_scales_linearly_with_runs_and_rings() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [11.25, 23.5];
    let bg = Color { r: 0.2, g: 0.3, b: 0.4, a: 1.0 };

    // Case 1: n bg spans (n glyph runs) with n rings on one line.
    let measure_disjoint = |ts: &mut TextSystem, n: usize| {
        let mut src = String::new();
        let mut spans = Vec::with_capacity(n);
        let mut rings = Vec::with_capacity(n);
        for i in 0..n {
            if i > 0 {
                src.push(' ');
            }
            let a = src.len();
            src.push_str("ab");
            let b = src.len();
            let sp = Span::new(a, b).bg(bg, 2.0);
            spans.push(if i % 2 == 0 { sp.bold() } else { sp });
            let rc = Color { r: (i % 7) as f32 * 0.1, g: 0.5, b: 0.8, a: 1.0 };
            rings.push((a as u32, b as u32, rc, 1.0));
        }
        let layout = ts.layout_rich(&src, &spans, TextStyle::new(14.0, 400.0), scale, None);
        let _ = take_deco_ring_visits();
        let mut got = Vec::new();
        ts.decorations_with_rings(&layout, &spans, &rings, 4.0, scale, origin, &mut got);
        let visits = take_deco_ring_visits();
        let mut expected = Vec::new();
        reference_decorations_with_rings(&layout, &spans, &rings, 4.0, scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "disjoint rings must match reference");
        visits
    };

    // Case 2: 1 wide bg span split into n runs with n rings inside it.
    let measure_wide_span = |ts: &mut TextSystem, n: usize| {
        let src = "A🦀".repeat(n);
        let spans = vec![Span::new(0, src.len()).bg(bg, 2.0)];
        let step = "A🦀".len() as u32;
        let rings: Vec<_> = (0..n)
            .map(|i| {
                let a = i as u32 * step;
                let rc = Color { r: (i % 5) as f32 * 0.2, g: 0.4, b: 0.9, a: 1.0 };
                (a, a + 1, rc, 1.0)
            })
            .collect();
        let layout = ts.layout_rich(&src, &spans, TextStyle::new(14.0, 400.0), scale, None);
        let _ = take_deco_ring_visits();
        let mut got = Vec::new();
        ts.decorations_with_rings(&layout, &spans, &rings, 4.0, scale, origin, &mut got);
        let visits = take_deco_ring_visits();
        let mut expected = Vec::new();
        reference_decorations_with_rings(&layout, &spans, &rings, 4.0, scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "wide span rings must match reference");
        visits
    };

    let d60 = measure_disjoint(&mut ts, 60);
    let d120 = measure_disjoint(&mut ts, 120);
    let w60 = measure_wide_span(&mut ts, 60);
    let w120 = measure_wide_span(&mut ts, 120);
    assert!(
        d120 * 10 <= d60 * 25,
        "decorations_into ring visits for n runs + n rings should scale ~2x, not ~4x (d60={d60}, d120={d120})"
    );
    assert!(
        w120 * 10 <= w60 * 25,
        "decorations_into ring visits for wide span with n runs + n rings should scale ~2x, not ~4x (w60={w60}, w120={w120})"
    );
}

#[test]
fn strike_decorations_scales_linearly_with_struck_ranges_and_boxes() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [11.25, 23.5];

    // Case 1: n distinct struck ranges on one line (with n styled spans/runs on that line).
    let measure_many_strikes = |ts: &mut TextSystem, n: usize| {
        let mut src = String::new();
        let mut spans = Vec::with_capacity(n);
        let mut strikes = Vec::with_capacity(n);
        for i in 0..n {
            if i > 0 {
                src.push(' ');
            }
            let a = src.len();
            src.push_str("ab");
            let b = src.len();
            let c = Color { r: (i % 6) as f32 * 0.15, g: 0.3, b: 0.7, a: 1.0 };
            let sp = Span::new(a, b).color(c);
            spans.push(if i % 2 == 0 { sp.size(18.0) } else { sp.size(12.0) });
            strikes.push((a as u32, b as u32, None));
        }
        let layout = ts.layout_rich(&src, &spans, TextStyle::new(14.0, 400.0), scale, None);
        let _ = take_strike_visits();
        let mut got = Vec::new();
        ts.strike_decorations(&layout, &spans, &strikes, Color::WHITE, scale, origin, &mut got);
        let visits = take_strike_visits();
        let mut expected = Vec::new();
        reference_strike_decorations(&layout, &spans, &strikes, Color::WHITE, scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "many-strikes must match reference");
        visits
    };

    // Case 2: 1 struck range spanning a line split by n pills into n+1 boxes.
    let measure_pill_split = |ts: &mut TextSystem, n: usize| {
        let mut src = String::new();
        let mut spans = Vec::with_capacity(n);
        let mut pills = Vec::with_capacity(n);
        for i in 0..n {
            let ws = src.len();
            src.push_str("ab");
            let we = src.len();
            if i % 2 == 0 {
                spans.push(Span::new(ws, we).size(20.0));
            }
            src.push(' ');
            let ps = src.len() as u32;
            src.push('p');
            let pe = src.len() as u32;
            pills.push(PillChip { start: ps, end: pe, icon: None, bg: None, fg: None, bare: None });
            if i + 1 < n {
                src.push(' ');
            }
        }
        let layout = ts.layout_blocks(&src, &spans, &[], &pills, TextStyle::new(14.0, 400.0), scale, None, Alignment::Start);
        let strikes = vec![(0u32, src.len() as u32, Some(Color::BLACK))];
        let _ = take_strike_visits();
        let mut got = Vec::new();
        ts.strike_decorations(&layout, &spans, &strikes, Color::WHITE, scale, origin, &mut got);
        let visits = take_strike_visits();
        let mut expected = Vec::new();
        reference_strike_decorations(&layout, &spans, &strikes, Color::WHITE, scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "pill-split strike must match reference");
        visits
    };

    let (mi60, ms60) = measure_many_strikes(&mut ts, 60);
    let (mi120, ms120) = measure_many_strikes(&mut ts, 120);
    let (pi60, _) = measure_pill_split(&mut ts, 60);
    let (pi120, _) = measure_pill_split(&mut ts, 120);
    assert!(
        mi120 * 10 <= mi60 * 25,
        "strike_decorations item visits for n strikes on one line should scale ~2x, not ~4x (mi60={mi60}, mi120={mi120})"
    );
    assert!(
        ms120 * 10 <= ms60 * 25,
        "strike_decorations span visits for n strikes on one line should scale ~2x, not ~4x (ms60={ms60}, ms120={ms120})"
    );
    assert!(
        pi120 * 10 <= pi60 * 25,
        "strike_decorations item visits for pill-split boxes should scale ~2x, not ~4x (pi60={pi60}, pi120={pi120})"
    );
}

#[test]
fn highlight_decorations_scales_linearly_with_boxes_and_highlights() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [11.25, 23.5];
    let bg = Color { r: 0.9, g: 0.8, b: 0.2, a: 0.5 };

    // Case 1: 1 highlight spanning a line split by n pills into n boxes.
    let measure_pill_boxes = |ts: &mut TextSystem, n: usize| {
        let mut src = String::new();
        let mut spans = Vec::with_capacity(n);
        let mut pills = Vec::with_capacity(n);
        for i in 0..n {
            let ws = src.len();
            src.push_str("ab");
            let we = src.len();
            if i % 2 == 0 {
                spans.push(Span::new(ws, we).size(20.0));
            }
            src.push(' ');
            let ps = src.len() as u32;
            src.push('p');
            let pe = src.len() as u32;
            pills.push(PillChip { start: ps, end: pe, icon: None, bg: None, fg: None, bare: None });
            if i + 1 < n {
                src.push(' ');
            }
        }
        let layout = ts.layout_blocks(&src, &spans, &[], &pills, TextStyle::new(14.0, 400.0), scale, None, Alignment::Start);
        let highlights = vec![TextHighlight::new(0..src.len(), bg).pad(2.0).vpad(1.0).radius(4.0)];
        let _ = take_highlight_run_visits();
        let mut got = Vec::new();
        ts.highlight_decorations(&layout, &highlights, src.len(), scale, origin, &mut got);
        let visits = take_highlight_run_visits();
        let mut expected = Vec::new();
        reference_highlight_decorations(&layout, &highlights, src.len(), scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "pill-split highlight must match reference");
        visits
    };

    // Case 2: n distinct highlights on one line (with n runs on that line).
    let measure_many_highlights = |ts: &mut TextSystem, n: usize| {
        let mut src = String::new();
        let mut spans = Vec::with_capacity(n);
        let mut highlights = Vec::with_capacity(n);
        for i in 0..n {
            if i > 0 {
                src.push(' ');
            }
            let a = src.len();
            src.push_str("ab");
            let b = src.len();
            spans.push(if i % 2 == 0 { Span::new(a, b).size(20.0) } else { Span::new(a, b).bold() });
            highlights.push(TextHighlight::new(a..b, bg).pad(2.0).vpad(1.0).radius(4.0));
        }
        let layout = ts.layout_rich(&src, &spans, TextStyle::new(14.0, 400.0), scale, None);
        let _ = take_highlight_run_visits();
        let mut got = Vec::new();
        ts.highlight_decorations(&layout, &highlights, src.len(), scale, origin, &mut got);
        let visits = take_highlight_run_visits();
        let mut expected = Vec::new();
        reference_highlight_decorations(&layout, &highlights, src.len(), scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "many-highlights must match reference");
        visits
    };

    let p60 = measure_pill_boxes(&mut ts, 60);
    let p120 = measure_pill_boxes(&mut ts, 120);
    let m60 = measure_many_highlights(&mut ts, 60);
    let m120 = measure_many_highlights(&mut ts, 120);
    assert!(
        p120 * 10 <= p60 * 25,
        "push_highlight_boxes run visits for n pill-split boxes should scale ~2x, not ~4x (p60={p60}, p120={p120})"
    );
    assert!(
        m120 * 10 <= m60 * 25,
        "push_highlight_boxes run visits for n highlights on one line should scale ~2x, not ~4x (m60={m60}, m120={m120})"
    );
}

#[test]
fn decorations_ring_lookup_matches_reference_for_overlapping_unsorted_and_wide_spans() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [5.5, 11.0];
    let bg = Color { r: 0.2, g: 0.3, b: 0.4, a: 1.0 };
    let c1 = Color { r: 0.9, g: 0.1, b: 0.1, a: 1.0 };
    let c2 = Color { r: 0.1, g: 0.9, b: 0.1, a: 1.0 };
    let c3 = Color { r: 0.1, g: 0.1, b: 0.9, a: 1.0 };
    let c4 = Color { r: 0.8, g: 0.6, b: 0.1, a: 1.0 };

    // Case A: unsorted, overlapping, zero-width, and degenerate rings over multiple spans.
    let src = "aa bb cc dd ee ff gg hh ii jj kk ll";
    let spans: Vec<Span> = (0..12)
        .map(|i| {
            let a = i * 3;
            let sp = Span::new(a, a + 2).bg(bg, 2.0);
            if i % 2 == 0 { sp.bold() } else { sp }
        })
        .collect();
    // A wide span over many flattened ring segments takes the last ring among all of them.
    let mut spans_wide = vec![Span::new(0, src.len()).bg(bg, 2.0)];
    spans_wide.extend(spans.iter().cloned());
    let rings = vec![
        (15, 21, c1, 1.0),
        (0, 12, c2, 1.5),
        (6, 18, c3, 2.0),
        (9, 10, c4, 0.0),  // zero width: ignored
        (24, 30, c4, 1.0),
        (27, 35, c2, 1.25),
        (3, 3, c1, 1.0),   // degenerate empty ring
        (0, 2, c3, 0.5),
        (12, 14, c1, 1.0),
        (18, 20, c2, 1.0),
        (21, 23, c3, 1.0),
        (30, 32, c4, 1.0),
    ];
    for sp_list in [&spans, &spans_wide] {
        let layout = ts.layout_rich(src, sp_list, TextStyle::new(14.0, 400.0), scale, None);
        let mut got = Vec::new();
        ts.decorations_with_rings(&layout, sp_list, &rings, 4.0, scale, origin, &mut got);
        let mut expected = Vec::new();
        reference_decorations_with_rings(&layout, sp_list, &rings, 4.0, scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "overlapping/unsorted rings must match reference");
    }

    // Case B: single-ring fast path and wide span with n non-overlapping trailing rings (worst case for .rev().find).
    let measure_miss_wide = |ts: &mut TextSystem, n: usize| {
        let mut s = "A🦀".repeat(n);
        let head_end = s.len();
        s.push_str(&"x".repeat(n * 2));
        let spans = vec![Span::new(0, head_end).bg(bg, 2.0)];
        let rings: Vec<_> = (0..n)
            .map(|i| ((head_end + i * 2) as u32, (head_end + i * 2 + 1) as u32, c1, 1.0))
            .collect();
        let layout = ts.layout_rich(&s, &spans, TextStyle::new(14.0, 400.0), scale, None);
        let _ = take_deco_ring_visits();
        let mut got = Vec::new();
        ts.decorations_with_rings(&layout, &spans, &rings, 4.0, scale, origin, &mut got);
        let visits = take_deco_ring_visits();
        let mut expected = Vec::new();
        reference_decorations_with_rings(&layout, &spans, &rings, 4.0, scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected));
        visits
    };
    let m60 = measure_miss_wide(&mut ts, 60);
    let m120 = measure_miss_wide(&mut ts, 120);
    assert!(
        m120 * 10 <= m60 * 25,
        "wide span with n non-matching rings should scale ~2x, not ~4x (m60={m60}, m120={m120})"
    );
}

#[test]
fn strike_and_highlight_match_reference_for_unsorted_overlaps_bidi_and_paragraphs() {
    let mut ts = TextSystem::default();
    let scale = 2.0;
    let origin = [9.5, 17.25];
    let c1 = Color { r: 0.8, g: 0.2, b: 0.2, a: 1.0 };
    let c2 = Color { r: 0.2, g: 0.7, b: 0.3, a: 1.0 };
    let c3 = Color { r: 0.2, g: 0.3, b: 0.9, a: 1.0 };

    // Mixed bidi + overlapping colored spans + unsorted strikes + explicit newline.
    let src = "hello שלום world\nsecond line";
    let spans = vec![
        Span::new(0, src.len()).color(c1),
        Span::new(0, 5).size(26.0).color(c2),
        Span::new(6, 14).size(18.0).color(c3),
    ];
    let layout = ts.layout_rich(src, &spans, TextStyle::new(14.0, 400.0), scale, None);
    let strikes = vec![
        (15u32, 20u32, None),
        (0u32, 5u32, None),
        (6u32, 14u32, None),
        (20u32, 21u32, Some(c2)), // newline byte only
        (0u32, src.len() as u32, None),
    ];
    let mut got_strikes = Vec::new();
    ts.strike_decorations(&layout, &spans, &strikes, Color::WHITE, scale, origin, &mut got_strikes);
    let mut exp_strikes = Vec::new();
    reference_strike_decorations(&layout, &spans, &strikes, Color::WHITE, scale, origin, &mut exp_strikes);
    assert_eq!(deco_snapshot(&got_strikes), deco_snapshot(&exp_strikes), "unsorted/bidi strikes must match reference");

    // Unsorted highlights on bidi + multi-line layout.
    let highlights = vec![
        TextHighlight::new(15..27, c3).pad(2.0).vpad(1.0).radius(3.0),
        TextHighlight::new(0..5, c1).pad(2.0).vpad(1.0).radius(3.0),
        TextHighlight::new(6..14, c2).pad(2.0).vpad(1.0).radius(3.0),
        TextHighlight::new(0..src.len(), c1).ring(c3, 1.0).pad(1.0).vpad(1.0).radius(4.0),
    ];
    let mut got_hl = Vec::new();
    ts.highlight_decorations(&layout, &highlights, src.len(), scale, origin, &mut got_hl);
    let mut exp_hl = Vec::new();
    reference_highlight_decorations(&layout, &highlights, src.len(), scale, origin, &mut exp_hl);
    assert_eq!(deco_snapshot(&got_hl), deco_snapshot(&exp_hl), "unsorted/bidi highlights must match reference");

    // Multi-paragraph TextFlow::Paragraphs with multiple highlights crossing newlines and blank lines.
    let multi = "First para\n\nThird para with BIG text";
    let big_start = multi.find("BIG").unwrap();
    let p_spans = vec![Span::new(big_start, big_start + 3).size(28.0)];
    let laid = ts.layout_text(
        &TextInput {
            content: multi,
            spans: &p_spans,
            blocks: &[],
            pills: &[],
            style: TextStyle::new(14.0, 400.0),
            shape_key: None,
            wrap_anywhere: false,
            pretty: false,
            redacted: false,
            by_line: false,
            by_paragraph: true,
            dir: TextDir::Fixed(Flow::Ltr),
        },
        true,
        scale,
        None,
        Alignment::Start,
        None,
    );
    let p_highlights = vec![
        TextHighlight::new(10..12, c2).pad(1.0).vpad(1.0).radius(3.0), // starts at para 0 end, crosses blank para 1
        TextHighlight::new(0..multi.len(), c1).pad(2.0).vpad(1.0).radius(4.0),
        TextHighlight::new(big_start..multi.len(), c3).pad(1.0).vpad(1.0).radius(3.0),
    ];
    let mut got_multi = Vec::new();
    ts.highlight_decorations_flow(laid.view(), &p_highlights, multi.len(), scale, origin, f32::NEG_INFINITY..f32::INFINITY, &mut got_multi);
    // Compare against running each highlight individually (single-highlight path, no cache).
    let mut exp_multi = Vec::new();
    for h in &p_highlights {
        ts.highlight_decorations_flow(laid.view(), std::slice::from_ref(h), multi.len(), scale, origin, f32::NEG_INFINITY..f32::INFINITY, &mut exp_multi);
    }
    assert_eq!(deco_snapshot(&got_multi), deco_snapshot(&exp_multi), "multi-paragraph cached highlights must match single-highlight output");
}
