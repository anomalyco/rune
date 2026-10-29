//! Native ```` ```mermaid ```` diagrams: flowcharts, sequence diagrams and state diagrams,
//! parsed and laid out in Rust and drawn with [`rune::Shape`]s and text, so they theme with the
//! app and stay crisp at any scale. No JavaScript, web view or network.
//!
//! **Flowcharts** (`graph` or `flowchart`, directions `TD`/`TB`/`BT`/`LR`/`RL`): node shapes
//! `[rect]`, `(rounded)`, `([stadium])`, `[[subroutine]]`, `[(cylinder)]`, `((circle))`,
//! `>asymmetric]`, `{diamond}`, `{{hexagon}}`, `[/parallelogram/]` and `[/trapezoid\]`; links
//! `-->`, `---`, `-.->`, `==>`, `~~~`, `<-->`, `--o`, `--x`, longer forms for longer links,
//! labels as `-- text -->`, `-- "text" -->` or `-->|text|`; chains (`A --> B --> C`), `&`
//! groups, quoted labels, `<br>` and `\n` breaks, nested `subgraph … end`, and node tints from
//! `style`, `classDef`, `class` and `:::class`. `linkStyle`, `click` and `%%` comments are
//! ignored.
//!
//! **Sequence diagrams** (`sequenceDiagram`): `participant` and `actor` with `as` aliases;
//! messages `->`, `-->`, `->>`, `-->>`, `-x`, `--x`, `-)`, `--)`, `<<->>` and `<<-->>`, with
//! `+`/`-` activation; `activate`/`deactivate`; `Note left of|right of|over A[,B]`;
//! `autonumber`; and `loop`, `alt`/`else`, `opt`, `par`/`and`, `critical`/`option` and `break`
//! frames (`rect` and `box` groups are accepted and not drawn).
//!
//! **State diagrams** (`stateDiagram` or `stateDiagram-v2`): `direction TB|BT|LR|RL`, `[*]`
//! start and end pseudostates, `state "Label" as id`, `state id <<choice>>` (`<<fork>>`,
//! `<<join>>`), `id : description`, transitions `A --> B` and `A --> B : label`, and composite
//! `state Name { … }` blocks.
//!
//! Anything else (gantt, pie, class, ER, timeline, gitGraph, mindmap, unknown statements or
//! unbalanced brackets) is unsupported: [`render`] returns `None` so callers can show the source.
//!
//! **Cost.** Parsing, measuring and layout happen once per source and style, on the first
//! build that sees them; later builds reuse the laid-out scene from a small cache. Work is
//! bounded for untrusted input: charts over 400 nodes, edges and subgraphs (or 40
//! participants), subgraphs nested more than 8 deep, and graphs whose long edges need more
//! than 3,000 bend points fall back to source; links longer than `--------->` draw at that
//! length.

mod flowchart;
mod flowchart_draw;
mod hover;
mod layered;
mod route;
mod sequence;
mod state;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use rune::{Align, Color, El, LineCap, Shape, TextStyle, canvas, col, el, oklch, oklcha, text, tok};

pub use hover::Hover;
use hover::Trace;

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
    /// What lights up while the pointer is over a node.
    pub hover: Hover,
    /// The colour it lights up in.
    pub hover_color: Color,
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
            hover: Hover::Edges,
            hover_color: t.text.alpha(0.9),
        }
    }
}

/// `src` drawn as a diagram card in `style`, or `None` when it is outside the supported subset.
pub fn render(src: &str, style: &DiagramStyle) -> Option<El> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    format!("{style:?}").hash(&mut h);
    let key = h.finish();
    let cached = SCENES.with_borrow(|c| c.map.get(&key).cloned());
    let scene = match cached {
        Some(s) => s,
        None => {
            // Defensive: a bug in layout must cost a diagram, not the app.
            let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scene(src, style))).ok().flatten().map(Rc::new);
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
}

/// Diagrams bigger than this are left as source: they would be unreadable at markdown width.
pub(crate) const MAX_ITEMS: usize = 400;

pub(crate) fn parse(src: &str) -> Option<Diagram> {
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
    // A front-matter block (`---` … `---`) carries a title and config; skip it.
    if lines.peek() == Some(&"---") {
        lines.next();
        for l in lines.by_ref() {
            if l == "---" {
                break;
            }
        }
    }
    let header = lines.next()?;
    let (kind, rest) = header.split_once(|c: char| c.is_whitespace() || c == ';').unwrap_or((header, ""));
    let body: Vec<&str> = lines.collect();
    match kind {
        "graph" | "flowchart" => flowchart::parse(rest, &body).map(Diagram::Flowchart),
        "sequenceDiagram" => sequence::parse(rest, &body).map(Diagram::Sequence),
        "stateDiagram" | "stateDiagram-v2" => state::parse(rest, &body).map(Diagram::State),
        _ => None,
    }
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

pub(crate) fn scene(src: &str, paint: &DiagramStyle) -> Option<Scene> {
    match parse(src)? {
        Diagram::Flowchart(f) => flowchart_draw::draw(&f, paint),
        Diagram::Sequence(s) => sequence::draw(&s, paint),
        Diagram::State(s) => state::draw(&s, paint),
    }
}

/// The card's padding on each side.
const CARD_PAD: f32 = 16.0;

/// The widest a diagram's drawing can be for its card to fit `paint.max_width`.
pub(crate) fn room(paint: &DiagramStyle) -> f32 {
    paint.max_width - 2.0 * CARD_PAD - 2.0
}

/// How much a canvas `width` points wide is drawn at to fit its card in `paint.max_width`.
pub(crate) fn fit(width: f32, paint: &DiagramStyle) -> f32 {
    if width <= room(paint) { 1.0 } else { (room(paint) / width).max(0.6) }
}

/// Text measured for a diagram: its size, and whether it wraps at [`WRAP`].
#[derive(Clone)]
pub(crate) struct Measured {
    pub text: String,
    pub size: [f32; 2],
    pub wrap: bool,
}

/// Labels wider than this wrap.
const WRAP: f32 = 200.0;

pub(crate) fn measure(s: &str, font: TextStyle) -> Measured {
    if s.is_empty() {
        return Measured { text: String::new(), size: [0.0, 0.0], wrap: false };
    }
    let (w, h) = rune::text::measure(s, &[], font, None);
    if w <= WRAP {
        return Measured { text: s.to_owned(), size: [w, h], wrap: false };
    }
    let (w, h) = rune::text::measure(s, &[], font, Some(WRAP));
    Measured { text: s.to_owned(), size: [w, h], wrap: true }
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
        let base_l = paint.node_fill.to_oklab()[0];
        let dark = base_l < 0.5;
        let fill = match self.fill {
            Some(c) => {
                let [_, a, b, _] = c.to_oklab();
                let chroma = a.hypot(b);
                if chroma < 0.01 {
                    oklab_mix(paint.node_fill, c, 0.2)
                } else {
                    let target = (chroma * 0.9).clamp(0.032, 0.075);
                    let scale = target / chroma;
                    let l = if dark { (base_l + 0.055).min(0.42) } else { (base_l - 0.035).max(0.82) };
                    Color::from_oklab([l, a * scale, b * scale, paint.node_fill.a])
                }
            }
            None => paint.node_fill,
        };
        let border = match self.stroke.or(self.fill) {
            Some(c) => {
                let [_, a, b, _] = c.to_oklab();
                let chroma = a.hypot(b);
                if chroma < 0.01 {
                    oklab_mix(paint.node_border, c, 0.35)
                } else {
                    let scale = (chroma * 1.1).clamp(0.07, 0.13) / chroma;
                    let l = if dark { 0.56 } else { 0.62 };
                    Color::from_oklab([l, a * scale, b * scale, 1.0])
                }
            }
            None => paint.node_border,
        };
        let text = match self.color {
            Some(c) if (c.to_oklab()[0] - fill.to_oklab()[0]).abs() > 0.35 => c,
            _ => paint.text,
        };
        (fill, border, text)
    }
}

fn oklab_mix(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (a.to_oklab(), b.to_oklab());
    Color::from_oklab([0, 1, 2, 3].map(|i| if i == 3 { a[3] } else { a[i] + (b[i] - a[i]) * t }))
}

pub(crate) fn parse_style_props(props: &str) -> NodeStyle {
    let mut out = NodeStyle::default();
    for part in props.split(',') {
        let Some((k, v)) = part.split_once(':') else { continue };
        let v = v.trim().trim_end_matches("!important").trim();
        match k.trim() {
            "fill" => out.fill = rune::color::hex(v),
            "stroke" => out.stroke = rune::color::hex(v),
            "color" => out.color = rune::color::hex(v),
            _ => {}
        }
    }
    out
}

#[derive(Clone)]
pub(crate) enum Op {
    Fill(Vec<[f32; 2]>, Color),
    Stroke { points: Vec<[f32; 2]>, closed: bool, width: f32, cap: LineCap, color: Color },
}

pub(crate) struct Label {
    pub text: String,
    pub at: [f32; 2],
    pub size: [f32; 2],
    pub font: TextStyle,
    pub color: Color,
    pub wrap: bool,
    pub align: Align,
}

/// Drawing in a diagram's own points; [`Canvas::finish`] moves it to the origin and scales it.
#[derive(Default)]
pub(crate) struct Canvas {
    pub(crate) ops: Vec<Op>,
    pub(crate) labels: Vec<Label>,
    /// Per node: its outline, where hovering it lights up its traces (empty when not drawn).
    pub(crate) regions: Vec<Vec<[f32; 2]>>,
    /// Per edge: its ends and its drawing in the hover colour.
    pub(crate) traces: Vec<Trace>,
    pub(crate) hover: Option<(Hover, Color)>,
}

impl Canvas {
    pub(crate) fn fill(&mut self, points: Vec<[f32; 2]>, color: Color) {
        if points.len() >= 3 {
            self.ops.push(Op::Fill(points, color));
        }
    }

    pub(crate) fn stroke(&mut self, points: Vec<[f32; 2]>, closed: bool, width: f32, color: Color) {
        if points.len() >= 2 {
            self.ops.push(Op::Stroke { points, closed, width, cap: LineCap::Round, color });
        }
    }

    /// A filled outline.
    pub(crate) fn shape(&mut self, points: Vec<[f32; 2]>, fill: Color, border: Color, width: f32) {
        self.fill(points.clone(), fill);
        self.stroke(points, true, width, border);
    }

    pub(crate) fn dashed(&mut self, points: &[[f32; 2]], width: f32, color: Color, on: f32, off: f32) {
        let mut dash: Vec<[f32; 2]> = Vec::new();
        let mut drawing = true;
        let mut left = on;
        // Bounded so a huge or non-finite length (where `t += step` stops advancing) can't spin.
        let mut budget = 20_000usize;
        for w in points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = dist(a, b);
            if !len.is_finite() {
                continue;
            }
            let mut t = 0.0;
            while t < len {
                let Some(rest) = budget.checked_sub(1) else { return };
                budget = rest;
                let step = left.min(len - t);
                let p0 = lerp(a, b, t / len);
                let p1 = lerp(a, b, (t + step) / len);
                if drawing {
                    if dash.is_empty() {
                        dash.push(p0);
                    }
                    dash.push(p1);
                }
                t += step;
                left -= step;
                if left <= 1e-3 {
                    if drawing && dash.len() >= 2 {
                        self.ops.push(Op::Stroke { points: std::mem::take(&mut dash), closed: false, width, cap: LineCap::Butt, color });
                    }
                    dash.clear();
                    drawing = !drawing;
                    left = if drawing { on } else { off };
                }
            }
        }
        if dash.len() >= 2 {
            self.ops.push(Op::Stroke { points: dash, closed: false, width, cap: LineCap::Butt, color });
        }
    }

    /// `m` centred on `center`.
    pub(crate) fn label(&mut self, m: &Measured, center: [f32; 2], font: TextStyle, color: Color, align: Align) {
        if m.text.is_empty() {
            return;
        }
        let at = [center[0] - m.size[0] / 2.0, center[1] - m.size[1] / 2.0];
        self.label_at(m, at, font, color, align);
    }

    pub(crate) fn label_at(&mut self, m: &Measured, at: [f32; 2], font: TextStyle, color: Color, align: Align) {
        if !m.text.is_empty() {
            self.labels.push(Label { text: m.text.clone(), at, size: m.size, font, color, wrap: m.wrap, align });
        }
    }

    /// Draws `other` over this.
    pub(crate) fn append(&mut self, other: Canvas) {
        self.ops.extend(other.ops);
        self.labels.extend(other.labels);
        self.traces.extend(other.traces);
    }

    /// The bounding box of everything drawn: `[x0, y0, x1, y1]`.
    fn bounds(&self) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let mut add = |p: [f32; 2], r: f32| {
            b = [b[0].min(p[0] - r), b[1].min(p[1] - r), b[2].max(p[0] + r), b[3].max(p[1] + r)];
        };
        for op in &self.ops {
            match op {
                Op::Fill(points, _) => points.iter().for_each(|&p| add(p, 0.0)),
                Op::Stroke { points, width, .. } => points.iter().for_each(|&p| add(p, width / 2.0)),
            }
        }
        for l in &self.labels {
            add(l.at, 0.0);
            add([l.at[0] + l.size[0], l.at[1] + l.size[1]], 0.0);
        }
        if b[0] > b[2] { [0.0; 4] } else { b }
    }

    pub(crate) fn width(&self) -> f32 {
        let b = self.bounds();
        b[2] - b[0]
    }

    pub(crate) fn finish(self, scale: f32) -> Option<Scene> {
        if self.ops.is_empty() && self.labels.is_empty() {
            return None;
        }
        let b = self.bounds();
        let pad = 1.0;
        let map = |p: [f32; 2]| [(p[0] - b[0]) * scale + pad, (p[1] - b[1]) * scale + pad];
        let paint = |shape: Shape, op: &Op| match op {
            Op::Fill(points, color) => shape.fill(points.iter().map(|&p| map(p)).collect::<Vec<_>>(), *color),
            Op::Stroke { points, closed, width, cap, color } => {
                shape.stroke_with(points.iter().map(|&p| map(p)).collect::<Vec<_>>(), *closed, width * scale.max(0.8), *cap, *color)
            }
        };
        let shape = self.ops.iter().fold(Shape::new(), paint);
        let hovers = self.hovers(map, paint);
        let labels = self
            .labels
            .into_iter()
            .map(|l| Label {
                at: map(l.at),
                size: [l.size[0] * scale, l.size[1] * scale],
                font: TextStyle { size: l.font.size * scale, line_height: l.font.line_height * scale, ..l.font },
                ..l
            })
            .collect();
        let size = [(b[2] - b[0]) * scale + 2.0 * pad, (b[3] - b[1]) * scale + 2.0 * pad];
        Some(Scene { size, shape, labels, hovers })
    }
}

/// A laid-out diagram, drawn the same way every build.
pub(crate) struct Scene {
    pub(crate) size: [f32; 2],
    shape: Shape,
    labels: Vec<Label>,
    /// Per hoverable node: its index, where it is (`[x, y, w, h]`) and what lights up.
    hovers: Vec<(usize, [f32; 4], Shape)>,
}

impl Scene {
    fn el(&self, paint: &DiagramStyle) -> El {
        let [w, h] = self.size;
        let mut body = el().key("diagram").size(w, h).child(canvas(self.shape.clone()).key("shape").absolute(0.0, 0.0).size(w, h));
        for (v, r, lit) in &self.hovers {
            let glow = canvas(lit.clone()).absolute(-r[0], -r[1]).size(w, h).group_opacity(0.0, 1.0);
            body = body.child(el().key(format!("node{v}")).absolute(r[0], r[1]).size(r[2], r[3]).hover_group().child(glow));
        }
        for (i, l) in self.labels.iter().enumerate() {
            // A point of slack so rounding never breaks a measured line.
            let mut t = text(l.text.clone())
                .key(format!("t{i}"))
                .font(l.font)
                .color(l.color)
                .align(l.align)
                .w(l.size[0] + 2.0)
                .absolute(l.at[0] - 1.0, l.at[1]);
            if l.wrap {
                t = t.wrap();
            }
            body = body.child(t);
        }
        col()
            .w_full()
            .items_center()
            .padding(CARD_PAD)
            .radius(6.0)
            .fill(paint.surface)
            .border(0.5, paint.surface_border)
            .clip_rounded(6.0)
            .child(body)
    }
}

pub(crate) fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

pub(crate) fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

pub(crate) fn unit(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let d = dist(a, b).max(1e-6);
    [(b[0] - a[0]) / d, (b[1] - a[1]) / d]
}

/// A rectangle with corners of radius `r`, as a polygon.
pub(crate) fn rounded_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Vec<[f32; 2]> {
    let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
    if r < 0.5 {
        return vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    }
    let steps = 6;
    let mut pts = Vec::with_capacity(4 * (steps + 1));
    let corners = [([x1 - r, y0 + r], -90.0f32), ([x1 - r, y1 - r], 0.0), ([x0 + r, y1 - r], 90.0), ([x0 + r, y0 + r], 180.0)];
    for (c, start) in corners {
        for i in 0..=steps {
            let a = (start + 90.0 * i as f32 / steps as f32).to_radians();
            pts.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
        }
    }
    pts
}

/// An ellipse's arc from `a0` to `a1` degrees (clockwise on screen), as points.
pub(crate) fn arc(c: [f32; 2], rx: f32, ry: f32, a0: f32, a1: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / steps as f32).to_radians();
            [c[0] + rx * a.cos(), c[1] + ry * a.sin()]
        })
        .collect()
}

pub(crate) fn ellipse(c: [f32; 2], rx: f32, ry: f32) -> Vec<[f32; 2]> {
    let mut pts = arc(c, rx, ry, 0.0, 360.0, 48);
    pts.pop();
    pts
}

/// A filled arrowhead with its tip at `tip`, pointing along `dir` (a unit vector).
pub(crate) fn arrowhead(tip: [f32; 2], dir: [f32; 2], len: f32, width: f32) -> Vec<[f32; 2]> {
    let base = [tip[0] - dir[0] * len, tip[1] - dir[1] * len];
    let n = [-dir[1] * width / 2.0, dir[0] * width / 2.0];
    vec![tip, [base[0] + n[0], base[1] + n[1]], [base[0] - n[0], base[1] - n[1]]]
}

/// A smooth curve through `pts` (Catmull-Rom, as flattened cubics).
pub(crate) fn smooth(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let n = pts.len();
    let tangent = |i: usize| {
        let (a, b) = if i == 0 {
            (pts[0], pts[1])
        } else if i == n - 1 {
            (pts[n - 2], pts[n - 1])
        } else {
            (pts[i - 1], pts[i + 1])
        };
        let k = if i == 0 || i == n - 1 { 1.0 } else { 0.5 };
        [(b[0] - a[0]) * k, (b[1] - a[1]) * k]
    };
    let mut out = vec![pts[0]];
    for i in 0..n - 1 {
        let (a, b) = (pts[i], pts[i + 1]);
        let (ta, tb) = (tangent(i), tangent(i + 1));
        let c1 = [a[0] + ta[0] / 3.0, a[1] + ta[1] / 3.0];
        let c2 = [b[0] - tb[0] / 3.0, b[1] - tb[1] / 3.0];
        let steps = ((dist(a, c1) + dist(c1, c2) + dist(c2, b)) / 4.0).ceil().clamp(2.0, 48.0) as usize;
        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            let u = 1.0 - t;
            let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            out.push([
                b0 * a[0] + b1 * c1[0] + b2 * c2[0] + b3 * b[0],
                b0 * a[1] + b1 * c1[1] + b2 * c2[1] + b3 * b[1],
            ]);
        }
    }
    out
}

/// A line split at `;` outside quotes.
pub(crate) fn statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
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
