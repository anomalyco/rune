//! Skia's analytic edges (`SkAnalyticEdge.cpp`, `SkAnalyticEdge.h`) and its edge builder for an
//! unclipped path (`SkEdgeBuilder.cpp`), with the 16.16 fixed point they step in.

use tiny_skia_path::path_geometry::{chop_cubic_at2, chop_quad_at, find_unit_quad_roots, new_t_values, valid_unit_divide};
use tiny_skia_path::{NormalizedF32Exclusive, Path, PathVerb, Point};

pub(super) type Fixed = i32;
pub(super) type FDot6 = i32;
pub(super) const ONE: Fixed = 1 << 16;
pub(super) const MAX: i32 = i32::MAX;
pub(super) const MIN: i32 = i32::MIN;
pub(super) const ACCURACY: i32 = 2;
const MAX_COEFF_SHIFT: i32 = 6;

pub(super) fn fdot6_to_fixed(x: FDot6) -> Fixed {
    x.wrapping_shl(10)
}
pub(super) fn fixed_to_fdot6(x: Fixed) -> FDot6 {
    x >> 10
}
pub(super) fn fdot6_round(x: FDot6) -> i32 {
    (x + 32) >> 6
}
pub(super) fn fixed_mul(a: Fixed, b: Fixed) -> Fixed {
    ((a as i64 * b as i64) >> 16) as i32
}
pub(super) fn fixed_div(a: i32, b: i32) -> Fixed {
    (((a as i64) << 16) / b as i64).clamp(MIN as i64, MAX as i64) as i32
}
pub(super) fn fdot6_div(a: FDot6, b: FDot6) -> Fixed {
    if a as i16 as i32 == a { (a << 16) / b } else { fixed_div(a, b) }
}
pub(super) fn ceil_to_int(x: Fixed) -> i32 {
    ((x as i64 + 0xFFFF) >> 16) as i32
}
pub(super) fn floor_to_int(x: Fixed) -> i32 {
    x >> 16
}
pub(super) fn ceil_to_fixed(x: Fixed) -> Fixed {
    ((x as i64 + 0xFFFF) as i32) & !0xFFFF
}
pub(super) fn floor_to_fixed(x: Fixed) -> Fixed {
    x & !0xFFFF
}
pub(super) fn round_to_fixed(x: Fixed) -> Fixed {
    (x.wrapping_add(0x8000)) & !0xFFFF
}
pub(super) fn sat_add(a: i32, b: i32) -> i32 {
    a.saturating_add(b)
}

/// A point to `FDot6` scaled up `shift` bits, as Skia converts it: `x` times `2^(6 + shift)`,
/// truncated (Chromium doesn't build Skia with `SK_RASTERIZE_EVEN_ROUNDING`; rounding misses its
/// pixels by up to 70 levels).
pub(super) fn to_fdot6(x: f32, shift: i32) -> FDot6 {
    (x * (1 << (6 + shift)) as f32) as i32
}

pub(super) fn snap_y(y: Fixed) -> Fixed {
    ((y as u32).wrapping_add((ONE >> (ACCURACY + 1)) as u32) >> (16 - ACCURACY) << (16 - ACCURACY)) as i32
}

/// `quick_inverse`: Skia's table of `(1 << 22) / x`, truncated, for |x| up to 1024.
pub(super) fn quick_inverse(x: FDot6) -> Fixed {
    if x == 0 { 0 } else { (1 << 22) / x }
}

pub(super) fn quick_div(a: FDot6, b: FDot6) -> Fixed {
    let (abs_a, abs_b) = (a.unsigned_abs(), b.unsigned_abs());
    if (8..1024).contains(&abs_b) && abs_a < (1 << 12) {
        return (a * quick_inverse(b)) >> 6;
    }
    fdot6_div(a, b)
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Line,
    Quad,
    Cubic,
}

#[derive(Clone, Copy)]
pub(super) struct Edge {
    pub(super) next: usize,
    pub(super) prev: usize,
    pub(super) x: Fixed,
    pub(super) dx: Fixed,
    pub(super) upper_x: Fixed,
    pub(super) y: Fixed,
    pub(super) upper_y: Fixed,
    pub(super) lower_y: Fixed,
    pub(super) dy: Fixed,
    pub(super) kind: Kind,
    pub(super) curve_count: i32,
    pub(super) curve_shift: i32,
    pub(super) winding: i32,
    // A quadratic's state (`SkAnalyticQuadraticEdge`) or a cubic's (`SkAnalyticCubicEdge`).
    pub(super) qx: Fixed,
    pub(super) qy: Fixed,
    pub(super) qdx: Fixed,
    pub(super) qdy: Fixed,
    pub(super) qddx: Fixed,
    pub(super) qddy: Fixed,
    pub(super) qdddx: Fixed,
    pub(super) qdddy: Fixed,
    pub(super) last_x: Fixed,
    pub(super) last_y: Fixed,
    pub(super) snapped_x: Fixed,
    pub(super) snapped_y: Fixed,
    pub(super) to_fixed_shift: i32,
}

impl Edge {
    pub(super) fn new() -> Self {
        Edge {
            next: 0,
            prev: 0,
            x: 0,
            dx: 0,
            upper_x: 0,
            y: 0,
            upper_y: 0,
            lower_y: 0,
            dy: 0,
            kind: Kind::Line,
            curve_count: 0,
            curve_shift: 0,
            winding: 1,
            qx: 0,
            qy: 0,
            qdx: 0,
            qdy: 0,
            qddx: 0,
            qddy: 0,
            qdddx: 0,
            qdddy: 0,
            last_x: 0,
            last_y: 0,
            snapped_x: 0,
            snapped_y: 0,
            to_fixed_shift: 0,
        }
    }

    pub(super) fn sentinel(at: i32) -> Self {
        Edge { x: at, upper_x: at, upper_y: at, lower_y: at, dy: MAX, ..Edge::new() }
    }

    /// `SkAnalyticEdge::setLine`.
    pub(super) fn set_line(&mut self, p0: Point, p1: Point) -> bool {
        let fx = |v: f32| fdot6_to_fixed(to_fdot6(v, ACCURACY)) >> ACCURACY;
        let (mut x0, mut y0, mut x1, mut y1) = (fx(p0.x), snap_y(fx(p0.y)), fx(p1.x), snap_y(fx(p1.y)));
        let mut winding = 1;
        if y0 > y1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
            winding = -1;
        }
        let dy = fixed_to_fdot6(y1 - y0);
        if dy == 0 {
            return false;
        }
        let dx = fixed_to_fdot6(x1.wrapping_sub(x0));
        let slope = quick_div(dx, dy);
        let abs_slope = slope.unsigned_abs() as i32;
        self.x = x0;
        self.dx = slope;
        self.upper_x = x0;
        self.y = y0;
        self.upper_y = y0;
        self.lower_y = y1;
        self.dy = if dx == 0 || slope == 0 {
            MAX
        } else if abs_slope < 1024 {
            quick_inverse(abs_slope)
        } else {
            quick_div(dy, dx).unsigned_abs() as i32
        };
        self.kind = Kind::Line;
        self.curve_count = 0;
        self.winding = winding;
        self.curve_shift = 0;
        true
    }

    /// `SkAnalyticEdge::updateLine`.
    pub(super) fn update_line(&mut self, mut x0: Fixed, mut y0: Fixed, mut x1: Fixed, mut y1: Fixed, slope: Fixed) -> bool {
        if y0 > y1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
            self.winding = -self.winding;
        }
        let dx = fixed_to_fdot6(x1.wrapping_sub(x0));
        let dy = fixed_to_fdot6(y1 - y0);
        if dy == 0 {
            return false;
        }
        let abs_slope = fixed_to_fdot6(slope).unsigned_abs() as i32;
        self.x = x0;
        self.dx = slope;
        self.upper_x = x0;
        self.y = y0;
        self.upper_y = y0;
        self.lower_y = y1;
        self.dy = if dx == 0 || slope == 0 {
            MAX
        } else if abs_slope < 1024 {
            quick_inverse(abs_slope)
        } else {
            quick_div(dy, dx).unsigned_abs() as i32
        };
        true
    }

    /// `SkAnalyticQuadraticEdge::setQuadratic`.
    pub(super) fn set_quad(&mut self, pts: &[Point]) -> bool {
        let f = |v: f32| to_fdot6(v, ACCURACY);
        let (mut x0, mut y0, x1, y1, mut x2, mut y2) = (f(pts[0].x), f(pts[0].y), f(pts[1].x), f(pts[1].y), f(pts[2].x), f(pts[2].y));
        let mut winding = 1;
        if y0 > y2 {
            std::mem::swap(&mut x0, &mut x2);
            std::mem::swap(&mut y0, &mut y2);
            winding = -1;
        }
        if fdot6_round(y0) == fdot6_round(y2) {
            return false;
        }
        let mut shift = diff_to_shift(((x1 << 1) - x0 - x2) >> 2, ((y1 << 1) - y0 - y2) >> 2, ACCURACY);
        shift = if shift == 0 { 1 } else { shift.min(MAX_COEFF_SHIFT) };
        self.winding = winding;
        self.kind = Kind::Quad;
        self.curve_count = 1 << shift;
        self.curve_shift = shift - 1;
        let div2 = |v: FDot6| v.wrapping_shl(16 - 6 - 1);
        let a = div2(x0 - x1 - x1 + x2);
        let b = fdot6_to_fixed(x1 - x0);
        self.qx = fdot6_to_fixed(x0);
        self.qdx = b + (a >> shift);
        self.qddx = a >> (shift - 1);
        let a = div2(y0 - y1 - y1 + y2);
        let b = fdot6_to_fixed(y1 - y0);
        self.qy = fdot6_to_fixed(y0);
        self.qdy = b + (a >> shift);
        self.qddy = a >> (shift - 1);
        self.last_x = fdot6_to_fixed(x2);
        self.last_y = fdot6_to_fixed(y2);
        for v in [&mut self.qx, &mut self.qy, &mut self.qdx, &mut self.qdy, &mut self.qddx, &mut self.qddy, &mut self.last_x, &mut self.last_y] {
            *v >>= ACCURACY;
        }
        self.qy = snap_y(self.qy);
        self.last_y = snap_y(self.last_y);
        self.snapped_x = self.qx;
        self.snapped_y = self.qy;
        self.update_quad()
    }

    /// `SkAnalyticQuadraticEdge::updateQuadratic`.
    pub(super) fn update_quad(&mut self) -> bool {
        let mut success = false;
        let mut count = self.curve_count;
        let (mut oldx, mut oldy) = (self.qx, self.qy);
        let (mut dx, mut dy) = (self.qdx, self.qdy);
        let (mut newx, mut newy, mut new_sx, mut new_sy);
        let shift = self.curve_shift;
        loop {
            let slope;
            count -= 1;
            if count > 0 {
                newx = oldx + (dx >> shift);
                newy = oldy + (dy >> shift);
                if (dy >> shift).unsigned_abs() as i64 >= (ONE * 2) as i64 && ((dy.unsigned_abs() as i64) << 6) > dx.unsigned_abs() as i64 {
                    let diff_y = fixed_to_fdot6(newy - self.snapped_y);
                    slope = if diff_y != 0 { quick_div(fixed_to_fdot6(newx - self.snapped_x), diff_y) } else { MAX };
                    new_sy = self.last_y.min(round_to_fixed(newy));
                    new_sx = newx - fixed_mul(slope, newy - new_sy);
                } else {
                    new_sy = self.last_y.min(snap_y(newy));
                    new_sx = newx;
                    let diff_y = fixed_to_fdot6(new_sy - self.snapped_y);
                    slope = if diff_y != 0 { quick_div(fixed_to_fdot6(newx - self.snapped_x), diff_y) } else { MAX };
                }
                dx += self.qddx;
                dy += self.qddy;
            } else {
                newx = self.last_x;
                newy = self.last_y;
                new_sy = newy;
                new_sx = newx;
                let diff_y = fixed_to_fdot6(newy - self.snapped_y);
                slope = if diff_y != 0 { quick_div(fixed_to_fdot6(newx - self.snapped_x), diff_y) } else { MAX };
            }
            if slope < MAX {
                success = self.update_line(self.snapped_x, self.snapped_y, new_sx, new_sy, slope);
            }
            oldx = newx;
            oldy = newy;
            if !(count > 0 && !success) {
                break;
            }
        }
        self.qx = newx;
        self.qy = newy;
        self.qdx = dx;
        self.qdy = dy;
        self.snapped_x = new_sx;
        self.snapped_y = new_sy;
        self.curve_count = count;
        success
    }

    /// `SkAnalyticCubicEdge::setCubic`.
    pub(super) fn set_cubic(&mut self, pts: &[Point]) -> bool {
        let f = |v: f32| to_fdot6(v, ACCURACY);
        let (mut x0, mut y0, mut x1, mut y1, mut x2, mut y2, mut x3, mut y3) =
            (f(pts[0].x), f(pts[0].y), f(pts[1].x), f(pts[1].y), f(pts[2].x), f(pts[2].y), f(pts[3].x), f(pts[3].y));
        let mut winding = 1;
        if y0 > y3 {
            std::mem::swap(&mut x0, &mut x3);
            std::mem::swap(&mut x1, &mut x2);
            std::mem::swap(&mut y0, &mut y3);
            std::mem::swap(&mut y1, &mut y2);
            winding = -1;
        }
        if fdot6_round(y0) == fdot6_round(y3) {
            return false;
        }
        let delta = |a: i32, b: i32, c: i32, d: i32| {
            let one = ((a * 8 - b * 15 + 6 * c + d) * 19) >> 9;
            let two = ((a + 6 * b - c * 15 + d * 8) * 19) >> 9;
            one.abs().max(two.abs())
        };
        let shift = (diff_to_shift(delta(x0, x1, x2, x3), delta(y0, y1, y2, y3), 2) + 1).min(MAX_COEFF_SHIFT);
        let (mut up, mut down) = (6, shift + 6 - 10);
        if down < 0 {
            down = 0;
            up = 10 - shift;
        }
        self.winding = winding;
        self.kind = Kind::Cubic;
        self.curve_count = -1 << shift;
        self.curve_shift = shift;
        self.to_fixed_shift = down;
        let coeffs = |p0: i32, p1: i32, p2: i32, p3: i32| {
            let b = (3 * (p1 - p0)).wrapping_shl(up as u32);
            let c = (3 * (p0 - p1 - p1 + p2)).wrapping_shl(up as u32);
            let d = (p3 + 3 * (p1 - p2) - p0).wrapping_shl(up as u32);
            let first = b.wrapping_add(c >> shift).wrapping_add(d >> (2 * shift));
            let second = (2 * c as i64 + ((3 * d as i64) >> (shift - 1))) as i32;
            let third = ((3 * d as i64) >> (shift - 1)) as i32;
            (fdot6_to_fixed(p0), first, second, third, fdot6_to_fixed(p3))
        };
        (self.qx, self.qdx, self.qddx, self.qdddx, self.last_x) = coeffs(x0, x1, x2, x3);
        (self.qy, self.qdy, self.qddy, self.qdddy, self.last_y) = coeffs(y0, y1, y2, y3);
        for v in [&mut self.qx, &mut self.qy, &mut self.qdx, &mut self.qdy, &mut self.qddx, &mut self.qddy, &mut self.qdddx, &mut self.qdddy, &mut self.last_x, &mut self.last_y] {
            *v >>= ACCURACY;
        }
        self.qy = snap_y(self.qy);
        self.snapped_y = self.qy;
        self.last_y = snap_y(self.last_y);
        self.update_cubic()
    }

    /// `SkAnalyticCubicEdge::updateCubic`.
    pub(super) fn update_cubic(&mut self) -> bool {
        let mut success;
        let mut count = self.curve_count;
        let (mut oldx, mut oldy) = (self.qx, self.qy);
        let (mut newx, mut newy);
        let (ddshift, dshift) = (self.curve_shift, self.to_fixed_shift);
        loop {
            count += 1;
            if count < 0 {
                newx = oldx.wrapping_add(self.qdx >> dshift);
                self.qdx = self.qdx.wrapping_add(self.qddx >> ddshift);
                self.qddx = self.qddx.wrapping_add(self.qdddx);
                newy = oldy.wrapping_add(self.qdy >> dshift);
                self.qdy = self.qdy.wrapping_add(self.qddy >> ddshift);
                self.qddy = self.qddy.wrapping_add(self.qdddy);
            } else {
                newx = self.last_x;
                newy = self.last_y;
            }
            if newy < oldy {
                newy = oldy;
            }
            let mut new_sy = snap_y(newy);
            if self.last_y < new_sy {
                new_sy = self.last_y;
                count = 0;
            }
            let slope = if fixed_to_fdot6(new_sy - self.snapped_y) == 0 {
                MAX
            } else {
                fdot6_div(fixed_to_fdot6(newx - oldx), fixed_to_fdot6(new_sy - self.snapped_y))
            };
            success = self.update_line(oldx, self.snapped_y, newx, new_sy, slope);
            oldx = newx;
            oldy = newy;
            self.snapped_y = new_sy;
            if !(count < 0 && !success) {
                break;
            }
        }
        self.qx = newx;
        self.qy = newy;
        self.curve_count = count;
        success
    }

    /// `SkAnalyticEdge::update`: true while the edge has more of itself.
    pub(super) fn update(&mut self) -> bool {
        match self.kind {
            _ if self.curve_count < 0 => self.update_cubic(),
            _ if self.curve_count > 0 => self.update_quad(),
            _ => false,
        }
    }

    pub(super) fn keep_continuous(&mut self) {
        if self.curve_count < 0 {
            self.qx = self.x;
            self.snapped_y = self.y;
        } else {
            self.snapped_x = self.x;
            self.snapped_y = self.y;
        }
    }

    pub(super) fn go_y(&mut self, y: Fixed) {
        if y == self.y.wrapping_add(ONE) {
            self.x = sat_add(self.x, self.dx);
            self.y = y;
        } else if y != self.y {
            self.x = (self.upper_x as i64 + ((self.dx as i64 * (y as i64 - self.upper_y as i64)) >> 16)).clamp(MIN as i64, MAX as i64) as i32;
            self.y = y;
        }
    }

    pub(super) fn go_y_shift(&mut self, y: Fixed, shift: i32) {
        self.y = y;
        self.x = sat_add(self.x, self.dx >> shift);
    }
}

pub(super) fn diff_to_shift(dx: FDot6, dy: FDot6, shift_aa: i32) -> i32 {
    let (dx, dy) = (dx.abs(), dy.abs());
    let dist = if dx > dy { dx + (dy >> 1) } else { dy + (dx >> 1) };
    let dist = (dist + (1 << (2 + shift_aa))) >> (3 + shift_aa);
    ((32 - (dist as u32).leading_zeros()) >> 1) as i32
}

/// `SkEdgeBuilder::build` for an unclipped path: lines (consecutive verticals combined),
/// quadratics and cubics cut at their y extrema, each contour closed. Two sentinels lead.
pub(super) fn build_edges(path: &Path) -> Vec<Edge> {
    let mut list = vec![Edge::sentinel(MIN), Edge::sentinel(MAX)];
    let add_line = |list: &mut Vec<Edge>, a: Point, b: Point| {
        let mut e = Edge::new();
        if !e.set_line(a, b) {
            return;
        }
        let vertical = e.dx == 0 && e.kind == Kind::Line;
        if vertical && list.len() > 2 {
            let last = list.len() - 1;
            match combine_vertical(&e, &mut list[last]) {
                Combine::Total => {
                    list.pop();
                }
                Combine::Partial => {}
                Combine::No => list.push(e),
            }
        } else {
            list.push(e);
        }
    };
    let add_quad = |list: &mut Vec<Edge>, p: [Point; 3]| {
        let mut mono = [Point::zero(); 5];
        let n = chop_quad_at_y_extrema(&p, &mut mono);
        for i in 0..=n {
            let mut e = Edge::new();
            if e.set_quad(&mono[i * 2..i * 2 + 3]) {
                list.push(e);
            }
        }
    };
    let add_cubic = |list: &mut Vec<Edge>, p: [Point; 4]| {
        let mut mono = [Point::zero(); 10];
        let n = chop_cubic_at_y_extrema(&p, &mut mono);
        for i in 0..=n {
            let mut e = Edge::new();
            if e.set_cubic(&mono[i * 3..i * 3 + 4]) {
                list.push(e);
            }
        }
    };
    let pts = path.points();
    let (mut i, mut start, mut last, mut open) = (0usize, Point::zero(), Point::zero(), false);
    let mut weight = 0usize;
    for verb in path.verbs() {
        match verb {
            PathVerb::Move => {
                if open {
                    add_line(&mut list, last, start);
                }
                start = pts[i];
                last = start;
                i += 1;
                open = false;
            }
            PathVerb::Line => {
                add_line(&mut list, last, pts[i]);
                last = pts[i];
                i += 1;
                open = true;
            }
            PathVerb::Quad => {
                add_quad(&mut list, [last, pts[i], pts[i + 1]]);
                last = pts[i + 1];
                i += 2;
                open = true;
            }
            PathVerb::Cubic => {
                add_cubic(&mut list, [last, pts[i], pts[i + 1], pts[i + 2]]);
                last = pts[i + 2];
                i += 3;
                open = true;
            }
            // As `SkEdgeBuilder`: quads a quarter pixel from the conic, each cut at its y extrema.
            PathVerb::Conic => {
                let w = path.weights()[weight];
                weight += 1;
                if let Some(q) = tiny_skia_path::path_geometry::AutoConicToQuads::compute(last, pts[i], pts[i + 1], w, 0.25) {
                    for k in 0..q.len as usize {
                        add_quad(&mut list, [q.points[2 * k], q.points[2 * k + 1], q.points[2 * k + 2]]);
                    }
                }
                last = pts[i + 1];
                i += 2;
                open = true;
            }
            PathVerb::Close => {
                if open {
                    add_line(&mut list, last, start);
                }
                last = start;
                open = false;
            }
        }
    }
    if open {
        add_line(&mut list, last, start);
    }
    list
}

pub(super) enum Combine {
    No,
    Partial,
    Total,
}

/// `SkAnalyticEdgeBuilder::combineVertical` (the path may cull to the right: it isn't convex).
pub(super) fn combine_vertical(edge: &Edge, last: &mut Edge) -> Combine {
    let close = |a: Fixed, b: Fixed| (a - b).abs() < 0x100;
    if last.kind != Kind::Line || last.dx != 0 || edge.x != last.x {
        return Combine::No;
    }
    if edge.winding == last.winding {
        if edge.lower_y == last.upper_y {
            last.upper_y = edge.upper_y;
            last.y = last.upper_y;
            return Combine::Partial;
        }
        if close(edge.upper_y, last.lower_y) {
            last.lower_y = edge.lower_y;
            return Combine::Partial;
        }
        return Combine::No;
    }
    if close(edge.upper_y, last.upper_y) {
        if close(edge.lower_y, last.lower_y) {
            return Combine::Total;
        }
        if edge.lower_y < last.lower_y {
            last.upper_y = edge.lower_y;
            last.y = last.upper_y;
            return Combine::Partial;
        }
        last.upper_y = last.lower_y;
        last.y = last.upper_y;
        last.lower_y = edge.lower_y;
        last.winding = edge.winding;
        return Combine::Partial;
    }
    if close(edge.lower_y, last.lower_y) {
        if edge.upper_y > last.upper_y {
            last.lower_y = edge.upper_y;
            return Combine::Partial;
        }
        last.lower_y = last.upper_y;
        last.upper_y = edge.upper_y;
        last.y = last.upper_y;
        last.winding = edge.winding;
        return Combine::Partial;
    }
    Combine::No
}

pub(super) fn chop_quad_at_y_extrema(src: &[Point; 3], dst: &mut [Point; 5]) -> usize {
    let (a, mut b, c) = (src[0].y, src[1].y, src[2].y);
    let (ab, mut bc) = (a - b, b - c);
    if ab < 0.0 {
        bc = -bc;
    }
    if ab == 0.0 || bc < 0.0 {
        if let Some(t) = valid_unit_divide(a - b, a - b - b + c) {
            chop_quad_at(src, t, dst);
            dst[1].y = dst[2].y;
            dst[3].y = dst[2].y;
            return 1;
        }
        b = if (a - b).abs() < (b - c).abs() { a } else { c };
    }
    dst[0] = Point::from_xy(src[0].x, a);
    dst[1] = Point::from_xy(src[1].x, b);
    dst[2] = Point::from_xy(src[2].x, c);
    0
}

pub(super) fn chop_cubic_at_y_extrema(src: &[Point; 4], dst: &mut [Point; 10]) -> usize {
    let mut ts = new_t_values();
    let (a, b, c, d) = (src[0].y, src[1].y, src[2].y, src[3].y);
    let roots = find_unit_quad_roots(d - a + 3.0 * (b - c), 2.0 * (a - b - b + c), b - a, &mut ts);
    let ts = &ts[..roots];
    chop_cubic_at(src, ts, dst);
    if !ts.is_empty() {
        dst[2].y = dst[3].y;
        dst[4].y = dst[3].y;
        if ts.len() == 2 {
            dst[5].y = dst[6].y;
            dst[7].y = dst[6].y;
        }
    }
    ts.len()
}

pub(super) fn chop_cubic_at(src: &[Point; 4], ts: &[NormalizedF32Exclusive], dst: &mut [Point]) {
    if ts.is_empty() {
        dst[..4].copy_from_slice(src);
        return;
    }
    let mut t = ts[0];
    let mut tmp = *src;
    let mut off = 0;
    for i in 0..ts.len() {
        chop_cubic_at2(&tmp, t, &mut dst[off..]);
        if i == ts.len() - 1 {
            break;
        }
        off += 3;
        tmp = [dst[off], dst[off + 1], dst[off + 2], dst[off + 3]];
        match valid_unit_divide(ts[i + 1].get() - ts[i].get(), 1.0 - ts[i].get()) {
            Some(n) => t = n,
            None => {
                dst[off + 4] = tmp[3];
                dst[off + 5] = tmp[3];
                dst[off + 6] = tmp[3];
                break;
            }
        }
    }
}
