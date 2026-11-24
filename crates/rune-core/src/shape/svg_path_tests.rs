use super::*;
use crate::color::Color;
use crate::shape::{FillRule, LineCap, Op, Shape, triangulate_compound};

fn approx_pt(a: [f32; 2], b: [f32; 2], tol: f32) {
    assert!(
        (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol,
        "point mismatch: {a:?} vs {b:?} (tol {tol})"
    );
}

fn tri_area(verts: &[[f32; 2]], tris: &[([usize; 3], u32)]) -> f32 {
    tris.iter()
        .map(|([a, b, c], _)| {
            let (a, b, c) = (verts[*a], verts[*b], verts[*c]);
            ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5
        })
        .sum()
}

#[derive(Debug, PartialEq)]
enum Cmd {
    Move([f32; 2]),
    Line([f32; 2]),
    Quad([f32; 2], [f32; 2]),
    Cubic([f32; 2], [f32; 2], [f32; 2]),
    Arc([f32; 2], f32, f32, f32, bool, bool, [f32; 2]),
    Close,
}

#[derive(Default)]
struct Recorder(Vec<Cmd>);

impl SvgPathSink for Recorder {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn move_to(&mut self, p: [f32; 2]) {
        self.0.push(Cmd::Move(p));
    }
    fn line_to(&mut self, p: [f32; 2]) {
        self.0.push(Cmd::Line(p));
    }
    fn quad_to(&mut self, c: [f32; 2], p: [f32; 2]) {
        self.0.push(Cmd::Quad(c, p));
    }
    fn curve_to(&mut self, c1: [f32; 2], c2: [f32; 2], p: [f32; 2]) {
        self.0.push(Cmd::Cubic(c1, c2, p));
    }
    fn arc_to(
        &mut self,
        from: [f32; 2],
        rx: f32,
        ry: f32,
        angle: f32,
        large: bool,
        sweep: bool,
        to: [f32; 2],
    ) {
        self.0.push(Cmd::Arc(from, rx, ry, angle, large, sweep, to));
    }
    fn close(&mut self) {
        self.0.push(Cmd::Close);
    }
}

fn record(d: &str) -> (Option<()>, Vec<Cmd>) {
    let mut rec = Recorder::default();
    let res = parse_svg_path(d, &mut rec);
    (res, rec.0)
}

#[test]
fn move_line_hv_and_close_absolute_and_relative() {
    let p = Path::from_svg("M 10 20 L 30 20 H 50 V 60 l -20 10 h -20 v -50 Z");
    assert_eq!(p.subpaths.len(), 1);
    let sub = &p.subpaths[0];
    assert!(sub.closed);
    assert_eq!(
        sub.points,
        vec![
            [10.0, 20.0],
            [30.0, 20.0],
            [50.0, 20.0],
            [50.0, 60.0],
            [30.0, 70.0],
            [10.0, 70.0],
            [10.0, 20.0],
        ]
    );
}

#[test]
fn implicit_linetos_after_moveto_and_initial_relative_m() {
    // SVG 2 9.3.3: initial lowercase m is absolute, subsequent coordinate pairs are relative l.
    let p = Path::from_svg("m 10 20 5 -10 15 0");
    assert_eq!(p.subpaths.len(), 1);
    assert!(!p.subpaths[0].closed);
    assert_eq!(p.subpaths[0].points, vec![[10.0, 20.0], [15.0, 10.0], [30.0, 10.0]]);

    // Uppercase M makes subsequent pairs absolute L.
    let p2 = Path::from_svg("M 10 20 5 -10 15 0");
    assert_eq!(p2.subpaths[0].points, vec![[10.0, 20.0], [5.0, -10.0], [15.0, 0.0]]);
}

#[test]
fn closepath_resets_current_point_to_subpath_start() {
    // Drawing command immediately after Z starts a new subpath at the closed subpath's initial point.
    let p = Path::from_svg("M 10 10 L 40 10 L 40 40 Z l 5 15");
    assert_eq!(p.subpaths.len(), 2);
    assert!(p.subpaths[0].closed);
    assert_eq!(p.subpaths[0].points, vec![[10.0, 10.0], [40.0, 10.0], [40.0, 40.0]]);
    assert!(!p.subpaths[1].closed);
    assert_eq!(p.subpaths[1].points, vec![[10.0, 10.0], [15.0, 25.0]]);

    // Relative m after Z is also relative to subpath_start.
    let p2 = Path::from_svg("M 10 10 L 40 10 L 40 40 Z m 2 3 l 8 0");
    assert_eq!(p2.subpaths.len(), 2);
    assert_eq!(p2.subpaths[1].points, vec![[12.0, 13.0], [20.0, 13.0]]);

    // Repeated Z is harmless.
    let p3 = Path::from_svg("M 0 0 L 10 0 L 10 10 Z Z");
    assert_eq!(p3.subpaths.len(), 1);
    assert!(p3.subpaths[0].closed);
}

#[test]
fn smooth_cubic_and_quadratic_reflection_rules() {
    // S after C reflects C's second control point; S after L uses current point.
    let (res, cmds) = record("M 0 0 C 10 20 30 40 50 50 S 80 90 100 100 L 110 110 S 120 130 140 140");
    assert_eq!(res, Some(()));
    assert_eq!(
        cmds,
        vec![
            Cmd::Move([0.0, 0.0]),
            Cmd::Cubic([10.0, 20.0], [30.0, 40.0], [50.0, 50.0]),
            // Reflection of (30, 40) about (50, 50) = (70, 60)
            Cmd::Cubic([70.0, 60.0], [80.0, 90.0], [100.0, 100.0]),
            Cmd::Line([110.0, 110.0]),
            // After L (non-cubic), first control point equals current point (110, 110)
            Cmd::Cubic([110.0, 110.0], [120.0, 130.0], [140.0, 140.0]),
        ]
    );

    // T after Q reflects Q's control point; chained T reflects previous T; T after C uses current point.
    let (res, cmds) = record("M 0 0 Q 10 30 20 0 T 40 0 T 60 0 C 65 5 70 5 75 0 T 90 0");
    assert_eq!(res, Some(()));
    assert_eq!(
        cmds,
        vec![
            Cmd::Move([0.0, 0.0]),
            Cmd::Quad([10.0, 30.0], [20.0, 0.0]),
            // Reflection of (10, 30) about (20, 0) = (30, -30)
            Cmd::Quad([30.0, -30.0], [40.0, 0.0]),
            // Reflection of (30, -30) about (40, 0) = (50, 30)
            Cmd::Quad([50.0, 30.0], [60.0, 0.0]),
            Cmd::Cubic([65.0, 5.0], [70.0, 5.0], [75.0, 0.0]),
            // After C (non-quad), control point equals current point (75, 0)
            Cmd::Quad([75.0, 0.0], [90.0, 0.0]),
        ]
    );

    // S after Q does not reflect Q's control point (must be C or S).
    let (res, cmds) = record("M 0 0 Q 10 30 20 0 S 30 10 40 0");
    assert_eq!(res, Some(()));
    assert_eq!(cmds[2], Cmd::Cubic([20.0, 0.0], [30.0, 10.0], [40.0, 0.0]));

    // Flattened Path::from_svg matches manual Path builder for C/S/Q/T.
    let from_svg = Path::from_svg("M 0 0 C 0 50 100 50 100 0 s 100 -50 100 0 q 25 40 50 0 t 50 0");
    let manual = Path::new()
        .move_to([0.0, 0.0])
        .cubic_to([0.0, 50.0], [100.0, 50.0], [100.0, 0.0])
        .cubic_to([100.0, -50.0], [200.0, -50.0], [200.0, 0.0])
        .quad_to([225.0, 40.0], [250.0, 0.0])
        .quad_to([275.0, -40.0], [300.0, 0.0]);
    assert_eq!(from_svg, manual);
}

#[test]
fn elliptical_arcs_cover_all_four_flag_combinations_and_rotation() {
    // From (10, 0) to (0, 10) with rx=10, ry=10:
    let on_circle = |pts: &[[f32; 2]], cx: f32, cy: f32, r: f32| {
        for &[x, y] in pts {
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            assert!((d - r).abs() < 0.15, "point ({x}, {y}) at dist {d} from ({cx}, {cy}), expected {r}");
        }
    };
    let midpoint = |pts: &[[f32; 2]]| {
        let m = pts.len() / 2;
        if pts.len() % 2 == 1 {
            pts[m]
        } else {
            [(pts[m - 1][0] + pts[m][0]) * 0.5, (pts[m - 1][1] + pts[m][1]) * 0.5]
        }
    };

    // (large=0, sweep=1): center (0,0), short 90-deg CW arc, midpoint ~(7.07, 7.07)
    let p01 = Path::from_svg("M 10 0 A 10 10 0 0 1 0 10");
    let pts01 = &p01.subpaths[0].points;
    assert_eq!(*pts01.first().unwrap(), [10.0, 0.0]);
    assert_eq!(*pts01.last().unwrap(), [0.0, 10.0]);
    on_circle(pts01, 0.0, 0.0, 10.0);
    approx_pt(midpoint(pts01), [7.071, 7.071], 0.25);

    // (large=1, sweep=0): center (0,0), long 270-deg CCW arc, midpoint ~(-7.07, -7.07)
    let p10 = Path::from_svg("M 10 0 A 10 10 0 1 0 0 10");
    let pts10 = &p10.subpaths[0].points;
    assert_eq!(*pts10.last().unwrap(), [0.0, 10.0]);
    on_circle(pts10, 0.0, 0.0, 10.0);
    approx_pt(midpoint(pts10), [-7.071, -7.071], 0.25);

    // (large=0, sweep=0): center (10,10), short 90-deg CCW arc, midpoint ~(2.93, 2.93)
    let p00 = Path::from_svg("M 10 0 A 10 10 0 0 0 0 10");
    let pts00 = &p00.subpaths[0].points;
    on_circle(pts00, 10.0, 10.0, 10.0);
    approx_pt(midpoint(pts00), [2.929, 2.929], 0.25);

    // (large=1, sweep=1): center (10,10), long 270-deg CW arc, midpoint ~(17.07, 17.07)
    let p11 = Path::from_svg("M 10 0 A 10 10 0 1 1 0 10");
    let pts11 = &p11.subpaths[0].points;
    on_circle(pts11, 10.0, 10.0, 10.0);
    approx_pt(midpoint(pts11), [17.071, 17.071], 0.25);

    // Rotated ellipse (rx=40, ry=20, angle=45 deg) centered at (50, 50):
    // Half-ellipse from +x' end to -x' end.
    let c = std::f32::consts::FRAC_1_SQRT_2;
    let from = [50.0 + 40.0 * c, 50.0 + 40.0 * c];
    let to = [50.0 - 40.0 * c, 50.0 - 40.0 * c];
    let d = format!("M {} {} A 40 20 45 0 1 {} {}", from[0], from[1], to[0], to[1]);
    let rot = Path::from_svg(&d);
    assert!(rot.subpaths[0].points.len() > 8);
    for &[x, y] in &rot.subpaths[0].points {
        let dx = x - 50.0;
        let dy = y - 50.0;
        let xp = c * dx + c * dy;
        let yp = -c * dx + c * dy;
        let eq = (xp / 40.0).powi(2) + (yp / 20.0).powi(2);
        assert!((eq - 1.0).abs() < 0.03, "point ({x}, {y}) off rotated ellipse: eq={eq}");
    }
}

#[test]
fn arc_edge_cases_zero_negative_undersized_packed_flags_and_full_circle() {
    // Zero radius degenerates to a straight line (W3C SVG 2 B.2.5).
    let zero_rx = Path::from_svg("M 0 0 A 0 20 0 0 1 30 40");
    assert_eq!(zero_rx.subpaths[0].points, vec![[0.0, 0.0], [30.0, 40.0]]);
    let zero_ry = Path::from_svg("M 0 0 A 20 0 0 1 0 30 40");
    assert_eq!(zero_ry.subpaths[0].points, vec![[0.0, 0.0], [30.0, 40.0]]);

    // Negative radii use absolute values.
    let neg = Path::from_svg("M 10 0 A -10 -10 0 0 1 0 10");
    let pos = Path::from_svg("M 10 0 A 10 10 0 0 1 0 10");
    assert_eq!(neg, pos);

    // Undersized radii (rx=2, ry=2 for distance 20) scale up to a semicircle of radius 10.
    let small = Path::from_svg("M 0 0 A 2 2 0 0 1 20 0");
    let pts = &small.subpaths[0].points;
    let mid = pts[pts.len() / 2];
    approx_pt(mid, [10.0, -10.0], 0.3);

    // Coincident start/end emits no segments.
    let same = Path::from_svg("M 5 5 A 10 10 0 1 1 5 5");
    assert_eq!(same.subpaths[0].points, vec![[5.0, 5.0]]);

    // Compact packed arc flags without whitespace separators: a10 10 0 0110 10.
    let (res, cmds) = record("M0 0a10 10 0 0110 10");
    assert_eq!(res, Some(()));
    assert_eq!(cmds, vec![Cmd::Move([0.0, 0.0]), Cmd::Arc([0.0, 0.0], 10.0, 10.0, 0.0, false, true, [10.0, 10.0])]);

    // Full circle from two 180-deg arcs: triangulated area matches pi * r^2.
    let circle = Path::from_svg("M 90 50 A 40 40 0 1 0 10 50 A 40 40 0 1 0 90 50 Z");
    let contours: Vec<Vec<[f32; 2]>> = circle.subpaths.iter().map(|s| s.points.clone()).collect();
    let (verts, tris) = triangulate_compound(&contours, FillRule::NonZero);
    let expected = std::f32::consts::PI * 40.0 * 40.0;
    assert!((tri_area(&verts, &tris) - expected).abs() < 30.0, "got {}", tri_area(&verts, &tris));

    // Extreme radii never panic or produce NaN/infinite vertices.
    for d in [
        "M 0 0 A 1e30 1e30 0 0 1 10 0",
        "M 0 0 A 1e-30 1e-30 0 0 1 10 0",
        "M 0 0 A 1e-20 1e20 45 1 1 10 10",
    ] {
        let p = Path::from_svg(d);
        assert!(p.subpaths[0].points.iter().all(|pt| pt[0].is_finite() && pt[1].is_finite()), "{d}");
        assert_eq!(*p.subpaths[0].points.last().unwrap(), [10.0, if d.ends_with("10 10") { 10.0 } else { 0.0 }]);
    }
}

#[test]
fn compact_numbers_exponents_whitespace_and_overflow() {
    // Tightly packed signs, decimals, exponents, and ASCII whitespace including form feed.
    let p = Path::from_svg(" \t\n\r\x0cM10-20L.5.5,1.2.3L1e2-1.5E+1L+2.5e-1 000000000000000000000000000000000000000005");
    let pts = &p.subpaths[0].points;
    assert_eq!(pts.len(), 5);
    approx_pt(pts[0], [10.0, -20.0], 1e-5);
    approx_pt(pts[1], [0.5, 0.5], 1e-5);
    approx_pt(pts[2], [1.2, 0.3], 1e-5);
    approx_pt(pts[3], [100.0, -15.0], 1e-4);
    approx_pt(pts[4], [0.25, 5.0], 1e-5);

    // Zero with large exponent stays zero; extreme negative exponent underflows to zero.
    let p0 = Path::from_svg("M 0e500 1e-99999999999 L 5 5");
    assert_eq!(p0.subpaths[0].points, vec![[0.0, 0.0], [5.0, 5.0]]);

    // Overflowing exponent stops parsing (non-finite) without emitting the bad command.
    let (res, cmds) = record("M 0 0 L 10 10 L 1e500 0 L 20 20");
    assert_eq!(res, None);
    assert_eq!(cmds, vec![Cmd::Move([0.0, 0.0]), Cmd::Line([10.0, 10.0])]);

    let (res2, cmds2) = record("M 0 0 L 1e99999999999999999 0");
    assert_eq!(res2, None);
    assert_eq!(cmds2, vec![Cmd::Move([0.0, 0.0])]);
}

#[test]
fn blink_chrome_stop_on_first_syntax_error() {
    // Empty and whitespace-only strings produce an empty Path.
    assert_eq!(Path::from_svg(""), Path::new());
    assert_eq!(Path::from_svg("   \t\n  "), Path::new());

    // First command must be M or m.
    assert_eq!(Path::from_svg("L 10 10"), Path::new());
    assert_eq!(Path::from_svg("10 10"), Path::new());
    assert_eq!(Path::from_svg("Z"), Path::new());

    // Unknown command stops and keeps prior valid commands.
    let p = Path::from_svg("M 0 0 L 10 0 L 10 10 X 50 50 L 0 10");
    assert_eq!(p.subpaths[0].points, vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);

    // Truncated command stops before emitting the incomplete segment.
    let p2 = Path::from_svg("M 0 0 L 10 0 C 1 2 3 4 5");
    assert_eq!(p2.subpaths[0].points, vec![[0.0, 0.0], [10.0, 0.0]]);

    // Bare numbers after Z stop parsing.
    let p3 = Path::from_svg("M 0 0 L 10 0 L 10 10 Z 20 20");
    assert_eq!(p3.subpaths.len(), 1);
    assert!(p3.subpaths[0].closed);

    // Invalid arc flag ('2') stops before emitting the arc.
    let p4 = Path::from_svg("M 0 0 L 5 0 A 5 5 0 2 1 10 0");
    assert_eq!(p4.subpaths[0].points, vec![[0.0, 0.0], [5.0, 0.0]]);

    // Dangling 'e' without exponent digits stops cleanly after consuming the valid number.
    let (res, cmds) = record("M 0 0 L 10 10e L 20 20");
    assert_eq!(res, None);
    assert_eq!(cmds, vec![Cmd::Move([0.0, 0.0]), Cmd::Line([10.0, 10.0])]);
}

#[test]
fn shape_svg_builders_match_path_builders_and_support_fill_rules() {
    let ring_d = "M 0 0 H 100 V 100 H 0 Z M 25 25 H 75 V 75 H 25 Z";
    let ring_path = Path::from_svg(ring_d);

    let nz = Shape::new().fill_svg(ring_d, Color::WHITE);
    let nz_via_path = Shape::new().fill_path(&ring_path, Color::WHITE);
    assert_eq!(nz, nz_via_path);

    let eo = Shape::new().fill_svg_with(ring_d, FillRule::EvenOdd, Color::WHITE);
    let eo_via_path = Shape::new().fill_path_with(&ring_path, FillRule::EvenOdd, Color::WHITE);
    assert_eq!(eo, eo_via_path);
    match &eo.ops[0] {
        Op::FillCompound { contours, rule, .. } => {
            assert_eq!(*rule, FillRule::EvenOdd);
            let (verts, tris) = triangulate_compound(contours, *rule);
            assert!((tri_area(&verts, &tris) - 7_500.0).abs() < 1e-1);
        }
        other => panic!("expected FillCompound, got {other:?}"),
    }

    // Stroke SVG matches stroke_path and supports drawn() trimming.
    let line_d = "M 0 0 H 60 V 40";
    let stroked = Shape::new().stroke_svg(line_d, 2.0, LineCap::Butt, Color::WHITE);
    assert_eq!(
        stroked,
        Shape::new().stroke_path(&Path::from_svg(line_d), 2.0, LineCap::Butt, Color::WHITE)
    );
    let half = stroked.drawn(0.5);
    match &half.ops[0] {
        Op::Stroke { points, width, cap, closed, .. } => {
            assert!(!closed);
            assert_eq!(*width, 2.0);
            assert_eq!(*cap, LineCap::Butt);
            assert_eq!(*points, vec![[0.0, 0.0], [50.0, 0.0]]);
        }
        other => panic!("expected Stroke, got {other:?}"),
    }

    // Empty or degenerate SVG strings produce empty shapes without panicking.
    let empty = Shape::new()
        .fill_svg("", Color::WHITE)
        .fill_svg_with("M 0 0 L 10 10", FillRule::EvenOdd, Color::WHITE)
        .stroke_svg("not-a-path", 2.0, LineCap::Round, Color::WHITE)
        .stroke_svg("M 0 0 L 10 0", 0.0, LineCap::Round, Color::WHITE);
    assert!(empty.is_empty());
}

#[test]
fn real_world_icon_path_parses_completely_through_shared_sink() {
    // GitHub mark path combining M, C, c, s, A and tight negative/decimal separators.
    let octocat = "M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8";
    let (res, cmds) = record(octocat);
    assert_eq!(res, Some(()), "octocat path must parse cleanly to end");
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Arc(..))), "contains arc command");
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Cubic(..))), "contains cubic commands");

    let path = Path::from_svg(octocat);
    assert_eq!(path.subpaths.len(), 1);
    assert!(path.subpaths[0].points.len() > 20);
    assert!(path.subpaths[0].points.iter().all(|p| p[0].is_finite() && p[1].is_finite()));
    let shape = Shape::new().fill_svg(octocat, Color::WHITE);
    assert!(!shape.is_empty());
}
