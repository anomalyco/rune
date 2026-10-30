//! C4 diagrams (`C4Context`, `C4Container`, `C4Component`, `C4Dynamic`, `C4Deployment`), laid
//! out on [`super::layered_draw`]: people, systems, containers and components as cards (stores as
//! cylinders, queues on their side, external ones in grey), boundaries and deployment nodes as
//! nested frames, and relationships with their technology.

use std::collections::HashMap;

use rune::{Color, TextStyle, oklch};

use super::canvas::{Scene, measure_wrap};
use super::chart::{bold, ink, small};
use super::layered::Spacing;
use super::layered_draw::{ClusterSpec, Dir, DrawNode, Edge, Head, Line, NodeShape, PERSON, draw_layered};
use super::{DiagramStyle, EdgeStyle, Hover, MAX_DEPTH, MAX_ITEMS, clean_quoted, word};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Base {
    Person,
    System,
    Container,
    Component,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Store {
    Plain,
    Db,
    Queue,
}

#[derive(Debug)]
pub(crate) struct Element {
    pub base: Base,
    pub store: Store,
    pub external: bool,
    pub label: String,
    pub techn: String,
    pub descr: String,
    pub boundary: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Boundary {
    pub title: String,
    pub parent: Option<usize>,
}

#[derive(Debug, Default)]
pub(crate) struct C4 {
    pub title: Option<String>,
    pub elements: Vec<Element>,
    pub boundaries: Vec<Boundary>,
    pub edges: Vec<Edge>,
}

struct Rel {
    from: String,
    to: String,
    label: String,
    techn: String,
    both: bool,
    way: Way,
}

/// Which way a relationship's arrow points, and so which end the layout puts above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    Down,
    /// `Rel_U`: toward `to`, which sits above.
    Up,
    /// `Rel_Back`: toward `from`.
    Back,
}

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<C4> {
    let dynamic = header == "C4Dynamic";
    let mut d = C4::default();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut rels: Vec<Rel> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for &l in body {
        if l.starts_with('}') {
            stack.pop();
            continue;
        }
        if let Some(rest) = word(l, "title") {
            d.title = Some(clean_quoted(rest)).filter(|t| !t.is_empty());
            continue;
        }
        let Some((open, close)) = l.find('(').zip(l.rfind(')')).filter(|(o, c)| o < c) else { continue };
        let name = l[..open].trim();
        let (args, named) = args(&l[open + 1..close]);
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        if let Some(kind) = boundary(name) {
            if stack.len() >= MAX_DEPTH {
                return None;
            }
            let ty = args.get(2).cloned().filter(|t| !t.is_empty()).or_else(|| kind.map(str::to_owned));
            let label = args.get(1).or(args.first()).cloned().unwrap_or_default();
            let title = match ty {
                Some(t) => format!("{label}\n[{t}]"),
                None => label,
            };
            d.boundaries.push(Boundary { title, parent: stack.last().copied() });
            if l[close + 1..].trim_start().starts_with('{') {
                stack.push(d.boundaries.len() - 1);
            }
        } else if let Some((base, store, external)) = element(name) {
            let Some(alias) = args.first().filter(|a| !a.is_empty()) else { continue };
            let (techn, descr) = if matches!(base, Base::Container | Base::Component) { (arg(2), arg(3)) } else { (String::new(), arg(2)) };
            let techn = named.get("techn").cloned().unwrap_or(techn);
            let descr = named.get("descr").cloned().unwrap_or(descr);
            ids.insert(alias.clone(), d.elements.len());
            d.elements.push(Element { base, store, external, label: arg(1), techn, descr, boundary: stack.last().copied() });
        } else if let Some((both, way, indexed)) = relation(name) {
            let a = if indexed { &args[args.len().min(1)..] } else { &args[..] };
            let get = |i: usize| a.get(i).cloned().unwrap_or_default();
            let techn = named.get("techn").cloned().unwrap_or_else(|| get(3));
            rels.push(Rel { from: get(0), to: get(1), label: get(2), techn, both, way });
        }
        if d.elements.len() + d.boundaries.len() + rels.len() > MAX_ITEMS {
            return None;
        }
    }
    let mut n = 0;
    for r in rels {
        let (Some(&a), Some(&b)) = (ids.get(&r.from), ids.get(&r.to)) else { continue };
        n += 1;
        let mut label = if dynamic { format!("{n}: {}", r.label) } else { r.label };
        if !r.techn.is_empty() {
            label = format!("{label}\n[{}]", r.techn);
        }
        let other = if r.both { Head::Arrow } else { Head::None };
        let (from, to, start, end) = match r.way {
            Way::Down => (a, b, other, Head::Arrow),
            Way::Up => (b, a, Head::Arrow, other),
            Way::Back => (a, b, Head::Arrow, other),
        };
        let label = Some(label).filter(|l| !l.trim().is_empty());
        d.edges.push(Edge { from, to, label, line: Line::Solid, start, end, minlen: 1, style: EdgeStyle::default() });
    }
    (!d.elements.is_empty()).then_some(d)
}

/// Positional arguments, unquoted, and `$name="value"` ones by name.
fn args(s: &str) -> (Vec<String>, HashMap<String, String>) {
    let (mut parts, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for ch in s.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            ',' if !quoted => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    parts.push(cur);
    let (mut pos, mut named) = (Vec::new(), HashMap::new());
    for p in parts {
        let p = p.trim();
        match p.strip_prefix('$').and_then(|r| r.split_once('=')) {
            Some((k, v)) => {
                named.insert(k.trim().to_owned(), clean_quoted(v));
            }
            None if !p.starts_with('$') => pos.push(clean_quoted(p)),
            None => {}
        }
    }
    (pos, named)
}

/// A boundary function, with the type it shows when none is given.
fn boundary(name: &str) -> Option<Option<&'static str>> {
    Some(match name {
        "Boundary" | "Deployment_Node" | "Node" | "Node_L" | "Node_R" => None,
        "Enterprise_Boundary" => Some("Enterprise"),
        "System_Boundary" => Some("System"),
        "Container_Boundary" => Some("Container"),
        _ => return None,
    })
}

/// `Person`, `SystemDb_Ext`, `ContainerQueue`, …
fn element(name: &str) -> Option<(Base, Store, bool)> {
    let (name, external) = name.strip_suffix("_Ext").map_or((name, false), |n| (n, true));
    let (base, rest) = [("Person", Base::Person), ("System", Base::System), ("Container", Base::Container), ("Component", Base::Component)]
        .into_iter()
        .find_map(|(w, b)| name.strip_prefix(w).map(|r| (b, r)))?;
    let store = match rest {
        "" => Store::Plain,
        "Db" if base != Base::Person => Store::Db,
        "Queue" if base != Base::Person => Store::Queue,
        _ => return None,
    };
    Some((base, store, external))
}

/// `Rel`, `BiRel`, `Rel_Back`, `Rel_U`, `RelIndex`, …: whether it points both ways, which way,
/// and whether an index comes first.
fn relation(name: &str) -> Option<(bool, Way, bool)> {
    let (both, rest) = match name.strip_prefix("BiRel") {
        Some(r) => (true, r),
        None => (false, name.strip_prefix("Rel")?),
    };
    match rest {
        "Index" => Some((both, Way::Down, true)),
        "_Back" => Some((both, Way::Back, false)),
        "_U" | "_Up" => Some((both, Way::Up, false)),
        "" | "_D" | "_Down" | "_L" | "_Left" | "_R" | "_Right" | "_Neighbor" => Some((both, Way::Down, false)),
        _ => None,
    }
}

/// `«external_system_db»` and the like.
fn stereotype(e: &Element) -> String {
    let base = match e.base {
        Base::Person => "person",
        Base::System => "system",
        Base::Container => "container",
        Base::Component => "component",
    };
    let store = match e.store {
        Store::Plain => "",
        Store::Db => "_db",
        Store::Queue => "_queue",
    };
    format!("«{}{base}{store}»", if e.external { "external_" } else { "" })
}

/// C4's blues, deepest for people; external elements in grey.
fn fill(e: &Element) -> Color {
    let l = match (e.base, e.external) {
        (Base::Person, true) => 0.46,
        (_, true) => 0.64,
        (Base::Person, false) => 0.36,
        (Base::System, false) => 0.48,
        (Base::Container, false) => 0.6,
        (Base::Component, false) => 0.76,
    };
    if e.external { oklch(l, 0.0, 0.0) } else { oklch(l, 0.11 - (l - 0.36) * 0.12, 252.0) }
}

const SPACING: Spacing = Spacing { node: 44.0, rank: 64.0, cluster: 20.0 };

pub(crate) fn draw(d: &C4, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Scene> {
    let note = small(paint);
    let fonts: [TextStyle; 3] = [TextStyle { italic: true, ..small(paint) }, bold(paint.font), TextStyle { italic: true, ..note }];
    let nodes: Vec<DrawNode> = d
        .elements
        .iter()
        .map(|e| {
            let techn = (!e.techn.is_empty()).then(|| format!("[{}]", e.techn)).unwrap_or_default();
            let lines: Vec<_> = [(stereotype(e), fonts[0]), (e.label.clone(), fonts[1]), (techn, fonts[2]), (e.descr.clone(), note)]
                .into_iter()
                .filter(|(t, _)| !t.is_empty())
                .map(|(t, f)| (measure_wrap(&t, f, 190.0), f))
                .collect();
            let shape = match e.store {
                Store::Plain => NodeShape::Round,
                Store::Db => NodeShape::Cylinder,
                Store::Queue => NodeShape::HCylinder,
            };
            let w = lines.iter().map(|l| l.0.size[0]).fold(0.0, f32::max) + if shape == NodeShape::HCylinder { 52.0 } else { 32.0 };
            let person = e.base == Base::Person;
            let mut h = lines.iter().map(|l| l.0.size[1] + 3.0).sum::<f32>() + 22.0 + if person { PERSON } else { 0.0 };
            if shape == NodeShape::Cylinder {
                h += 16.0;
            }
            let f = fill(e);
            let border = oklch((f.to_oklab()[0] - 0.1).max(0.2), 0.06, 252.0);
            DrawNode::Card { shape, lines, size: [w.max(150.0), h.max(64.0)], cluster: e.boundary, colors: [f, border, ink(f)], person }
        })
        .collect();
    let clusters: Vec<ClusterSpec> = d.boundaries.iter().map(|b| ClusterSpec { title: b.title.clone(), parent: b.parent }).collect();
    draw_layered(Dir::Down, &nodes, &d.edges, &clusters, &SPACING, paint, hover)
}

#[cfg(test)]
#[path = "c4_tests.rs"]
mod tests;
