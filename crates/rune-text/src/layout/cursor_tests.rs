use super::*;
use parley::{editing::Cursor, editing::Selection, layout::Affinity};

fn layout(ts: &mut TextSystem, text: &str, max_w: Option<f32>) -> Rc<Layout<Ink>> {
    ts.layout_rich(text, &[], TextStyle::new(15.0, 400.0), 2.0, max_w)
}

fn parley_boxes(l: &Layout<Ink>, a: usize, b: usize) -> Vec<(parley::BoundingBox, usize)> {
    Selection::new(
        Cursor::from_byte_index(l, a, Affinity::Downstream),
        Cursor::from_byte_index(l, b, Affinity::Upstream),
    )
    .geometry(l)
}

fn line_indices(boxes: &[(parley::BoundingBox, usize)]) -> Vec<usize> {
    let mut lines: Vec<usize> = boxes.iter().map(|&(_, li)| li).collect();
    lines.dedup();
    lines
}

fn cluster_x_range(l: &Layout<Ink>, byte: usize) -> (usize, f64, f64) {
    let cl = parley::Cluster::from_byte_index(l, byte).expect("cluster for byte");
    let line = cl.line();
    let m = line.metrics();
    let x0 = (m.inline_min_coord + cl.visual_offset().unwrap_or(m.offset)) as f64;
    let x1 = x0 + cl.advance() as f64;
    let line_ix = l.lines().position(|ln| ln.text_range() == line.text_range()).unwrap();
    (line_ix, x0, x1)
}

fn boxes_cover(boxes: &[(parley::BoundingBox, usize)], line_ix: usize, x0: f64, x1: f64) -> bool {
    boxes.iter().any(|&(bb, li)| li == line_ix && bb.x0 <= x0 + 0.5 && bb.x1 >= x1 - 0.5)
}

fn assert_matches_parley(l: &Layout<Ink>, label: &str, text: &str) {
    let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    for (i, &a) in bounds.iter().enumerate() {
        for &b in &bounds[i + 1..] {
            let ours = range_boxes(l, a, b, true);
            let theirs = parley_boxes(l, a, b);
            assert_eq!(
                ours.len(),
                theirs.len(),
                "box count mismatch for {label} {text:?} {a}..{b}: ours={ours:?} theirs={theirs:?}"
            );
            for ((ob, ol), (tb, tl)) in ours.iter().zip(&theirs) {
                assert_eq!(ol, tl, "line mismatch for {label} {text:?} {a}..{b}");
                assert!(
                    (ob.x0 - tb.x0).abs() <= 0.5
                        && (ob.y0 - tb.y0).abs() <= 0.5
                        && (ob.x1 - tb.x1).abs() <= 0.5
                        && (ob.y1 - tb.y1).abs() <= 0.5,
                    "rect mismatch for {label} {text:?} {a}..{b}: ours={ob:?} theirs={tb:?}"
                );
            }
        }
    }
}

#[test]
fn rtl_line_ending_in_newline_paints_every_covered_line() {
    let mut ts = TextSystem::default();

    let two = "تمام يا أستاذ\nالشحنة وصلت";
    let l = layout(&mut ts, two, Some(1000.0));
    assert_eq!(l.len(), 2);
    let nl = two.find('\n').unwrap();

    // Drag from byte 0 across the first word on line 0.
    let first_word_end = two.find(' ').unwrap();
    let b_partial = range_boxes(&l, 0, first_word_end, true);
    assert_eq!(line_indices(&b_partial), vec![0], "partial drag from byte 0 on line 0: {b_partial:?}");
    let (li0, c0, c1) = cluster_x_range(&l, 0);
    assert!(boxes_cover(&b_partial, li0, c0, c1), "first cluster must be covered: {b_partial:?}");

    // First paragraph up to and including the newline (triple-click).
    let b_para = range_boxes(&l, 0, nl + 1, true);
    assert_eq!(line_indices(&b_para), vec![0], "first paragraph with newline: {b_para:?}");
    assert!(boxes_cover(&b_para, li0, c0, c1));

    // Select-all across both RTL lines.
    let b_all = range_boxes(&l, 0, two.len(), true);
    assert_eq!(line_indices(&b_all), vec![0, 1], "select-all on two RTL lines: {b_all:?}");
    assert!(boxes_cover(&b_all, li0, c0, c1));

    // Three RTL lines: triple-clicking the middle paragraph (starts right after first '\n').
    let three = "سطر أول\nسطر ثان\nسطر ثالث";
    let l3 = layout(&mut ts, three, Some(1000.0));
    assert_eq!(l3.len(), 3);
    let n1 = three.find('\n').unwrap();
    let n2 = three[n1 + 1..].find('\n').unwrap() + n1 + 1;
    let b_mid = range_boxes(&l3, n1 + 1, n2 + 1, true);
    assert_eq!(line_indices(&b_mid), vec![1], "middle RTL paragraph: {b_mid:?}");
    let (li1, m0, m1) = cluster_x_range(&l3, n1 + 1);
    assert!(boxes_cover(&b_mid, li1, m0, m1));

    // Empty middle line between RTL paragraphs: select-all paints all three lines.
    let blank_mid = "سطر أول\n\nسطر ثالث";
    let lb = layout(&mut ts, blank_mid, Some(1000.0));
    assert_eq!(lb.len(), 3);
    let b_blank = range_boxes(&lb, 0, blank_mid.len(), true);
    assert_eq!(line_indices(&b_blank), vec![0, 1, 2], "select-all with empty RTL line: {b_blank:?}");

    // RTL line with embedded Latin token ending in '\n'.
    let mixed = "رقم INV-2048\nالشحنة وصلت";
    let lm = layout(&mut ts, mixed, Some(1000.0));
    assert_eq!(lm.len(), 2);
    let mnl = mixed.find('\n').unwrap();
    let b_mix = range_boxes(&lm, 0, mnl + 1, true);
    assert_eq!(line_indices(&b_mix), vec![0], "RTL+Latin line ending in newline: {b_mix:?}");
    let (ml0, mx0, mx1) = cluster_x_range(&lm, 0);
    assert!(boxes_cover(&b_mix, ml0, mx0, mx1));

    // CRLF line ending on an RTL line.
    let crlf = "تمام يا أستاذ\r\nالشحنة وصلت";
    let lc = layout(&mut ts, crlf, Some(1000.0));
    assert_eq!(lc.len(), 2);
    let b_crlf = range_boxes(&lc, 0, crlf.len(), true);
    assert_eq!(line_indices(&b_crlf), vec![0, 1], "RTL CRLF select-all: {b_crlf:?}");
}

#[test]
fn selected_line_break_shows_at_reading_order_end() {
    let mut ts = TextSystem::default();

    // Pure RTL: selecting the trailing '\n' extends the left edge, not the right edge.
    let rtl = "تمام يا أستاذ\nالشحنة وصلت";
    let lr = layout(&mut ts, rtl, Some(1000.0));
    let nl = rtl.find('\n').unwrap();
    let without_nl = range_boxes(&lr, 0, nl, true);
    let with_nl = range_boxes(&lr, 0, nl + 1, true);
    assert_eq!(without_nl.len(), 1);
    assert_eq!(with_nl.len(), 1);
    assert!(with_nl[0].0.x0 < without_nl[0].0.x0 - 1.0, "RTL newline should extend left: without={without_nl:?} with={with_nl:?}");
    assert!((with_nl[0].0.x1 - without_nl[0].0.x1).abs() < 0.5, "RTL newline must not extend right: without={without_nl:?} with={with_nl:?}");

    // RTL with embedded Latin right before '\n': newline still extends left.
    let rtl_mix = "تمام INV-2048\nالشحنة وصلت";
    let lrm = layout(&mut ts, rtl_mix, Some(1000.0));
    let mnl = rtl_mix.find('\n').unwrap();
    let m_without = range_boxes(&lrm, 0, mnl, true);
    let m_with = range_boxes(&lrm, 0, mnl + 1, true);
    assert_eq!(m_without.len(), 1);
    assert_eq!(m_with.len(), 1);
    assert!(m_with[0].0.x0 < m_without[0].0.x0 - 1.0, "RTL+Latin newline should extend left: without={m_without:?} with={m_with:?}");
    assert!((m_with[0].0.x1 - m_without[0].0.x1).abs() < 0.5, "RTL+Latin newline must not extend right: without={m_without:?} with={m_with:?}");

    // Selecting only the suffix of an embedded LTR run plus '\n' on an RTL line keeps the
    // newline highlight at the line's left end without covering the unselected prefix.
    let suffix_start = rtl_mix.find("2048").unwrap();
    let suffix_boxes = range_boxes(&lrm, suffix_start, mnl + 1, true);
    assert_eq!(suffix_boxes.len(), 2, "disjoint LTR suffix and RTL newline stay separate: {suffix_boxes:?}");
    let (_, inv_x0, inv_x1) = cluster_x_range(&lrm, rtl_mix.find("INV").unwrap());
    assert!(!boxes_cover(&suffix_boxes, 0, inv_x0, inv_x1), "unselected prefix must not be covered: {suffix_boxes:?}");

    // LTR: selecting the trailing '\n' extends the right edge, not the left edge.
    let ltr = "Hello there\nsecond line";
    let ll = layout(&mut ts, ltr, Some(1000.0));
    let lnl = ltr.find('\n').unwrap();
    let l_without = range_boxes(&ll, 0, lnl, true);
    let l_with = range_boxes(&ll, 0, lnl + 1, true);
    assert_eq!(l_without.len(), 1);
    assert_eq!(l_with.len(), 1);
    assert!(l_with[0].0.x1 > l_without[0].0.x1 + 1.0, "LTR newline should extend right: without={l_without:?} with={l_with:?}");
    assert!((l_with[0].0.x0 - l_without[0].0.x0).abs() < 0.5, "LTR newline must not extend left: without={l_without:?} with={l_with:?}");
}

#[test]
fn ranges_parley_paints_correctly_keep_identical_boxes() {
    let mut ts = TextSystem::default();
    let samples = [
        "Hello world",
        "Hello there\nsecond line\n\nfourth line",
        "Hello world with several words that wrap across lines cleanly",
        "Hello   world   spaced",
        "Hello تمام world\nsecond line",
        "Hello تمام\nsecond line",
        "تمام يا أستاذ",
        "تمام يا أستاذ هذا نص عربي طويل يلتف على أكثر من سطر بدون فواصل صريحة",
    ];
    for text in samples {
        for width in [1000.0f32, 260.0f32] {
            let l = layout(&mut ts, text, Some(width));
            assert_matches_parley(&l, &format!("default w={width}"), text);
        }
    }

    // Center and end alignments match parley on every range.
    let style = TextStyle::new(15.0, 400.0);
    let aligned_text = "First line here\nsecond longer line that wraps cleanly";
    for align in [Alignment::Center, Alignment::End] {
        let l = ts.layout_aligned(aligned_text, &[], style, 2.0, Some(300.0), align);
        assert_matches_parley(&l, &format!("align={align:?}"), aligned_text);
    }
    // Justified lines keep the stretched visual width on full-line selections.
    let lj = ts.layout_aligned(aligned_text, &[], style, 2.0, Some(300.0), Alignment::Justify);
    let mid = lj.get(1).expect("justified wrapped line");
    let mid_r = mid.text_range();
    let mid_single = range_boxes(&lj, mid_r.start, mid_r.end, true);
    let mid_all = range_boxes(&lj, 0, aligned_text.len(), true);
    let expected_end = (mid.metrics().inline_min_coord + mid.metrics().offset) as f64
        + mid.runs().map(|r| r.advance() as f64).sum::<f64>();
    assert!((mid_single[0].0.x1 - expected_end).abs() <= 0.5, "justified single line: {mid_single:?} vs {expected_end}");
    let all_mid = mid_all.iter().find(|&&(_, li)| li == 1).expect("middle line in select-all");
    assert!((all_mid.0.x1 - expected_end).abs() <= 0.5, "justified middle line: {all_mid:?} vs {expected_end}");

    // Indented block (non-zero inline_min_coord) and multi-size spans.
    let rich_text = "Big then small\nsecond line";
    let spans = [Span::new(0, 3).size(32.0)];
    let lr = ts.layout_rich(rich_text, &spans, style, 2.0, Some(600.0));
    assert_matches_parley(&lr, "multi-size spans", rich_text);

    let quoted = "Quoted line one\nquoted line two";
    let blocks = [Block::Quote, Block::Quote];
    let lq = ts.layout_blocks(quoted, &[], &blocks, &[], style, 2.0, Some(600.0), Alignment::Start);
    assert_matches_parley(&lq, "quoted block", quoted);
}

fn parley_caret(l: &Layout<Ink>, byte: usize, aff: Affinity) -> parley::BoundingBox {
    let cur = Cursor::from_byte_index(l, byte, aff);
    let mut bb = cur.geometry(l, 2.0);
    let inline_min = match cur.visual_clusters(l) {
        [Some(left), _] => left.line().metrics().inline_min_coord,
        [_, Some(right)] => right.line().metrics().inline_min_coord,
        _ => l.get(l.len().saturating_sub(1)).map_or(0.0, |ln| ln.metrics().inline_min_coord),
    };
    if inline_min != 0.0 {
        bb.x0 += inline_min as f64;
        bb.x1 += inline_min as f64;
    }
    bb
}

fn caret_x(l: &Layout<Ink>, byte: usize, aff: Affinity) -> (f64, usize) {
    let cur = Cursor::from_byte_index(l, byte, aff);
    let bb = cursor_geometry(&cur, l, 2.0);
    let mid_y = ((bb.y0 + bb.y1) * 0.5) as f32;
    let li = line_for_offset(l, mid_y).map_or(l.len().saturating_sub(1), |(i, _)| i);
    (bb.x0, li)
}

fn assert_carets_match_parley(l: &Layout<Ink>, label: &str, text: &str) {
    let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    for &b in &bounds {
        for aff in [Affinity::Downstream, Affinity::Upstream] {
            let cur = Cursor::from_byte_index(l, b, aff);
            let ours = cursor_geometry(&cur, l, 2.0);
            let theirs = parley_caret(l, b, aff);
            assert!(
                (ours.x0 - theirs.x0).abs() <= 0.5
                    && (ours.y0 - theirs.y0).abs() <= 0.5
                    && (ours.x1 - theirs.x1).abs() <= 0.5
                    && (ours.y1 - theirs.y1).abs() <= 0.5,
                "caret mismatch for {label} {text:?} byte={b} {aff:?}: ours={ours:?} theirs={theirs:?}"
            );
        }
    }
}

#[test]
fn rtl_embedded_ltr_caret_positions_match_bidi_boundaries() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(15.0, 400.0);

    // Core #243 repro: RTL "و123" (unwrapped and wrapped).
    // Visual order left-to-right: [ '1' (2..3) | '2' (3..4) | '3' (4..5) | 'و' (0..2) ].
    for max_w in [None, Some(400.0)] {
        let text = "و123";
        let l = ts.layout_rich_with(text, &[], style, 2.0, max_w, RichLayoutOptions::new().direction(Direction::Auto));
        assert!(l.is_rtl());
        let (_, x1_0, x1_1) = cluster_x_range(&l, 2);
        let (_, x2_0, x2_1) = cluster_x_range(&l, 3);
        let (_, x3_0, x3_1) = cluster_x_range(&l, 4);
        let (_, xv_0, xv_1) = cluster_x_range(&l, 0);
        assert!((x1_1 - x2_0).abs() < 0.5 && (x2_1 - x3_0).abs() < 0.5 && (x3_1 - xv_0).abs() < 0.5);

        let expected = [
            (0usize, xv_1, "start of RTL text -> right of و"),
            (2, xv_0, "before 123 -> right edge of 123 (between 3 and و)"),
            (3, x1_1, "between 1 and 2"),
            (4, x2_1, "between 2 and 3"),
            (5, x1_0, "after 123 (end of RTL text) -> left edge of 123"),
        ];
        for (byte, exp_x, desc) in expected {
            for aff in [Affinity::Downstream, Affinity::Upstream] {
                let (x, li) = caret_x(&l, byte, aff);
                assert_eq!(li, 0, "{desc} ({aff:?}, max_w={max_w:?}): wrong line");
                assert!((x - exp_x).abs() <= 0.5, "{desc} ({aff:?}, max_w={max_w:?}): got x={x:.1}, expected {exp_x:.1}");
            }
            // Hit-testing at the drawn caret x round-trips back to the same logical byte offset.
            let mid_y = (l.get(0).unwrap().metrics().block_min_coord + l.get(0).unwrap().metrics().block_max_coord) * 0.5;
            let hit = cursor_from_point(&l, exp_x as f32, mid_y);
            assert_eq!(hit.index(), byte, "hit-test at x={exp_x:.1} ({desc}, max_w={max_w:?}) should return byte {byte}");
        }
    }

    // RTL with embedded LTR in the middle: "و123ب" -> visual [ 'ب' | '1' | '2' | '3' | 'و' ].
    let mid = "و123ب";
    let lm = ts.layout_rich_with(mid, &[], style, 2.0, Some(400.0), RichLayoutOptions::new().direction(Direction::Auto));
    let (_, xb_0, xb_1) = cluster_x_range(&lm, 5);
    let (_, x1_0, _) = cluster_x_range(&lm, 2);
    let (_, _, x3_1) = cluster_x_range(&lm, 4);
    let (_, xv_0, xv_1) = cluster_x_range(&lm, 0);
    assert!((caret_x(&lm, 0, Affinity::Downstream).0 - xv_1).abs() <= 0.5);
    assert!((caret_x(&lm, 2, Affinity::Downstream).0 - xv_0).abs() <= 0.5, "before 123 between 3 and و (x3_1={x3_1:.1})");
    assert!((caret_x(&lm, 5, Affinity::Downstream).0 - xb_1).abs() <= 0.5, "after 123 between ب and 1 (x1_0={x1_0:.1})");
    assert!((caret_x(&lm, 7, Affinity::Downstream).0 - xb_0).abs() <= 0.5);

    // RTL starting with an embedded LTR run: "123و" -> visual [ 'و' | '1' | '2' | '3' ].
    let lead = "123و";
    let ll = ts.layout_rich_with(lead, &[], style, 2.0, Some(400.0), RichLayoutOptions::new().direction(Direction::Rtl));
    let (_, xv_0, xv_1) = cluster_x_range(&ll, 3);
    let (_, _, x3_1) = cluster_x_range(&ll, 2);
    assert!((caret_x(&ll, 0, Affinity::Downstream).0 - x3_1).abs() <= 0.5, "start of RTL 123و -> right of 3");
    assert!((caret_x(&ll, 3, Affinity::Downstream).0 - xv_1).abs() <= 0.5, "between 123 and و");
    assert!((caret_x(&ll, 5, Affinity::Downstream).0 - xv_0).abs() <= 0.5, "end of RTL 123و -> left of و");

    // LTR ending in an embedded RTL run: "abcسلام" -> visual [ a b c م ا ل س ].
    let ltr_rtl = "abcسلام";
    let la = ts.layout_rich_with(ltr_rtl, &[], style, 2.0, Some(400.0), RichLayoutOptions::new().direction(Direction::Auto));
    let (_, _, xc_1) = cluster_x_range(&la, 2);
    let (_, _, xs_1) = cluster_x_range(&la, 3);
    assert!((caret_x(&la, 3, Affinity::Downstream).0 - xc_1).abs() <= 0.5, "before سلام -> between c and م");
    assert!((caret_x(&la, ltr_rtl.len(), Affinity::Downstream).0 - xs_1).abs() <= 0.5, "end of LTR abcسلام -> right of س");

    // Longer Persian string from #243 ending in a number.
    let persian = "تایپ کنید: سلام دنیا — تایپ\u{200c}شده با English و 123";
    let lp = ts.layout_rich_with(persian, &[], style, 2.0, Some(800.0), RichLayoutOptions::new().direction(Direction::Auto));
    let d1 = persian.find("123").unwrap();
    let d3 = d1 + 2;
    let (_, x1_0, _) = cluster_x_range(&lp, d1);
    let (_, _, x3_1) = cluster_x_range(&lp, d3);
    let eng_start = persian.find("English").unwrap();
    let eng_end = eng_start + "English".len();
    let (_, xe_0, _) = cluster_x_range(&lp, eng_start);
    let (_, _, xh_1) = cluster_x_range(&lp, eng_end - 1);
    assert!((caret_x(&lp, d1, Affinity::Downstream).0 - x3_1).abs() <= 0.5, "before 123 -> right of 3");
    assert!((caret_x(&lp, persian.len(), Affinity::Downstream).0 - x1_0).abs() <= 0.5, "end of Persian string -> left of 1");
    assert!((caret_x(&lp, eng_start, Affinity::Downstream).0 - xh_1).abs() <= 0.5, "before English -> right of h");
    assert!((caret_x(&lp, eng_end, Affinity::Downstream).0 - xe_0).abs() <= 0.5, "after English -> left of E");

    // RTL + embedded LTR + explicit newline: byte 0 stays on line 0, end of line 0 is left of 123.
    let nl_text = "و123\nب";
    let lnl = ts.layout_rich_with(nl_text, &[], style, 2.0, Some(400.0), RichLayoutOptions::new().direction(Direction::Auto));
    let (_, x1_0, _) = cluster_x_range(&lnl, 2);
    let (_, xv_0, xv_1) = cluster_x_range(&lnl, 0);
    let (_, xb_0, xb_1) = cluster_x_range(&lnl, 6);
    assert_eq!(caret_x(&lnl, 0, Affinity::Downstream).1, 0, "byte 0 must stay on line 0");
    assert!((caret_x(&lnl, 0, Affinity::Downstream).0 - xv_1).abs() <= 0.5);
    assert!((caret_x(&lnl, 2, Affinity::Downstream).0 - xv_0).abs() <= 0.5);
    assert!((caret_x(&lnl, 5, Affinity::Downstream).0 - x1_0).abs() <= 0.5);
    assert_eq!(caret_x(&lnl, 6, Affinity::Downstream).1, 1);
    assert!((caret_x(&lnl, 6, Affinity::Downstream).0 - xb_1).abs() <= 0.5);
    assert!((caret_x(&lnl, 8, Affinity::Downstream).0 - xb_0).abs() <= 0.5);

    // Nested isolate (L2 digits inside L1 RTL isolate inside L0 LTR paragraph).
    let nested = "a \u{2067}و 123 ب\u{2069} z";
    let ln = ts.layout_rich_with(nested, &[], style, 2.0, Some(600.0), RichLayoutOptions::new().direction(Direction::Ltr));
    let nd1 = nested.find("123").unwrap();
    let (_, nx1_0, _) = cluster_x_range(&ln, nd1);
    let (_, _, nx3_1) = cluster_x_range(&ln, nd1 + 2);
    assert!((caret_x(&ln, nd1, Affinity::Downstream).0 - nx3_1).abs() <= 0.5, "nested before 123 -> right of 3 (L1 RTL)");
    assert!((caret_x(&ln, nd1 + 3, Affinity::Downstream).0 - nx1_0).abs() <= 0.5, "nested after 123 -> left of 1 (L1 RTL)");
}

#[test]
fn visual_step_and_hit_stay_consistent_across_embedded_ltr_runs() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(15.0, 400.0);

    // Visual stops on RTL "و123": right-to-left [0, 2, 4, 3, 5], left-to-right [5, 3, 4, 2, 0].
    let l = ts.layout_rich_with("و123", &[], style, 2.0, Some(400.0), RichLayoutOptions::new().direction(Direction::Auto));
    let mut at = (0usize, Affinity::Downstream);
    let mut left_walk = vec![at.0];
    while let Some(next) = visual_step(&l, at.0, at.1, false, false) {
        let prev_x = caret_x(&l, at.0, at.1).0;
        let next_x = caret_x(&l, next.0, next.1).0;
        assert!(next_x < prev_x - 0.5, "Left step must move strictly left: {at:?} ({prev_x:.1}) -> {next:?} ({next_x:.1})");
        at = next;
        left_walk.push(at.0);
    }
    assert_eq!(left_walk, vec![0, 2, 4, 3, 5]);

    let mut right_walk = vec![at.0];
    while let Some(next) = visual_step(&l, at.0, at.1, true, false) {
        let prev_x = caret_x(&l, at.0, at.1).0;
        let next_x = caret_x(&l, next.0, next.1).0;
        assert!(next_x > prev_x + 0.5, "Right step must move strictly right: {at:?} ({prev_x:.1}) -> {next:?} ({next_x:.1})");
        at = next;
        right_walk.push(at.0);
    }
    assert_eq!(right_walk, vec![5, 3, 4, 2, 0]);

    // Clicking far left / far right of the RTL line lands on end (5) / start (0).
    let mid_y = (l.get(0).unwrap().metrics().block_min_coord + l.get(0).unwrap().metrics().block_max_coord) * 0.5;
    assert_eq!(cursor_from_point(&l, 0.0, mid_y).index(), 5, "left of RTL line -> end of text");
    assert_eq!(cursor_from_point(&l, 420.0, mid_y).index(), 0, "right of RTL line -> start of text");

    // Double-click cluster hit-test identifies the clicked cluster (و vs 3) on each side of byte 2.
    let (_, x3_0, x3_1) = cluster_x_range(&l, 4);
    let (_, xv_0, xv_1) = cluster_x_range(&l, 0);
    let flow = TextFlow::Single(&l);
    let hit_3 = flow.hit(((x3_0 + x3_1) * 0.5) as f32 + (x3_1 - x3_0) as f32 * 0.25, mid_y);
    assert_eq!(hit_3.index, 2, "right half of 3 -> caret between 3 and و (byte 2)");
    assert_eq!(hit_3.before, Some(4), "right half of 3 -> cluster 3 (byte 4) for word selection");
    let hit_v = flow.hit(((xv_0 + xv_1) * 0.5) as f32 - (xv_1 - xv_0) as f32 * 0.25, mid_y);
    assert_eq!(hit_v.index, 2, "left half of و -> caret between 3 and و (byte 2)");
    assert_eq!(hit_v.before, Some(0), "left half of و -> cluster و (byte 0) for word selection");
}

#[test]
fn carets_parley_places_correctly_keep_identical_geometry() {
    let mut ts = TextSystem::default();
    let samples = [
        "",
        "Hello world",
        "Hello there\nsecond line\n\nfourth line",
        "Hello there\n",
        "first\r\nsecond",
        "Hello world with several words that wrap across lines cleanly",
        "Hello   world   spaced",
        "a👨‍👩‍👧🇯🇵👋🏽❤️b",
        "cafe\u{301} fi",
        "تمام يا أستاذ",
        "تمام يا أستاذ هذا نص عربي طويل يلتف على أكثر من سطر بدون فواصل صريحة",
    ];
    for text in samples {
        for width in [1000.0f32, 260.0f32] {
            let l = layout(&mut ts, text, Some(width));
            assert_carets_match_parley(&l, &format!("default w={width}"), text);
        }
    }

    let style = TextStyle::new(15.0, 400.0);
    let aligned = "First line here\nsecond longer line that wraps cleanly";
    for align in [Alignment::Center, Alignment::End, Alignment::Justify] {
        let l = ts.layout_aligned(aligned, &[], style, 2.0, Some(300.0), align);
        assert_carets_match_parley(&l, &format!("align={align:?}"), aligned);
    }

    let rich = "Big then small\nsecond line";
    let spans = [Span::new(0, 3).size(32.0)];
    let lr = ts.layout_rich(rich, &spans, style, 2.0, Some(600.0));
    assert_carets_match_parley(&lr, "multi-size spans", rich);

    let quoted = "Quoted line one\nquoted line two";
    let blocks = [Block::Quote, Block::Quote];
    let lq = ts.layout_blocks(quoted, &[], &blocks, &[], style, 2.0, Some(600.0), Alignment::Start);
    assert_carets_match_parley(&lq, "quoted block", quoted);
}

#[test]
fn replaced_pill_hit_testing_splits_at_midpoint_and_carets_bracket_image() {
    use parley::{editing::Cursor, layout::Affinity};
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;
    let body = "oi :gato: tudo";
    let start = 3u32;
    let end = 9u32;
    let img_w = 22.0f32;
    let pills = [PillChip {
        start,
        end,
        icon: None,
        image: None,
        bg: None,
        fg: None,
        ring: None,
        radius: None,
        pad_x: None,
        margin_x: None,
        bare: Some(Box::new(BareIcon {
            size: 0.0,
            gap: 0.0,
            radius: 0.0,
            image: None,
            replace: Some(Replace { width: img_w, ascent: 16.0, descent: 4.0, bleed: 0.0 }),
            label: None,
            help: None,
        })),
    }];
    let spec = TextInput {
        content: body,
        spans: &[],
        blocks: &[],
        pills: &pills,
        span_boxes: &[],
        inline_boxes: &[],
        text_indent: 0.0,
        style,
        shape_key: None,
        wrap_anywhere: false,
        pretty: false,
        redacted: false,
        by_line: false,
        by_paragraph: false,
        dir: TextDir::Auto,
    };
    let l = ts.layout_for_spec(&spec, scale, None, Alignment::Start);

    let mut boxes = Vec::new();
    bare_icons(&l, &pills, scale, [0.0, 0.0], &mut boxes);
    assert_eq!(boxes.len(), 1);
    let (_, at, sz) = boxes[0];
    assert!((sz[0] - img_w * scale).abs() <= 1.0, "replaced box width={}", sz[0]);

    // Carets before and after the pill bracket the replaced image box cleanly.
    let c_before = cursor_geometry(&Cursor::from_byte_index(&l, start as usize, Affinity::Downstream), &l, 2.0);
    let c_after = cursor_geometry(&Cursor::from_byte_index(&l, end as usize, Affinity::Upstream), &l, 2.0);
    assert!((c_before.x0 as f32 - at[0]).abs() <= 1.5, "before caret={} box_x={}", c_before.x0, at[0]);
    assert!(((c_after.x0 - c_before.x0) as f32 - sz[0]).abs() <= 1.5, "caret span={} box_w={}", c_after.x0 - c_before.x0, sz[0]);

    // Clicking the left half lands before the pill; clicking the right half lands after it.
    let mid_y = at[1] + sz[1] * 0.5;
    let hit_left = cursor_from_point(&l, at[0] + sz[0] * 0.25, mid_y);
    assert_eq!(hit_left.index(), start as usize);
    assert_eq!(hit_left.affinity(), Affinity::Downstream);
    let hit_right = cursor_from_point(&l, at[0] + sz[0] * 0.75, mid_y);
    assert_eq!(hit_right.index(), end as usize);
    assert_eq!(hit_right.affinity(), Affinity::Upstream);

    // Selection highlight (`pill_boxes = true`) covers the whole replaced image box.
    let sel = range_boxes(&l, start as usize, end as usize, true);
    assert_eq!(sel.len(), 1);
    assert!(((sel[0].0.x1 - sel[0].0.x0) as f32 - sz[0]).abs() <= 1.5, "selection width={}", sel[0].0.x1 - sel[0].0.x0);
}
