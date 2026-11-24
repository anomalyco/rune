//! SVG 2 path data (`d`) lexer, parser, and arc-to-cubic conversion.
//!
//! Follows the W3C SVG 2 Path Data grammar and Blink/Chromium error handling (stops at the first
//! syntax error or non-finite number while keeping all valid commands parsed before it). Shared
//! between `rune-core` (`Path::from_svg`) and `rune-text` (monochrome icon rasterization) so
//! there is a single parser across Rune.

use std::f64::consts::{FRAC_PI_2, TAU};

use super::Path;

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

    fn pair(&mut self) -> Option<[f32; 2]> {
        Some([self.number()?, self.number()?])
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Curve {
    Cubic,
    Quad,
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn reflect(about: [f32; 2], of: [f32; 2]) -> [f32; 2] {
    [2.0 * about[0] - of[0], 2.0 * about[1] - of[1]]
}

/// Receiver for parsed SVG 2 path data commands.
///
/// Implemented internally by `Path` (converting arcs to cubics and flattening) and by
/// `rune-text`'s icon pipeline (converting arcs to Skia conics).
#[doc(hidden)]
pub trait SvgPathSink {
    fn is_empty(&self) -> bool;
    fn move_to(&mut self, p: [f32; 2]);
    fn line_to(&mut self, p: [f32; 2]);
    fn quad_to(&mut self, c: [f32; 2], p: [f32; 2]);
    fn curve_to(&mut self, c1: [f32; 2], c2: [f32; 2], p: [f32; 2]);
    /// An arc from `from` to `to` (both radii non-zero, the ends apart).
    #[allow(clippy::too_many_arguments)]
    fn arc_to(
        &mut self,
        from: [f32; 2],
        rx: f32,
        ry: f32,
        angle: f32,
        large: bool,
        sweep: bool,
        to: [f32; 2],
    );
    fn close(&mut self);
}

/// Parses SVG 2 path data `d` into `out`, returning `Some(())` if the entire string was valid
/// or `None` after stopping at the first syntax error (preserving commands emitted before it).
#[doc(hidden)]
pub fn parse_svg_path(d: &str, out: &mut impl SvgPathSink) -> Option<()> {
    let mut p = Lexer { s: d.as_bytes(), i: 0 };
    let (mut cur, mut start) = ([0.0f32; 2], [0.0f32; 2]);
    // The previous segment's second control point, for `S` (after C/S) and `T` (after Q/T).
    let mut control: Option<(Curve, [f32; 2])> = None;
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
        let origin = if rel { cur } else { [0.0, 0.0] };
        let mut next_control = None;
        match cmd.to_ascii_uppercase() {
            b'M' => {
                cur = add(p.pair()?, origin);
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
                cur = add(p.pair()?, origin);
                out.line_to(cur);
            }
            b'H' => {
                cur[0] = p.number()? + origin[0];
                out.line_to(cur);
            }
            b'V' => {
                cur[1] = p.number()? + origin[1];
                out.line_to(cur);
            }
            b'C' => {
                let (c1, c2, to) = (
                    add(p.pair()?, origin),
                    add(p.pair()?, origin),
                    add(p.pair()?, origin),
                );
                out.curve_to(c1, c2, to);
                next_control = Some((Curve::Cubic, c2));
                cur = to;
            }
            b'S' => {
                let c1 = match control {
                    Some((Curve::Cubic, c)) => reflect(cur, c),
                    _ => cur,
                };
                let (c2, to) = (add(p.pair()?, origin), add(p.pair()?, origin));
                out.curve_to(c1, c2, to);
                next_control = Some((Curve::Cubic, c2));
                cur = to;
            }
            b'Q' => {
                let (c, to) = (add(p.pair()?, origin), add(p.pair()?, origin));
                out.quad_to(c, to);
                next_control = Some((Curve::Quad, c));
                cur = to;
            }
            b'T' => {
                let c = match control {
                    Some((Curve::Quad, c)) => reflect(cur, c),
                    _ => cur,
                };
                let to = add(p.pair()?, origin);
                out.quad_to(c, to);
                next_control = Some((Curve::Quad, c));
                cur = to;
            }
            b'A' => {
                let (rx, ry, angle) = (p.number()?, p.number()?, p.number()?);
                let (large, sweep) = (p.flag()?, p.flag()?);
                let to = add(p.pair()?, origin);
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
    let int_digits = &mantissa[..dot];
    let first_nonzero = int_digits.iter().position(|&c| c != b'0').unwrap_or(int_digits.len());
    let integer = int_digits[first_nonzero..]
        .iter()
        .rev()
        .fold((0.0f32, 1.0f32), |(sum, mul), c| (sum + mul * f32::from(c - b'0'), mul * 10.0))
        .0;
    let decimal = mantissa.get(dot + 1..).unwrap_or(&[]).iter().fold((0.0f32, 1.0f32), |(sum, frac), c| {
        let frac = frac * 0.1;
        (sum + f32::from(c - b'0') * frac, frac)
    });
    let number = (integer + decimal.0) * sign;
    let Some(exp_bytes) = exponent.get(1..) else { return number };
    let exp = match std::str::from_utf8(exp_bytes).ok().and_then(|e| e.parse::<i32>().ok()) {
        Some(exp) => exp,
        None => {
            return if exp_bytes.first() == Some(&b'-') {
                0.0 * sign
            } else {
                f32::INFINITY * sign
            };
        }
    };
    if exp == 0 || number == 0.0 {
        number
    } else {
        number * 10f64.powi(exp) as f32
    }
}

/// Parses SVG 2 path data into a flattened [`Path`].
pub(super) fn parse_into_path(d: &str) -> Path {
    let mut sink = PathSink {
        path: Path::new(),
        subpath_start: [0.0, 0.0],
        after_close: false,
    };
    let _ = parse_svg_path(d, &mut sink);
    sink.path
}

struct PathSink {
    path: Path,
    subpath_start: [f32; 2],
    after_close: bool,
}

impl PathSink {
    /// If a drawing command follows `Z` without a new `M`, SVG 2 starts a new subpath from the
    /// closed subpath's initial point (not its final vertex).
    fn ensure_open(&mut self) {
        if self.after_close {
            self.path = std::mem::take(&mut self.path).move_to(self.subpath_start);
            self.after_close = false;
        }
    }
}

impl SvgPathSink for PathSink {
    fn is_empty(&self) -> bool {
        self.path.subpaths.is_empty()
    }

    fn move_to(&mut self, p: [f32; 2]) {
        self.path = std::mem::take(&mut self.path).move_to(p);
        self.subpath_start = p;
        self.after_close = false;
    }

    fn line_to(&mut self, p: [f32; 2]) {
        self.ensure_open();
        self.path = std::mem::take(&mut self.path).line_to(p);
    }

    fn quad_to(&mut self, c: [f32; 2], p: [f32; 2]) {
        self.ensure_open();
        self.path = std::mem::take(&mut self.path).quad_to(c, p);
    }

    fn curve_to(&mut self, c1: [f32; 2], c2: [f32; 2], p: [f32; 2]) {
        self.ensure_open();
        self.path = std::mem::take(&mut self.path).cubic_to(c1, c2, p);
    }

    fn arc_to(
        &mut self,
        from: [f32; 2],
        rx: f32,
        ry: f32,
        angle: f32,
        large: bool,
        sweep: bool,
        to: [f32; 2],
    ) {
        self.ensure_open();
        arc_to_cubics(&mut self.path, from, rx, ry, angle, large, sweep, to);
    }

    fn close(&mut self) {
        self.path = std::mem::take(&mut self.path).close();
        self.after_close = true;
    }
}

/// Converts an SVG 2 elliptical arc (endpoint parameterization) into one or more cubic Bézier
/// segments of at most 90° each, following W3C SVG 2 Appendix B.2.4 / B.2.5, and appends them
/// to `path`.
#[allow(clippy::too_many_arguments)]
fn arc_to_cubics(
    path: &mut Path,
    from: [f32; 2],
    rx: f32,
    ry: f32,
    angle_deg: f32,
    large_arc: bool,
    sweep: bool,
    to: [f32; 2],
) {
    let mut rx = f64::from(rx).abs();
    let mut ry = f64::from(ry).abs();
    if !rx.is_finite() || !ry.is_finite() || rx == 0.0 || ry == 0.0 {
        *path = std::mem::take(path).line_to(to);
        return;
    }

    let phi = f64::from(angle_deg).to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();

    // B.2.4 step 1: rotate half-chord into ellipse frame.
    let dx2 = (f64::from(from[0]) - f64::from(to[0])) * 0.5;
    let dy2 = (f64::from(from[1]) - f64::from(to[1])) * 0.5;
    let x1p = cos_phi * dx2 + sin_phi * dy2;
    let y1p = -sin_phi * dx2 + cos_phi * dy2;

    // B.2.5: scale radii up when too small to reach between endpoints.
    let mut rx2 = rx * rx;
    let mut ry2 = ry * ry;
    if rx2 == 0.0 || ry2 == 0.0 {
        *path = std::mem::take(path).line_to(to);
        return;
    }
    let x1p2 = x1p * x1p;
    let y1p2 = y1p * y1p;
    let lambda = x1p2 / rx2 + y1p2 / ry2;
    if !lambda.is_finite() {
        *path = std::mem::take(path).line_to(to);
        return;
    }
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
        rx2 = rx * rx;
        ry2 = ry * ry;
    }

    // B.2.4 step 2: center in rotated frame.
    let denom = rx2 * y1p2 + ry2 * x1p2;
    let prod = rx2 * ry2;
    if !prod.is_finite() || denom <= 0.0 || !denom.is_finite() {
        *path = std::mem::take(path).line_to(to);
        return;
    }
    let num = (prod - denom).max(0.0);
    let coef = (num / denom).sqrt() * if large_arc == sweep { -1.0 } else { 1.0 };
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * -(ry * x1p / rx);

    // B.2.4 step 3: center in original frame.
    let mid_x = (f64::from(from[0]) + f64::from(to[0])) * 0.5;
    let mid_y = (f64::from(from[1]) + f64::from(to[1])) * 0.5;
    let cx = cos_phi * cxp - sin_phi * cyp + mid_x;
    let cy = sin_phi * cxp + cos_phi * cyp + mid_y;
    let max = f64::from(f32::MAX) * 0.25;
    if !cx.is_finite() || !cy.is_finite() || rx > max || ry > max || cx.abs() > max || cy.abs() > max {
        *path = std::mem::take(path).line_to(to);
        return;
    }

    // B.2.4 step 4: start angle and sweep angle on the unit circle.
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = uy.atan2(ux);
    let mut dtheta = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += TAU;
    }
    if dtheta.abs() < 1e-9 || !dtheta.is_finite() {
        *path = std::mem::take(path).line_to(to);
        return;
    }

    // Split into <= 90-degree cubic segments.
    let segs = (dtheta.abs() / FRAC_PI_2).ceil().max(1.0) as usize;
    let delta = dtheta / segs as f64;
    let alpha = (4.0 / 3.0) * (delta * 0.25).tan();
    let map = |x: f64, y: f64| -> [f32; 2] {
        [
            (cx + cos_phi * rx * x - sin_phi * ry * y) as f32,
            (cy + sin_phi * rx * x + cos_phi * ry * y) as f32,
        ]
    };

    for i in 0..segs {
        let t0 = theta1 + i as f64 * delta;
        let t1 = t0 + delta;
        let (s0, c0) = t0.sin_cos();
        let (s1, c1) = t1.sin_cos();
        let cp1 = map(c0 - alpha * s0, s0 + alpha * c0);
        let cp2 = map(c1 + alpha * s1, s1 - alpha * c1);
        let end = if i + 1 == segs { to } else { map(c1, s1) };
        if cp1[0].is_finite()
            && cp1[1].is_finite()
            && cp2[0].is_finite()
            && cp2[1].is_finite()
            && end[0].is_finite()
            && end[1].is_finite()
        {
            *path = std::mem::take(path).cubic_to(cp1, cp2, end);
        } else {
            *path = std::mem::take(path).line_to(to);
            return;
        }
    }
}

#[cfg(test)]
#[path = "svg_path_tests.rs"]
mod tests;
