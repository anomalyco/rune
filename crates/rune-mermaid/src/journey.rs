//! User journeys (`journey`): tasks left to right under their sections, each with a face at its
//! score (1 to 5) and dots for the actors in it, and a legend of actors.

use rune::{Align, oklch};

use super::canvas::{Canvas, Measured, Scene, arc, arrowhead, ellipse, measure, measure_wrap, rounded_rect};
use super::chart::{bold, dark, hue, ink, small, soft, solid, strong};
use super::{DiagramStyle, MAX_ITEMS, clean, word};

#[derive(Debug, PartialEq)]
pub(crate) struct Task {
    pub name: String,
    pub score: f32,
    pub actors: Vec<usize>,
    pub section: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Journey {
    pub title: Option<String>,
    pub sections: Vec<String>,
    pub actors: Vec<String>,
    pub tasks: Vec<Task>,
}

pub(crate) fn parse(body: &[&str]) -> Option<Journey> {
    let mut j = Journey { title: None, sections: Vec::new(), actors: Vec::new(), tasks: Vec::new() };
    for &l in body {
        if let Some(rest) = word(l, "title") {
            j.title = Some(clean(rest)).filter(|s| !s.is_empty());
            continue;
        }
        if let Some(rest) = word(l, "section") {
            j.sections.push(clean(rest));
            continue;
        }
        if l.starts_with("accTitle") || l.starts_with("accDescr") {
            continue;
        }
        let mut parts = l.splitn(3, ':');
        let (Some(name), Some(score)) = (parts.next(), parts.next()) else { continue };
        let Ok(score) = score.trim().parse::<f32>() else { continue };
        if !score.is_finite() {
            continue;
        }
        if j.tasks.len() + j.sections.len() >= MAX_ITEMS {
            return None;
        }
        let mut actors = Vec::new();
        for a in parts.next().unwrap_or("").split(',').map(clean).filter(|a| !a.is_empty()) {
            let i = j.actors.iter().position(|x| *x == a).unwrap_or_else(|| {
                j.actors.push(a);
                j.actors.len() - 1
            });
            if !actors.contains(&i) {
                actors.push(i);
            }
        }
        j.tasks.push(Task { name: clean(name), score: score.clamp(1.0, 5.0), actors, section: j.sections.len().checked_sub(1) });
    }
    (!j.tasks.is_empty()).then_some(j)
}

const COL: f32 = 112.0;
const GAP: f32 = 10.0;
const STEP: f32 = 20.0;
const FACE: f32 = 11.0;
const DOT: f32 = 4.5;

fn actor_hue(i: usize) -> f32 {
    hue(i + 6)
}

/// Green for 5 down to red for 1.
fn mood(score: f32, paint: &DiagramStyle) -> rune::Color {
    let h = 25.0 + (score - 1.0) / 4.0 * 125.0;
    if dark(paint) { oklch(0.7, 0.14, h) } else { oklch(0.8, 0.14, h) }
}

pub(crate) fn draw(j: &Journey, paint: &DiagramStyle) -> Option<Scene> {
    let head = bold(paint.font);
    let note = small(paint);
    let x_of = |i: usize| i as f32 * (COL + GAP);
    let mut c = Canvas::default();

    // The legend: one row of actors.
    let mut y = 0.0;
    if !j.actors.is_empty() {
        let mut x = 0.0;
        let names: Vec<Measured> = j.actors.iter().map(|a| measure(a, note)).collect();
        let h = names.iter().map(|m| m.size[1]).fold(0.0, f32::max);
        for (i, m) in names.iter().enumerate() {
            c.fill(ellipse([x + DOT + 1.0, h / 2.0], DOT + 1.0, DOT + 1.0), solid(paint, actor_hue(i)));
            c.label_at(m, [x + 2.0 * DOT + 8.0, 0.0], note, paint.muted, Align::Start);
            x += 2.0 * DOT + 8.0 + m.size[0] + 16.0;
        }
        y = h + 12.0;
    }

    if !j.sections.is_empty() {
        let titles: Vec<Measured> = j.sections.iter().map(|s| measure_wrap(s, head, 3.0 * COL)).collect();
        let band = titles.iter().map(|m| m.size[1]).fold(0.0, f32::max) + 14.0;
        for (s, m) in titles.iter().enumerate() {
            let cols: Vec<usize> = (0..j.tasks.len()).filter(|&i| j.tasks[i].section == Some(s)).collect();
            let (Some(&a), Some(&b)) = (cols.first(), cols.last()) else { continue };
            let fill = solid(paint, hue(s));
            c.fill(rounded_rect(x_of(a), y, x_of(b) + COL, y + band, 6.0), fill);
            c.label(m, [(x_of(a) + x_of(b) + COL) / 2.0, y + band / 2.0], head, ink(fill), Align::Center);
        }
        y += band + 8.0;
    }

    let names: Vec<Measured> = j.tasks.iter().map(|t| measure_wrap(&t.name, paint.font, COL - 16.0)).collect();
    let box_h = names.iter().map(|m| m.size[1]).fold(0.0, f32::max) + 20.0;
    let faces_top = y + box_h + 22.0 + FACE;
    let faces_bottom = faces_top + 4.0 * STEP;
    for (i, (t, m)) in j.tasks.iter().zip(&names).enumerate() {
        let x = x_of(i);
        let h = hue(t.section.unwrap_or(0));
        let mid = x + COL / 2.0;
        let face = [mid, faces_top + (5.0 - t.score) * STEP];
        c.dashed(&[[mid, y + box_h], [mid, faces_bottom + FACE + 8.0]], 1.0, paint.text.alpha(0.25), 4.0, 3.0);
        c.shape(rounded_rect(x, y, x + COL, y + box_h, 6.0), soft(paint, h), strong(paint, h), 1.0);
        c.label(m, [mid, y + box_h / 2.0 + 2.0], paint.font, paint.text, Align::Center);
        for (k, &a) in t.actors.iter().enumerate() {
            c.shape(ellipse([x + 12.0 + k as f32 * (2.0 * DOT + 4.0), y], DOT, DOT), solid(paint, actor_hue(a)), paint.surface, 1.5);
        }
        draw_face(&mut c, face, t.score, paint);
    }
    let base = faces_bottom + FACE + 8.0;
    let right = x_of(j.tasks.len() - 1) + COL + 12.0;
    c.stroke(vec![[-4.0, base], [right - 8.0, base]], false, 1.5, paint.line);
    c.fill(arrowhead([right, base], [1.0, 0.0], 9.0, 8.0), paint.line);
    c.finish_fit(paint)
}

fn draw_face(c: &mut Canvas, at: [f32; 2], score: f32, paint: &DiagramStyle) {
    let line = oklch(0.25, 0.02, 60.0);
    c.shape(ellipse(at, FACE, FACE), mood(score, paint), line.alpha(0.6), 1.0);
    for dx in [-3.8, 3.8] {
        c.fill(ellipse([at[0] + dx, at[1] - 3.0], 1.4, 1.6), line);
    }
    let mouth = if score > 3.0 {
        arc([at[0], at[1] + 1.0], 5.0, 4.0, 20.0, 160.0, 12)
    } else if score < 3.0 {
        arc([at[0], at[1] + 8.0], 5.0, 3.5, 200.0, 340.0, 12)
    } else {
        vec![[at[0] - 4.5, at[1] + 4.5], [at[0] + 4.5, at[1] + 4.5]]
    };
    c.stroke(mouth, false, 1.3, line);
}

#[cfg(test)]
#[path = "journey_tests.rs"]
mod tests;
