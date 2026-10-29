use super::*;
use crate::{DiagramStyle, Op, scene};

/// The largest turn between consecutive segments of `pts`, in degrees.
fn sharpest_turn(pts: &[[f32; 2]]) -> f32 {
    let dirs: Vec<f32> = pts.windows(2).filter(|w| crate::dist(w[0], w[1]) > 1e-3).map(|w| (w[1][1] - w[0][1]).atan2(w[1][0] - w[0][0])).collect();
    dirs.windows(2)
        .map(|d| {
            let mut a = (d[1] - d[0]).to_degrees().abs();
            if a > 180.0 {
                a = 360.0 - a;
            }
            a
        })
        .fold(0.0, f32::max)
}

#[test]
fn curves_leave_and_arrive_along_the_rank_axis() {
    let pts = curve(&[[0.0, 0.0], [40.0, 100.0], [40.0, 200.0]]);
    assert_eq!(pts.first(), Some(&[0.0, 0.0]));
    assert_eq!(pts.last(), Some(&[40.0, 200.0]));
    let (a, b) = (pts[0], pts[1]);
    assert!((b[0] - a[0]).abs() < (b[1] - a[1]).abs() * 0.05, "leaves straight down: {a:?} → {b:?}");
    assert!(sharpest_turn(&pts) < 6.0, "no kink where the segments meet: {}", sharpest_turn(&pts));
    assert_eq!(cubic([5.0, 0.0], [5.0, 90.0]).len(), 2, "an aligned pair is one straight segment");
}

#[test]
fn flowchart_edges_draw_without_kinks() {
    let src = "flowchart TD\n  A[Audio] --> B[Parakeet writes text<br/>'went to the avatar']\n  B --> C{Any word that looks<br/>or sounds like a term?}\n  C -- no --> F[Paste as is]\n  C -- \"avatar ≈ Abattoir\" --> D[Score the same audio twice:<br/>as 'avatar' and as 'Abattoir']\n  D --> E{Abattoir score + boost<br/>≥ avatar score?}\n  E -- yes --> G[Write Abattoir]\n  E -- no --> F";
    let paint = DiagramStyle::neutral(true);
    let Some(crate::Diagram::Flowchart(f)) = crate::parse(src) else { panic!() };
    let c = crate::flowchart_draw::draw_canvas(&f, &paint).expect("draws");
    let lines: Vec<&Vec<[f32; 2]>> = c.ops.iter().filter_map(|op| match op {
        Op::Stroke { points, closed: false, color, .. } if *color == paint.line => Some(points),
        _ => None,
    }).collect();
    assert_eq!(lines.len(), 11, "one stroke per edge, broken around its label");
    for l in lines {
        assert!(sharpest_turn(l) < 12.0, "edge turns {}° at once: {l:?}", sharpest_turn(l));
    }
    assert_eq!(c.traces.len(), 7, "every edge keeps a trace for hover");
    assert!(scene(src, &paint).is_some());
}


#[test]
fn arrows_point_into_the_node_they_reach() {
    let paint = DiagramStyle::neutral(true);
    let Some(crate::Diagram::Flowchart(f)) = crate::parse("flowchart TD\n  A[Audio] --> B[Parakeet]") else { panic!() };
    let c = crate::flowchart_draw::draw_canvas(&f, &paint).expect("draws");
    let head = c.ops.iter().find_map(|op| match op {
        Op::Fill(p, col) if *col == paint.line && p.len() == 3 => Some(p.clone()),
        _ => None,
    });
    let tip = head.expect("an arrowhead")[0];
    let b_top = c.regions[1].iter().map(|p| p[1]).fold(f32::MAX, f32::min);
    assert!((tip[1] - b_top).abs() < 0.5, "tip {tip:?} on B's top {b_top}");
}

#[test]
fn long_edges_stay_clear_of_the_nodes_they_pass() {
    let paint = DiagramStyle::neutral(true);
    let src = "flowchart TD\n  A[Audio] --> B[Parakeet writes text]\n  B --> C{Any word that looks<br/>or sounds like a term?}\n  C -- no --> F[Paste as is]\n  C -- \"avatar ≈ Abattoir\" --> D[Score the same audio twice:<br/>as 'avatar' and as 'Abattoir']\n  D --> E{Abattoir score + boost<br/>≥ avatar score?}\n  E -- yes --> G[Write Abattoir]\n  E -- no --> F";
    let Some(crate::Diagram::Flowchart(f)) = crate::parse(src) else { panic!() };
    let c = crate::flowchart_draw::draw_canvas(&f, &paint).expect("draws");
    let (d, e) = (&c.regions[4], &c.regions[5]);
    let lines: Vec<&Vec<[f32; 2]>> = c.ops.iter().filter_map(|op| match op {
        Op::Stroke { points, closed: false, color, .. } if *color == paint.line => Some(points),
        _ => None,
    }).collect();
    // The long "no" edge, C to F, in two runs around its label.
    for w in lines[2..4].iter().flat_map(|l| l.windows(2)) {
        for k in 0..=8 {
            let p = crate::lerp(w[0], w[1], k as f32 / 8.0);
            assert!(!inside(d, p) && !inside(e, p), "the long \"no\" edge crosses a node at {p:?}");
        }
    }
}

#[test]
fn labels_break_the_line_instead_of_covering_it() {
    let rect = [10.0, 40.0, 30.0, 60.0];
    let runs = crate::flowchart_draw::outside(&[[20.0, 0.0], [20.0, 50.0], [20.0, 100.0]], rect);
    assert_eq!(runs.len(), 2);
    assert!((runs[0].last().unwrap()[1] - 40.0).abs() < 0.1 && (runs[1][0][1] - 60.0).abs() < 0.1, "{runs:?}");
    assert_eq!(crate::flowchart_draw::outside(&[[0.0, 0.0], [0.0, 9.0]], rect).len(), 1, "a curve that misses the label is whole");
}

#[test]
fn every_direction_draws_smooth_edges_that_reach_their_nodes() {
    let paint = DiagramStyle::neutral(true);
    for dir in ["TD", "BT", "LR", "RL"] {
        let src = format!("flowchart {dir}\n  A[Start] --> B[Left]\n  A --> C[Right]\n  B --> D[End]\n  C -- label --> D\n  A --> D");
        let Some(crate::Diagram::Flowchart(f)) = crate::parse(&src) else { panic!() };
        let c = crate::flowchart_draw::draw_canvas(&f, &paint).expect("draws");
        let mut lines = 0;
        for op in &c.ops {
            if let Op::Stroke { points, closed: false, color, .. } = op
                && *color == paint.line
            {
                lines += 1;
                assert!(sharpest_turn(points) < 12.0, "{dir}: edge turns {}° at once", sharpest_turn(points));
            }
        }
        assert!(lines >= 5, "{dir}: {lines} strokes");
        let tips: Vec<[f32; 2]> = c.ops.iter().filter_map(|op| match op {
            Op::Fill(p, col) if *col == paint.line && p.len() == 3 => Some(p[0]),
            _ => None,
        }).collect();
        assert_eq!(tips.len(), 5, "{dir}: one arrowhead per edge");
        for t in tips {
            let touching = c.regions.iter().any(|r| (0..r.len()).any(|k| seg_dist(t, r[k], r[(k + 1) % r.len()]) < 1.0));
            assert!(touching, "{dir}: arrow tip {t:?} doesn't touch a node");
        }
    }
}
