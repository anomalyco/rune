//! Flowchart node geometry, outlines, connectors and layered scene drawing.

use rune::{Align, TextStyle};

use super::flowchart::{Dir, Edge, Flowchart, Head, Line, NodeShape};
use super::layered::{GCluster, GEdge, Graph, Spacing, layout};
use super::route::Ctx;
use super::{Canvas, DiagramStyle, Measured, Scene, arc, arrowhead, ellipse, fit, measure, room, rounded_rect, smooth, unit};

const PAD: [f32; 2] = [30.0, 22.0];
const SPACING: Spacing = Spacing { node: 36.0, rank: 44.0, cluster: 14.0 };

struct Sized {
    label: Measured,
    size: [f32; 2],
}

pub(crate) fn node_size(shape: NodeShape, t: [f32; 2]) -> [f32; 2] {
    let [w, h] = [t[0] + PAD[0], (t[1] + PAD[1]).max(40.0)];
    match shape {
        NodeShape::Rect | NodeShape::Round => [w.max(60.0), h],
        NodeShape::Stadium | NodeShape::Hexagon => [w + h / 2.0, h],
        NodeShape::Subroutine => [w + 16.0, h],
        NodeShape::Asymmetric => [w + h / 3.0, h],
        NodeShape::Cylinder => [w.max(60.0), h + 16.0],
        NodeShape::Parallelogram | NodeShape::Trapezoid => [w + h * 0.6, h],
        NodeShape::Circle => {
            let d = (t[0] * t[0] + t[1] * t[1]).sqrt() + 20.0;
            [d.max(48.0), d.max(48.0)]
        }
        NodeShape::Diamond => {
            let a = t[0] / 2.0 + t[1] + 12.0;
            let b = t[1] / 2.0 * a / (t[1] + 12.0) + 10.0;
            [2.0 * a, (2.0 * b).max(40.0)]
        }
    }
}

/// Where the line from `c` toward `toward` leaves a node of `shape` and `size` centred at `c`.
pub(crate) fn boundary(shape: NodeShape, c: [f32; 2], size: [f32; 2], toward: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (toward[0] - c[0], toward[1] - c[1]);
    if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
        return c;
    }
    let (a, b) = (size[0] / 2.0, size[1] / 2.0);
    let t = match shape {
        NodeShape::Circle => 1.0 / ((dx / a).powi(2) + (dy / b).powi(2)).sqrt(),
        NodeShape::Diamond => 1.0 / (dx.abs() / a + dy.abs() / b),
        _ => {
            let tx = if dx.abs() > 1e-3 { a / dx.abs() } else { f32::MAX };
            let ty = if dy.abs() > 1e-3 { b / dy.abs() } else { f32::MAX };
            tx.min(ty)
        }
    };
    let t = t.min(1.0);
    [c[0] + dx * t, c[1] + dy * t]
}

pub(crate) fn outline(shape: NodeShape, c: [f32; 2], s: [f32; 2]) -> Vec<[f32; 2]> {
    let (x0, y0, x1, y1) = (c[0] - s[0] / 2.0, c[1] - s[1] / 2.0, c[0] + s[0] / 2.0, c[1] + s[1] / 2.0);
    let h = s[1];
    match shape {
        NodeShape::Rect | NodeShape::Subroutine => rounded_rect(x0, y0, x1, y1, 3.0),
        NodeShape::Round => rounded_rect(x0, y0, x1, y1, 10.0),
        NodeShape::Stadium => rounded_rect(x0, y0, x1, y1, h / 2.0),
        NodeShape::Circle => ellipse(c, s[0] / 2.0, s[1] / 2.0),
        NodeShape::Diamond => vec![[c[0], y0], [x1, c[1]], [c[0], y1], [x0, c[1]]],
        NodeShape::Hexagon => {
            let i = h / 4.0;
            vec![[x0 + i, y0], [x1 - i, y0], [x1, c[1]], [x1 - i, y1], [x0 + i, y1], [x0, c[1]]]
        }
        NodeShape::Asymmetric => vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0 + h / 3.0, c[1]]],
        NodeShape::Parallelogram => {
            let k = h * 0.3;
            vec![[x0 + k, y0], [x1, y0], [x1 - k, y1], [x0, y1]]
        }
        NodeShape::Trapezoid => {
            let k = h * 0.3;
            vec![[x0 + k, y0], [x1 - k, y0], [x1, y1], [x0, y1]]
        }
        NodeShape::Cylinder => {
            let ry = 8.0;
            let rx = s[0] / 2.0;
            let mut pts = arc([c[0], y0 + ry], rx, ry, 180.0, 360.0, 20);
            pts.extend(arc([c[0], y1 - ry], rx, ry, 0.0, 180.0, 20));
            pts
        }
    }
}

pub(crate) fn draw(f: &Flowchart, paint: &DiagramStyle) -> Option<Scene> {
    let canvas = draw_canvas(f, paint)?;
    let scale = fit(canvas.width(), paint);
    canvas.finish(scale)
}

/// `f` drawn at full size, before [`Canvas::finish`].
pub(crate) fn draw_canvas(f: &Flowchart, paint: &DiagramStyle) -> Option<Canvas> {
    let nodes: Vec<Sized> = f
        .nodes
        .iter()
        .map(|n| {
            let label = measure(&n.label, paint.font);
            Sized { size: node_size(n.shape, label.size), label }
        })
        .collect();
    let edge_font = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
    let labels: Vec<Option<Measured>> = f.edges.iter().map(|e| e.label.as_ref().map(|l| measure(l, edge_font))).collect();
    let title_font = TextStyle { weight: 600.0, ..edge_font };
    let titles: Vec<Measured> = f.subgraphs.iter().map(|s| measure(&s.title, title_font)).collect();
    let fonts = Fonts { node: paint.font, edge: edge_font, title: title_font };
    let mut canvas = draw_in(f, f.dir, &nodes, &labels, &titles, fonts, paint)?;
    if canvas.width() > room(paint) && matches!(f.dir, Dir::Right | Dir::Left) {
        let down = draw_in(f, if f.dir == Dir::Right { Dir::Down } else { Dir::Up }, &nodes, &labels, &titles, fonts, paint)?;
        if down.width() < canvas.width() {
            canvas = down;
        }
    }
    Some(canvas)
}

#[derive(Clone, Copy)]
struct Fonts {
    node: TextStyle,
    edge: TextStyle,
    title: TextStyle,
}

fn draw_in(f: &Flowchart, dir: Dir, nodes: &[Sized], labels: &[Option<Measured>], titles: &[Measured], fonts: Fonts, paint: &DiagramStyle) -> Option<Canvas> {
    let across = matches!(dir, Dir::Right | Dir::Left);
    let ax = |s: [f32; 2]| if across { [s[1], s[0]] } else { s };
    let flip = matches!(dir, Dir::Up | Dir::Left);
    let graph = Graph {
        sizes: nodes.iter().map(|n| ax(n.size)).collect(),
        edges: f
            .edges
            .iter()
            .zip(labels)
            .map(|(e, l)| {
                let (from, to) = if flip { (e.to, e.from) } else { (e.from, e.to) };
                GEdge { from, to, label: l.as_ref().map(|m| ax(label_box(m))), minlen: e.minlen }
            })
            .collect(),
        node_cluster: f.nodes.iter().map(|n| n.cluster).collect(),
        clusters: f
            .subgraphs
            .iter()
            .zip(titles)
            .map(|(s, t)| {
                let lead = if t.text.is_empty() { 0.0 } else { t.size[1] + 8.0 };
                GCluster { parent: s.parent, lead: if across { [lead, 0.0] } else { [0.0, lead] } }
            })
            .collect(),
    };
    let placed = layout(&graph, &SPACING)?;
    let pt = |p: [f32; 2]| if across { [p[1], p[0]] } else { p };
    let mut c = Canvas::default();
    c.hover(paint);

    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = f.subgraphs[i].parent {
            i = p;
            d += 1;
            if d > f.subgraphs.len() {
                break;
            }
        }
        d
    };
    let mut title_boxes = Vec::new();
    let mut order: Vec<usize> = (0..f.subgraphs.len()).collect();
    order.sort_by_key(|&i| depth(i));
    for i in order {
        let b = placed.clusters[i];
        if b == [0.0; 4] {
            continue;
        }
        let (p0, p1) = (pt([b[0], b[1]]), pt([b[2], b[3]]));
        c.shape(rounded_rect(p0[0], p0[1], p1[0], p1[1], 6.0), paint.cluster_fill, paint.surface_border, 1.0);
        c.label_at(&titles[i], [p0[0] + 10.0, p0[1] + 7.0], fonts.title, paint.muted, Align::Start);
        title_boxes.push(title_box(&titles[i], p0));
    }

    let centers: Vec<[f32; 2]> = placed.centers.iter().map(|&p| pt(p)).collect();
    let sizes: Vec<[f32; 2]> = nodes.iter().map(|n| n.size).collect();
    let ends = f.edges.iter().map(|e| (e.from, e.to)).collect();
    let boxes = labels.iter().map(|l| l.as_ref().map(|m| ax(label_box(m)))).collect();
    let ctx = Ctx::new(&placed, &sizes, f.nodes.iter().map(|n| n.shape).collect(), boxes, across, ends).titles(title_boxes);
    edges(&mut c, &ctx, &f.edges, labels, flip, fonts.edge, paint);

    for (i, n) in f.nodes.iter().enumerate() {
        let (ctr, size) = (centers[i], nodes[i].size);
        c.region(i, outline(n.shape, ctr, size));
        let (fill, border, text_color) = n.style.resolve(paint);
        c.shape(outline(n.shape, ctr, size), fill, border, 1.0);
        let (x0, y0, x1, y1) = (ctr[0] - size[0] / 2.0, ctr[1] - size[1] / 2.0, ctr[0] + size[0] / 2.0, ctr[1] + size[1] / 2.0);
        match n.shape {
            NodeShape::Subroutine => {
                c.stroke(vec![[x0 + 8.0, y0], [x0 + 8.0, y1]], false, 1.0, border);
                c.stroke(vec![[x1 - 8.0, y0], [x1 - 8.0, y1]], false, 1.0, border);
            }
            NodeShape::Cylinder => c.stroke(arc([ctr[0], y0 + 8.0], size[0] / 2.0, 8.0, 0.0, 180.0, 20), false, 1.0, border),
            _ => {}
        }
        let center = if n.shape == NodeShape::Cylinder { [ctr[0], ctr[1] + 4.0] } else { ctr };
        c.label(&nodes[i].label, center, fonts.node, text_color, Align::Center);
    }
    Some(c)
}

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
    }
}

/// Every edge routed through `ctx`, with its label in a gap in the line; each edge's drawing is
/// kept as a trace so hovering either node lights it up.
pub(crate) fn edges(c: &mut Canvas, ctx: &Ctx, edges: &[Edge], labels: &[Option<Measured>], flip: bool, font: TextStyle, paint: &DiagramStyle) {
    for (i, e) in edges.iter().enumerate() {
        let start = c.ops.len();
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
                    c.fill(rounded_rect(g[0], g[1], g[2], g[3], 4.0), paint.surface);
                }
                c.label(m, at, font, paint.text, Align::Center);
            }
        }
        c.trace(e.from, e.to, start, paint.line);
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
                    run.push(super::lerp(a, b, t0));
                }
                let mut next = Vec::new();
                if t1 < 1.0 {
                    next.extend([super::lerp(a, b, t1), b]);
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
fn connector(c: &mut Canvas, runs: &[Vec<[f32; 2]>], tips: [([f32; 2], [f32; 2]); 2], e: &Edge, paint: &DiagramStyle) {
    let width = match e.line {
        Line::Thick => 2.5,
        _ => 1.25,
    };
    for run in runs {
        match e.line {
            Line::Invisible => return,
            Line::Dotted => c.dashed(run, width, paint.line, 3.0, 3.0),
            _ => c.stroke(run.clone(), false, width, paint.line),
        }
    }
    head(c, e.end, tips[1].0, tips[1].1, width, paint);
    head(c, e.start, tips[0].0, tips[0].1, width, paint);
}

fn head(c: &mut Canvas, h: Head, tip: [f32; 2], d: [f32; 2], width: f32, paint: &DiagramStyle) {
    let back = |by: f32| [tip[0] - d[0] * by, tip[1] - d[1] * by];
    match h {
        Head::None => {}
        Head::Arrow => c.fill(arrowhead(tip, d, ARROW + width, 6.5 + width * 1.6), paint.line),
        Head::Circle => c.shape(ellipse(back(4.0), 4.0, 4.0), paint.node_fill, paint.line, width),
        Head::Cross => {
            let m = back(4.0);
            let (a, b) = ([d[0] + d[1], d[1] - d[0]], [d[0] - d[1], d[1] + d[0]]);
            let k = 3.5 / std::f32::consts::SQRT_2;
            c.stroke(vec![[m[0] - a[0] * k, m[1] - a[1] * k], [m[0] + a[0] * k, m[1] + a[1] * k]], false, width, paint.line);
            c.stroke(vec![[m[0] - b[0] * k, m[1] - b[1] * k], [m[0] + b[0] * k, m[1] + b[1] * k]], false, width, paint.line);
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
