//! Requirement diagrams (`requirementDiagram`), laid out on [`super::layered_draw`]: requirements
//! and elements as boxes of their fields, joined by `contains`, `satisfies` and the other
//! relationships.

use std::collections::HashMap;

use rune::Color;

use super::canvas::Scene;
use super::layered::Spacing;
use super::layered_draw::{Dir, DrawNode, Edge, Head, Line, class_box, draw_layered};
use super::{DiagramStyle, EdgeStyle, Hover, MAX_ITEMS, NodeStyle, StyleSheet, clean_quoted, parse_style_props, word};

#[derive(Debug)]
pub(crate) struct Req {
    pub name: String,
    /// "Requirement", "Functional Requirement", "Element", …; empty for a name only ever related.
    pub kind: String,
    /// `Key: value` rows.
    pub fields: Vec<String>,
    pub style: NodeStyle,
}

#[derive(Debug)]
pub(crate) struct Requirements {
    pub dir: Dir,
    pub nodes: Vec<Req>,
    pub edges: Vec<Edge>,
}

const KINDS: [(&str, &str); 7] = [
    ("requirement", "Requirement"),
    ("functionalRequirement", "Functional Requirement"),
    ("interfaceRequirement", "Interface Requirement"),
    ("performanceRequirement", "Performance Requirement"),
    ("physicalRequirement", "Physical Requirement"),
    ("designConstraint", "Design Constraint"),
    ("element", "Element"),
];

const RELATIONS: [&str; 7] = ["contains", "copies", "derives", "satisfies", "verifies", "refines", "traces"];

pub(crate) fn parse(body: &[&str]) -> Option<Requirements> {
    let mut d = Requirements { dir: Dir::Down, nodes: Vec::new(), edges: Vec::new() };
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut styles = StyleSheet::default();
    let mut open: Option<usize> = None;
    for &l in body {
        if let Some(i) = open {
            let (l, closes) = l.strip_suffix('}').map_or((l, false), |r| (r.trim(), true));
            if let Some((k, v)) = l.split_once(':') {
                let v = clean_quoted(v);
                let row = match k.trim().to_ascii_lowercase().as_str() {
                    "id" => Some(format!("ID: {v}")),
                    "text" => Some(format!("Text: {v}")),
                    "risk" => Some(format!("Risk: {}", title_case(&v))),
                    "verifymethod" => Some(format!("Verification: {}", title_case(&v))),
                    "type" => Some(format!("Type: {v}")),
                    "docref" => Some(format!("Doc Ref: {v}")),
                    _ => None,
                };
                d.nodes[i].fields.extend(row);
            }
            if closes {
                open = None;
            }
            continue;
        }
        if let Some(rest) = word(l, "direction") {
            d.dir = Dir::parse(rest).unwrap_or(d.dir);
        } else if let Some(rest) = word(l, "classDef") {
            let _ = styles.class_def(rest);
        } else if let Some(rest) = word(l, "class") {
            if let Some((names, class)) = rest.split_once(char::is_whitespace) {
                for n in names.split(',') {
                    styles.add_class(&clean_quoted(n), class.trim());
                }
            }
        } else if let Some(rest) = word(l, "style") {
            if let Some((names, props)) = rest.split_once(char::is_whitespace) {
                let st = parse_style_props(props);
                for n in names.split(',') {
                    styles.add_style(&clean_quoted(n), st);
                }
            }
        } else if let Some((kind, rest)) = KINDS.iter().find_map(|(w, k)| word(l, w).map(|r| (*k, r))) {
            let Some(name) = rest.strip_suffix('{').map(str::trim).filter(|n| !n.is_empty()) else { continue };
            let i = node(&mut d, &mut ids, &mut styles, name);
            d.nodes[i].kind = kind.to_owned();
            open = Some(i);
        } else if let Some((from, rel, to)) = relation(l) {
            let (a, b) = (node(&mut d, &mut ids, &mut styles, from), node(&mut d, &mut ids, &mut styles, to));
            let contains = rel == "contains";
            d.edges.push(Edge {
                from: a,
                to: b,
                label: Some(format!("«{rel}»")),
                line: if contains { Line::Solid } else { Line::Dotted },
                start: if contains { Head::Contains } else { Head::None },
                end: if contains { Head::None } else { Head::Arrow },
                minlen: 1,
                style: EdgeStyle::default(),
            });
        }
        if d.nodes.len() + d.edges.len() > MAX_ITEMS {
            return None;
        }
    }
    for (name, i) in ids {
        d.nodes[i].style = styles.get(&name);
    }
    (!d.nodes.is_empty()).then_some(d)
}

/// The node `name` (`"quoted"`, maybe with a `:::class`) names, added if it's new.
fn node(d: &mut Requirements, ids: &mut HashMap<String, usize>, styles: &mut StyleSheet, name: &str) -> usize {
    let (name, class) = name.split_once(":::").map_or((name, None), |(n, c)| (n, Some(c.trim())));
    let name = clean_quoted(name);
    let i = *ids.entry(name.clone()).or_insert_with(|| {
        d.nodes.push(Req { name, kind: String::new(), fields: Vec::new(), style: NodeStyle::default() });
        d.nodes.len() - 1
    });
    if let Some(c) = class {
        styles.add_class(&d.nodes[i].name, c);
    }
    i
}

/// `a - satisfies -> b`, or `b <- satisfies - a`: the source, the relationship and the target.
fn relation(l: &str) -> Option<(&str, &'static str, &str)> {
    let (from, mid, to) = if let Some((left, to)) = l.split_once("->") {
        let (from, mid) = left.rsplit_once('-')?;
        (from, mid, to)
    } else {
        let (to, right) = l.split_once("<-")?;
        let (mid, from) = right.split_once('-')?;
        (from, mid, to)
    };
    let rel = RELATIONS.iter().find(|r| r.eq_ignore_ascii_case(mid.trim()))?;
    let (from, to) = (from.trim(), to.trim());
    (!from.is_empty() && !to.is_empty()).then_some((from, rel, to))
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect())
}

const SPACING: Spacing = Spacing { node: 40.0, rank: 56.0, cluster: 16.0 };

pub(crate) fn draw(d: &Requirements, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Scene> {
    let nodes: Vec<DrawNode> =
        d.nodes.iter().map(|r| class_box((!r.kind.is_empty()).then_some(r.kind.as_str()), &r.name, &r.fields, &[], None, r.style, paint)).collect();
    draw_layered(d.dir, &nodes, &d.edges, &[], &SPACING, paint, hover)
}

#[cfg(test)]
#[path = "requirement_tests.rs"]
mod tests;
