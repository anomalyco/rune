//! Block diagrams (`block-beta`, `block`): blocks in rows of `columns`, spanning cells with
//! `id:2`, gaps from `space`, nested `block … end` groups, block arrows `id<["label"]>(right)`,
//! and edges between blocks, laid out on a grid rather than by rank.

use std::collections::HashMap;

use rune::Align;

use super::canvas::{Canvas, Measured, Scene, measure_wrap, over_color, rounded_rect, unit};
use super::connector::{connector, inset, label_box};
use super::flowchart::{ident, link, shape};
use super::layered_draw::{Edge, NodeShape, boundary, node_size};
use super::shape as shapes;
use super::{Cur, DiagramStyle, EdgeStyle, MAX_DEPTH, MAX_ITEMS, NodeStyle, StyleSheet, clean_quoted, statements, word};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Point {
    Right,
    Left,
    Up,
    Down,
    /// Both ways across.
    X,
    /// Both ways up and down.
    Y,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Kind {
    Node,
    Space,
    Arrow(Point),
    Group { columns: Option<usize>, children: Vec<usize> },
}

#[derive(Debug)]
pub(crate) struct Block {
    #[cfg_attr(not(test), allow(dead_code, reason = "tests find blocks by id"))]
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    pub span: usize,
    pub kind: Kind,
    pub style: NodeStyle,
}

/// Block 0 is the diagram itself: a group that isn't drawn.
#[derive(Debug)]
pub(crate) struct BlockDiagram {
    pub blocks: Vec<Block>,
    pub edges: Vec<Edge>,
}

struct Item {
    id: String,
    label: Option<String>,
    shape: Option<NodeShape>,
    arrow: Option<Point>,
    span: usize,
}

struct Parser {
    d: BlockDiagram,
    ids: HashMap<String, usize>,
    stack: Vec<usize>,
    styles: StyleSheet,
    over: bool,
}

pub(crate) fn parse(body: &[&str]) -> Option<BlockDiagram> {
    let root = Block { id: String::new(), label: String::new(), shape: NodeShape::Rect, span: 1, kind: Kind::Group { columns: None, children: Vec::new() }, style: NodeStyle::default() };
    let mut p = Parser {
        d: BlockDiagram { blocks: vec![root], edges: Vec::new() },
        ids: HashMap::new(),
        stack: vec![0],
        styles: StyleSheet::default(),
        over: false,
    };
    for &line in body {
        for s in statements(line) {
            p.statement(s);
            if p.over {
                return None;
            }
        }
    }
    let Parser { mut d, ids, styles, .. } = p;
    for (id, i) in ids {
        d.blocks[i].style = styles.get(&id);
    }
    (d.blocks.len() > 1).then_some(d)
}

impl Parser {
    fn budget(&mut self) -> Option<()> {
        self.over |= self.d.blocks.len() + self.d.edges.len() >= MAX_ITEMS;
        (!self.over).then_some(())
    }

    fn group(&mut self) -> usize {
        *self.stack.last().unwrap_or(&0)
    }

    fn statement(&mut self, s: &str) -> Option<()> {
        if let Some(rest) = word(s, "columns") {
            let n = rest.parse::<usize>().ok().map(|n| n.clamp(1, 24));
            let g = self.group();
            if let Kind::Group { columns, .. } = &mut self.d.blocks[g].kind {
                *columns = n;
            }
        } else if s == "end" {
            if self.stack.len() > 1 {
                self.stack.pop();
            }
        } else if s == "block" || s.starts_with("block:") {
            let mut parts = s["block".len()..].trim_start_matches(':').split(':');
            let id = parts.next().unwrap_or("").trim();
            let span = parts.next().and_then(|n| n.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 24);
            if self.stack.len() > MAX_DEPTH {
                self.over = true;
                return None;
            }
            let i = self.add(Item { id: id.to_owned(), label: Some(String::new()), shape: None, arrow: None, span })?;
            self.d.blocks[i].kind = Kind::Group { columns: None, children: Vec::new() };
            self.stack.push(i);
        } else if let Some(rest) = word(s, "classDef") {
            self.styles.class_def(rest)?;
        } else if let Some(rest) = word(s, "class") {
            self.styles.assign_classes(rest)?;
        } else if let Some(rest) = word(s, "style") {
            self.styles.style(rest)?;
        } else if self.edges(s).is_none() {
            self.items(s)?;
        }
        Some(())
    }

    /// `a --> b -- "label" --> c`, read whole first so a line that isn't one adds nothing.
    fn edges(&mut self, s: &str) -> Option<()> {
        let mut c = Cur::new(s);
        let mut refs = vec![item(&mut c)?];
        let mut links = Vec::new();
        loop {
            c.skip_ws();
            if c.done() {
                break;
            }
            links.push(link(&mut c)?);
            c.skip_ws();
            refs.push(item(&mut c)?);
        }
        if links.is_empty() {
            return None;
        }
        let ends: Vec<usize> = refs.into_iter().map(|r| self.add(r)).collect::<Option<_>>()?;
        for (l, w) in links.into_iter().zip(ends.windows(2)) {
            self.budget()?;
            self.d.edges.push(Edge { from: w[0], to: w[1], label: l.label, line: l.line, start: l.start, end: l.end, minlen: 1, style: EdgeStyle::default() });
        }
        Some(())
    }

    /// `a b["label"]:2 space c`, read whole first.
    fn items(&mut self, s: &str) -> Option<()> {
        let mut c = Cur::new(s);
        let mut items = Vec::new();
        loop {
            c.skip_ws();
            if c.done() {
                break;
            }
            items.push(item(&mut c)?);
        }
        for it in items {
            self.add(it)?;
        }
        Some(())
    }

    /// A block placed in the open group, or the one `id` already names (taking a new label or shape).
    fn add(&mut self, it: Item) -> Option<usize> {
        if let Some(&i) = self.ids.get(&it.id) {
            let b = &mut self.d.blocks[i];
            if let Some(l) = it.label {
                b.label = l;
            }
            if let Some(s) = it.shape {
                b.shape = s;
            }
            return Some(i);
        }
        self.budget()?;
        let kind = match it.arrow {
            _ if it.id == "space" => Kind::Space,
            Some(p) => Kind::Arrow(p),
            None => Kind::Node,
        };
        let label = it.label.unwrap_or_else(|| it.id.clone());
        let i = self.d.blocks.len();
        if kind != Kind::Space && !it.id.is_empty() {
            self.ids.insert(it.id.clone(), i);
        }
        self.d.blocks.push(Block { id: it.id, label, shape: it.shape.unwrap_or(NodeShape::Rect), span: it.span, kind, style: NodeStyle::default() });
        let g = self.group();
        if let Kind::Group { children, .. } = &mut self.d.blocks[g].kind {
            children.push(i);
        }
        Some(i)
    }
}

/// `id`, `id["label"]`, `id(round)`, `id<["label"]>(right)`, each with an optional `:span`.
fn item(c: &mut Cur) -> Option<Item> {
    let id = ident(c)?;
    let (mut label, mut shape_of, mut arrow) = (None, None, None);
    if c.eat("<[") {
        let end = c.rest().find("]>")?;
        label = Some(clean_quoted(&c.rest()[..end]));
        c.i += end + 2;
        let dir = c.rest().strip_prefix('(').and_then(|r| r.find(')').map(|e| &r[..e]))?;
        c.i += dir.len() + 2;
        arrow = Some(match dir.trim() {
            "right" => Point::Right,
            "left" => Point::Left,
            "up" => Point::Up,
            "down" => Point::Down,
            "x" => Point::X,
            "y" => Point::Y,
            _ => return None,
        });
    } else if let Some((l, s)) = shape(c) {
        (label, shape_of) = (l, Some(s));
    }
    let mut span = 1;
    if c.peek() == Some(':') && c.peek2().is_some_and(|d| d.is_ascii_digit()) {
        c.bump();
        let start = c.i;
        while c.peek().is_some_and(|d| d.is_ascii_digit()) {
            c.bump();
        }
        span = c.s[start..c.i].parse::<usize>().ok()?.clamp(1, 24);
    }
    Some(Item { id, label, shape: shape_of, arrow, span })
}

const GAP: f32 = 10.0;
const PAD: f32 = 10.0;
/// The gap between rows when edges run between blocks, so an arrow and its label show.
const EDGE_GAP: f32 = 40.0;

/// Each group's rows: the children in it with the column each starts at.
fn rows(d: &BlockDiagram, g: usize) -> (usize, Vec<Vec<(usize, usize)>>) {
    let Kind::Group { columns, children } = &d.blocks[g].kind else { return (1, Vec::new()) };
    let cols = columns.unwrap_or_else(|| children.iter().map(|&k| d.blocks[k].span).sum::<usize>()).max(1);
    let mut out: Vec<Vec<(usize, usize)>> = vec![Vec::new()];
    let mut at = 0;
    for &k in children {
        let span = d.blocks[k].span.min(cols);
        if at > 0 && at + span > cols {
            out.push(Vec::new());
            at = 0;
        }
        if let Some(row) = out.last_mut() {
            row.push((k, at));
        }
        at += span;
    }
    (cols, out)
}

struct Layout<'a> {
    d: &'a BlockDiagram,
    labels: Vec<Measured>,
    natural: Vec<[f32; 2]>,
    rects: Vec<[f32; 4]>,
    /// The gap between rows.
    row_gap: f32,
}

impl Layout<'_> {
    fn pad(g: usize) -> f32 {
        if g == 0 { 0.0 } else { PAD }
    }

    fn measure(&mut self, i: usize) -> [f32; 2] {
        let m = &self.labels[i];
        let size = match &self.d.blocks[i].kind {
            Kind::Space => [0.0, 0.0],
            Kind::Node => node_size(self.d.blocks[i].shape, m.size),
            Kind::Arrow(Point::Up | Point::Down | Point::Y) => [m.size[0] + 40.0, m.size[1] + 56.0],
            Kind::Arrow(_) => [m.size[0] + 64.0, m.size[1] + 36.0],
            Kind::Group { .. } => {
                let (cols, rows) = rows(self.d, i);
                let mut cell: f32 = 0.0;
                let mut height = 0.0;
                for row in &rows {
                    let mut tall: f32 = 0.0;
                    for &(k, _) in row {
                        let s = self.measure(k);
                        let span = self.d.blocks[k].span.min(cols) as f32;
                        cell = cell.max((s[0] - (span - 1.0) * GAP) / span);
                        tall = tall.max(s[1]);
                    }
                    height += tall;
                }
                let pad = Self::pad(i);
                [cols as f32 * cell + (cols as f32 - 1.0) * GAP + 2.0 * pad, height + (rows.len() as f32 - 1.0).max(0.0) * self.row_gap + 2.0 * pad]
            }
        };
        self.natural[i] = size;
        size
    }

    fn place(&mut self, i: usize, r: [f32; 4]) {
        self.rects[i] = r;
        if !matches!(self.d.blocks[i].kind, Kind::Group { .. }) {
            return;
        }
        let (cols, rows) = rows(self.d, i);
        let pad = Self::pad(i);
        let cell = (r[2] - 2.0 * pad - (cols as f32 - 1.0) * GAP) / cols as f32;
        let mut y = r[1] + pad + (r[3] - self.natural[i][1]).max(0.0) / 2.0;
        for row in rows {
            let tall = row.iter().map(|&(k, _)| self.natural[k][1]).fold(0.0, f32::max);
            for (k, at) in row {
                let span = self.d.blocks[k].span.min(cols) as f32;
                let x = r[0] + pad + at as f32 * (cell + GAP);
                self.place(k, [x, y, span * cell + (span - 1.0) * GAP, tall]);
            }
            y += tall + self.row_gap;
        }
    }

    /// Where block `i` is drawn: its centre and size.
    fn frame(&self, i: usize) -> ([f32; 2], [f32; 2]) {
        let [x, y, w, h] = self.rects[i];
        let n = self.natural[i];
        let size = match self.d.blocks[i].kind {
            Kind::Group { .. } => [w, h],
            Kind::Arrow(_) => n,
            _ if matches!(self.d.blocks[i].shape, NodeShape::Circle | NodeShape::DoubleCircle) => n,
            _ => [w, n[1]],
        };
        ([x + w / 2.0, y + h / 2.0], size)
    }
}

pub(crate) fn draw(d: &BlockDiagram, paint: &DiagramStyle) -> Option<Scene> {
    let n = d.blocks.len();
    let mut l = Layout { d, labels: d.blocks.iter().map(|b| measure_wrap(&b.label, paint.font, 180.0)).collect(), natural: vec![[0.0; 2]; n], rects: vec![[0.0; 4]; n], row_gap: if d.edges.is_empty() { GAP } else { EDGE_GAP } };
    let [w, h] = l.measure(0);
    l.place(0, [0.0, 0.0, w, h]);
    let mut c = Canvas::default();
    let group_border = over_color(paint.node_border.alpha(0.45), paint.surface_border);
    for i in 1..n {
        if matches!(d.blocks[i].kind, Kind::Group { .. }) {
            let [x, y, w, h] = l.rects[i];
            let fill = d.blocks[i].style.fill.map_or(paint.cluster_fill, |_| d.blocks[i].style.resolve(paint).0);
            c.shape(rounded_rect(x, y, x + w, y + h, 6.0), fill, group_border, 1.0);
        }
    }
    let edge_font = super::chart::small(paint);
    for e in &d.edges {
        let ((ca, sa), (cb, sb)) = (l.frame(e.from), l.frame(e.to));
        let outline = |i: usize| if matches!(d.blocks[i].kind, Kind::Node) { d.blocks[i].shape } else { NodeShape::Rect };
        let (pa, pb) = (boundary(outline(e.from), ca, sa, cb), boundary(outline(e.to), cb, sb, ca));
        let u = unit(pa, pb);
        let (a, b) = ([pa[0] + u[0] * inset(e.start), pa[1] + u[1] * inset(e.start)], [pb[0] - u[0] * inset(e.end), pb[1] - u[1] * inset(e.end)]);
        connector(&mut c, &[vec![a, b]], [(pa, [-u[0], -u[1]]), (pb, u)], e, paint);
        if let Some(text) = &e.label {
            let m = measure_wrap(text, edge_font, 160.0);
            let mid = [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0];
            let s = label_box(&m);
            c.fill(rounded_rect(mid[0] - s[0] / 2.0, mid[1] - s[1] / 2.0, mid[0] + s[0] / 2.0, mid[1] + s[1] / 2.0, 4.0), paint.surface);
            c.label(&m, mid, edge_font, paint.text, Align::Center);
        }
    }
    for (i, b) in d.blocks.iter().enumerate().skip(1) {
        let (ctr, size) = l.frame(i);
        let colors = b.style.resolve(paint);
        match b.kind {
            Kind::Node => shapes::draw(&mut c, b.shape, &l.labels[i], ctr, size, colors.into(), paint),
            Kind::Arrow(p) => {
                c.shape(arrow(p, ctr, size), colors.0, colors.1, 1.0);
                c.label(&l.labels[i], ctr, paint.font, colors.2, Align::Center);
            }
            Kind::Space | Kind::Group { .. } => {}
        }
    }
    c.finish_fit(paint)
}

/// A block arrow's outline: a shaft with a head at one end (or both, for `x` and `y`).
fn arrow(p: Point, ctr: [f32; 2], size: [f32; 2]) -> Vec<[f32; 2]> {
    let upright = matches!(p, Point::Up | Point::Down | Point::Y);
    let (len, thick) = if upright { (size[1], size[0]) } else { (size[0], size[1]) };
    let (hl, ht) = (len / 2.0, thick / 2.0);
    let head = (thick * 0.55).min(len * 0.4);
    let shaft = ht * 0.62;
    let both = matches!(p, Point::X | Point::Y);
    let back = if both { -hl + head } else { -hl };
    let mut pts = vec![[back, -shaft], [hl - head, -shaft], [hl - head, -ht], [hl, 0.0], [hl - head, ht], [hl - head, shaft], [back, shaft]];
    if both {
        pts.extend([[back, ht], [-hl, 0.0], [back, -ht]]);
    }
    let flip = matches!(p, Point::Left | Point::Up);
    pts.into_iter()
        .map(|[u, v]| {
            let u = if flip { -u } else { u };
            if upright { [ctr[0] + v, ctr[1] + u] } else { [ctr[0] + u, ctr[1] + v] }
        })
        .collect()
}

#[cfg(test)]
#[path = "block_tests.rs"]
mod tests;
