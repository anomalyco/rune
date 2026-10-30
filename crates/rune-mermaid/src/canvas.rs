//! Vector canvas, text measurement, geometry primitives and scene assembly for [`crate::render`].

use rune::{Align, Color, El, LineCap, Shape, TextStyle, canvas, col, el, text};

use crate::DiagramStyle;
use crate::hover::{Hover, Spot, Trace};

/// The card's padding on each side.
pub(crate) const CARD_PAD: f32 = 16.0;

/// The widest a diagram's drawing can be for its card to fit `paint.max_width`.
pub(crate) fn room(paint: &DiagramStyle) -> f32 {
    paint.max_width - 2.0 * CARD_PAD - 2.0
}

/// How much a canvas `width` points wide is drawn at to fit its card in `paint.max_width`.
pub(crate) fn fit(width: f32, paint: &DiagramStyle) -> f32 {
    if width <= room(paint) { 1.0 } else { (room(paint) / width).max(0.6) }
}

/// Text measured for a diagram: its size, and whether it wraps at [`WRAP`].
#[derive(Clone, Default)]
pub(crate) struct Measured {
    pub text: String,
    pub size: [f32; 2],
    pub wrap: bool,
}

/// Labels wider than this wrap.
const WRAP: f32 = 200.0;

pub(crate) fn measure(s: &str, font: TextStyle) -> Measured {
    measure_wrap(s, font, WRAP)
}

pub(crate) fn measure_wrap(s: &str, font: TextStyle, max_w: f32) -> Measured {
    if s.is_empty() {
        return Measured::default();
    }
    let (w, h) = rune::text::measure(s, &[], font, None);
    if w <= max_w {
        return Measured { text: s.to_owned(), size: [w, h], wrap: false };
    }
    let (w, h) = rune::text::measure(s, &[], font, Some(max_w));
    Measured { text: s.to_owned(), size: [w, h], wrap: true }
}

/// Straight-alpha `top` composited over `bottom` in linear sRGB.
pub(crate) fn over_color(top: Color, bottom: Color) -> Color {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 1e-6 {
        return Color::TRANSPARENT;
    }
    let m = |t: f32, b: f32| (t * top.a + b * bottom.a * (1.0 - top.a)) / a;
    Color { r: m(top.r, bottom.r), g: m(top.g, bottom.g), b: m(top.b, bottom.b), a }
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
        let shift = self.labels.len();
        self.ops.extend(other.ops);
        self.labels.extend(other.labels);
        self.traces.extend(other.traces.into_iter().map(|t| Trace { label: t.label.map(|(i, g)| (i + shift, g)), ..t }));
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
        let lit = self.hover.map_or(Color::TRANSPARENT, |h| h.1);
        Some(Scene { title: None, size, shape, labels, hovers, lit })
    }
}

/// A laid-out diagram, drawn the same way every build.
pub(crate) struct Scene {
    pub(crate) title: Option<String>,
    pub(crate) size: [f32; 2],
    shape: Shape,
    pub(crate) labels: Vec<Label>,
    /// The hover groups over edges, edge labels and nodes, and what each lights up.
    pub(crate) hovers: Vec<Spot>,
    /// The hover colour.
    lit: Color,
}

impl Scene {
    pub(crate) fn with_title(mut self, title: Option<String>) -> Self {
        if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
            self.title = Some(t);
        }
        self
    }

    pub(crate) fn el(&self, paint: &DiagramStyle) -> El {
        let [w, h] = self.size;
        let mut body = el().key("diagram").size(w, h).child(canvas(self.shape.clone()).key("shape").absolute(0.0, 0.0).size(w, h));
        let label = |l: &Label, color: Color, at: [f32; 2]| {
            // A point of slack so rounding never breaks a measured line.
            let t = text(l.text.clone()).font(l.font).color(color).align(l.align).w(l.size[0] + 2.0).absolute(at[0] - 1.0, at[1]);
            if l.wrap { t.wrap() } else { t }
        };
        for (i, l) in self.labels.iter().enumerate() {
            body = body.child(label(l, l.color, l.at).key(format!("t{i}")));
        }
        // Over the labels, so a lit label's backing covers the one it lights.
        for s in &self.hovers {
            let r = s.rect;
            let mut glow = el().absolute(-r[0], -r[1]).size(w, h).group_opacity(0.0, 1.0).child(canvas(s.lit.clone()).key("lit").absolute(0.0, 0.0).size(w, h));
            for &i in &s.labels {
                if let Some(l) = self.labels.get(i) {
                    glow = glow.child(label(l, self.lit, l.at).key(format!("t{i}")));
                }
            }
            body = body.child(el().key(s.key.clone()).absolute(r[0], r[1]).size(r[2], r[3]).hover_group().child(glow));
        }
        let mut card = col()
            .w_full()
            .items_center()
            .padding(CARD_PAD)
            .radius(6.0)
            .fill(paint.surface)
            .border(0.5, paint.surface_border)
            .clip_rounded(6.0);
        if let Some(title) = &self.title {
            let tf = TextStyle { size: paint.font.size + 1.0, line_height: paint.font.line_height + 2.0, weight: 600.0, ..paint.font };
            card = card.gap(12.0).child(text(title.clone()).key("title").font(tf).color(paint.text));
        }
        card.child(body)
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

/// A diamond head (for UML composition and aggregation) with its tip at `tip`, along `dir`.
pub(crate) fn diamond_head(tip: [f32; 2], dir: [f32; 2], len: f32, width: f32) -> Vec<[f32; 2]> {
    let mid = [tip[0] - dir[0] * len / 2.0, tip[1] - dir[1] * len / 2.0];
    let tail = [tip[0] - dir[0] * len, tip[1] - dir[1] * len];
    let n = [-dir[1] * width / 2.0, dir[0] * width / 2.0];
    vec![tip, [mid[0] + n[0], mid[1] + n[1]], tail, [mid[0] - n[0], mid[1] - n[1]]]
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
