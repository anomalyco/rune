use super::*;
use crate::Color;
use crate::shape::{Op, Shape};

fn square(x: f32, y: f32, s: f32) -> [[f32; 2]; 4] {
    [[x, y], [x + s, y], [x + s, y + s], [x, y + s]]
}

/// Land with a lake, wound the same way, so the rules differ in the lake.
fn land() -> Path {
    Path::new().polygon(square(0.0, 0.0, 100.0)).polygon(square(40.0, 40.0, 20.0))
}

#[test]
fn contains_misses_holes_under_even_odd() {
    let area = Area::new(&land(), FillRule::EvenOdd);
    assert!(area.contains([10.0, 10.0]));
    assert!(!area.contains([50.0, 50.0]), "a point in the lake misses");
    assert!(!area.contains([150.0, 50.0]));
    let nonzero = Area::new(&land(), FillRule::NonZero);
    assert!(nonzero.contains([50.0, 50.0]), "same-winding nesting fills under NonZero");
}

#[test]
fn contains_misses_opposite_wound_hole_under_nonzero() {
    let mut lake = square(40.0, 40.0, 20.0);
    lake.reverse();
    let area = Area::new(&Path::new().polygon(square(0.0, 0.0, 100.0)).polygon(lake), FillRule::NonZero);
    assert!(area.contains([10.0, 90.0]));
    assert!(!area.contains([50.0, 50.0]));
}

#[test]
fn contains_follows_the_rule_on_a_crossing_star() {
    let star: Vec<[f32; 2]> = (0..5)
        .map(|i| {
            let a = (i * 2) as f32 * std::f32::consts::TAU / 5.0 - std::f32::consts::FRAC_PI_2;
            [100.0 + 80.0 * a.cos(), 100.0 + 80.0 * a.sin()]
        })
        .collect();
    let path = Path::new().polygon(star);
    assert!(Area::new(&path, FillRule::NonZero).contains([100.0, 100.0]));
    assert!(!Area::new(&path, FillRule::EvenOdd).contains([100.0, 100.0]));
}

#[test]
fn triangles_match_a_path_fill_and_clones_share_them() {
    let area = Area::new(&land(), FillRule::EvenOdd);
    let (verts, tris) = area.triangles();
    let contours: Vec<Vec<[f32; 2]>> = vec![square(0.0, 0.0, 100.0).to_vec(), square(40.0, 40.0, 20.0).to_vec()];
    let (v, t) = crate::shape::triangulate_compound(&contours, FillRule::EvenOdd);
    assert_eq!((verts, tris), (v.as_slice(), t.as_slice()));
    let copy = area.clone();
    assert!(copy.shares(&area));
    assert_eq!(copy, Area::new(&land(), FillRule::EvenOdd));
    assert_ne!(area, Area::new(&land(), FillRule::NonZero));
}

#[test]
fn fill_area_draws_the_area_and_skips_empty_ones() {
    let area = Area::new(&land(), FillRule::EvenOdd);
    let shape = Shape::new().fill_area(&area, Color::WHITE);
    let [Op::FillArea { area: drawn, .. }] = shape.ops.as_slice() else { panic!("one FillArea op") };
    assert!(drawn.shares(&area));
    let faded = shape.drawn(0.5);
    assert!(matches!(faded.ops.as_slice(), [Op::FillArea { area: a, .. }] if a.shares(&area)));

    let empty = Area::new(&Path::new().polygon([[0.0, 0.0], [1.0, 1.0]]), FillRule::NonZero);
    assert!(empty.is_empty());
    assert!(!empty.contains([0.5, 0.5]));
    assert!(Shape::new().fill_area(&empty, Color::WHITE).is_empty());
}

#[test]
fn polygon_replaces_a_dangling_move_to_and_ignores_short_input() {
    let p = Path::new().move_to([5.0, 5.0]).polygon(square(0.0, 0.0, 10.0));
    assert_eq!(p, Path::new().polygon(square(0.0, 0.0, 10.0)));
    assert_eq!(Path::new().polygon([[1.0, 1.0]]), Path::new());
    assert_eq!(Area::new(&p, FillRule::NonZero).bounds(), Some([0.0, 0.0, 10.0, 10.0]));
}
