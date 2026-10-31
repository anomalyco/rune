//! Mindmaps (`mindmap`): a tree nested by indentation, drawn from its root out to both sides with
//! each top-level branch in its own colour. Node shapes are `[square]`, `(rounded)`,
//! `((circle))`, `))bang((`, `)cloud(` and `{{hexagon}}`; a bare line is a plain node.

use rune::{Align, Color, TextStyle};

use super::canvas::{Canvas, Measured, Scene, measure_wrap, over_color, rounded_rect};
use super::chart::{bold, hue, ink, s_curve, soft, solid, strong};
use super::shape::{self, NodeShape};
use super::{DiagramStyle, MAX_DEPTH, MAX_ITEMS, clean_quoted};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Look {
    Plain,
    Square,
    Rounded,
    Circle,
    Bang,
    Cloud,
    Hexagon,
}

#[derive(Debug)]
pub(crate) struct Node {
    pub text: String,
    pub look: Look,
    pub depth: usize,
    pub children: Vec<usize>,
}

/// Nodes in source order; the first is the root.
#[derive(Debug)]
pub(crate) struct Mindmap {
    pub nodes: Vec<Node>,
}

pub(crate) fn parse(body: &[&str], indents: &[usize]) -> Option<Mindmap> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for (i, line) in body.iter().enumerate() {
        let l = line.trim();
        if l.starts_with("::icon(") || l.starts_with(":::") || l.starts_with("accTitle") || l.starts_with("accDescr") {
            continue;
        }
        let Some((text, look)) = node(l) else { continue };
        if nodes.len() >= MAX_ITEMS {
            return None;
        }
        let indent = indents.get(i).copied().unwrap_or(0);
        let id = nodes.len();
        if id == 0 {
            nodes.push(Node { text, look, depth: 0, children: Vec::new() });
            stack.push((indent, 0));
            continue;
        }
        // A line no deeper than the root still hangs off it: there is one root.
        while stack.len() > 1 && stack.last().is_some_and(|s| s.0 >= indent) {
            stack.pop();
        }
        let parent = stack.last().map_or(0, |s| s.1);
        let depth = nodes[parent].depth + 1;
        if depth > MAX_DEPTH {
            return None;
        }
        nodes[parent].children.push(id);
        nodes.push(Node { text, look, depth, children: Vec::new() });
        stack.push((indent, id));
    }
    (!nodes.is_empty()).then_some(Mindmap { nodes })
}

/// `id[text]`, `id((text))`, … or a bare line. An opener without its closer reads as written.
fn node(l: &str) -> Option<(String, Look)> {
    let l = match l.rfind(":::") {
        Some(i) if !l[i..].contains([')', ']', '}']) => l[..i].trim(),
        _ => l,
    };
    if l.is_empty() {
        return None;
    }
    if let Some(p) = l.find(['[', '(', ')', '{']) {
        let rest = &l[p..];
        const SHAPES: [(&str, &str, Look); 7] = [
            ("((", "))", Look::Circle),
            ("))", "((", Look::Bang),
            ("{{", "}}", Look::Hexagon),
            (")", "(", Look::Cloud),
            ("(", ")", Look::Rounded),
            ("[", "]", Look::Square),
            ("{", "}", Look::Rounded),
        ];
        for (open, close, look) in SHAPES {
            if let Some(inner) = rest.strip_prefix(open).and_then(|r| r.strip_suffix(close))
                && !inner.is_empty()
            {
                return Some((clean_quoted(inner), look));
            }
        }
    }
    Some((clean_quoted(l), Look::Plain))
}

const GAP_X: f32 = 40.0;
const GAP_Y: f32 = 10.0;
const WRAP: f32 = 150.0;

struct Placed {
    label: Measured,
    size: [f32; 2],
    /// -1 left of the root, 1 right, 0 the root.
    side: f32,
    /// The top-level branch it's on.
    branch: usize,
    at: [f32; 2],
}

pub(crate) fn draw(m: &Mindmap, paint: &DiagramStyle) -> Option<Scene> {
    let n = m.nodes.len();
    let font = |i: usize| if i == 0 { bold(paint.font) } else { paint.font };
    let mut placed: Vec<Placed> = (0..n)
        .map(|i| {
            let node = &m.nodes[i];
            let label = measure_wrap(&node.text, font(i), WRAP);
            Placed { size: size(look(i, node.look), label.size), label, side: 0.0, branch: 0, at: [0.0; 2] }
        })
        .collect();
    let mut block = vec![0.0f32; n];
    for i in (0..n).rev() {
        let kids = &m.nodes[i].children;
        let stacked: f32 = kids.iter().map(|&k| block[k]).sum::<f32>() + GAP_Y * kids.len().saturating_sub(1) as f32;
        block[i] = placed[i].size[1].max(stacked);
    }

    // Top-level branches fill the right side in order until it holds half the height.
    let top = &m.nodes[0].children;
    let total: f32 = top.iter().map(|&k| block[k]).sum();
    let mut right = Vec::new();
    let mut left = Vec::new();
    let mut held = 0.0;
    for &k in top {
        if right.is_empty() || held + block[k] / 2.0 <= total / 2.0 {
            held += block[k];
            right.push(k);
        } else {
            left.push(k);
        }
    }
    for (b, &k) in top.iter().enumerate() {
        mark(m, &mut placed, k, b, if right.contains(&k) { 1.0 } else { -1.0 });
    }

    // Each depth on each side is a column as wide as its widest node.
    let mut cols = [vec![0.0f32; MAX_DEPTH + 1], vec![0.0f32; MAX_DEPTH + 1]];
    for (i, p) in placed.iter().enumerate().skip(1) {
        let c = &mut cols[usize::from(p.side > 0.0)][m.nodes[i].depth];
        *c = c.max(p.size[0]);
    }
    for (kids, side) in [(&right, 1.0f32), (&left, -1.0)] {
        let col = &cols[usize::from(side > 0.0)];
        let height: f32 = kids.iter().map(|&k| block[k]).sum::<f32>() + GAP_Y * kids.len().saturating_sub(1) as f32;
        let mut y = -height / 2.0;
        for &k in kids.iter() {
            place(m, &mut placed, &block, col, k, y);
            y += block[k] + GAP_Y;
        }
    }

    let mut c = Canvas::default();
    for (i, node) in m.nodes.iter().enumerate() {
        let p = &placed[i];
        for &k in &node.children {
            let q = &placed[k];
            let a = [p.at[0] + q.side * p.size[0] / 2.0, p.at[1]];
            let b = [q.at[0] - q.side * q.size[0] / 2.0, q.at[1]];
            let width = (3.5 - node.depth as f32 * 0.75).max(1.25);
            c.stroke(s_curve(a, b, false), false, width, solid(paint, branch_hue(q.branch)));
        }
    }
    for (i, node) in m.nodes.iter().enumerate() {
        let p = &placed[i];
        let h = branch_hue(p.branch);
        let (fill, border) = match node.depth {
            0 => (solid(paint, hue(0)), strong(paint, hue(0))),
            1 => (soft(paint, h), strong(paint, h)),
            _ => (over_color(soft(paint, h).alpha(0.55), paint.surface), strong(paint, h).alpha(0.7)),
        };
        let text = if node.depth == 0 { ink(fill) } else { paint.text };
        draw_node(&mut c, look(i, node.look), p, [fill, border, text], font(i));
    }
    c.finish_fit(paint)
}

/// A plain root is drawn rounded, so it reads as the centre.
fn look(i: usize, look: Look) -> Look {
    if i == 0 && look == Look::Plain { Look::Rounded } else { look }
}

/// Branch `b`'s hue; the root keeps the first.
fn branch_hue(b: usize) -> f32 {
    hue(b + 1)
}

fn mark(m: &Mindmap, placed: &mut [Placed], i: usize, branch: usize, side: f32) {
    placed[i].side = side;
    placed[i].branch = branch;
    for &k in &m.nodes[i].children {
        mark(m, placed, k, branch, side);
    }
}

/// Places `i`'s subtree in a band of its block height from `top`.
fn place(m: &Mindmap, placed: &mut [Placed], block: &[f32], col: &[f32], i: usize, top: f32) {
    let node = &m.nodes[i];
    let side = placed[i].side;
    let inner = col[1..node.depth].iter().map(|w| w + GAP_X).sum::<f32>() + placed[0].size[0] / 2.0 + GAP_X;
    placed[i].at = [side * (inner + placed[i].size[0] / 2.0), top + block[i] / 2.0];
    let stacked: f32 = node.children.iter().map(|&k| block[k]).sum::<f32>() + GAP_Y * node.children.len().saturating_sub(1) as f32;
    let mut y = top + (block[i] - stacked) / 2.0;
    for &k in &node.children {
        place(m, placed, block, col, k, y);
        y += block[k] + GAP_Y;
    }
}

fn as_shape(look: Look) -> Option<NodeShape> {
    match look {
        Look::Circle => Some(NodeShape::Circle),
        Look::Bang => Some(NodeShape::Bang),
        Look::Cloud => Some(NodeShape::Cloud),
        Look::Hexagon => Some(NodeShape::Hexagon),
        _ => None,
    }
}

fn size(look: Look, t: [f32; 2]) -> [f32; 2] {
    match as_shape(look) {
        Some(s) => {
            let [w, h] = shape::node_size(s, t);
            if look == Look::Hexagon { [w, (t[1] + 16.0).max(34.0)] } else { [w, h] }
        }
        None => [t[0] + 24.0, (t[1] + 14.0).max(32.0)],
    }
}

fn draw_node(c: &mut Canvas, look: Look, p: &Placed, [fill, border, text]: [Color; 3], font: TextStyle) {
    let [x, y] = p.at;
    let [w, h] = p.size;
    let (x0, y0, x1, y1) = (x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0);
    match look {
        Look::Plain => {
            c.fill(rounded_rect(x0, y0, x1, y1, 4.0), fill);
            c.stroke(vec![[x0, y1], [x1, y1]], false, 2.0, border);
        }
        Look::Square => c.shape(rounded_rect(x0, y0, x1, y1, 0.0), fill, border, 1.0),
        Look::Rounded => c.shape(rounded_rect(x0, y0, x1, y1, 10.0), fill, border, 1.0),
        _ => {
            if let Some(s) = as_shape(look) {
                c.shape(shape::outline(s, p.at, p.size), fill, border, 1.0);
            }
        }
    }
    c.label(&p.label, p.at, font, text, Align::Center);
}

#[cfg(test)]
#[path = "mindmap_tests.rs"]
mod tests;
