//! Native ```` ```mermaid ```` diagrams: flowcharts, sequence diagrams, state diagrams and class
//! diagrams, parsed and laid out in Rust and drawn with [`rune::Shape`]s and text, so they theme
//! with the app and stay crisp at any scale. No JavaScript, web view or network.
//!
//! **Flowcharts** (`graph` or `flowchart`, directions `TD`/`TB`/`BT`/`LR`/`RL`): node shapes
//! `[rect]`, `(rounded)`, `([stadium])`, `[[subroutine]]`, `[(cylinder)]`, `((circle))`,
//! `(((double circle)))`, `>asymmetric]`, `{diamond}`, `{{hexagon}}`, `[/lean right/]`,
//! `[\lean left\]`, `[/trapezoid\]`, `[\inverted trapezoid/]`, and every v11
//! `@{ shape: …, label: … }` name and alias (documents, stacked shapes, cylinders, braces,
//! markers, bang, cloud and the rest; icons and images draw as their frame); edge ids
//! (`A e1@--> B`, with `e1@{ … }` settings read and left out); links `-->`, `---`, `-.->`, `==>`, `~~~`, `<-->`, `--o`, `--x`,
//! longer forms for longer links, labels as `-- text -->`, `-- "text" -->` or `-->|text|`; chains
//! (`A --> B --> C`), `&` groups, quoted labels, `<br>` and `\n` breaks, nested `subgraph … end`
//! (including edges to/from subgraph ids), node tints from `style`/`classDef`/`class`/`:::class`,
//! and edge tints from `linkStyle`.
//!
//! **Sequence diagrams** (`sequenceDiagram`): `participant` and `actor` with `as` aliases;
//! `box` participant column groups; `rect` highlight bands; messages `->`, `-->`, `->>`, `-->>`,
//! `-x`, `--x`, `-)`, `--)`, `<<->>` and `<<-->>`, with `+`/`-` activation; `activate` and
//! `deactivate`; `Note left of|right of|over A[,B]`; `autonumber [start [step]]` and
//! `autonumber off`; `create` and `destroy`; v11 participant types (`@{ "type": "database" }`:
//! boundary, control, entity, database, collections, queue); and `loop`, `alt`/`else`, `opt`,
//! `par`/`and`, `critical`/`option` and `break` frames.
//!
//! **State diagrams** (`stateDiagram` or `stateDiagram-v2`): `direction TB|BT|LR|RL`, `[*]`
//! start and end pseudostates, `state "Label" as id`, `state id <<choice>>` (`<<fork>>`,
//! `<<join>>`), `id : description`, transitions `A --> B : label`, composite `state Name { … }`
//! with `--` concurrent regions, and `note left of|right of` notes, on one line or up to `end note`.
//!
//! **Class diagrams** (`classDiagram`): `class Name { +field \n +method() }`, `Name : +member`,
//! `<<trait>>` / `<<enum>>` stereotypes, `namespace Name { … }`, and UML relationships `<|--`,
//! `*--`, `o--`, `-->`, `..>`, `..|>`, `--` and `..` with labels and cardinalities.
//!
//! **Leniency.** A statement that doesn't read is skipped and the rest still draws; a shape opener
//! without its closer (`B[/items split view]`) is a plain rectangle holding the text as written;
//! blocks left open close at the end. [`render`] returns `None`, so callers can show the source,
//! only for a diagram type it doesn't know (gantt, pie, ER, timeline, gitGraph, mindmap, …), when
//! nothing reads, or past the size bounds (400 items, nesting 8 deep).

mod canvas;
mod class;
mod connector;
mod flowchart;
mod hover;
mod layered;
mod layered_draw;
mod route;
mod sequence;
mod sequence_draw;
mod shape;
mod state;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use rune::{Color, El, TextStyle, oklch, oklcha, tok};

use canvas::Scene;
pub use hover::Hover;

/// Colours and type a diagram draws with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagramStyle {
    pub text: Color,
    pub muted: Color,
    pub surface: Color,
    pub surface_border: Color,
    pub node_fill: Color,
    pub node_border: Color,
    pub line: Color,
    pub cluster_fill: Color,
    pub note_fill: Color,
    pub note_border: Color,
    pub font: TextStyle,
    pub max_width: f32,
}

impl Hash for DiagramStyle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        [
            self.text,
            self.muted,
            self.surface,
            self.surface_border,
            self.node_fill,
            self.node_border,
            self.line,
            self.cluster_fill,
            self.note_fill,
            self.note_border,
        ]
        .hash(state);
        self.font.hash(state);
        let bits = if self.max_width == 0.0 { 0 } else if self.max_width.is_nan() { f32::NAN.to_bits() } else { self.max_width.to_bits() };
        bits.hash(state);
    }
}

impl DiagramStyle {
    /// Neutral light or dark colours derived from Rune's core tokens.
    pub fn neutral(dark: bool) -> Self {
        let t = tok::theme(dark);
        Self {
            text: t.text,
            muted: t.text_secondary,
            surface: t.bg,
            surface_border: t.separator,
            node_fill: t.card,
            node_border: t.text.alpha(0.32),
            line: t.text.alpha(0.55),
            cluster_fill: t.text.alpha(0.04),
            note_fill: oklcha(if dark { 0.3 } else { 0.95 }, 0.04, 80.0, 1.0),
            note_border: oklch(if dark { 0.4 } else { 0.85 }, 0.08, 80.0),
            font: TextStyle::new(13.0, 400.0).lh(18.0),
            max_width: 680.0,
        }
    }
}

/// `src` drawn as a diagram card in `style`, or `None` when it is outside the supported subset.
pub fn render(src: &str, style: &DiagramStyle) -> Option<El> {
    render_with_hover(src, style, Hover::Edges, style.text.alpha(0.9))
}

/// [`render`], with `hover` lighting up in `color` while the pointer is over a flowchart, state
/// or class diagram node.
pub fn render_with_hover(src: &str, style: &DiagramStyle, hover: Hover, color: Color) -> Option<El> {
    let hover = (hover != Hover::Off).then_some((hover, color));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    style.hash(&mut h);
    hover.hash(&mut h);
    // Layout measures labels in the app's fonts.
    rune::text::font_epoch().hash(&mut h);
    let key = h.finish();
    let cached = SCENES.with_borrow(|c| c.map.get(&key).cloned());
    let scene = match cached {
        Some(s) => s,
        None => {
            // Defensive: a bug in layout must cost a diagram, not the app.
            let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scene(src, style, hover))).ok().flatten().map(Rc::new);
            SCENES.with_borrow_mut(|c| {
                if c.order.len() >= CACHE_LEN
                    && let Some(old) = c.order.pop_front()
                {
                    c.map.remove(&old);
                }
                c.order.push_back(key);
                c.map.insert(key, built.clone());
            });
            built
        }
    };
    scene.map(|s| s.el(style))
}

/// Whether `src` parses as a supported diagram.
pub fn supported(src: &str) -> bool {
    parse(src).is_some()
}

pub(crate) enum Diagram {
    Flowchart(flowchart::Flowchart),
    Sequence(sequence::Sequence),
    State(state::StateDiagram),
    Class(class::ClassDiagram),
}

pub(crate) struct Parsed {
    pub title: Option<String>,
    pub diagram: Diagram,
}

/// Diagrams bigger than this are left as source: they would be unreadable at markdown width.
pub(crate) const MAX_ITEMS: usize = 400;

/// How deep subgraphs, composite states, frames and other blocks may nest.
pub(crate) const MAX_DEPTH: usize = 8;

pub(crate) fn parse(src: &str) -> Option<Parsed> {
    let mut cleaned = Vec::new();
    let mut in_directive = false;
    for raw in src.lines() {
        let l = raw.trim();
        if in_directive {
            if let Some(pos) = l.find("}%%") {
                in_directive = false;
                let rest = l[pos + 3..].trim();
                if !rest.is_empty() && !rest.starts_with("%%") {
                    cleaned.push(rest);
                }
            }
            continue;
        }
        if l.starts_with("%%{") {
            if let Some(pos) = l.find("}%%") {
                let rest = l[pos + 3..].trim();
                if !rest.is_empty() && !rest.starts_with("%%") {
                    cleaned.push(rest);
                }
            } else {
                in_directive = true;
            }
            continue;
        }
        if !l.is_empty() && !l.starts_with("%%") {
            cleaned.push(l);
        }
    }
    let mut lines = cleaned.into_iter().peekable();
    let mut fm_title = None;
    // A front-matter block (`---` … `---`) may carry `title: …` and config.
    if lines.peek() == Some(&"---") {
        lines.next();
        for l in lines.by_ref() {
            if l == "---" {
                break;
            }
            if let Some(rest) = l.strip_prefix("title:") {
                let t = clean_quoted(rest);
                if !t.is_empty() {
                    fm_title = Some(t);
                }
            }
        }
    }
    let header = lines.next()?;
    let (kind, rest) = header.split_once(|c: char| c.is_whitespace() || c == ';').unwrap_or((header, ""));
    let body: Vec<&str> = lines.collect();
    let (body_title, diagram) = match kind {
        "graph" | "flowchart" => flowchart::parse(rest, &body).map(|f| (f.title.clone(), Diagram::Flowchart(f)))?,
        "sequenceDiagram" => sequence::parse(rest, &body).map(|s| (s.title.clone(), Diagram::Sequence(s)))?,
        "stateDiagram" | "stateDiagram-v2" => state::parse(rest, &body).map(|s| (s.title.clone(), Diagram::State(s)))?,
        "classDiagram" => class::parse(rest, &body).map(|c| (c.title.clone(), Diagram::Class(c)))?,
        _ => return None,
    };
    Some(Parsed { title: body_title.or(fm_title), diagram })
}

const CACHE_LEN: usize = 16;

#[derive(Default)]
struct Cache {
    map: HashMap<u64, Option<Rc<Scene>>>,
    order: VecDeque<u64>,
}

thread_local! {
    static SCENES: RefCell<Cache> = RefCell::new(Cache::default());
}

pub(crate) fn scene(src: &str, paint: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Scene> {
    let parsed = parse(src)?;
    let s = match &parsed.diagram {
        Diagram::Flowchart(f) => flowchart::draw(f, paint, hover),
        Diagram::Sequence(s) => sequence::draw(s, paint),
        Diagram::State(s) => state::draw(s, paint, hover),
        Diagram::Class(c) => class::draw(c, paint, hover),
    }?;
    Some(s.with_title(parsed.title))
}

/// Strips one pair of surrounding `"…"` quotes (unescaping `\"`), then runs [`clean`].
pub(crate) fn clean_quoted(s: &str) -> String {
    let t = s.trim();
    if let Some(inner) = t.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return clean(&inner.replace("\\\"", "\""));
    }
    clean(t)
}

/// Mermaid label markup to plain text: `<br>` and `\n` breaks, entity codes, markdown-string backticks.
pub(crate) fn clean(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('`').and_then(|s| s.strip_suffix('`')).unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(['<', '#', '&', '\\']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let lower = rest.get(..6).unwrap_or(rest).to_ascii_lowercase();
        let (rep, len) = if lower.starts_with("<br>") {
            ("\n", 4)
        } else if lower.starts_with("<br/>") {
            ("\n", 5)
        } else if lower.starts_with("<br />") {
            ("\n", 6)
        } else if rest.starts_with("\\n") {
            ("\n", 2)
        } else if rest.starts_with("#quot;") {
            ("\"", 6)
        } else if rest.starts_with("#59;") {
            (";", 4)
        } else if rest.starts_with("#35;") {
            ("#", 4)
        } else if rest.starts_with("&amp;") {
            ("&", 5)
        } else if rest.starts_with("&lt;") {
            ("<", 4)
        } else if rest.starts_with("&gt;") {
            (">", 4)
        } else if rest.starts_with("&quot;") {
            ("\"", 6)
        } else if rest.starts_with("&apos;") || rest.starts_with("#apos;") {
            ("'", 6)
        } else {
            (&rest[..1], 1)
        };
        out.push_str(rep);
        rest = &rest[len..];
    }
    out.push_str(rest);
    out.lines().map(str::trim).collect::<Vec<_>>().join("\n")
}

/// Optional CSS-like node colours from `style` or `classDef`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NodeStyle {
    pub fill: Option<Color>,
    pub stroke: Option<Color>,
    pub color: Option<Color>,
}

impl NodeStyle {
    pub(crate) fn merge(self, over: NodeStyle) -> NodeStyle {
        NodeStyle {
            fill: over.fill.or(self.fill),
            stroke: over.stroke.or(self.stroke),
            color: over.color.or(self.color),
        }
    }

    /// Resolves `(fill, border, text)` against `paint` in Oklab so light-mode Mermaid hex fills
    /// (like `fill:#dcf5dc`) read cleanly in both dark and light themes without washing out text.
    pub(crate) fn resolve(self, paint: &DiagramStyle) -> (Color, Color, Color) {
        let fill = self.fill.map_or(paint.node_fill, |c| adapt_fill(paint.node_fill, c, 0.055, 0.035));
        let border = self.stroke.or(self.fill).map_or(paint.node_border, |c| adapt_stroke(paint.node_border, c, paint.node_fill.to_oklab()[0] < 0.5));
        let text = match self.color {
            Some(c) if (c.to_oklab()[0] - fill.to_oklab()[0]).abs() > 0.35 => c,
            _ => paint.text,
        };
        (fill, border, text)
    }
}

/// Optional edge overrides from `linkStyle`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EdgeStyle {
    pub stroke: Option<Color>,
    pub width: Option<f32>,
}

impl EdgeStyle {
    pub(crate) fn merge(self, over: EdgeStyle) -> EdgeStyle {
        EdgeStyle { stroke: over.stroke.or(self.stroke), width: over.width.or(self.width) }
    }
}

/// Adapts a user colour `c` into a subtle Oklab fill harmonious with `base`.
pub(crate) fn adapt_fill(base: Color, c: Color, dark_lift: f32, light_drop: f32) -> Color {
    let base_l = base.to_oklab()[0];
    let dark = base_l < 0.5;
    let [_, a, b, _] = c.to_oklab();
    let chroma = a.hypot(b);
    if chroma < 0.01 {
        return oklab_mix(base, c, 0.2);
    }
    let scale = (chroma * 0.9).clamp(0.032, 0.075) / chroma;
    let l = if dark { (base_l + dark_lift).min(0.42) } else { (base_l - light_drop).max(0.82) };
    Color::from_oklab([l, a * scale, b * scale, base.a])
}

pub(crate) fn adapt_stroke(base: Color, c: Color, dark: bool) -> Color {
    let [_, a, b, _] = c.to_oklab();
    let chroma = a.hypot(b);
    if chroma < 0.01 {
        return oklab_mix(base, c, 0.35);
    }
    let scale = (chroma * 1.1).clamp(0.07, 0.13) / chroma;
    Color::from_oklab([if dark { 0.58 } else { 0.60 }, a * scale, b * scale, 1.0])
}

pub(crate) fn oklab_mix(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (a.to_oklab(), b.to_oklab());
    Color::from_oklab([0, 1, 2, 3].map(|i| if i == 3 { a[3] } else { a[i] + (b[i] - a[i]) * t }))
}

pub(crate) fn parse_style_props(props: &str) -> NodeStyle {
    let mut out = NodeStyle::default();
    for part in props.split([',', ';']) {
        let Some((k, v)) = part.split_once(':') else { continue };
        let v = v.trim().trim_end_matches("!important").trim();
        match k.trim() {
            "fill" => out.fill = parse_color_spec(v),
            "stroke" => out.stroke = parse_color_spec(v),
            "color" => out.color = parse_color_spec(v),
            _ => {}
        }
    }
    out
}

pub(crate) fn parse_edge_style_props(props: &str) -> EdgeStyle {
    let mut out = EdgeStyle::default();
    for part in props.split([',', ';']) {
        let Some((k, v)) = part.split_once(':') else { continue };
        let v = v.trim().trim_end_matches("!important").trim();
        match k.trim() {
            "stroke" => out.stroke = parse_color_spec(v),
            "stroke-width" => out.width = v.trim_end_matches("px").trim().parse::<f32>().ok().map(|w| w.clamp(0.75, 5.0)),
            _ => {}
        }
    }
    out
}

/// `#hex`, `rgb(r, g, b)`, `rgba(r, g, b, a)`, or a common CSS colour word.
pub(crate) fn parse_color_spec(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(c) = rune::color::hex(s) {
        return Some(c);
    }
    let lower = s.to_ascii_lowercase();
    if let Some(inner) = lower.strip_prefix("rgb(").or_else(|| lower.strip_prefix("rgba(")).and_then(|r| r.strip_suffix(')')) {
        let nums: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse::<f32>().ok()).collect();
        if nums.len() >= 3 {
            let [r, g, b] = [nums[0], nums[1], nums[2]].map(|v| rune::color::srgb_decode((v / 255.0).clamp(0.0, 1.0)));
            let a = nums.get(3).copied().unwrap_or(1.0).clamp(0.0, 1.0);
            return Some(Color { r, g, b, a });
        }
    }
    let hex = match lower.as_str() {
        "red" | "crimson" | "salmon" | "lightcoral" | "pink" => "#ef4444",
        "green" | "lightgreen" | "palegreen" | "emerald" | "lime" => "#22c55e",
        "blue" | "lightblue" | "skyblue" | "dodgerblue" => "#3b82f6",
        "yellow" | "lightyellow" | "gold" | "amber" | "orange" => "#eab308",
        "purple" | "violet" | "lavender" | "magenta" => "#a855f7",
        "cyan" | "aqua" | "teal" | "turquoise" => "#06b6d4",
        "gray" | "grey" | "lightgray" | "lightgrey" | "silver" => "#9ca3af",
        _ => return None,
    };
    rune::color::hex(hex)
}

/// A line split at `;` outside quotes and brackets (and never at an HTML entity's `;`).
pub(crate) fn statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut escaped, mut brackets) = (0usize, false, false, 0usize);
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted {
            match c {
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '[' | '(' | '{' => brackets += 1,
            ']' | ')' | '}' => brackets = brackets.saturating_sub(1),
            ';' if brackets == 0 => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out.into_iter().map(str::trim).filter(|s| !s.is_empty() && !s.starts_with("%%")).collect()
}

/// A byte cursor over one statement.
pub(crate) struct Cur<'a> {
    pub s: &'a str,
    pub i: usize,
}

impl<'a> Cur<'a> {
    pub fn new(s: &'a str) -> Self {
        Cur { s, i: 0 }
    }
    pub fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }
    pub fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    pub fn peek2(&self) -> Option<char> {
        self.rest().chars().nth(1)
    }
    pub fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.i += c.len_utf8();
        }
    }
    pub fn eat(&mut self, t: &str) -> bool {
        let hit = self.rest().starts_with(t);
        if hit {
            self.i += t.len();
        }
        hit
    }
    pub fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }
    pub fn done(&self) -> bool {
        self.i >= self.s.len()
    }
}

#[cfg(test)]
mod tests;
