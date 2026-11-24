use super::*;

fn area(points: &[[f32; 2]], tris: &[([usize; 3], u32)]) -> f32 {
    tris.iter()
        .map(|([a, b, c], _)| {
            let (a, b, c) = (points[*a], points[*b], points[*c]);
            ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5
        })
        .sum()
}

#[test]
fn a_concave_polygon_triangulates_to_its_area_with_boundary_edges_marked() {
    // An L shape, clockwise in y-down screen space.
    let l = [
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 10.0],
        [10.0, 10.0],
        [10.0, 30.0],
        [0.0, 30.0],
    ];
    let tris = triangulate(&l);
    assert_eq!(tris.len(), 4);
    assert!((area(&l, &tris) - 400.0).abs() < 1e-3);
    let boundary_edges: u32 = tris.iter().map(|(_, m)| m.count_ones()).sum();
    assert_eq!(
        boundary_edges, 6,
        "every polygon edge once, no interior edge"
    );
}

#[test]
fn a_waveform_envelope_triangulates_completely() {
    let top: Vec<[f32; 2]> = (0..200)
        .map(|i| [i as f32, 20.0 - ((i as f32) * 0.2).sin().abs() * 18.0 - 1.0])
        .collect();
    let mut env = top.clone();
    env.extend(top.iter().rev().map(|p| [p[0], 40.0 - p[1]]));
    let tris = triangulate(&env);
    assert_eq!(tris.len(), env.len() - 2);
    let exact: f32 = (0..env.len())
        .map(|i| {
            let (a, b) = (env[i], env[(i + 1) % env.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        .abs()
        * 0.5;
    assert!((area(&env, &tris) - exact).abs() < exact * 1e-3);
}

#[test]
fn curves_flatten_within_tolerance_and_end_on_their_points() {
    let p = Path::new()
        .move_to([0.0, 0.0])
        .cubic_to([0.0, 50.0], [100.0, 50.0], [100.0, 0.0])
        .close();
    let s = &p.subpaths[0];
    assert!(s.closed);
    assert_eq!(*s.points.last().unwrap(), [100.0, 0.0]);
    let mid = s.points[s.points.len() / 2];
    assert!((mid[1] - 37.5).abs() < 1.0, "{mid:?}");
}

fn strokes(s: &Shape) -> Vec<Vec<[f32; 2]>> {
    s.ops.iter().filter_map(|op| if let Op::Stroke { points, .. } = op { Some(points.clone()) } else { None }).collect()
}

fn fill_alpha(s: &Shape) -> Option<f32> {
    s.ops.iter().find_map(|op| if let Op::Fill { color, .. } = op { Some(color.a) } else { None })
}

#[test]
fn a_drawn_shape_traces_its_strokes_in_order_and_lands_its_head_last() {
    // A connector: an L-shaped line 100 points long, a second 20-point run, then its head.
    let s = Shape::new()
        .stroke(vec![[0.0, 0.0], [0.0, 60.0], [40.0, 60.0]], 1.0, Color::WHITE)
        .stroke(vec![[40.0, 70.0], [40.0, 90.0]], 1.0, Color::WHITE)
        .fill(vec![[36.0, 90.0], [44.0, 90.0], [40.0, 96.0]], Color::WHITE);
    assert_eq!(strokes(&s.drawn(0.0)), Vec::<Vec<[f32; 2]>>::new(), "nothing drawn yet");
    assert_eq!(fill_alpha(&s.drawn(0.0)), None);
    // 60 of 120 points: down the first leg, not yet round the corner.
    assert_eq!(strokes(&s.drawn(0.5)), vec![vec![[0.0, 0.0], [0.0, 60.0]]]);
    // 80 points: round the corner, halfway along the second leg.
    assert_eq!(strokes(&s.drawn(80.0 / 120.0)), vec![vec![[0.0, 0.0], [0.0, 60.0], [20.0, 60.0]]]);
    // The head fades in over the last 8 points of line.
    assert_eq!(fill_alpha(&s.drawn(108.0 / 120.0)), None);
    let a = fill_alpha(&s.drawn(116.0 / 120.0)).unwrap();
    assert!((a - 0.5).abs() < 1e-4, "{a}");
    assert_eq!(s.drawn(1.0), s, "drawn whole");
    assert_eq!(s.drawn(f32::NAN), s);
}

#[test]
fn a_closed_stroke_draws_on_round_to_its_start_and_fills_alone_fade() {
    let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    let s = Shape::new().stroke_with(square, true, 1.0, LineCap::Round, Color::WHITE);
    assert_eq!(strokes(&s.drawn(0.875)), vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 5.0]]]);
    let dot = Shape::new().fill(vec![[0.0, 0.0], [4.0, 0.0], [2.0, 4.0]], Color::WHITE);
    assert_eq!(fill_alpha(&dot.drawn(0.25)), Some(0.25));
}

#[test]
fn a_trimmed_shape_keeps_the_stretch_between_start_and_end() {
    let s = Shape::new()
        .stroke(vec![[0.0, 0.0], [0.0, 60.0], [40.0, 60.0]], 1.0, Color::WHITE)
        .stroke(vec![[40.0, 70.0], [40.0, 90.0]], 1.0, Color::WHITE);
    // 30 to 105 of 120 points: round the corner, then 5 points down the second stroke.
    assert_eq!(
        strokes(&s.trimmed(0.25, 0.875)),
        vec![vec![[0.0, 30.0], [0.0, 60.0], [40.0, 60.0]], vec![[40.0, 70.0], [40.0, 75.0]]]
    );
    // A cut on a vertex keeps no zero-length piece there.
    assert_eq!(strokes(&s.trimmed(0.5, 0.875)), vec![vec![[0.0, 60.0], [40.0, 60.0]], vec![[40.0, 70.0], [40.0, 75.0]]]);
    assert!(strokes(&s.trimmed(0.6, 0.6)).is_empty(), "nothing between equal ends");
    assert!(strokes(&s.trimmed(0.7, 0.2)).is_empty());
    assert_eq!(s.trimmed(0.0, 1.0), s);
    assert_eq!(s.trimmed(f32::NAN, f32::NAN), s);
    assert_eq!(s.trimmed(0.0, 0.5), s.drawn(0.5));
}

#[test]
fn a_closed_stroke_trims_across_its_closing_edge() {
    let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    let s = Shape::new().stroke_with(square, true, 1.0, LineCap::Round, Color::WHITE);
    assert_eq!(strokes(&s.trimmed(0.625, 1.0)), vec![vec![[5.0, 10.0], [0.0, 10.0], [0.0, 0.0]]]);
}

#[test]
fn empty_and_degenerate_ops_are_dropped() {
    let s = Shape::new()
        .stroke(vec![[0.0, 0.0]], 1.0, Color::WHITE)
        .fill(vec![[0.0, 0.0], [1.0, 1.0]], Color::WHITE);
    assert!(s.is_empty());
}

/// Exact half-open triangle ownership matching Rune's `KIND_TRIANGLE` shader (`owns`), so
/// points on shared interior diagonals or bridges are owned by exactly one triangle.
fn point_in_tri(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let owns = |u: [f32; 2], v: [f32; 2], w: [f32; 2]| {
        let swap = u[0] > v[0] || (u[0] == v[0] && u[1] > v[1]);
        let (lo, hi) = if swap { (v, u) } else { (u, v) };
        let e = [hi[0] - lo[0], hi[1] - lo[1]];
        let side = e[0] * (w[1] - lo[1]) - e[1] * (w[0] - lo[0]);
        let val = e[0] * (p[1] - lo[1]) - e[1] * (p[0] - lo[0]);
        if side > 0.0 { val >= 0.0 } else { val < 0.0 }
    };
    owns(a, b, c) && owns(b, c, a) && owns(c, a, b)
}

fn hits_at(verts: &[[f32; 2]], tris: &[([usize; 3], u32)], p: [f32; 2]) -> usize {
    tris.iter()
        .filter(|([a, b, c], _)| point_in_tri(p, verts[*a], verts[*b], verts[*c]))
        .count()
}

fn rect_cw(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn rect_ccw(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
    vec![[x0, y0], [x0, y1], [x1, y1], [x1, y0]]
}

fn pentagram(cx: f32, cy: f32, r: f32) -> Vec<[f32; 2]> {
    let order = [0usize, 2, 4, 1, 3];
    order
        .iter()
        .map(|&k| {
            let a = k as f32 * std::f32::consts::TAU / 5.0 - std::f32::consts::FRAC_PI_2;
            [cx + a.cos() * r, cy + a.sin() * r]
        })
        .collect()
}

fn aa_edge_count(tris: &[([usize; 3], u32)]) -> u32 {
    tris.iter().map(|(_, m)| m.count_ones()).sum()
}

#[test]
fn compound_frame_cuts_transparent_hole_and_leaves_bridges_unmarked() {
    let contours = vec![rect_cw(0.0, 0.0, 200.0, 200.0), rect_ccw(50.0, 50.0, 150.0, 150.0)];
    let (verts, tris) = triangulate_compound(&contours, FillRule::NonZero);
    assert!((area(&verts, &tris) - 30_000.0).abs() < 1e-2, "area={}", area(&verts, &tris));
    assert_eq!(hits_at(&verts, &tris, [100.0, 100.0]), 0, "center hole must be uncovered");
    for pt in [[25.0, 100.0], [175.0, 100.0], [100.0, 25.0], [100.0, 175.0]] {
        assert_eq!(hits_at(&verts, &tris, pt), 1, "frame point {pt:?} covered once");
    }
    assert_eq!(aa_edge_count(&tris), 8, "4 outer + 4 hole boundary edges only; no bridge/diagonal AA");
    // Explicitly confirm no edge connecting an outer vertex to an inner vertex carries an AA bit.
    let is_outer = |p: [f32; 2]| p[0] == 0.0 || p[0] == 200.0 || p[1] == 0.0 || p[1] == 200.0;
    for &([ia, ib, ic], m) in &tris {
        for (bit, (u, v)) in [(1, (ia, ib)), (2, (ib, ic)), (4, (ic, ia))] {
            if is_outer(verts[u]) != is_outer(verts[v]) {
                assert_eq!(m & bit, 0, "bridge edge ({:?} -> {:?}) must not be AA-marked", verts[u], verts[v]);
            }
        }
    }
}

#[test]
fn same_winding_inner_ring_fills_under_nonzero_and_cuts_hole_under_evenodd() {
    let contours = vec![rect_cw(0.0, 0.0, 100.0, 100.0), rect_cw(25.0, 25.0, 75.0, 75.0)];
    let (vn, tn) = triangulate_compound(&contours, FillRule::NonZero);
    assert!((area(&vn, &tn) - 10_000.0).abs() < 1e-2);
    assert_eq!(hits_at(&vn, &tn, [50.0, 50.0]), 1, "NonZero covers inner ring once without double-fill");
    assert_eq!(aa_edge_count(&tn), 4, "redundant inner contour emits no AA edges");

    let (ve, te) = triangulate_compound(&contours, FillRule::EvenOdd);
    assert!((area(&ve, &te) - 7_500.0).abs() < 1e-2);
    assert_eq!(hits_at(&ve, &te, [50.0, 50.0]), 0, "EvenOdd cuts transparent hole");
    assert_eq!(hits_at(&ve, &te, [10.0, 50.0]), 1);
    assert_eq!(aa_edge_count(&te), 8);
}

#[test]
fn multiple_holes_and_concave_outer_visibility_triangulate_cleanly() {
    // Concave U-shaped outer polygon with a hanging prong that blocks direct horizontal rays
    // from a hole in the left lobe to the right outer wall.
    let u_outer = vec![
        [0.0, 0.0], [100.0, 0.0], [100.0, 80.0], [120.0, 80.0],
        [120.0, 0.0], [220.0, 0.0], [220.0, 160.0], [0.0, 160.0],
    ];
    let left_hole = rect_ccw(20.0, 20.0, 70.0, 60.0);
    let right_hole = rect_ccw(150.0, 20.0, 200.0, 60.0);
    let bottom_hole = rect_ccw(60.0, 100.0, 160.0, 140.0);
    let expected_area = (220.0 * 160.0 - 20.0 * 80.0) - (50.0 * 40.0 + 50.0 * 40.0 + 100.0 * 40.0);
    let (verts, tris) = triangulate_compound(
        &[u_outer, left_hole, right_hole, bottom_hole],
        FillRule::NonZero,
    );
    assert!((area(&verts, &tris) - expected_area).abs() < 1e-1, "got {}", area(&verts, &tris));
    assert_eq!(hits_at(&verts, &tris, [45.0, 40.0]), 0, "left hole");
    assert_eq!(hits_at(&verts, &tris, [175.0, 40.0]), 0, "right hole");
    assert_eq!(hits_at(&verts, &tris, [110.0, 120.0]), 0, "bottom hole");
    assert_eq!(hits_at(&verts, &tris, [110.0, 40.0]), 0, "outside notch");
    assert_eq!(hits_at(&verts, &tris, [10.0, 40.0]), 1);
    assert_eq!(hits_at(&verts, &tris, [110.0, 90.0]), 1);
    assert_eq!(aa_edge_count(&tris), 8 + 4 + 4 + 4);
}

#[test]
fn nested_island_inside_hole_and_four_level_bullseye() {
    let contours = vec![
        rect_cw(0.0, 0.0, 200.0, 200.0),
        rect_ccw(20.0, 20.0, 180.0, 180.0),
        rect_cw(40.0, 40.0, 160.0, 160.0),
        rect_ccw(70.0, 70.0, 130.0, 130.0),
    ];
    let expected = (200.0 * 200.0 - 160.0 * 160.0) + (120.0 * 120.0 - 60.0 * 60.0);
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let (verts, tris) = triangulate_compound(&contours, rule);
        assert!((area(&verts, &tris) - expected).abs() < 1e-1, "{rule:?}: {}", area(&verts, &tris));
        assert_eq!(hits_at(&verts, &tris, [10.0, 100.0]), 1, "outer ring");
        assert_eq!(hits_at(&verts, &tris, [30.0, 100.0]), 0, "outer hole");
        assert_eq!(hits_at(&verts, &tris, [55.0, 100.0]), 1, "island ring");
        assert_eq!(hits_at(&verts, &tris, [100.0, 100.0]), 0, "inner hole");
        assert_eq!(aa_edge_count(&tris), 16);
    }
}

#[test]
fn self_crossing_pentagram_star_respects_nonzero_and_evenodd() {
    let star = vec![pentagram(100.0, 100.0, 80.0)];
    let (vn, tn) = triangulate_compound(&star, FillRule::NonZero);
    let (ve, te) = triangulate_compound(&star, FillRule::EvenOdd);

    // NonZero fills the center pentagon once with no interior AA edges.
    assert_eq!(hits_at(&vn, &tn, [100.0, 100.0]), 1, "NonZero fills star center");
    assert_eq!(hits_at(&vn, &tn, [100.0, 35.0]), 1, "NonZero fills top spike");
    assert_eq!(aa_edge_count(&tn), 10, "NonZero pentagram has only 10 outer boundary edges");

    // EvenOdd leaves the center pentagon empty while filling all 5 spikes.
    assert_eq!(hits_at(&ve, &te, [100.0, 100.0]), 0, "EvenOdd leaves star center hollow");
    assert_eq!(hits_at(&ve, &te, [100.0, 35.0]), 1, "EvenOdd fills top spike");
    assert_eq!(aa_edge_count(&te), 15, "EvenOdd pentagram has 5 triangular spikes = 15 boundary edges");
    assert!(area(&vn, &tn) > area(&ve, &te) + 1000.0, "NonZero area must exceed EvenOdd by inner pentagon");
}

#[test]
fn overlapping_contours_merge_under_nonzero_and_xor_under_evenodd() {
    let contours = vec![rect_cw(0.0, 0.0, 100.0, 100.0), rect_cw(50.0, 50.0, 150.0, 150.0)];
    let (vn, tn) = triangulate_compound(&contours, FillRule::NonZero);
    assert!((area(&vn, &tn) - 17_500.0).abs() < 1e-1, "union area={}", area(&vn, &tn));
    assert_eq!(hits_at(&vn, &tn, [75.0, 75.0]), 1, "overlap filled once without double-blending");
    assert_eq!(aa_edge_count(&tn), 8, "8-edge union outline");

    let (ve, te) = triangulate_compound(&contours, FillRule::EvenOdd);
    assert!((area(&ve, &te) - 15_000.0).abs() < 1e-1, "xor area={}", area(&ve, &te));
    assert_eq!(hits_at(&ve, &te, [75.0, 75.0]), 0, "overlap hollow under EvenOdd");
    assert_eq!(hits_at(&ve, &te, [25.0, 25.0]), 1);
    assert_eq!(hits_at(&ve, &te, [125.0, 125.0]), 1);
}

#[test]
fn compound_edges_handle_empty_degenerate_non_finite_and_trimming() {
    let empty = Shape::new().fill_path_with(&Path::new(), FillRule::EvenOdd, Color::WHITE);
    assert!(empty.is_empty());

    let degenerate = vec![
        vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]],
        vec![[f32::NAN, 0.0], [10.0, 0.0], [10.0, 10.0]],
        vec![[f32::INFINITY, 0.0], [10.0, 0.0], [10.0, 10.0]],
    ];
    let (_, tris) = triangulate_compound(&degenerate, FillRule::NonZero);
    assert!(tris.is_empty());

    // Duplicate closing vertex matches clean polygon.
    let closed_dup = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]]];
    let (v, t) = triangulate_compound(&closed_dup, FillRule::NonZero);
    assert!((area(&v, &t) - 100.0).abs() < 1e-2);

    // Single compound fill does not self-overlap; drawn() fades it.
    let path = Path::new()
        .move_to([0.0, 0.0]).line_to([40.0, 0.0]).line_to([40.0, 40.0]).line_to([0.0, 40.0]).close()
        .move_to([10.0, 10.0]).line_to([10.0, 30.0]).line_to([30.0, 30.0]).line_to([30.0, 10.0]).close();
    let s = Shape::new().fill_path(&path, Color::WHITE);
    assert!(!s.overlaps());
    let half = s.drawn(0.5);
    match &half.ops[0] {
        Op::FillCompound { color, rule, .. } => {
            assert_eq!(*rule, FillRule::NonZero);
            assert!((color.a - 0.5).abs() < 1e-4);
        }
        other => panic!("expected FillCompound, got {other:?}"),
    }
}

#[test]
fn curved_bezier_ring_and_touching_hole_triangulate_without_overlap() {
    // Approximate a circle with 4 cubic Bezier arcs.
    let add_circle = |mut p: Path, cx: f32, cy: f32, r: f32, ccw: bool| -> Path {
        let k = r * 0.552_284_8;
        if !ccw {
            p = p.move_to([cx + r, cy])
                .cubic_to([cx + r, cy + k], [cx + k, cy + r], [cx, cy + r])
                .cubic_to([cx - k, cy + r], [cx - r, cy + k], [cx - r, cy])
                .cubic_to([cx - r, cy - k], [cx - k, cy - r], [cx, cy - r])
                .cubic_to([cx + k, cy - r], [cx + r, cy - k], [cx + r, cy])
                .close();
        } else {
            p = p.move_to([cx + r, cy])
                .cubic_to([cx + r, cy - k], [cx + k, cy - r], [cx, cy - r])
                .cubic_to([cx - k, cy - r], [cx - r, cy - k], [cx - r, cy])
                .cubic_to([cx - r, cy + k], [cx - k, cy + r], [cx, cy + r])
                .cubic_to([cx + k, cy + r], [cx + r, cy + k], [cx + r, cy])
                .close();
        }
        p
    };
    let ring = add_circle(add_circle(Path::new(), 50.0, 50.0, 40.0, false), 50.0, 50.0, 20.0, true);
    let contours: Vec<Vec<[f32; 2]>> = ring.subpaths.iter().map(|s| s.points.clone()).collect();
    let (verts, tris) = triangulate_compound(&contours, FillRule::NonZero);
    let expected = std::f32::consts::PI * (40.0 * 40.0 - 20.0 * 20.0);
    assert!((area(&verts, &tris) - expected).abs() < 40.0, "got {}", area(&verts, &tris));
    assert_eq!(hits_at(&verts, &tris, [50.0, 50.0]), 0, "center hole empty");
    assert_eq!(hits_at(&verts, &tris, [80.0, 50.0]), 1, "ring wall covered once");

    // Hole whose corner touches the outer boundary edge.
    let touching = vec![
        rect_cw(0.0, 0.0, 100.0, 100.0),
        vec![[50.0, 0.0], [25.0, 50.0], [50.0, 75.0], [75.0, 50.0]],
    ];
    let (vt, tt) = triangulate_compound(&touching, FillRule::EvenOdd);
    assert_eq!(hits_at(&vt, &tt, [50.0, 40.0]), 0, "touching diamond hole empty");
    assert_eq!(hits_at(&vt, &tt, [10.0, 50.0]), 1);
    assert_eq!(hits_at(&vt, &tt, [90.0, 50.0]), 1);
    let hole_area = 0.5 * 50.0 * 75.0;
    assert!((area(&vt, &tt) - (10_000.0 - hole_area)).abs() < 1e-1, "got {}", area(&vt, &tt));
}
