//! SVG path data (`d`) to path segments, following the SVG 2 path grammar. Replaces zeno's own
//! string parser, which reflects a stale control point when `S`/`T` follows a non-curve and
//! reads neither exponents nor zero-radius arcs.
//!
//! Like browsers, a syntax error ends the path: everything before it still renders.

use swash::zeno::Point;
#[cfg(test)]
use swash::zeno::{Angle, ArcSize, ArcSweep, Command, PathBuilder};

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')) {
            self.i += 1;
        }
    }

    fn skip_comma_ws(&mut self) {
        self.skip_ws();
        if self.peek() == Some(b',') {
            self.i += 1;
            self.skip_ws();
        }
    }

    fn at_number(&self) -> bool {
        matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'-' | b'+'))
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        self.i - start
    }

    /// `sign? (digits ('.' digits?)? | '.' digits) exponent?`, then an optional separator.
    fn number(&mut self) -> Option<f32> {
        let start = self.i;
        if matches!(self.peek(), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut mantissa = self.digits();
        if self.peek() == Some(b'.') {
            self.i += 1;
            mantissa += self.digits();
        }
        if mantissa == 0 {
            self.i = start;
            return None;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mark = self.i;
            self.i += 1;
            if matches!(self.peek(), Some(b'-' | b'+')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                self.i = mark;
            }
        }
        let v = blink_number(&self.s[start..self.i]);
        self.skip_comma_ws();
        v.is_finite().then_some(v)
    }

    /// Arc flags are one character each, so `a1 1 0 011 1` packs both flags and an x.
    fn flag(&mut self) -> Option<bool> {
        let v = match self.peek()? {
            b'0' => false,
            b'1' => true,
            _ => return None,
        };
        self.i += 1;
        self.skip_comma_ws();
        Some(v)
    }

    fn pair(&mut self) -> Option<Point> {
        Some(Point::new(self.number()?, self.number()?))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Curve {
    Cubic,
    Quad,
}

fn reflect(about: Point, of: Point) -> Point {
    Point::new(2.0 * about.x - of.x, 2.0 * about.y - of.y)
}

/// zeno's commands for `d` (the parser's own tests read them).
#[cfg(test)]
pub(crate) fn parse(d: &str) -> Vec<Command> {
    let mut out = Vec::new();
    let _ = parse_into(d, &mut out);
    out
}

/// Where parsed segments go: zeno's commands (an arc as cubics), or Skia's elements (an arc as
/// the conics `SkPath::arcTo` makes).
pub(crate) trait Sink {
    fn is_empty(&self) -> bool;
    fn move_to(&mut self, p: Point);
    fn line_to(&mut self, p: Point);
    fn quad_to(&mut self, c: Point, p: Point);
    fn curve_to(&mut self, c1: Point, c2: Point, p: Point);
    /// An arc from `from` to `to` (both radii non-zero, the ends apart).
    #[allow(clippy::too_many_arguments)]
    fn arc_to(&mut self, from: Point, rx: f32, ry: f32, angle: f32, large: bool, sweep: bool, to: Point);
    fn close(&mut self);
}

#[cfg(test)]
impl Sink for Vec<Command> {
    fn is_empty(&self) -> bool {
        <[Command]>::is_empty(self)
    }
    fn move_to(&mut self, p: Point) {
        self.push(Command::MoveTo(p));
    }
    fn line_to(&mut self, p: Point) {
        self.push(Command::LineTo(p));
    }
    fn quad_to(&mut self, c: Point, p: Point) {
        self.push(Command::QuadTo(c, p));
    }
    fn curve_to(&mut self, c1: Point, c2: Point, p: Point) {
        self.push(Command::CurveTo(c1, c2, p));
    }
    fn arc_to(&mut self, _from: Point, rx: f32, ry: f32, angle: f32, large: bool, sweep: bool, to: Point) {
        let size = if large { ArcSize::Large } else { ArcSize::Small };
        let sweep = if sweep { ArcSweep::Positive } else { ArcSweep::Negative };
        PathBuilder::arc_to(self, rx, ry, Angle::from_degrees(angle), size, sweep, to);
        // Land exactly on the endpoint so following relative segments don't drift.
        if let Some(Command::CurveTo(_, _, end)) = self.last_mut() {
            *end = to;
        }
    }
    fn close(&mut self) {
        self.push(Command::Close);
    }
}

pub(crate) fn parse_into(d: &str, out: &mut impl Sink) -> Option<()> {
    let mut p = Lexer { s: d.as_bytes(), i: 0 };
    let (mut cur, mut start) = (Point::ZERO, Point::ZERO);
    // The previous segment's second control point, for `S` (after C/S) and `T` (after Q/T).
    let mut control: Option<(Curve, Point)> = None;
    let mut cmd = 0u8;
    p.skip_ws();
    while let Some(c) = p.peek() {
        if c.is_ascii_alphabetic() {
            if out.is_empty() && !matches!(c, b'M' | b'm') {
                return None;
            }
            cmd = c;
            p.i += 1;
            p.skip_ws();
        } else if !p.at_number() || matches!(cmd, 0 | b'Z' | b'z') {
            return None;
        }
        let rel = cmd.is_ascii_lowercase();
        let origin = if rel { cur } else { Point::ZERO };
        let mut next_control = None;
        match cmd.to_ascii_uppercase() {
            b'M' => {
                cur = p.pair()? + origin;
                start = cur;
                out.move_to(cur);
                // Further coordinate pairs after a moveto are implicit linetos.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'Z' => {
                out.close();
                cur = start;
            }
            b'L' => {
                cur = p.pair()? + origin;
                out.line_to(cur);
            }
            b'H' => {
                cur.x = p.number()? + origin.x;
                out.line_to(cur);
            }
            b'V' => {
                cur.y = p.number()? + origin.y;
                out.line_to(cur);
            }
            b'C' => {
                let (c1, c2, to) = (p.pair()? + origin, p.pair()? + origin, p.pair()? + origin);
                out.curve_to(c1, c2, to);
                next_control = Some((Curve::Cubic, c2));
                cur = to;
            }
            b'S' => {
                let c1 = match control {
                    Some((Curve::Cubic, c)) => reflect(cur, c),
                    _ => cur,
                };
                let (c2, to) = (p.pair()? + origin, p.pair()? + origin);
                out.curve_to(c1, c2, to);
                next_control = Some((Curve::Cubic, c2));
                cur = to;
            }
            b'Q' => {
                let (c, to) = (p.pair()? + origin, p.pair()? + origin);
                out.quad_to(c, to);
                next_control = Some((Curve::Quad, c));
                cur = to;
            }
            b'T' => {
                let c = match control {
                    Some((Curve::Quad, c)) => reflect(cur, c),
                    _ => cur,
                };
                let to = p.pair()? + origin;
                out.quad_to(c, to);
                next_control = Some((Curve::Quad, c));
                cur = to;
            }
            b'A' => {
                let (rx, ry, angle) = (p.number()?, p.number()?, p.number()?);
                let (large, sweep) = (p.flag()?, p.flag()?);
                let to = p.pair()? + origin;
                if rx == 0.0 || ry == 0.0 {
                    out.line_to(to);
                } else if to != cur {
                    out.arc_to(cur, rx, ry, angle, large, sweep, to);
                }
                cur = to;
            }
            _ => return None,
        }
        control = next_control;
        p.skip_comma_ws();
    }
    Some(())
}

/// A number's value as Blink's `GenericParseNumber` works it out in `float`: the integer digits
/// summed from the right, each decimal digit times a running power of 0.1, so `11.7778` can land
/// an ulp or two from the nearest float. Chromium strokes those points, and an ulp is enough to
/// tip an edge onto the next quarter pixel.
fn blink_number(s: &[u8]) -> f32 {
    let (sign, s) = match s.first() {
        Some(b'-') => (-1.0, &s[1..]),
        Some(b'+') => (1.0, &s[1..]),
        _ => (1.0, s),
    };
    let e = s.iter().position(|c| matches!(c, b'e' | b'E')).unwrap_or(s.len());
    let (mantissa, exponent) = s.split_at(e);
    let dot = mantissa.iter().position(|&c| c == b'.').unwrap_or(mantissa.len());
    let integer = mantissa[..dot].iter().rev().fold((0.0f32, 1.0f32), |(sum, mul), c| (sum + mul * f32::from(c - b'0'), mul * 10.0)).0;
    let decimal = mantissa.get(dot + 1..).unwrap_or(&[]).iter().fold((0.0f32, 1.0f32), |(sum, frac), c| {
        let frac = frac * 0.1;
        (sum + f32::from(c - b'0') * frac, frac)
    });
    let number = (integer + decimal.0) * sign;
    let Some(exp) = exponent.get(1..).and_then(|e| std::str::from_utf8(e).ok()).and_then(|e| e.parse::<i32>().ok()) else { return number };
    if exp == 0 { number } else { number * 10f64.powi(exp) as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Command::*;

    fn pt(x: f32, y: f32) -> Point {
        Point::new(x, y)
    }

    fn near(a: &[Command], b: &[Command]) -> bool {
        let close = |p: Point, q: Point| (p.x - q.x).abs() < 1e-4 && (p.y - q.y).abs() < 1e-4;
        a.len() == b.len()
            && a.iter().zip(b).all(|pair| match pair {
                (MoveTo(p), MoveTo(q)) | (LineTo(p), LineTo(q)) => close(*p, *q),
                (QuadTo(p, p2), QuadTo(q, q2)) => close(*p, *q) && close(*p2, *q2),
                (CurveTo(p, p2, p3), CurveTo(q, q2, q3)) => close(*p, *q) && close(*p2, *q2) && close(*p3, *q3),
                (Close, Close) => true,
                _ => false,
            })
    }

    #[track_caller]
    fn check(d: &str, want: &[Command]) {
        let got = parse(d);
        assert!(near(&got, want), "{d:?}\n got {got:?}\nwant {want:?}");
    }

    #[test]
    fn smooth_cubic_after_a_non_curve_starts_at_the_current_point() {
        // SVG 2 §9.3.6: with no preceding C/S, the first control point is the current point.
        check("M1 8s2.5-4.5 7-4.5", &[MoveTo(pt(1.0, 8.0)), CurveTo(pt(1.0, 8.0), pt(3.5, 3.5), pt(8.0, 3.5))]);
        check(
            "M0 0L4 0S6 2 8 0",
            &[MoveTo(pt(0.0, 0.0)), LineTo(pt(4.0, 0.0)), CurveTo(pt(4.0, 0.0), pt(6.0, 2.0), pt(8.0, 0.0))],
        );
        // A quadratic doesn't seed a cubic reflection, and vice versa.
        check(
            "M0 0Q2 2 4 0S6 2 8 0",
            &[
                MoveTo(pt(0.0, 0.0)),
                QuadTo(pt(2.0, 2.0), pt(4.0, 0.0)),
                CurveTo(pt(4.0, 0.0), pt(6.0, 2.0), pt(8.0, 0.0)),
            ],
        );
        check(
            "M0 0C1 1 2 1 3 0T6 0",
            &[
                MoveTo(pt(0.0, 0.0)),
                CurveTo(pt(1.0, 1.0), pt(2.0, 1.0), pt(3.0, 0.0)),
                QuadTo(pt(3.0, 0.0), pt(6.0, 0.0)),
            ],
        );
    }

    #[test]
    fn smooth_curves_reflect_the_previous_control_point() {
        check(
            "M0 0C1 2 3 2 4 0S7-2 8 0",
            &[
                MoveTo(pt(0.0, 0.0)),
                CurveTo(pt(1.0, 2.0), pt(3.0, 2.0), pt(4.0, 0.0)),
                CurveTo(pt(5.0, -2.0), pt(7.0, -2.0), pt(8.0, 0.0)),
            ],
        );
        // Implicitly repeated `s` reflects the previous `s`, in relative coordinates.
        check(
            "M0 0c1 2 3 2 4 0s3-2 4 0 3 2 4 0",
            &[
                MoveTo(pt(0.0, 0.0)),
                CurveTo(pt(1.0, 2.0), pt(3.0, 2.0), pt(4.0, 0.0)),
                CurveTo(pt(5.0, -2.0), pt(7.0, -2.0), pt(8.0, 0.0)),
                CurveTo(pt(9.0, 2.0), pt(11.0, 2.0), pt(12.0, 0.0)),
            ],
        );
        check(
            "M0 0Q2 2 4 0T8 0t4 0",
            &[
                MoveTo(pt(0.0, 0.0)),
                QuadTo(pt(2.0, 2.0), pt(4.0, 0.0)),
                QuadTo(pt(6.0, -2.0), pt(8.0, 0.0)),
                QuadTo(pt(10.0, 2.0), pt(12.0, 0.0)),
            ],
        );
    }

    #[test]
    fn arc_flags_may_be_packed_without_separators() {
        let spaced = parse("M2 4a2 2 0 0 1 4 0");
        let packed = parse("M2 4a2 2 0 014 0");
        let commas = parse("M2,4a2,2,0,0,1,4,0");
        assert!(spaced.len() > 1);
        assert!(near(&spaced, &packed), "{packed:?}");
        assert!(near(&spaced, &commas), "{commas:?}");
        let Some(CurveTo(_, _, end)) = packed.last() else { panic!("{packed:?}") };
        assert_eq!(*end, pt(6.0, 4.0));
        // Sweep 1 from (2,4) to (6,4) about (4,4) passes through the top, y = 2.
        let top = packed.iter().any(|c| matches!(c, CurveTo(_, _, p) if (p.y - 2.0).abs() < 1e-3));
        assert!(top, "{packed:?}");
        // `a1 1 0 0 1 1 1` written as `a1 1 0 011 1`, then an implicit second arc.
        assert!(near(&parse("M0 0a1 1 0 0 1 1 1 1 1 0 0 1 1 1"), &parse("M0 0a1 1 0 011 1 1 1 0 011 1")));
    }

    #[test]
    fn degenerate_arcs_follow_the_spec() {
        // Zero radius is a straight line; a zero-length arc is omitted.
        check("M0 0A0 5 0 0 1 4 4", &[MoveTo(pt(0.0, 0.0)), LineTo(pt(4.0, 4.0))]);
        check("M1 1a3 3 0 0 1 0 0L2 2", &[MoveTo(pt(1.0, 1.0)), LineTo(pt(2.0, 2.0))]);
        // Radii too small for the chord scale up to a half ellipse instead of failing.
        let half = parse("M0 0A1 1 0 0 1 10 0");
        assert!(half.iter().any(|c| matches!(c, CurveTo(_, _, p) if (p.y + 5.0).abs() < 1e-3)), "{half:?}");
    }

    #[test]
    fn implicit_repeats_and_moveto_linetos() {
        check("M1 1 2 2 3 3", &[MoveTo(pt(1.0, 1.0)), LineTo(pt(2.0, 2.0)), LineTo(pt(3.0, 3.0))]);
        check("m1 1 2 2 3 3", &[MoveTo(pt(1.0, 1.0)), LineTo(pt(3.0, 3.0)), LineTo(pt(6.0, 6.0))]);
        check(
            "M0 0h1 2v3-1",
            &[
                MoveTo(pt(0.0, 0.0)),
                LineTo(pt(1.0, 0.0)),
                LineTo(pt(3.0, 0.0)),
                LineTo(pt(3.0, 3.0)),
                LineTo(pt(3.0, 2.0)),
            ],
        );
        check("M0 0H5 2V1", &[MoveTo(pt(0.0, 0.0)), LineTo(pt(5.0, 0.0)), LineTo(pt(2.0, 0.0)), LineTo(pt(2.0, 1.0))]);
    }

    #[test]
    fn relative_segments_follow_the_current_point_and_close() {
        check(
            "M10 10l5 0L20 20l-5 5zl1 1",
            &[
                MoveTo(pt(10.0, 10.0)),
                LineTo(pt(15.0, 10.0)),
                LineTo(pt(20.0, 20.0)),
                LineTo(pt(15.0, 25.0)),
                Close,
                LineTo(pt(11.0, 11.0)),
            ],
        );
        check("M1 1m2 2 1 0", &[MoveTo(pt(1.0, 1.0)), MoveTo(pt(3.0, 3.0)), LineTo(pt(4.0, 3.0))]);
        check("M1 1zm2 2", &[MoveTo(pt(1.0, 1.0)), Close, MoveTo(pt(3.0, 3.0))]);
    }

    #[test]
    fn numbers_split_on_signs_dots_and_exponents() {
        check("M2.3.4L-1-2", &[MoveTo(pt(2.3, 0.4)), LineTo(pt(-1.0, -2.0))]);
        check("M1e1 2E-1L+.5-.5e1", &[MoveTo(pt(10.0, 0.2)), LineTo(pt(0.5, -5.0))]);
        check(" \n M 1 , 2 L 3,4 ", &[MoveTo(pt(1.0, 2.0)), LineTo(pt(3.0, 4.0))]);
    }

    #[test]
    fn errors_keep_what_came_before() {
        check("M1 1L2 2L3", &[MoveTo(pt(1.0, 1.0)), LineTo(pt(2.0, 2.0))]);
        check("M1 1L2 2X4 4", &[MoveTo(pt(1.0, 1.0)), LineTo(pt(2.0, 2.0))]);
        check("L1 1", &[]);
        check("M0 0z1 1", &[MoveTo(pt(0.0, 0.0)), Close]);
    }
}
