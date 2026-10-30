//! Quadrant charts (`quadrantChart`): `x-axis`/`y-axis` ends, four named quadrants, and points
//! at `[x, y]` in 0 to 1, with `radius`, `color`, `stroke-color` and `stroke-width` set inline
//! or through `classDef` and `:::class`.

use std::collections::HashMap;

use rune::{Align, Color};

use super::canvas::{Canvas, Measured, Scene, ellipse, fit, measure, measure_wrap, over_color, room};
use super::chart::{bold, small, soft, strong};
use super::{DiagramStyle, MAX_ITEMS, clean, parse_color_spec, word};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Look {
    pub radius: Option<f32>,
    pub color: Option<Color>,
    pub stroke: Option<Color>,
    pub stroke_width: Option<f32>,
}

impl Look {
    fn or(self, base: &Look) -> Look {
        Look {
            radius: self.radius.or(base.radius),
            color: self.color.or(base.color),
            stroke: self.stroke.or(base.stroke),
            stroke_width: self.stroke_width.or(base.stroke_width),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct Point {
    pub name: String,
    pub at: [f32; 2],
    pub look: Look,
}

#[derive(Debug, Default)]
pub(crate) struct Quadrant {
    pub title: Option<String>,
    /// Low and high ends; an empty high end means one label for the whole axis.
    pub x: [String; 2],
    pub y: [String; 2],
    /// Quadrants 1 (top right) to 4 (bottom right), anticlockwise.
    pub names: [String; 4],
    pub points: Vec<Point>,
}

pub(crate) fn parse(body: &[&str]) -> Option<Quadrant> {
    let mut q = Quadrant::default();
    let mut classes: HashMap<String, Look> = HashMap::new();
    let mut pending: Vec<(String, [f32; 2], Look, Option<String>)> = Vec::new();
    let mut any = false;
    for &l in body {
        let axis = |rest: &str| {
            let (a, b) = rest.split_once("-->").unwrap_or((rest, ""));
            [clean(a), clean(b)]
        };
        if let Some(rest) = word(l, "title") {
            q.title = Some(clean(rest)).filter(|t| !t.is_empty());
        } else if let Some(rest) = word(l, "x-axis") {
            q.x = axis(rest);
            any = true;
        } else if let Some(rest) = word(l, "y-axis") {
            q.y = axis(rest);
            any = true;
        } else if let Some((n, rest)) = l.strip_prefix("quadrant-").and_then(|r| r.split_once(char::is_whitespace)) {
            if let Ok(n @ 1..=4) = n.parse::<usize>() {
                q.names[n - 1] = clean(rest);
                any = true;
            }
        } else if let Some(rest) = word(l, "classDef") {
            let (name, props) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            classes.insert(name.to_owned(), look(props));
        } else if let Some(p) = point(l) {
            if pending.len() >= MAX_ITEMS {
                return None;
            }
            pending.push(p);
        }
    }
    q.points = pending
        .into_iter()
        .map(|(name, at, look, class)| {
            let base = class.and_then(|c| classes.get(&c).cloned()).unwrap_or_default();
            Point { name, at, look: look.or(&base) }
        })
        .collect();
    (any || !q.points.is_empty()).then_some(q)
}

/// `Name[:::class]: [x, y] props`.
fn point(l: &str) -> Option<(String, [f32; 2], Look, Option<String>)> {
    let open = l.find('[')?;
    let close = open + l[open..].find(']')?;
    let head = l[..open].trim().strip_suffix(':')?;
    let (name, class) = match head.split_once(":::") {
        Some((n, c)) => (n, Some(c.trim().to_owned())),
        None => (head, None),
    };
    let (x, y) = l[open + 1..close].split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    if !(x.is_finite() && y.is_finite()) {
        return None;
    }
    Some((clean(name.trim().trim_matches('"')), [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)], look(&l[close + 1..]), class))
}

fn look(props: &str) -> Look {
    let mut out = Look::default();
    for part in props.split(',') {
        let Some((k, v)) = part.split_once(':') else { continue };
        let v = v.trim();
        let px = || v.trim_end_matches("px").trim().parse::<f32>().ok().filter(|r| r.is_finite());
        match k.trim() {
            "radius" => out.radius = px().map(|r| r.clamp(1.0, 30.0)),
            "color" => out.color = parse_color_spec(v),
            "stroke-color" => out.stroke = parse_color_spec(v),
            "stroke-width" => out.stroke_width = px().map(|w| w.clamp(0.0, 8.0)),
            _ => {}
        }
    }
    out
}

const HUE: f32 = 255.0;

pub(crate) fn draw(q: &Quadrant, paint: &DiagramStyle) -> Option<Scene> {
    let note = small(paint);
    let head = bold(paint.font);
    let ys: Vec<Measured> = q.y.iter().map(|t| measure_wrap(t, note, 96.0)).collect();
    let yw = ys.iter().map(|m| m.size[0]).fold(0.0, f32::max);
    let x0 = if yw > 0.0 { yw + 12.0 } else { 0.0 };
    let side = (room(paint) - x0).clamp(220.0, 440.0);
    let half = side / 2.0;
    let (x1, y1) = (x0 + side, side);

    let mut c = Canvas::default();
    // 1 top right, 2 top left, 3 bottom left, 4 bottom right.
    let origins = [[x0 + half, 0.0], [x0, 0.0], [x0, half], [x0 + half, half]];
    let tints = [0.95, 0.7, 0.45, 0.7];
    for (o, a) in origins.iter().zip(tints) {
        let fill = over_color(soft(paint, HUE).alpha(a), paint.surface);
        c.fill(vec![*o, [o[0] + half, o[1]], [o[0] + half, o[1] + half], [o[0], o[1] + half]], fill);
    }
    c.stroke(vec![[x0 + half, 0.0], [x0 + half, y1]], false, 2.0, paint.surface);
    c.stroke(vec![[x0, half], [x1, half]], false, 2.0, paint.surface);
    for (o, name) in origins.iter().zip(&q.names) {
        let m = measure_wrap(name, head, half - 16.0);
        let y = if q.points.is_empty() { o[1] + half / 2.0 } else { o[1] + 10.0 + m.size[1] / 2.0 };
        c.label(&m, [o[0] + half / 2.0, y], head, paint.text, Align::Center);
    }
    let xs: Vec<Measured> = q.x.iter().map(|t| measure_wrap(t, note, half - 8.0)).collect();
    let below = y1 + 8.0;
    if xs[1].text.is_empty() {
        c.label_at(&xs[0], [x0 + half - xs[0].size[0] / 2.0, below], note, paint.muted, Align::Center);
    } else {
        for (i, m) in xs.iter().enumerate() {
            c.label_at(m, [x0 + half * (i as f32 + 0.5) - m.size[0] / 2.0, below], note, paint.muted, Align::Center);
        }
    }
    if ys[1].text.is_empty() {
        c.label_at(&ys[0], [x0 - 12.0 - ys[0].size[0], half - ys[0].size[1] / 2.0], note, paint.muted, Align::End);
    } else {
        // Low at the bottom, high at the top.
        for (i, m) in ys.iter().enumerate() {
            let mid = if i == 0 { half * 1.5 } else { half * 0.5 };
            c.label_at(m, [x0 - 12.0 - m.size[0], mid - m.size[1] / 2.0], note, paint.muted, Align::End);
        }
    }
    for p in &q.points {
        let at = [x0 + p.at[0] * side, y1 - p.at[1] * side];
        let r = p.look.radius.unwrap_or(5.0);
        let fill = p.look.color.unwrap_or_else(|| strong(paint, HUE));
        let width = p.look.stroke_width.unwrap_or(if p.look.stroke.is_some() { 2.0 } else { 0.0 });
        c.fill(ellipse(at, r, r), fill);
        if width > 0.0 {
            c.stroke(ellipse(at, r, r), true, width, p.look.stroke.unwrap_or(fill));
        }
        let m = measure(&p.name, note);
        c.label_at(&m, [at[0] - m.size[0] / 2.0, at[1] + r + 3.0], note, paint.text, Align::Center);
    }
    let scale = fit(c.width(), paint);
    c.finish(scale)
}

#[cfg(test)]
#[path = "quadrant_tests.rs"]
mod tests;
