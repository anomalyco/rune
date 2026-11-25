//! [`Area`]: a [`Path`]'s subpaths filled together under a [`FillRule`], triangulated once when it
//! is made, and hit-tested under the same rule.

use std::sync::Arc;

use super::planar::{is_filled, winding_number};
use super::{FillRule, Path, triangulate_compound};

/// Every subpath of a [`Path`] filled as one region under a [`FillRule`]: an outline with holes,
/// a group of islands, contours that cross. It is triangulated once, by [`Area::new`]; clones
/// share the triangles, so an area kept in state redraws (pans, zooms, repaints) without
/// triangulating again. [`Shape::fill_area`](super::Shape::fill_area) draws it, and
/// [`Area::contains`] hit-tests it under the same rule, so a point in a hole misses.
///
/// ```
/// use rune_core::Color;
/// use rune_core::shape::{Area, FillRule, Path, Shape};
///
/// let land = Path::new()
///     .polygon([[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]])
///     .polygon([[40.0, 40.0], [60.0, 40.0], [60.0, 60.0], [40.0, 60.0]]); // a lake
/// let area = Area::new(&land, FillRule::EvenOdd);
/// assert!(area.contains([10.0, 10.0]));
/// assert!(!area.contains([50.0, 50.0]));
/// let map = Shape::new().fill_area(&area, Color::WHITE);
/// assert!(!map.is_empty());
/// ```
#[derive(Clone, Debug)]
pub struct Area(Arc<Inner>);

/// Three vertex indices and the mask of which edges (bits 0, 1, 2) are antialiased.
type Triangle = ([usize; 3], u32);

#[derive(Debug)]
struct Inner {
    contours: Vec<Vec<[f32; 2]>>,
    rule: FillRule,
    /// `[min_x, min_y, max_x, max_y]` of the contours, if any.
    bounds: Option<[f32; 4]>,
    verts: Vec<[f32; 2]>,
    tris: Vec<Triangle>,
}

impl Area {
    /// Triangulates the closed and open subpaths of `path` (open ones close back to their start)
    /// together under `rule`. Subpaths of fewer than three points add nothing.
    pub fn new(path: &Path, rule: FillRule) -> Area {
        let contours: Vec<Vec<[f32; 2]>> = path
            .subpaths
            .iter()
            .filter(|sub| sub.points.len() >= 3)
            .map(|sub| sub.points.clone())
            .collect();
        let (verts, tris) = if contours.is_empty() { (Vec::new(), Vec::new()) } else { triangulate_compound(&contours, rule) };
        let bounds = contours
            .iter()
            .flatten()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .fold(None, |b: Option<[f32; 4]>, p| {
                let [x0, y0, x1, y1] = b.unwrap_or([p[0], p[1], p[0], p[1]]);
                Some([x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1])])
            });
        Area(Arc::new(Inner { contours, rule, bounds, verts, tris }))
    }

    /// The rule it was filled under.
    pub fn rule(&self) -> FillRule {
        self.0.rule
    }

    /// Whether it fills nothing (no subpath of three or more points, or all of zero area).
    pub fn is_empty(&self) -> bool {
        self.0.tris.is_empty()
    }

    /// `[min_x, min_y, max_x, max_y]` of its contours, or `None` when it has none.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        self.0.bounds
    }

    /// Whether `p` is inside under the area's rule: its winding number across all contours is
    /// non-zero ([`FillRule::NonZero`]) or odd ([`FillRule::EvenOdd`]), so a point in a hole
    /// misses. Points on an edge may land on either side.
    pub fn contains(&self, p: [f32; 2]) -> bool {
        let Some([x0, y0, x1, y1]) = self.0.bounds else { return false };
        if !(p[0] >= x0 && p[0] <= x1 && p[1] >= y0 && p[1] <= y1) {
            return false;
        }
        let q = [p[0] as f64, p[1] as f64];
        let wn: i32 = self
            .0
            .contours
            .iter()
            .map(|c| {
                let poly: Vec<[f64; 2]> = c.iter().filter(|p| p[0].is_finite() && p[1].is_finite()).map(|p| [p[0] as f64, p[1] as f64]).collect();
                winding_number(&poly, q)
            })
            .sum();
        is_filled(wn, self.0.rule)
    }

    /// The contours it was made from, each closed back to its start.
    #[doc(hidden)]
    pub fn contours(&self) -> &[Vec<[f32; 2]>] {
        &self.0.contours
    }

    /// The triangulation: vertices, and triangles of three vertex indices with a mask of which
    /// edges (bits 0, 1, 2) lie on the boundary and are antialiased.
    #[doc(hidden)]
    pub fn triangles(&self) -> (&[[f32; 2]], &[Triangle]) {
        (&self.0.verts, &self.0.tris)
    }

    /// Whether `self` and `other` share one triangulation (one is a clone of the other).
    #[doc(hidden)]
    pub fn shares(&self, other: &Area) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl PartialEq for Area {
    fn eq(&self, other: &Area) -> bool {
        self.shares(other) || (self.0.rule == other.0.rule && self.0.contours == other.0.contours)
    }
}

#[cfg(test)]
#[path = "area_tests.rs"]
mod tests;
