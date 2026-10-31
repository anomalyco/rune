//! XY charts (`xychart-beta`, `xychart`): bars and lines over a category or numeric x-axis and a
//! value y-axis, upright or (`horizontal`) on their side.

use rune::Align;

use super::canvas::{Canvas, Measured, Scene, ellipse, measure, measure_wrap, room};
use super::chart::{axis_x, hue, small, solid};
use super::pie::number;
use super::{DiagramStyle, MAX_ITEMS, clean_quoted, word};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Bar,
    Line,
}

#[derive(Debug)]
pub(crate) struct Series {
    pub kind: Kind,
    pub name: String,
    pub values: Vec<f32>,
}

#[derive(Debug, Default)]
pub(crate) struct XyChart {
    pub title: Option<String>,
    pub horizontal: bool,
    pub x_title: String,
    /// Category names; empty when the x-axis is a range or not given.
    pub bands: Vec<String>,
    pub x_range: Option<[f32; 2]>,
    pub y_title: String,
    pub y_range: Option<[f32; 2]>,
    pub series: Vec<Series>,
}

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<XyChart> {
    let mut x = XyChart { horizontal: header.trim() == "horizontal", ..XyChart::default() };
    let mut items = 0;
    for &l in body {
        if let Some(rest) = word(l, "title") {
            x.title = Some(clean_quoted(rest)).filter(|t| !t.is_empty());
        } else if let Some(rest) = word(l, "x-axis") {
            let (title, bands, range) = axis(rest);
            items += bands.len();
            (x.x_title, x.bands, x.x_range) = (title, bands, range);
        } else if let Some(rest) = word(l, "y-axis") {
            let (title, _, range) = axis(rest);
            (x.y_title, x.y_range) = (title, range);
        } else if let Some((kind, rest)) = word(l, "bar").map(|r| (Kind::Bar, r)).or_else(|| word(l, "line").map(|r| (Kind::Line, r))) {
            let Some(open) = rest.find('[') else { continue };
            let values: Option<Vec<f32>> = list(&rest[open..]).iter().map(|v| v.parse::<f32>().ok().filter(|f| f.is_finite())).collect();
            let Some(values) = values.filter(|v| !v.is_empty()) else { continue };
            items += values.len();
            x.series.push(Series { kind, name: clean_quoted(&rest[..open]), values });
        }
        if items > MAX_ITEMS {
            return None;
        }
    }
    (!x.series.is_empty()).then_some(x)
}

/// `"Title" [a, b]`, `Title 0 --> 10`, or a title alone.
fn axis(rest: &str) -> (String, Vec<String>, Option<[f32; 2]>) {
    if let Some(open) = rest.find('[') {
        return (clean_quoted(&rest[..open]), list(&rest[open..]).iter().map(|s| clean_quoted(s)).collect(), None);
    }
    if let Some((a, b)) = rest.split_once("-->") {
        let a = a.trim();
        let (title, lo) = a.rsplit_once(char::is_whitespace).unwrap_or(("", a));
        if let (Ok(lo), Ok(hi)) = (lo.parse::<f32>(), b.trim().parse::<f32>())
            && lo.is_finite()
            && hi.is_finite()
            && lo != hi
        {
            return (clean_quoted(title), Vec::new(), Some([lo.min(hi), lo.max(hi)]));
        }
    }
    (clean_quoted(rest), Vec::new(), None)
}

/// The items of `[a, "b, c", d]`, trimmed.
fn list(s: &str) -> Vec<String> {
    let Some(inner) = s.trim().strip_prefix('[').and_then(|r| r.rfind(']').map(|e| &r[..e])) else { return Vec::new() };
    if inner.trim().is_empty() {
        return Vec::new();
    }
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for ch in inner.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_owned()).collect()
}

/// A round step near `raw`: 1, 2 or 5 times a power of ten.
fn nice(raw: f32) -> f32 {
    let p = 10f32.powf(raw.max(1e-9).log10().floor());
    let m = raw / p;
    p * if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    }
}

/// The value axis's ends and ticks: `range` as given, or the data's (taking in zero) rounded out.
pub(crate) fn scale(range: Option<[f32; 2]>, series: &[Series]) -> ([f32; 2], Vec<f32>) {
    let [lo, hi] = range.unwrap_or_else(|| {
        let (lo, hi) = series.iter().flat_map(|s| &s.values).fold((0.0f32, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
        let hi = if hi - lo < 1e-9 { lo + 1.0 } else { hi };
        let step = nice((hi - lo) / 5.0);
        [(lo / step).floor() * step, (hi / step).ceil() * step]
    });
    let step = nice((hi - lo) / 5.0);
    let first = (lo / step).ceil();
    let ticks = (0..12)
        .map(|k| (first + k as f32) * step)
        .take_while(|t| *t <= hi + step * 1e-3)
        .map(|t| if t.abs() < step * 1e-3 { 0.0 } else { t })
        .collect();
    ([lo, hi], ticks)
}

const PLOT_H: f32 = 240.0;

pub(crate) fn draw(x: &XyChart, paint: &DiagramStyle) -> Option<Scene> {
    let n = x.series.iter().map(|s| s.values.len()).max()?;
    let note = small(paint);
    let ([lo, hi], ticks) = scale(x.y_range, &x.series);
    let names: Vec<String> = match x.x_range {
        _ if !x.bands.is_empty() => (0..n).map(|i| x.bands.get(i).cloned().unwrap_or_default()).collect(),
        Some([a, b]) => (0..n).map(|i| number(f64::from(a + (b - a) * i as f32 / (n.max(2) - 1) as f32))).collect(),
        None => vec![String::new(); n],
    };
    let mut c = Canvas::default();

    let mut top = 0.0;
    if x.series.iter().any(|s| !s.name.is_empty()) {
        let mut at = 0.0;
        let mut h: f32 = 0.0;
        for (i, s) in x.series.iter().enumerate() {
            let m = measure(&s.name, note);
            c.fill(ellipse([at + 5.0, m.size[1] / 2.0], 5.0, 5.0), solid(paint, hue(i)));
            c.label_at(&m, [at + 14.0, 0.0], note, paint.muted, Align::Start);
            at += 14.0 + m.size[0] + 16.0;
            h = h.max(m.size[1]);
        }
        top = h + 12.0;
    }
    let value_title = measure(&x.y_title, note);
    let band_title = measure(&x.x_title, note);
    let values: Vec<(f32, Measured)> = ticks.iter().map(|&t| (t, measure(&number(f64::from(t)), note))).collect();
    let v01 = |v: f32| ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    let base = v01(0.0);

    // Plot bounds and where a value lands along a band, as `point(u px along the bands, v 0..1)`.
    let (x0, y0, x1, y1);
    let labels: Vec<Measured>;
    if x.horizontal {
        labels = names.iter().map(|t| measure_wrap(t, note, 120.0)).collect();
        let lw = labels.iter().map(|m| m.size[0]).fold(0.0, f32::max);
        let band = (labels.iter().map(|m| m.size[1]).fold(0.0, f32::max) + 10.0).max(26.0);
        if !band_title.text.is_empty() {
            c.label_at(&band_title, [0.0, top], note, paint.muted, Align::Start);
            top += band_title.size[1] + 6.0;
        }
        (x0, y0) = (if lw > 0.0 { lw + 10.0 } else { 0.0 }, top);
        (x1, y1) = ((room(paint)).max(x0 + 200.0), y0 + band * n as f32);
    } else {
        let vw = values.iter().map(|v| v.1.size[0]).fold(0.0, f32::max);
        if !value_title.text.is_empty() {
            c.label_at(&value_title, [0.0, top], note, paint.muted, Align::Start);
            top += value_title.size[1] + 8.0;
        }
        (x0, y0) = (vw + 10.0, top);
        (x1, y1) = (room(paint).max(x0 + 200.0), top + PLOT_H);
        let band = (x1 - x0) / n as f32;
        labels = names.iter().map(|t| measure_wrap(t, note, (band - 4.0).max(24.0))).collect();
    }
    let length = if x.horizontal { y1 - y0 } else { x1 - x0 };
    let band = length / n as f32;
    let point = |u: f32, v: f32| if x.horizontal { [x0 + v * (x1 - x0), y0 + u] } else { [x0 + u, y1 - v * (y1 - y0)] };

    // The value axis: grid lines and tick labels.
    if x.horizontal {
        let ticks: Vec<(f32, Measured)> = values.iter().map(|(t, m)| (point(0.0, v01(*t))[0], m.clone())).collect();
        axis_x(&mut c, &ticks, [x0, x1], [y0, y1], note, paint);
        if !value_title.text.is_empty() {
            let h = values.iter().map(|v| v.1.size[1]).fold(0.0, f32::max);
            c.label(&value_title, [(x0 + x1) / 2.0, y1 + h + 14.0 + value_title.size[1] / 2.0], note, paint.muted, Align::Center);
        }
    } else {
        for (t, m) in &values {
            let y = point(0.0, v01(*t))[1];
            c.stroke(vec![[x0, y], [x1, y]], false, 1.0, paint.text.alpha(0.08));
            c.label_at(m, [x0 - 8.0 - m.size[0], y - m.size[1] / 2.0], note, paint.muted, Align::End);
        }
    }
    let axis_line = [point(0.0, base), point(length, base)];
    c.stroke(axis_line.to_vec(), false, 1.0, paint.surface_border);

    // Band labels.
    for (i, m) in labels.iter().enumerate() {
        let mid = (i as f32 + 0.5) * band;
        if x.horizontal {
            c.label_at(m, [x0 - 10.0 - m.size[0], y0 + mid - m.size[1] / 2.0], note, paint.muted, Align::End);
        } else {
            c.label_at(m, [x0 + mid - m.size[0] / 2.0, y1 + 6.0], note, paint.muted, Align::Center);
        }
    }
    if !x.horizontal && !band_title.text.is_empty() {
        let h = labels.iter().map(|m| m.size[1]).fold(0.0, f32::max);
        c.label(&band_title, [(x0 + x1) / 2.0, y1 + h + 14.0 + band_title.size[1] / 2.0], note, paint.muted, Align::Center);
    }

    // Bars side by side within a band, then lines over them.
    let bars: Vec<usize> = (0..x.series.len()).filter(|&i| x.series[i].kind == Kind::Bar).collect();
    let group = band * 0.72;
    let each = group / bars.len().max(1) as f32;
    for (k, &i) in bars.iter().enumerate() {
        let fill = solid(paint, hue(i));
        for (b, &v) in x.series[i].values.iter().enumerate() {
            let u0 = b as f32 * band + (band - group) / 2.0 + k as f32 * each + each * 0.06;
            let u1 = u0 + each * 0.88;
            let (p, q) = (point(u0, base), point(u1, v01(v)));
            let (ax, ay, bx, by) = (p[0].min(q[0]), p[1].min(q[1]), p[0].max(q[0]), p[1].max(q[1]));
            c.fill(vec![[ax, ay], [bx, ay], [bx, by], [ax, by]], fill);
        }
    }
    for (i, s) in x.series.iter().enumerate().filter(|(_, s)| s.kind == Kind::Line) {
        let color = solid(paint, hue(i));
        let pts: Vec<[f32; 2]> = s.values.iter().enumerate().map(|(b, &v)| point((b as f32 + 0.5) * band, v01(v))).collect();
        c.stroke(pts.clone(), false, 2.0, color);
        for p in pts {
            c.shape(ellipse(p, 3.0, 3.0), color, paint.surface, 1.0);
        }
    }
    c.finish_fit(paint)
}

#[cfg(test)]
#[path = "xychart_tests.rs"]
mod tests;
