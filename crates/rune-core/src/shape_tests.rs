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
