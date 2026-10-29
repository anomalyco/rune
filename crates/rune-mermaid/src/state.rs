//! State diagrams (`stateDiagram` and `stateDiagram-v2`), laid out on [`super::layered_draw`].

use std::collections::HashMap;

use rune::Color;

use super::canvas::{Measured, Scene, measure};
use super::layered::Spacing;
use super::layered_draw::{ClusterSpec, Dir, DrawNode, Edge, Head, Line, NodeShape, draw_layered, node_size};
use super::{
    DiagramStyle, EdgeStyle, Hover, MAX_ITEMS, NodeStyle, clean_quoted, parse_style_props, statements,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateKind {
    Normal,
    Start,
    End,
    Choice,
    Bar,
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
}

#[derive(Debug)]
pub(crate) struct StateDiagram {
    pub title: Option<String>,
    pub dir: Dir,
    pub states: Vec<StateNode>,
    pub edges: Vec<Edge>,
    pub composites: Vec<Composite>,
}

const MAX_DEPTH: usize = 8;

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<StateDiagram> {
    let mut p = Parser {
        d: StateDiagram { title: None, dir: Dir::Down, states: Vec::new(), edges: Vec::new(), composites: Vec::new() },
        ids: HashMap::new(),
        composite_ids: HashMap::new(),
        stack: Vec::new(),
        in_note: false,
        pseudo_seq: 0,
        class_defs: HashMap::new(),
        state_classes: HashMap::new(),
        state_styles: HashMap::new(),
    };
    for line in std::iter::once(header).chain(body.iter().copied()) {
        for stmt in statements(line) {
            p.statement(stmt)?;
        }
    }
    if p.in_note || !p.stack.is_empty() {
        return None;
    }
    for (id, &idx) in &p.ids {
        let mut st = NodeStyle::default();
        if let Some(classes) = p.state_classes.get(id) {
            for cls in classes {
                if let Some(&cs) = p.class_defs.get(cls) {
                    st = st.merge(cs);
                }
            }
        }
        if let Some(&inline) = p.state_styles.get(id) {
            st = st.merge(inline);
        }
        p.d.states[idx].style = st;
    }
    (!p.d.states.is_empty()).then_some(p.d)
}

struct Parser {
    d: StateDiagram,
    ids: HashMap<String, usize>,
    composite_ids: HashMap<String, usize>,
    stack: Vec<usize>,
    in_note: bool,
    pseudo_seq: usize,
    class_defs: HashMap<String, NodeStyle>,
    state_classes: HashMap<String, Vec<String>>,
    state_styles: HashMap<String, NodeStyle>,
}

impl Parser {
    fn budget(&self) -> Option<()> {
        (self.d.states.len() + self.d.edges.len() + self.d.composites.len() < MAX_ITEMS).then_some(())
    }

    fn statement(&mut self, s: &str) -> Option<()> {
        if self.in_note {
            if s.eq_ignore_ascii_case("end note") {
                self.in_note = false;
            }
            return Some(());
        }
        if s == "}" {
            self.stack.pop()?;
            return Some(());
        }
        if s == "--" {
            return (!self.stack.is_empty()).then_some(());
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
            "note" | "Note" => {
                if !s.contains(':') {
                    self.in_note = true;
                }
            }
            "hide" | "accTitle" | "accDescr" => {}
            "classDef" => {
                let rest = s["classDef".len()..].trim();
                if let Some((names, props)) = rest.split_once(char::is_whitespace) {
                    let st = parse_style_props(props);
                    for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        self.class_defs.insert(name.to_owned(), st);
                    }
                }
            }
            "class" => {
                let rest = s["class".len()..].trim();
                if let Some((targets, classes)) = rest.split_once(char::is_whitespace) {
                    let cls_list: Vec<String> = classes.split(',').map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()).collect();
                    for id in targets.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        self.state_classes.entry(id.to_owned()).or_default().extend(cls_list.iter().cloned());
                    }
                }
            }
            "style" => {
                let rest = s["style".len()..].trim();
                if let Some((targets, props)) = rest.split_once(char::is_whitespace) {
                    let st = parse_style_props(props);
                    for id in targets.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        let cur = self.state_styles.get(id).copied().unwrap_or_default();
                        self.state_styles.insert(id.to_owned(), cur.merge(st));
                    }
                }
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
        let (id, label) = if let Some(after_q) = core.strip_prefix('"') {
            let end_q = after_q.find('"')?;
            let label = clean_quoted(&core[..end_q + 2]);
            let tail = after_q[end_q + 1..].trim();
            let id = tail.strip_prefix("as ")?.trim();
            (id, label)
        } else if let Some((label, id)) = core.split_once(" as ") {
            (id.trim(), clean_quoted(label))
        } else {
            (core.trim(), clean_quoted(core))
        };
        if !valid_state_id(id) {
            return None;
        }
        if opens {
            self.budget()?;
            if self.stack.len() >= MAX_DEPTH {
                return None;
            }
            self.d.composites.push(Composite { title: label, parent: self.stack.last().copied() });
            let cid = self.d.composites.len() - 1;
            self.composite_ids.insert(id.to_owned(), cid);
            self.stack.push(cid);
        } else {
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
            from,
            to,
            label,
            line: Line::Solid,
            start: Head::None,
            end: Head::Arrow,
            minlen: 1,
            style: EdgeStyle::default(),
        });
        Some(())
    }

    fn description(&mut self, s: &str) -> Option<()> {
        let (id, desc) = s.split_once(':')?;
        let id = self.strip_class(id.trim())?;
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

    fn strip_class(&mut self, raw: &str) -> Option<String> {
        match raw.split_once(":::") {
            Some((id, cls)) => {
                let (id, cls) = (id.trim(), cls.trim());
                if id.is_empty() || cls.is_empty() {
                    return None;
                }
                self.state_classes.entry(id.to_owned()).or_default().push(cls.to_owned());
                Some(id.to_owned())
            }
            None => Some(raw.to_owned()),
        }
    }

    fn endpoint(&mut self, raw: &str, is_start: bool) -> Option<usize> {
        if raw == "[*]" {
            self.budget()?;
            self.pseudo_seq += 1;
            let scope = self.stack.last().copied();
            let key = format!("__pseudo_{}_{}_{:?}", if is_start { "start" } else { "end" }, self.pseudo_seq, scope);
            let kind = if is_start { StateKind::Start } else { StateKind::End };
            self.d.states.push(StateNode { id: key, label: String::new(), desc: Vec::new(), kind, cluster: scope, style: NodeStyle::default() });
            return Some(self.d.states.len() - 1);
        }
        let id = self.strip_class(raw)?;
        if !valid_state_id(&id) {
            return None;
        }
        if let Some(&cid) = self.composite_ids.get(&id)
            && let Some(member) = self.d.states.iter().position(|st| st.cluster == Some(cid))
        {
            return Some(member);
        }
        self.ensure_state(&id)
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
