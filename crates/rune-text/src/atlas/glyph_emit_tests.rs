use super::*;

#[test]
fn ink_spans_do_not_overflow_at_65536() {
    let mut spans: Vec<Span> = (0..65_537)
        .map(|i| {
            let color = if i % 2 == 0 { Color::BLACK } else { Color::WHITE };
            Span::new(i, i + 1).color(color)
        })
        .collect();
    let mut out = Vec::new();
    set_ink_spans(&mut out, &spans);
    assert_eq!(out.len(), 65_537);
    assert_eq!(out[65_534], (65_534, 65_535, 65_535));
    assert_eq!(out[65_535], (65_535, 65_536, 65_536));
    assert_eq!(out[65_536], (65_536, 65_537, 65_537));

    let mut ts = TextSystem::default();
    ts.set_glyph_inks(Color::BLACK, &spans);
    let mut cursor = 0;
    assert_eq!(ts.ink_at(0, &mut cursor), 1);
    assert_eq!(ts.ink_at(65_535, &mut cursor), 65_536);
    assert_eq!(ts.ink_at(65_536, &mut cursor), 65_537);

    // Emit real glyph quads (direct and cached) for a tiny text whose coloured spans sit past
    // index 65_535, exercising both the overlapping path and rasteriser ink lookups.
    spans.clear();
    spans.resize(65_535, Span::new(0, 0));
    spans.push(Span::new(0, 2).color(Color::BLACK));
    spans.push(Span::new(1, 3).color(Color::WHITE));
    let layout = ts.layout_rich("ABC", &spans, TextStyle::new(14.0, 400.0), 1.0, None);
    ts.set_glyph_inks(Color::BLACK, &spans);

    let mut direct = Vec::new();
    ts.glyphs_for_layout(&layout, 1.0, [0.0, 0.0], true, &mut direct);
    assert_eq!(direct.len(), 3);
    assert_eq!(direct.iter().map(|q| q.ink).collect::<Vec<_>>(), [65_536, 65_537, 65_537]);
    assert_eq!(spans[direct[0].ink as usize - 1].color, Some(Color::BLACK));
    assert_eq!(spans[direct[1].ink as usize - 1].color, Some(Color::WHITE));
    assert_eq!(spans[direct[2].ink as usize - 1].color, Some(Color::WHITE));

    let mut cached = Vec::new();
    ts.glyphs_in_band_cached(&layout, [0.0, 0.0], 1.0, true, f32::NEG_INFINITY..f32::INFINITY, &mut cached);
    assert_eq!(cached.len(), 3);
    assert_eq!(cached.iter().map(|q| q.ink).collect::<Vec<_>>(), [65_536, 65_537, 65_537]);
}
