//! A diagram's drawing split into the things it shows (each node, edge and frame), so the
//! finished scene keys each by what it is: a diagram streaming in brings each new node, edge and
//! frame in on its own while the ones already there glide to their new places, and a finished
//! diagram draws the same pieces at once.

use std::collections::HashMap;

use rune::{Shape, Transition, canvas};

use super::canvas::{Canvas, Op};
use super::layered_draw::Edge;

/// What a run of drawing is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A node, a participant, a note: fades and grows in.
    Node,
    /// A connector or message: draws on from its source to its target.
    Edge,
    /// A subgraph, a frame, a band, lifelines: fades in.
    Frame,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Kind::Node => "n:",
            Kind::Edge => "e:",
            Kind::Frame => "g:",
        }
    }

    /// How a piece of this kind enters while its diagram streams in.
    pub(crate) fn enter(self) -> Transition {
        match self {
            Kind::Node => Transition::scale(0.96),
            Kind::Edge => Transition::draw(),
            Kind::Frame => Transition::opacity(),
        }
    }

    /// How a label of this kind enters: an edge's after its line is under way.
    pub(crate) fn label_enter(self) -> (Transition, f32) {
        match self {
            Kind::Node => (Transition::scale(0.96), 0.0),
            Kind::Edge => (Transition::opacity(), 0.12),
            Kind::Frame => (Transition::opacity(), 0.0),
        }
    }
}

/// Which thing a run of drawing belongs to: the `i`th of its kind (named by [`Names`], so a
/// flowchart node keeps its key when nodes before it come and go), or one named outright.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Part {
    Index(Kind, usize),
    Key(Kind, String),
}

/// What the `i`th node, edge and frame are called (its Mermaid id). Unnamed ones go by index.
#[derive(Clone, Default)]
pub(crate) struct Names {
    pub nodes: Vec<String>,
    pub edges: Vec<String>,
    pub frames: Vec<String>,
}

impl Names {
    /// Nodes and frames named as given, edges by their ends' names (`A>B`, then `A>B~1` for a
    /// second edge between the same two).
    pub(crate) fn new(nodes: Vec<String>, edges: &[Edge], frames: Vec<String>) -> Names {
        let id = |i: usize| nodes.get(i).cloned().unwrap_or_else(|| format!("#{i}"));
        let mut seen: HashMap<(usize, usize), usize> = HashMap::new();
        let edges = edges
            .iter()
            .map(|e| {
                let n = seen.entry((e.from, e.to)).or_insert(0);
                *n += 1;
                let name = format!("{}>{}", id(e.from), id(e.to));
                if *n == 1 { name } else { format!("{name}~{}", *n - 1) }
            })
            .collect();
        Names { nodes, edges, frames }
    }

    fn key(&self, part: &Part) -> (String, Kind) {
        match part {
            Part::Index(k, i) => {
                let list = match k {
                    Kind::Node => &self.nodes,
                    Kind::Edge => &self.edges,
                    Kind::Frame => &self.frames,
                };
                let name = list.get(*i).cloned().unwrap_or_else(|| format!("#{i}"));
                (format!("{}{name}", k.prefix()), *k)
            }
            Part::Key(k, s) => (format!("{}{s}", k.prefix()), *k),
        }
    }
}

/// Where a part starts: the first op and label drawn for it (`None`: drawing that is no part).
pub(crate) type Mark = (usize, usize, Option<Part>);

impl Canvas {
    /// What is drawn from here on belongs to `part`, until the next call (`None` ends it).
    pub(crate) fn part(&mut self, part: Option<Part>) {
        self.marks.push((self.ops.len(), self.labels.len(), part));
    }

    /// What `part` is keyed by, in its pieces and its hover groups alike.
    pub(crate) fn part_key(&self, part: &Part) -> (String, Kind) {
        let (key, kind) = self.names.key(part);
        // Turned on its side, every edge takes a new route: a diagram streaming in glides its
        // nodes to their new places and draws its edges on afresh, rather than bending the old
        // lines into the new ones.
        (if self.turned && kind == Kind::Edge { format!("{key}|v") } else { key }, kind)
    }

    /// The part drawn now: the one the last [`Canvas::part`] named.
    pub(crate) fn current_part(&self) -> Option<Part> {
        self.marks.last().and_then(|m| m.2.clone())
    }

    /// The runs of drawing, in order: each run's part, its ops and its labels.
    pub(crate) fn runs(&self) -> Vec<(Option<&Part>, std::ops::Range<usize>, std::ops::Range<usize>)> {
        let mut out = Vec::new();
        let mut at = (0, 0, None);
        for (o, l, p) in self.marks.iter().map(|(o, l, p)| (*o, *l, p.as_ref())).chain([(self.ops.len(), self.labels.len(), None)]) {
            if o > at.0 || l > at.1 {
                out.push((at.2, at.0..o, at.1..l));
            }
            at = (o, l, p);
        }
        out
    }
}

/// One keyed piece of a finished scene: its own shape, placed at `at` and `size` big, in the
/// diagram's finished points.
pub(crate) struct Piece {
    pub key: String,
    pub kind: Option<Kind>,
    pub at: [f32; 2],
    pub size: [f32; 2],
    pub shape: Shape,
}

/// Each label's key and kind, beside [`super::canvas::Scene::labels`].
pub(crate) type LabelKey = (String, Option<Kind>);

/// Splits `c`'s finished ops (already moved by `map`) into keyed pieces, and keys its labels by
/// the part each belongs to.
pub(crate) fn pieces(c: &Canvas, map: impl Fn([f32; 2]) -> [f32; 2]) -> (Vec<Piece>, Vec<LabelKey>) {
    let mut pieces = Vec::new();
    let mut label_keys = vec![(String::new(), None); c.labels.len()];
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut rest_labels = 0;
    let mut rest_runs = 0;
    for (part, ops, labels) in c.runs() {
        let (key, kind) = match part {
            Some(p) => {
                let (key, kind) = c.part_key(p);
                (key, Some(kind))
            }
            None => {
                rest_runs += 1;
                (if rest_runs == 1 { "shape".to_owned() } else { format!("shape{rest_runs}") }, None)
            }
        };
        let n = seen.entry(key.clone()).or_insert(0);
        let run_key = if *n == 0 { key } else { format!("{key}~{n}") };
        *n += 1;
        for (j, i) in labels.enumerate() {
            label_keys[i] = match kind {
                Some(k) => (format!("{run_key}.t{j}"), Some(k)),
                None => {
                    rest_labels += 1;
                    (format!("t{}", rest_labels - 1), None)
                }
            };
        }
        if let Some(piece) = piece(&c.ops[ops], run_key, kind, &map) {
            pieces.push(piece);
        }
    }
    (pieces, label_keys)
}

fn piece(ops: &[Op], key: String, kind: Option<Kind>, map: &impl Fn([f32; 2]) -> [f32; 2]) -> Option<Piece> {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    let mut add = |p: [f32; 2], r: f32| b = [b[0].min(p[0] - r), b[1].min(p[1] - r), b[2].max(p[0] + r), b[3].max(p[1] + r)];
    for op in ops {
        match op {
            Op::Fill(points, _) => points.iter().for_each(|&p| add(map(p), 0.0)),
            // A point beyond the stroke's half width for its round caps and antialiasing.
            Op::Stroke { points, width, .. } => points.iter().for_each(|&p| add(map(p), width / 2.0 + 1.0)),
        }
    }
    if b[0] > b[2] {
        return None;
    }
    let local = |p: [f32; 2]| {
        let q = map(p);
        [q[0] - b[0], q[1] - b[1]]
    };
    let shape = ops.iter().fold(Shape::new(), |s, op| match op {
        Op::Fill(points, color) => s.fill(points.iter().map(|&p| local(p)).collect::<Vec<_>>(), *color),
        Op::Stroke { points, closed, width, cap, color } => s.stroke_with(points.iter().map(|&p| local(p)).collect::<Vec<_>>(), *closed, *width, *cap, *color),
    });
    let size = [(b[2] - b[0]).max(1.0), (b[3] - b[1]).max(1.0)];
    // Coordinates stretch with the piece, so a frame or lifeline that grows springs to its new
    // size instead of jumping.
    Some(Piece { key, kind, at: [b[0], b[1]], size, shape: shape.view_box(size[0], size[1]) })
}

impl Piece {
    /// Placed by offset, not layout: a reply's growth row snaps the layout of what it holds,
    /// but an offset still springs, so a piece glides to its new place there too.
    pub(crate) fn el(&self, streaming: bool, scale: f32) -> rune::El {
        let enter = match self.kind {
            Some(k) if streaming => k.enter(),
            None if streaming => Transition::opacity(),
            _ => Transition::none(),
        };
        canvas(self.shape.clone()).key(self.key.clone()).absolute(0.0, 0.0).offset(self.at[0] * scale, self.at[1] * scale).size(self.size[0] * scale, self.size[1] * scale).transition(enter)
    }
}

#[cfg(test)]
#[path = "parts_tests.rs"]
mod tests;
