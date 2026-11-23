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
