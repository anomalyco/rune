//! State diagrams (`stateDiagram` and `stateDiagram-v2`), laid out on [`super::layered`].

use std::collections::HashMap;

use rune::{Align, TextStyle};

use super::flowchart::{Dir, Edge, Head, Line, NodeShape};
use super::flowchart_draw::{edges, label_box, node_size, outline, title_box};
use super::route::Ctx;
use super::layered::{GCluster, GEdge, Graph, Spacing, layout};
use super::{
    Canvas, DiagramStyle, MAX_ITEMS, Measured, NodeStyle, Scene, clean_quoted, ellipse, fit,
    measure, parse_style_props, room, rounded_rect, statements,
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
    pub dir: Dir,
    pub states: Vec<StateNode>,
    pub edges: Vec<Edge>,
    pub composites: Vec<Composite>,
}

const MAX_DEPTH: usize = 8;

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<StateDiagram> {
    let mut p = Parser {
        d: StateDiagram { dir: Dir::Down, states: Vec::new(), edges: Vec::new(), composites: Vec::new() },
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
            // Concurrent region divider inside a composite state.
            return (!self.stack.is_empty()).then_some(());
        }
        let word = s.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match word {
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
        self.d.edges.push(Edge { from, to, label, line: Line::Solid, start: Head::None, end: Head::Arrow, minlen: 1 });
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
        if let Some(&cid) = self.composite_ids.get(&id) {
            // Edges to a composite state route to its first member when it has one, else a node for it.
            if let Some(member) = self.d.states.iter().position(|st| st.cluster == Some(cid)) {
                return Some(member);
            }
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

struct SizedState {
    label: Measured,
    shape: NodeShape,
    size: [f32; 2],
}

pub(crate) fn draw(d: &StateDiagram, paint: &DiagramStyle) -> Option<Scene> {
    let nodes: Vec<SizedState> = d
        .states
        .iter()
        .map(|s| match s.kind {
            StateKind::Start => SizedState { label: Measured { text: String::new(), size: [0.0, 0.0], wrap: false }, shape: NodeShape::Circle, size: [18.0, 18.0] },
            StateKind::End => SizedState { label: Measured { text: String::new(), size: [0.0, 0.0], wrap: false }, shape: NodeShape::Circle, size: [22.0, 22.0] },
            StateKind::Choice => SizedState { label: Measured { text: String::new(), size: [0.0, 0.0], wrap: false }, shape: NodeShape::Diamond, size: [28.0, 28.0] },
            StateKind::Bar => SizedState { label: Measured { text: String::new(), size: [0.0, 0.0], wrap: false }, shape: NodeShape::Rect, size: [64.0, 8.0] },
            StateKind::Normal => {
                let full = if s.desc.is_empty() { s.label.clone() } else { format!("{}\n{}", s.label, s.desc.join("\n")) };
                let label = measure(&full, paint.font);
                SizedState { size: node_size(NodeShape::Round, label.size), shape: NodeShape::Round, label }
            }
        })
        .collect();
    let edge_font = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
    let labels: Vec<Option<Measured>> = d.edges.iter().map(|e| e.label.as_ref().map(|l| measure(l, edge_font))).collect();
    let title_font = TextStyle { weight: 600.0, ..edge_font };
    let titles: Vec<Measured> = d.composites.iter().map(|c| measure(&c.title, title_font)).collect();
    let mut canvas = draw_in(d, d.dir, &nodes, &labels, &titles, edge_font, title_font, paint)?;
    if canvas.width() > room(paint) && matches!(d.dir, Dir::Right | Dir::Left) {
        let down = draw_in(d, if d.dir == Dir::Right { Dir::Down } else { Dir::Up }, &nodes, &labels, &titles, edge_font, title_font, paint)?;
        if down.width() < canvas.width() {
            canvas = down;
        }
    }
    let scale = fit(canvas.width(), paint);
    canvas.finish(scale)
}

#[allow(clippy::too_many_arguments)]
fn draw_in(
    d: &StateDiagram,
    dir: Dir,
    nodes: &[SizedState],
    labels: &[Option<Measured>],
    titles: &[Measured],
    edge_font: TextStyle,
    title_font: TextStyle,
    paint: &DiagramStyle,
) -> Option<Canvas> {
    let across = matches!(dir, Dir::Right | Dir::Left);
    let ax = |s: [f32; 2]| if across { [s[1], s[0]] } else { s };
    let flip = matches!(dir, Dir::Up | Dir::Left);
    let graph = Graph {
        sizes: nodes.iter().map(|n| ax(n.size)).collect(),
        edges: d
            .edges
            .iter()
            .zip(labels)
            .map(|(e, l)| {
                let (from, to) = if flip { (e.to, e.from) } else { (e.from, e.to) };
                GEdge { from, to, label: l.as_ref().map(|m| ax(label_box(m))), minlen: e.minlen }
            })
            .collect(),
        node_cluster: d.states.iter().map(|s| s.cluster).collect(),
        clusters: d
            .composites
            .iter()
            .zip(titles)
            .map(|(c, t)| {
                let lead = if t.text.is_empty() { 0.0 } else { t.size[1] + 10.0 };
                GCluster { parent: c.parent, lead: if across { [lead, 0.0] } else { [0.0, lead] } }
            })
            .collect(),
    };
    let placed = layout(&graph, &SPACING)?;
    let pt = |p: [f32; 2]| if across { [p[1], p[0]] } else { p };
    let mut c = Canvas::default();
    c.hover(paint);

    let depth = |mut i: usize| {
        let mut dep = 0;
        while let Some(p) = d.composites[i].parent {
            i = p;
            dep += 1;
            if dep > d.composites.len() {
                break;
            }
        }
        dep
    };
    let mut title_boxes = Vec::new();
    let mut order: Vec<usize> = (0..d.composites.len()).collect();
    order.sort_by_key(|&i| depth(i));
    for i in order {
        let b = placed.clusters[i];
        if b == [0.0; 4] {
            continue;
        }
        let (p0, p1) = (pt([b[0], b[1]]), pt([b[2], b[3]]));
        c.shape(rounded_rect(p0[0], p0[1], p1[0], p1[1], 8.0), paint.cluster_fill, paint.node_border, 1.0);
        c.label_at(&titles[i], [p0[0] + 10.0, p0[1] + 7.0], title_font, paint.muted, Align::Start);
        title_boxes.push(title_box(&titles[i], p0));
    }

    let centers: Vec<[f32; 2]> = placed.centers.iter().map(|&p| pt(p)).collect();
    let sizes: Vec<[f32; 2]> = nodes.iter().map(|n| n.size).collect();
    let ends = d.edges.iter().map(|e| (e.from, e.to)).collect();
    let boxes = labels.iter().map(|l| l.as_ref().map(|m| ax(label_box(m)))).collect();
    let ctx = Ctx::new(&placed, &sizes, nodes.iter().map(|n| n.shape).collect(), boxes, across, ends).titles(title_boxes);
    edges(&mut c, &ctx, &d.edges, labels, flip, edge_font, paint);

    for (i, s) in d.states.iter().enumerate() {
        let (ctr, size) = (centers[i], nodes[i].size);
        c.region(i, outline(nodes[i].shape, ctr, size));
        match s.kind {
            StateKind::Start => c.fill(ellipse(ctr, 8.0, 8.0), paint.line),
            StateKind::End => {
                c.shape(ellipse(ctr, 10.0, 10.0), paint.node_fill, paint.line, 1.5);
                c.fill(ellipse(ctr, 5.5, 5.5), paint.line);
            }
            StateKind::Choice => c.shape(outline(NodeShape::Diamond, ctr, size), paint.node_fill, paint.node_border, 1.0),
            StateKind::Bar => c.fill(rounded_rect(ctr[0] - size[0] / 2.0, ctr[1] - size[1] / 2.0, ctr[0] + size[0] / 2.0, ctr[1] + size[1] / 2.0, 2.0), paint.line),
            StateKind::Normal => {
                let (fill, border, text_color) = s.style.resolve(paint);
                c.shape(outline(NodeShape::Round, ctr, size), fill, border, 1.0);
                c.label(&nodes[i].label, ctr, paint.font, text_color, Align::Center);
            }
        }
    }
    Some(c)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
