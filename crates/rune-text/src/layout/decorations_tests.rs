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
