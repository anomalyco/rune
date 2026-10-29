//! Sequence diagrams: the parser for `sequenceDiagram` sources and their drawing.

use std::collections::HashMap;

use rune::{Align, Color, TextStyle};

use super::canvas::{
    Canvas, Measured, Scene, arrowhead, ellipse, fit, measure, over_color, rounded_rect,
};
use super::{DiagramStyle, MAX_ITEMS, adapt_fill, clean, clean_quoted, parse_color_spec};

#[derive(Debug)]
pub(crate) struct Participant {
    #[cfg_attr(not(test), allow(dead_code, reason = "tests check parsing by id"))]
    pub id: String,
    pub label: String,
    pub actor: bool,
    pub group: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct ParticipantBox {
    pub title: String,
    pub color: Option<Color>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tip {
    None,
    Filled,
    Open,
    Cross,
}

#[derive(Debug)]
pub(crate) struct Message {
    pub from: usize,
    pub to: usize,
    pub text: String,
    pub dotted: bool,
    pub tip: Tip,
    pub both: bool,
    pub activate: bool,
    pub deactivate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotePos {
    Left(usize),
    Right(usize),
    Over(usize, usize),
}

#[derive(Debug)]
pub(crate) enum Event {
    Message(Message),
    Note(NotePos, String),
    Activate(usize),
    Deactivate(usize),
    /// A frame (`Some(kind)`) or a `rect` highlight band (`None`, with optional colour).
    Open { kind: Option<String>, label: String, fill: Option<Color> },
    Else(String),
    End,
}

#[derive(Debug)]
pub(crate) struct Sequence {
    pub title: Option<String>,
    pub participants: Vec<Participant>,
    pub boxes: Vec<ParticipantBox>,
    pub events: Vec<Event>,
    pub autonumber: bool,
}

pub(crate) fn parse(_header: &str, body: &[&str]) -> Option<Sequence> {
    let mut p = Parser {
        s: Sequence { title: None, participants: Vec::new(), boxes: Vec::new(), events: Vec::new(), autonumber: false },
        ids: HashMap::new(),
        depth: 0,
        active_box: None,
    };
    for line in body {
        p.line(line)?;
        if p.s.participants.len() > 40 || p.s.events.len() > MAX_ITEMS {
            return None;
        }
    }
    if p.active_box.is_some() || p.depth != 0 {
        return None;
    }
    (!p.s.participants.is_empty()).then_some(p.s)
}

struct Parser {
    s: Sequence,
    ids: HashMap<String, usize>,
    depth: usize,
    active_box: Option<usize>,
}

const ARROWS: &[(&str, bool, Tip, bool)] = &[
    ("<<-->>", true, Tip::Filled, true),
    ("<<->>", false, Tip::Filled, true),
    ("-->>", true, Tip::Filled, false),
    ("->>", false, Tip::Filled, false),
    ("--x", true, Tip::Cross, false),
    ("-x", false, Tip::Cross, false),
    ("--)", true, Tip::Open, false),
    ("-)", false, Tip::Open, false),
    ("-->", true, Tip::None, false),
    ("->", false, Tip::None, false),
];

impl Parser {
    fn line(&mut self, l: &str) -> Option<()> {
        let l = l.trim().trim_end_matches(';').trim_end();
        let (word, rest) = l.split_once(char::is_whitespace).map_or((l, ""), |(w, r)| (w, r.trim()));
        match word.trim_end_matches(':') {
            "participant" | "actor" => self.declare(rest, word == "actor")?,
            "create" => {
                let (kind, name) = rest.split_once(char::is_whitespace).map_or((rest, ""), |(w, r)| (w, r.trim()));
                match kind {
                    "participant" | "actor" => self.declare(name, kind == "actor")?,
                    _ => return None,
                }
            }
            "title" => {
                let t = clean_quoted(l["title".len()..].trim().trim_start_matches(':'));
                if !t.is_empty() {
                    self.s.title = Some(t);
                }
            }
            "destroy" | "accTitle" | "accDescr" | "link" | "links" | "properties" | "details" => {}
            "autonumber" => self.s.autonumber = true,
            "activate" => {
                let i = self.id(rest)?;
                self.s.events.push(Event::Activate(i));
            }
            "deactivate" => {
                let i = self.id(rest)?;
                self.s.events.push(Event::Deactivate(i));
            }
            "Note" | "note" => self.note(rest)?,
            kind @ ("loop" | "alt" | "opt" | "par" | "critical" | "break") => {
                self.depth += 1;
                self.s.events.push(Event::Open { kind: Some(kind.to_owned()), label: clean(rest), fill: None });
            }
            "rect" => {
                self.depth += 1;
                self.s.events.push(Event::Open { kind: None, label: String::new(), fill: parse_color_spec(rest) });
            }
            "box" => {
                if self.active_box.is_some() {
                    return None;
                }
                let (color, title) = parse_box_header(rest);
                self.s.boxes.push(ParticipantBox { title, color });
                self.active_box = Some(self.s.boxes.len() - 1);
            }
            "else" | "and" | "option" => {
                if self.depth == 0 {
                    return None;
                }
                self.s.events.push(Event::Else(clean(rest)));
            }
            "end" => {
                if self.active_box.is_some() {
                    self.active_box = None;
                } else {
                    self.depth = self.depth.checked_sub(1)?;
                    self.s.events.push(Event::End);
                }
            }
            _ => self.message(l)?,
        }
        Some(())
    }

    fn declare(&mut self, rest: &str, actor: bool) -> Option<()> {
        if rest.contains("@{") {
            return None;
        }
        let (id, label) = match rest.split_once(" as ") {
            Some((id, label)) => (id.trim(), clean_quoted(label)),
            None => (rest.trim(), clean_quoted(rest)),
        };
        let i = self.id(id)?;
        let p = &mut self.s.participants[i];
        p.label = label;
        p.actor = actor;
        if p.group.is_none() {
            p.group = self.active_box;
        }
        Some(())
    }

    fn id(&mut self, name: &str) -> Option<usize> {
        let name = name.trim().trim_matches('"');
        if name.is_empty() || name.contains([':', ',']) {
            return None;
        }
        if let Some(&i) = self.ids.get(name) {
            return Some(i);
        }
        self.s.participants.push(Participant { id: name.to_owned(), label: clean(name), actor: false, group: self.active_box });
        self.ids.insert(name.to_owned(), self.s.participants.len() - 1);
        Some(self.s.participants.len() - 1)
    }

    fn note(&mut self, rest: &str) -> Option<()> {
        let (place, text) = rest.split_once(':')?;
        let place = place.trim();
        let lower = place.to_ascii_lowercase();
        let pos = if lower.starts_with("left of ") {
            NotePos::Left(self.id(&place["left of ".len()..])?)
        } else if lower.starts_with("right of ") {
            NotePos::Right(self.id(&place["right of ".len()..])?)
        } else if lower.starts_with("over ") {
            let who = &place["over ".len()..];
            match who.split_once(',') {
                Some((a, b)) => {
                    let (a, b) = (self.id(a)?, self.id(b)?);
                    NotePos::Over(a.min(b), a.max(b))
                }
                None => {
                    let a = self.id(who)?;
                    NotePos::Over(a, a)
                }
            }
        } else {
            return None;
        };
        self.s.events.push(Event::Note(pos, clean_quoted(text)));
        Some(())
    }

    fn message(&mut self, l: &str) -> Option<()> {
        let (lhs, text) = l.split_once(':').unwrap_or((l, ""));
        let (at, &(tok, dotted, tip, both)) = lhs
            .char_indices()
            .filter(|&(_, ch)| ch == '-' || ch == '<')
            .find_map(|(i, _)| ARROWS.iter().find(|a| lhs[i..].starts_with(a.0)).map(|a| (i, a)))?;
        let from = lhs[..at].trim();
        let rest = lhs[at + tok.len()..].trim_start();
        let (activate, deactivate, to) = match rest.as_bytes().first() {
            Some(b'+') => (true, false, &rest[1..]),
            Some(b'-') => (false, true, &rest[1..]),
            _ => (false, false, rest),
        };
        let (from, to) = (self.id(from)?, self.id(to)?);
        self.s.events.push(Event::Message(Message { from, to, text: clean(text), dotted, tip, both, activate, deactivate }));
        Some(())
    }
}

fn parse_box_header(rest: &str) -> (Option<Color>, String) {
    let rest = rest.trim();
    if rest.is_empty() {
        return (None, String::new());
    }
    if let Some(after_paren) = rest.strip_prefix("rgb(").or_else(|| rest.strip_prefix("rgba("))
        && let Some(end) = after_paren.find(')')
    {
        let col_len = rest.len() - after_paren.len() + end + 1;
        return (parse_color_spec(&rest[..col_len]), clean_quoted(&rest[col_len..]));
    }
    let (first, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if first.eq_ignore_ascii_case("transparent") {
        return (None, clean_quoted(tail));
    }
    if let Some(c) = parse_color_spec(first) {
        return (Some(c), clean_quoted(tail));
    }
    (None, clean_quoted(rest))
}

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
    let boxes: Vec<[f32; 2]> = s
        .participants
        .iter()
        .zip(&heads)
        .map(|(p, m)| if p.actor { [m.size[0].max(40.0), 40.0 + m.size[1]] } else { [(m.size[0] + 24.0).max(72.0), m.size[1] + 16.0] })
        .collect();
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
    let mut count = 0;
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
                count += 1;
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
                    if s.autonumber {
                        front.shape(ellipse([sx, line_y], 8.0, 8.0), paint.line, paint.line, 1.0);
                        front.label(&measure(&count.to_string(), num_font), [sx, line_y], num_font, paint.surface, Align::Center);
                    }
                    let (lo, hi) = (cx[m.from].min(cx[m.to]) - BAR, cx[m.from].max(cx[m.to]) + BAR);
                    touch(&mut frames, lo.min(mid - t.size[0] / 2.0), hi.max(mid + t.size[0] / 2.0));
                    y = line_y + 16.0;
                }
                if m.deactivate {
                    close_bar(&mut bars, &mut active, m.from, line_y);
                }
            }
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

    for (i, p) in s.participants.iter().enumerate() {
        let [w, h] = boxes[i];
        lifelines.stroke(vec![[cx[i], top_h], [cx[i], end]], false, 1.0, paint.node_border);
        for top in [top_h - h, end] {
            if p.actor {
                actor(&mut tops, [cx[i], top], paint);
                tops.label(&heads[i], [cx[i], top + 40.0 + heads[i].size[1] / 2.0], font, paint.text, Align::Center);
            } else {
                tops.shape(rounded_rect(cx[i] - w / 2.0, top, cx[i] + w / 2.0, top + h, 4.0), paint.node_fill, paint.node_border, 1.0);
                tops.label(&heads[i], [cx[i], top + h / 2.0], font, paint.text, Align::Center);
            }
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

fn actor(c: &mut Canvas, top: [f32; 2], paint: &DiagramStyle) {
    let [x, y] = top;
    c.shape(ellipse([x, y + 7.0], 7.0, 7.0), paint.node_fill, paint.line, 1.25);
    c.stroke(vec![[x, y + 14.0], [x, y + 26.0]], false, 1.25, paint.line);
    c.stroke(vec![[x - 10.0, y + 18.0], [x + 10.0, y + 18.0]], false, 1.25, paint.line);
    c.stroke(vec![[x - 8.0, y + 36.0], [x, y + 26.0], [x + 8.0, y + 36.0]], false, 1.25, paint.line);
}

#[cfg(test)]
#[path = "sequence_tests.rs"]
mod tests;
