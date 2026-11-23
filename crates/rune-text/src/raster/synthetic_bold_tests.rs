use super::*;
use crate::Color;
use crate::layout::span_map::take_span_scan_steps;

fn reference_ink_at(spans: &[Span], at: usize) -> u32 {
    let layout_spans: Vec<&Span> = crate::layout::span::layout_spans(spans).collect();
    layout_spans
        .iter()
        .rposition(|s| (s.start as usize) <= at && at < s.end as usize)
        .and_then(|i| i.checked_add(1)?.try_into().ok())
        .filter(|&n: &u32| n < SYNTHETIC_BOLD)
        .unwrap_or(0)
        | SYNTHETIC_BOLD
}

fn assert_matches_reference(ranges: &[Range<usize>], spans: &[Span], got: &[(Range<usize>, u32)]) {
    // Every emitted piece must be non-empty, sorted within its source range, and assign the exact
    // same brush to every byte as the brute-force reference.
    let mut covered: Vec<(usize, u32)> = Vec::new();
    for (r, ink) in got {
        assert!(r.start < r.end, "empty synthetic-bold piece {r:?}");
        for at in r.clone() {
            assert_eq!(*ink, reference_ink_at(spans, at), "brush mismatch at byte {at}");
            covered.push((at, *ink));
        }
    }
    let expected: Vec<(usize, u32)> = ranges
        .iter()
        .flat_map(|r| r.clone().map(|at| (at, reference_ink_at(spans, at))))
        .collect();
    assert_eq!(covered, expected, "covered bytes and brushes must match reference exactly");
}

#[test]
fn synthetic_bold_inks_match_reference_on_overlapping_and_unsorted_spans() {
    let spans = vec![
        Span::new(10, 22).bold(),
        Span::new(0, 14).color(Color::WHITE), // colour-only: not a layout span
        Span::new(2, 16).underline(Color::BLACK),
        Span::new(6, 12).mono(),
        Span::new(8, 8).italic(), // empty: ignored
        Span::new(0, 20).bg(Color::BLACK, 2.0), // shadows earlier layout spans
        Span::new(18, 25).italic(),
    ];
    let ranges = vec![0..4, 4..15, 17..28];
    let got = inks(&ranges, &spans);
    assert_matches_reference(&ranges, &spans, &got);

    // Backwards / overlapping input ranges also match the reference.
    let unsorted_ranges = vec![15..26, 0..10, 5..18];
    let got_unsorted = inks(&unsorted_ranges, &spans);
    assert_matches_reference(&unsorted_ranges, &spans, &got_unsorted);

    // No spans and no ranges edge cases.
    assert_eq!(inks(&[3..9], &[]), vec![(3..9, SYNTHETIC_BOLD)]);
    assert!(inks(&[], &spans).is_empty());
}

#[test]
fn synthetic_bold_inks_scale_linearly_with_overlapping_spans() {
    let measure = |n: usize| -> usize {
        let spans: Vec<Span> = (0..n).map(|i| Span::new(i * 2, i * 2 + 12).bold()).collect();
        let ranges: Vec<Range<usize>> = (0..n).map(|i| i * 2..i * 2 + 2).collect();
        let _ = take_span_scan_steps();
        let got = inks(&ranges, &spans);
        let steps = take_span_scan_steps();
        assert_eq!(got.len(), n);
        steps
    };

    let s200 = measure(200);
    let s400 = measure(400);
    assert!(
        s400 <= 400 * 16,
        "expected linear scan steps for 400 overlapping spans, got {s400} (s200={s200})"
    );
    assert!(
        s400 * 10 <= s200 * 25,
        "doubling overlapping spans should scale ~2x, not ~4x (s200={s200}, s400={s400})"
    );

    // One wide range over many overlapping spans also scales linearly.
    let wide = |n: usize| -> usize {
        let spans: Vec<Span> = (0..n).map(|i| Span::new(i * 2, i * 2 + 12).bold()).collect();
        let range = 0..(n - 1) * 2 + 12;
        let _ = take_span_scan_steps();
        let got = inks(std::slice::from_ref(&range), &spans);
        let steps = take_span_scan_steps();
        assert_eq!(got.len(), n);
        steps
    };
    let w200 = wide(200);
    let w400 = wide(400);
    assert!(w400 <= 400 * 16, "expected linear steps for one wide range, got {w400} (w200={w200})");
    assert!(w400 * 10 <= w200 * 25, "doubling overlapping spans under one range should scale ~2x (w200={w200}, w400={w400})");
}
