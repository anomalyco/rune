//! Shared node geometry, connector arrows and scene rendering on top of [`super::layered`] for
//! flowcharts, state diagrams and class diagrams.

use rune::{Align, Color, TextStyle};

use super::canvas::{Canvas, Measured, Scene, arc, ellipse, fit, measure, over_color, room, rounded_rect};
use super::connector::{self, label_box, title_box};
use super::layered::{GCluster, GEdge, Graph, Spacing, layout};
use super::route::Ctx;
use super::{DiagramStyle, EdgeStyle, Hover, NodeStyle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dir {
    Down,
    Up,
    Right,
    Left,
}

impl Dir {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "" | "TD" | "TB" => Some(Dir::Down),
            "BT" => Some(Dir::Up),
            "LR" => Some(Dir::Right),
            "RL" => Some(Dir::Left),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeShape {
    Rect,
    Round,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    Asymmetric,
    Diamond,
    Hexagon,
    Parallelogram,
    Trapezoid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Solid,
    Dotted,
    Thick,
    Invisible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Head {
    None,
    Arrow,
    Circle,
    Cross,
    OpenTriangle,
    DiamondFilled,
    DiamondOpen,
}

#[derive(Debug)]
pub(crate) struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub line: Line,
    pub start: Head,
    pub end: Head,
    pub minlen: usize,
    pub style: EdgeStyle,
}

pub(crate) struct ClusterSpec {
    pub title: String,
    pub parent: Option<usize>,
}

pub(crate) enum DrawNode {
    Shape {
        shape: NodeShape,
        label: Measured,
        size: [f32; 2],
        cluster: Option<usize>,
        style: NodeStyle,
    },
    Start {
        cluster: Option<usize>,
    },
    End {
        cluster: Option<usize>,
    },
    Bar {
        cluster: Option<usize>,
    },
    ClassBox {
        stereo: Option<Measured>,
        name: Measured,
        fields: Vec<Measured>,
        methods: Vec<Measured>,
        size: [f32; 2],
        cluster: Option<usize>,
        style: NodeStyle,
    },
}

impl DrawNode {
    fn size(&self) -> [f32; 2] {
        match self {
            DrawNode::Shape { size, .. } | DrawNode::ClassBox { size, .. } => *size,
            DrawNode::Start { .. } => [18.0, 18.0],
            DrawNode::End { .. } => [22.0, 22.0],
            DrawNode::Bar { .. } => [64.0, 8.0],
        }
    }

    fn shape(&self) -> NodeShape {
        match self {
            DrawNode::Shape { shape, .. } => *shape,
            DrawNode::Start { .. } | DrawNode::End { .. } => NodeShape::Circle,
            DrawNode::Bar { .. } | DrawNode::ClassBox { .. } => NodeShape::Rect,
        }
    }

    fn cluster(&self) -> Option<usize> {
        match self {
            DrawNode::Shape { cluster, .. }
            | DrawNode::Start { cluster }
            | DrawNode::End { cluster }
            | DrawNode::Bar { cluster }
            | DrawNode::ClassBox { cluster, .. } => *cluster,
        }
    }
}

const PAD: [f32; 2] = [30.0, 22.0];

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

pub(crate) fn draw_layered(
    dir: Dir,
    nodes: &[DrawNode],
    edges: &[Edge],
    clusters: &[ClusterSpec],
    spacing: &Spacing,
    paint: &DiagramStyle,
    hover: Option<(Hover, Color)>,
) -> Option<Scene> {
    let canvas = layered_canvas(dir, nodes, edges, clusters, spacing, paint, hover)?;
    let scale = fit(canvas.width(), paint);
    canvas.finish(scale)
}

/// [`draw_layered`] at full size, before [`Canvas::finish`].
pub(crate) fn layered_canvas(
    dir: Dir,
    nodes: &[DrawNode],
    edges: &[Edge],
    clusters: &[ClusterSpec],
    spacing: &Spacing,
    paint: &DiagramStyle,
    hover: Option<(Hover, Color)>,
) -> Option<Canvas> {
    let edge_font = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
    let labels: Vec<Option<Measured>> = edges.iter().map(|e| e.label.as_ref().map(|l| measure(l, edge_font))).collect();
    let title_font = TextStyle { weight: 600.0, ..edge_font };
    let titles: Vec<Measured> = clusters.iter().map(|c| measure(&c.title, title_font)).collect();
    let fonts = Fonts { edge: edge_font, title: title_font };
    let parts = Parts { nodes, edges, clusters, labels: &labels, titles: &titles };
    let mut canvas = draw_in(dir, &parts, fonts, spacing, paint, hover)?;
    // Reflow a horizontal diagram to vertical only when horizontal scaling would drop below 78%.
    if canvas.width() > room(paint) / 0.78 && matches!(dir, Dir::Right | Dir::Left) {
        let down = draw_in(if dir == Dir::Right { Dir::Down } else { Dir::Up }, &parts, fonts, spacing, paint, hover)?;
        if down.width() < canvas.width() {
            canvas = down;
        }
    }
    Some(canvas)
}

#[derive(Clone, Copy)]
struct Fonts {
    edge: TextStyle,
    title: TextStyle,
}

struct Parts<'a> {
    nodes: &'a [DrawNode],
    edges: &'a [Edge],
    clusters: &'a [ClusterSpec],
    labels: &'a [Option<Measured>],
    titles: &'a [Measured],
}

fn draw_in(dir: Dir, p: &Parts, fonts: Fonts, spacing: &Spacing, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Canvas> {
    let Parts { nodes, edges, clusters, labels, titles } = *p;
    let across = matches!(dir, Dir::Right | Dir::Left);
    let ax = |s: [f32; 2]| if across { [s[1], s[0]] } else { s };
    let flip = matches!(dir, Dir::Up | Dir::Left);
    let graph = Graph {
        sizes: nodes.iter().map(|n| ax(n.size())).collect(),
        edges: edges
            .iter()
            .zip(labels)
            .map(|(e, l)| {
                let (from, to) = if flip { (e.to, e.from) } else { (e.from, e.to) };
                GEdge { from, to, label: l.as_ref().map(|m| ax(label_box(m))), minlen: e.minlen }
            })
            .collect(),
        node_cluster: nodes.iter().map(DrawNode::cluster).collect(),
        clusters: clusters
            .iter()
            .zip(titles)
            .map(|(s, t)| {
                let lead = if t.text.is_empty() { 0.0 } else { t.size[1] + 8.0 };
                let min_w = if t.text.is_empty() { 0.0 } else { t.size[0] + 24.0 };
                GCluster {
                    parent: s.parent,
                    lead: if across { [lead, 0.0] } else { [0.0, lead] },
                    min_span: if across { [0.0, min_w] } else { [min_w, 0.0] },
                }
            })
            .collect(),
    };
    let placed = layout(&graph, spacing)?;
    let pt = |p: [f32; 2]| if across { [p[1], p[0]] } else { p };
    let mut c = Canvas { hover, ..Canvas::default() };

    let chain_of = |mut c_opt: Option<usize>| {
        let mut v = Vec::new();
        while let Some(i) = c_opt {
            if v.len() > clusters.len() {
                break;
            }
            v.push(i);
            c_opt = clusters[i].parent;
        }
        v.reverse();
        v
    };
    let cluster_border = over_color(paint.node_border.alpha(0.45), paint.surface_border);
    let mut title_boxes = Vec::new();
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    order.sort_by_key(|&i| chain_of(Some(i)).len());
    for i in order {
        let b = placed.clusters[i];
        if b == [0.0; 4] {
            continue;
        }
        let (p0, p1) = (pt([b[0], b[1]]), pt([b[2], b[3]]));
        c.shape(rounded_rect(p0[0], p0[1], p1[0], p1[1], 6.0), paint.cluster_fill, cluster_border, 1.0);
        c.label_at(&titles[i], [p0[0] + 10.0, p0[1] + 7.0], fonts.title, paint.muted, Align::Start);
        title_boxes.push(title_box(&titles[i], p0));
    }

    let centers: Vec<[f32; 2]> = placed.centers.iter().map(|&p| pt(p)).collect();
    let sizes: Vec<[f32; 2]> = nodes.iter().map(DrawNode::size).collect();
    let ends = edges.iter().map(|e| (e.from, e.to)).collect();
    let boxes = labels.iter().map(|l| l.as_ref().map(|m| ax(label_box(m)))).collect();
    let ctx = Ctx::new(&placed, &sizes, nodes.iter().map(DrawNode::shape).collect(), boxes, across, ends).titles(title_boxes);
    // A label pill takes the colour of the innermost subgraph holding both its ends.
    let backs: Vec<Color> = edges
        .iter()
        .map(|e| {
            let (ca, cb) = (chain_of(nodes[e.from].cluster()), chain_of(nodes[e.to].cluster()));
            let common = ca.iter().zip(&cb).take_while(|(x, y)| x == y).count();
            (0..common).fold(paint.surface, |bg, _| over_color(paint.cluster_fill, bg))
        })
        .collect();
    connector::edges(&mut c, &ctx, edges, labels, &backs, flip, fonts.edge, paint);

    for (i, n) in nodes.iter().enumerate() {
        c.region(i, outline(n.shape(), centers[i], n.size()));
        draw_node(&mut c, n, centers[i], paint);
    }
    Some(c)
}

fn draw_node(c: &mut Canvas, node: &DrawNode, ctr: [f32; 2], paint: &DiagramStyle) {
    match node {
        DrawNode::Start { .. } => c.fill(ellipse(ctr, 8.0, 8.0), paint.line),
        DrawNode::End { .. } => {
            c.shape(ellipse(ctr, 10.0, 10.0), paint.node_fill, paint.line, 1.5);
            c.fill(ellipse(ctr, 5.5, 5.5), paint.line);
        }
        DrawNode::Bar { .. } => {
            let s = node.size();
            c.fill(rounded_rect(ctr[0] - s[0] / 2.0, ctr[1] - s[1] / 2.0, ctr[0] + s[0] / 2.0, ctr[1] + s[1] / 2.0, 2.0), paint.line);
        }
        DrawNode::Shape { shape, label, size, style, .. } => {
            let (fill, border, text_color) = style.resolve(paint);
            c.shape(outline(*shape, ctr, *size), fill, border, 1.0);
            let (x0, y0, x1, y1) = (ctr[0] - size[0] / 2.0, ctr[1] - size[1] / 2.0, ctr[0] + size[0] / 2.0, ctr[1] + size[1] / 2.0);
            match shape {
                NodeShape::Subroutine => {
                    c.stroke(vec![[x0 + 8.0, y0], [x0 + 8.0, y1]], false, 1.0, border);
                    c.stroke(vec![[x1 - 8.0, y0], [x1 - 8.0, y1]], false, 1.0, border);
                }
                NodeShape::Cylinder => c.stroke(arc([ctr[0], y0 + 8.0], size[0] / 2.0, 8.0, 0.0, 180.0, 20), false, 1.0, border),
                _ => {}
            }
            let center = if *shape == NodeShape::Cylinder { [ctr[0], ctr[1] + 4.0] } else { ctr };
            c.label(label, center, paint.font, text_color, Align::Center);
        }
        DrawNode::ClassBox { stereo, name, fields, methods, size, style, .. } => {
            let (fill, border, text_color) = style.resolve(paint);
            let (x0, y0, x1, y1) = (ctr[0] - size[0] / 2.0, ctr[1] - size[1] / 2.0, ctr[0] + size[0] / 2.0, ctr[1] + size[1] / 2.0);
            c.shape(rounded_rect(x0, y0, x1, y1, 4.0), fill, border, 1.0);
            let head_font = TextStyle { weight: 600.0, ..paint.font };
            let mem_font = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
            let stereo_font = TextStyle { size: paint.font.size - 2.0, line_height: paint.font.line_height - 4.0, italic: true, ..paint.font };
            let mut y = y0 + 8.0;
            if let Some(st) = stereo {
                c.label(st, [ctr[0], y + st.size[1] / 2.0], stereo_font, paint.muted, Align::Center);
                y += st.size[1] + 2.0;
            }
            c.label(name, [ctr[0], y + name.size[1] / 2.0], head_font, text_color, Align::Center);
            y += name.size[1] + 8.0;
            for section in [fields, methods] {
                if section.is_empty() {
                    continue;
                }
                c.stroke(vec![[x0, y], [x1, y]], false, 1.0, border);
                y += 6.0;
                for item in section {
                    c.label_at(item, [x0 + 12.0, y], mem_font, text_color, Align::Start);
                    y += item.size[1] + 3.0;
                }
                y += 3.0;
            }
        }
    }
}
