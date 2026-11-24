use parley::layout::Affinity;

use crate::layout::hash;
use crate::layout::rich::TextInput;
use crate::{
    Alignment, Flow, TextDir, TextLayout, TextStyle, TextSystem, cursor_from_point, cursor_geometry,
    range_boxes,
};

fn body() -> TextStyle {
    TextStyle::new(14.0, 400.0).lh(20.0)
}

fn spec<'a>(content: &'a str, text_indent: f32, dir: TextDir, pretty: bool, by_paragraph: bool) -> TextInput<'a> {
    TextInput {
        content,
        spans: &[],
        blocks: &[],
        pills: &[],
        span_boxes: &[],
        text_indent,
        style: body(),
        shape_key: None,
        wrap_anywhere: false,
        pretty,
        redacted: false,
        by_line: false,
        by_paragraph,
        dir,
    }
}

#[test]
fn positive_indent_shifts_only_first_line_and_shrinks_available_advance() {
    let mut ts = TextSystem::default();
    let sample = "First paragraph with enough words to wrap across multiple lines cleanly.
Second paragraph starts after an explicit newline and stays flush.";
    for scale in [1.0f32, 2.0] {
        let max_w = 200.0 * scale;
        let plain = ts.layout_for_spec(&spec(sample, 0.0, TextDir::Fixed(Flow::Ltr), false, false), scale, Some(max_w), Alignment::Start);
        let indented = ts.layout_for_spec(&spec(sample, 24.0, TextDir::Fixed(Flow::Ltr), false, false), scale, Some(max_w), Alignment::Start);

        assert!(indented.len() >= 3, "expected multiple wrapped lines");
        let l0 = indented.get(0).unwrap();
        let l1 = indented.get(1).unwrap();
        assert!((l0.metrics().indent - 24.0 * scale).abs() < 0.5);
        assert!((l0.metrics().offset - 24.0 * scale).abs() < 0.5);
        assert_eq!(l1.metrics().indent, 0.0);
        assert_eq!(l1.metrics().offset, 0.0);

        // Reduced first-line advance fits fewer or equal characters than unindented line 0, and
        // its right edge stays inside `max_w`.
        let p0 = plain.get(0).unwrap();
        assert!(l0.text_range().end <= p0.text_range().end);
        assert!(l0.metrics().offset + l0.metrics().advance - l0.metrics().hanging_advance <= max_w + 1.0);

        // Explicit `\n` paragraph stays unindented (CSS Text 3 §9.1 without `each-line`).
        let para2_byte = sample.find("Second").unwrap();
        let para2_line = indented.lines().find(|l| l.text_range().contains(&para2_byte)).unwrap();
        assert_eq!(para2_line.metrics().indent, 0.0);
        assert_eq!(para2_line.metrics().offset, 0.0);
    }
}

#[test]
fn negative_hanging_indent_pulls_line_zero_left_and_widens_first_line_advance() {
    let mut ts = TextSystem::default();
    let sample = "10:42 Ada: This compact chat message hangs its timestamp into the left padding and wraps its body cleanly.";
    for scale in [1.0f32, 2.0] {
        let max_w = 180.0 * scale;
        let plain = ts.layout_for_spec(&spec(sample, 0.0, TextDir::Fixed(Flow::Ltr), false, false), scale, Some(max_w), Alignment::Start);
        let hanging = ts.layout_for_spec(&spec(sample, -48.0, TextDir::Fixed(Flow::Ltr), false, false), scale, Some(max_w), Alignment::Start);

        assert!(hanging.len() >= 2, "expected hanging paragraph to wrap");
        let l0 = hanging.get(0).unwrap();
        let l1 = hanging.get(1).unwrap();
        assert!((l0.metrics().indent - (-48.0 * scale)).abs() < 0.5);
        assert!((l0.metrics().offset - (-48.0 * scale)).abs() < 0.5);
        assert_eq!(l1.metrics().indent, 0.0);
        assert_eq!(l1.metrics().offset, 0.0);

        // Negative text_indent widens available advance on line 0 (`max_w + 48*scale`), so line 0
        // packs strictly more text than unindented while still ending inside `max_w`.
        let p0 = plain.get(0).unwrap();
        assert!(
            l0.text_range().end > p0.text_range().end,
            "hanging line 0 must fit more text: hanging_end={} plain_end={}",
            l0.text_range().end,
            p0.text_range().end,
        );
        let right_edge = l0.metrics().offset + l0.metrics().advance - l0.metrics().hanging_advance;
        assert!(right_edge <= max_w + 1.0, "hanging line 0 right edge {right_edge} exceeded {max_w}");
    }
}

#[test]
fn rtl_mirrors_positive_and_negative_indent_to_inline_start_edge() {
    let mut ts = TextSystem::default();
    let arabic = "مرحبا بالعالم هذه فقرة عربية طويلة نسبيا تلتف عبر عدة أسطر لاختبار المسافة البادئة";
    let scale = 1.0;
    let max_w = 180.0;

    let pos = ts.layout_for_spec(&spec(arabic, 28.0, TextDir::Fixed(Flow::Rtl), false, false), scale, Some(max_w), Alignment::Start);
    let neg = ts.layout_for_spec(&spec(arabic, -28.0, TextDir::Fixed(Flow::Rtl), false, false), scale, Some(max_w), Alignment::Start);
    assert!(pos.is_rtl() && neg.is_rtl());
    assert!(pos.len() >= 2 && neg.len() >= 2);

    // Positive RTL indent shifts line 0's right (inline-start) edge inward by 28 px; line 1 ends at `max_w`.
    let p0 = *pos.get(0).unwrap().metrics();
    let p1 = *pos.get(1).unwrap().metrics();
    let p0_right = p0.offset + p0.advance;
    let p1_right = p1.offset + p1.advance;
    assert!((p0_right - (max_w - 28.0)).abs() < 1.5, "RTL positive indent right edge={p0_right}, expected {}", max_w - 28.0);
    assert!((p1_right - max_w).abs() < 1.5, "RTL line 1 right edge={p1_right}, expected {max_w}");

    // Negative RTL indent shifts line 0's right (inline-start) edge outward past `max_w` by 28 px,
    // fitting more characters on line 0 while keeping its left edge inside `>= 0`.
    let n0 = *neg.get(0).unwrap().metrics();
    let n1 = *neg.get(1).unwrap().metrics();
    let n0_right = n0.offset + n0.advance;
    let n1_right = n1.offset + n1.advance;
    assert!((n0_right - (max_w + 28.0)).abs() < 1.5, "RTL hanging right edge={n0_right}, expected {}", max_w + 28.0);
    assert!(n0.offset >= -1.0, "RTL hanging line 0 left edge should stay within container: {}", n0.offset);
    assert!((n1_right - max_w).abs() < 1.5, "RTL hanging line 1 right edge={n1_right}, expected {max_w}");
    assert!(neg.get(0).unwrap().text_range().end > pos.get(0).unwrap().text_range().end);
}

#[test]
fn hit_testing_caret_and_selection_boxes_on_indented_and_hanging_lines_ltr_and_rtl() {
    let mut ts = TextSystem::default();
    let ltr = "10:42 Ada: Hanging prefix sits in negative x while line two starts at zero cleanly.";
    let scale = 1.0;
    let max_w = 180.0;

    // 1. LTR hanging indent (-48).
    let hang = ts.layout_for_spec(&spec(ltr, -48.0, TextDir::Fixed(Flow::Ltr), false, false), scale, Some(max_w), Alignment::Start);
    let m0 = *hang.get(0).unwrap().metrics();
    let y0 = (m0.block_min_coord + m0.block_max_coord) * 0.5;
    let c0 = parley::editing::Cursor::from_byte_index(&hang, 0, Affinity::Downstream);
    let bb0 = cursor_geometry(&c0, &hang, 1.5);
    assert!((bb0.x0 as f32 - (-48.0)).abs() < 1.0, "LTR hanging caret at byte 0: {:?}", bb0);

    // Hit-test inside the hanging negative-x prefix hits leading bytes rather than clamping past them.
    let hit_start = cursor_from_point(&hang, -47.0, y0);
    assert_eq!(hit_start.index(), 0);
    let ada_byte = ltr.find("Ada").unwrap();
    let c_ada = parley::editing::Cursor::from_byte_index(&hang, ada_byte, Affinity::Downstream);
    let bb_ada = cursor_geometry(&c_ada, &hang, 1.5);
    assert!(bb_ada.x0 < 0.0, "Ada should sit inside negative hanging strip: {:?}", bb_ada);
    let hit_ada = cursor_from_point(&hang, bb_ada.x0 as f32 + 1.0, y0);
    assert!(
        (hit_ada.index() as isize - ada_byte as isize).abs() <= 1,
        "expected hit near Ada ({ada_byte}), got {}",
        hit_ada.index(),
    );

    // Selection boxes across line 0 and line 1 start at -48 on line 0 and 0 on line 1.
    let boxes = range_boxes(&hang, 0, ltr.len(), false);
    assert!(boxes.len() >= 2);
    assert!((boxes[0].0.x0 as f32 - (-48.0)).abs() < 1.0, "line 0 selection box: {:?}", boxes[0].0);
    assert!(boxes[1].0.x0.abs() < 1.0, "line 1 selection box: {:?}", boxes[1].0);

    // 2. LTR positive indent (+32).
    let pos = ts.layout_for_spec(&spec(ltr, 32.0, TextDir::Fixed(Flow::Ltr), false, false), scale, Some(max_w), Alignment::Start);
    let pm0 = *pos.get(0).unwrap().metrics();
    let py0 = (pm0.block_min_coord + pm0.block_max_coord) * 0.5;
    let p_bb0 = cursor_geometry(&parley::editing::Cursor::from_byte_index(&pos, 0, Affinity::Downstream), &pos, 1.5);
    assert!((p_bb0.x0 as f32 - 32.0).abs() < 1.0);
    assert_eq!(cursor_from_point(&pos, 4.0, py0).index(), 0, "clicking in positive indent margin lands at byte 0");

    // 3. RTL hanging indent (-28).
    let arabic = "مرحبا بالعالم هذه فقرة عربية طويلة نسبيا تلتف عبر عدة أسطر";
    let rtl_hang = ts.layout_for_spec(&spec(arabic, -28.0, TextDir::Fixed(Flow::Rtl), false, false), scale, Some(max_w), Alignment::Start);
    let rm0 = *rtl_hang.get(0).unwrap().metrics();
    let ry0 = (rm0.block_min_coord + rm0.block_max_coord) * 0.5;
    let r_bb0 = cursor_geometry(&parley::editing::Cursor::from_byte_index(&rtl_hang, 0, Affinity::Downstream), &rtl_hang, 1.5);
    assert!((r_bb0.x0 as f32 - (max_w + 28.0)).abs() < 2.0, "RTL hanging byte 0 caret: {:?}", r_bb0);
    let rtl_hit_start = cursor_from_point(&rtl_hang, max_w + 27.0, ry0);
    assert_eq!(rtl_hit_start.index(), 0, "clicking right hanging edge in RTL hits byte 0");
    let rtl_boxes = range_boxes(&rtl_hang, 0, arabic.len(), false);
    assert!(rtl_boxes.len() >= 2);
    assert!((rtl_boxes[0].0.x1 as f32 - (max_w + 28.0)).abs() < 2.0, "RTL line 0 selection extends into hanging right edge: {:?}", rtl_boxes[0].0);
    assert!((rtl_boxes[1].0.x1 as f32 - max_w).abs() < 2.0, "RTL line 1 selection ends at container edge: {:?}", rtl_boxes[1].0);
}

#[test]
fn pretty_wrap_respects_positive_and_negative_first_line_indent() {
    let mut ts = TextSystem::default();
    let prose = "A short prose paragraph designed to end with a single orphan word.";
    let max_w = 190.0;

    for (indent, align) in [
        (24.0, Alignment::Start),
        (-32.0, Alignment::Start),
        (24.0, Alignment::Center),
        (-32.0, Alignment::End),
    ] {
        let laid = ts.layout_for_spec(&spec(prose, indent, TextDir::Fixed(Flow::Ltr), true, false), 1.0, Some(max_w), align);
        assert!(laid.len() >= 2);
        let l0 = *laid.get(0).unwrap().metrics();
        assert!((l0.indent - indent).abs() < 0.5);
        for (i, line) in laid.lines().enumerate() {
            let m = line.metrics();
            let used = m.advance - m.hanging_advance;
            let avail = max_w - if i == 0 { indent } else { 0.0 };
            assert!(used <= avail + 1.5, "line {i} overflowed under pretty wrap (indent={indent}, align={align:?}): used={used} avail={avail}");
            if align == Alignment::End {
                assert!((m.offset + used - max_w).abs() < 2.0, "End alignment stayed anchored to max_w: offset={} used={used}", m.offset);
            }
        }
    }
}

#[test]
fn cache_key_separates_indents_and_rebreaks_without_reshaping() {
    let mut ts = TextSystem::default();
    let sample = "Rebreaking a cached paragraph across text_indent values never shapes twice.";
    let s0 = spec(sample, 0.0, TextDir::Fixed(Flow::Ltr), false, false);
    let shape = s0.fresh_shape_key(1.0);
    assert_eq!(hash::break_key(shape, None, Alignment::Start, None, 0.0), shape);
    assert_eq!(hash::break_key(shape, None, Alignment::Start, None, -0.0), shape);
    assert_ne!(hash::break_key(shape, None, Alignment::Start, None, 16.0), shape);
    assert_ne!(
        hash::break_key(shape, Some(180.0), Alignment::Start, None, 16.0),
        hash::break_key(shape, Some(180.0), Alignment::Start, None, -16.0),
    );

    let before = ts.shaped;
    let l0 = ts.layout_for_spec(&s0, 1.0, Some(180.0), Alignment::Start);
    let l_pos = ts.layout_for_spec(&spec(sample, 24.0, TextDir::Fixed(Flow::Ltr), false, false), 1.0, Some(180.0), Alignment::Start);
    let l_neg = ts.layout_for_spec(&spec(sample, -24.0, TextDir::Fixed(Flow::Ltr), false, false), 1.0, Some(180.0), Alignment::Start);
    let l0_again = ts.layout_for_spec(&s0, 1.0, Some(180.0), Alignment::Start);

    assert_eq!(ts.shaped - before, 1, "changing text_indent must rebreak without reshaping");
    assert_eq!(l0.get(0).unwrap().metrics().offset, 0.0);
    assert!((l_pos.get(0).unwrap().metrics().offset - 24.0).abs() < 0.5);
    assert!((l_neg.get(0).unwrap().metrics().offset - (-24.0)).abs() < 0.5);
    assert_eq!(l0_again.get(0).unwrap().metrics().offset, 0.0);
}

#[test]
fn edge_cases_empty_zero_negative_zero_huge_and_non_finite() {
    let mut ts = TextSystem::default();
    let max_w = 180.0;

    // Empty string with positive and negative indent stays finite and places caret at indent.
    for indent in [0.0, 24.0, -24.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let l = ts.layout_for_spec(&spec("", indent, TextDir::Fixed(Flow::Ltr), false, false), 1.0, Some(max_w), Alignment::Start);
        let bb = cursor_geometry(&parley::editing::Cursor::from_byte_index(&l, 0, Affinity::Downstream), &l, 1.5);
        assert!(bb.x0.is_finite() && bb.x1.is_finite());
        let expected = if indent.is_finite() { indent } else { 0.0 };
        assert!((bb.x0 as f32 - expected).abs() < 0.5, "empty caret x0={} expected={expected}", bb.x0);
    }

    // 0.0 and -0.0 produce identical layouts.
    let sample = "Edge test words wrapping neatly.";
    let lz = ts.layout_for_spec(&spec(sample, 0.0, TextDir::Fixed(Flow::Ltr), false, false), 1.0, Some(100.0), Alignment::Start);
    let lnz = ts.layout_for_spec(&spec(sample, -0.0, TextDir::Fixed(Flow::Ltr), false, false), 1.0, Some(100.0), Alignment::Start);
    assert_eq!(lz.get(0).unwrap().metrics().offset, lnz.get(0).unwrap().metrics().offset);

    // Huge positive and negative indents never panic or produce NaN and still place text on line 0.
    for huge in [10_000.0, -10_000.0, 1e20, -1e20] {
        let l = ts.layout_for_spec(&spec(sample, huge, TextDir::Fixed(Flow::Ltr), true, false), 1.0, Some(120.0), Alignment::Start);
        assert!(!l.is_empty());
        let m0 = *l.get(0).unwrap().metrics();
        assert!(m0.offset.is_finite() && m0.advance.is_finite() && m0.indent.is_finite());
        assert!(!l.get(0).unwrap().text_range().is_empty(), "line 0 keeps at least its first word under huge indent {huge}");
    }
}

#[test]
fn by_paragraph_indents_only_first_paragraph_and_rebreaks_on_indent_change() {
    let mut ts = TextSystem::default();
    let multi = "First paragraph wraps onto another line when narrow.
Second paragraph stays unindented.";
    let first = ts.layout_text(&spec(multi, 24.0, TextDir::Fixed(Flow::Ltr), false, true), true, 1.0, Some(160.0), Alignment::Start, None);
    let TextLayout::Paragraphs(pl) = &first else { panic!("expected ParagraphLayout") };
    assert_eq!(pl.paragraphs.len(), 2);
    assert!((pl.paragraphs[0].layout.get(0).unwrap().metrics().offset - 24.0).abs() < 0.5);
    assert_eq!(pl.paragraphs[1].layout.get(0).unwrap().metrics().offset, 0.0);

    // Updating text_indent rebreaks paragraph 0 instead of reusing the stale offset.
    let updated = ts.layout_text(&spec(multi, -32.0, TextDir::Fixed(Flow::Ltr), false, true), true, 1.0, Some(160.0), Alignment::Start, Some(&first));
    let TextLayout::Paragraphs(pl2) = &updated else { panic!("expected ParagraphLayout") };
    assert!((pl2.paragraphs[0].layout.get(0).unwrap().metrics().offset - (-32.0)).abs() < 0.5);
    assert_eq!(pl2.paragraphs[1].layout.get(0).unwrap().metrics().offset, 0.0);

    // Blank first paragraph still places byte-0 caret at the indented start.
    let blank_first = ts.layout_text(&spec("
Second line", 20.0, TextDir::Fixed(Flow::Ltr), false, true), true, 1.0, Some(160.0), Alignment::Start, None);
    let caret = blank_first.view().caret(0, Affinity::Downstream, 1.5);
    assert!((caret.x0 as f32 - 20.0).abs() < 0.5, "blank first paragraph caret={:?}", caret);
}
