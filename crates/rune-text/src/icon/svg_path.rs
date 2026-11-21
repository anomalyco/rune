//! SVG path data (`d`) to path segments, following the SVG 2 path grammar. Replaces zeno's own
//! string parser, which reflects a stale control point when `S`/`T` follows a non-curve and
//! reads neither exponents nor zero-radius arcs.
//!
//! Like browsers, a syntax error ends the path: everything before it still renders.

use swash::zeno::Point;

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

/// Where parsed segments go: Skia's elements, an arc as the conics `SkPath::arcTo` makes
/// (`super::convex::parse`).
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
