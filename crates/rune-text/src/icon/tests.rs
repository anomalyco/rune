use super::*;
use crate::{IconKind, TextSystem};

fn centre(body: &'static str) -> u8 {
    let icon = Box::leak(Box::new(Icon { name: "t", view: 16.0, body }));
    icon_coverage(icon, 32)[16 * 32 + 16]
}

/// Weighted vertical and horizontal centroid of coverage in a `px × px` mask.
fn centroid(cov: &[u8], px: u32) -> (f32, f32) {
    let (mut sx, mut sy, mut sum) = (0.0f64, 0.0f64, 0.0f64);
    for y in 0..px {
        for x in 0..px {
            let a = cov[(y * px + x) as usize] as f64;
            sx += (x as f64 + 0.5) * a;
            sy += (y as f64 + 0.5) * a;
            sum += a;
        }
    }
    assert!(sum > 0.0, "expected non-empty coverage");
    ((sx / sum) as f32, (sy / sum) as f32)
}

#[test]
fn opacity_scales_an_elements_coverage() {
    assert_eq!(centre(r#"<circle cx="8" cy="8" r="6" fill="currentColor"/>"#), 255);
    let half = centre(r#"<circle cx="8" cy="8" r="6" fill="currentColor" opacity=".5"/>"#);
    assert!((126..=129).contains(&half), "opacity .5 is half coverage, got {half}");
    let fill = centre(r#"<circle cx="8" cy="8" r="6" fill="currentColor" fill-opacity="0.25"/>"#);
    assert!((62..=65).contains(&fill), "fill-opacity, got {fill}");
    let both = centre(r#"<circle cx="8" cy="8" r="6" fill="currentColor" opacity=".5" fill-opacity=".5"/>"#);
    assert!((62..=65).contains(&both), "they multiply, got {both}");
}

#[test]
fn stroke_opacity_leaves_the_fill_alone() {
    let body = r#"<rect x="0" y="0" width="16" height="16" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-opacity=".3"/>"#;
    assert_eq!(centre(body), 255);
}

/// Issue #2551 repro: a wide 737×665 viewBox fits the width and centres vertically, matching
/// Chromium's default `preserveAspectRatio="xMidYMid meet"` (and the manual `translate(0 36)`
/// workaround on a square 737 box).
#[test]
fn wide_737x665_viewbox_centres_vertically_like_svg_xmidymid_meet() {
    static MARK: Icon = Icon::svg(
        "mark",
        r#"<svg viewBox="0 0 737 665"><rect x="0" y="0" width="737" height="665" fill="currentColor"/></svg>"#,
    );
    static LITERAL_MARK: Icon = Icon {
        name: "mark-lit",
        view: 737.0,
        body: r#"<svg viewBox="0 0 737 665"><rect x="0" y="0" width="737" height="665" fill="currentColor"/></svg>"#,
    };
    static MANUAL_WORKAROUND: Icon = Icon {
        name: "mark-manual",
        view: 737.0,
        body: r#"<g transform="translate(0 36)"><rect x="0" y="0" width="737" height="665" fill="currentColor"/></g>"#,
    };
    assert_eq!(MARK.view_box(), Some([0.0, 0.0, 737.0, 665.0]));
    for px in [16u32, 32] {
        let cov = icon_coverage(&MARK, px);
        let lit = icon_coverage(&LITERAL_MARK, px);
        let manual = icon_coverage(&MANUAL_WORKAROUND, px);
        assert_eq!(cov, lit, "Icon::svg and struct literal must match at {px}px");
        assert_eq!(cov, manual, "non-square viewBox must match exact xMidYMid meet offset at {px}px");
        let (cx, cy) = centroid(&cov, px);
        let mid = px as f32 * 0.5;
        assert!((cx - mid).abs() < 0.01, "x centroid {cx} != {mid} at {px}px");
        assert!((cy - mid).abs() < 0.01, "y centroid {cy} != {mid} at {px}px");
    }
}

#[test]
fn wide_and_tall_viewboxes_letterbox_symmetrically() {
    static WIDE: Icon = Icon::svg(
        "wide",
        r#"<svg viewBox="0 0 200 100"><rect x="0" y="0" width="200" height="100" fill="currentColor"/></svg>"#,
    );
    let wcov = icon_coverage(&WIDE, 32);
    for y in 0..32u32 {
        let expected = if (8..24).contains(&y) { 255 } else { 0 };
        for x in 0..32u32 {
            assert_eq!(wcov[(y * 32 + x) as usize], expected, "wide ({x},{y})");
        }
    }

    static TALL: Icon = Icon::svg(
        "tall",
        r#"<svg viewBox="0 0 100 200"><rect x="0" y="0" width="100" height="200" fill="currentColor"/></svg>"#,
    );
    let tcov = icon_coverage(&TALL, 32);
    for y in 0..32u32 {
        for x in 0..32u32 {
            let expected = if (8..24).contains(&x) { 255 } else { 0 };
            assert_eq!(tcov[(y * 32 + x) as usize], expected, "tall ({x},{y})");
        }
    }
}

#[test]
fn non_zero_origin_viewbox_translates_and_centres() {
    static SHIFTED_WIDE: Icon = Icon::svg(
        "shifted-wide",
        r#"<svg viewBox="10 -20 40 20"><rect x="10" y="-20" width="40" height="20" fill="currentColor"/></svg>"#,
    );
    assert_eq!(SHIFTED_WIDE.view_box(), Some([10.0, -20.0, 40.0, 20.0]));
    let cov = icon_coverage(&SHIFTED_WIDE, 32);
    for y in 0..32u32 {
        let expected = if (8..24).contains(&y) { 255 } else { 0 };
        for x in 0..32u32 {
            assert_eq!(cov[(y * 32 + x) as usize], expected, "shifted wide ({x},{y})");
        }
    }

    static SHIFTED_TALL: Icon = Icon::svg(
        "shifted-tall",
        r#"<svg viewBox="-50 100 50 100"><rect x="-50" y="100" width="50" height="100" fill="currentColor"/></svg>"#,
    );
    let tcov = icon_coverage(&SHIFTED_TALL, 32);
    for y in 0..32u32 {
        for x in 0..32u32 {
            let expected = if (8..24).contains(&x) { 255 } else { 0 };
            assert_eq!(tcov[(y * 32 + x) as usize], expected, "shifted tall ({x},{y})");
        }
    }
}

#[test]
fn square_icon_regression_is_bit_identical() {
    static FRAGMENT: Icon = Icon {
        name: "sq-frag",
        view: 24.0,
        body: r#"<path d="M4 6h16M4 12h16M4 18h10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
    };
    static WRAPPED: Icon = Icon::svg(
        "sq-svg",
        r#"<svg viewBox="0 0 24 24"><path d="M4 6h16M4 12h16M4 18h10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>"#,
    );
    static NO_VB: Icon = Icon {
        name: "sq-no-vb",
        view: 24.0,
        body: r#"<svg fill="none"><path d="M4 6h16M4 12h16M4 18h10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>"#,
    };
    assert_eq!(FRAGMENT.view_box(), Some([0.0, 0.0, 24.0, 24.0]));
    assert_eq!(WRAPPED.view_box(), Some([0.0, 0.0, 24.0, 24.0]));
    assert_eq!(NO_VB.view_box(), Some([0.0, 0.0, 24.0, 24.0]));
    for px in [16u32, 20, 32] {
        let a = icon_coverage(&FRAGMENT, px);
        assert_eq!(a, icon_coverage(&WRAPPED, px), "wrapped square differs at {px}px");
        assert_eq!(a, icon_coverage(&NO_VB, px), "svg without viewBox differs at {px}px");
    }
}

#[test]
fn viewbox_syntax_variants_and_rejection() {
    let variants = [
        r#"<svg viewBox="0,0,200,100"><rect width="200" height="100" fill="currentColor"/></svg>"#,
        r#"<svg viewBox="0, 0, 200, 100"><rect width="200" height="100" fill="currentColor"/></svg>"#,
        "<svg viewBox = '  0\t0\n200 , 100  '><rect width='200' height='100' fill='currentColor'/></svg>",
        r#"<?xml version="1.0"?><!-- comment --><svg viewBox="0 0 200 100"><rect width="200" height="100" fill="currentColor"/></svg><!-- tail -->"#,
        r#"<svg viewBox="0 0 200 100"><svg viewBox="0 0 10 10"><rect width="200" height="100" fill="currentColor"/></svg></svg>"#,
    ];
    let expected = icon_coverage(
        &Icon::svg("ref", r#"<svg viewBox="0 0 200 100"><rect width="200" height="100" fill="currentColor"/></svg>"#),
        32,
    );
    for body in variants {
        let icon = Icon::svg("v", body);
        assert_eq!(icon.view_box(), Some([0.0, 0.0, 200.0, 100.0]), "failed for {body}");
        assert_eq!(icon_coverage(&icon, 32), expected, "coverage mismatch for {body}");
    }

    // Sibling element after </svg>: not a single root <svg>, so falls back to `icon.view`.
    let siblings = Icon {
        name: "sib",
        view: 24.0,
        body: r#"<svg viewBox="0 0 200 100"><rect width="10" height="10"/></svg><rect width="24" height="24"/>"#,
    };
    assert_eq!(siblings.view_box(), Some([0.0, 0.0, 24.0, 24.0]));

    // Malformed, zero, negative, or over-cap viewBoxes disable rendering rather than falling back.
    for bad in [
        r#"<svg viewBox="0 0 0 16"><rect width="16" height="16"/></svg>"#,
        r#"<svg viewBox="0 0 16 0"><rect width="16" height="16"/></svg>"#,
        r#"<svg viewBox="0 0 -16 16"><rect width="16" height="16"/></svg>"#,
        r#"<svg viewBox="0 0 16"><rect width="16" height="16"/></svg>"#,
        r#"<svg viewBox="0 0 16 16 16"><rect width="16" height="16"/></svg>"#,
        r#"<svg viewBox="0 0 16 16,"><rect width="16" height="16"/></svg>"#,
        r#"<svg viewBox="0,,0 16 16"><rect width="16" height="16"/></svg>"#,
        r#"<svg viewBox="0 0 NaN 16"><rect width="16" height="16"/></svg>"#,
        r#"<svg viewBox="0 0 5000 16"><rect width="16" height="16"/></svg>"#,
    ] {
        let icon = Icon { name: "bad", view: 16.0, body: bad };
        assert_eq!(icon.view_box(), None, "expected None for {bad}");
        assert!(icon_coverage(&icon, 16).iter().all(|&c| c == 0), "expected blank for {bad}");
    }
}

#[test]
fn animated_mono_icon_honours_non_square_and_non_zero_origin_viewbox() {
    static ANIM_WIDE: Icon = Icon::svg(
        "anim-wide",
        r#"<svg viewBox="10 -20 40 20"><rect x="10" y="-20" width="40" height="20" fill="currentColor"><animate attributeName="opacity" values="1;0.5;1" dur="1s" repeatCount="indefinite"/></rect></svg>"#,
    );
    let mut text = TextSystem::default();
    assert_eq!(text.icon_kind(&ANIM_WIDE), IconKind::AnimatedMono);

    // Reduced-motion resting frame (`icon_coverage` -> `render_rest_coverage`) centres cleanly.
    let rest = icon_coverage(&ANIM_WIDE, 32);
    for y in 0..32u32 {
        let expected = if (8..24).contains(&y) { 255 } else { 0 };
        for x in 0..32u32 {
            assert_eq!(rest[(y * 32 + x) as usize], expected, "anim rest ({x},{y})");
        }
    }

    // Live animated atlas tile at t = 0 matches the resting coverage texel-for-texel.
    let anim_q = text
        .animated_icon_quad(&ANIM_WIDE, (32, 32.0), [0.0, 0.0], Facing::Upright, 0.0, 0.0, 1)
        .expect("animated quad");
    let static_q = text.icon_quad(&ANIM_WIDE, 32, [0.0, 0.0]).expect("static quad");
    assert_eq!(anim_q.quad.rect, static_q.rect);

    // Animated icon with invalid viewBox is rejected and draws nothing.
    static ANIM_BAD: Icon = Icon {
        name: "anim-bad",
        view: 16.0,
        body: r#"<svg viewBox="0 0 0 16"><rect width="16" height="16"><animate attributeName="opacity" values="1;0" dur="1s" repeatCount="indefinite"/></rect></svg>"#,
    };
    assert_eq!(text.icon_kind(&ANIM_BAD), IconKind::Mono);
    assert!(icon_coverage(&ANIM_BAD, 16).iter().all(|&c| c == 0));
}

#[test]
fn mirrored_non_square_icon_stays_centred() {
    // Asymmetric tall viewBox: left half filled, right half empty.
    static ASYM_TALL: Icon = Icon::svg(
        "asym-tall",
        r#"<svg viewBox="0 0 100 200"><rect x="0" y="0" width="50" height="200" fill="currentColor"/></svg>"#,
    );
    let mut text = TextSystem::default();
    assert!(text.icon_quad_facing(&ASYM_TALL, 32, [0.0, 0.0], Facing::Upright).is_some());
    assert!(text.icon_quad_facing(&ASYM_TALL, 32, [0.0, 0.0], Facing::Mirrored).is_some());

    // Upright occupies x in 8..16; mirrored reverses each 32px row, so it occupies x in 16..24.
    let mut mirrored = icon_coverage(&ASYM_TALL, 32);
    mirrored.as_chunks_mut::<32>().0.iter_mut().for_each(|row| row.reverse());
    for y in 0..32u32 {
        for x in 0..32u32 {
            let expected = if (16..24).contains(&x) { 255 } else { 0 };
            assert_eq!(mirrored[(y * 32 + x) as usize], expected, "mirrored ({x},{y})");
        }
    }
}

#[cfg(feature = "svg")]
#[test]
fn color_icon_honours_non_square_and_non_zero_origin_viewbox_in_square_canvas() {
    use rune_core::color::Color;

    // Include explicit non-square width/height on the outer <svg> to ensure color_icon still
    // produces a square 32×32 canvas centred with xMidYMid meet (never stretched!).
    static WIDE_COLOR: Icon = Icon::svg(
        "wide-color",
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="737" height="665" viewBox="10 -20 40 20"><rect x="10" y="-20" width="40" height="20" fill="#ff0000"/></svg>"##,
    );
    static TALL_COLOR: Icon = Icon::svg(
        "tall-color",
        r##"<svg width="100" height="200" viewBox="0 0 100 200"><rect x="0" y="0" width="100" height="200" fill="#00ff00"/></svg>"##,
    );
    static BAD_COLOR: Icon = Icon {
        name: "bad-color",
        view: 16.0,
        body: r##"<svg viewBox="0 0 0 16"><rect width="16" height="16" fill="#ff0000"/></svg>"##,
    };

    let mut text = TextSystem::default();
    assert_eq!(text.icon_kind(&WIDE_COLOR), IconKind::Color);

    for one in [false, true] {
        let pixels = text.color_icon(&WIDE_COLOR, 32, Color::WHITE, one).expect("wide color pixels");
        assert_eq!(pixels.size(), (32, 32), "color_icon must stay square (one={one})");
        let alpha: Vec<u8> = pixels
            .with_rgba(|w, h, b| {
                assert_eq!((w, h), (32, 32));
                b.as_chunks::<4>().0.iter().map(|px| px[3]).collect()
            })
            .expect("rgba frame");
        for y in 0..32u32 {
            let expected = if (8..24).contains(&y) { 255 } else { 0 };
            for x in 0..32u32 {
                assert_eq!(alpha[(y * 32 + x) as usize], expected, "wide color (one={one}) at ({x},{y})");
            }
        }
    }

    let tall = text.color_icon(&TALL_COLOR, 32, Color::WHITE, false).expect("tall color pixels");
    assert_eq!(tall.size(), (32, 32), "tall color_icon must stay square");
    let talpha: Vec<u8> = tall
        .with_rgba(|w, h, b| {
            assert_eq!((w, h), (32, 32));
            b.as_chunks::<4>().0.iter().map(|px| px[3]).collect()
        })
        .expect("rgba frame");
    for y in 0..32u32 {
        for x in 0..32u32 {
            let expected = if (8..24).contains(&x) { 255 } else { 0 };
            assert_eq!(talpha[(y * 32 + x) as usize], expected, "tall color at ({x},{y})");
        }
    }

    assert!(text.color_icon(&BAD_COLOR, 32, Color::WHITE, false).is_none());
}
