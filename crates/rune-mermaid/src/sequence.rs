//! Sequence diagrams: the parser for `sequenceDiagram` sources and their drawing.

use std::collections::HashMap;

use rune::Color;

pub(crate) use super::sequence_draw::draw;
use super::{MAX_DEPTH, MAX_ITEMS, clean, clean_quoted, parse_color_spec};

#[derive(Debug)]
pub(crate) struct Participant {
    #[cfg_attr(not(test), allow(dead_code, reason = "tests check parsing by id"))]
    pub id: String,
    pub label: String,
    pub kind: Kind,
    pub group: Option<usize>,
}

/// How a participant's head draws: `actor`, or a v11 `participant A@{ "type": "…" }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Box,
    Actor,
    Boundary,
    Control,
    Entity,
    Database,
    Collections,
    Queue,
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
    /// `create`: the participant's head draws here instead of at the top.
    Create(usize),
    /// `destroy`: the participant's lifeline ends after its next message.
    Destroy(usize),
    /// `autonumber [start [step]]` from here on, or `autonumber off` (`None`).
    Number(Option<(u32, u32)>),
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
    // A line that doesn't read is skipped; one past the bounds leaves the whole diagram as source.
    for line in body {
        let (people, events) = (p.s.participants.len(), p.s.events.len());
        if p.line(line).is_none() {
            // Whatever the line added before it failed goes too.
            p.s.participants.truncate(people);
            p.s.events.truncate(events);
            p.ids.retain(|_, &mut i| i < people);
        }
        if p.s.participants.len() > 40 || p.s.events.len() > MAX_ITEMS || p.depth > MAX_DEPTH {
            return None;
        }
    }
    // Frames still open at the end close there.
    p.s.events.extend((0..p.depth).map(|_| Event::End));
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
            "participant" | "actor" => {
                self.declare(rest, word == "actor")?;
            }
            "create" => {
                let (kind, name) = rest.split_once(char::is_whitespace).map_or((rest, ""), |(w, r)| (w, r.trim()));
                let i = match kind {
                    "participant" | "actor" => self.declare(name, kind == "actor")?,
                    _ => return None,
                };
                self.s.events.push(Event::Create(i));
            }
            "destroy" => {
                let i = self.id(rest)?;
                self.s.events.push(Event::Destroy(i));
            }
            "title" => {
                let t = clean_quoted(l["title".len()..].trim().trim_start_matches(':'));
                if !t.is_empty() {
                    self.s.title = Some(t);
                }
            }
            "accTitle" | "accDescr" | "link" | "links" | "properties" | "details" => {}
            "autonumber" => {
                let mut nums = rest.split_whitespace().map(str::parse::<u32>);
                let on = match (rest, nums.next(), nums.next()) {
                    ("off", ..) => None,
                    (_, None, _) => Some((1, 1)),
                    (_, Some(Ok(start)), None) => Some((start, 1)),
                    (_, Some(Ok(start)), Some(Ok(step))) => Some((start, step)),
                    _ => return None,
                };
                self.s.autonumber |= on.is_some();
                self.s.events.push(Event::Number(on));
            }
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

    fn declare(&mut self, rest: &str, actor: bool) -> Option<usize> {
        let (rest, meta) = rest.split_once("@{").map_or((rest, ""), |(id, meta)| (id, meta));
        let kind = if actor { Kind::Actor } else { meta_kind(meta) };
        let (id, label) = match rest.split_once(" as ") {
            Some((id, label)) => (id.trim(), clean_quoted(label)),
            None => (rest.trim(), clean_quoted(rest)),
        };
        let i = self.id(id)?;
        let p = &mut self.s.participants[i];
        p.label = label;
        p.kind = kind;
        if p.group.is_none() {
            p.group = self.active_box;
        }
        Some(i)
    }

    fn id(&mut self, name: &str) -> Option<usize> {
        let name = name.trim().trim_matches('"');
        if name.is_empty() || name.contains([':', ',']) {
            return None;
        }
        if let Some(&i) = self.ids.get(name) {
            return Some(i);
        }
        self.s.participants.push(Participant { id: name.to_owned(), label: clean(name), kind: Kind::Box, group: self.active_box });
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

/// The kind a v11 `@{ "type": "database" }` names; anything else is a plain box.
fn meta_kind(meta: &str) -> Kind {
    let Some((_, after)) = meta.split_once("type") else { return Kind::Box };
    let name = after.trim_start_matches(|c: char| c == '"' || c == ':' || c.is_whitespace());
    let name: String = name.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    match name.as_str() {
        "actor" => Kind::Actor,
        "boundary" => Kind::Boundary,
        "control" => Kind::Control,
        "entity" => Kind::Entity,
        "database" => Kind::Database,
        "collections" => Kind::Collections,
        "queue" => Kind::Queue,
        _ => Kind::Box,
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

#[cfg(test)]
#[path = "sequence_tests.rs"]
mod tests;
