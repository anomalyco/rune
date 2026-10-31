//! Timelines (`timeline`): time periods left to right with their events stacked under them,
//! grouped and coloured by `section`. Too many periods to fit across run top to bottom instead.

use rune::Align;

use super::canvas::{Canvas, Measured, Scene, arrowhead, measure_wrap, room, rounded_rect};
use super::chart::{bold, hue, ink, small, soft, solid, strong};
use super::{DiagramStyle, MAX_ITEMS, clean, word};

#[derive(Debug)]
pub(crate) struct Period {
    pub time: String,
    pub events: Vec<String>,
    pub section: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Timeline {
    pub title: Option<String>,
    pub sections: Vec<String>,
    pub periods: Vec<Period>,
    /// `timeline TD`: top to bottom however many periods there are.
    pub down: bool,
}

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<Timeline> {
    let mut t = Timeline { title: None, sections: Vec::new(), periods: Vec::new(), down: header.trim() == "TD" || header.trim() == "TB" };
    let mut items = 0;
    for &l in body {
        if let Some(rest) = word(l, "title") {
            t.title = Some(clean(rest)).filter(|s| !s.is_empty());
            continue;
        }
        if let Some(rest) = word(l, "section") {
            t.sections.push(clean(rest));
            items += 1;
            continue;
        }
        if l.starts_with("accTitle") || l.starts_with("accDescr") {
            continue;
        }
        let mut parts = l.split(':').map(clean);
        let time = parts.next().unwrap_or_default();
        let events: Vec<String> = parts.filter(|e| !e.is_empty()).collect();
        items += events.len() + usize::from(!time.is_empty());
        if items > MAX_ITEMS {
            return None;
        }
        // `: event` on its own line adds to the period above.
        if time.is_empty() {
            if let Some(p) = t.periods.last_mut() {
                p.events.extend(events);
            }
            continue;
        }
        let section = t.sections.len().checked_sub(1);
        t.periods.push(Period { time, events, section });
    }
    (!t.periods.is_empty()).then_some(t)
}

const COL: f32 = 136.0;
const GAP: f32 = 12.0;
const PAD: [f32; 2] = [10.0, 7.0];

/// The hue of period `i`: its section's, or its own when there are none.
fn period_hue(t: &Timeline, i: usize) -> f32 {
    hue(t.periods[i].section.unwrap_or(i))
}

pub(crate) fn draw(t: &Timeline, paint: &DiagramStyle) -> Option<Scene> {
    let n = t.periods.len() as f32;
    if t.down || n * COL + (n - 1.0) * GAP > room(paint) / 0.75 {
        return draw_down(t, paint);
    }
    let head = bold(paint.font);
    let note = small(paint);
    let inner = COL - 2.0 * PAD[0];
    let times: Vec<Measured> = t.periods.iter().map(|p| measure_wrap(&p.time, head, inner)).collect();
    let events: Vec<Vec<Measured>> = t.periods.iter().map(|p| p.events.iter().map(|e| measure_wrap(e, note, inner)).collect()).collect();
    let titles: Vec<Measured> = t.sections.iter().map(|s| measure_wrap(s, head, 4.0 * COL)).collect();
    let x_of = |i: usize| i as f32 * (COL + GAP);

    let mut c = Canvas::default();
    let mut y = 0.0;
    if !t.sections.is_empty() {
        let band = titles.iter().map(|m| m.size[1]).fold(0.0, f32::max) + 2.0 * PAD[1];
        for (s, m) in titles.iter().enumerate() {
            let cols: Vec<usize> = (0..t.periods.len()).filter(|&i| t.periods[i].section == Some(s)).collect();
            let (Some(&a), Some(&b)) = (cols.first(), cols.last()) else { continue };
            let fill = solid(paint, hue(s));
            c.fill(rounded_rect(x_of(a), 0.0, x_of(b) + COL, band, 6.0), fill);
            c.label(m, [(x_of(a) + x_of(b) + COL) / 2.0, band / 2.0], head, ink(fill), Align::Center);
        }
        y = band + 10.0;
    }
    let box_h = times.iter().map(|m| m.size[1]).fold(0.0, f32::max) + 2.0 * PAD[1];
    let line_y = y + box_h + 18.0;
    let mut bottom = line_y;
    for (i, m) in times.iter().enumerate() {
        let h = period_hue(t, i);
        let x = x_of(i);
        let mut ey = line_y + 18.0;
        let boxes: Vec<(f32, f32)> = events[i]
            .iter()
            .map(|e| {
                let top = ey;
                ey += e.size[1] + 2.0 * PAD[1] + 8.0;
                (top, e.size[1] + 2.0 * PAD[1])
            })
            .collect();
        let end = boxes.last().map_or(line_y, |b| b.0 + b.1 / 2.0);
        c.dashed(&[[x + COL / 2.0, y + box_h], [x + COL / 2.0, end]], 1.0, strong(paint, h).alpha(0.6), 4.0, 3.0);
        c.shape(rounded_rect(x, y, x + COL, y + box_h, 6.0), soft(paint, h), strong(paint, h), 1.0);
        c.label(m, [x + COL / 2.0, y + box_h / 2.0], head, paint.text, Align::Center);
        for (e, (top, eh)) in events[i].iter().zip(boxes) {
            c.shape(rounded_rect(x + 6.0, top, x + COL - 6.0, top + eh, 5.0), paint.node_fill, strong(paint, h).alpha(0.55), 1.0);
            c.label(e, [x + COL / 2.0, top + eh / 2.0], note, paint.text, Align::Center);
            bottom = bottom.max(top + eh);
        }
    }
    let right = x_of(t.periods.len() - 1) + COL + 14.0;
    c.stroke(vec![[-6.0, line_y], [right - 8.0, line_y]], false, 2.0, paint.line);
    c.fill(arrowhead([right, line_y], [1.0, 0.0], 10.0, 9.0), paint.line);
    c.finish_fit(paint)
}

/// Periods down a line on the left, each with its events as chips flowing to its right.
fn draw_down(t: &Timeline, paint: &DiagramStyle) -> Option<Scene> {
    let head = bold(paint.font);
    let note = small(paint);
    let times: Vec<Measured> = t.periods.iter().map(|p| measure_wrap(&p.time, head, 140.0)).collect();
    let tw = times.iter().map(|m| m.size[0]).fold(0.0, f32::max);
    let spine = tw + 18.0;
    let x0 = spine + 18.0;
    let x1 = room(paint).max(x0 + 160.0);
    let mut c = Canvas::default();
    let mut y = 0.0;
    let mut section = None;
    for (i, (p, m)) in t.periods.iter().zip(&times).enumerate() {
        if p.section != section
            && let Some(s) = p.section
        {
            section = Some(s);
            let title = measure_wrap(&t.sections[s], head, x1 - 2.0 * PAD[0]);
            let band = title.size[1] + 2.0 * PAD[1];
            let fill = solid(paint, hue(s));
            c.fill(rounded_rect(0.0, y, x1, y + band, 6.0), fill);
            c.label_at(&title, [PAD[0], y + PAD[1]], head, ink(fill), Align::Start);
            y += band + 10.0;
        }
        let h = period_hue(t, i);
        let chips: Vec<Measured> = p.events.iter().map(|e| measure_wrap(e, note, x1 - x0 - 2.0 * PAD[0])).collect();
        let mut at = Vec::new();
        let (mut cx, mut cy, mut row_h) = (x0, y, 0.0f32);
        for chip in &chips {
            let w = chip.size[0] + 2.0 * PAD[0];
            if cx > x0 && cx + w > x1 {
                cx = x0;
                cy += row_h + 6.0;
                row_h = 0.0;
            }
            at.push([cx, cy]);
            cx += w + 6.0;
            row_h = row_h.max(chip.size[1] + 2.0 * PAD[1]);
        }
        let row_bottom = (cy + row_h).max(y + m.size[1] + 2.0 * PAD[1]);
        let mid = y + (m.size[1] + 2.0 * PAD[1]) / 2.0;
        c.label_at(m, [spine - 14.0 - m.size[0], mid - m.size[1] / 2.0], head, paint.text, Align::End);
        c.stroke(vec![[spine, y - 5.0], [spine, row_bottom + 5.0]], false, 2.0, paint.line);
        c.shape(super::canvas::ellipse([spine, mid], 5.0, 5.0), solid(paint, h), paint.surface, 2.0);
        for (chip, a) in chips.iter().zip(at) {
            let w = chip.size[0] + 2.0 * PAD[0];
            let ch = chip.size[1] + 2.0 * PAD[1];
            c.shape(rounded_rect(a[0], a[1], a[0] + w, a[1] + ch, 5.0), soft(paint, h), strong(paint, h).alpha(0.6), 1.0);
            c.label_at(chip, [a[0] + PAD[0], a[1] + PAD[1]], note, paint.text, Align::Start);
        }
        y = row_bottom + 10.0;
    }
    c.finish(1.0)
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;
