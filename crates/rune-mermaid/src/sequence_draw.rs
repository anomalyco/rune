//! Sequence diagram drawing: participants across the top, events down the page.

use rune::{Align, Color, TextStyle};

use super::canvas::{Canvas, Measured, Scene, arrowhead, ellipse, fit, measure, over_color, rounded_rect};
use super::sequence::{Event, Kind, NotePos, Sequence, Tip};
use super::shape::{self, NodeShape};
use super::{DiagramStyle, adapt_fill};

const BAR: f32 = 10.0;

struct Frame {
    kind: Option<Measured>,
    label: Measured,
    fill: Option<Color>,
    top: f32,
    dividers: Vec<(f32, Measured)>,
    x: [f32; 2],
}

pub(crate) fn draw(s: &Sequence, paint: &DiagramStyle) -> Option<Scene> {
    let font = paint.font;
    let small = TextStyle { size: font.size - 1.0, line_height: font.line_height - 2.0, ..font };
    let bold = TextStyle { weight: 600.0, ..small };
    let num_font = TextStyle { size: 10.0, line_height: 12.0, weight: 600.0, ..font };
    let n = s.participants.len();
    let heads: Vec<Measured> = s.participants.iter().map(|p| measure(&p.label, font)).collect();
    let boxes: Vec<[f32; 2]> = s.participants.iter().zip(&heads).map(|(p, m)| head_size(p.kind, m.size)).collect();
    let box_titles: Vec<Measured> = s.boxes.iter().map(|b| measure(&b.title, bold)).collect();
    let box_lead = if s.boxes.is_empty() {
        0.0
    } else {
        let th = box_titles.iter().map(|t| t.size[1]).fold(0.0, f32::max);
        if th > 0.0 { th + 18.0 } else { 10.0 }
    };
    let max_head_h = boxes.iter().map(|b| b[1]).fold(0.0, f32::max);
    let top_h = box_lead + max_head_h;
    let texts: Vec<Option<Measured>> = s
        .events
        .iter()
        .map(|e| match e {
            Event::Message(m) => Some(measure(&m.text, small)),
            Event::Note(_, t) => Some(measure(t, small)),
            Event::Open { label: t, .. } | Event::Else(t) => Some(measure(&if t.is_empty() { String::new() } else { format!("[{t}]") }, small)),
            _ => None,
        })
        .collect();
    let width_of = |i: usize| texts[i].as_ref().map_or(0.0, |m| m.size[0]);

    let number = if s.autonumber { 20.0 } else { 0.0 };
    let mut need: Vec<(usize, usize, f32)> = Vec::new();
    for (i, e) in s.events.iter().enumerate() {
        match e {
            Event::Message(m) if m.from != m.to => need.push((m.from.min(m.to), m.from.max(m.to), width_of(i) + 32.0 + number)),
            Event::Message(m) if m.from + 1 < n => need.push((m.from, m.from + 1, width_of(i) + 48.0)),
            Event::Note(NotePos::Right(a), _) if a + 1 < n => need.push((*a, a + 1, width_of(i) + 40.0)),
            Event::Note(NotePos::Left(a), _) if *a > 0 => need.push((a - 1, *a, width_of(i) + 40.0)),
            _ => {}
        }
    }
    let mut cx = vec![boxes[0][0] / 2.0 + if s.participants[0].group.is_some() { 10.0 } else { 0.0 }; n];
    for j in 1..n {
        let boundary_gap = if s.participants[j - 1].group != s.participants[j].group { 20.0 } else { 0.0 };
        cx[j] = cx[j - 1] + (boxes[j - 1][0] + boxes[j][0]) / 2.0 + 24.0 + boundary_gap;
        for &(a, b, d) in &need {
            if b == j {
                cx[j] = cx[j].max(cx[a] + d);
            }
        }
    }

    let (mut groups_bg, mut back, mut lifelines, mut bars, mut front, mut tops) = (
        Canvas::default(),
        Canvas::default(),
        Canvas::default(),
        Canvas::default(),
        Canvas::default(),
        Canvas::default(),
    );
    let mut tabs = Canvas::default();
    let mut active: Vec<Vec<f32>> = vec![Vec::new(); n];
    let mut frames: Vec<Frame> = Vec::new();
    let mut y = top_h + 20.0;
    let mut number: Option<(u32, u32)> = None;
    // Where each lifeline starts and ends, and who goes at their next message.
    let mut starts = vec![top_h; n];
    let mut ends: Vec<Option<f32>> = vec![None; n];
    let mut doomed: Vec<usize> = Vec::new();
    let edge_x = |active: &[Vec<f32>], i: usize, right: bool| {
        let d = active[i].len();
        if d == 0 { cx[i] } else { cx[i] + (d - 1) as f32 * 4.0 + if right { BAR / 2.0 } else { -BAR / 2.0 } }
    };
    let close_bar = |bars: &mut Canvas, active: &mut [Vec<f32>], i: usize, y: f32| {
        let d = active[i].len();
        if let Some(top) = active[i].pop() {
            let x = cx[i] + (d - 1) as f32 * 4.0;
            bars.shape(rounded_rect(x - BAR / 2.0, top, x + BAR / 2.0, y.max(top + 8.0), 1.0), paint.node_fill, paint.node_border, 1.0);
        }
    };
    let touch = |frames: &mut [Frame], x0: f32, x1: f32| {
        for f in frames.iter_mut() {
            f.x = [f.x[0].min(x0), f.x[1].max(x1)];
        }
    };

    for (i, e) in s.events.iter().enumerate() {
        match e {
            Event::Message(m) => {
                let t = texts[i].as_ref().expect("messages are measured");
                if m.activate {
                    active[m.to].push(y + t.size[1] + 6.0);
                }
                let line_y;
                if m.from == m.to {
                    let x = edge_x(&active, m.from, true);
                    front.label_at(t, [x + 8.0, y], small, paint.text, Align::Start);
                    line_y = y + t.size[1] + 6.0;
                    let pts = vec![[x, line_y], [x + 30.0, line_y], [x + 30.0, line_y + 18.0], [x + 2.0 + tip_len(m.tip), line_y + 18.0]];
                    stroke(&mut front, pts, m.dotted, paint);
                    tip(&mut front, m.tip, [x, line_y + 18.0], [-1.0, 0.0], paint);
                    touch(&mut frames, cx[m.from] - BAR, (x + 30.0).max(x + 8.0 + t.size[0]));
                    y = line_y + 18.0 + 16.0;
                } else {
                    let right = m.to > m.from;
                    let sx = edge_x(&active, m.from, right);
                    let ex = edge_x(&active, m.to, !right);
                    let dir = if right { 1.0 } else { -1.0 };
                    let mid = (sx + ex) / 2.0;
                    let has_text = !t.text.is_empty();
                    front.label(t, [mid, y + t.size[1] / 2.0], small, paint.text, Align::Center);
                    line_y = y + if has_text { t.size[1] + 6.0 } else { 4.0 };
                    let a = if m.both { sx + dir * tip_len(m.tip) } else { sx };
                    stroke(&mut front, vec![[a, line_y], [ex - dir * tip_len(m.tip), line_y]], m.dotted, paint);
                    tip(&mut front, m.tip, [ex, line_y], [dir, 0.0], paint);
                    if m.both {
                        tip(&mut front, m.tip, [sx, line_y], [-dir, 0.0], paint);
                    }
                    if let Some((k, step)) = &mut number {
                        let m = measure(&k.to_string(), num_font);
                        front.shape(rounded_rect(sx - (m.size[0] / 2.0 + 4.0).max(8.0), line_y - 8.0, sx + (m.size[0] / 2.0 + 4.0).max(8.0), line_y + 8.0, 8.0), paint.line, paint.line, 1.0);
                        front.label(&m, [sx, line_y], num_font, paint.surface, Align::Center);
                        *k = k.saturating_add(*step);
                    }
                    let (lo, hi) = (cx[m.from].min(cx[m.to]) - BAR, cx[m.from].max(cx[m.to]) + BAR);
                    touch(&mut frames, lo.min(mid - t.size[0] / 2.0), hi.max(mid + t.size[0] / 2.0));
                    y = line_y + 16.0;
                }
                if m.deactivate {
                    close_bar(&mut bars, &mut active, m.from, line_y);
                }
                let end_y = if m.from == m.to { line_y + 18.0 } else { line_y };
                doomed.retain(|&d| {
                    let gone = d == m.from || d == m.to;
                    if gone {
                        ends[d] = Some(end_y + 14.0);
                        cross(&mut front, [cx[d], end_y + 14.0], paint);
                        y = y.max(end_y + 30.0);
                    }
                    !gone
                });
            }
            Event::Create(a) => {
                let [w, h] = boxes[*a];
                let top = y;
                head(&mut tops, s.participants[*a].kind, &heads[*a], [cx[*a], top], [w, h], paint);
                starts[*a] = top + h;
                touch(&mut frames, cx[*a] - w / 2.0, cx[*a] + w / 2.0);
                y = top + h + 14.0;
            }
            Event::Destroy(a) => doomed.push(*a),
            Event::Number(on) => number = *on,
            Event::Note(pos, _) => {
                let t = texts[i].as_ref().expect("notes are measured");
                let w = t.size[0] + 20.0;
                let (x0, x1) = match *pos {
                    NotePos::Left(a) => (cx[a] - 12.0 - w, cx[a] - 12.0),
                    NotePos::Right(a) => (cx[a] + 12.0, cx[a] + 12.0 + w),
                    NotePos::Over(a, b) => {
                        let span = (cx[b] - cx[a] + 40.0).max(w);
                        let mid = (cx[a] + cx[b]) / 2.0;
                        (mid - span / 2.0, mid + span / 2.0)
                    }
                };
                let h = t.size[1] + 14.0;
                y += 2.0;
                front.shape(rounded_rect(x0, y, x1, y + h, 2.0), paint.note_fill, paint.note_border, 1.0);
                front.label(t, [(x0 + x1) / 2.0, y + h / 2.0], small, paint.text, Align::Center);
                touch(&mut frames, x0, x1);
                y += h + 14.0;
            }
            Event::Activate(a) => active[*a].push(y - 8.0),
            Event::Deactivate(a) => close_bar(&mut bars, &mut active, *a, y - 8.0),
            Event::Open { kind, fill, .. } => {
                let label = texts[i].clone().expect("frames are measured");
                let kind = kind.as_deref().map(|k| measure(k, bold));
                let h = kind.as_ref().map_or(if fill.is_some() { 8.0 } else { 0.0 }, |k| k.size[1].max(label.size[1]) + 14.0);
                frames.push(Frame { kind, label, fill: *fill, top: y, dividers: Vec::new(), x: [f32::MAX, f32::MIN] });
                y += h;
            }
            Event::Else(_) => {
                let label = texts[i].clone().expect("dividers are measured");
                let h = label.size[1] + 14.0;
                if let Some(f) = frames.last_mut() {
                    f.dividers.push((y, label));
                }
                y += h;
            }
            Event::End => {
                let Some(mut f) = frames.pop() else { continue };
                if f.x[0] > f.x[1] {
                    f.x = [cx[0], cx[n - 1]];
                }
                let x0 = f.x[0] - 12.0;
                let tab_w = f.kind.as_ref().map_or(0.0, |k| k.size[0] + 18.0);
                let x1 = (f.x[1] + 12.0).max(x0 + tab_w + f.label.size[0] + 20.0);
                let bottom = y - 4.0;
                if let Some(kind) = &f.kind {
                    let th = kind.size[1].max(f.label.size[1]) + 6.0;
                    back.stroke(vec![[x0, f.top], [x1, f.top], [x1, bottom], [x0, bottom]], true, 1.0, paint.node_border);
                    let tab = vec![[x0, f.top], [x0 + tab_w, f.top], [x0 + tab_w, f.top + th - 6.0], [x0 + tab_w - 6.0, f.top + th], [x0, f.top + th]];
                    tabs.shape(tab, paint.node_fill, paint.node_border, 1.0);
                    tabs.label_at(kind, [x0 + 9.0, f.top + 3.0], bold, paint.text, Align::Start);
                    tabs.label_at(&f.label, [x0 + tab_w + 8.0, f.top + 3.0], small, paint.muted, Align::Start);
                    for (dy, label) in &f.dividers {
                        back.dashed(&[[x0, *dy], [x1, *dy]], 1.0, paint.node_border, 4.0, 3.0);
                        tabs.label_at(label, [x0 + 9.0, dy + 4.0], small, paint.muted, Align::Start);
                    }
                } else if let Some(rect_col) = f.fill {
                    let band = adapt_fill(paint.surface, rect_col, 0.04, 0.03).alpha(0.55);
                    back.fill(rounded_rect(x0, f.top - 2.0, x1, bottom, 4.0), band);
                }
                touch(&mut frames, x0, x1);
                y = bottom + 16.0;
            }
        }
    }
    let end = y + 4.0;
    for a in 0..n {
        while !active[a].is_empty() {
            close_bar(&mut bars, &mut active, a, end - 8.0);
        }
    }

    // Participant `box` columns behind everything else.
    let box_border = over_color(paint.node_border.alpha(0.45), paint.surface_border);
    for (b_idx, b) in s.boxes.iter().enumerate() {
        let members: Vec<usize> = (0..n).filter(|&i| s.participants[i].group == Some(b_idx)).collect();
        let (Some(&first), Some(&last)) = (members.first(), members.last()) else { continue };
        let mut x0 = cx[first] - boxes[first][0] / 2.0 - 10.0;
        let mut x1 = cx[last] + boxes[last][0] / 2.0 + 10.0;
        let title = &box_titles[b_idx];
        if x1 - x0 < title.size[0] + 20.0 {
            let mid = (x0 + x1) / 2.0;
            x0 = mid - title.size[0] / 2.0 - 10.0;
            x1 = mid + title.size[0] / 2.0 + 10.0;
        }
        let fill = b.color.map_or(paint.cluster_fill, |col| adapt_fill(paint.surface, col, 0.035, 0.03));
        groups_bg.shape(rounded_rect(x0, 0.0, x1, end + max_head_h + 10.0, 6.0), fill, box_border, 1.0);
        groups_bg.label(title, [(x0 + x1) / 2.0, 7.0 + title.size[1] / 2.0], bold, paint.muted, Align::Center);
    }

    let created: Vec<bool> = (0..n).map(|i| s.events.iter().any(|e| matches!(e, Event::Create(a) if *a == i))).collect();
    for (i, p) in s.participants.iter().enumerate() {
        let size = boxes[i];
        let stop = ends[i].unwrap_or(end);
        lifelines.stroke(vec![[cx[i], starts[i]], [cx[i], stop]], false, 1.0, paint.node_border);
        if !created[i] {
            head(&mut tops, p.kind, &heads[i], [cx[i], top_h - size[1]], size, paint);
        }
        if ends[i].is_none() {
            head(&mut tops, p.kind, &heads[i], [cx[i], end], size, paint);
        }
    }
    let mut c = groups_bg;
    for layer in [back, lifelines, bars, tabs, front, tops] {
        c.append(layer);
    }
    let scale = fit(c.width(), paint);
    c.finish(scale)
}

fn tip_len(t: Tip) -> f32 {
    match t {
        Tip::Filled => 8.0,
        Tip::None | Tip::Open | Tip::Cross => 0.0,
    }
}

fn stroke(c: &mut Canvas, pts: Vec<[f32; 2]>, dotted: bool, paint: &DiagramStyle) {
    if dotted {
        c.dashed(&pts, 1.25, paint.line, 4.0, 3.0);
    } else {
        c.stroke(pts, false, 1.25, paint.line);
    }
}

fn tip(c: &mut Canvas, t: Tip, at: [f32; 2], d: [f32; 2], paint: &DiagramStyle) {
    let back = |by: f32, side: f32| [at[0] - d[0] * by - d[1] * side, at[1] - d[1] * by + d[0] * side];
    match t {
        Tip::None => {}
        Tip::Filled => c.fill(arrowhead(at, d, 9.0, 8.0), paint.line),
        Tip::Open => c.stroke(vec![back(8.0, 4.5), at, back(8.0, -4.5)], false, 1.25, paint.line),
        Tip::Cross => {
            c.stroke(vec![back(9.0, 4.0), back(1.0, -4.0)], false, 1.5, paint.line);
            c.stroke(vec![back(9.0, -4.0), back(1.0, 4.0)], false, 1.5, paint.line);
        }
    }
}

fn head_size(kind: Kind, t: [f32; 2]) -> [f32; 2] {
    match kind {
        Kind::Box => [(t[0] + 24.0).max(72.0), t[1] + 16.0],
        Kind::Actor => [t[0].max(40.0), 40.0 + t[1]],
        Kind::Boundary | Kind::Control | Kind::Entity => [t[0].max(44.0), 36.0 + t[1]],
        Kind::Database => shape::node_size(NodeShape::Cylinder, t),
        Kind::Collections => shape::node_size(NodeShape::StackedRect, t),
        Kind::Queue => shape::node_size(NodeShape::HCylinder, t),
    }
}

/// A participant's head, `size` wide and tall, its top centre at `top`.
fn head(c: &mut Canvas, kind: Kind, label: &Measured, top: [f32; 2], size: [f32; 2], paint: &DiagramStyle) {
    let [x, y] = top;
    let (w, h) = (size[0], size[1]);
    let ring = |c: &mut Canvas, at: [f32; 2], r: f32| c.shape(ellipse(at, r, r), paint.node_fill, paint.line, 1.25);
    let under = |c: &mut Canvas, glyph: f32| c.label(label, [x, y + glyph + label.size[1] / 2.0], paint.font, paint.text, Align::Center);
    let colors = [paint.node_fill, paint.node_border, paint.text];
    match kind {
        Kind::Box => {
            c.shape(rounded_rect(x - w / 2.0, y, x + w / 2.0, y + h, 4.0), paint.node_fill, paint.node_border, 1.0);
            c.label(label, [x, y + h / 2.0], paint.font, paint.text, Align::Center);
        }
        Kind::Actor => {
            actor(c, top, paint);
            under(c, 40.0);
        }
        Kind::Boundary => {
            ring(c, [x + 5.0, y + 15.0], 11.0);
            c.stroke(vec![[x - 14.0, y + 4.0], [x - 14.0, y + 26.0]], false, 1.25, paint.line);
            c.stroke(vec![[x - 14.0, y + 15.0], [x - 6.0, y + 15.0]], false, 1.25, paint.line);
            under(c, 36.0);
        }
        Kind::Control => {
            ring(c, [x, y + 17.0], 12.0);
            c.stroke(vec![[x + 4.0, y + 1.0], [x - 1.0, y + 5.0], [x + 4.0, y + 9.0]], false, 1.25, paint.line);
            under(c, 36.0);
        }
        Kind::Entity => {
            ring(c, [x, y + 14.0], 12.0);
            c.stroke(vec![[x - 12.0, y + 29.0], [x + 12.0, y + 29.0]], false, 1.25, paint.line);
            under(c, 36.0);
        }
        Kind::Database => shape::draw(c, NodeShape::Cylinder, label, [x, y + h / 2.0], size, colors, paint),
        Kind::Collections => shape::draw(c, NodeShape::StackedRect, label, [x, y + h / 2.0], size, colors, paint),
        Kind::Queue => shape::draw(c, NodeShape::HCylinder, label, [x, y + h / 2.0], size, colors, paint),
    }
}

/// The X that ends a destroyed participant's lifeline.
fn cross(c: &mut Canvas, at: [f32; 2], paint: &DiagramStyle) {
    let [x, y] = at;
    c.stroke(vec![[x - 7.0, y - 7.0], [x + 7.0, y + 7.0]], false, 2.0, paint.line);
    c.stroke(vec![[x - 7.0, y + 7.0], [x + 7.0, y - 7.0]], false, 2.0, paint.line);
}

fn actor(c: &mut Canvas, top: [f32; 2], paint: &DiagramStyle) {
    let [x, y] = top;
    c.shape(ellipse([x, y + 7.0], 7.0, 7.0), paint.node_fill, paint.line, 1.25);
    c.stroke(vec![[x, y + 14.0], [x, y + 26.0]], false, 1.25, paint.line);
    c.stroke(vec![[x - 10.0, y + 18.0], [x + 10.0, y + 18.0]], false, 1.25, paint.line);
    c.stroke(vec![[x - 8.0, y + 36.0], [x, y + 26.0], [x + 8.0, y + 36.0]], false, 1.25, paint.line);
}
