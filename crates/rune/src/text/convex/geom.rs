//! The float steps Ganesh's convex tessellator takes, as Skia takes them: points
//! (`SkPoint::setLength` scales in double and rounds once), Wang's formula for how many points a
//! curve may flatten into, and `GrPathUtils`' recursive flattening.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(in super::super) struct V {
    pub(in super::super) x: f32,
    pub(in super::super) y: f32,
}

impl V {
    pub(in super::super) fn new(x: f32, y: f32) -> Self {
        V { x, y }
    }

    pub(super) fn dot(self, o: V) -> f32 {
        self.x * o.x + self.y * o.y
    }

    pub(super) fn cross(self, o: V) -> f32 {
        self.x * o.y - self.y * o.x
    }

    pub(super) fn distance_sqd(self, o: V) -> f32 {
        let d = self - o;
        d.dot(d)
    }

    /// `SkPointPriv::MakeOrthog`: `side` 1 (right) turns `(x, y)` to `(-y, x)`, -1 to `(y, -x)`.
    pub(super) fn orthog(self, side: i32) -> V {
        if side == 1 { V::new(-self.y, self.x) } else { V::new(self.y, -self.x) }
    }

    /// `SkPoint::setLength`: false (and zero) if it can't be.
    pub(super) fn set_length(&mut self, length: f32) -> bool {
        let (xx, yy) = (self.x as f64, self.y as f64);
        let scale = length as f64 / (xx * xx + yy * yy).sqrt();
        let (x, y) = ((xx * scale) as f32, (yy * scale) as f32);
        if !x.is_finite() || !y.is_finite() || (x == 0.0 && y == 0.0) {
            *self = V::new(0.0, 0.0);
            return false;
        }
        *self = V::new(x, y);
        true
    }

    pub(super) fn normalize(&mut self) -> bool {
        self.set_length(1.0)
    }
}

impl Add for V {
    type Output = V;
    fn add(self, o: V) -> V {
        V::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for V {
    type Output = V;
    fn sub(self, o: V) -> V {
        V::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for V {
    type Output = V;
    fn mul(self, s: f32) -> V {
        V::new(self.x * s, self.y * s)
    }
}

impl Neg for V {
    type Output = V;
    fn neg(self) -> V {
        V::new(-self.x, -self.y)
    }
}

/// `SkScalarNearlyZero`, `SkScalarNearlyEqual`: within 1/4096.
pub(super) fn nearly_zero(x: f32) -> bool {
    x.abs() <= 1.0 / 4096.0
}

pub(super) fn nearly_equal(a: f32, b: f32) -> bool {
    nearly_zero(a - b)
}

fn midpoint(a: V, b: V) -> V {
    V::new(((a.x as f64 + b.x as f64) * 0.5) as f32, ((a.y as f64 + b.y as f64) * 0.5) as f32)
}

/// Wang's formula's `nextlog2`: ceil(log2(x)) for x > 1, else 0.
fn nextlog2(x: f32) -> u32 {
    if x <= 1.0 || x.is_nan() {
        return 0;
    }
    let bits = x.to_bits().wrapping_add((1 << 23) - 1);
    (((bits >> 23) & 0xFF) as i32 - 127).max(0) as u32
}

fn nextlog16(x: f32) -> u32 {
    (nextlog2(x) + 3) >> 2
}

pub(super) fn quadratic_log2(precision: f32, p: [V; 3]) -> u32 {
    let v = p[1] * -2.0 + p[0] + p[2];
    nextlog16((v.x * v.x + v.y * v.y) * (0.0625 * (precision * precision)))
}

pub(super) fn cubic_log2(precision: f32, p: [V; 4]) -> u32 {
    let a = p[1] * -2.0 + p[0] + p[2];
    let b = p[2] * -2.0 + p[1] + p[3];
    nextlog16((a.x * a.x + a.y * a.y).max(b.x * b.x + b.y * b.y) * (0.5625 * (precision * precision)))
}

/// `max_bezier_vertices`.
pub(super) fn max_vertices(log2: u32) -> u32 {
    1 << log2.min(10)
}

/// `SkPointPriv::DistanceToLineSegmentBetweenSqd`.
pub(super) fn distance_to_segment_sqd(pt: V, a: V, b: V) -> f32 {
    let (u, v) = (b - a, pt - a);
    let u_len = u.dot(u);
    let u_dot_v = u.dot(v);
    if u_dot_v <= 0.0 {
        return v.dot(v);
    }
    if u_dot_v > u_len {
        return b.distance_sqd(pt);
    }
    let det = u.cross(v);
    let t = det / u_len * det;
    if t.is_finite() { t } else { v.dot(v) }
}

/// `GrPathUtils::generateQuadraticPoints`: the points after `p0`.
pub(super) fn quadratic_points(p0: V, p1: V, p2: V, tol_sqd: f32, out: &mut Vec<V>, left: u32) {
    if left < 2 || distance_to_segment_sqd(p1, p0, p2) < tol_sqd {
        out.push(p2);
        return;
    }
    let q = [midpoint(p0, p1), midpoint(p1, p2)];
    let r = midpoint(q[0], q[1]);
    quadratic_points(p0, q[0], r, tol_sqd, out, left >> 1);
    quadratic_points(r, q[1], p2, tol_sqd, out, left >> 1);
}

/// `GrPathUtils::generateCubicPoints`: the points after `p0`.
pub(super) fn cubic_points(p0: V, p1: V, p2: V, p3: V, tol_sqd: f32, out: &mut Vec<V>, left: u32) {
    if left < 2 || (distance_to_segment_sqd(p1, p0, p3) < tol_sqd && distance_to_segment_sqd(p2, p0, p3) < tol_sqd) {
        out.push(p3);
        return;
    }
    let q = [midpoint(p0, p1), midpoint(p1, p2), midpoint(p2, p3)];
    let r = [midpoint(q[0], q[1]), midpoint(q[1], q[2])];
    let s = midpoint(r[0], r[1]);
    cubic_points(p0, q[0], r[0], s, tol_sqd, out, left >> 1);
    cubic_points(s, r[1], q[2], p3, tol_sqd, out, left >> 1);
}
