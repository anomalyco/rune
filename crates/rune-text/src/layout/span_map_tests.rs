use super::*;

#[test]
fn sorted_disjoint_get_and_backwards_seek() {
    let map = SpanMap::new([(2..5, 10u32), (8..12, 20), (12..15, 30)]);
    assert_eq!(map.get(0), None);
    assert_eq!(map.get(1), None);
    assert_eq!(map.get(2), Some(10));
    assert_eq!(map.get(4), Some(10));
    assert_eq!(map.get(5), None);
    assert_eq!(map.get(7), None);
    assert_eq!(map.get(8), Some(20));
    assert_eq!(map.get(11), Some(20));
    assert_eq!(map.get(12), Some(30));
    assert_eq!(map.get(15), None);
    // Backwards jumps (as bidi runs can produce) fall back to binary search accurately.
    assert_eq!(map.get(10), Some(20));
    assert_eq!(map.get(3), Some(10));
    assert_eq!(map.get(6), None);
    assert_eq!(map.get(14), Some(30));
    assert_eq!(map.get(0), None);
}

#[test]
fn overlapping_and_unsorted_preserve_later_wins_precedence() {
    let input = vec![
        (10..20, 1u32),
        (0..15, 2),
        (5..12, 3),
        (8..8, 99), // empty span ignored
        (12..18, 4),
    ];
    let map = SpanMap::new(input.clone());
    for at in 0..24 {
        let expected = input.iter().rev().find_map(|(r, v)| r.contains(&at).then_some(*v));
        assert_eq!(map.get(at), expected, "mismatch at byte {at}");
    }
    // Reverse query pass verifies binary-search fallback matches too.
    for at in (0..24).rev() {
        let expected = input.iter().rev().find_map(|(r, v)| r.contains(&at).then_some(*v));
        assert_eq!(map.get(at), expected, "reverse mismatch at byte {at}");
    }
}

#[test]
fn overlaps_matches_brute_force_on_disjoint_and_overlapping_inputs() {
    let disjoint = vec![(2..5, ()), (8..11, ()), (11..14, ())];
    let overlapping = vec![(10..15, ()), (2..6, ()), (4..8, ()), (7..7, ())];
    for input in [disjoint, overlapping] {
        let map = SpanMap::new(input.clone());
        for a in 0..18 {
            for b in a..18 {
                let expected = a < b && input.iter().any(|(r, _)| r.start < r.end && r.start < b && a < r.end);
                assert_eq!(map.overlaps(a..b), expected, "overlaps({a}..{b})");
            }
        }
    }
}

#[test]
fn monotonic_queries_are_linear_in_spans_plus_queries() {
    let n = 4_000usize;
    let _ = take_span_scan_steps();
    let map = SpanMap::new((0..n).map(|i| (i * 2..i * 2 + 2, i as u32)));
    for i in 0..n {
        assert_eq!(map.get(i * 2), Some(i as u32));
    }
    let steps = take_span_scan_steps();
    assert!(steps <= n * 4, "expected O(n) steps, got {steps} for n={n}");
}

#[test]
fn for_each_in_splits_ranges_across_entries_and_gaps() {
    let map = SpanMap::new([(2..5, 10u32), (8..12, 20), (12..15, 30)]);
    let collect = |r: Range<usize>| {
        let mut out = Vec::new();
        map.for_each_in(r, |piece, v| out.push((piece, v)));
        out
    };
    assert_eq!(collect(0..10), vec![(0..2, None), (2..5, Some(10)), (5..8, None), (8..10, Some(20))]);
    assert_eq!(collect(10..16), vec![(10..12, Some(20)), (12..15, Some(30)), (15..16, None)]);
    // Backwards jump resets via binary search.
    assert_eq!(collect(3..9), vec![(3..5, Some(10)), (5..8, None), (8..9, Some(20))]);
    assert_eq!(collect(5..5), vec![]);
    assert_eq!(collect(20..25), vec![(20..25, None)]);
}
