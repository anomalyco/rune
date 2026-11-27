use swash::zeno::Cap;

use super::anim::{self, AnimState, CompileOutcome};
use super::dash::{self, DashArray, DashOutcome, StrokeDashSpec};
use super::{Facing, Icon, icon_coverage};
use crate::{IconKind, TextSystem};

fn ink_sum(cov: &[u8]) -> u64 {
    cov.iter().map(|&b| b as u64).sum()
}

fn region_ink(cov: &[u8], px: u32, xs: std::ops::Range<u32>, ys: std::ops::Range<u32>) -> u64 {
    let mut sum = 0u64;
    for y in ys {
        for x in xs.clone() {
            sum += cov[(y * px + x) as usize] as u64;
        }
    }
    sum
}

fn compile_anim(body: &str, view: f32) -> anim::parse::CompiledAnim {
    match anim::compile(body, view) {
        CompileOutcome::Animated(plan) => plan,
        CompileOutcome::StaticOnly => panic!("expected Animated, got StaticOnly"),
        CompileOutcome::Rejected => panic!("expected Animated, got Rejected"),
    }
}

#[test]
fn parse_dash_array_rules_and_rejections() {
    let diag = dash::view_box_diagonal([0.0, 0.0, 100.0, 100.0]);
    assert!((diag - 100.0).abs() < 1e-4);

    // Odd-length lists repeat once to become even.
    let one = dash::parse_dash_array("1", diag, false).unwrap();
    assert_eq!(one.as_slice(), &[1.0, 1.0]);

    let three = dash::parse_dash_array("2, 4 6px", diag, false).unwrap();
    assert_eq!(three.as_slice(), &[2.0, 4.0, 6.0, 2.0, 4.0, 6.0]);

    // Percentages resolve against normalized viewport diagonal.
    let pct = dash::parse_dash_array("25% 10%", diag, false).unwrap();
    assert!((pct.as_slice()[0] - 25.0).abs() < 1e-3);
    assert!((pct.as_slice()[1] - 10.0).abs() < 1e-3);

    // "none" is valid for absolute dasharray, rejected for deltas.
    let none = dash::parse_dash_array("none", diag, false).unwrap();
    assert!(none.is_none());
    assert!(dash::parse_dash_array("none", diag, true).is_none());

    // Negative values rejected for absolute, allowed for SMIL deltas.
    assert!(dash::parse_dash_array("-1 2", diag, false).is_none());
    let delta = dash::parse_dash_array("-1 2", diag, true).unwrap();
    assert_eq!(delta.as_slice(), &[-1.0, 2.0]);

    // Malformed punctuation, empty, non-finite, and over-cap lists are rejected.
    for bad in ["", "   ", ",1 2", "1 2,", "1,,2", "1, ,2", "NaN 1", "inf 1", "1 abc"] {
        assert!(dash::parse_dash_array(bad, diag, false).is_none(), "should reject {bad:?}");
    }
    let over_even = vec!["1"; 34].join(" ");
    assert!(dash::parse_dash_array(&over_even, diag, false).is_none());
    let over_odd = vec!["1"; 17].join(" ");
    assert!(dash::parse_dash_array(&over_odd, diag, false).is_none());
}

#[test]
fn dash_array_lcm_interpolation_and_none_fallback() {
    let a = dash::parse_dash_array("4 2", 100.0, false).unwrap();
    let b = dash::parse_dash_array("0 2 8 6", 100.0, false).unwrap();
    let mid = dash::lerp_dash_array(a, b, 0.5, false);
    assert_eq!(mid.as_slice(), &[2.0, 2.0, 6.0, 4.0]);

    // Odd "1" -> [1, 1] interpolates smoothly with "0 1" -> [0, 1].
    let odd = dash::parse_dash_array("1", 100.0, false).unwrap();
    let pair = dash::parse_dash_array("0 1", 100.0, false).unwrap();
    assert_eq!(dash::lerp_dash_array(odd, pair, 0.5, false).as_slice(), &[0.5, 1.0]);

    // "none" falls back to discrete midpoint step.
    assert!(dash::lerp_dash_array(DashArray::NONE, pair, 0.25, false).is_none());
    assert_eq!(dash::lerp_dash_array(DashArray::NONE, pair, 0.75, false).as_slice(), &[0.0, 1.0]);

    // Negative / non-finite pathLength rejected, 0 and positive accepted.
    assert_eq!(dash::parse_path_length("0"), Some(0.0));
    assert_eq!(dash::parse_path_length("1"), Some(1.0));
    assert_eq!(dash::parse_path_length("-1"), None);
    assert_eq!(dash::parse_path_length("NaN"), None);
}

/// Exact issue #2483 pencil-draw scenario: `pathLength="1"`, `stroke-dasharray="1"`,
/// and `<animate attributeName="stroke-dashoffset" values="0;1;0"/>`.
#[test]
fn smil_stroke_dashoffset_pencil_draw_with_path_length_1() {
    static PENCIL: Icon = Icon {
        name: "pencil-draw",
        view: 24.0,
        body: r#"<path d="M4 12 L20 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" pathLength="1" stroke-dasharray="1" stroke-dashoffset="0"><animate attributeName="stroke-dashoffset" values="0;1;0" dur="1s" repeatCount="indefinite"/></path>"#,
    };

    let mut text = TextSystem::default();
    assert_eq!(text.icon_kind(&PENCIL), IconKind::AnimatedMono);
    assert!(text.animated_icon_quad(&PENCIL, (48, 48.0), [0.0, 0.0], Facing::Upright, 0.0, 0.0, 1).is_some());

    let plan = compile_anim(PENCIL.body, PENCIL.view);
    let px = 48u32;

    let (cov0, _) = plan.render_test(px, 0.0);
    let (cov25, _) = plan.render_test(px, 0.25);
    let (cov50, _) = plan.render_test(px, 0.5);
    let (cov75, _) = plan.render_test(px, 0.75);
    let (cov100, _) = plan.render_test(px, 1.0);

    // Resting coverage (reduced motion) matches t = 0 exactly.
    assert_eq!(icon_coverage(&PENCIL, px), cov0);

    // t = 0: full stroke from x=4..20 (device x=8..40), so both left and right halves have ink.
    assert!(region_ink(&cov0, px, 8..22, 18..30) > 1000, "t=0 left half should have ink");
    assert!(region_ink(&cov0, px, 26..40, 18..30) > 1000, "t=0 right half should have ink");

    // t = 0.25 (offset = 0.5): left half drawn, right half completely clear past round cap.
    assert!(region_ink(&cov25, px, 8..22, 18..30) > 1000, "t=0.25 left half should have ink");
    assert_eq!(region_ink(&cov25, px, 28..44, 0..px), 0, "t=0.25 right half must be blank");
    assert!(ink_sum(&cov25) < ink_sum(&cov0));

    // t = 0.5 (offset = 1.0): fully hidden — zero ink pixels even with stroke-linecap="round".
    assert_eq!(ink_sum(&cov50), 0, "t=0.5 (offset=1) must draw zero pixels");

    // Symmetric redraw back to full stroke.
    assert_eq!(cov75, cov25);
    assert_eq!(cov100, cov0);
}

#[test]
fn smil_stroke_dashoffset_draw_in_and_erase_out_negative_offset() {
    let body = r#"<path d="M4 12 L20 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="butt" pathLength="1" stroke-dasharray="1" stroke-dashoffset="1"><animate attributeName="stroke-dashoffset" values="1;0;-1" dur="1s" fill="freeze"/></path>"#;
    let plan = compile_anim(body, 24.0);
    let px = 48u32;

    let (c0, _) = plan.render_test(px, 0.0);
    let (c25, _) = plan.render_test(px, 0.25);
    let (c50, _) = plan.render_test(px, 0.5);
    let (c75, _) = plan.render_test(px, 0.75);
    let (c100, st100) = plan.render_test(px, 1.0);

    assert_eq!(ink_sum(&c0), 0, "offset=1 starts blank");
    assert!(region_ink(&c25, px, 8..24, 0..px) > 1000 && region_ink(&c25, px, 25..44, 0..px) == 0, "offset=0.5 draws left half");
    assert!(region_ink(&c50, px, 8..24, 0..px) > 1000 && region_ink(&c50, px, 24..40, 0..px) > 1000, "offset=0 draws full line");
    assert!(region_ink(&c75, px, 4..23, 0..px) == 0 && region_ink(&c75, px, 24..40, 0..px) > 1000, "offset=-0.5 draws right half");
    assert_eq!(ink_sum(&c100), 0, "offset=-1 finishes blank");
    assert_eq!(st100, AnimState::Finished);
}

#[test]
fn smil_stroke_dasharray_animation_and_modes() {
    // Direct stroke-dasharray draw from "0 1" to "1 0" with pathLength="1".
    let body = r#"<path d="M4 12 L20 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="butt" pathLength="1"><animate attributeName="stroke-dasharray" values="0 1; 1 0" dur="1s" fill="freeze"/></path>"#;
    let plan = compile_anim(body, 24.0);
    let px = 48u32;

    let (c0, _) = plan.render_test(px, 0.0);
    let (c50, _) = plan.render_test(px, 0.5);
    let (c100, st100) = plan.render_test(px, 1.0);

    assert_eq!(ink_sum(&c0), 0, "0 1 butt cap should be blank");
    assert!(region_ink(&c50, px, 8..24, 0..px) > 1000 && region_ink(&c50, px, 25..44, 0..px) == 0, "0.5 0.5 draws left half");
    assert!(region_ink(&c100, px, 8..24, 0..px) > 1000 && region_ink(&c100, px, 24..40, 0..px) > 1000, "1 0 draws full line");
    assert_eq!(st100, AnimState::Finished);

    // Odd-to-even LCM interpolation: from="1" ([1,1]) to="0 1" ([0,1]).
    let odd_to_even = r#"<line x1="4" y1="12" x2="20" y2="12" stroke="currentColor" stroke-width="2" pathLength="1"><animate attributeName="stroke-dasharray" from="1" to="0 1" dur="1s" fill="freeze"/></line>"#;
    let plan2 = compile_anim(odd_to_even, 24.0);
    assert!(ink_sum(&plan2.render_test(px, 0.0).0) > 2000);
    assert!(ink_sum(&plan2.render_test(px, 0.5).0) > 800);
    assert!(ink_sum(&plan2.render_test(px, 0.5).0) < ink_sum(&plan2.render_test(px, 0.0).0));
    assert_eq!(ink_sum(&plan2.render_test(px, 1.0).0), 0);

    // <set attributeName="stroke-dasharray" to="none"> clears inherited dash at begin=0.5s.
    let set_none = r#"<g stroke-dasharray="0 1"><line x1="4" y1="12" x2="20" y2="12" stroke="currentColor" stroke-width="2" pathLength="1"><set attributeName="stroke-dasharray" to="none" begin="0.5s"/></line></g>"#;
    let plan3 = compile_anim(set_none, 24.0);
    assert_eq!(ink_sum(&plan3.render_test(px, 0.25).0), 0, "before <set> inherits 0 1");
    assert!(ink_sum(&plan3.render_test(px, 0.75).0) > 2000, "after <set to='none'> draws solid");

    // by-only additive animation on stroke-dasharray and stroke-dashoffset.
    let by_body = r#"<line x1="4" y1="12" x2="20" y2="12" stroke="currentColor" stroke-width="2" pathLength="1" stroke-dasharray="0 1" stroke-dashoffset="0"><animate attributeName="stroke-dasharray" by="1 -1" dur="1s" fill="freeze"/></line>"#;
    let plan4 = compile_anim(by_body, 24.0);
    assert_eq!(ink_sum(&plan4.render_test(px, 0.0).0), 0);
    assert!(ink_sum(&plan4.render_test(px, 1.0).0) > 2000);

    // calcMode="paced" and calcMode="spline" compile and progress monotonically.
    let paced = r#"<line x1="4" y1="12" x2="20" y2="12" stroke="currentColor" stroke-width="2" pathLength="1" stroke-dasharray="1"><animate attributeName="stroke-dashoffset" values="1;0.9;0" calcMode="paced" dur="1s" fill="freeze"/></line>"#;
    let plan_paced = compile_anim(paced, 24.0);
    let p50 = ink_sum(&plan_paced.render_test(px, 0.5).0);
    let p100 = ink_sum(&plan_paced.render_test(px, 1.0).0);
    assert!(p50 > p100 / 3 && p50 < (p100 * 2) / 3, "paced midpoint should be ~50% drawn (got {p50} of {p100})");
}

#[test]
fn closed_shapes_and_curved_paths_dash_progressively_with_path_length() {
    let px = 48u32;

    // Circle: SVG 2 starts at 3 o'clock (right) and progresses clockwise (bottom-right first).
    let circle = r#"<circle cx="12" cy="12" r="8" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" pathLength="1" stroke-dasharray="1"><animate attributeName="stroke-dashoffset" from="1" to="0" dur="1s" fill="freeze"/></circle>"#;
    let plan_c = compile_anim(circle, 24.0);
    assert_eq!(ink_sum(&plan_c.render_test(px, 0.0).0), 0, "circle t=0 must be empty");
    let (c25, _) = plan_c.render_test(px, 0.25);
    assert!(region_ink(&c25, px, 24..48, 24..48) > 500, "circle 25% should draw bottom-right quadrant");
    assert_eq!(region_ink(&c25, px, 0..20, 0..20), 0, "circle 25% top-left quadrant must be empty");
    let (c100, _) = plan_c.render_test(px, 1.0);
    assert!(region_ink(&c100, px, 0..24, 0..24) > 500, "circle 100% covers all quadrants");

    // Rounded rect: starts top-left and draws clockwise.
    let rect = r#"<rect x="4" y="4" width="16" height="16" rx="3" fill="none" stroke="currentColor" stroke-width="2" pathLength="1" stroke-dasharray="1"><animate attributeName="stroke-dashoffset" from="1" to="0" dur="1s" fill="freeze"/></rect>"#;
    let plan_r = compile_anim(rect, 24.0);
    assert_eq!(ink_sum(&plan_r.render_test(px, 0.0).0), 0, "rect t=0 must be empty");
    let (r25, _) = plan_r.render_test(px, 0.25);
    assert!(region_ink(&r25, px, 8..40, 0..16) > 500, "rect 25% draws top edge");
    assert_eq!(region_ink(&r25, px, 0..48, 32..48), 0, "rect 25% bottom edge must be empty");
    assert!(ink_sum(&plan_r.render_test(px, 1.0).0) > ink_sum(&r25) * 2);

    // SVG arc (conic) path: quad-flattened for dashing so arc length & shape stay accurate.
    let arc = r#"<path d="M4 16 A8 8 0 0 1 20 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" pathLength="1" stroke-dasharray="1"><animate attributeName="stroke-dashoffset" from="1" to="0" dur="1s" fill="freeze"/></path>"#;
    let plan_a = compile_anim(arc, 24.0);
    assert_eq!(ink_sum(&plan_a.render_test(px, 0.0).0), 0, "arc t=0 must be empty");
    let (a50, _) = plan_a.render_test(px, 0.5);
    let (a100, _) = plan_a.render_test(px, 1.0);
    assert!(region_ink(&a50, px, 4..24, 0..px) > 500, "arc 50% draws left half");
    assert_eq!(region_ink(&a50, px, 29..44, 0..px), 0, "arc 50% right half blank");
    assert!(region_ink(&a100, px, 29..44, 0..px) > 500, "arc 100% draws right half too");

    // Cubic Bezier curve.
    let cubic = r#"<path d="M4 18 C4 4 20 4 20 18" fill="none" stroke="currentColor" stroke-width="2" pathLength="1" stroke-dasharray="1"><animate attributeName="stroke-dashoffset" from="1" to="0" dur="1s" fill="freeze"/></path>"#;
    let plan_b = compile_anim(cubic, 24.0);
    assert_eq!(ink_sum(&plan_b.render_test(px, 0.0).0), 0);
    let b50 = ink_sum(&plan_b.render_test(px, 0.5).0);
    let b100 = ink_sum(&plan_b.render_test(px, 1.0).0);
    assert!(b50 > 500 && b100 > b50 + 500);
}

#[test]
fn group_inheritance_and_child_none_override() {
    // Animating stroke-dashoffset on <g> drives child paths, while a child with
    // stroke-dasharray="none" stays solid.
    let body = r#"<g stroke="currentColor" stroke-width="2" stroke-dasharray="1"><animate attributeName="stroke-dashoffset" from="1" to="0" dur="1s" fill="freeze"/><path d="M4 6 L20 6" fill="none" pathLength="1"/><path d="M4 18 L20 18" fill="none" stroke-dasharray="none"/></g>"#;
    let plan = compile_anim(body, 24.0);
    let px = 48u32;

    let (c0, _) = plan.render_test(px, 0.0);
    let (c100, _) = plan.render_test(px, 1.0);

    // Top path (y=6 -> device y=12) inherits dash and is hidden at t=0, visible at t=1.
    assert_eq!(region_ink(&c0, px, 0..px, 6..18), 0);
    assert!(region_ink(&c100, px, 0..px, 6..18) > 1000);
    // Bottom path (y=18 -> device y=36) overrides stroke-dasharray="none" and is solid at both t=0 and t=1.
    assert!(region_ink(&c0, px, 0..px, 30..42) > 1000);
    assert_eq!(region_ink(&c0, px, 0..px, 30..42), region_ink(&c100, px, 0..px, 30..42));
}

#[test]
fn static_icon_dashing_and_undashed_unchanged() {
    static DASHED_HALF: Icon = Icon {
        name: "static-dashed-half",
        view: 24.0,
        body: r#"<path d="M4 12 L20 12" fill="none" stroke="currentColor" stroke-width="2" pathLength="1" stroke-dasharray="0.5 0.5" stroke-dashoffset="0"/>"#,
    };
    static DASHED_HIDDEN: Icon = Icon {
        name: "static-dashed-hidden",
        view: 24.0,
        body: r#"<path d="M4 12 L20 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" pathLength="1" stroke-dasharray="1" stroke-dashoffset="1"/>"#,
    };
    static SOLID: Icon = Icon {
        name: "static-solid",
        view: 24.0,
        body: r#"<path d="M4 12 L20 12" fill="none" stroke="currentColor" stroke-width="2"/>"#,
    };
    static SOLID_NONE_DASH: Icon = Icon {
        name: "static-solid-none-dash",
        view: 24.0,
        body: r#"<g stroke-dasharray="1"><path d="M4 12 L20 12" fill="none" stroke="currentColor" stroke-width="2" stroke-dasharray="none"/></g>"#,
    };

    let px = 48u32;
    let half = icon_coverage(&DASHED_HALF, px);
    assert!(region_ink(&half, px, 8..24, 0..px) > 1000);
    assert_eq!(region_ink(&half, px, 25..44, 0..px), 0);

    assert_eq!(ink_sum(&icon_coverage(&DASHED_HIDDEN, px)), 0);

    // Explicit stroke-dasharray="none" matches an undashed static icon bit-for-bit.
    assert_eq!(icon_coverage(&SOLID, px), icon_coverage(&SOLID_NONE_DASH, px));
}

#[test]
fn hostile_and_edge_case_dash_inputs_are_bounded_and_safe() {
    let px = 32u32;

    // Microscopic dash interval would create 100k+ segments without MAX_DASH_SEGMENTS cap:
    // must finish cleanly (falling back to solid stroke) without stalling or panicking.
    let mut pb = tiny_skia_path::PathBuilder::new();
    pb.move_to(0.0, 12.0);
    pb.line_to(24.0, 12.0);
    let path = pb.finish().unwrap();
    let tiny = StrokeDashSpec {
        array: dash::parse_dash_array("0.0001 0.0001", 24.0, false).unwrap(),
        offset: 0.0,
        path_length: None,
    };
    assert!(matches!(dash::apply_dash(&path, &tiny, Cap::Butt, 2.0), DashOutcome::Solid));

    // All-zero dasharray draws solid per SVG 2.
    let zeros = StrokeDashSpec {
        array: dash::parse_dash_array("0 0", 24.0, false).unwrap(),
        offset: 0.0,
        path_length: None,
    };
    assert!(matches!(dash::apply_dash(&path, &zeros, Cap::Butt, 2.0), DashOutcome::Solid));

    // pathLength="0" with leading positive dash draws solid; leading 0 with Butt draws empty.
    let pl0_solid = StrokeDashSpec {
        array: dash::parse_dash_array("1 1", 24.0, false).unwrap(),
        offset: 0.0,
        path_length: Some(0.0),
    };
    assert!(matches!(dash::apply_dash(&path, &pl0_solid, Cap::Butt, 2.0), DashOutcome::Solid));
    let pl0_empty = StrokeDashSpec {
        array: dash::parse_dash_array("0 1", 24.0, false).unwrap(),
        offset: 0.0,
        path_length: Some(0.0),
    };
    assert!(matches!(dash::apply_dash(&path, &pl0_empty, Cap::Butt, 2.0), DashOutcome::Empty));

    // Negative pathLength is ignored; percentage stroke-dashoffset animates cleanly.
    let pct_offset = r#"<line x1="2" y1="12" x2="22" y2="12" stroke="currentColor" stroke-width="2" pathLength="-5" stroke-dasharray="10 10"><animate attributeName="stroke-dashoffset" values="0%;50%" dur="1s" fill="freeze"/></line>"#;
    let plan = compile_anim(pct_offset, 24.0);
    let (a, _) = plan.render_test(px, 0.0);
    let (b, _) = plan.render_test(px, 1.0);
    assert_ne!(a, b, "percentage stroke-dashoffset must shift the dash pattern");
}
