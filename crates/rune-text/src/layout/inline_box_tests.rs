//! Unit tests for in-flow `InlineBox` / `InlineAlign` layout, hashing, measurement,
//! caret hit testing, and selection ranges.

use parley::{Alignment, PositionedInlineBox, PositionedLayoutItem};
use rune_core::tok::TextStyle;

use crate::{
    Direction, INLINE_BOX, InlineAlign, InlineBox, InlineBoxRun, RichLayoutOptions,
    TextInput, TextSystem, cursor_from_point, cursor_geometry, measure, measure_inline,
    measure_inline_with, measure_with, mix_inline_boxes, range_boxes, shape_key, spec_height,
    text_width,
};

fn positioned_boxes(layout: &parley::Layout<crate::Ink>) -> Vec<(usize, PositionedInlineBox)> {
    let mut out = Vec::new();
    for (li, line) in layout.lines().enumerate() {
        for item in line.items() {
            if let PositionedLayoutItem::InlineBox(b) = item
                && b.id & INLINE_BOX != 0
            {
                out.push((li, b));
            }
        }
    }
    out
}

fn lay(
    ts: &mut TextSystem,
    text: &str,
    boxes: &[InlineBoxRun],
    style: TextStyle,
    scale: f32,
    max_width: Option<f32>,
    options: RichLayoutOptions,
) -> std::rc::Rc<parley::Layout<crate::Ink>> {
    ts.layout_inline_with(text, &[], boxes, style, scale, max_width, options)
}

#[test]
fn empty_slice_keeps_shape_key_and_layout_bit_identical() {
    let style = TextStyle::new(14.0, 400.0);
    let body = "Open the file and edit it.";
    let base_key = shape_key(body, &[], style, 2.0, false, &[]);
    assert_eq!(mix_inline_boxes(base_key, &[]), base_key);

    let with_box = [InlineBoxRun { at: 5, slot: InlineBox::new(40.0, 16.0), child: 0 }];
    assert_ne!(mix_inline_boxes(base_key, &with_box), base_key);

    // Child ordinal does not affect shaping key.
    let with_child_9 = [InlineBoxRun { at: 5, slot: InlineBox::new(40.0, 16.0), child: 9 }];
    assert_eq!(mix_inline_boxes(base_key, &with_box), mix_inline_boxes(base_key, &with_child_9));

    // Empty inline boxes measure bit-identically to measure / measure_with.
    assert_eq!(measure(body, &[], style, Some(180.0)), measure_inline(body, &[], &[], style, Some(180.0)));
    let opts = RichLayoutOptions::new().wrap_anywhere().align(Alignment::Center);
    assert_eq!(
        measure_with(body, &[], style, Some(120.0), opts),
        measure_inline_with(body, &[], &[], style, Some(120.0), opts),
    );

    // Layout with empty inline_boxes is bit-identical to layout_rich.
    let mut ts = TextSystem::default();
    let plain = ts.layout_rich(body, &[], style, 2.0, Some(220.0));
    let empty_inline = lay(&mut ts, body, &[], style, 2.0, Some(220.0), RichLayoutOptions::default());
    assert_eq!(plain.width().to_bits(), empty_inline.width().to_bits());
    assert_eq!(plain.full_width().to_bits(), empty_inline.full_width().to_bits());
    assert_eq!(plain.height().to_bits(), empty_inline.height().to_bits());
    assert_eq!(plain.len(), empty_inline.len());
}

#[test]
fn inline_box_clamps_negative_and_non_finite_values() {
    let bad = InlineBox::new(-10.0, f32::NAN).baseline(f32::INFINITY);
    assert_eq!(bad.width(), 0.0);
    assert_eq!(bad.height(), 0.0);
    assert_eq!(bad.baseline_offset(), Some(0.0));

    let neg_base = InlineBox::new(f32::NEG_INFINITY, -0.5).baseline(-4.0);
    assert_eq!(neg_base.width(), 0.0);
    assert_eq!(neg_base.height(), 0.0);
    assert_eq!(neg_base.baseline_offset(), Some(0.0));

    // Huge finite dimension never produces NaN/Inf in layout.
    let huge = InlineBox::new(f32::MAX, 16.0);
    let (w, h) = measure_inline("a b", &[], &[(1, huge)], TextStyle::new(14.0, 400.0), Some(200.0));
    assert!(w.is_finite() && h.is_finite());
}

#[test]
fn all_inline_align_variants_and_custom_baseline_position_correctly() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;
    let body = "Open  now";

    let run = |ts: &mut TextSystem, slot: InlineBox| {
        let boxes = [InlineBoxRun { at: 5, slot, child: 0 }];
        let l = lay(ts, body, &boxes, style, scale, None, RichLayoutOptions::default());
        let line = l.get(0).unwrap();
        let m = *line.metrics();
        let placed = positioned_boxes(&l);
        assert_eq!(placed.len(), 1);
        (l, m, placed[0].1.clone())
    };

    // Baseline (default): bottom of box sits on line baseline.
    let (_, m_base, b_base) = run(&mut ts, InlineBox::new(20.0, 12.0));
    assert!(
        ((b_base.y + b_base.height) - m_base.baseline).abs() <= 1.0,
        "default Baseline sits on line baseline: y={} h={} baseline={}",
        b_base.y,
        b_base.height,
        m_base.baseline
    );

    // Custom baseline(6.0) on 12pt box: midpoint sits on line baseline (6pt above, 6pt below).
    let (_, m_cb, b_cb) = run(&mut ts, InlineBox::new(20.0, 12.0).baseline(6.0));
    assert!(
        ((b_cb.y + 6.0 * scale) - m_cb.baseline).abs() <= 1.0,
        "custom baseline sits at y + baseline*scale: y={} baseline={}",
        b_cb.y,
        m_cb.baseline
    );
    assert!(b_cb.y > b_base.y, "lowering box baseline pushes box down relative to default bottom baseline");

    // TextTop sits higher than TextBottom for a small box.
    let (_, _, b_top) = run(&mut ts, InlineBox::new(20.0, 8.0).align(InlineAlign::TextTop));
    let (_, _, b_bot) = run(&mut ts, InlineBox::new(20.0, 8.0).align(InlineAlign::TextBottom));
    assert!(b_top.y < b_bot.y, "TextTop ({}) must sit above TextBottom ({})", b_top.y, b_bot.y);

    // Middle sits between TextTop and TextBottom.
    let (_, _, b_mid) = run(&mut ts, InlineBox::new(20.0, 8.0).align(InlineAlign::Middle));
    assert!(
        b_top.y < b_mid.y && b_mid.y < b_bot.y,
        "Middle ({}) must sit between TextTop ({}) and TextBottom ({})",
        b_mid.y,
        b_top.y,
        b_bot.y
    );

    // TextCenter centres a taller box symmetrically around the font's ascent/descent box.
    let plain = ts.layout_rich(body, &[], style, scale, None);
    let plain_h = plain.height();
    let (l_tall, m_tall, b_tall) = run(&mut ts, InlineBox::new(24.0, 36.0).align(InlineAlign::TextCenter));
    assert!(l_tall.height() > plain_h + 10.0, "taller TextCenter box grows line height: {} vs {}", l_tall.height(), plain_h);
    let above_baseline = m_tall.baseline - b_tall.y;
    let below_baseline = (b_tall.y + b_tall.height) - m_tall.baseline;
    assert!(
        above_baseline > 0.0 && below_baseline > 0.0 && (above_baseline - below_baseline).abs() < style.size * scale,
        "TextCenter splits tall box symmetrically around text: above={above_baseline} below={below_baseline}"
    );
}

#[test]
fn box_wraps_whole_and_never_splits() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let body = "Open  now";
    let slot = InlineBox::new(80.0, 16.0).align(InlineAlign::TextCenter);
    let boxes = [InlineBoxRun { at: 5, slot, child: 0 }];

    // Wide enough for one line.
    let wide = lay(&mut ts, body, &boxes, style, 1.0, Some(300.0), RichLayoutOptions::default());
    assert_eq!(wide.len(), 1);
    let p_wide = positioned_boxes(&wide);
    assert_eq!(p_wide.len(), 1);
    assert_eq!(p_wide[0].0, 0);
    assert!((p_wide[0].1.width - 80.0).abs() < 0.01);

    // Narrower than "Open " + 80pt: box moves whole onto line 1.
    let open_w = measure("Open ", &[], style, None).0;
    let cap = open_w + 40.0; // enough for "Open " and half the box, not the whole box
    let wrapped = lay(&mut ts, body, &boxes, style, 1.0, Some(cap), RichLayoutOptions::default());
    assert!(wrapped.len() >= 2, "expected wrap onto second line, got {} lines", wrapped.len());
    let p_wrap = positioned_boxes(&wrapped);
    assert_eq!(p_wrap.len(), 1);
    assert_eq!(p_wrap[0].0, 1, "box must move whole to line 1");
    assert!((p_wrap[0].1.width - 80.0).abs() < 0.01, "box width stays atomic");
}

#[test]
fn trailing_end_offset_empty_string_multiple_and_out_of_order_boxes() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);

    // Trailing box at at == text.len() (e.g. external-link arrow).
    let link = "Docs";
    let arrow = InlineBox::new(12.0, 12.0).align(InlineAlign::TextCenter);
    let (plain_w, _) = measure(link, &[], style, None);
    let (with_w, _) = measure_inline(link, &[], &[(link.len(), arrow)], style, None);
    assert!((with_w - plain_w - 12.0).abs() <= 0.5, "trailing box adds width: {plain_w} -> {with_w}");

    // Empty string with an inline box reserves the box's exact room.
    let solo = InlineBox::new(48.0, 28.0);
    let l_empty = lay(
        &mut ts,
        "",
        &[InlineBoxRun { at: 0, slot: solo, child: 0 }],
        style,
        1.0,
        None,
        RichLayoutOptions::default(),
    );
    let p_empty = positioned_boxes(&l_empty);
    assert_eq!(p_empty.len(), 1);
    assert!((p_empty[0].1.width - 48.0).abs() < 0.01);
    assert!((p_empty[0].1.height - 28.0).abs() < 0.01);
    let (ew, eh) = measure_inline("", &[], &[(0, solo)], style, None);
    assert!((ew - 48.0).abs() <= 0.5 && eh >= 28.0, "empty string measure: ({ew}, {eh})");

    // Multiple boxes supplied out of order still lay out in logical order with preserved ids.
    let body = "A B C";
    let boxes = [
        InlineBoxRun { at: 4, slot: InlineBox::new(20.0, 14.0), child: 1 },
        InlineBoxRun { at: 2, slot: InlineBox::new(30.0, 14.0), child: 0 },
    ];
    let l_multi = lay(&mut ts, body, &boxes, style, 1.0, None, RichLayoutOptions::default());
    let placed = positioned_boxes(&l_multi);
    assert_eq!(placed.len(), 2);
    let b0 = &placed.iter().find(|(_, b)| b.id == INLINE_BOX).unwrap().1;
    let b1 = &placed.iter().find(|(_, b)| b.id == (INLINE_BOX | 1)).unwrap().1;
    assert!(b1.x < b0.x, "box at byte 2 ({}) precedes box at byte 4 ({})", b1.x, b0.x);

    // Out-of-bounds and mid-UTF-8-char offsets are silently ignored.
    let utf8 = "café";
    let invalid = [
        InlineBoxRun { at: 4, slot: InlineBox::new(20.0, 14.0), child: 0 }, // inside 'é' (bytes 3..5)
        InlineBoxRun { at: 99, slot: InlineBox::new(20.0, 14.0), child: 1 },
    ];
    let l_inv = lay(&mut ts, utf8, &invalid, style, 1.0, None, RichLayoutOptions::default());
    assert!(positioned_boxes(&l_inv).is_empty());
}

#[test]
fn rtl_text_places_inline_box_in_visual_order() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    // Hebrew "שלום  עולם" with box between the two words.
    let rtl = "שלום  עולם";
    let mid = "שלום ".len();
    let slot = InlineBox::new(32.0, 16.0).align(InlineAlign::TextCenter);
    let boxes = [InlineBoxRun { at: mid as u32, slot, child: 0 }];
    let opts = RichLayoutOptions::new().direction(Direction::Rtl);
    let l = lay(&mut ts, rtl, &boxes, style, 1.0, Some(240.0), opts);
    let placed = positioned_boxes(&l);
    assert_eq!(placed.len(), 1);
    let b = &placed[0].1;
    assert!((b.width - 32.0).abs() < 0.01);
    // In RTL with width 240, the line aligns to the right edge so the box sits in the right half.
    assert!(b.x > 100.0 && b.x + b.width <= 240.5, "RTL box visual x={}..{}", b.x, b.x + b.width);

    // Clicking inside the RTL inline box snaps cleanly to its byte index.
    let hit = cursor_from_point(&l, b.x + b.width * 0.5, b.y + b.height * 0.5);
    assert_eq!(hit.index(), mid);
}

#[test]
fn caret_hit_test_and_selection_pass_over_inline_box_like_inline_block() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let body = "Open  and edit";
    let slot = InlineBox::new(50.0, 16.0).align(InlineAlign::TextCenter);
    let boxes = [InlineBoxRun { at: 5, slot, child: 0 }];
    let l = lay(&mut ts, body, &boxes, style, 1.0, None, RichLayoutOptions::default());
    let placed = positioned_boxes(&l);
    assert_eq!(placed.len(), 1);
    let b = &placed[0].1;

    // Clicking anywhere inside the inline box snaps cleanly to byte 5.
    for frac in [0.1, 0.5, 0.9] {
        let c = cursor_from_point(&l, b.x + b.width * frac, b.y + b.height * 0.5);
        assert_eq!(c.index(), 5, "click at frac {frac} inside inline box snaps to ib.index");
    }

    // Selection range across byte 5 highlights glyphs before and after, skipping the box's interior.
    let sel = range_boxes(&l, 0, body.len(), true);
    assert_eq!(sel.len(), 2, "selection splits cleanly around the atomic inline box: {sel:?}");
    assert!(sel[0].0.x1 <= f64::from(b.x) + 1.0, "left selection stops at box start: {:?} vs {}", sel[0].0, b.x);
    assert!(sel[1].0.x0 >= f64::from(b.x + b.width) - 1.0, "right selection starts after box: {:?} vs {}", sel[1].0, b.x + b.width);

    // Link range_boxes(..., false) also excludes the inline box and keeps exact glyph bounds.
    let link_left = range_boxes(&l, 0, 4, false);
    assert_eq!(link_left.len(), 1);
    assert!(link_left[0].0.x1 <= f64::from(b.x));
    let link_right = range_boxes(&l, 6, 9, false);
    assert_eq!(link_right.len(), 1);
    assert!(link_right[0].0.x0 >= f64::from(b.x + b.width));

    // Caret geometry at byte 5 is finite and sits at the box boundary.
    let cur = parley::editing::Cursor::from_byte_index(&l, 5, parley::layout::Affinity::Downstream);
    let bb = cursor_geometry(&cur, &l, 1.5);
    assert!(bb.x0.is_finite() && (bb.x0 - f64::from(b.x)).abs() <= 1.5);

    // Box-only empty string: clicking left, inside, or right of the box stays at byte 0 without panicking.
    let l_solo = lay(
        &mut ts,
        "",
        &[InlineBoxRun { at: 0, slot, child: 0 }],
        style,
        1.0,
        None,
        RichLayoutOptions::default(),
    );
    assert_eq!(cursor_from_point(&l_solo, -5.0, 5.0).index(), 0);
    assert_eq!(cursor_from_point(&l_solo, 25.0, 5.0).index(), 0);
    assert_eq!(cursor_from_point(&l_solo, 999.0, 5.0).index(), 0);
}

#[test]
fn measure_inline_matches_layout_to_the_bit() {
    let style = TextStyle::new(14.0, 400.0);
    let body = "Open  and check  done";
    let b0 = InlineBox::new(64.0, 18.0).align(InlineAlign::TextCenter);
    let b1 = InlineBox::new(16.0, 24.0).align(InlineAlign::Baseline);
    let pairs = [(5, b0), (16, b1)];
    let runs = [
        InlineBoxRun { at: 5, slot: b0, child: 0 },
        InlineBoxRun { at: 16, slot: b1, child: 1 },
    ];
    let mut ts = TextSystem::default();
    for max_w in [None, Some(220.0), Some(110.0), Some(0.0)] {
        let measured = crate::lend(&mut ts, || measure_inline(body, &[], &pairs, style, max_w));
        let input = TextInput {
            content: body,
            spans: &[],
            blocks: &[],
            pills: &[],
            span_boxes: &[],
            inline_boxes: &runs,
            text_indent: 0.0,
            style,
            shape_key: None,
            wrap_anywhere: false,
            pretty: false,
            redacted: false,
            by_line: false,
            by_paragraph: false,
            dir: crate::TextDir::Auto,
        };
        let laid = ts.layout_for_spec(&input, 1.0, max_w, Alignment::Start);
        let w = text_width(if max_w.is_some() { laid.width() } else { laid.full_width() });
        let h = spec_height(&laid, style, 1.0, true);
        assert_eq!(measured.0.to_bits(), w.to_bits(), "width parity at max_w={max_w:?}");
        assert_eq!(measured.1.to_bits(), h.to_bits(), "height parity at max_w={max_w:?}");
    }
}
