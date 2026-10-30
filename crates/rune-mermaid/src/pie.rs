//! Pie charts (`pie`): slices from twelve o'clock clockwise in source order, percentages on the
//! slices wide enough to hold them, and a legend (with values under `showData`).

use rune::{Align, TextStyle};

use super::canvas::{Canvas, Measured, Scene, arc, measure, room, rounded_rect};
use super::chart::{hue, ink, solid};
use super::{DiagramStyle, MAX_ITEMS, clean_quoted, statements};

#[derive(Debug)]
pub(crate) struct Pie {
    pub title: Option<String>,
    pub show_data: bool,
    pub slices: Vec<(String, f64)>,
}

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<Pie> {
    let mut p = Pie { title: None, show_data: false, slices: Vec::new() };
    for line in std::iter::once(header).chain(body.iter().copied()) {
        for stmt in statements(line) {
            let mut s = stmt;
            if let Some(rest) = s.strip_prefix("showData") {
                p.show_data = true;
                s = rest.trim();
            }
            if let Some(rest) = s.strip_prefix("title").filter(|r| r.is_empty() || r.starts_with(char::is_whitespace)) {
                p.title = Some(clean_quoted(rest)).filter(|t| !t.is_empty());
                continue;
            }
            if s.is_empty() || s.starts_with("accTitle") || s.starts_with("accDescr") {
                continue;
            }
            let Some((label, value)) = s.rsplit_once(':') else { continue };
            let Ok(v) = value.trim().parse::<f64>() else { continue };
            if !(v.is_finite() && v > 0.0) {
                continue;
            }
            if p.slices.len() >= MAX_ITEMS {
                return None;
            }
            p.slices.push((clean_quoted(label), v));
        }
    }
    (!p.slices.is_empty()).then_some(p)
}

/// `386`, `12.5`, `0.33`.
pub(crate) fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{v:.0}");
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

const R: f32 = 100.0;
const SWATCH: f32 = 12.0;

pub(crate) fn draw(p: &Pie, paint: &DiagramStyle) -> Option<Scene> {
    let small = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
    let pct_font = TextStyle { weight: 600.0, ..small };
    let total: f64 = p.slices.iter().map(|s| s.1).sum();
    let names: Vec<Measured> = p.slices.iter().map(|s| measure(&s.0, paint.font)).collect();
    let values: Vec<Option<Measured>> = p.slices.iter().map(|s| p.show_data.then(|| measure(&number(s.1), paint.font))).collect();
    let row_h = paint.font.line_height.max(SWATCH) + 6.0;
    let legend_w = names
        .iter()
        .zip(&values)
        .map(|(n, v)| SWATCH + 8.0 + n.size[0] + v.as_ref().map_or(0.0, |v| v.size[0] + 8.0))
        .fold(0.0, f32::max);
    let legend_h = p.slices.len() as f32 * row_h - 6.0;
    let beside = 2.0 * R + 32.0 + legend_w <= room(paint);
    let (centre, legend) = if beside {
        let h = legend_h.max(2.0 * R);
        ([R, h / 2.0], [2.0 * R + 32.0, (h - legend_h) / 2.0])
    } else {
        ([R.max(legend_w / 2.0), R], [(R - legend_w / 2.0).max(0.0), 2.0 * R + 20.0])
    };

    let mut c = Canvas::default();
    let mut a = -90.0f32;
    let mut edges = Vec::new();
    for (i, s) in p.slices.iter().enumerate() {
        let sweep = (s.1 / total) as f32 * 360.0;
        let steps = ((sweep / 4.0).ceil() as usize).max(2);
        let mut pts = arc(centre, R, R, a, a + sweep, steps);
        if p.slices.len() > 1 {
            pts.push(centre);
        }
        c.fill(pts, solid(paint, hue(i)));
        edges.push(a);
        a += sweep;
    }
    if p.slices.len() > 1 {
        for a in edges {
            let r = a.to_radians();
            c.stroke(vec![centre, [centre[0] + R * r.cos(), centre[1] + R * r.sin()]], false, 2.0, paint.surface);
        }
    }
    let mut a = -90.0f32;
    for (i, s) in p.slices.iter().enumerate() {
        let frac = s.1 / total;
        let sweep = frac as f32 * 360.0;
        let m = measure(&format!("{:.0}%", frac * 100.0), pct_font);
        // Only where the slice is wide enough at that radius to hold it.
        let room_across = 2.0 * R * 0.66 * (sweep / 2.0).min(90.0).to_radians().sin();
        if p.slices.len() == 1 || m.size[0] + 6.0 <= room_across {
            let mid = (a + sweep / 2.0).to_radians();
            let at = if p.slices.len() == 1 { centre } else { [centre[0] + R * 0.66 * mid.cos(), centre[1] + R * 0.66 * mid.sin()] };
            c.label(&m, at, pct_font, ink(solid(paint, hue(i))), Align::Center);
        }
        a += sweep;
    }
    for (i, (n, v)) in names.iter().zip(&values).enumerate() {
        let y = legend[1] + i as f32 * row_h;
        let mid = y + (row_h - 6.0) / 2.0;
        let x = legend[0];
        c.fill(rounded_rect(x, mid - SWATCH / 2.0, x + SWATCH, mid + SWATCH / 2.0, 3.0), solid(paint, hue(i)));
        c.label_at(n, [x + SWATCH + 8.0, mid - n.size[1] / 2.0], paint.font, paint.text, Align::Start);
        if let Some(v) = v {
            c.label_at(v, [x + SWATCH + 16.0 + n.size[0], mid - v.size[1] / 2.0], paint.font, paint.muted, Align::Start);
        }
    }
    c.finish(1.0)
}

#[cfg(test)]
#[path = "pie_tests.rs"]
mod tests;
