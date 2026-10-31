//! Entity-relationship diagrams (`erDiagram`), laid out on [`super::layered_draw`]: entities as
//! attribute tables, relationships in crow's-foot notation.

use std::collections::HashMap;

use rune::{Color, TextStyle};

use super::canvas::{Measured, Scene, measure_wrap};
use super::layered::Spacing;
use super::layered_draw::{Dir, DrawNode, ENTITY_PAD, Edge, Head, Line, draw_layered, entity_row_h};
use super::{DiagramStyle, EdgeStyle, Hover, MAX_ITEMS, NodeStyle, StyleSheet, clean, clean_quoted, statements};

#[derive(Debug)]
pub(crate) struct Entity {
    #[cfg_attr(not(test), allow(dead_code, reason = "tests check parsing by id"))]
    pub id: String,
    pub label: String,
    pub attrs: Vec<Attr>,
    pub style: NodeStyle,
}

/// `type name PK, FK "comment"`.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Attr {
    pub ty: String,
    pub name: String,
    pub keys: String,
    pub comment: String,
}

#[derive(Debug)]
pub(crate) struct ErDiagram {
    pub title: Option<String>,
    pub dir: Dir,
    pub entities: Vec<Entity>,
    pub edges: Vec<Edge>,
}

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<ErDiagram> {
    let mut p = Parser {
        d: ErDiagram { title: None, dir: Dir::Down, entities: Vec::new(), edges: Vec::new() },
        ids: HashMap::new(),
        open: None,
        skipping: false,
        styles: StyleSheet::default(),
        attrs: 0,
        over: false,
    };
    for line in std::iter::once(header).chain(body.iter().copied()) {
        for stmt in statements(line) {
            if p.skipping {
                p.skipping = stmt != "}";
                continue;
            }
            let (entities, edges) = (p.d.entities.len(), p.d.edges.len());
            if p.statement(stmt).is_none() {
                if p.over {
                    return None;
                }
                p.d.entities.truncate(entities);
                p.d.edges.truncate(edges);
                p.ids.retain(|_, &mut i| i < entities);
                p.skipping = stmt.ends_with('{');
            }
        }
    }
    for (id, &idx) in &p.ids {
        p.d.entities[idx].style = p.styles.get(id);
    }
    (!p.d.entities.is_empty()).then_some(p.d)
}

struct Parser {
    d: ErDiagram,
    ids: HashMap<String, usize>,
    /// The entity whose `{ … }` attribute block is open.
    open: Option<usize>,
    /// Inside the body of a block that didn't read, until its `}`.
    skipping: bool,
    styles: StyleSheet,
    attrs: usize,
    over: bool,
}

impl Parser {
    fn budget(&mut self) -> Option<()> {
        self.over |= self.d.entities.len() + self.d.edges.len() >= MAX_ITEMS || self.attrs > MAX_ITEMS * 4;
        (!self.over).then_some(())
    }

    fn statement(&mut self, s: &str) -> Option<()> {
        if let Some(idx) = self.open {
            if s == "}" {
                self.open = None;
                return Some(());
            }
            let attr = parse_attr(s)?;
            self.attrs += 1;
            self.budget()?;
            self.d.entities[idx].attrs.push(attr);
            return Some(());
        }
        let word = s.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match word {
            "title" => {
                let t = clean_quoted(s["title".len()..].trim());
                if !t.is_empty() {
                    self.d.title = Some(t);
                }
                return Some(());
            }
            "direction" => {
                self.d.dir = Dir::parse(s["direction".len()..].trim())?;
                return Some(());
            }
            "accTitle" | "accDescr" => return Some(()),
            "classDef" => {
                self.styles.class_def(&s["classDef".len()..])?;
                return Some(());
            }
            "class" => {
                let (ids, cls) = s["class".len()..].trim().rsplit_once(char::is_whitespace)?;
                for id in ids.split(',').map(str::trim) {
                    self.styles.add_class(id, cls.trim());
                }
                return Some(());
            }
            "style" => {
                self.styles.style(&s["style".len()..])?;
                return Some(());
            }
            _ => {}
        }
        if self.relation(s).is_some() {
            return Some(());
        }
        // `NAME`, `NAME["Alias"]`, `"Name with spaces"`, optionally opening `{ … }`.
        let (head, block) = match s.find('{') {
            Some(at) => (s[..at].trim(), Some(s[at + 1..].trim())),
            None => (s, None),
        };
        let idx = self.entity(head)?;
        match block {
            None => {}
            Some("") => self.open = Some(idx),
            Some(rest) => {
                // `NAME { string id }` on one line.
                let inner = rest.strip_suffix('}')?.trim();
                if !inner.is_empty() {
                    let attr = parse_attr(inner)?;
                    self.attrs += 1;
                    self.budget()?;
                    self.d.entities[idx].attrs.push(attr);
                }
            }
        }
        Some(())
    }

    fn relation(&mut self, s: &str) -> Option<()> {
        let (lhs, label) = match colon_outside_quotes(s) {
            Some(at) => (s[..at].trim(), Some(clean_quoted(&s[at + 1..]))),
            None => (s.trim(), None),
        };
        let (a, start, line, end, b) = symbolic(lhs).or_else(|| worded(lhs))?;
        let from = self.entity(a)?;
        let to = self.entity(b)?;
        self.budget()?;
        let label = label.filter(|l| !l.is_empty());
        self.d.edges.push(Edge { from, to, label, line, start, end, minlen: 1, style: EdgeStyle::default() });
        Some(())
    }

    fn entity(&mut self, raw: &str) -> Option<usize> {
        let (raw, class) = match raw.split_once(":::") {
            Some((id, cls)) => (id.trim(), Some(cls.trim())),
            None => (raw.trim(), None),
        };
        let (id, alias) = match raw.find('[') {
            Some(at) if raw.ends_with(']') && at > 0 => (raw[..at].trim(), Some(clean_quoted(&raw[at + 1..raw.len() - 1]))),
            _ => (raw, None),
        };
        let id = match id.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
            Some(q) if !q.is_empty() && !q.contains('"') => q,
            _ if valid_id(id) => id,
            _ => return None,
        };
        if let Some(cls) = class {
            self.styles.add_class(id, cls);
        }
        let idx = match self.ids.get(id) {
            Some(&i) => i,
            None => {
                self.budget()?;
                self.d.entities.push(Entity { id: id.to_owned(), label: clean(id), attrs: Vec::new(), style: NodeStyle::default() });
                self.ids.insert(id.to_owned(), self.d.entities.len() - 1);
                self.d.entities.len() - 1
            }
        };
        if let Some(a) = alias.filter(|a| !a.is_empty()) {
            self.d.entities[idx].label = a;
        }
        Some(idx)
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '*'))
}

/// `A ||--o{ B`: two marks either side of `--` (identifying) or `..` (not).
fn symbolic(s: &str) -> Option<(&str, Head, Line, Head, &str)> {
    let b = s.as_bytes();
    for i in 2..b.len().saturating_sub(3) {
        let line = match &b[i..i + 2] {
            b"--" => Line::Solid,
            b".." => Line::Dotted,
            _ => continue,
        };
        let left = match &b[i - 2..i] {
            b"||" => Head::OnlyOne,
            b"|o" => Head::ZeroOrOne,
            b"}|" => Head::OneOrMore,
            b"}o" => Head::ZeroOrMore,
            _ => continue,
        };
        let right = match &b[i + 2..i + 4] {
            b"||" => Head::OnlyOne,
            b"o|" => Head::ZeroOrOne,
            b"|{" => Head::OneOrMore,
            b"o{" => Head::ZeroOrMore,
            _ => continue,
        };
        let (a, z) = (s[..i - 2].trim(), s[i + 4..].trim());
        if a.is_empty() || z.is_empty() {
            return None;
        }
        return Some((a, left, line, right, z));
    }
    None
}

/// `CAR only one to zero or more DRIVER`, with `optionally to` for a dotted line.
fn worded(s: &str) -> Option<(&str, Head, Line, Head, &str)> {
    let (a, rest) = first_token(s)?;
    let (rest, z) = last_token(rest)?;
    let words = rest.trim();
    let (left, line, right) = if let Some((l, r)) = words.split_once(" optionally to ") {
        (l, Line::Dotted, r)
    } else {
        let (l, r) = words.split_once(" to ")?;
        (l, Line::Solid, r)
    };
    Some((a, card(left.trim())?, line, card(right.trim())?, z))
}

fn card(s: &str) -> Option<Head> {
    Some(match s.to_ascii_lowercase().as_str() {
        "only one" | "1" | "one" | "exactly one" => Head::OnlyOne,
        "zero or one" | "one or zero" => Head::ZeroOrOne,
        "one or more" | "one or many" | "many(1)" | "1+" => Head::OneOrMore,
        "zero or more" | "zero or many" | "many(0)" | "0+" | "many" => Head::ZeroOrMore,
        _ => return None,
    })
}

fn first_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')? + 2;
        return Some((&s[..end], &s[end..]));
    }
    let end = s.find(char::is_whitespace)?;
    Some((&s[..end], &s[end..]))
}

fn last_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    if let Some(rest) = s.strip_suffix('"') {
        let start = rest.rfind('"')?;
        return Some((&s[..start], &s[start..]));
    }
    let start = s.rfind(char::is_whitespace)? + 1;
    Some((&s[..start], &s[start..]))
}

fn colon_outside_quotes(s: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, ch) in s.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ':' if !quoted && !s[i..].starts_with(":::") && !s[..i].ends_with(':') => return Some(i),
            _ => {}
        }
    }
    None
}

/// `type name [PK|FK|UK[, …]] ["comment"]`.
fn parse_attr(s: &str) -> Option<Attr> {
    let (body, comment) = match s.find('"') {
        Some(at) => (s[..at].trim(), clean_quoted(&s[at..])),
        None => (s.trim(), String::new()),
    };
    let mut words = body.split_whitespace();
    let ty = words.next()?;
    let name = words.next()?;
    let mut keys = Vec::new();
    for w in words.flat_map(|w| w.split(',')).map(str::trim).filter(|w| !w.is_empty()) {
        match w {
            "PK" | "FK" | "UK" => keys.push(w),
            _ => return None,
        }
    }
    Some(Attr { ty: generics(ty), name: name.to_owned(), keys: keys.join(", "), comment })
}

/// `list~string~` as `list<string>`.
fn generics(s: &str) -> String {
    let mut open = false;
    s.chars()
        .map(|c| match c {
            '~' => {
                open = !open;
                if open { '<' } else { '>' }
            }
            c => c,
        })
        .collect()
}

const SPACING: Spacing = Spacing { node: 40.0, rank: 52.0, cluster: 16.0 };

pub(crate) fn draw(d: &ErDiagram, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Scene> {
    let head_font = TextStyle { weight: 600.0, ..paint.font };
    let row_font = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
    let nodes: Vec<DrawNode> = d
        .entities
        .iter()
        .map(|e| {
            let name = measure_wrap(&e.label, head_font, 220.0);
            let rows: Vec<Vec<Measured>> = e
                .attrs
                .iter()
                .map(|a| [&a.ty, &a.name, &a.keys, &a.comment].iter().map(|t| measure_wrap(t, row_font, 180.0)).collect())
                .collect();
            // A column nobody fills takes no room.
            let mut cols = vec![0.0f32; 4];
            for row in &rows {
                for (w, m) in cols.iter_mut().zip(row) {
                    if !m.text.is_empty() {
                        *w = w.max(m.size[0] + ENTITY_PAD[0] * 2.0);
                    }
                }
            }
            let table: f32 = cols.iter().sum();
            let w = table.max(name.size[0] + 28.0).max(90.0);
            // The last filled column takes up any slack.
            if let Some(last) = cols.iter_mut().rev().find(|w| **w > 0.0) {
                *last += w - table;
            }
            let h = name.size[1] + ENTITY_PAD[1] * 2.0 + rows.iter().map(|r| entity_row_h(r)).sum::<f32>();
            DrawNode::Entity { name, rows, cols, size: [w, h.max(36.0)], style: e.style }
        })
        .collect();
    draw_layered(d.dir, &nodes, &d.edges, &[], &SPACING, paint, hover)
}

#[cfg(test)]
#[path = "er_tests.rs"]
mod tests;
