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

fn reference_ink_spans(spans: &[Span]) -> Vec<(u32, u32, u32)> {
    let input: Vec<(u32, u32, u32)> = spans
        .iter()
        .enumerate()
        .filter_map(|(i, s)| (s.start < s.end && s.color.is_some()).then_some((s.start, s.end, i as u32 + 1)))
        .collect();
    if input.is_empty() {
        return Vec::new();
    }
    let mut bounds = Vec::with_capacity(input.len() * 2);
    for &(a, b, _) in &input {
        bounds.push(a);
        bounds.push(b);
    }
    bounds.sort_unstable();
    bounds.dedup();
    let mut out: Vec<(u32, u32, u32)> = Vec::new();
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        if let Some(&(_, _, ink)) = input.iter().rev().find(|&&(sa, sb, _)| sa <= a && b <= sb) {
            if let Some(last) = out.last_mut().filter(|l| l.1 == a && l.2 == ink) {
                last.1 = b;
            } else {
                out.push((a, b, ink));
            }
        }
    }
    out
}

#[test]
fn set_ink_spans_matches_reference_on_overlapping_and_unsorted_inputs() {
    let cases = vec![
        vec![
            Span::new(10, 20).color(Color::BLACK),
            Span::new(0, 15).color(Color::WHITE),
            Span::new(4, 12).bold(), // no colour: ignored but occupies span ordinal
            Span::new(5, 12).color(Color::BLACK),
            Span::new(8, 8).color(Color::WHITE), // empty: ignored
            Span::new(12, 18).color(Color::WHITE),
        ],
        vec![
            Span::new(0, 30).color(Color::BLACK),
            Span::new(5, 10).color(Color::WHITE),
            Span::new(15, 20).color(Color::WHITE),
            Span::new(0, 30).color(Color::BLACK), // fully shadows earlier spans
        ],
        vec![
            Span::new(20, 25).color(Color::BLACK),
            Span::new(10, 15).color(Color::WHITE),
            Span::new(0, 5).color(Color::BLACK),
        ],
    ];
    let mut out = Vec::new();
    for spans in cases {
        set_ink_spans(&mut out, &spans);
        assert_eq!(out, reference_ink_spans(&spans));
    }
}

#[test]
fn set_ink_spans_overlap_path_scales_linearly() {
    let measure = |n: usize| -> usize {
        let spans: Vec<Span> = (0..n)
            .map(|i| {
                let c = if i % 2 == 0 { Color::BLACK } else { Color::WHITE };
                Span::new(i * 2, i * 2 + 16).color(c)
            })
            .collect();
        let mut out = Vec::new();
        let _ = crate::layout::span_map::take_span_scan_steps();
        set_ink_spans(&mut out, &spans);
        let steps = crate::layout::span_map::take_span_scan_steps();
        assert_eq!(out.first().unwrap().0, 0);
        assert_eq!(out.last().unwrap().1, (n * 2 + 14) as u32);
        steps
    };

    let s200 = measure(200);
    let s400 = measure(400);
    assert!(
        s400 <= 400 * 12,
        "expected linear sweep steps for 400 overlapping ink spans, got {s400} (s200={s200})"
    );
    assert!(
        s400 * 10 <= s200 * 25,
        "doubling overlapping ink spans should scale ~2x, not ~4x (s200={s200}, s400={s400})"
    );
}
