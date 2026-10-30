//! What charts share: a series palette that themes with [`DiagramStyle`], text colour on a fill,
//! smaller and bolder type, an S-curve between two points, and a horizontal axis with grid lines.

use rune::{Align, Color, TextStyle, oklch};

use super::DiagramStyle;
use super::canvas::{Canvas, Measured};

/// Series hues, spread so neighbours never look alike.
const HUES: [f32; 12] = [255.0, 25.0, 150.0, 70.0, 305.0, 195.0, 350.0, 110.0, 280.0, 170.0, 45.0, 225.0];

pub(crate) fn dark(paint: &DiagramStyle) -> bool {
    paint.surface.to_oklab()[0] < 0.5
}

/// The hue of series `i`.
pub(crate) fn hue(i: usize) -> f32 {
    HUES[i % HUES.len()]
}

/// A series' solid colour: pie slices, lines, swatches.
pub(crate) fn solid(paint: &DiagramStyle, h: f32) -> Color {
    if dark(paint) { oklch(0.62, 0.12, h) } else { oklch(0.72, 0.12, h) }
}

/// A series' quiet fill, for bars that carry text and bands behind rows.
pub(crate) fn soft(paint: &DiagramStyle, h: f32) -> Color {
    if dark(paint) { oklch(0.38, 0.07, h) } else { oklch(0.9, 0.05, h) }
}

/// A series' outline, for [`soft`] fills.
pub(crate) fn strong(paint: &DiagramStyle, h: f32) -> Color {
    if dark(paint) { oklch(0.7, 0.12, h) } else { oklch(0.56, 0.13, h) }
}

/// Text that reads on `fill`.
pub(crate) fn ink(fill: Color) -> Color {
    if fill.to_oklab()[0] > 0.6 { oklch(0.22, 0.01, 260.0) } else { oklch(0.97, 0.005, 260.0) }
}

/// The diagram's type a point smaller: axis ticks, legends, notes.
pub(crate) fn small(paint: &DiagramStyle) -> TextStyle {
    TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font }
}

pub(crate) fn bold(font: TextStyle) -> TextStyle {
    TextStyle { weight: 600.0, ..font }
}

/// A curve from `a` to `b` that leaves and arrives level (or upright when `upright`).
pub(crate) fn s_curve(a: [f32; 2], b: [f32; 2], upright: bool) -> Vec<[f32; 2]> {
    let (c1, c2) = if upright {
        let m = (a[1] + b[1]) / 2.0;
        ([a[0], m], [b[0], m])
    } else {
        let m = (a[0] + b[0]) / 2.0;
        ([m, a[1]], [m, b[1]])
    };
    (0..=20)
        .map(|i| {
            let t = i as f32 / 20.0;
            let u = 1.0 - t;
            let (k0, k1, k2, k3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            [k0 * a[0] + k1 * c1[0] + k2 * c2[0] + k3 * b[0], k0 * a[1] + k1 * c1[1] + k2 * c2[1] + k3 * b[1]]
        })
        .collect()
}

/// Grid lines from `top` to `bottom` at each tick's x, between `x0` and `x1`, with its label
/// centred under `bottom` and kept inside `[x0, x1]` (one that would overlap the last is dropped).
pub(crate) fn axis_x(c: &mut Canvas, ticks: &[(f32, Measured)], [x0, x1]: [f32; 2], [top, bottom]: [f32; 2], font: TextStyle, paint: &DiagramStyle) {
    let grid = paint.text.alpha(0.08);
    c.stroke(vec![[x0, bottom], [x1, bottom]], false, 1.0, paint.surface_border);
    let mut right = f32::MIN;
    for (x, m) in ticks {
        if *x < x0 - 0.5 || *x > x1 + 0.5 {
            continue;
        }
        c.stroke(vec![[*x, top], [*x, bottom]], false, 1.0, grid);
        c.stroke(vec![[*x, bottom], [*x, bottom + 4.0]], false, 1.0, paint.surface_border);
        let left = (x - m.size[0] / 2.0).clamp(x0, (x1 - m.size[0]).max(x0));
        if left < right + 8.0 {
            continue;
        }
        c.label_at(m, [left, bottom + 6.0], font, paint.muted, Align::Start);
        right = left + m.size[0];
    }
}

#[cfg(test)]
#[path = "chart_tests.rs"]
mod tests;
