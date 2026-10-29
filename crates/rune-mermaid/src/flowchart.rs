//! Flowcharts: the parser for `graph`/`flowchart` sources and their drawing on [`super::layered_draw`].

use std::collections::HashMap;

use rune::Color;

use super::canvas::{Canvas, Scene, fit, measure};
use super::layered::{MAX_MINLEN, Spacing};
use super::layered_draw::{ClusterSpec, Dir, DrawNode, Edge, Head, Line, NodeShape, layered_canvas, node_size};
use super::{
    Cur, DiagramStyle, Hover, EdgeStyle, MAX_ITEMS, NodeStyle, clean, clean_quoted,
    parse_edge_style_props, parse_style_props, statements,
};

#[derive(Debug)]
pub(crate) struct Node {
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    pub cluster: Option<usize>,
    pub style: NodeStyle,
    pub explicit_shape: bool,
}

#[derive(Debug)]
pub(crate) struct Subgraph {
    pub id: Option<String>,
    pub title: String,
    pub parent: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Flowchart {
    pub title: Option<String>,
    pub dir: Dir,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub subgraphs: Vec<Subgraph>,
}

/// `header` is what follows `graph`/`flowchart` on its line; `body` the remaining lines.
pub(crate) fn parse(header: &str, body: &[&str]) -> Option<Flowchart> {
    let (dir, rest) = header.trim_start().split_once(|c: char| c.is_whitespace() || c == ';').unwrap_or((header.trim(), ""));
    let dir = Dir::parse(dir)?;
    let mut p = Parser {
        f: Flowchart { title: None, dir, nodes: Vec::new(), edges: Vec::new(), subgraphs: Vec::new() },
        ids: HashMap::new(),
        stack: Vec::new(),
        class_defs: HashMap::new(),
        node_classes: HashMap::new(),
        node_styles: HashMap::new(),
        default_edge_style: EdgeStyle::default(),
        edge_styles: HashMap::new(),
    };
    for line in std::iter::once(rest).chain(body.iter().copied()) {
        for stmt in statements(line) {
            p.statement(stmt)?;
        }
    }
    if !p.stack.is_empty() {
        return None;
    }
    for (id, &idx) in &p.ids {
        let mut st = NodeStyle::default();
        if let Some(classes) = p.node_classes.get(id) {
            for cls in classes {
                if let Some(&cs) = p.class_defs.get(cls) {
                    st = st.merge(cs);
                }
            }
        }
        if let Some(&inline) = p.node_styles.get(id) {
            st = st.merge(inline);
        }
        p.f.nodes[idx].style = st;
    }
    for (i, e) in p.f.edges.iter_mut().enumerate() {
        let mut st = p.default_edge_style;
        if let Some(&es) = p.edge_styles.get(&i) {
            st = st.merge(es);
        }
        e.style = st;
    }
    resolve_subgraph_endpoints(&mut p.f);
    (!p.f.nodes.is_empty()).then_some(p.f)
}

/// Redirects edges targeting a `subgraph id` placeholder onto that subgraph's first/last member.
fn resolve_subgraph_endpoints(f: &mut Flowchart) {
    let mut redirects: HashMap<usize, (usize, usize)> = HashMap::new();
    for (sg_idx, sg) in f.subgraphs.iter().enumerate() {
        let Some(id) = &sg.id else { continue };
        let Some(node_idx) = f.nodes.iter().position(|n| &n.id == id && !n.explicit_shape) else { continue };
        let members: Vec<usize> = (0..f.nodes.len())
            .filter(|&i| i != node_idx && in_cluster(i, sg_idx, &f.nodes, &f.subgraphs))
            .collect();
        if let (Some(&first), Some(&last)) = (members.first(), members.last()) {
            redirects.insert(node_idx, (first, last));
        }
    }
    if redirects.is_empty() {
        return;
    }
    for e in &mut f.edges {
        if let Some(&(_, last)) = redirects.get(&e.from) {
            e.from = last;
        }
        if let Some(&(first, _)) = redirects.get(&e.to) {
            e.to = first;
        }
    }
    let mut keep = Vec::with_capacity(f.nodes.len());
    let mut remap = vec![0usize; f.nodes.len()];
    for i in 0..f.nodes.len() {
        if !redirects.contains_key(&i) {
            remap[i] = keep.len();
            keep.push(i);
        }
    }
    let mut old = std::mem::take(&mut f.nodes).into_iter().enumerate().collect::<HashMap<_, _>>();
    f.nodes = keep.into_iter().filter_map(|i| old.remove(&i)).collect();
    for e in &mut f.edges {
        e.from = remap[e.from];
        e.to = remap[e.to];
    }
}

fn in_cluster(node_idx: usize, target: usize, nodes: &[Node], subgraphs: &[Subgraph]) -> bool {
    let mut cur = nodes[node_idx].cluster;
    let mut steps = 0;
    while let Some(c) = cur {
        if c == target {
            return true;
        }
        cur = subgraphs[c].parent;
        steps += 1;
        if steps > subgraphs.len() {
            break;
        }
    }
    false
}

struct Parser {
    f: Flowchart,
    ids: HashMap<String, usize>,
    stack: Vec<usize>,
    class_defs: HashMap<String, NodeStyle>,
    node_classes: HashMap<String, Vec<String>>,
    node_styles: HashMap<String, NodeStyle>,
    default_edge_style: EdgeStyle,
    edge_styles: HashMap<usize, EdgeStyle>,
}

const MAX_DEPTH: usize = 8;

struct Link {
    line: Line,
    start: Head,
    end: Head,
    minlen: usize,
    label: Option<String>,
}

impl Parser {
    fn budget(&self) -> Option<()> {
        (self.f.nodes.len() + self.f.edges.len() + self.f.subgraphs.len() < MAX_ITEMS).then_some(())
    }

    fn statement(&mut self, s: &str) -> Option<()> {
        let word = s.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match word {
            "subgraph" => {
                let rest = s["subgraph".len()..].trim();
                let (sg_id, title) = match rest.find('[') {
                    Some(i) if rest.ends_with(']') => {
                        let raw_id = rest[..i].trim();
                        let id = (!raw_id.is_empty()).then(|| raw_id.to_owned());
                        (id, clean_quoted(&rest[i + 1..rest.len() - 1]))
                    }
                    _ => {
                        let id = (!rest.starts_with('"') && !rest.contains(char::is_whitespace) && !rest.is_empty()).then(|| rest.to_owned());
                        (id, clean_quoted(rest))
                    }
                };
                self.budget()?;
                if self.stack.len() >= MAX_DEPTH {
                    return None;
                }
                self.f.subgraphs.push(Subgraph { id: sg_id, title, parent: self.stack.last().copied() });
                self.stack.push(self.f.subgraphs.len() - 1);
            }
            "end" if s == "end" => {
                self.stack.pop()?;
            }
            "title" => {
                let t = clean_quoted(s["title".len()..].trim());
                if !t.is_empty() {
                    self.f.title = Some(t);
                }
            }
            "direction" => {
                let rest = s["direction".len()..].trim();
                if self.stack.is_empty() && self.f.edges.is_empty()
                    && let Some(d) = Dir::parse(rest)
                {
                    self.f.dir = d;
                }
            }
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
                        self.node_classes.entry(id.to_owned()).or_default().extend(cls_list.iter().cloned());
                    }
                }
            }
            "style" => {
                let rest = s["style".len()..].trim();
                if let Some((targets, props)) = rest.split_once(char::is_whitespace) {
                    let st = parse_style_props(props);
                    for id in targets.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        let cur = self.node_styles.get(id).copied().unwrap_or_default();
                        self.node_styles.insert(id.to_owned(), cur.merge(st));
                    }
                }
            }
            "linkStyle" => {
                let rest = s["linkStyle".len()..].trim();
                if let Some((targets, props)) = rest.split_once(char::is_whitespace) {
                    let st = parse_edge_style_props(props);
                    for tok in targets.split(',').map(str::trim) {
                        if tok == "default" {
                            self.default_edge_style = self.default_edge_style.merge(st);
                        } else if let Ok(idx) = tok.parse::<usize>() {
                            let cur = self.edge_styles.get(&idx).copied().unwrap_or_default();
                            self.edge_styles.insert(idx, cur.merge(st));
                        }
                    }
                }
            }
            "click" | "accTitle" | "accDescr" => {}
            _ => self.chain(s)?,
        }
        Some(())
    }

    fn chain(&mut self, s: &str) -> Option<()> {
        let mut c = Cur::new(s);
        let mut left = self.group(&mut c)?;
        loop {
            c.skip_ws();
            if c.done() {
                return Some(());
            }
            let link = link(&mut c)?;
            c.skip_ws();
            let right = self.group(&mut c)?;
            for &from in &left {
                for &to in &right {
                    self.budget()?;
                    self.f.edges.push(Edge {
                        from,
                        to,
                        label: link.label.clone(),
                        line: link.line,
                        start: link.start,
                        end: link.end,
                        minlen: link.minlen,
                        style: EdgeStyle::default(),
                    });
                }
            }
            left = right;
        }
    }

    fn group(&mut self, c: &mut Cur) -> Option<Vec<usize>> {
        let mut v = vec![self.node(c)?];
        loop {
            c.skip_ws();
            if !c.eat("&") {
                return Some(v);
            }
            c.skip_ws();
            v.push(self.node(c)?);
        }
    }

    fn node(&mut self, c: &mut Cur) -> Option<usize> {
        let id = ident(c)?;
        let shape = shape(c)?;
        if c.eat(":::") {
            let cls = ident(c)?;
            self.node_classes.entry(id.clone()).or_default().push(cls);
        }
        let cluster = self.stack.last().copied();
        let i = match self.ids.get(&id) {
            Some(&i) => i,
            None => {
                self.budget()?;
                self.f.nodes.push(Node {
                    label: id.clone(),
                    id: id.clone(),
                    shape: NodeShape::Rect,
                    cluster,
                    style: NodeStyle::default(),
                    explicit_shape: false,
                });
                self.ids.insert(id, self.f.nodes.len() - 1);
                self.f.nodes.len() - 1
            }
        };
        let n = &mut self.f.nodes[i];
        if n.cluster.is_none() {
            n.cluster = cluster;
        }
        if let Some((label, shape)) = shape {
            if let Some(l) = label {
                n.label = l;
            }
            n.shape = shape;
            n.explicit_shape = true;
        }
        Some(i)
    }
}

fn ident(c: &mut Cur) -> Option<String> {
    let start = c.i;
    while let Some(ch) = c.peek() {
        let word = |ch: char| ch.is_alphanumeric() || ch == '_';
        if word(ch) || (ch == '-' && c.i > start && c.peek2().is_some_and(word)) {
            c.bump();
        } else {
            break;
        }
    }
    (c.i > start).then(|| c.s[start..c.i].to_owned())
}

const SHAPES: &[(&str, &str, NodeShape)] = &[
    ("(((", ")))", NodeShape::Circle),
    ("([", "])", NodeShape::Stadium),
    ("[[", "]]", NodeShape::Subroutine),
    ("[(", ")]", NodeShape::Cylinder),
    ("((", "))", NodeShape::Circle),
    ("{{", "}}", NodeShape::Hexagon),
    (">", "]", NodeShape::Asymmetric),
    ("[", "]", NodeShape::Rect),
    ("(", ")", NodeShape::Round),
    ("{", "}", NodeShape::Diamond),
];

fn find_close(s: &str, open: &str, close: &str) -> Option<usize> {
    let nested = open.len() == 1 && close.len() == 1 && open != ">";
    let (mut depth, mut quoted, mut escaped) = (1usize, false, false);
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let ch = rest.chars().next()?;
        if escaped {
            escaped = false;
            i += ch.len_utf8();
            continue;
        }
        if quoted {
            match ch {
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            i += ch.len_utf8();
            continue;
        }
        if ch == '"' {
            quoted = true;
            i += 1;
            continue;
        }
        if rest.starts_with(close) {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
            i += close.len();
            continue;
        }
        if nested && rest.starts_with(open) {
            depth += 1;
            i += open.len();
            continue;
        }
        i += ch.len_utf8();
    }
    None
}

fn find_quote_end(s: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut escaped = false;
    for (i, ch) in s.char_indices() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some((out, i + 1)),
            _ => out.push(ch),
        }
    }
    None
}

fn find_unquoted(s: &str, needle: &str) -> Option<usize> {
    let (mut quoted, mut escaped) = (false, false);
    for (i, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted {
            match ch {
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
            continue;
        }
        if s[i..].starts_with(needle) {
            return Some(i);
        }
    }
    None
}

/// Mermaid v11 `@{ shape: cyl, label: "Postgres" }` node metadata.
fn parse_at_shape(c: &mut Cur) -> Option<(Option<String>, NodeShape)> {
    c.eat("@{");
    let end = find_close(c.rest(), "{", "}")?;
    let inner = &c.rest()[..end];
    let mut kind = NodeShape::Rect;
    let mut label = None;
    let mut saw_key = false;
    for part in split_top_commas(inner) {
        let (k, v) = part.split_once(':')?;
        saw_key = true;
        let v = v.trim();
        match k.trim() {
            "shape" => {
                kind = match v.trim_matches('"').to_ascii_lowercase().as_str() {
                    "rect" | "proc" | "process" | "doc" | "card" => NodeShape::Rect,
                    "rounded" | "event" => NodeShape::Round,
                    "stadium" | "pill" | "terminal" => NodeShape::Stadium,
                    "subproc" | "subprocess" | "subroutine" | "fr-rect" => NodeShape::Subroutine,
                    "cyl" | "cylinder" | "db" | "database" | "lin-cyl" => NodeShape::Cylinder,
                    "circle" | "circ" | "sm-circ" | "dbl-circ" | "f-circ" => NodeShape::Circle,
                    "diam" | "diamond" | "decision" => NodeShape::Diamond,
                    "hex" | "hexagon" | "prepare" => NodeShape::Hexagon,
                    "lean-r" | "lean-l" | "in-out" | "out-in" | "parallelogram" => NodeShape::Parallelogram,
                    "trap-b" | "trap-t" | "trapezoid" | "manual" => NodeShape::Trapezoid,
                    "odd" | "flag" => NodeShape::Asymmetric,
                    _ => return None,
                };
            }
            "label" => label = Some(clean_quoted(v)),
            _ => {}
        }
    }
    if !saw_key {
        return None;
    }
    c.i += end + 1;
    Some((label, kind))
}

fn split_top_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                let seg = s[start..i].trim();
                if !seg.is_empty() {
                    out.push(seg);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

/// A node's bracketed label: `Some(None)` when there is none, `None` when it is malformed.
fn shape(c: &mut Cur) -> Option<Option<(Option<String>, NodeShape)>> {
    if c.rest().starts_with("@{") {
        return parse_at_shape(c).map(Some);
    }
    for open in ["[/", "[\\"] {
        if c.rest().starts_with(open) {
            c.eat(open);
            let rest = c.rest();
            let end = find_unquoted(rest, "/]").into_iter().chain(find_unquoted(rest, "\\]")).min()?;
            let same = rest[end..].starts_with(&open[1..]);
            let label = clean_quoted(&rest[..end]);
            c.i += end + 2;
            return Some(Some((Some(label), if same { NodeShape::Parallelogram } else { NodeShape::Trapezoid })));
        }
    }
    let Some(&(open, close, kind)) = SHAPES.iter().find(|(open, ..)| c.rest().starts_with(open)) else {
        return Some(None);
    };
    c.eat(open);
    c.skip_ws();
    let label = if c.peek() == Some('"') {
        c.bump();
        let (label, used) = find_quote_end(c.rest())?;
        c.i += used;
        c.skip_ws();
        if !c.eat(close) {
            return None;
        }
        label
    } else {
        let end = find_close(c.rest(), open, close)?;
        let label = c.rest()[..end].to_owned();
        c.i += end + close.len();
        label
    };
    Some(Some((Some(clean(&label)), kind)))
}

fn run(c: &mut Cur) -> String {
    let start = c.i;
    while c.peek().is_some_and(|ch| matches!(ch, '-' | '=' | '.' | '~')) {
        c.bump();
    }
    c.s[start..c.i].to_owned()
}

fn end_head(c: &mut Cur) -> Head {
    match c.peek() {
        Some('>') => {
            c.bump();
            Head::Arrow
        }
        Some(ch @ ('o' | 'x')) if c.peek2().is_none_or(|n| n.is_whitespace() || n == '|') => {
            c.bump();
            if ch == 'o' { Head::Circle } else { Head::Cross }
        }
        _ => Head::None,
    }
}

fn link(c: &mut Cur) -> Option<Link> {
    let start = if c.eat("<") {
        Head::Arrow
    } else if matches!(c.peek(), Some('o' | 'x')) && matches!(c.peek2(), Some('-' | '=')) {
        let ch = c.peek();
        c.bump();
        if ch == Some('o') { Head::Circle } else { Head::Cross }
    } else {
        Head::None
    };
    let first = run(c);
    if first.len() < 2 {
        return None;
    }
    let mut end = end_head(c);
    let mut body = first.clone();
    let mut label = None;
    if end == Head::None && matches!(first.as_str(), "--" | "==" | "-.") {
        let close = if first == "-." { ".-" } else { &first[..] };
        let at = find_unquoted(c.rest(), close)?;
        let text = clean_quoted(&c.rest()[..at]);
        if text.is_empty() {
            return None;
        }
        label = Some(text);
        c.i += at;
        body = run(c);
        end = end_head(c);
    }
    let line = if body.contains('~') {
        Line::Invisible
    } else if body.contains('=') {
        Line::Thick
    } else if body.contains('.') {
        Line::Dotted
    } else {
        Line::Solid
    };
    let minlen = match line {
        Line::Dotted => body.matches('.').count(),
        _ => {
            let n = body.chars().filter(|&ch| ch == '-' || ch == '=' || ch == '~').count();
            if end == Head::None { n.saturating_sub(2) } else { n.saturating_sub(1) }
        }
    }
    .clamp(1, MAX_MINLEN);
    let save = c.i;
    c.skip_ws();
    if c.eat("|") {
        let at = find_unquoted(c.rest(), "|")?;
        let text = clean_quoted(&c.rest()[..at]);
        c.i += at + 1;
        label = (!text.is_empty()).then_some(text).or(label);
    } else {
        c.i = save;
    }
    Some(Link { line, start, end, minlen, label })
}

const SPACING: Spacing = Spacing { node: 36.0, rank: 44.0, cluster: 14.0 };

pub(crate) fn draw(f: &Flowchart, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Scene> {
    let canvas = draw_canvas(f, paint, hover)?;
    let scale = fit(canvas.width(), paint);
    canvas.finish(scale)
}

/// `f` drawn at full size, before [`Canvas::finish`].
pub(crate) fn draw_canvas(f: &Flowchart, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Canvas> {
    let nodes: Vec<DrawNode> = f
        .nodes
        .iter()
        .map(|n| {
            let label = measure(&n.label, paint.font);
            DrawNode::Shape { shape: n.shape, size: node_size(n.shape, label.size), label, cluster: n.cluster, style: n.style }
        })
        .collect();
    let clusters: Vec<ClusterSpec> = f.subgraphs.iter().map(|s| ClusterSpec { title: s.title.clone(), parent: s.parent }).collect();
    layered_canvas(f.dir, &nodes, &f.edges, &clusters, &SPACING, paint, hover)
}

#[cfg(test)]
#[path = "flowchart_tests.rs"]
mod tests;
