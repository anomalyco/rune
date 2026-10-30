//! Shared node geometry, connector arrows and scene rendering on top of [`super::layered`] for
//! flowcharts, state diagrams and class diagrams.

use rune::{Align, Color, TextStyle};

use super::canvas::{Canvas, Measured, Scene, ellipse, fit, measure, over_color, room, rounded_rect};
use super::connector::{self, label_box, title_box};
use super::layered::{GCluster, GEdge, Graph, Spacing, layout};
use super::route::Ctx;
use super::shape;
pub(crate) use super::shape::{NodeShape, boundary, node_size, outline};
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
    /// Entity-relationship cardinalities, drawn in crow's-foot notation.
    OnlyOne,
    ZeroOrOne,
    OneOrMore,
    ZeroOrMore,
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
    /// An entity: its name over a table of attributes, one row each, in columns `cols` wide.
    Entity {
        name: Measured,
        rows: Vec<Vec<Measured>>,
        cols: Vec<f32>,
        size: [f32; 2],
        style: NodeStyle,
    },
}

impl DrawNode {
    fn size(&self) -> [f32; 2] {
        match self {
            DrawNode::Shape { size, .. } | DrawNode::ClassBox { size, .. } | DrawNode::Entity { size, .. } => *size,
            DrawNode::Start { .. } => [18.0, 18.0],
            DrawNode::End { .. } => [22.0, 22.0],
            DrawNode::Bar { .. } => [64.0, 8.0],
        }
    }

    fn shape(&self) -> NodeShape {
        match self {
            DrawNode::Shape { shape, .. } => *shape,
            DrawNode::Start { .. } | DrawNode::End { .. } => NodeShape::Circle,
            DrawNode::Bar { .. } | DrawNode::ClassBox { .. } | DrawNode::Entity { .. } => NodeShape::Rect,
        }
    }

    fn cluster(&self) -> Option<usize> {
        match self {
            DrawNode::Shape { cluster, .. }
            | DrawNode::Start { cluster }
            | DrawNode::End { cluster }
            | DrawNode::Bar { cluster }
            | DrawNode::ClassBox { cluster, .. } => *cluster,
            DrawNode::Entity { .. } => None,
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
            shape::draw(c, *shape, label, ctr, *size, [fill, border, text_color], paint);
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
        DrawNode::Entity { name, rows, cols, size, style } => {
            let (fill, border, text_color) = style.resolve(paint);
            let (x0, y0, x1, y1) = (ctr[0] - size[0] / 2.0, ctr[1] - size[1] / 2.0, ctr[0] + size[0] / 2.0, ctr[1] + size[1] / 2.0);
            let head_font = TextStyle { weight: 600.0, ..paint.font };
            let row_font = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
            let head_h = name.size[1] + ENTITY_PAD[1] * 2.0;
            c.shape(rounded_rect(x0, y0, x1, y1, 4.0), fill, border, 1.0);
            c.label(name, [ctr[0], y0 + head_h / 2.0], head_font, text_color, Align::Center);
            let mut y = y0 + head_h;
            let stripe = text_color.alpha(0.035);
            for (r, row) in rows.iter().enumerate() {
                let h = entity_row_h(row);
                if r % 2 == 1 {
                    c.fill(vec![[x0 + 0.5, y], [x1 - 0.5, y], [x1 - 0.5, y + h], [x0 + 0.5, y + h]], stripe);
                }
                c.stroke(vec![[x0, y], [x1, y]], false, if r == 0 { 1.0 } else { 0.5 }, if r == 0 { border } else { border.alpha(0.5) });
                let mut x = x0;
                for (i, (cell, w)) in row.iter().zip(cols).enumerate() {
                    // Keys are muted; comments, last, are muted and may run the rest of the width.
                    let color = if i >= 2 { paint.muted } else { text_color };
                    c.label_at(cell, [x + ENTITY_PAD[0], y + (h - cell.size[1]) / 2.0], row_font, color, Align::Start);
                    x += w;
                }
                y += h;
            }
        }
    }
}

/// An entity's padding inside each cell: `[across, down]`.
pub(crate) const ENTITY_PAD: [f32; 2] = [10.0, 5.0];

pub(crate) fn entity_row_h(row: &[Measured]) -> f32 {
    row.iter().map(|m| m.size[1]).fold(0.0, f32::max) + ENTITY_PAD[1] * 2.0
}
