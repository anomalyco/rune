//! The paths Chromium's GPU raster (Skia's Ganesh) draws with its own geometry rather than by
//! scan converting a mask, ported step for step: closed convex strokes (`tess`, the convex
//! tessellator), convex fills (`fill`) and hairlines (`hair`), each a mesh of triangles the GPU
//! shades (`raster`). Skia's paths keep conics, so these take Skia's path elements, not zeno's.

mod fill;
mod geom;
mod hair;
mod raster;
mod tess;

pub(super) use fill::fill;
pub(super) use geom::V;
use geom::nearly_zero;
pub(super) use hair::render as hairline;
pub(super) use raster::{fill_quad_edges, fill_triangles};
pub(super) use tess::stroke;
use swash::zeno::Transform;

/// A path element as Skia keeps it, conics and all, in user units.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum El {
    Move(V),
    Line(V),
    Quad(V, V),
    Cubic(V, V, V),
    Conic(V, V, f32),
    Close,
}

impl El {
    /// The points it adds after the one it starts from.
    fn points(&self) -> Vec<V> {
        match *self {
            El::Move(a) | El::Line(a) => vec![a],
            El::Quad(a, b) | El::Conic(a, b, _) => vec![a, b],
            El::Cubic(a, b, c) => vec![a, b, c],
            El::Close => vec![],
        }
    }
}

/// An oval as `SkPath::addOval` builds it: clockwise from its right end, four quarter conics.
pub(super) fn oval(center: V, radii: V) -> Vec<El> {
    let (l, t, r, b) = (center.x - radii.x, center.y - radii.y, center.x + radii.x, center.y + radii.y);
    let w = std::f32::consts::FRAC_1_SQRT_2;
    let (cx, cy) = (l * 0.5 + r * 0.5, t * 0.5 + b * 0.5);
    vec![
        El::Move(V::new(r, cy)),
        El::Conic(V::new(r, b), V::new(cx, b), w),
        El::Conic(V::new(l, b), V::new(l, cy), w),
        El::Conic(V::new(l, t), V::new(cx, t), w),
        El::Conic(V::new(r, t), V::new(r, cy), w),
        El::Close,
    ]
}

pub(super) fn map(ctm: Transform, p: V) -> V {
    let q = ctm.transform_point(swash::zeno::Point::new(p.x, p.y));
    V::new(q.x, q.y)
}

/// `SkAutoConicToQuads` at a quarter pixel: each quad's control and end, in pairs.
pub(super) fn conic_quads(p: [V; 3], w: f32) -> Vec<V> {
    let pt = |v: V| tiny_skia_path::Point::from_xy(v.x, v.y);
    let Some(q) = tiny_skia_path::path_geometry::AutoConicToQuads::compute(pt(p[0]), pt(p[1]), pt(p[2]), w, 0.25) else { return vec![p[1], p[2]] };
    q.points[1..1 + 2 * q.len as usize].iter().map(|a| V::new(a.x, a.y)).collect()
}


/// Whether Ganesh hands a stroke of `path` to its convex tessellator: one closed contour
/// (`SkPathPriv::IsClosedSingleContour`) that Skia's convexity test passes.
pub(super) fn closed_convex(path: &[El]) -> bool {
    let moves = path.iter().filter(|c| matches!(c, El::Move(_))).count();
    let closes = path.iter().position(|c| matches!(c, El::Close));
    moves == 1 && closes == Some(path.len() - 1) && direction(path).is_some()
}

/// `SkPathPriv::ComputeConvexity`, for a path in user units: None if it isn't convex, else its
/// winding (1 clockwise, -1 counter, 0 unknown: a line or a point).
pub(super) fn direction(path: &[El]) -> Option<i32> {
    let points: Vec<V> = path.iter().flat_map(El::points).collect();
    if concave_by_sign(&points) {
        return None;
    }
    let mut state = Convexicator::default();
    let mut contours = 0;
    let mut needs_close = false;
    for cmd in path {
        let pts: Vec<V> = match *cmd {
            El::Move(a) => {
                if contours == 1 {
                    if !state.close() {
                        return None;
                    }
                    needs_close = false;
                    contours += 1;
                } else if contours == 0 {
                    state.set_move(a);
                }
                continue;
            }
            El::Close => {
                if contours >= 2 {
                    return None;
                }
                if contours == 1 {
                    if !state.close() {
                        return None;
                    }
                    needs_close = false;
                    contours += 1;
                }
                continue;
            }
            _ => cmd.points(),
        };
        match contours {
            0 => {
                contours = 1;
                needs_close = true;
            }
            1 => {}
            _ => return None,
        }
        if !pts.into_iter().all(|q| state.add_pt(q)) {
            return None;
        }
    }
    if needs_close && !state.close() {
        return None;
    }
    (!(state.first_dir == 0 && state.reversals >= 3)).then_some(state.first_dir)
}

/// `Convexicator::IsConcaveBySign`: the direction's signs flip more than three times.
fn concave_by_sign(points: &[V]) -> bool {
    if points.len() <= 3 {
        return false;
    }
    let (mut dxes, mut dyes, mut last_sx, mut last_sy) = (0, 0, 2, 2);
    let mut cur = points[0];
    let mut step = |q: V| {
        let v = q - cur;
        if v.x != 0.0 || v.y != 0.0 {
            if !v.x.is_finite() || !v.y.is_finite() {
                return true;
            }
            let (sx, sy) = ((v.x < 0.0) as i32, (v.y < 0.0) as i32);
            dxes += (sx != last_sx) as i32;
            dyes += (sy != last_sy) as i32;
            if dxes > 3 || dyes > 3 {
                return true;
            }
            (last_sx, last_sy) = (sx, sy);
        }
        cur = q;
        false
    };
    points[1..].iter().any(|&q| step(q)) || step(points[0])
}

/// `Convexicator`, for one contour: `first_dir` 1 clockwise, -1 counter, 0 unknown.
#[derive(Default)]
struct Convexicator {
    first_pt: V,
    first_vec: V,
    last_pt: V,
    last_vec: V,
    expected: Option<i32>,
    first_dir: i32,
    reversals: i32,
}

impl Convexicator {
    fn set_move(&mut self, pt: V) {
        self.first_pt = pt;
        self.last_pt = pt;
        self.expected = None;
    }

    fn add_pt(&mut self, pt: V) -> bool {
        if self.last_pt == pt {
            return true;
        }
        if self.first_pt == self.last_pt && self.expected.is_none() && self.last_vec == V::new(0.0, 0.0) {
            self.last_vec = pt - self.last_pt;
            self.first_vec = self.last_vec;
        } else if !self.add_vec(pt - self.last_pt) {
            return false;
        }
        self.last_pt = pt;
        true
    }

    fn close(&mut self) -> bool {
        let (first, vec) = (self.first_pt, self.first_vec);
        self.add_pt(first) && self.add_vec(vec)
    }

    fn add_vec(&mut self, v: V) -> bool {
        let cross = self.last_vec.cross(v);
        if !cross.is_finite() {
            return false;
        }
        if cross == 0.0 {
            if self.last_vec.dot(v) < 0.0 {
                self.last_vec = v;
                self.reversals += 1;
                return self.reversals < 3;
            }
            return true;
        }
        let dir = if cross > 0.0 { 1 } else { -1 };
        match self.expected {
            None => {
                self.expected = Some(dir);
                self.first_dir = dir;
            }
            Some(e) if e != dir => {
                self.first_dir = 0;
                return false;
            }
            Some(_) => {}
        }
        self.last_vec = v;
        true
    }
}

/// Skia's elements for SVG path data: arcs as the conics `SkPath::arcTo` makes, the rest as is.
pub(super) fn parse(d: &str) -> Vec<El> {
    let mut out = Elements(Vec::new());
    let _ = super::svg_path::parse_into(d, &mut out);
    out.0
}

struct Elements(Vec<El>);

fn v(p: swash::zeno::Point) -> V {
    V::new(p.x, p.y)
}

impl super::svg_path::Sink for Elements {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn move_to(&mut self, p: swash::zeno::Point) {
        self.0.push(El::Move(v(p)));
    }
    fn line_to(&mut self, p: swash::zeno::Point) {
        self.0.push(El::Line(v(p)));
    }
    fn quad_to(&mut self, c: swash::zeno::Point, p: swash::zeno::Point) {
        self.0.push(El::Quad(v(c), v(p)));
    }
    fn curve_to(&mut self, c1: swash::zeno::Point, c2: swash::zeno::Point, p: swash::zeno::Point) {
        self.0.push(El::Cubic(v(c1), v(c2), v(p)));
    }
    fn arc_to(&mut self, from: swash::zeno::Point, rx: f32, ry: f32, angle: f32, large: bool, sweep: bool, to: swash::zeno::Point) {
        arc_to(&mut self.0, v(from), rx, ry, angle, large, sweep, v(to));
    }
    fn close(&mut self) {
        self.0.push(El::Close);
    }
}

/// `SkPath::arcTo` for an SVG arc (`sweep` is SVG's sweep flag: clockwise on screen), as conics.
#[allow(clippy::too_many_arguments)]
fn arc_to(out: &mut Vec<El>, from: V, rx: f32, ry: f32, angle: f32, large: bool, sweep: bool, to: V) {
    use std::f32::consts::PI;
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let mid = (from - to) * 0.5;
    let rotate = |deg: f32, p: V| {
        let (s, c) = rotation(deg);
        V::new(c * p.x - s * p.y, s * p.x + c * p.y)
    };
    let m = rotate(-angle, mid);
    let (sq_rx, sq_ry, sq_x, sq_y) = (rx * rx, ry * ry, m.x * m.x, m.y * m.y);
    let radii_scale = sq_x / sq_rx + sq_y / sq_ry;
    if radii_scale > 1.0 {
        let s = radii_scale.sqrt();
        rx *= s;
        ry *= s;
    }
    // setScale(1 / rx, 1 / ry), preRotate(-angle): rotated, then scaled.
    let to_unit = |p: V| {
        let r = rotate(-angle, p);
        V::new(r.x * (1.0 / rx), r.y * (1.0 / ry))
    };
    let (mut u0, mut u1) = (to_unit(from), to_unit(to));
    let mut delta = u1 - u0;
    let d = delta.x * delta.x + delta.y * delta.y;
    let mut scale = (1.0 / d - 0.25).max(0.0).sqrt();
    if !sweep != large {
        scale = -scale;
    }
    delta = delta * scale;
    let mut center = (u0 + u1) * 0.5;
    center = V::new(center.x - delta.y, center.y + delta.x);
    u0 = u0 - center;
    u1 = u1 - center;
    let (theta1, theta2) = (u0.y.atan2(u0.x), u1.y.atan2(u1.x));
    let mut arc = theta2 - theta1;
    if arc < 0.0 && sweep {
        arc += PI * 2.0;
    } else if arc > 0.0 && !sweep {
        arc -= PI * 2.0;
    }
    if arc.abs() < PI / (1000.0 * 1000.0) {
        out.push(El::Line(to));
        return;
    }
    // setRotate(angle), preScale(rx, ry): scaled, then rotated.
    let from_unit = |p: V| rotate(angle, V::new(p.x * rx, p.y * ry));
    let segments = (arc / (2.0 * PI / 3.0)).abs().ceil() as i32;
    let width = arc / segments as f32;
    let t = (0.5 * width).tan();
    if !t.is_finite() {
        return;
    }
    let w = (0.5 + width.cos() * 0.5).sqrt();
    let is_int = |s: f32| s == s.floor();
    let integers = nearly_zero(PI / 2.0 - width.abs()) && is_int(rx) && is_int(ry) && is_int(to.x) && is_int(to.y);
    let mut start = theta1;
    for _ in 0..segments {
        let end = start + width;
        let (sin_end, cos_end) = (snap_to_zero(end.sin()), snap_to_zero(end.cos()));
        let end_pt = V::new(cos_end, sin_end) + center;
        let ctrl = V::new(end_pt.x + t * sin_end, end_pt.y - t * cos_end);
        let (mut c, mut e) = (from_unit(ctrl), from_unit(end_pt));
        if integers {
            c = V::new(c.x.round(), c.y.round());
            e = V::new(e.x.round(), e.y.round());
        }
        out.push(El::Conic(c, e, w));
        start = end;
    }
    // The final point is the arc's end by definition.
    if let Some(El::Conic(_, e, _)) = out.last_mut() {
        *e = to;
    }
}

/// `SkMatrix::setRotate`'s sine and cosine, each snapped to zero within 1/4096.
fn rotation(deg: f32) -> (f32, f32) {
    let rad = deg * (std::f32::consts::PI / 180.0);
    (snap_to_zero(rad.sin()), snap_to_zero(rad.cos()))
}

fn snap_to_zero(x: f32) -> f32 {
    if nearly_zero(x) { 0.0 } else { x }
}
