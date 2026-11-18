use super::*;
use crate::canvas::{Op, measure};

const ALL: &[NodeShape] = &[
    S::Rect,
    S::Round,
    S::Stadium,
    S::Subroutine,
    S::Cylinder,
    S::Circle,
    S::DoubleCircle,
    S::Asymmetric,
    S::Diamond,
    S::Hexagon,
    S::LeanRight,
    S::LeanLeft,
    S::TrapBottom,
    S::TrapTop,
    S::SmallCircle,
    S::FilledCircle,
    S::FramedCircle,
    S::CrossedCircle,
    S::Bang,
    S::Cloud,
    S::Card,
    S::Hourglass,
    S::Bolt,
    S::BraceLeft,
    S::BraceRight,
    S::Braces,
    S::Delay,
    S::HCylinder,
    S::LinedCylinder,
    S::CurvedTrapezoid,
    S::DividedRect,
    S::Document,
    S::LinedDocument,
    S::Documents,
    S::TaggedDocument,
    S::Triangle,
    S::FlippedTriangle,
    S::Fork,
    S::WindowPane,
    S::LinedRect,
    S::NotchedPentagon,
    S::SlopedRect,
    S::StackedRect,
    S::Flag,
    S::BowRect,
    S::TaggedRect,
    S::Text,
    S::Note,
];

fn inside(p: [f32; 2], c: [f32; 2], s: [f32; 2], slack: f32) -> bool {
    (p[0] - c[0]).abs() <= s[0] / 2.0 + slack && (p[1] - c[1]).abs() <= s[1] / 2.0 + slack
}

#[test]
fn every_shape_stays_in_its_box_and_holds_its_label() {
    let paint = DiagramStyle::neutral(false);
    let label = measure("Process step", paint.font);
    let c = [100.0, 80.0];
    for &shape in ALL {
        let size = node_size(shape, label.size);
        let poly = outline(shape, c, size);
        assert!(poly.len() >= 3, "{shape:?}");
        assert!(poly.iter().all(|&p| inside(p, c, size, 0.5)), "{shape:?} outline leaves its box");
        let mut canvas = Canvas::default();
        draw(&mut canvas, shape, &label, c, size, [paint.node_fill, paint.node_border, paint.text], &paint);
        for op in &canvas.ops {
            let (Op::Fill(points, _) | Op::Stroke { points, .. }) = op;
            assert!(points.iter().all(|&p| inside(p, c, size, 0.5)), "{shape:?} draws outside its box");
        }
        if shape.unlabelled() {
            assert!(canvas.labels.is_empty(), "{shape:?} is too small for a label");
        } else {
            let l = &canvas.labels[0];
            let (lo, hi) = (l.at, [l.at[0] + l.size[0], l.at[1] + l.size[1]]);
            assert!(inside(lo, c, size, 0.5) && inside(hi, c, size, 0.5), "{shape:?} label leaves its box");
        }
    }
}

#[test]
fn boundary_points_stay_on_or_in_the_box() {
    let c = [0.0, 0.0];
    for &shape in ALL {
        let size = node_size(shape, [80.0, 18.0]);
        for toward in [[0.0, 200.0], [0.0, -200.0], [200.0, 0.0], [-150.0, 90.0]] {
            let b = boundary(shape, c, size, toward);
            assert!(inside(b, c, size, 0.5), "{shape:?} toward {toward:?}: {b:?}");
            assert!(b[0].abs() + b[1].abs() > 3.0, "{shape:?} toward {toward:?} stops at the centre");
        }
    }
}

#[test]
fn a_ray_leaves_a_triangle_on_its_slanted_side() {
    let size = [100.0, 60.0];
    let b = boundary(S::Triangle, [0.0, 0.0], size, [0.0, -100.0]);
    assert!((b[1] + 30.0).abs() < 1e-3, "straight up meets the apex: {b:?}");
    let b = boundary(S::Triangle, [0.0, 0.0], size, [100.0, -60.0]);
    assert!(b[0] < 50.0 && b[0] > 0.0 && b[1] > -30.0, "up and right meets the slant first: {b:?}");
}

#[test]
fn a_cylinder_has_mermaids_rims_and_its_label_under_the_front_one() {
    let paint = DiagramStyle::neutral(false);
    let label = measure("Note: lasts, has versions", paint.font);
    let size = node_size(S::Cylinder, label.size);
    let ry = rim(size[0]);
    assert!((ry - size[0] / 2.0 / (2.5 + size[0] / 50.0)).abs() < 1e-3, "rim {ry} for width {}", size[0]);
    let mut c = Canvas::default();
    draw(&mut c, S::Cylinder, &label, [0.0, 0.0], size, [paint.node_fill, paint.node_border, paint.text], &paint);
    let (top, bottom) = (-size[1] / 2.0, size[1] / 2.0);
    // Past the outline, all a cylinder draws is its top rim, however it's painted.
    let body = outline(S::Cylinder, [0.0, 0.0], size);
    let rims: Vec<&Vec<[f32; 2]>> = c.ops.iter().map(|op| match op {
        Op::Fill(points, _) | Op::Stroke { points, .. } => points,
    }).filter(|p| **p != body).collect();
    assert!(!rims.is_empty(), "a front rim on top");
    assert!(rims.iter().flat_map(|p| p.iter()).all(|p| p[1] >= top - 0.01 && p[1] <= top + 2.0 * ry + 0.01), "nothing but the top rim inside the body");
    let l = &c.labels[0];
    assert!(l.at[1] >= top + 2.0 * ry, "the label clears the front rim: {} vs {}", l.at[1], top + 2.0 * ry);
    assert!(l.at[1] + l.size[1] <= bottom - ry, "and the bottom rim");
}
