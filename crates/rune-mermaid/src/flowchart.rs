//! Flowcharts: the parser for `graph`/`flowchart` sources and their drawing on top of
//! [`super::layered`].

use std::collections::HashMap;

use super::layered::MAX_MINLEN;
use super::{Cur, MAX_ITEMS, NodeStyle, clean, clean_quoted, parse_style_props, statements};

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
pub(crate) enum NodeShape {
    Rect,
    Round,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    Asymmetric,
    Diamond,
    Hexagon,
    Parallelogram,
    Trapezoid,
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
}

#[derive(Debug)]
pub(crate) struct Node {
    #[cfg_attr(not(test), allow(dead_code, reason = "tests check parsing by id"))]
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    pub cluster: Option<usize>,
    pub style: NodeStyle,
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
}

#[derive(Debug)]
pub(crate) struct Subgraph {
    pub title: String,
    pub parent: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Flowchart {
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
        f: Flowchart { dir, nodes: Vec::new(), edges: Vec::new(), subgraphs: Vec::new() },
        ids: HashMap::new(),
        stack: Vec::new(),
        class_defs: HashMap::new(),
        node_classes: HashMap::new(),
        node_styles: HashMap::new(),
        over: false,
    };
    for line in std::iter::once(rest).chain(body.iter().copied()) {
        for stmt in statements(line) {
            let (nodes, edges) = (p.f.nodes.len(), p.f.edges.len());
            if p.statement(stmt).is_none() {
                // Past a size or depth bound the chart falls back; a statement that doesn't
                // parse is dropped whole and the rest still draws.
                if p.over {
                    return None;
                }
                p.f.nodes.truncate(nodes);
                p.f.edges.truncate(edges);
                p.ids.retain(|_, &mut i| i < nodes);
            }
        }
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
    (!p.f.nodes.is_empty()).then_some(p.f)
}

struct Parser {
    f: Flowchart,
    ids: HashMap<String, usize>,
    stack: Vec<usize>,
    class_defs: HashMap<String, NodeStyle>,
    node_classes: HashMap<String, Vec<String>>,
    node_styles: HashMap<String, NodeStyle>,
    /// Set once a size or depth bound is hit, which fails the whole chart.
    over: bool,
}

/// Subgraphs nest at most this deep.
const MAX_DEPTH: usize = 8;

struct Link {
    line: Line,
    start: Head,
    end: Head,
    minlen: usize,
    label: Option<String>,
}

impl Parser {
    /// `None` once the chart already holds [`MAX_ITEMS`] nodes, edges and subgraphs, checked
    /// before each one is added so no single statement can build an unbounded amount.
    fn budget(&mut self) -> Option<()> {
        self.over |= self.f.nodes.len() + self.f.edges.len() + self.f.subgraphs.len() >= MAX_ITEMS;
        (!self.over).then_some(())
    }

    fn statement(&mut self, s: &str) -> Option<()> {
        let word = s.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match word {
            "subgraph" => {
                let rest = s["subgraph".len()..].trim();
                let title = match rest.find('[') {
                    Some(i) if rest.ends_with(']') => clean_quoted(&rest[i + 1..rest.len() - 1]),
                    _ => clean_quoted(rest),
                };
                self.budget()?;
                if self.stack.len() >= MAX_DEPTH {
                    self.over = true;
                    return None;
                }
                self.f.subgraphs.push(Subgraph { title, parent: self.stack.last().copied() });
                self.stack.push(self.f.subgraphs.len() - 1);
            }
            "end" if s == "end" => {
                self.stack.pop()?;
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
            "linkStyle" | "click" | "accTitle" | "accDescr" => {}
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
                self.f.nodes.push(Node { label: id.clone(), id: id.clone(), shape: NodeShape::Rect, cluster, style: NodeStyle::default() });
                self.ids.insert(id, self.f.nodes.len() - 1);
                self.f.nodes.len() - 1
            }
        };
        let n = &mut self.f.nodes[i];
        // A node first used outside a subgraph joins the first subgraph that mentions it.
        if n.cluster.is_none() {
            n.cluster = cluster;
        }
        if let Some((label, shape)) = shape {
            n.label = label;
            n.shape = shape;
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

/// Openers and closers, longest first.
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

/// Finds a closing delimiter outside `"…"` quotes, tracking single-character bracket nesting.
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

/// Finds `needle` outside `"…"` quotes.
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

/// A node's bracketed label: `Some(None)` when there is none, `None` when it is malformed.
fn shape(c: &mut Cur) -> Option<Option<(String, NodeShape)>> {
    if c.rest().starts_with("@{") {
        return None;
    }
    // `[/…/]`, `[\…\]` lean; `[/…\]`, `[\…/]` are trapezoids. With no closing `/]` or `\]`
    // it is a plain `[…]` label that starts with the slash.
    for open in ["[/", "[\\"] {
        if let Some(rest) = c.rest().strip_prefix(open)
            && let Some(end) = find_unquoted(rest, "/]").into_iter().chain(find_unquoted(rest, "\\]")).min()
        {
            c.eat(open);
            let rest = c.rest();
            let same = rest[end..].starts_with(&open[1..]);
            let label = clean_quoted(&rest[..end]);
            c.i += end + 2;
            return Some(Some((label, if same { NodeShape::Parallelogram } else { NodeShape::Trapezoid })));
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
    Some(Some((clean(&label), kind)))
}

fn run(c: &mut Cur) -> String {
    let start = c.i;
    while c.peek().is_some_and(|ch| matches!(ch, '-' | '=' | '.' | '~')) {
        c.bump();
    }
    c.s[start..c.i].to_owned()
}

/// A head right after a link's body: `>`, or `o`/`x` when no identifier follows.
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
        // `-- text -->`: the text runs to the closing body outside quotes.
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

#[cfg(test)]
#[path = "flowchart_tests.rs"]
mod tests;
