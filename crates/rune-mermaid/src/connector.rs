//! Edges on layered diagrams: each routed through [`Ctx`], broken where its label sits, with its
//! heads at both ends and a hover trace.

use rune::{Align, Color, TextStyle};

use super::canvas::{Canvas, Measured, arrowhead, diamond_head, ellipse, lerp, rounded_rect, smooth, unit};
use super::layered_draw::{Edge, Head, Line};
use super::route::Ctx;
use super::{DiagramStyle, adapt_stroke};

const ARROW: f32 = 9.0;

/// The room a label takes on its edge: its text with a little air, where the line breaks off.
pub(crate) fn label_box(m: &Measured) -> [f32; 2] {
    [m.size[0] + 12.0, m.size[1] + 6.0]
}

/// Where a subgraph's title sits, with a little air, for a subgraph whose top left is `p0`.
pub(crate) fn title_box(m: &Measured, p0: [f32; 2]) -> [f32; 4] {
    [p0[0] + 6.0, p0[1] + 3.0, p0[0] + 14.0 + m.size[0], p0[1] + 11.0 + m.size[1]]
}

/// How far a head of kind `h` takes its end of the line back.
fn inset(h: Head) -> f32 {
    match h {
        Head::None => 0.0,
        Head::Arrow => ARROW * 0.7,
        Head::Circle => 8.0,
        Head::Cross => 4.0,
        Head::OpenTriangle => 10.0,
        Head::DiamondFilled | Head::DiamondOpen => 12.0,
    }
}

/// The colour and width `e` draws in.
fn pen(e: &Edge, paint: &DiagramStyle) -> (Color, f32) {
    let width = e.style.width.unwrap_or(match e.line {
        Line::Thick => 2.5,
        _ => 1.25,
    });
    let color = e.style.stroke.map_or(paint.line, |col| adapt_stroke(paint.line, col, paint.node_fill.to_oklab()[0] < 0.5));
    (color, width)
}

/// Every edge routed through `ctx`, with its label in a gap in the line (on a pill of `backs[i]`
/// where it would cover a subgraph's border); each edge's drawing is kept as a trace so hovering
/// either node lights it up.
#[allow(clippy::too_many_arguments)]
pub(crate) fn edges(c: &mut Canvas, ctx: &Ctx, edges: &[Edge], labels: &[Option<Measured>], backs: &[Color], flip: bool, font: TextStyle, paint: &DiagramStyle) {
    for (i, e) in edges.iter().enumerate() {
        let start = c.ops.len();
        let (color, _) = pen(e, paint);
        if ctx.placed.paths[i].len() <= 1 {
            let s = labels[i].as_ref().map_or(0.0, |m| label_box(m)[0]);
            let at = self_loop(c, e, ctx.screen(ctx.placed.centers[e.from]), ctx.sizes[e.from], paint);
            if let Some(m) = &labels[i] {
                c.label(m, [at[0] + s / 2.0, at[1]], font, paint.text, Align::Center);
            }
        } else {
            let r = ctx.route(i, flip, [inset(e.start), inset(e.end)]);
            let gap = labels[i].as_ref().zip(r.label).map(|(m, at)| {
                let s = label_box(m);
                [at[0] - s[0] / 2.0, at[1] - s[1] / 2.0, at[0] + s[0] / 2.0, at[1] + s[1] / 2.0]
            });
            let runs = gap.map_or_else(|| vec![r.curve.clone()], |g| outside(&r.curve, g));
            connector(c, &runs, [(r.tip_start, r.dir_start), (r.tip_end, r.dir_end)], e, paint);
            if let (Some(m), Some(at), Some(g)) = (&labels[i], r.label, gap) {
                // A label left on a subgraph's border hides it, as the line does around it.
                if ctx.on_border(g) {
                    c.fill(rounded_rect(g[0], g[1], g[2], g[3], 4.0), backs.get(i).copied().unwrap_or(paint.surface));
                }
                c.label(m, at, font, paint.text, Align::Center);
            }
        }
        c.trace(e.from, e.to, start, color);
    }
}

/// The runs of `curve` outside `rect` (`[x0, y0, x1, y1]`), cut where they cross its edge.
pub(crate) fn outside(curve: &[[f32; 2]], rect: [f32; 4]) -> Vec<Vec<[f32; 2]>> {
    let mut runs: Vec<Vec<[f32; 2]>> = vec![Vec::new()];
    for w in curve.windows(2) {
        let (a, b) = (w[0], w[1]);
        let run = runs.last_mut().unwrap();
        if run.is_empty() {
            run.push(a);
        }
        match inside_span(a, b, rect) {
            None => run.push(b),
            Some((t0, t1)) => {
                if t0 > 0.0 {
                    run.push(lerp(a, b, t0));
                }
                let mut next = Vec::new();
                if t1 < 1.0 {
                    next.extend([lerp(a, b, t1), b]);
                }
                runs.push(next);
            }
        }
    }
    runs.retain(|r| r.len() >= 2);
    runs
}

/// The part of `a`→`b` inside `rect`, as a range of `t` (Liang–Barsky).
fn inside_span(a: [f32; 2], b: [f32; 2], rect: [f32; 4]) -> Option<(f32, f32)> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-d[0], a[0] - rect[0]), (d[0], rect[2] - a[0]), (-d[1], a[1] - rect[1]), (d[1], rect[3] - a[1])] {
        if p.abs() < 1e-9 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 { t0 = t0.max(r) } else { t1 = t1.min(r) }
        }
    }
    (t0 < t1).then_some((t0, t1))
}

/// Draws `e` along `runs` (its curve, broken where a label sits) with its heads at `tips`
/// (`[start, end]`, each a tip and the unit direction it points).
pub(crate) fn connector(c: &mut Canvas, runs: &[Vec<[f32; 2]>], tips: [([f32; 2], [f32; 2]); 2], e: &Edge, paint: &DiagramStyle) {
    let (color, width) = pen(e, paint);
    for run in runs {
        match e.line {
            Line::Invisible => return,
            Line::Dotted => c.dashed(run, width, color, 3.0, 3.0),
            _ => c.stroke(run.clone(), false, width, color),
        }
    }
    head(c, e.end, tips[1].0, tips[1].1, width, color, paint);
    head(c, e.start, tips[0].0, tips[0].1, width, color, paint);
}

fn head(c: &mut Canvas, h: Head, tip: [f32; 2], d: [f32; 2], width: f32, color: Color, paint: &DiagramStyle) {
    let back = |by: f32| [tip[0] - d[0] * by, tip[1] - d[1] * by];
    match h {
        Head::None => {}
        Head::Arrow => c.fill(arrowhead(tip, d, ARROW + width, 6.5 + width * 1.6), color),
        Head::OpenTriangle => c.shape(arrowhead(tip, d, 10.0, 9.0), paint.node_fill, color, width),
        Head::DiamondFilled => c.fill(diamond_head(tip, d, 12.0, 8.0), color),
        Head::DiamondOpen => c.shape(diamond_head(tip, d, 12.0, 8.0), paint.node_fill, color, width),
        Head::Circle => c.shape(ellipse(back(4.0), 4.0, 4.0), paint.node_fill, color, width),
        Head::Cross => {
            let m = back(4.0);
            let (a, b) = ([d[0] + d[1], d[1] - d[0]], [d[0] - d[1], d[1] + d[0]]);
            let k = 3.5 / std::f32::consts::SQRT_2;
            c.stroke(vec![[m[0] - a[0] * k, m[1] - a[1] * k], [m[0] + a[0] * k, m[1] + a[1] * k]], false, width, color);
            c.stroke(vec![[m[0] - b[0] * k, m[1] - b[1] * k], [m[0] + b[0] * k, m[1] + b[1] * k]], false, width, color);
        }
    }
}

/// A loop off the node's right side; returns the left middle of where its label goes.
pub(crate) fn self_loop(c: &mut Canvas, e: &Edge, ctr: [f32; 2], size: [f32; 2], paint: &DiagramStyle) -> [f32; 2] {
    let x = ctr[0] + size[0] / 2.0;
    let (a, b) = ([x, ctr[1] - size[1] / 4.0], [x, ctr[1] + size[1] / 4.0]);
    let mut path = [a, [x + 18.0, a[1] - 4.0], [x + 26.0, ctr[1]], [x + 18.0, b[1] + 4.0], b];
    let n = path.len();
    let (dir_end, dir_start) = (unit(path[n - 2], path[n - 1]), unit(path[1], path[0]));
    let (tip_end, tip_start) = (path[n - 1], path[0]);
    path[n - 1] = [tip_end[0] - dir_end[0] * inset(e.end), tip_end[1] - dir_end[1] * inset(e.end)];
    path[0] = [tip_start[0] - dir_start[0] * inset(e.start), tip_start[1] - dir_start[1] * inset(e.start)];
    connector(c, &[smooth(&path)], [(tip_start, dir_start), (tip_end, dir_end)], e, paint);
    [x + 30.0, ctr[1]]
}
