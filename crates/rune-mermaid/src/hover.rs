//! What hovering a flowchart or state diagram lights up. Each node keeps its outline as a region
//! and each edge a copy of its drawing in the hover colour, its label's index and a backing for
//! it. The finished scene puts a hover group over every node that shows its edges and their
//! labels (and with [`Hover::Neighbours`] the outlines they reach), and a chain of hover groups
//! along every edge and over its label that show that edge and its label. Hover groups light by
//! their rectangles alone, so an edge's rectangles are cut back from every node's box and a node
//! under the pointer always wins.

use rune::{Color, LineCap, Shape};

use super::canvas::{Canvas, Op, dist, lerp, rounded_rect};

/// What hovering a flowchart or state diagram node lights up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Hover {
    /// Nothing.
    Off,
    /// The edges in and out of it, with their labels.
    #[default]
    Edges,
    /// Its edges, its outline and the outlines of the nodes they reach.
    Neighbours,
}

/// How far either side of an edge's line the pointer lights it, in points.
pub(crate) const EDGE_SLOP: f32 = 8.0;
/// The most rectangles one edge's line is split into, and in a whole diagram.
const EDGE_HITS: usize = 16;
const ALL_EDGE_HITS: usize = 1200;

pub(crate) struct Trace {
    pub ends: (usize, usize),
    pub ops: Vec<Op>,
    /// The line as drawn, for its hit rectangles.
    pub line: Vec<[f32; 2]>,
    /// Its label's index in [`Canvas::labels`] and its box, `[x0, y0, x1, y1]`.
    pub label: Option<(usize, [f32; 4])>,
}

/// One hover group: what it is (`node3`, `edge2.0`), where (`[x, y, w, h]`, finished), what it
/// draws lit, and which labels it shows in the hover colour.
pub(crate) struct Spot {
    pub key: String,
    pub rect: [f32; 4],
    pub lit: Shape,
    pub labels: Vec<usize>,
}

impl Canvas {
    /// Node `i` drawn as `outline`: hovering it lights up its traces.
    pub(crate) fn region(&mut self, i: usize, outline: Vec<[f32; 2]>) {
        if self.hover.is_none() {
            return;
        }
        if self.regions.len() <= i {
            self.regions.resize(i + 1, Vec::new());
        }
        self.regions[i] = outline;
    }

    /// The ops drawn since `start` are the edge `from` → `to`: keeps a copy with every `line`
    /// colour turned to the hover colour, a little heavier, for hovering either end or the edge.
    /// `label` is the label drawn with it (its index in [`Canvas::labels`] and box) and the colour
    /// behind it, which its lit backing covers before tinting.
    pub(crate) fn trace(&mut self, from: usize, to: usize, start: usize, line: Color, label: Option<(usize, [f32; 4], Color)>) {
        let Some((_, lit)) = self.hover else { return };
        let mut path = Vec::new();
        let mut ops: Vec<Op> = self.ops[start..]
            .iter()
            .map(|op| match op {
                Op::Fill(p, c) if *c == line => Op::Fill(p.clone(), lit),
                Op::Stroke { points, closed, width, cap, color } if *color == line => {
                    if !closed {
                        path.extend_from_slice(points);
                    }
                    Op::Stroke { points: points.clone(), closed: *closed, width: width + 0.5, cap: *cap, color: lit }
                }
                op => op.clone(),
            })
            .collect();
        if let Some((_, g, back)) = label {
            let pill = rounded_rect(g[0], g[1], g[2], g[3], 4.0);
            ops.push(Op::Fill(pill.clone(), back));
            ops.push(Op::Fill(pill.clone(), Color { a: lit.a * 0.14, ..lit }));
            ops.push(Op::Stroke { points: pill, closed: true, width: 1.0, cap: LineCap::Round, color: lit });
        }
        self.traces.push(Trace { ends: (from, to), ops, line: path, label: label.map(|(i, g, _)| (i, g)) });
    }

    /// Every hover group, edges first so a node's lit drawing lands over theirs.
    pub(crate) fn hovers(&self, map: impl Fn([f32; 2]) -> [f32; 2], paint: impl Fn(Shape, &Op) -> Shape) -> Vec<Spot> {
        let Some((reach, lit)) = self.hover else { return Vec::new() };
        let drawn = |v: usize| self.regions.get(v).is_some_and(|r| r.len() >= 3);
        let rect = |lo: [f32; 2], hi: [f32; 2]| {
            let (a, z) = (map(lo), map(hi));
            [a[0], a[1], z[0] - a[0], z[1] - a[1]]
        };
        let boxes: Vec<[f32; 4]> = (0..self.regions.len()).filter(|&v| drawn(v)).map(|v| {
            let (lo, hi) = bounds(&self.regions[v]);
            rect(lo, hi)
        }).collect();
        let outline = |v: usize| Op::Stroke { points: self.regions[v].clone(), closed: true, width: 1.0, cap: LineCap::Round, color: lit };
        let mut spots = Vec::new();
        let mut budget = ALL_EDGE_HITS;
        for (k, t) in self.traces.iter().enumerate() {
            let shape = t.ops.iter().fold(Shape::new(), &paint);
            let labels: Vec<usize> = t.label.iter().map(|l| l.0).collect();
            let line: Vec<[f32; 2]> = t.line.iter().map(|&p| map(p)).collect();
            let mut hits = cut_away(line_hits(&line, EDGE_SLOP), &boxes);
            hits.truncate(budget);
            budget -= hits.len();
            if let Some((_, g)) = t.label {
                hits.push(rect([g[0], g[1]], [g[2], g[3]]));
            }
            for (j, r) in hits.into_iter().enumerate() {
                spots.push(Spot { key: format!("edge{k}.{j}"), rect: r, lit: shape.clone(), labels: labels.clone() });
            }
        }
        for v in (0..self.regions.len()).filter(|&v| drawn(v)) {
            let mine: Vec<&Trace> = self.traces.iter().filter(|t| t.ends.0 == v || t.ends.1 == v).collect();
            if mine.is_empty() {
                continue;
            }
            let mut shape = mine.iter().flat_map(|t| &t.ops).fold(Shape::new(), &paint);
            if reach == Hover::Neighbours {
                let mut near: Vec<usize> = mine.iter().flat_map(|t| [t.ends.0, t.ends.1]).filter(|&u| drawn(u)).collect();
                near.sort_unstable();
                near.dedup();
                shape = near.into_iter().fold(shape, |s, u| paint(s, &outline(u)));
            }
            let (lo, hi) = bounds(&self.regions[v]);
            let labels = mine.iter().filter_map(|t| t.label.map(|l| l.0)).collect();
            spots.push(Spot { key: format!("node{v}"), rect: rect(lo, hi), lit: shape, labels });
        }
        spots
    }
}

fn bounds(pts: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    pts.iter().fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]))
}

/// Rectangles (`[x, y, w, h]`) covering `line` to `slop` either side: a run along it grows while
/// its box stays thin, so straight runs take one rectangle and curves a few; at most [`EDGE_HITS`].
pub(crate) fn line_hits(line: &[[f32; 2]], slop: f32) -> Vec<[f32; 4]> {
    // Points at most a few points apart, so a long diagonal splits too.
    let mut pts: Vec<[f32; 2]> = Vec::new();
    for w in line.windows(2) {
        let n = (dist(w[0], w[1]) / 4.0).ceil().clamp(1.0, 200.0) as usize;
        pts.extend((0..n).map(|i| lerp(w[0], w[1], i as f32 / n as f32)));
    }
    pts.extend(line.last());
    pts.retain(|p| p[0].is_finite() && p[1].is_finite());
    if pts.is_empty() {
        return Vec::new();
    }
    let mut thin = slop;
    loop {
        let mut out = Vec::new();
        let (mut lo, mut hi) = (pts[0], pts[0]);
        for w in pts.windows(2) {
            let p = w[1];
            let (nlo, nhi) = ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]);
            if (nhi[0] - nlo[0]).min(nhi[1] - nlo[1]) > thin {
                out.push([lo[0] - slop, lo[1] - slop, hi[0] - lo[0] + 2.0 * slop, hi[1] - lo[1] + 2.0 * slop]);
                (lo, hi) = ([w[0][0].min(p[0]), w[0][1].min(p[1])], [w[0][0].max(p[0]), w[0][1].max(p[1])]);
            } else {
                (lo, hi) = (nlo, nhi);
            }
        }
        out.push([lo[0] - slop, lo[1] - slop, hi[0] - lo[0] + 2.0 * slop, hi[1] - lo[1] + 2.0 * slop]);
        if out.len() <= EDGE_HITS || thin > 4096.0 {
            return out;
        }
        thin *= 2.0;
    }
}

/// `hits` with every part over one of `nodes` cut away, keeping the biggest side left of each cut
/// and dropping what gets too small to aim at.
pub(crate) fn cut_away(hits: Vec<[f32; 4]>, nodes: &[[f32; 4]]) -> Vec<[f32; 4]> {
    hits.into_iter()
        .filter_map(|r| {
            nodes.iter().try_fold(r, |r, n| {
                let (x0, y0, x1, y1) = (r[0], r[1], r[0] + r[2], r[1] + r[3]);
                let (nx0, ny0, nx1, ny1) = (n[0], n[1], n[0] + n[2], n[1] + n[3]);
                if nx0 >= x1 || nx1 <= x0 || ny0 >= y1 || ny1 <= y0 {
                    return Some(r);
                }
                let sides = [[x0, y0, nx0 - x0, r[3]], [nx1, y0, x1 - nx1, r[3]], [x0, y0, r[2], ny0 - y0], [x0, ny1, r[2], y1 - ny1]];
                sides.into_iter().filter(|s| s[2] >= 4.0 && s[3] >= 4.0).max_by(|a, b| (a[2] * a[3]).total_cmp(&(b[2] * b[3])))
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "hover_tests.rs"]
mod tests;
