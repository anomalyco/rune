//! Lines, outlines and arrowheads made opaque before they are drawn.
//!
//! Rune draws a stroke as one capsule per segment, so a translucent stroke blends twice at every
//! joint: a curve gets beads, a rounded corner gets brighter than the edges beside it, and a line
//! gets brighter where it runs under its own arrowhead or meets a node's outline. Each
//! translucent ink colour is replaced with what it would show over the fills beneath it (the
//! card's surface, a subgraph's tint, a node's fill), so overlaps and joints paint the same
//! colour twice and look like one stroke. Area fills (tints, bands, slices) keep their alpha.
//!
//! The blend is in sRGB-encoded values, as apps blend by default (`App::srgb_blend`).

use rune::Color;

use super::Op;
use super::hover::Trace;

/// `ops` with their ink made opaque over `surface`: every stroke, and every fill in a stroke's
/// colour (arrowheads and other filled heads). The hover `traces` draw over `ops`; their ink is
/// flattened over the same fills.
pub(crate) fn flatten(ops: &mut [Op], traces: &mut [Trace], surface: Color) {
    let mut inks: Vec<Color> = Vec::new();
    for op in ops.iter().chain(traces.iter().flat_map(|t| &t.ops)) {
        if let Op::Stroke { color, .. } = op
            && translucent(*color)
            && !inks.contains(color)
        {
            inks.push(*color);
        }
    }
    if inks.is_empty() {
        return;
    }
    let areas: Vec<Area> = ops
        .iter()
        .enumerate()
        .filter_map(|(i, op)| match op {
            Op::Fill(points, color) if !inks.contains(color) => Some(Area { at: i, bounds: bounds(points), points: points.clone(), color: *color }),
            _ => None,
        })
        .collect();
    let backdrop = Backdrop { areas, surface };
    let n = ops.len();
    for (i, op) in ops.iter_mut().enumerate() {
        backdrop.flatten(op, i, &inks);
    }
    for op in traces.iter_mut().flat_map(|t| &mut t.ops) {
        backdrop.flatten(op, n, &inks);
    }
}

struct Area {
    at: usize,
    bounds: [f32; 4],
    points: Vec<[f32; 2]>,
    color: Color,
}

struct Backdrop {
    areas: Vec<Area>,
    surface: Color,
}

impl Backdrop {
    fn flatten(&self, op: &mut Op, at: usize, inks: &[Color]) {
        match op {
            Op::Stroke { points, closed, color, .. } if translucent(*color) => {
                let (p, own) = if *closed { (centroid(points), Some(&points[..])) } else { (midpoint(points), None) };
                *color = over(*color, self.under(p, at, own));
            }
            Op::Fill(points, color) if inks.contains(color) => *color = over(*color, self.under(centroid(points), at, None)),
            _ => {}
        }
    }

    /// The colour at `p` from the fills drawn before op `at`, over the surface; `own` is the
    /// outline being drawn, whose own fill doesn't count.
    fn under(&self, p: [f32; 2], at: usize, own: Option<&[[f32; 2]]>) -> Color {
        self.areas
            .iter()
            .take_while(|a| a.at < at)
            .filter(|a| own != Some(&a.points[..]) && inside(p, a.bounds, &a.points))
            .fold(self.surface, |under, a| over(a.color, under))
    }
}

/// Ink that shows what's under it; clear ink stays clear.
fn translucent(c: Color) -> bool {
    c.a > 0.0 && c.a < 1.0
}

/// `top` over `bottom`, blended in sRGB-encoded values.
pub(crate) fn over(top: Color, bottom: Color) -> Color {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 1e-6 {
        return Color::TRANSPARENT;
    }
    let m = |t: f32, b: f32| dec((enc(t) * top.a + enc(b) * bottom.a * (1.0 - top.a)) / a);
    Color { r: m(top.r, bottom.r), g: m(top.g, bottom.g), b: m(top.b, bottom.b), a }
}

fn enc(c: f32) -> f32 {
    rune::color::srgb_encode(c.clamp(0.0, 1.0))
}

fn dec(c: f32) -> f32 {
    rune::color::srgb_decode(c.clamp(0.0, 1.0))
}

fn bounds(points: &[[f32; 2]]) -> [f32; 4] {
    points.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])])
}

fn centroid(points: &[[f32; 2]]) -> [f32; 2] {
    let n = points.len().max(1) as f32;
    let s = points.iter().fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
    [s[0] / n, s[1] / n]
}

fn midpoint(points: &[[f32; 2]]) -> [f32; 2] {
    let i = points.len() / 2;
    match points.len() {
        0 => [0.0, 0.0],
        n if n % 2 == 0 => crate::canvas::lerp(points[i - 1], points[i], 0.5),
        _ => points[i],
    }
}

/// Whether `p` is inside the polygon `points` (even-odd).
fn inside(p: [f32; 2], b: [f32; 4], points: &[[f32; 2]]) -> bool {
    if p[0] < b[0] || p[0] > b[2] || p[1] < b[1] || p[1] > b[3] {
        return false;
    }
    let mut odd = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (a, c) = (points[i], points[j]);
        if (a[1] > p[1]) != (c[1] > p[1]) && p[0] < (c[0] - a[0]) * (p[1] - a[1]) / (c[1] - a[1]) + a[0] {
            odd = !odd;
        }
        j = i;
    }
    odd
}

#[cfg(test)]
#[path = "ink_tests.rs"]
mod tests;
