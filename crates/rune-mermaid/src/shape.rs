//! Flowchart node shapes: every Mermaid v11 shape, its size around a label, its outline (what
//! edges meet and hovering lights up) and its drawing.

use rune::{Align, Color};

use super::DiagramStyle;
use super::canvas::{Canvas, Measured, arc, ellipse, rounded_rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeShape {
    Rect,
    Round,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    DoubleCircle,
    Asymmetric,
    Diamond,
    Hexagon,
    /// `[/text/]`
    LeanRight,
    /// `[\text\]`
    LeanLeft,
    /// `[/text\]`, wider at the bottom.
    TrapBottom,
    /// `[\text/]`, wider at the top.
    TrapTop,
    SmallCircle,
    FilledCircle,
    FramedCircle,
    CrossedCircle,
    Bang,
    Cloud,
    Card,
    Hourglass,
    Bolt,
    BraceLeft,
    BraceRight,
    Braces,
    Delay,
    HCylinder,
    LinedCylinder,
    CurvedTrapezoid,
    DividedRect,
    Document,
    LinedDocument,
    Documents,
    TaggedDocument,
    Triangle,
    FlippedTriangle,
    Fork,
    WindowPane,
    LinedRect,
    NotchedPentagon,
    SlopedRect,
    StackedRect,
    Flag,
    BowRect,
    TaggedRect,
    Text,
    /// A sticky note with a folded corner, in the note colours.
    Note,
}

use NodeShape as S;

impl NodeShape {
    /// The shape a Mermaid `@{ shape: … }` name stands for, by short name or alias.
    pub(crate) fn named(name: &str) -> Option<Self> {
        Some(match name {
            "rect" | "proc" | "process" | "rectangle" | "icon" | "image" => S::Rect,
            "rounded" | "event" => S::Round,
            "stadium" | "pill" | "terminal" => S::Stadium,
            "fr-rect" | "framed-rectangle" | "subproc" | "subprocess" | "subroutine" => S::Subroutine,
            "cyl" | "cylinder" | "database" | "db" => S::Cylinder,
            "circle" | "circ" => S::Circle,
            "dbl-circ" | "double-circle" => S::DoubleCircle,
            "odd" => S::Asymmetric,
            "diam" | "decision" | "diamond" | "question" => S::Diamond,
            "hex" | "hexagon" | "prepare" => S::Hexagon,
            "lean-r" | "lean-right" | "in-out" => S::LeanRight,
            "lean-l" | "lean-left" | "out-in" => S::LeanLeft,
            "trap-b" | "priority" | "trapezoid" | "trapezoid-bottom" => S::TrapBottom,
            "trap-t" | "inv-trapezoid" | "manual" | "trapezoid-top" => S::TrapTop,
            "sm-circ" | "small-circle" | "start" => S::SmallCircle,
            "f-circ" | "filled-circle" | "junction" => S::FilledCircle,
            "fr-circ" | "framed-circle" | "stop" => S::FramedCircle,
            "cross-circ" | "crossed-circle" | "summary" => S::CrossedCircle,
            "bang" => S::Bang,
            "cloud" => S::Cloud,
            "notch-rect" | "card" | "notched-rectangle" => S::Card,
            "hourglass" | "collate" => S::Hourglass,
            "bolt" | "com-link" | "lightning-bolt" => S::Bolt,
            "brace" | "brace-l" | "comment" => S::BraceLeft,
            "brace-r" => S::BraceRight,
            "braces" => S::Braces,
            "delay" | "half-rounded-rectangle" => S::Delay,
            "h-cyl" | "das" | "horizontal-cylinder" => S::HCylinder,
            "lin-cyl" | "disk" | "lined-cylinder" => S::LinedCylinder,
            "curv-trap" | "curved-trapezoid" | "display" => S::CurvedTrapezoid,
            "div-rect" | "div-proc" | "divided-process" | "divided-rectangle" => S::DividedRect,
            "doc" | "document" => S::Document,
            "lin-doc" | "lined-document" => S::LinedDocument,
            "docs" | "documents" | "st-doc" | "stacked-document" => S::Documents,
            "tag-doc" | "tagged-document" => S::TaggedDocument,
            "tri" | "extract" | "triangle" => S::Triangle,
            "flip-tri" | "flipped-triangle" | "manual-file" => S::FlippedTriangle,
            "fork" | "join" => S::Fork,
            "win-pane" | "internal-storage" | "window-pane" => S::WindowPane,
            "lin-rect" | "lin-proc" | "lined-process" | "lined-rectangle" | "shaded-process" => S::LinedRect,
            "notch-pent" | "loop-limit" | "notched-pentagon" => S::NotchedPentagon,
            "sl-rect" | "manual-input" | "sloped-rectangle" => S::SlopedRect,
            "st-rect" | "processes" | "procs" | "stacked-rectangle" => S::StackedRect,
            "flag" | "paper-tape" => S::Flag,
            "bow-rect" | "bow-tie-rectangle" | "stored-data" => S::BowRect,
            "tag-rect" | "tag-proc" | "tagged-process" | "tagged-rectangle" => S::TaggedRect,
            "text" => S::Text,
            _ => return None,
        })
    }

    /// Markers too small to hold their label, which they don't draw.
    fn unlabelled(self) -> bool {
        matches!(self, S::SmallCircle | S::FilledCircle | S::FramedCircle | S::CrossedCircle | S::Fork)
    }

    /// Shapes whose side facing the next rank is straight, so edges can meet it anywhere along it.
    pub(crate) fn flat(self) -> bool {
        !matches!(
            self,
            S::Stadium
                | S::Cylinder
                | S::Circle
                | S::DoubleCircle
                | S::Diamond
                | S::Hexagon
                | S::LeanRight
                | S::LeanLeft
                | S::TrapBottom
                | S::TrapTop
                | S::SmallCircle
                | S::FilledCircle
                | S::FramedCircle
                | S::CrossedCircle
                | S::Bang
                | S::Cloud
                | S::LinedCylinder
                | S::Triangle
                | S::FlippedTriangle
        )
    }
}

const PAD: [f32; 2] = [30.0, 22.0];
const GLYPH: f32 = 32.0;
/// How far a stacked shape's back copies sit up and to the right.
const STACK: f32 = 5.0;

pub(crate) fn node_size(shape: NodeShape, t: [f32; 2]) -> [f32; 2] {
    let [w, h] = [t[0] + PAD[0], (t[1] + PAD[1]).max(40.0)];
    let rect = w.max(60.0);
    match shape {
        S::Rect | S::Round | S::TaggedRect | S::Note => [rect, h],
        S::Stadium | S::Hexagon => [w + h / 2.0, h],
        S::Subroutine | S::LinedRect | S::Card => [w + 16.0, h],
        S::Asymmetric => [w + h / 3.0, h],
        S::Cylinder => [rect, h + 16.0],
        S::LinedCylinder => [rect, h + 22.0],
        S::HCylinder => [w + 24.0, h],
        S::LeanRight | S::LeanLeft | S::TrapBottom | S::TrapTop => [w + h * 0.6, h],
        S::Circle => circle(t, 20.0),
        S::DoubleCircle => circle(t, 30.0),
        S::Diamond => {
            let a = t[0] / 2.0 + t[1] + 12.0;
            let b = t[1] / 2.0 * a / (t[1] + 12.0) + 10.0;
            [2.0 * a, (2.0 * b).max(40.0)]
        }
        S::SmallCircle | S::FilledCircle => [14.0, 14.0],
        S::FramedCircle => [20.0, 20.0],
        S::CrossedCircle => [30.0, 30.0],
        S::Fork => [70.0, 10.0],
        S::Bang => [w * 1.5, h * 1.7],
        S::Cloud => [w * 1.35, h * 1.55],
        S::Hourglass | S::Bolt => [t[0].max(GLYPH), if t[1] > 0.0 { GLYPH + 6.0 + t[1] } else { GLYPH }],
        S::BraceLeft | S::BraceRight => [w + 6.0, h],
        S::Braces => [w + 18.0, h],
        S::Delay | S::CurvedTrapezoid | S::BowRect => [w + h / 2.0, h],
        S::DividedRect => [rect, h + 10.0],
        S::Document | S::LinedDocument | S::TaggedDocument | S::Flag | S::SlopedRect => [rect + 8.0, h + 12.0],
        S::Documents => [rect + 8.0 + 2.0 * STACK, h + 12.0 + 2.0 * STACK],
        S::StackedRect => [rect + 2.0 * STACK, h + 2.0 * STACK],
        S::Triangle | S::FlippedTriangle => {
            let w = 1.6 * (t[0] + 16.0);
            [w.max(60.0), (0.7 * w).max(3.0 * t[1] + 16.0)]
        }
        S::WindowPane => [rect + 10.0, h + 10.0],
        S::NotchedPentagon => [w + 24.0, h],
        S::Text => [t[0] + 12.0, t[1] + 12.0],
    }
}

fn circle(t: [f32; 2], pad: f32) -> [f32; 2] {
    let d = ((t[0] * t[0] + t[1] * t[1]).sqrt() + pad).max(48.0);
    [d, d]
}

/// Where the line from `c` toward `toward` leaves a node of `shape` and `size` centred at `c`.
pub(crate) fn boundary(shape: NodeShape, c: [f32; 2], size: [f32; 2], toward: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (toward[0] - c[0], toward[1] - c[1]);
    if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
        return c;
    }
    let (a, b) = (size[0] / 2.0, size[1] / 2.0);
    let rect = || {
        let tx = if dx.abs() > 1e-3 { a / dx.abs() } else { f32::MAX };
        let ty = if dy.abs() > 1e-3 { b / dy.abs() } else { f32::MAX };
        tx.min(ty)
    };
    let t = match shape {
        S::Circle | S::DoubleCircle | S::SmallCircle | S::FilledCircle | S::FramedCircle | S::CrossedCircle => {
            1.0 / ((dx / a).powi(2) + (dy / b).powi(2)).sqrt()
        }
        S::Diamond => 1.0 / (dx.abs() / a + dy.abs() / b),
        S::Rect
        | S::Round
        | S::Subroutine
        | S::Asymmetric
        | S::Stadium
        | S::Hexagon
        | S::Cylinder
        | S::LeanRight
        | S::LeanLeft
        | S::TrapBottom
        | S::TrapTop
        | S::Hourglass
        | S::Bolt
        | S::BraceLeft
        | S::BraceRight
        | S::Braces
        | S::Text => rect(),
        _ => ray_exit(&outline(shape, c, size), c, [dx, dy]).unwrap_or_else(rect),
    };
    let t = t.min(1.0);
    [c[0] + dx * t, c[1] + dy * t]
}

/// The furthest `t` at which `c + t·d` crosses `poly`.
fn ray_exit(poly: &[[f32; 2]], c: [f32; 2], d: [f32; 2]) -> Option<f32> {
    let mut best: Option<f32> = None;
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let e = [q[0] - p[0], q[1] - p[1]];
        let den = d[0] * e[1] - d[1] * e[0];
        if den.abs() < 1e-9 {
            continue;
        }
        let w = [p[0] - c[0], p[1] - c[1]];
        let t = (w[0] * e[1] - w[1] * e[0]) / den;
        let u = (w[0] * d[1] - w[1] * d[0]) / den;
        if t > 0.0 && (0.0..=1.0).contains(&u) && best.is_none_or(|b| t > b) {
            best = Some(t);
        }
    }
    best
}

/// The box a node takes: `(x0, y0, x1, y1)`.
fn bounds(c: [f32; 2], s: [f32; 2]) -> (f32, f32, f32, f32) {
    (c[0] - s[0] / 2.0, c[1] - s[1] / 2.0, c[0] + s[0] / 2.0, c[1] + s[1] / 2.0)
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// A wavy line from `x0` to `x1` around `y`, one and a half periods long.
fn wave(x0: f32, x1: f32, y: f32, amp: f32) -> Vec<[f32; 2]> {
    let n = 24;
    (0..=n)
        .map(|i| {
            let f = i as f32 / n as f32;
            [x0 + (x1 - x0) * f, y + amp * (f * std::f32::consts::TAU * 1.5).sin()]
        })
        .collect()
}

/// A quadratic Bézier from `a` through control `k` to `b`, as points.
fn quad(a: [f32; 2], k: [f32; 2], b: [f32; 2]) -> Vec<[f32; 2]> {
    (0..=8)
        .map(|i| {
            let t = i as f32 / 8.0;
            let m = 1.0 - t;
            [m * m * a[0] + 2.0 * m * t * k[0] + t * t * b[0], m * m * a[1] + 2.0 * m * t * k[1] + t * t * b[1]]
        })
        .collect()
}

/// A curly brace down the line `x` from `y0` to `y1`, its point toward `side` (-1 left, 1 right).
fn brace(x: f32, y0: f32, y1: f32, side: f32) -> Vec<[f32; 2]> {
    let (r, cy) = (6.0, (y0 + y1) / 2.0);
    let mut p = quad([x - side * r, y0], [x, y0], [x, y0 + r]);
    p.extend(quad([x, cy - r], [x, cy], [x + side * r, cy]));
    p.extend(quad([x + side * r, cy], [x, cy], [x, cy + r]));
    p.extend(quad([x, y1 - r], [x, y1], [x - side * r, y1]));
    p
}

/// A document's outline: a box whose bottom edge waves.
fn document(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
    let mut p = vec![[x0, y0], [x1, y0]];
    p.extend(wave(x1, x0, y1 - 6.0, 5.0));
    p
}

/// The glyph's box for shapes drawn as a glyph over their label.
fn glyph_box(c: [f32; 2], s: [f32; 2]) -> (f32, f32, f32, f32) {
    let y0 = c[1] - s[1] / 2.0;
    (c[0] - GLYPH / 2.0, y0, c[0] + GLYPH / 2.0, y0 + GLYPH)
}

pub(crate) fn outline(shape: NodeShape, c: [f32; 2], s: [f32; 2]) -> Vec<[f32; 2]> {
    let (x0, y0, x1, y1) = bounds(c, s);
    let (h, cy) = (s[1], c[1]);
    match shape {
        S::Rect | S::Subroutine | S::DividedRect | S::LinedRect | S::WindowPane | S::TaggedRect => rounded_rect(x0, y0, x1, y1, 3.0),
        S::Round => rounded_rect(x0, y0, x1, y1, 10.0),
        S::Stadium => rounded_rect(x0, y0, x1, y1, h / 2.0),
        S::Circle | S::DoubleCircle | S::SmallCircle | S::FilledCircle | S::FramedCircle | S::CrossedCircle => {
            ellipse(c, s[0] / 2.0, s[1] / 2.0)
        }
        S::Diamond => vec![[c[0], y0], [x1, cy], [c[0], y1], [x0, cy]],
        S::Hexagon => {
            let i = h / 4.0;
            vec![[x0 + i, y0], [x1 - i, y0], [x1, cy], [x1 - i, y1], [x0 + i, y1], [x0, cy]]
        }
        S::Asymmetric => vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0 + h / 3.0, cy]],
        S::LeanRight => vec![[x0 + h * 0.3, y0], [x1, y0], [x1 - h * 0.3, y1], [x0, y1]],
        S::LeanLeft => vec![[x0, y0], [x1 - h * 0.3, y0], [x1, y1], [x0 + h * 0.3, y1]],
        S::TrapBottom => vec![[x0 + h * 0.3, y0], [x1 - h * 0.3, y0], [x1, y1], [x0, y1]],
        S::TrapTop => vec![[x0, y0], [x1, y0], [x1 - h * 0.3, y1], [x0 + h * 0.3, y1]],
        S::Cylinder | S::LinedCylinder => {
            let (rx, ry) = (s[0] / 2.0, 8.0);
            let mut pts = arc([c[0], y0 + ry], rx, ry, 180.0, 360.0, 20);
            pts.extend(arc([c[0], y1 - ry], rx, ry, 0.0, 180.0, 20));
            pts
        }
        S::HCylinder => {
            let (rx, ry) = (8.0, h / 2.0);
            let mut pts = arc([x1 - rx, cy], rx, ry, -90.0, 90.0, 20);
            pts.extend(arc([x0 + rx, cy], rx, ry, 90.0, 270.0, 20));
            pts
        }
        S::Bang => (0..28)
            .map(|i| {
                let a = (i as f32 * 360.0 / 28.0).to_radians();
                let r = if i % 2 == 0 { 1.0 } else { 0.8 };
                [c[0] + s[0] / 2.0 * r * a.cos(), cy + s[1] / 2.0 * r * a.sin()]
            })
            .collect(),
        S::Cloud => (0..120)
            .map(|i| {
                let a = (i as f32 * 3.0).to_radians();
                let r = 0.84 + 0.16 * (5.0 * a).sin().abs().sqrt();
                [c[0] + s[0] / 2.0 * r * a.cos(), cy + s[1] / 2.0 * r * a.sin()]
            })
            .collect(),
        S::Card => vec![[x0 + 12.0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0 + 12.0]],
        S::Note => vec![[x0, y0], [x1 - 10.0, y0], [x1, y0 + 10.0], [x1, y1], [x0, y1]],
        S::NotchedPentagon => vec![[x0 + 12.0, y0], [x1 - 12.0, y0], [x1, y0 + 12.0], [x1, y1], [x0, y1], [x0, y0 + 12.0]],
        S::SlopedRect => vec![[x0, y0 + 12.0], [x1, y0], [x1, y1], [x0, y1]],
        S::Delay => {
            let mut p = vec![[x0, y0]];
            p.extend(arc([x1 - h / 2.0, cy], h / 2.0, h / 2.0, -90.0, 90.0, 16));
            p.push([x0, y1]);
            p
        }
        S::CurvedTrapezoid => {
            let k = h / 4.0;
            let mut p = vec![[x0 + k, y0]];
            p.extend(arc([x1 - h / 2.0, cy], h / 2.0, h / 2.0, -90.0, 90.0, 16));
            p.extend(quad([x0 + k, y1], [x0, y1], [x0, cy]));
            p.extend(quad([x0, cy], [x0, y0], [x0 + k, y0]));
            p
        }
        S::BowRect => {
            let rx = h / 4.0;
            let mut p = arc([x1, cy], rx, h / 2.0, 270.0, 90.0, 16);
            p.extend(arc([x0 + rx, cy], rx, h / 2.0, 90.0, 270.0, 16));
            p
        }
        S::Document | S::LinedDocument | S::TaggedDocument => document(x0, y0, x1, y1),
        S::Documents => document(x0, y0 + 2.0 * STACK, x1 - 2.0 * STACK, y1),
        S::StackedRect => rect(x0, y0 + 2.0 * STACK, x1 - 2.0 * STACK, y1),
        S::Flag => {
            let mut p = wave(x0, x1, y0 + 6.0, 5.0);
            p.extend(wave(x1, x0, y1 - 6.0, 5.0));
            p
        }
        S::Triangle => vec![[c[0], y0], [x1, y1], [x0, y1]],
        S::FlippedTriangle => vec![[x0, y0], [x1, y0], [c[0], y1]],
        S::Fork => rounded_rect(x0, y0, x1, y1, 2.0),
        S::Hourglass | S::Bolt | S::BraceLeft | S::BraceRight | S::Braces | S::Text => rect(x0, y0, x1, y1),
    }
}

/// Draws a node of `shape` at `ctr` with its label, in `fill`, `border` and `text` colours.
#[allow(clippy::too_many_arguments, reason = "one node's whole drawing")]
pub(crate) fn draw(
    c: &mut Canvas,
    shape: NodeShape,
    label: &Measured,
    ctr: [f32; 2],
    size: [f32; 2],
    [fill, border, text]: [Color; 3],
    paint: &DiagramStyle,
) {
    let (x0, y0, x1, y1) = bounds(ctr, size);
    let h = size[1];
    let line = |c: &mut Canvas, pts: Vec<[f32; 2]>| c.stroke(pts, false, 1.0, border);
    let mut at = ctr;
    match shape {
        S::Text => {}
        S::BraceLeft => {
            c.stroke(brace(x0 + 6.0, y0 + 2.0, y1 - 2.0, -1.0), false, 1.25, border);
            at[0] += 3.0;
        }
        S::BraceRight => {
            c.stroke(brace(x1 - 6.0, y0 + 2.0, y1 - 2.0, 1.0), false, 1.25, border);
            at[0] -= 3.0;
        }
        S::Braces => {
            c.stroke(brace(x0 + 6.0, y0 + 2.0, y1 - 2.0, -1.0), false, 1.25, border);
            c.stroke(brace(x1 - 6.0, y0 + 2.0, y1 - 2.0, 1.0), false, 1.25, border);
        }
        S::FilledCircle | S::Fork => c.fill(outline(shape, ctr, size), paint.line),
        S::FramedCircle => {
            c.shape(outline(shape, ctr, size), fill, paint.line, 1.5);
            c.fill(ellipse(ctr, 5.5, 5.5), paint.line);
        }
        S::Hourglass => {
            let (g0, gy0, g1, gy1) = glyph_box(ctr, size);
            let mid = [ctr[0], (gy0 + gy1) / 2.0];
            c.shape(vec![[g0, gy0], [g1, gy0], mid, [g1, gy1], [g0, gy1], mid], fill, border, 1.0);
            at[1] = gy1 + 6.0 + label.size[1] / 2.0;
        }
        S::Bolt => {
            let (g0, gy0, _, _) = glyph_box(ctr, size);
            let p = |x: f32, y: f32| [g0 + x * GLYPH, gy0 + y * GLYPH];
            c.shape(vec![p(0.6, 0.0), p(0.15, 0.58), p(0.45, 0.58), p(0.35, 1.0), p(0.85, 0.4), p(0.55, 0.4), p(0.75, 0.0)], fill, border, 1.0);
            at[1] = gy0 + GLYPH + 6.0 + label.size[1] / 2.0;
        }
        S::Documents => {
            for k in [2.0, 1.0] {
                let d = k * STACK;
                c.shape(document(x0 + d, y0 + 2.0 * STACK - d, x1 - 2.0 * STACK + d, y1 - d), fill, border, 1.0);
            }
            c.shape(outline(shape, ctr, size), fill, border, 1.0);
            at = [ctr[0] - STACK, ctr[1] + STACK - 3.0];
        }
        S::StackedRect => {
            for k in [2.0, 1.0] {
                let d = k * STACK;
                c.shape(rect(x0 + d, y0 + 2.0 * STACK - d, x1 - 2.0 * STACK + d, y1 - d), fill, border, 1.0);
            }
            c.shape(outline(shape, ctr, size), fill, border, 1.0);
            at = [ctr[0] - STACK, ctr[1] + STACK];
        }
        S::Note => {
            c.shape(outline(shape, ctr, size), paint.note_fill, paint.note_border, 1.0);
            c.stroke(vec![[x1 - 10.0, y0], [x1 - 10.0, y0 + 10.0], [x1, y0 + 10.0]], false, 1.0, paint.note_border);
        }
        _ => {
            c.shape(outline(shape, ctr, size), fill, border, 1.0);
            match shape {
                S::Subroutine => {
                    line(c, vec![[x0 + 8.0, y0], [x0 + 8.0, y1]]);
                    line(c, vec![[x1 - 8.0, y0], [x1 - 8.0, y1]]);
                }
                S::Cylinder => {
                    line(c, arc([ctr[0], y0 + 8.0], size[0] / 2.0, 8.0, 0.0, 180.0, 20));
                    at[1] += 4.0;
                }
                S::LinedCylinder => {
                    line(c, arc([ctr[0], y0 + 8.0], size[0] / 2.0, 8.0, 0.0, 180.0, 20));
                    line(c, arc([ctr[0], y0 + 14.0], size[0] / 2.0, 8.0, 0.0, 180.0, 20));
                    at[1] += 7.0;
                }
                S::HCylinder => {
                    line(c, arc([x1 - 8.0, ctr[1]], 8.0, h / 2.0, 90.0, 270.0, 20));
                    at[0] -= 4.0;
                }
                S::DoubleCircle => c.stroke(ellipse(ctr, size[0] / 2.0 - 5.0, size[1] / 2.0 - 5.0), true, 1.0, border),
                S::CrossedCircle => {
                    let r = size[0] / 2.0 * std::f32::consts::FRAC_1_SQRT_2;
                    line(c, vec![[ctr[0] - r, ctr[1] - r], [ctr[0] + r, ctr[1] + r]]);
                    line(c, vec![[ctr[0] - r, ctr[1] + r], [ctr[0] + r, ctr[1] - r]]);
                }
                S::DividedRect => {
                    line(c, vec![[x0, y0 + 10.0], [x1, y0 + 10.0]]);
                    at[1] += 5.0;
                }
                S::LinedRect => {
                    line(c, vec![[x0 + 8.0, y0], [x0 + 8.0, y1]]);
                    at[0] += 4.0;
                }
                S::WindowPane => {
                    line(c, vec![[x0 + 10.0, y0], [x0 + 10.0, y1]]);
                    line(c, vec![[x0, y0 + 10.0], [x1, y0 + 10.0]]);
                    at = [at[0] + 5.0, at[1] + 5.0];
                }
                S::TaggedRect => line(c, vec![[x1 - 12.0, y1], [x1, y1 - 12.0]]),
                S::LinedDocument => {
                    line(c, vec![[x0 + 8.0, y0], [x0 + 8.0, y1 - 6.0]]);
                    at = [at[0] + 4.0, at[1] - 3.0];
                }
                S::TaggedDocument => {
                    line(c, vec![[x1 - 14.0, y1 - 4.0], [x1, y1 - 16.0]]);
                    at[1] -= 3.0;
                }
                S::Document => at[1] -= 3.0,
                S::SlopedRect => at[1] += 6.0,
                S::CurvedTrapezoid => at[0] -= h / 8.0,
                S::Delay => at[0] -= h / 6.0,
                S::Card => at[0] += 4.0,
                S::Triangle => at[1] = y0 + 0.68 * h,
                S::FlippedTriangle => at[1] = y0 + 0.32 * h,
                _ => {}
            }
        }
    }
    if !shape.unlabelled() {
        c.label(label, at, paint.font, text, Align::Center);
    }
}

#[cfg(test)]
#[path = "shape_tests.rs"]
mod tests;
