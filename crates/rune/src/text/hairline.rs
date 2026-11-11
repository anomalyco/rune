//! Icon strokes no wider than a device pixel, drawn as Chromium draws them: as a hairline, not
//! as the area of the stroke's outline. Measured against Chromium's raster of `@opencode/ui`'s
//! icons at a true 1x (a chevron, the plus, the menu, the grid and the magnifying glass, every
//! pixel within 4 of 255):
//!
//! - **Coverage** at a pixel's centre is one less its distance from the path, and no more than
//!   its distance past an open end plus half a pixel: a line covers a band two pixels wide that
//!   fades linearly from its middle, with square-ish ends that end half a pixel out. Caps and
//!   joins don't draw.
//! - **Pieces:** each straight segment is its own piece, and a run of curves is one; where pieces
//!   overlap (a corner) their coverage adds as two translucent strokes do, `a + b - ab`. A
//!   closed run of curves (a circle) has no ends.
//! - **Width:** a stroke under a pixel wide draws each piece at its width's share of that
//!   coverage before the pieces combine, so a corner of a 0.8 px box is `1 - (1 - 0.8a)(1 - 0.8b)`.
//!
//! So a 1 pt diagonal at 1x is two pixels at 65% where its outline's area gives one at 70% and
//! one at 5%, and an icon reads lighter, as Chromium's do.

use swash::zeno::{Command, Point, Transform};

/// One piece: its points in device pixels, and whether its first and last points are ends that
/// fade (a closed run of curves has none).
struct Piece {
    pts: Vec<Point>,
    ends: bool,
}

/// Curve steps per device pixel of a curve's control polygon, and the fewest.
const STEPS_PER_PX: f32 = 1.0;
const MIN_STEPS: usize = 8;

/// Draws `path` stroked `width` device pixels wide (at most one) under `ctm` into `out`, a
/// `px` × `px` coverage canvas, which it clears first.
pub(super) fn render(path: &[Command], ctm: Transform, width: f32, px: u32, out: &mut [u8]) {
    out.iter_mut().for_each(|b| *b = 0);
    let pieces = pieces(path, ctm);
    let n = px as usize;
    let mut acc = vec![0f32; n * n];
    // Each piece draws at the width's share of its coverage, then over the others.
    let w = width.clamp(0.0, 1.0);
    for piece in &pieces {
        let (lo, hi) = bounds(&piece.pts);
        let (x0, y0) = ((lo.x - 1.5).floor().max(0.0) as usize, (lo.y - 1.5).floor().max(0.0) as usize);
        let (x1, y1) = (((hi.x + 1.5).ceil().max(0.0) as usize).min(n), ((hi.y + 1.5).ceil().max(0.0) as usize).min(n));
        for y in y0..y1 {
            for x in x0..x1 {
                let c = w * coverage(piece, Point::new(x as f32 + 0.5, y as f32 + 0.5));
                if c > 0.0 {
                    let a = &mut acc[y * n + x];
                    *a = *a + c - *a * c;
                }
            }
        }
    }
    for (o, a) in out.iter_mut().zip(acc) {
        *o = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

/// Whether Chromium draws `path` under `ctm` as a rectangle rather than as a path: one closed
/// subpath of three or four axis-aligned lines round a box, under a transform that keeps it
/// axis-aligned (`SkPath::isRect`, which Skia's GPU device turns into `drawRect`). Stroked, a
/// rectangle covers its stroke's exact area, mitred corners and all, however thin: Desktop's
/// `outline-dots` (three 1 × 1 squares stroked 1 wide) are three solid 2 × 2 pixel dots at 1x,
/// where a hairline leaves their corners at 75%.
pub(super) fn draws_as_rect(path: &[Command], ctm: Transform) -> bool {
    if ctm.xy != 0.0 || ctm.yx != 0.0 {
        return false;
    }
    let [Command::MoveTo(start), lines @ .., Command::Close] = path else { return false };
    let mut pts = vec![*start];
    for cmd in lines {
        let Command::LineTo(p) = cmd else { return false };
        pts.push(*p);
    }
    if pts.len() == 5 && pts[4] == pts[0] {
        pts.pop();
    }
    if pts.len() != 4 {
        return false;
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
    axis.iter().all(Option::is_some) && (0..4).all(|i| axis[i] != axis[(i + 1) % 4])
}

fn bounds(pts: &[Point]) -> (Point, Point) {
    pts.iter().fold((Point::new(f32::MAX, f32::MAX), Point::new(f32::MIN, f32::MIN)), |(lo, hi), p| {
        (Point::new(lo.x.min(p.x), lo.y.min(p.y)), Point::new(hi.x.max(p.x), hi.y.max(p.y)))
    })
}

/// `path` under `ctm` as hairline pieces.
fn pieces(path: &[Command], ctm: Transform) -> Vec<Piece> {
    let at = |p: Point| ctm.transform_point(p);
    let mut out = Vec::new();
    let (mut cur, mut start) = (Point::new(0.0, 0.0), Point::new(0.0, 0.0));
    // The run of curves being built, and whether this subpath has drawn anything else.
    let mut run: Option<Vec<Point>> = None;
    let mut only_curves = true;
    let flush = |run: &mut Option<Vec<Point>>, out: &mut Vec<Piece>| {
        if let Some(pts) = run.take() && pts.len() > 1 {
            out.push(Piece { pts, ends: true });
        }
    };
    for cmd in path {
        match *cmd {
            Command::MoveTo(p) => {
                flush(&mut run, &mut out);
                (cur, start, only_curves) = (at(p), at(p), true);
            }
            Command::LineTo(p) => {
                flush(&mut run, &mut out);
                let p = at(p);
                out.push(Piece { pts: vec![cur, p], ends: true });
                only_curves = false;
                cur = p;
            }
            Command::CurveTo(c1, c2, p) => {
                let (c1, c2, p) = (at(c1), at(c2), at(p));
                let steps = steps(&[cur, c1, c2, p]);
                let pts = run.get_or_insert_with(|| vec![cur]);
                pts.extend((1..=steps).map(|i| cubic(cur, c1, c2, p, i as f32 / steps as f32)));
                cur = p;
            }
            Command::QuadTo(c, p) => {
                let (c, p) = (at(c), at(p));
                let steps = steps(&[cur, c, p]);
                let pts = run.get_or_insert_with(|| vec![cur]);
                pts.extend((1..=steps).map(|i| quad(cur, c, p, i as f32 / steps as f32)));
                cur = p;
            }
            Command::Close => {
                let back = (cur.x - start.x).hypot(cur.y - start.y) > 1e-3;
                match run.take() {
                    // A closed run of curves alone (a circle) has no ends.
                    Some(pts) if only_curves && !back && pts.len() > 1 => out.push(Piece { pts, ends: false }),
                    other => {
                        run = other;
                        flush(&mut run, &mut out);
                        if back { out.push(Piece { pts: vec![cur, start], ends: true }); }
                    }
                }
                cur = start;
                only_curves = true;
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

fn steps(poly: &[Point]) -> usize {
    let len: f32 = poly.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum();
    ((len * STEPS_PER_PX).ceil() as usize).max(MIN_STEPS)
}

fn cubic(p0: Point, p1: Point, p2: Point, p3: Point, t: f32) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(a * p0.x + b * p1.x + c * p2.x + d * p3.x, a * p0.y + b * p1.y + c * p2.y + d * p3.y)
}

fn quad(p0: Point, p1: Point, p2: Point, t: f32) -> Point {
    let u = 1.0 - t;
    let (a, b, c) = (u * u, 2.0 * u * t, t * t);
    Point::new(a * p0.x + b * p1.x + c * p2.x, a * p0.y + b * p1.y + c * p2.y)
}

/// Piece `piece`'s coverage at `q`: one less the distance to it, and, past or near an open end,
/// no more than the distance past that end plus half a pixel.
fn coverage(piece: &Piece, q: Point) -> f32 {
    let cl = |v: f32| v.clamp(0.0, 1.0);
    let segs = piece.pts.len() - 1;
    let total: f32 = piece.pts.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum();
    let mut best = 0f32;
    let mut run = 0f32;
    for (i, w) in piece.pts.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let len = (b.x - a.x).hypot(b.y - a.y);
        if len < 1e-6 { continue; }
        let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
        let t = (q.x - a.x) * ux + (q.y - a.y) * uy;
        // Beyond this segment's span only an open piece's first and last segments reach, along
        // their own lines (their ends fade there); elsewhere a neighbour or the joint covers it.
        let reach = (t >= 0.0 || (i == 0 && piece.ends)) && (t <= len || (i + 1 == segs && piece.ends));
        let d = if reach {
            ((q.x - a.x) * uy - (q.y - a.y) * ux).abs()
        } else {
            let p = if t < 0.0 { a } else { b };
            (q.x - p.x).hypot(q.y - p.y)
        };
        let s = run + t;
        let ends = if piece.ends { cl(s + 0.5).min(cl(total - s + 0.5)) } else { 1.0 };
        best = best.max(cl(1.0 - d).min(ends));
        run += len;
    }
    best
}
