//! Icon elements covered as Chromium covers them. Chromium rasters an inline SVG on the GPU with
//! Skia's Ganesh, which first simplifies each shape (`GrStyledShape`) and then picks an op for
//! what's left. Measured on Desktop at true 1, 1.25, 1.5, 1.875 and 2x
//! (`harness/probe-icon-coverage.ts` with every Desktop icon and `proof/.../icons/shapes`):
//!
//! - **A rectangle** (a closed axis-aligned rect path, or a straight axis-aligned line stroked
//!   with butt or square caps, which Ganesh turns into a filled one): the ramp to its nearest
//!   edge, where a ramp is half a pixel plus the distance inside, clamped, so a corner pixel is the
//!   lesser of its two edges' cover. Stroked with mitred corners, its outer rect's ramp less its
//!   inner one's. (Ganesh batches rects into one op, and a batch holding a turned quad covers all
//!   of them by the product of their edges instead: on Desktop only `appearance` has one.)
//! - **A stroked turned line**, butt or square: the ramp to its outline's nearest edge.
//! - **A rounded rect, a circle, or an axis-aligned line with round caps** (a capsule, which
//!   Ganesh turns into a round rect): the ramp of the round rect's distance; stroked, the outer
//!   ramp times the inner one's complement.
//! - **A stroke a device pixel wide or less**, of anything else: Ganesh's hairlines
//!   (`super::convex::hairline`), at the width's share of full coverage.
//! - **A closed convex path stroked with mitred or bevelled joins**, or **a convex fill** (a
//!   stroke's outline counts once it's applied): Ganesh's own geometry (`super::convex`).
//! - **Anything else** (more than one subpath, a polyline, round joins): Skia's stroker and its
//!   analytic scan converter (`super::aaa`) into a mask of the path's own, as Ganesh's small-path
//!   renderer does it, so where the icon lands doesn't change its pixels.
//!
//! One remaining difference: Chromium sends a `<circle>` whose float box isn't exactly square
//! to the path renderers rather than the circle op. SVG path arcs keep their conic weights.

use super::convex::{El, V};
use swash::zeno::{Cap, Fill, Join, Point, Transform};

/// An icon element as Chromium hands it to Skia: a path, or a shape Blink draws as itself.
pub(super) enum Shape<'a> {
    /// Skia's elements, conics and all.
    Path(&'a [El]),
    /// `<circle>`, `<ellipse>`: centre and radii, in user units.
    Oval { center: Point, radii: Point },
    /// `<rect>` with corners rounded `r`, in user units.
    Rounded { lo: Point, hi: Point, r: f32 },
}

/// How a stroke is drawn: its width in user units, caps, joins and miter limit.
pub(super) struct Pen {
    pub width: f32,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f32,
}

/// The op Chromium draws one element's fill (no `pen`) or stroke with, in device pixels.
pub(super) enum Op {
    /// A stroke at most a device pixel wide, this wide: `super::hairline`.
    Hairline(f32),
    /// An axis-aligned rect, `[min, max]`: its ramp.
    Rect { outer: [Point; 2], inner: Option<[Point; 2]> },
    /// A rect stroked `2 hw` wide with mitred corners (`AAStrokeRectOp`): the stroke's outer
    /// and inner rects (none where the stroke fills it), each with a pixel-wide ramp that the
    /// stroke's own coverage tops (less than full for a stroke under a pixel).
    StrokeRect { outside: [Point; 2], inside: Option<[Point; 2]>, hw: f32 },
    /// A turned straight stroke's outline: the ramp to its nearest edge.
    Quad([Point; 4]),
    /// A round rect's ramp, less a hole's for a stroke.
    Round { outer: Round, inner: Option<Round> },
    /// A closed convex path stroked with mitred or bevelled joins: Ganesh's convex tessellator
    /// (`super::convex`).
    ConvexStroke,
    /// Skia's stroker and analytic scan converter.
    Path,
}

/// A round rect in device pixels: `[min, max]` and its corners' radius (half its short side
/// for a circle or a capsule).
#[derive(Clone, Copy)]
pub(super) struct Round {
    lo: Point,
    hi: Point,
    r: f32,
}

/// The op for `shape` under `ctm`: filled with no `pen`, else stroked with it.
pub(super) fn op(shape: &Shape<'_>, ctm: Transform, pen: Option<&Pen>) -> Op {
    let scale = (ctm.xx * ctm.yy - ctm.xy * ctm.yx).abs().sqrt();
    let upright = ctm.xy == 0.0 && ctm.yx == 0.0 && ctm.xx.abs() == ctm.yy.abs();
    let hw = pen.map_or(0.0, |p| p.width * scale * 0.5);
    match shape {
        Shape::Oval { center, radii } => {
            if !upright || radii.x != radii.y {
                // An ellipse goes to Ganesh's path renderers as the oval path it is.
                return match pen {
                    Some(p) if linearizes(p, scale, ctm) => Op::ConvexStroke,
                    _ => thin(pen, scale),
                };
            }
            let c = ctm.transform_point(*center);
            let r = radii.x * scale;
            ring(Round { lo: Point::new(c.x - r, c.y - r), hi: Point::new(c.x + r, c.y + r), r }, pen.map(|_| hw))
        }
        Shape::Rounded { lo, hi, r } => {
            if !upright {
                return thin(pen, scale);
            }
            let (a, b) = (ctm.transform_point(*lo), ctm.transform_point(*hi));
            let (lo, hi) = (Point::new(a.x.min(b.x), a.y.min(b.y)), Point::new(a.x.max(b.x), a.y.max(b.y)));
            // Blink draws a `<rect>` as the rect it is, however its float sides add up.
            if *r == 0.0 {
                return rect(&[lo, hi], pen, hw, scale);
            }
            ring(Round { lo, hi, r: r * scale }, pen.map(|_| hw))
        }
        Shape::Path(path) => {
            if let Some(r) = rect_corners(path, ctm) {
                return rect(&r, pen, hw, scale);
            }
            match (pen, path) {
                // `AALinearizingConvexPathRenderer` takes these, a device pixel wide to twenty.
                (Some(p), _) if linearizes(p, scale, ctm) && super::convex::closed_convex(path) => Op::ConvexStroke,
                (Some(p), [El::Move(a), El::Line(b)]) => line(at(ctm, *a), at(ctm, *b), p.cap, hw, upright)
                    .unwrap_or_else(|| thin(pen, scale)),
                _ => thin(pen, scale),
            }
        }
    }
}

/// Whether `AALinearizingConvexPathRenderer` takes a closed convex path stroked with `pen`:
/// mitred or bevelled, a device pixel wide to twenty, under a similarity.
fn linearizes(pen: &Pen, scale: f32, ctm: Transform) -> bool {
    // Ganesh's hairline renderer comes first in its chain and takes a stroke a pixel wide.
    pen.join != Join::Round && pen.width * scale > 1.0 + 1.0 / 256.0 && pen.width * scale <= 20.0 && is_similarity(ctm)
}

/// Skia's elements for `shape`: an oval as `SkPath::addOval` makes it, conics and all.
pub(super) fn elements(shape: &Shape<'_>) -> Vec<super::convex::El> {
    match shape {
        Shape::Path(path) => path.to_vec(),
        Shape::Oval { center, radii } => super::convex::oval(V::new(center.x, center.y), V::new(radii.x, radii.y)),
        Shape::Rounded { .. } => Vec::new(),
    }
}

/// Whether `ctm` keeps shapes' angles and proportions (`SkMatrix::isSimilarity`).
fn is_similarity(ctm: Transform) -> bool {
    let (a, b, c, d) = (ctm.xx, ctm.xy, ctm.yx, ctm.yy);
    (a * a + b * b - (c * c + d * d)).abs() <= 1.0 / 4096.0 && (a * c + b * d).abs() <= 1.0 / 4096.0
}

/// A rect's op: filled, or stroked with mitred corners (`2 hw` wide).
fn rect(r: &[Point; 2], pen: Option<&Pen>, hw: f32, scale: f32) -> Op {
    match pen {
        None => Op::Rect { outer: *r, inner: None },
        Some(p) if p.join == Join::Miter => {
            let hollow = r[1].x - r[0].x > 2.0 * hw && r[1].y - r[0].y > 2.0 * hw;
            Op::StrokeRect { outside: grow(*r, hw), inside: hollow.then(|| grow(*r, -hw)), hw }
        }
        Some(_) => thin(pen, scale),
    }
}

/// A path's op once nothing simpler takes it: a hairline for a stroke a device pixel wide or
/// less, else Skia's scan converter.
fn thin(pen: Option<&Pen>, scale: f32) -> Op {
    match pen {
        Some(p) if p.width * scale <= 1.0 + 1.0 / 256.0 => Op::Hairline(p.width * scale),
        _ => Op::Path,
    }
}

/// A round rect filled (no `hw`) or stroked `2 hw` wide.
fn ring(shape: Round, hw: Option<f32>) -> Op {
    let Some(hw) = hw else { return Op::Round { outer: shape, inner: None } };
    let outer = Round { lo: Point::new(shape.lo.x - hw, shape.lo.y - hw), hi: Point::new(shape.hi.x + hw, shape.hi.y + hw), r: shape.r + hw };
    let inner = Round { lo: Point::new(shape.lo.x + hw, shape.lo.y + hw), hi: Point::new(shape.hi.x - hw, shape.hi.y - hw), r: (shape.r - hw).max(0.0) };
    let hollow = inner.hi.x > inner.lo.x && inner.hi.y > inner.lo.y;
    Op::Round { outer, inner: hollow.then_some(inner) }
}

/// A straight stroke from `a` to `b` (device pixels) `2 hw` wide, as Ganesh simplifies it: a
/// point with round caps is a circle and with square ones a square; an axis-aligned line is a
/// filled rect (a capsule with round caps); a turned one stroked butt or square is its quad.
/// Ganesh simplifies the line before it looks at the width, so a line a pixel wide is one of these
/// too; only one under a pixel is left to its hairline renderer.
fn line(a: Point, b: Point, cap: Cap, hw: f32, upright: bool) -> Option<Op> {
    let (ext, thin) = (if cap == Cap::Butt { 0.0 } else { hw }, hw * 2.0 < 1.0 - 1.0 / 256.0);
    let (lo, hi) = (Point::new(a.x.min(b.x), a.y.min(b.y)), Point::new(a.x.max(b.x), a.y.max(b.y)));
    if upright && (a.x == b.x || a.y == b.y) {
        let (ox, oy) = if a.y == b.y && a.x != b.x { (ext, hw) } else if a.x == b.x && a.y != b.y { (hw, ext) } else { (ext, ext) };
        let (lo, hi) = (Point::new(lo.x - ox, lo.y - oy), Point::new(hi.x + ox, hi.y + oy));
        return match cap {
            _ if hi.x <= lo.x || hi.y <= lo.y => None,
            Cap::Round => Some(Op::Round { outer: Round { lo, hi, r: hw }, inner: None }),
            _ if thin => None,
            _ => Some(Op::Rect { outer: [lo, hi], inner: None }),
        };
    }
    let len = (b.x - a.x).hypot(b.y - a.y);
    if cap == Cap::Round || thin || len < 1e-6 {
        return None;
    }
    let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
    let (a, b) = (Point::new(a.x - ux * ext, a.y - uy * ext), Point::new(b.x + ux * ext, b.y + uy * ext));
    let (nx, ny) = (-uy * hw, ux * hw);
    Some(Op::Quad([Point::new(a.x + nx, a.y + ny), Point::new(b.x + nx, b.y + ny), Point::new(b.x - nx, b.y - ny), Point::new(a.x - nx, a.y - ny)]))
}

impl Op {
    /// Covers `out` (`px` × `px`) with an op that needs no path.
    pub(super) fn render(&self, px: u32, out: &mut [u8]) {
        let n = px as usize;
        for y in 0..n {
            for x in 0..n {
                let q = Point::new(x as f32 + 0.5, y as f32 + 0.5);
                let c = match self {
                    Op::Rect { outer, inner } => ramp(inside_rect(outer, q)) - inner.as_ref().map_or(0.0, |r| ramp(inside_rect(r, q))),
                    Op::StrokeRect { outside, inside, hw } => {
                        // The four nested rects: the outer ramp from `outset` beyond the stroke to
                        // `inset` in, the inner one likewise, each a pixel across.
                        let inset = hw.min(0.5);
                        let full = if inset < 0.5 { 2.0 * inset / (inset + 0.5) } else { 1.0 };
                        let outset = 1.0 - inset;
                        let a = (inside_rect(outside, q) + outset).clamp(0.0, 1.0);
                        let b = inside.as_ref().map_or(1.0, |r| {
                            // A hole under two AA outsets across collapses its innermost rect
                            // to its centre on that axis, and Ganesh lerps the inner corners'
                            // coverage back toward the stroke's by how far the edges crossed.
                            let half = [(r[1].x - r[0].x) * 0.5, (r[1].y - r[0].y) * 0.5];
                            let backset = (1.0 - half[0].min(half[1]) / outset).max(0.0);
                            if backset == 0.0 {
                                return (outset - inside_rect(r, q)).clamp(0.0, 1.0);
                            }
                            let x = ((q.x - r[0].x).min(r[1].x - q.x) + inset) / (inset + half[0].min(outset));
                            let y = ((q.y - r[0].y).min(r[1].y - q.y) + inset) / (inset + half[1].min(outset));
                            1.0 - (1.0 - backset) * x.min(y).clamp(0.0, 1.0)
                        });
                        full * a.min(b)
                    }
                    Op::Quad(quad) => ramp(inside_quad(quad, q)),
                    Op::Round { outer, inner } => ramp(outer.inside(q)) * inner.map_or(1.0, |r| ramp(-r.inside(q))),
                    Op::Hairline(_) | Op::Path | Op::ConvexStroke => 0.0,
                };
                out[y * n + x] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
}

impl Round {
    /// How far `q` is inside the round rect's edge (negative outside).
    fn inside(&self, q: Point) -> f32 {
        let (cx, cy) = ((self.lo.x + self.hi.x) * 0.5, (self.lo.y + self.hi.y) * 0.5);
        let r = self.r.min((self.hi.x - self.lo.x) * 0.5).min((self.hi.y - self.lo.y) * 0.5);
        let dx = (q.x - cx).abs() - ((self.hi.x - self.lo.x) * 0.5 - r);
        let dy = (q.y - cy).abs() - ((self.hi.y - self.lo.y) * 0.5 - r);
        r - (dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0))
    }
}

fn ramp(inside: f32) -> f32 {
    (0.5 + inside).clamp(0.0, 1.0)
}

fn grow([lo, hi]: [Point; 2], by: f32) -> [Point; 2] {
    [Point::new(lo.x - by, lo.y - by), Point::new(hi.x + by, hi.y + by)]
}

/// How far `q` is inside the rect's nearest edge (negative outside).
fn inside_rect([lo, hi]: &[Point; 2], q: Point) -> f32 {
    (q.x - lo.x).min(hi.x - q.x).min(q.y - lo.y).min(hi.y - q.y)
}

/// How far `q` is inside the convex quad's nearest edge, either winding.
fn inside_quad(quad: &[Point; 4], q: Point) -> f32 {
    let wind = {
        let (a, b, c) = (quad[0], quad[1], quad[2]);
        ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).signum()
    };
    (0..4)
        .map(|i| {
            let (a, b) = (quad[i], quad[(i + 1) % 4]);
            let len = (b.x - a.x).hypot(b.y - a.y).max(1e-6);
            wind * ((b.x - a.x) * (q.y - a.y) - (b.y - a.y) * (q.x - a.x)) / len
        })
        .fold(f32::MAX, f32::min)
}

/// Covers `out` (`px` × `px`, cleared first) with `shape` under `ctm` as Skia's analytic scan
/// converter does (`super::aaa`): stroked first, if `pen`, as Skia strokes (in user space, its
/// curves approximated for the device scale, then mapped to device pixels).
pub(super) fn render_aaa(shape: &Shape<'_>, pen: Option<&Pen>, fill: Fill, ctm: Transform, origin: Point, px: u32, out: &mut [u8]) {
    out.iter_mut().for_each(|b| *b = 0);
    let shape_in = shape;
    let ts = tiny_skia_path::Transform::from_row(ctm.xx, ctm.xy, ctm.yx, ctm.yy, ctm.x, ctm.y);
    let res = tiny_skia_path::PathStroker::compute_resolution_scale(&ts);
    let Some(user) = skia_path(shape, 0.25 / res) else { return };
    let shape = match pen {
        Some(p) => {
            let stroke = tiny_skia_path::Stroke {
                width: p.width,
                miter_limit: p.miter_limit,
                line_cap: match p.cap {
                    Cap::Butt => tiny_skia_path::LineCap::Butt,
                    Cap::Round => tiny_skia_path::LineCap::Round,
                    Cap::Square => tiny_skia_path::LineCap::Square,
                },
                line_join: match p.join {
                    Join::Miter => tiny_skia_path::LineJoin::Miter,
                    Join::Round => tiny_skia_path::LineJoin::Round,
                    Join::Bevel => tiny_skia_path::LineJoin::Bevel,
                },
                dash: None,
            };
            user.stroke(&stroke, res)
        }
        None => Some(user),
    };
    let Some(outline) = shape else { return };
    // Filled (a stroke's outline, once it's applied), a convex path whose winding Skia knows
    // takes Ganesh's convex fill; one with no area, nothing.
    let els = match (pen, shape_in) {
        (None, Shape::Oval { .. }) => elements(shape_in),
        _ => elements_of(&outline),
    };
    if let Some(dir) = super::convex::direction(&els).filter(|&d| d != 0) {
        let placed = Transform { x: ctm.x + origin.x, y: ctm.y + origin.y, ..ctm };
        if let Some(mesh) = super::convex::fill(&els, placed, dir) {
            super::convex::fill_quad_edges(&mesh, [origin.x, origin.y], px, out);
        }
        return;
    }
    let rule = match (pen, fill) {
        (None, Fill::EvenOdd) => super::aaa::Rule::EvenOdd,
        _ => super::aaa::Rule::NonZero,
    };
    // As Ganesh's small-path renderer does it: into a mask of its own, the transform's whole
    // pixels taken out and the path's bounds a pixel in from the mask's corner. Where the path
    // ends up on the device doesn't change how its floats round.
    let (fx, fy) = (ctm.x - ctm.x.floor(), ctm.y - ctm.y.floor());
    let frac = tiny_skia_path::Transform::from_row(ctm.xx, ctm.xy, ctm.yx, ctm.yy, fx, fy);
    let b = outline.bounds();
    let corners = [(b.left(), b.top()), (b.right(), b.top()), (b.left(), b.bottom()), (b.right(), b.bottom())].map(|(x, y)| {
        let mut p = [tiny_skia_path::Point::from_xy(x, y)];
        frac.map_points(&mut p);
        p[0]
    });
    let (l, t) = corners.iter().fold((f32::MAX, f32::MAX), |(l, t), p| (l.min(p.x), t.min(p.y)));
    let (r, btm) = corners.iter().fold((f32::MIN, f32::MIN), |(r, b), p| (r.max(p.x), b.max(p.y)));
    let (dx, dy) = (l.floor(), t.floor());
    let (w, h) = ((r.ceil() - dx) as usize + 2, (btm.ceil() - dy) as usize + 2);
    let pad = 1.0;
    let Some(device) = outline.transform(frac.post_translate(pad - dx, pad - dy)) else { return };
    let mut mask = vec![0u8; w * h];
    super::aaa::fill(&device, rule, w, h, &mut mask);
    let (ox, oy) = ((ctm.x.floor() + dx - pad) as i64, (ctm.y.floor() + dy - pad) as i64);
    let n = px as i64;
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let (cx, cy) = (ox + x, oy + y);
            if (0..n).contains(&cx) && (0..n).contains(&cy) {
                out[(cy * n + cx) as usize] = mask[(y * w as i64 + x) as usize];
            }
        }
    }
}

/// `path`'s elements.
fn elements_of(path: &tiny_skia_path::Path) -> Vec<El> {
    let p = |q: tiny_skia_path::Point| V::new(q.x, q.y);
    path.segments()
        .map(|s| match s {
            tiny_skia_path::PathSegment::MoveTo(a) => El::Move(p(a)),
            tiny_skia_path::PathSegment::LineTo(a) => El::Line(p(a)),
            tiny_skia_path::PathSegment::QuadTo(a, b) => El::Quad(p(a), p(b)),
            tiny_skia_path::PathSegment::CubicTo(a, b, c) => El::Cubic(p(a), p(b), p(c)),
            tiny_skia_path::PathSegment::ConicTo(a, b, w) => El::Conic(p(a), p(b), w),
            tiny_skia_path::PathSegment::Close => El::Close,
        })
        .collect()
}

fn at(ctm: Transform, p: V) -> Point {
    ctm.transform_point(Point::new(p.x, p.y))
}

/// `shape` as Skia's path type, its conics cut into quads `tolerance` user units from them at
/// most: an oval as `SkPath::addOval` builds it (four conics), a round rect as `addRRect` does.
fn skia_path(shape: &Shape<'_>, tolerance: f32) -> Option<tiny_skia_path::Path> {
    let mut b = tiny_skia_path::PathBuilder::new();
    b.set_conic_tolerance(tolerance);
    match shape {
        Shape::Path(path) => {
            // Conics stay conics, for Skia's stroker to stroke as conics.
            b.set_keep_conics(true);
            for el in path.iter() {
                match *el {
                    El::Move(p) => b.move_to(p.x, p.y),
                    El::Line(p) => b.line_to(p.x, p.y),
                    El::Quad(c, p) => b.quad_to(c.x, c.y, p.x, p.y),
                    El::Cubic(c1, c2, p) => b.cubic_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y),
                    El::Conic(c, p, w) => b.conic_to(c.x, c.y, p.x, p.y, w),
                    El::Close => b.close(),
                }
            }
        }
        Shape::Oval { center, radii } => b.push_oval(tiny_skia_path::Rect::from_ltrb(center.x - radii.x, center.y - radii.y, center.x + radii.x, center.y + radii.y)?),
        Shape::Rounded { lo, hi, r } => push_round_rect(&mut b, *lo, *hi, *r),
    }
    b.finish()
}

/// A round rect as `SkPath::addRRect` builds it: clockwise from the top edge's left end, each
/// corner a quarter conic.
fn push_round_rect(b: &mut tiny_skia_path::PathBuilder, lo: Point, hi: Point, r: f32) {
    let r = r.min((hi.x - lo.x) * 0.5).min((hi.y - lo.y) * 0.5);
    if r <= 0.0 {
        b.move_to(lo.x, lo.y);
        b.line_to(hi.x, lo.y);
        b.line_to(hi.x, hi.y);
        b.line_to(lo.x, hi.y);
        b.close();
        return;
    }
    let w = std::f32::consts::FRAC_1_SQRT_2;
    b.move_to(lo.x + r, lo.y);
    b.line_to(hi.x - r, lo.y);
    b.conic_to(hi.x, lo.y, hi.x, lo.y + r, w);
    b.line_to(hi.x, hi.y - r);
    b.conic_to(hi.x, hi.y, hi.x - r, hi.y, w);
    b.line_to(lo.x + r, hi.y);
    b.conic_to(lo.x, hi.y, lo.x, hi.y - r, w);
    b.line_to(lo.x, lo.y + r);
    b.conic_to(lo.x, lo.y, lo.x + r, lo.y, w);
    b.close();
}

/// The rectangle `path` is under `ctm`, in device pixels `[min, max]`, if Chromium draws it as one:
/// one closed subpath of three or four axis-aligned lines round a box, under a transform that
/// keeps it axis-aligned (`SkPath::isRect`, which Ganesh draws with its rect ops).
fn rect_corners(path: &[El], ctm: Transform) -> Option<[Point; 2]> {
    if ctm.xy != 0.0 || ctm.yx != 0.0 {
        return None;
    }
    let [El::Move(start), lines @ .., El::Close] = path else { return None };
    let mut pts = vec![*start];
    for cmd in lines {
        let El::Line(p) = cmd else { return None };
        pts.push(*p);
    }
    if pts.len() == 5 && pts[4] == pts[0] {
        pts.pop();
    }
    if pts.len() != 4 {
        return None;
    }
    // Each side vertical (`Some(true)`) or horizontal, and the four alternating.
    let axis: Vec<_> = (0..4)
        .map(|i| (pts[i], pts[(i + 1) % 4]))
        .map(|(a, b)| match (a.x == b.x, a.y == b.y) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        })
        .collect();
    if !(axis.iter().all(Option::is_some) && (0..4).all(|i| axis[i] != axis[(i + 1) % 4])) {
        return None;
    }
    let d: Vec<Point> = pts.iter().map(|p| at(ctm, *p)).collect();
    let (lo, hi) = bounds(&d);
    Some([lo, hi])
}

fn bounds(pts: &[Point]) -> (Point, Point) {
    pts.iter().fold((Point::new(f32::MAX, f32::MAX), Point::new(f32::MIN, f32::MIN)), |(lo, hi), p| {
        (Point::new(lo.x.min(p.x), lo.y.min(p.y)), Point::new(hi.x.max(p.x), hi.y.max(p.y)))
    })
}
