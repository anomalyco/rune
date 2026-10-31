//! State diagrams (`stateDiagram` and `stateDiagram-v2`), laid out on [`super::layered_draw`].

use std::collections::HashMap;

use rune::Color;

use super::canvas::{Measured, Scene, measure};
use super::layered::Spacing;
use super::layered_draw::{ClusterSpec, Dir, DrawNode, Edge, Head, Line, NodeShape, draw_layered, node_size};
use super::{
    DiagramStyle, EdgeStyle, Hover, MAX_DEPTH, MAX_ITEMS, NodeStyle, StyleSheet, clean_quoted, statements,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateKind {
    Normal,
    Start,
    End,
    Choice,
    Bar,
    Note,
}

#[derive(Debug)]
pub(crate) struct StateNode {
    #[cfg_attr(not(test), allow(dead_code, reason = "tests check parsing by id"))]
    pub id: String,
    pub label: String,
    pub desc: Vec<String>,
    pub kind: StateKind,
    pub cluster: Option<usize>,
    pub style: NodeStyle,
}

#[derive(Debug)]
pub(crate) struct Composite {
    pub title: String,
    pub parent: Option<usize>,
    /// One of a composite's concurrent regions, split by `--`.
    pub region: bool,
}

#[derive(Debug)]
pub(crate) struct StateDiagram {
    pub title: Option<String>,
    pub dir: Dir,
    pub states: Vec<StateNode>,
    pub edges: Vec<Edge>,
    pub composites: Vec<Composite>,
}

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<StateDiagram> {
    let mut p = Parser {
        d: StateDiagram { title: None, dir: Dir::Down, states: Vec::new(), edges: Vec::new(), composites: Vec::new() },
        ids: HashMap::new(),
        composite_ids: HashMap::new(),
        stack: Vec::new(),
        note: None,
        pseudo_seq: 0,
        styles: StyleSheet::default(),
        over: false,
    };
    // A statement that doesn't read is skipped; one past the bounds leaves the whole diagram as source.
    for line in std::iter::once(header).chain(body.iter().copied()) {
        if p.note.is_some() {
            if p.note_line(line).is_none() && p.over {
                return None;
            }
            continue;
        }
        for stmt in statements(line) {
            let (states, edges) = (p.d.states.len(), p.d.edges.len());
            if p.statement(stmt).is_none() {
                if p.over {
                    return None;
                }
                // Whatever the statement added before it failed goes too.
                p.d.states.truncate(states);
                p.d.edges.truncate(edges);
                p.ids.retain(|_, &mut i| i < states);
            }
        }
    }
    // A note left open closes at the end.
    if p.note.is_some() && p.note_line("end note").is_none() && p.over {
        return None;
    }
    for (id, &idx) in &p.ids {
        p.d.states[idx].style = p.styles.get(id);
    }
    p.resolve_composites();
    (!p.d.states.is_empty()).then_some(p.d)
}

struct Parser {
    d: StateDiagram,
    ids: HashMap<String, usize>,
    composite_ids: HashMap<String, usize>,
    stack: Vec<usize>,
    /// A multi-line note being read: its state, side and lines so far.
    note: Option<(String, Side, Vec<String>)>,
    pseudo_seq: usize,
    styles: StyleSheet,
    /// Set once the source goes past [`MAX_ITEMS`] or [`MAX_DEPTH`].
    over: bool,
}

impl Parser {
    fn budget(&mut self) -> Option<()> {
        self.over |= self.d.states.len() + self.d.edges.len() + self.d.composites.len() >= MAX_ITEMS;
        (!self.over).then_some(())
    }

    fn note_line(&mut self, line: &str) -> Option<()> {
        let line = line.trim();
        if !line.eq_ignore_ascii_case("end note") {
            if let Some((_, _, lines)) = &mut self.note {
                lines.push(line.to_owned());
            }
            return Some(());
        }
        let (target, side, lines) = self.note.take()?;
        self.add_note(&target, side, &lines.join("\n"))
    }

    fn statement(&mut self, s: &str) -> Option<()> {
        if s == "}" {
            let top = self.stack.pop()?;
            if self.d.composites[top].region {
                self.stack.pop();
            }
            return Some(());
        }
        if s == "--" {
            return self.divide();
        }
        let word = s.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match word {
            "title" => {
                let t = clean_quoted(s["title".len()..].trim());
                if !t.is_empty() {
                    self.d.title = Some(t);
                }
            }
            "direction" => {
                let rest = s["direction".len()..].trim();
                let dir = Dir::parse(rest)?;
                if self.stack.is_empty() && self.d.edges.is_empty() {
                    self.d.dir = dir;
                }
            }
            "note" | "Note" => self.note(s[word.len()..].trim())?,
            "hide" | "accTitle" | "accDescr" => {}
            "classDef" => {
                let _ = self.styles.class_def(&s["classDef".len()..]);
            }
            "class" => {
                let _ = self.styles.assign_classes(&s["class".len()..]);
            }
            "style" => {
                let _ = self.styles.style(&s["style".len()..]);
            }
            "state" => self.state_decl(s["state".len()..].trim())?,
            _ if s.contains("-->") => self.transition(s)?,
            _ if s.contains(':') => self.description(s)?,
            _ => {
                if !valid_state_id(s) {
                    return None;
                }
                self.ensure_state(s)?;
            }
        }
        Some(())
    }

    fn state_decl(&mut self, rest: &str) -> Option<()> {
        let opens = rest.ends_with('{');
        let core = if opens { rest[..rest.len() - 1].trim() } else { rest };
        if let Some((id_part, stereo)) = core.split_once("<<") {
            let stereo = stereo.strip_suffix(">>")?.trim().to_ascii_lowercase();
            let kind = match stereo.as_str() {
                "choice" => StateKind::Choice,
                "fork" | "join" => StateKind::Bar,
                _ => StateKind::Normal,
            };
            let id = id_part.trim();
            if !valid_state_id(id) {
                return None;
            }
            let idx = self.ensure_state(id)?;
            self.d.states[idx].kind = kind;
            return Some(());
        }
        let named = name_and_label(core).filter(|(id, _)| valid_state_id(id));
        if opens {
            self.budget()?;
            if self.stack.len() >= MAX_DEPTH {
                self.over = true;
                return None;
            }
            // A composite whose name doesn't read still opens, so its `}` closes it and not its parent.
            let title = named.as_ref().map_or_else(|| clean_quoted(core), |(_, l)| l.clone());
            self.d.composites.push(Composite { title, parent: self.stack.last().copied(), region: false });
            let cid = self.d.composites.len() - 1;
            if let Some((id, _)) = named {
                self.composite_ids.insert(id.to_owned(), cid);
            }
            self.stack.push(cid);
        } else {
            let (id, label) = named?;
            let idx = self.ensure_state(id)?;
            self.d.states[idx].label = label;
        }
        Some(())
    }

    fn transition(&mut self, s: &str) -> Option<()> {
        let arrow = s.find("-->")?;
        let left_raw = s[..arrow].trim();
        let after = s[arrow + 3..].trim();
        let (right_raw, label) = match after.split_once(':') {
            Some((r, l)) => {
                let text = clean_quoted(l);
                (r.trim(), (!text.is_empty()).then_some(text))
            }
            None => (after, None),
        };
        let from = self.endpoint(left_raw, true)?;
        let to = self.endpoint(right_raw, false)?;
        self.budget()?;
        self.d.edges.push(Edge {
            from: from[0],
            to: to[0],
            label,
            line: Line::Solid,
            start: Head::None,
            end: Head::Arrow,
            minlen: 1,
            style: EdgeStyle::default(),
        });
        self.hold_regions(&from, &to);
        Some(())
    }

    /// Invisible edges that keep a composite's other regions beside the one an edge is drawn to,
    /// for edges that run forward in the source; a back edge (`retry --> Ready`) holds nothing, so
    /// breaking its cycle can't flip a region below what follows the composite.
    fn hold_regions(&mut self, from: &[usize], to: &[usize]) {
        let forward = from.iter().max() < to.iter().min();
        if !forward {
            return;
        }
        let pairs = from[1..].iter().map(|&f| (f, to[0])).chain(to[1..].iter().map(|&t| (from[0], t)));
        for (from, to) in pairs.collect::<Vec<_>>() {
            if self.budget().is_none() {
                return;
            }
            self.d.edges.push(Edge {
                from,
                to,
                label: None,
                line: Line::Invisible,
                start: Head::None,
                end: Head::None,
                minlen: 1,
                style: EdgeStyle::default(),
            });
        }
    }

    /// Where an edge meets composite `cid`: it enters at the first state of each region and leaves
    /// from the last, the first region's being the one drawn.
    fn composite_ends(&self, cid: usize, leaving: bool) -> Vec<usize> {
        let regions: Vec<usize> = (0..self.d.composites.len()).filter(|&r| self.d.composites[r].region && self.d.composites[r].parent == Some(cid)).collect();
        let scopes = if regions.is_empty() { vec![cid] } else { regions };
        let pick = |scope: usize| {
            let mut members = (0..self.d.states.len()).filter(|&i| self.d.states[i].kind != StateKind::Note && self.within(self.d.states[i].cluster, scope));
            if leaving { members.filter(|&i| self.d.states[i].kind != StateKind::Start).last() } else { members.next() }
        };
        scopes.into_iter().filter_map(pick).collect()
    }

    fn description(&mut self, s: &str) -> Option<()> {
        let (id, desc) = s.split_once(':')?;
        let id = self.styles.strip_inline(id)?;
        if !valid_state_id(&id) {
            return None;
        }
        let idx = self.ensure_state(&id)?;
        let text = clean_quoted(desc);
        if !text.is_empty() {
            self.d.states[idx].desc.push(text);
        }
        Some(())
    }

    fn endpoint(&mut self, raw: &str, is_start: bool) -> Option<Vec<usize>> {
        if raw == "[*]" {
            self.budget()?;
            self.pseudo_seq += 1;
            let scope = self.stack.last().copied();
            let key = format!("__pseudo_{}_{}_{:?}", if is_start { "start" } else { "end" }, self.pseudo_seq, scope);
            let kind = if is_start { StateKind::Start } else { StateKind::End };
            self.d.states.push(StateNode { id: key, label: String::new(), desc: Vec::new(), kind, cluster: scope, style: NodeStyle::default() });
            return Some(vec![self.d.states.len() - 1]);
        }
        let id = self.styles.strip_inline(raw)?;
        if !valid_state_id(&id) {
            return None;
        }
        if let Some(&cid) = self.composite_ids.get(&id) {
            let ends = self.composite_ends(cid, is_start);
            if !ends.is_empty() {
                return Some(ends);
            }
        }
        Some(vec![self.ensure_state(&id)?])
    }

    /// A state named before its composite was declared (`[*] --> Busy`, then `state Busy {`) is
    /// that composite: its edges move onto the composite as if named after it, and it goes.
    fn resolve_composites(&mut self) {
        let mut moves: HashMap<usize, (Vec<usize>, Vec<usize>)> = HashMap::new();
        for (id, &cid) in &self.composite_ids {
            let Some(&at) = self.ids.get(id) else { continue };
            if self.within(self.d.states[at].cluster, cid) {
                continue;
            }
            let (enter, leave) = (self.composite_ends(cid, false), self.composite_ends(cid, true));
            if !enter.is_empty() && !leave.is_empty() {
                moves.insert(at, (enter, leave));
            }
        }
        if moves.is_empty() {
            return;
        }
        for e in 0..self.d.edges.len() {
            let (from, to) = (self.d.edges[e].from, self.d.edges[e].to);
            let from = moves.get(&from).map_or_else(|| vec![from], |m| m.1.clone());
            let to = moves.get(&to).map_or_else(|| vec![to], |m| m.0.clone());
            (self.d.edges[e].from, self.d.edges[e].to) = (from[0], to[0]);
            self.hold_regions(&from, &to);
        }
        let mut remap = Vec::with_capacity(self.d.states.len());
        let mut kept = 0;
        for i in 0..self.d.states.len() {
            remap.push(kept);
            kept += usize::from(!moves.contains_key(&i));
        }
        for e in &mut self.d.edges {
            (e.from, e.to) = (remap[e.from], remap[e.to]);
        }
        let mut i = 0;
        self.d.states.retain(|_| {
            i += 1;
            !moves.contains_key(&(i - 1))
        });
        self.ids.clear();
    }

    /// Whether `cluster` is composite `cid` or sits inside it.
    fn within(&self, mut cluster: Option<usize>, cid: usize) -> bool {
        for _ in 0..=MAX_DEPTH {
            match cluster {
                Some(c) if c == cid => return true,
                Some(c) => cluster = self.d.composites[c].parent,
                None => return false,
            }
        }
        false
    }

    /// `--` inside a composite: what came before becomes its first region, and a new one opens.
    fn divide(&mut self) -> Option<()> {
        let &top = self.stack.last()?;
        let parent = if self.d.composites[top].region {
            self.stack.pop();
            self.d.composites[top].parent?
        } else {
            let first = self.region(top)?;
            for st in &mut self.d.states {
                if st.cluster == Some(top) {
                    st.cluster = Some(first);
                }
            }
            for (i, c) in self.d.composites.iter_mut().enumerate() {
                if i != first && c.parent == Some(top) {
                    c.parent = Some(first);
                }
            }
            top
        };
        let next = self.region(parent)?;
        self.stack.push(next);
        Some(())
    }

    fn region(&mut self, parent: usize) -> Option<usize> {
        self.budget()?;
        if self.stack.len() >= MAX_DEPTH {
            self.over = true;
            return None;
        }
        self.d.composites.push(Composite { title: String::new(), parent: Some(parent), region: true });
        Some(self.d.composites.len() - 1)
    }

    /// `right of A : text` on one line, or `left of A` opening a note that `end note` closes.
    fn note(&mut self, rest: &str) -> Option<()> {
        let (place, text) = rest.split_once(':').map_or((rest, None), |(p, t)| (p.trim(), Some(t)));
        let lower = place.to_ascii_lowercase();
        let (side, target) = if let Some(t) = lower.strip_prefix("right of ") {
            (Side::Right, &place[place.len() - t.len()..])
        } else if let Some(t) = lower.strip_prefix("left of ") {
            (Side::Left, &place[place.len() - t.len()..])
        } else {
            return None;
        };
        let target = target.trim();
        if !valid_state_id(target) {
            return None;
        }
        match text {
            Some(t) => self.add_note(target, side, &clean_quoted(t)),
            None => {
                self.note = Some((target.to_owned(), side, Vec::new()));
                Some(())
            }
        }
    }

    /// A note beside `target`, joined to it by a dotted line.
    fn add_note(&mut self, target: &str, side: Side, text: &str) -> Option<()> {
        let at = self.ensure_state(target)?;
        self.budget()?;
        let cluster = self.d.states[at].cluster;
        let id = format!("__note_{}", self.d.states.len());
        self.d.states.push(StateNode { id, label: text.to_owned(), desc: Vec::new(), kind: StateKind::Note, cluster, style: NodeStyle::default() });
        let note = self.d.states.len() - 1;
        let (from, to) = if side == Side::Left { (note, at) } else { (at, note) };
        self.budget()?;
        self.d.edges.push(Edge { from, to, label: None, line: Line::Dotted, start: Head::None, end: Head::None, minlen: 1, style: EdgeStyle::default() });
        Some(())
    }

    fn ensure_state(&mut self, id: &str) -> Option<usize> {
        let cluster = self.stack.last().copied();
        if let Some(&i) = self.ids.get(id) {
            if self.d.states[i].cluster.is_none() {
                self.d.states[i].cluster = cluster;
            }
            return Some(i);
        }
        self.budget()?;
        self.d.states.push(StateNode {
            id: id.to_owned(),
            label: id.to_owned(),
            desc: Vec::new(),
            kind: StateKind::Normal,
            cluster,
            style: NodeStyle::default(),
        });
        let idx = self.d.states.len() - 1;
        self.ids.insert(id.to_owned(), idx);
        Some(idx)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// `"Label" as id`, `Label as id` or a bare `id`.
fn name_and_label(core: &str) -> Option<(&str, String)> {
    if let Some(after_q) = core.strip_prefix('"') {
        let end_q = after_q.find('"')?;
        let label = clean_quoted(&core[..end_q + 2]);
        let id = after_q[end_q + 1..].trim().strip_prefix("as ")?.trim();
        Some((id, label))
    } else if let Some((label, id)) = core.split_once(" as ") {
        Some((id.trim(), clean_quoted(label)))
    } else {
        Some((core.trim(), clean_quoted(core)))
    }
}

fn valid_state_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

const SPACING: Spacing = Spacing { node: 34.0, rank: 42.0, cluster: 14.0 };

pub(crate) fn draw(d: &StateDiagram, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Scene> {
    let nodes: Vec<DrawNode> = d
        .states
        .iter()
        .map(|s| match s.kind {
            StateKind::Start => DrawNode::Start { cluster: s.cluster },
            StateKind::End => DrawNode::End { cluster: s.cluster },
            StateKind::Bar => DrawNode::Bar { cluster: s.cluster },
            StateKind::Choice => DrawNode::Shape {
                shape: NodeShape::Diamond,
                label: Measured::default(),
                size: [28.0, 28.0],
                cluster: s.cluster,
                style: s.style,
            },
            StateKind::Note => {
                let label = measure(&s.label, paint.font);
                DrawNode::Shape { shape: NodeShape::Note, size: node_size(NodeShape::Note, label.size), label, cluster: s.cluster, style: s.style }
            }
            StateKind::Normal => {
                let full = if s.desc.is_empty() { s.label.clone() } else { format!("{}\n{}", s.label, s.desc.join("\n")) };
                let label = measure(&full, paint.font);
                DrawNode::Shape { shape: NodeShape::Round, size: node_size(NodeShape::Round, label.size), label, cluster: s.cluster, style: s.style }
            }
        })
        .collect();
    let clusters: Vec<ClusterSpec> = d.composites.iter().map(|c| ClusterSpec { title: c.title.clone(), parent: c.parent }).collect();
    draw_layered(d.dir, &nodes, &d.edges, &clusters, &SPACING, paint, hover)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
