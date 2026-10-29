//! What hovering a flowchart or state diagram node lights up: each node keeps its outline as a
//! region and each edge a copy of its drawing in the hover colour, and the finished scene puts a
//! hover group over every node that shows its edges (and with [`Hover::Neighbours`] the outlines
//! they reach) while the pointer is on it.

use rune::{Color, LineCap, Shape};

use super::{Canvas, DiagramStyle, Op};

/// What hovering a flowchart or state diagram node lights up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hover {
    /// Nothing.
    Off,
    /// The edges in and out of it.
    #[default]
    Edges,
    /// Its edges, its outline and the outlines of the nodes they reach.
    Neighbours,
}

pub(crate) struct Trace {
    pub ends: (usize, usize),
    pub ops: Vec<Op>,
}

impl Canvas {
    /// Lights edges up in `paint`'s hover colour while a node is hovered.
    pub(crate) fn hover(&mut self, paint: &DiagramStyle) {
        self.hover = (paint.hover != Hover::Off).then_some((paint.hover, paint.hover_color));
    }

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
    /// colour turned to the hover colour, a little heavier, for hovering either end.
    pub(crate) fn trace(&mut self, from: usize, to: usize, start: usize, line: Color) {
        let Some((_, lit)) = self.hover else { return };
        let ops = self.ops[start..]
            .iter()
            .map(|op| match op {
                Op::Fill(p, c) if *c == line => Op::Fill(p.clone(), lit),
                Op::Stroke { points, closed, width, cap, color } if *color == line => {
                    Op::Stroke { points: points.clone(), closed: *closed, width: width + 0.5, cap: *cap, color: lit }
                }
                op => op.clone(),
            })
            .collect();
        self.traces.push(Trace { ends: (from, to), ops });
    }

    /// Per hoverable node: its index, where it is (`[x, y, w, h]`, finished) and what lights up.
    pub(crate) fn hovers(&self, map: impl Fn([f32; 2]) -> [f32; 2], paint: impl Fn(Shape, &Op) -> Shape) -> Vec<(usize, [f32; 4], Shape)> {
        let Some((reach, lit)) = self.hover else { return Vec::new() };
        let outline = |v: usize| Op::Stroke { points: self.regions[v].clone(), closed: true, width: 1.0, cap: LineCap::Round, color: lit };
        (0..self.regions.len())
            .filter(|&v| self.regions[v].len() >= 3)
            .filter_map(|v| {
                let mine: Vec<&Trace> = self.traces.iter().filter(|t| t.ends.0 == v || t.ends.1 == v).collect();
                if mine.is_empty() {
                    return None;
                }
                let mut shape = mine.iter().flat_map(|t| &t.ops).fold(Shape::new(), &paint);
                if reach == Hover::Neighbours {
                    let mut near: Vec<usize> = mine.iter().flat_map(|t| [t.ends.0, t.ends.1]).filter(|&u| self.regions.get(u).is_some_and(|r| r.len() >= 3)).collect();
                    near.sort_unstable();
                    near.dedup();
                    shape = near.into_iter().fold(shape, |s, u| paint(s, &outline(u)));
                }
                let pts = &self.regions[v];
                let (lo, hi) = pts.iter().fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
                let (a, z) = (map(lo), map(hi));
                Some((v, [a[0], a[1], z[0] - a[0], z[1] - a[1]], shape))
            })
            .collect()
    }
}
