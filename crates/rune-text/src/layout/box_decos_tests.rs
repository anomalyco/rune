use super::*;
use crate::layout::rich::TextInput;
use parley::Alignment;

const KEY_BG: Color = Color { r: 0.2, g: 0.25, b: 0.35, a: 1.0 };
const VAL_BG: Color = Color { r: 0.15, g: 0.4, b: 0.75, a: 1.0 };
const RING: Color = Color { r: 0.5, g: 0.7, b: 1.0, a: 1.0 };
const OVER_C: Color = Color { r: 0.9, g: 0.2, b: 0.2, a: 1.0 };
const STRIKE_C: Color = Color { r: 0.2, g: 0.8, b: 0.3, a: 1.0 };
const UNDER_C: Color = Color { r: 0.2, g: 0.4, b: 0.95, a: 1.0 };

fn layout_with_boxes(
    ts: &mut TextSystem,
    content: &str,
    span_boxes: &[SpanBoxRun],
    style: TextStyle,
    scale: f32,
    max_width: Option<f32>,
    dir: TextDir,
) -> Rc<Layout<Ink>> {
    let input = TextInput {
        content,
        spans: &[],
        blocks: &[],
        pills: &[],
        span_boxes,
        text_indent: 0.0,
        style,
        shape_key: None,
        wrap_anywhere: false,
        pretty: false,
        redacted: false,
        by_line: false,
        by_paragraph: false,
        dir,
    };
    ts.layout_for_spec(&input, scale, max_width, Alignment::Start)
}

#[test]
fn span_box_padding_grows_layout_width_and_adjacent_chips_meet_without_overlap() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let scale = 2.0;
    let src = "search from:ada now";
    let key_s = src.find("from:").unwrap();
    let key_e = key_s + "from:".len();
    let val_e = key_e + "ada".len();

    let plain = layout_with_boxes(&mut ts, src, &[], style, scale, None, TextDir::Auto);
    let boxes = vec![
        SpanBoxRun {
            start: key_s as u32,
            end: key_e as u32,
            style: SpanBox::new().bg(KEY_BG).pad_x(6.0, 4.0).corners([5.0, 0.0, 0.0, 5.0]),
        },
        SpanBoxRun {
            start: key_e as u32,
            end: val_e as u32,
            style: SpanBox::new()
                .bg(VAL_BG)
                .pad_x(4.0, 6.0)
                .corners([0.0, 5.0, 5.0, 0.0])
                .border(RING, 1.0),
        },
    ];
    let padded = layout_with_boxes(&mut ts, src, &boxes, style, scale, None, TextDir::Auto);

    // Total added horizontal padding = (6 + 4 + 4 + 6) pt = 20 pt = 40 device px at scale 2.
    let grew = padded.full_width() - plain.full_width();
    assert!((grew - 40.0).abs() < 1.5, "expected +40 px width growth, got {grew}");

    // Padded shape key differs from plain; changing only bg/corners keeps the same shape key.
    let base_key = shape_key(src, &[], style, scale, false, &[]);
    let pad_key = mix_span_boxes(base_key, &boxes);
    assert_ne!(pad_key, base_key);
    let recolored = vec![
        SpanBoxRun { style: boxes[0].style.bg(VAL_BG).corners([2.0; 4]), ..boxes[0] },
        boxes[1],
    ];
    assert_eq!(mix_span_boxes(base_key, &recolored), pad_key);

    let mut out = Vec::new();
    span_box_decorations(&padded, &boxes, style.size, scale, [10.0, 20.0], &mut out);
    assert_eq!(out.len(), 2, "adjacent boxes must never merge");
    let (k, v) = (&out[0], &out[1]);
    assert_eq!(k.color, KEY_BG);
    assert_eq!(v.color, VAL_BG);
    assert_eq!(k.corners, Some([10.0, 0.0, 0.0, 10.0]));
    assert_eq!(v.corners, Some([0.0, 10.0, 10.0, 0.0]));
    assert_eq!(k.ring, None);
    assert_eq!(v.ring, Some((RING, 2.0)));
    // Right edge of key box equals left edge of value box pixel-for-pixel, same vertical band.
    assert!((k.rect[0] + k.rect[2] - v.rect[0]).abs() < 0.01, "k={:?} v={:?}", k.rect, v.rect);
    assert_eq!(k.rect[1], v.rect[1], "adjacent chips share top edge");
    assert_eq!(k.rect[3], v.rect[3], "adjacent chips share height");
}

#[test]
fn span_box_zero_pad_covers_exact_subrange_without_changing_shape_key() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let src = "hello world";
    let boxes = vec![SpanBoxRun {
        start: 6,
        end: 11,
        style: SpanBox::new().bg(KEY_BG).corners([3.0, 1.0, 2.0, 4.0]),
    }];
    let base_key = shape_key(src, &[], style, 1.0, false, &[]);
    assert_eq!(mix_span_boxes(base_key, &boxes), base_key, "zero-pad SpanBox adds no shaping key salt");

    let layout = layout_with_boxes(&mut ts, src, &boxes, style, 1.0, None, TextDir::Auto);
    let mut out = Vec::new();
    span_box_decorations(&layout, &boxes, style.size, 1.0, [0.0, 0.0], &mut out);
    assert_eq!(out.len(), 1);
    assert!(out[0].rect[0] > 15.0, "starts at 'world', not 'hello': {:?}", out[0].rect);
    assert!(out[0].rect[2] > 15.0 && out[0].rect[0] + out[0].rect[2] <= layout.full_width() + 1.5);
    assert_eq!(out[0].corners, Some([3.0, 1.0, 2.0, 4.0]));
}

#[test]
fn span_box_wraps_cleanly_slicing_corners_and_sanitizing_bad_numbers() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let src = "prefix alpha beta gamma delta suffix";
    let start = src.find("alpha").unwrap();
    let end = src.find("delta").unwrap() + "delta".len();
    let boxes = vec![SpanBoxRun {
        start: start as u32,
        end: end as u32,
        style: SpanBox::new().bg(KEY_BG).pad(6.0).corners([5.0, 6.0, 7.0, 8.0]),
    }];
    let layout = layout_with_boxes(&mut ts, src, &boxes, style, 1.0, Some(110.0), TextDir::Auto);
    assert!(layout.len() >= 2, "expected wrapped lines, got {}", layout.len());

    let mut out = Vec::new();
    span_box_decorations(&layout, &boxes, style.size, 1.0, [0.0, 0.0], &mut out);
    assert!(out.len() >= 2, "wrapped span box emits one rect per line, got {}", out.len());
    // First line keeps left corners [tl, 0, 0, bl]; last line keeps right corners [0, tr, br, 0].
    let first = out.first().unwrap().corners.unwrap();
    let last = out.last().unwrap().corners.unwrap();
    assert_eq!((first[0], first[1], first[2], first[3]), (5.0, 0.0, 0.0, 8.0));
    assert_eq!((last[0], last[1], last[2], last[3]), (0.0, 6.0, 7.0, 0.0));
    for mid in &out[1..out.len() - 1] {
        assert_eq!(mid.corners, Some([0.0; 4]));
    }

    // Negative / NaN / infinite inputs sanitize to 0 or None cleanly.
    let bad = SpanBox::new()
        .pad(-4.0)
        .pad_x(f32::NAN, f32::INFINITY)
        .corners([-1.0, f32::NAN, f32::INFINITY, 3.0])
        .border(RING, 0.0);
    assert_eq!((bad.pad_left(), bad.pad_right()), (0.0, 0.0));
    assert_eq!(bad.corner_radii(), [0.0, 0.0, 0.0, 3.0]);
    assert_eq!(bad.border_ring(), None);
    let neg_border = SpanBox::new().border(RING, -2.0);
    assert_eq!(neg_border.border_ring(), None);
}

#[test]
fn span_box_rtl_grows_width_and_slices_corners_in_visual_order() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let src = "مرحبا بالعالم الجميل";
    let first_end = "مرحبا".len();
    let boxes = vec![SpanBoxRun {
        start: 0,
        end: first_end as u32,
        style: SpanBox::new().bg(KEY_BG).pad_x(5.0, 7.0).corners([4.0, 6.0, 6.0, 4.0]),
    }];
    let plain = layout_with_boxes(&mut ts, src, &[], style, 1.0, None, TextDir::Fixed(Flow::Rtl));
    let padded = layout_with_boxes(&mut ts, src, &boxes, style, 1.0, None, TextDir::Fixed(Flow::Rtl));
    let grew = padded.full_width() - plain.full_width();
    assert!((grew - 12.0).abs() < 1.5, "RTL span box width grew by {grew}, expected ~12");

    let mut out = Vec::new();
    span_box_decorations(&padded, &boxes, style.size, 1.0, [0.0, 0.0], &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].corners, Some([4.0, 6.0, 6.0, 4.0]));
    assert!(out[0].rect[2] >= 12.0);

    // Wrapped RTL span box: line 0 gets visual-right corners, last line gets visual-left corners.
    let whole = vec![SpanBoxRun {
        start: 0,
        end: src.len() as u32,
        style: SpanBox::new().bg(KEY_BG).pad(4.0).corners([2.0, 5.0, 6.0, 3.0]),
    }];
    let wrapped = layout_with_boxes(&mut ts, src, &whole, style, 1.0, Some(70.0), TextDir::Fixed(Flow::Rtl));
    if wrapped.len() >= 2 {
        let mut wout = Vec::new();
        span_box_decorations(&wrapped, &whole, style.size, 1.0, [0.0, 0.0], &mut wout);
        assert!(wout.len() >= 2);
        assert_eq!(wout.first().unwrap().corners, Some([0.0, 5.0, 6.0, 0.0]));
        assert_eq!(wout.last().unwrap().corners, Some([2.0, 0.0, 0.0, 3.0]));
    }
}

#[test]
fn overline_sits_above_strike_and_underline_splits_on_wrap_and_follows_font_size() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0).lh(22.0);
    let scale = 2.0;
    let origin = [10.0, 20.0];
    let src = "OVER plain\nwrap this overlined text across lines";
    let spans = vec![Span::new(0, 4).color(UNDER_C).underline(UNDER_C)];
    let layout = ts.layout_rich(src, &spans, style, scale, Some(180.0));

    let mut under = Vec::new();
    ts.decorations_with_rings(&layout, &spans, &[], SPAN_RADIUS, scale, origin, &mut under);
    let mut strikes = Vec::new();
    ts.strike_decorations(&layout, &spans, &[(0, 4, Some(STRIKE_C))], Color::BLACK, scale, origin, &mut strikes);
    let mut overs = Vec::new();
    ts.overline_decorations(
        &layout,
        &spans,
        &[
            (0, 4, Some(OVER_C)),
            (11, src.len() as u32, None),
            (5, 5, Some(OVER_C)), // empty ignored
            (8, 6, Some(OVER_C)), // inverted ignored
        ],
        Color::BLACK,
        scale,
        origin,
        &mut overs,
    );

    assert!(!under.is_empty());
    assert_eq!(strikes.len(), 1);
    assert!(overs.len() >= 3, "expected 1 segment for OVER + >=2 wrapped segments, got {}", overs.len());
    let over0 = &overs[0];
    let strike0 = &strikes[0];
    let under0 = &under[0];
    assert_eq!(over0.color, OVER_C);
    assert_eq!(over0.radius, 0.0);
    assert_eq!(over0.corners, None);
    assert!(
        over0.rect[1] < strike0.rect[1] && strike0.rect[1] < under0.rect[1],
        "expected overline ({}) < strike ({}) < underline ({})",
        over0.rect[1],
        strike0.rect[1],
        under0.rect[1],
    );
    assert!(over0.rect[3] >= 1.0, "overline thickness must be >= 1 px");
    // Uncolored second range falls back to default_color (Color::BLACK).
    assert_eq!(overs[1].color, Color::BLACK);

    // Mixed font size: larger run puts its overline higher (smaller y) than the smaller run.
    let mixed = "BIG small";
    let m_spans = vec![Span::new(0, 3).size(28.0).color(OVER_C)];
    let m_layout = ts.layout_rich(mixed, &m_spans, style, scale, None);
    let mut m_overs = Vec::new();
    ts.overline_decorations(&m_layout, &m_spans, &[(0, 3, None), (4, 9, None)], Color::BLACK, scale, origin, &mut m_overs);
    assert_eq!(m_overs.len(), 2);
    assert_eq!(m_overs[0].color, OVER_C, "None color inherits covering span color");
    assert_eq!(m_overs[1].color, Color::BLACK, "uncovered range inherits default color");
    assert!(
        m_overs[0].rect[1] < m_overs[1].rect[1],
        "28pt run overline y={} must sit higher than 14pt run y={}",
        m_overs[0].rect[1],
        m_overs[1].rect[1],
    );
}
