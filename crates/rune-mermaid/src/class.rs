//! UML class and Rust struct/trait diagrams (`classDiagram`), laid out on [`super::layered_draw`].

use std::collections::HashMap;

use rune::{Color, TextStyle};

use super::canvas::{Measured, Scene, measure_wrap};
use super::layered::Spacing;
use super::layered_draw::{ClusterSpec, Dir, DrawNode, Edge, Head, Line, draw_layered};
use super::{DiagramStyle, EdgeStyle, Hover, MAX_ITEMS, NodeStyle, clean, clean_quoted, parse_style_props, statements};

#[derive(Debug)]
pub(crate) struct ClassNode {
    #[cfg_attr(not(test), allow(dead_code, reason = "tests check parsing by id"))]
    pub id: String,
    pub label: String,
    pub stereo: Option<String>,
    pub fields: Vec<String>,
    pub methods: Vec<String>,
    pub namespace: Option<usize>,
    pub style: NodeStyle,
}

#[derive(Debug)]
pub(crate) struct Namespace {
    pub title: String,
}

#[derive(Debug)]
pub(crate) struct ClassDiagram {
    pub title: Option<String>,
    pub dir: Dir,
    pub classes: Vec<ClassNode>,
    pub edges: Vec<Edge>,
    pub namespaces: Vec<Namespace>,
}

/// Relationship tokens, longest first: `(token, start_head, end_head, line)`.
const RELS: &[(&str, Head, Head, Line)] = &[
    ("<|..", Head::OpenTriangle, Head::None, Line::Dotted),
    ("..|>", Head::None, Head::OpenTriangle, Line::Dotted),
    ("<|--", Head::OpenTriangle, Head::None, Line::Solid),
    ("--|>", Head::None, Head::OpenTriangle, Line::Solid),
    ("*--", Head::DiamondFilled, Head::None, Line::Solid),
    ("--*", Head::None, Head::DiamondFilled, Line::Solid),
    ("o--", Head::DiamondOpen, Head::None, Line::Solid),
    ("--o", Head::None, Head::DiamondOpen, Line::Solid),
    ("<-->", Head::Arrow, Head::Arrow, Line::Solid),
    ("<..>", Head::Arrow, Head::Arrow, Line::Dotted),
    ("-->", Head::None, Head::Arrow, Line::Solid),
    ("<--", Head::Arrow, Head::None, Line::Solid),
    ("..>", Head::None, Head::Arrow, Line::Dotted),
    ("<..", Head::Arrow, Head::None, Line::Dotted),
    ("--", Head::None, Head::None, Line::Solid),
    ("..", Head::None, Head::None, Line::Dotted),
];

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<ClassDiagram> {
    let mut p = Parser {
        d: ClassDiagram { title: None, dir: Dir::Down, classes: Vec::new(), edges: Vec::new(), namespaces: Vec::new() },
        ids: HashMap::new(),
        active_class: None,
        active_ns: None,
        class_defs: HashMap::new(),
        node_classes: HashMap::new(),
        node_styles: HashMap::new(),
        skipping: false,
        members: 0,
        over: false,
    };
    // A statement that doesn't read is skipped (with its `{ … }` body); one past the bounds leaves
    // the whole diagram as source.
    for line in std::iter::once(header).chain(body.iter().copied()) {
        for stmt in statements(line) {
            if p.skipping {
                p.skipping = stmt != "}";
            } else {
                let (classes, edges) = (p.d.classes.len(), p.d.edges.len());
                if p.statement(stmt).is_none() {
                    if p.over {
                        return None;
                    }
                    // Whatever the statement added before it failed goes too.
                    p.d.classes.truncate(classes);
                    p.d.edges.truncate(edges);
                    p.ids.retain(|_, &mut i| i < classes);
                    p.skipping = stmt.ends_with('{');
                }
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
        p.d.classes[idx].style = st;
    }
    (!p.d.classes.is_empty()).then_some(p.d)
}

struct Parser {
    d: ClassDiagram,
    ids: HashMap<String, usize>,
    active_class: Option<usize>,
    active_ns: Option<usize>,
    class_defs: HashMap<String, NodeStyle>,
    node_classes: HashMap<String, Vec<String>>,
    node_styles: HashMap<String, NodeStyle>,
    /// Inside the body of a block that didn't read, until its `}`.
    skipping: bool,
    members: usize,
    /// Set once the source goes past [`MAX_ITEMS`] (or four times that in members).
    over: bool,
}

impl Parser {
    fn budget(&mut self) -> Option<()> {
        self.over |= self.d.classes.len() + self.d.edges.len() + self.d.namespaces.len() >= MAX_ITEMS;
        (!self.over).then_some(())
    }

    fn statement(&mut self, s: &str) -> Option<()> {
        if let Some(idx) = self.active_class {
            if s == "}" {
                self.active_class = None;
                return Some(());
            }
            self.add_member(idx, s);
            return (!self.over).then_some(());
        }
        if s == "}" {
            self.active_ns.take()?;
            return Some(());
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
                let d = Dir::parse(s["direction".len()..].trim())?;
                if self.d.edges.is_empty() {
                    self.d.dir = d;
                }
            }
            "namespace" => {
                if self.active_ns.is_some() {
                    return None;
                }
                let rest = s["namespace".len()..].trim().strip_suffix('{')?.trim();
                self.budget()?;
                self.d.namespaces.push(Namespace { title: clean_quoted(rest) });
                self.active_ns = Some(self.d.namespaces.len() - 1);
            }
            "class" => self.class_decl(s["class".len()..].trim())?,
            "note" | "Note" | "link" | "click" | "callback" | "accTitle" | "accDescr" => {}
            "classDef" => {
                let rest = s["classDef".len()..].trim();
                if let Some((names, props)) = rest.split_once(char::is_whitespace) {
                    let st = parse_style_props(props);
                    for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        self.class_defs.insert(name.to_owned(), st);
                    }
                }
            }
            "cssClass" => {
                let rest = s["cssClass".len()..].trim();
                if let Some((targets, cls)) = rest.rsplit_once(char::is_whitespace) {
                    let cls = cls.trim().trim_matches('"').to_owned();
                    for id in targets.trim().trim_matches('"').split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        self.node_classes.entry(id.to_owned()).or_default().push(cls.clone());
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
            _ if s.starts_with("<<") && s.contains(">>") => {
                let end = s.find(">>")?;
                let stereo = clean(&s[2..end]);
                let id = s[end + 2..].trim();
                let idx = self.ensure_class(id)?;
                self.d.classes[idx].stereo = Some(stereo);
            }
            _ => {
                if let Some(()) = self.try_relation(s) {
                    return Some(());
                }
                if let Some((lhs, member)) = s.split_once(':') {
                    let id = self.strip_class(lhs.trim())?;
                    let idx = self.ensure_class(&id)?;
                    self.add_member(idx, member.trim());
                    return (!self.over).then_some(());
                }
                return None;
            }
        }
        Some(())
    }

    fn class_decl(&mut self, rest: &str) -> Option<()> {
        let opens = rest.ends_with('{');
        let core = if opens { rest[..rest.len() - 1].trim() } else { rest };
        let (core, inline_stereo) = if let Some(at) = core.find("<<") {
            let end = core[at + 2..].find(">>")?;
            let st = clean(&core[at + 2..at + 2 + end]);
            (core[..at].trim(), Some(st))
        } else {
            (core, None)
        };
        let core = self.strip_class(core)?;
        let (id, label) = if let Some(b) = core.find('[')
            && core.ends_with(']')
        {
            (core[..b].trim(), Some(clean_quoted(&core[b + 1..core.len() - 1])))
        } else if let Some((id, alias)) = core.split_once(" as ") {
            (id.trim(), Some(clean_quoted(alias)))
        } else {
            (core.as_str(), None)
        };
        // `class A,B` declares multiple empty classes on one line.
        if !opens && label.is_none() && id.contains(',') {
            for part in id.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let idx = self.ensure_class(part)?;
                if let Some(st) = &inline_stereo {
                    self.d.classes[idx].stereo = Some(st.clone());
                }
            }
            return Some(());
        }
        let idx = self.ensure_class(id)?;
        if let Some(l) = label {
            self.d.classes[idx].label = l;
        }
        if let Some(st) = inline_stereo {
            self.d.classes[idx].stereo = Some(st);
        }
        if opens {
            self.active_class = Some(idx);
        }
        Some(())
    }

    fn add_member(&mut self, idx: usize, raw: &str) {
        let t = raw.trim();
        if t.is_empty() {
            return;
        }
        if let Some(st) = t.strip_prefix("<<").and_then(|r| r.strip_suffix(">>")) {
            self.d.classes[idx].stereo = Some(clean(st));
            return;
        }
        self.members += 1;
        self.over |= self.members > MAX_ITEMS * 4;
        let cleaned = clean(&tilde_generics(t));
        if cleaned.contains('(') && cleaned.contains(')') {
            self.d.classes[idx].methods.push(cleaned);
        } else {
            self.d.classes[idx].fields.push(cleaned);
        }
    }

    fn try_relation(&mut self, s: &str) -> Option<()> {
        let (lhs_rel, label_part) = match find_colon_outside_quotes(s) {
            Some(at) => (&s[..at], Some(clean_quoted(&s[at + 1..]))),
            None => (s, None),
        };
        let (pos, &(tok, start_h, end_h, line)) = find_rel(lhs_rel)?;
        let (left_id, left_card) = parse_side(lhs_rel[..pos].trim(), true)?;
        let (right_id, right_card) = parse_side(lhs_rel[pos + tok.len()..].trim(), false)?;
        let left_id = self.strip_class(&left_id)?;
        let right_id = self.strip_class(&right_id)?;
        let from = self.ensure_class(&left_id)?;
        let to = self.ensure_class(&right_id)?;
        self.budget()?;
        let mut parts = Vec::new();
        if let Some(lc) = left_card.filter(|c| !c.is_empty()) {
            parts.push(lc);
        }
        if let Some(lbl) = label_part.filter(|c| !c.is_empty()) {
            parts.push(lbl);
        }
        if let Some(rc) = right_card.filter(|c| !c.is_empty()) {
            parts.push(rc);
        }
        let label = (!parts.is_empty()).then(|| parts.join(" · "));
        // Inheritance `<|--` puts the base class at the top and the derived class below it.
        let (from, to, start, end) = if start_h == Head::OpenTriangle && end_h == Head::None {
            (from, to, start_h, end_h)
        } else if end_h == Head::OpenTriangle && start_h == Head::None {
            (to, from, Head::OpenTriangle, Head::None)
        } else {
            (from, to, start_h, end_h)
        };
        self.d.edges.push(Edge { from, to, label, line, start, end, minlen: 1, style: EdgeStyle::default() });
        Some(())
    }

    fn strip_class(&mut self, raw: &str) -> Option<String> {
        match raw.split_once(":::") {
            Some((id, cls)) => {
                let (id, cls) = (id.trim(), cls.trim());
                if id.is_empty() || cls.is_empty() {
                    return None;
                }
                self.node_classes.entry(id.to_owned()).or_default().push(cls.to_owned());
                Some(id.to_owned())
            }
            None => Some(raw.trim().to_owned()),
        }
    }

    fn ensure_class(&mut self, raw: &str) -> Option<usize> {
        let (id, generic) = match raw.find('~') {
            Some(at) if raw.ends_with('~') && at > 0 => (&raw[..at], Some(&raw[at + 1..raw.len() - 1])),
            _ => (raw, None),
        };
        let id = id.trim().trim_matches('`');
        if !valid_class_id(id) {
            return None;
        }
        let ns = self.active_ns;
        if let Some(&idx) = self.ids.get(id) {
            if self.d.classes[idx].namespace.is_none() {
                self.d.classes[idx].namespace = ns;
            }
            if let Some(g) = generic {
                self.d.classes[idx].label = format!("{id}<{g}>");
            }
            return Some(idx);
        }
        self.budget()?;
        let label = match generic {
            Some(g) => format!("{id}<{g}>"),
            None => id.to_owned(),
        };
        self.d.classes.push(ClassNode {
            id: id.to_owned(),
            label,
            stereo: None,
            fields: Vec::new(),
            methods: Vec::new(),
            namespace: ns,
            style: NodeStyle::default(),
        });
        let idx = self.d.classes.len() - 1;
        self.ids.insert(id.to_owned(), idx);
        Some(idx)
    }
}

fn valid_class_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn tilde_generics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut open = false;
    for ch in s.chars() {
        if ch == '~' {
            out.push(if open { '>' } else { '<' });
            open = !open;
        } else {
            out.push(ch);
        }
    }
    out
}

fn find_colon_outside_quotes(s: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, ch) in s.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if ch == ':' && !quoted && s.as_bytes().get(i + 1) != Some(&b':') && i.checked_sub(1).and_then(|p| s.as_bytes().get(p)) != Some(&b':') {
            return Some(i);
        }
    }
    None
}

fn find_rel(s: &str) -> Option<(usize, &'static (&'static str, Head, Head, Line))> {
    let mut quoted = false;
    for (i, ch) in s.char_indices() {
        if ch == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if let Some(rel) = RELS.iter().find(|r| s[i..].starts_with(r.0)) {
            // Avoid matching a single `-` inside a hyphenated identifier like `rune-kit`:
            // `--` only counts as a relation when whitespace or a quote borders it.
            let prev = s[..i].chars().next_back();
            let next = s[i + rel.0.len()..].chars().next();
            if rel.0 == "--" && prev.is_some_and(|c| c.is_alphanumeric()) && next.is_some_and(|c| c.is_alphanumeric()) {
                continue;
            }
            return Some((i, rel));
        }
    }
    None
}

fn parse_side(s: &str, left: bool) -> Option<(String, Option<String>)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if left && s.ends_with('"') {
        let without_end = &s[..s.len() - 1];
        let q = without_end.rfind('"')?;
        let card = clean(&without_end[q + 1..]);
        return Some((without_end[..q].trim().to_owned(), Some(card)));
    }
    if !left && let Some(rest) = s.strip_prefix('"') {
        let q = rest.find('"')?;
        let card = clean(&rest[..q]);
        return Some((rest[q + 1..].trim().to_owned(), Some(card)));
    }
    Some((s.to_owned(), None))
}

const SPACING: Spacing = Spacing { node: 38.0, rank: 48.0, cluster: 16.0 };

pub(crate) fn draw(d: &ClassDiagram, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Scene> {
    let head_font = TextStyle { weight: 600.0, ..paint.font };
    let mem_font = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
    let stereo_font = TextStyle { size: paint.font.size - 2.0, line_height: paint.font.line_height - 4.0, italic: true, ..paint.font };
    let nodes: Vec<DrawNode> = d
        .classes
        .iter()
        .map(|c| {
            let stereo = c.stereo.as_ref().map(|s| measure_wrap(&format!("«{s}»"), stereo_font, 240.0));
            let name = measure_wrap(&c.label, head_font, 240.0);
            let fields: Vec<Measured> = c.fields.iter().map(|f| measure_wrap(f, mem_font, 240.0)).collect();
            let methods: Vec<Measured> = c.methods.iter().map(|m| measure_wrap(m, mem_font, 240.0)).collect();
            let mut w = name.size[0].max(stereo.as_ref().map_or(0.0, |s| s.size[0]));
            for m in fields.iter().chain(&methods) {
                w = w.max(m.size[0]);
            }
            let w = (w + 26.0).max(84.0);
            let mut h = 16.0 + name.size[1] + stereo.as_ref().map_or(0.0, |s| s.size[1] + 2.0);
            for sec in [&fields, &methods] {
                if !sec.is_empty() {
                    h += 9.0 + sec.iter().map(|m| m.size[1] + 3.0).sum::<f32>();
                }
            }
            DrawNode::ClassBox { stereo, name, fields, methods, size: [w, h.max(38.0)], cluster: c.namespace, style: c.style }
        })
        .collect();
    let clusters: Vec<ClusterSpec> = d.namespaces.iter().map(|n| ClusterSpec { title: n.title.clone(), parent: None }).collect();
    draw_layered(d.dir, &nodes, &d.edges, &clusters, &SPACING, paint, hover)
}

#[cfg(test)]
#[path = "class_tests.rs"]
mod tests;
