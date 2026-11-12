//! Drives every short sequence of selectable-text input through the runtime and checks the
//! selection after every letter against a literal transcription of the code before
//! `selectable::phase` replaced it: one `Held` struct with `path: Option<String>`, `content`,
//! `anchor`, `focus`, `far: Option<(String, String)>`, the `between` hash, `unit`, `origin`, a
//! `dragging: bool`, the run of presses, `shift` and the Select All pick.
//!
//! The reference sees only the letters and the document they edit. It keeps its own fields, its
//! own run of presses and its own copy of the document, and knows the window's few facts as
//! constants: which text each spot is on, the byte a press there lands on (a line's start or
//! end), and the words of these texts. `between` is kept as the list it hashed, which compares
//! the same way. The window has no virtual list, so the virtual ends stay empty on both sides,
//! and both compare that.
//!
//! Two alphabets: one for the orders a press, drag, release, lost release, Shift and the double
//! click interval come in, one for what happens to a selection while it's held or placed (the
//! text changing or remounting under it, focus moving, Select All, the Copy menu's word, a
//! handler clearing it).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::runtime::selectable::phase::Phase;
use crate::runtime::selectable::Unit;
use crate::{Key, KeyEvent, Mods, PhysicalInput, TextStyle, text};

/// Sequences of up to this many letters.
const LENGTH: usize = 5;
/// The double-click interval both sides run with.
const INTERVAL: f64 = 0.3;
/// `CLICK_SLOP`, as the reference transcribes it.
const SLOP: f32 = 4.0;
/// Each text's row height, and the x of a line's start and of a point past its end.
const ROW: f32 = 24.0;
const LEFT: f32 = 2.0;
const RIGHT: f32 = 296.0;
/// `SECONDARY_SELECTS_WORD`, transcribed.
const MENU_SELECTS_WORD: bool = cfg!(target_os = "macos");

/// The window's selectable texts, top to bottom: A, M and B in a selection group, C on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum T { A, M, B, C }

/// Where a pointer letter goes: the start of a text's line, a point past its end, or the empty
/// space under everything.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot { Start(T), End(T), Blank }

fn row(t: T) -> f32 {
    match t { T::A => 0.0, T::M => 1.0, T::B => 2.0, T::C => 3.0 }
}

fn point(spot: Spot) -> (f32, f32) {
    match spot {
        Spot::Start(t) => (LEFT, row(t) * ROW + ROW / 2.0),
        Spot::End(t) => (RIGHT, row(t) * ROW + ROW / 2.0),
        Spot::Blank => (LEFT, 150.0),
    }
}

#[derive(Clone, Copy, Debug)]
enum Letter {
    Press(Spot),
    Drag(Spot),
    Up,
    Lost,
    Shift,
    Tick,
    Menu,
    SelectAll,
    Focus,
    Append,
    Rewrite,
    EditM,
    Remount,
}

const POINTER: [Letter; 12] = [
    Letter::Press(Spot::Start(T::A)), Letter::Press(Spot::End(T::A)), Letter::Press(Spot::End(T::B)),
    Letter::Press(Spot::Start(T::C)), Letter::Press(Spot::Blank), Letter::Drag(Spot::Start(T::A)),
    Letter::Drag(Spot::End(T::B)), Letter::Drag(Spot::End(T::C)), Letter::Up, Letter::Lost, Letter::Shift, Letter::Tick,
];

/// `Tx::clear_text_selection` clears through the same call focus moving away does, so a
/// handler's clear plays only in the walk over virtual rows (`selection_phase_virtual`).
const HELD: [Letter; 12] = [
    Letter::Press(Spot::Start(T::A)), Letter::Press(Spot::End(T::B)), Letter::Press(Spot::Start(T::C)),
    Letter::Drag(Spot::End(T::B)), Letter::Up, Letter::Menu, Letter::SelectAll, Letter::Focus,
    Letter::Append, Letter::Rewrite, Letter::EditM, Letter::Remount,
];

/// The app's state: what the letters that edit change.
#[derive(Clone, Default, PartialEq)]
struct Doc {
    /// A spacer goes before the group, so the group's unkeyed index (and every path in it) moves.
    remounted: bool,
    /// Words streamed onto A's end.
    appended: u8,
    /// A's first letter is capitalised: a change inside what's selected.
    rewritten: bool,
    /// M, between A and B, is rewritten.
    m_edited: bool,
}

impl Doc {
    fn content(&self, t: T) -> String {
        match t {
            T::A => {
                let mut a = if self.rewritten { "Alpha beta".to_owned() } else { "alpha beta".to_owned() };
                for _ in 0..self.appended { a.push_str(" x"); }
                a
            }
            T::M => if self.m_edited { "MIDDLE".to_owned() } else { "middle".to_owned() },
            T::B => "gamma".to_owned(),
            T::C => "delta".to_owned(),
        }
    }

    fn group_path(&self) -> &'static str {
        if self.remounted { "root/#1" } else { "root/#0" }
    }

    fn path(&self, t: T) -> String {
        match t {
            T::A => format!("{}/a", self.group_path()),
            T::M => format!("{}/m", self.group_path()),
            T::B => format!("{}/b", self.group_path()),
            T::C => "root/c".to_owned(),
        }
    }

    /// The live selectable text at `path`.
    fn find(&self, path: &str) -> Option<T> {
        [T::A, T::M, T::B, T::C].into_iter().find(|&t| self.path(t) == path)
    }
}

fn view(doc: &Doc, _: &mut Cx<Doc>) -> El {
    let line = |key: &'static str, content: String| {
        text(content).key(key).font(TextStyle::new(14.0, 400.0)).w_full().h(ROW).selectable(true)
    };
    let mut root = col().key("root").w(300.0);
    if doc.remounted { root = root.child(el().w(300.0).h(0.0)); }
    root.child(col().w_full().selection_group(true)
            .child(line("a", doc.content(T::A)))
            .child(line("m", doc.content(T::M)))
            .child(line("b", doc.content(T::B))))
        .child(line("c", doc.content(T::C)))
        .child(el().key("button").w(100.0).h(20.0).focusable(true))
        .child(el().key("blank").w_full().h(80.0))
}

/// What a press on `t` at `spot` lands on: a line's start, or its end.
fn hit(doc: &Doc, t: T, spot: Spot) -> usize {
    match spot {
        Spot::Start(_) | Spot::Blank => 0,
        Spot::End(_) => doc.content(t).len(),
    }
}

/// Which way the selection's drag reaches: `selectable::Unit`, as the reference keeps it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum U { Char, Word, Paragraph }

/// `unit_at` on these texts: single lines of ASCII words split by single spaces.
fn unit_at(text: &str, i: usize, unit: U) -> (usize, usize) {
    let mut i = i.min(text.len());
    if i > 0 && i == text.len() && unit != U::Char { i -= 1; }
    match unit {
        U::Char => (i, i),
        U::Paragraph => (0, text.len()),
        U::Word => {
            let b = text.as_bytes();
            let space = |k: usize| b[k] == b' ';
            if text.is_empty() { return (0, 0); }
            let (mut a, mut e) = (i, i + 1);
            while a > 0 && space(a - 1) == space(i) { a -= 1; }
            while e < b.len() && space(e) == space(i) { e += 1; }
            (a, e)
        }
    }
}

/// `extend`, transcribed.
fn extend(origin: (usize, usize), at: (usize, usize)) -> (usize, usize) {
    if at.0 < origin.0 { (origin.1, at.0) } else { (origin.0, at.1.max(origin.1)) }
}

/// `kept`, transcribed.
fn kept(then: &str, now: &str, upto: usize) -> bool {
    if upto == 0 { return now.starts_with(then); }
    then.get(..upto).is_some_and(|a| now.get(..upto) == Some(a))
}

/// `same_keyed_path`, transcribed.
fn same_keyed_path(a: &str, b: &str) -> bool {
    fn split(p: &str) -> Option<(&str, &str)> {
        let last_key = p.rmatch_indices('/').find(|&(i, _)| !p[i + 1..].starts_with('#'))
            .map_or_else(|| (!p.starts_with('#')).then_some(0), |(i, _)| Some(i + 1))?;
        let end = last_key + p[last_key..].find('/').unwrap_or(p.len() - last_key);
        Some((&p[..end], &p[end..]))
    }
    let (Some((ka, ta)), Some((kb, tb))) = (split(a), split(b)) else { return false };
    ta == tb && ka.split('/').filter(|s| !s.starts_with('#')).eq(kb.split('/').filter(|s| !s.starts_with('#')))
}

fn grouped(t: T) -> bool { t != T::C }

fn order(t: T) -> f32 { row(t) }

/// `end_same_group`: both in the one group.
fn same_group(a: T, b: T) -> bool { grouped(a) && grouped(b) }

/// `press_run::PressRun`'s last press.
struct Run {
    path: String,
    at: f64,
    point: (f32, f32),
    count: u32,
}

/// What Select All's pick names in the window now.
enum Picked { Group, Text(T) }

/// `selectable::Held` and the runtime's selection code before `selectable::phase`, field for
/// field.
struct Dev {
    path: Option<String>,
    content: String,
    anchor: usize,
    focus: usize,
    far: Option<(String, String)>,
    /// The texts the `between` hash was taken over.
    between: Vec<(String, String)>,
    unit: U,
    origin: (usize, usize),
    dragging: bool,
    presses: Option<Run>,
    shift: bool,
    pick: Option<String>,
}

impl Dev {
    fn new() -> Self {
        Self { path: None, content: String::new(), anchor: 0, focus: 0, far: None, between: Vec::new(), unit: U::Char,
            origin: (0, 0), dragging: false, presses: None, shift: false, pick: None }
    }

    fn press_count(&mut self, path: &str, now: f64, point: (f32, f32)) -> u32 {
        let follows = |then: f64, was: (f32, f32)| {
            now - then <= INTERVAL && (point.0 - was.0).abs() <= SLOP && (point.1 - was.1).abs() <= SLOP
        };
        match &mut self.presses {
            Some(last) if last.path == path && follows(last.at, last.point) => {
                (last.at, last.point) = (now, point);
                last.count += 1;
                last.count
            }
            _ => {
                self.presses = Some(Run { path: path.to_owned(), at: now, point, count: 1 });
                1
            }
        }
    }

    /// `selection_holder_end`.
    fn holder(&self, doc: &Doc) -> Option<T> {
        let path = self.path.as_deref()?;
        let h = doc.find(path)?;
        let upto = match &self.far {
            None => self.anchor.max(self.focus).max(self.origin.1),
            Some((far, _)) if doc.find(far).is_some_and(|f| order(f) > order(h)) => self.content.len(),
            Some(_) => self.anchor.max(self.origin.1),
        };
        kept(&self.content, &doc.content(h), upto).then_some(h)
    }

    /// `focus_holder_end`.
    fn focus_holder(&self, doc: &Doc, holder: T) -> Option<T> {
        let Some((path, content)) = &self.far else { return Some(holder) };
        let f = doc.find(path)?;
        let upto = if order(f) > order(holder) { self.focus } else { content.len() };
        (same_group(f, holder) && kept(content, &doc.content(f), upto)).then_some(f)
    }

    fn ends(&self, doc: &Doc) -> Option<(T, T)> {
        let h = self.holder(doc)?;
        Some((h, self.focus_holder(doc, h)?))
    }

    /// `selection_ranges`.
    fn ranges(&self, doc: &Doc) -> Vec<(T, usize, usize)> {
        let Some((h, f)) = self.ends(doc) else { return Vec::new() };
        if f == h {
            let (a, b) = (self.anchor.min(self.focus), self.anchor.max(self.focus));
            return if a < b && b <= doc.content(h).len() { vec![(h, a, b)] } else { Vec::new() };
        }
        let ((first, a), (last, b)) = if order(h) < order(f) { ((h, self.anchor), (f, self.focus)) } else { ((f, self.focus), (h, self.anchor)) };
        [T::A, T::M, T::B].into_iter().filter(|&j| order(j) >= order(first) && order(j) <= order(last)).filter_map(|j| {
            let len = doc.content(j).len();
            let (a, b) = (if j == first { a.min(len) } else { 0 }, if j == last { b.min(len) } else { len });
            (a < b).then_some((j, a, b))
        }).collect()
    }

    /// `Runtime::text_selection`.
    fn text_selection(&self, doc: &Doc) -> Option<(String, std::ops::Range<usize>)> {
        let h = self.holder(doc)?;
        let range = match &self.far {
            None => self.anchor.min(self.focus)..self.anchor.max(self.focus),
            Some(_) => {
                let f = self.focus_holder(doc, h)?;
                if f == h {
                    self.anchor.min(self.focus)..self.anchor.max(self.focus)
                } else {
                    let len = doc.content(h).len();
                    let (a, b) = if order(h) < order(f) { (self.anchor.min(len), len) } else { (0, self.anchor.min(len)) };
                    if a < b { a..b } else { self.anchor..self.anchor }
                }
            }
        };
        Some((self.path.clone()?, range))
    }

    /// `between_hash_ends`, as the texts it hashes.
    fn between_texts(&self, doc: &Doc, h: T, f: T) -> Vec<(String, String)> {
        if h == f || !grouped(h) { return Vec::new(); }
        let (first, last) = if order(h) < order(f) { (h, f) } else { (f, h) };
        [T::A, T::M, T::B].into_iter().filter(|&j| order(j) > order(first) && order(j) < order(last))
            .map(|j| (doc.path(j), doc.content(j))).collect()
    }

    /// `seal_between`, with `sync_selection_pins` a no-op without virtual lists, and
    /// `remember_pick`.
    fn seal(&mut self, doc: &Doc) {
        self.between = match self.ends(doc) {
            None => Vec::new(),
            Some((h, f)) => self.between_texts(doc, h, f),
        };
        if let Some(h) = self.holder(doc) {
            self.pick = Some(if grouped(h) { doc.group_path().to_owned() } else { doc.path(h) });
        }
    }

    /// `clear_text_selection`: `clear_held`, then the pins.
    fn clear(&mut self) {
        self.path = None;
        self.far = None;
        self.dragging = false;
    }

    /// `selection_down` from `press_selection`.
    fn down(&mut self, doc: &Doc, spot: Spot, now: f64) {
        let target = match spot { Spot::Start(t) | Spot::End(t) => Some(t), Spot::Blank => None };
        let Some(i) = target else {
            self.clear();
            self.presses = None;
            return;
        };
        let at = hit(doc, i, spot);
        let (path, content) = (doc.path(i), doc.content(i));
        let clicks = self.press_count(&path, now, point(spot)).min(3);
        let holder = self.holder(doc).filter(|&h| h == i || same_group(h, i));
        match holder {
            Some(h) if clicks == 1 && self.shift => {
                self.unit = U::Char;
                self.origin = (self.anchor, self.anchor);
                self.focus = at;
                self.far = (h != i).then_some((path, content));
            }
            _ => {
                let unit = match clicks { 1 => U::Char, 2 => U::Word, _ => U::Paragraph };
                let origin = unit_at(&content, at, unit);
                (self.unit, self.origin) = (unit, origin);
                (self.anchor, self.focus) = origin;
                self.path = Some(path);
                self.content = content;
                self.far = None;
            }
        }
        self.dragging = true;
        self.seal(doc);
    }

    /// `selection_drag`: in the group a spot below it reaches B's end; C, on its own, takes the
    /// byte its own line has under the spot's x.
    fn drag(&mut self, doc: &Doc, spot: Spot) {
        if !self.dragging { return; }
        let Some(h) = self.holder(doc) else { self.clear(); return };
        let (j, at) = if grouped(h) {
            match spot {
                Spot::Start(t) | Spot::End(t) if grouped(t) => (t, hit(doc, t, spot)),
                _ => (T::B, doc.content(T::B).len()),
            }
        } else {
            (h, hit(doc, h, spot))
        };
        let at = unit_at(&doc.content(j), at, self.unit);
        let (anchor, focus) = if j == h {
            extend(self.origin, at)
        } else if order(j) > order(h) {
            (self.origin.0, at.1)
        } else {
            (self.origin.1, at.0)
        };
        let same_far = if j == h { self.far.is_none() } else { self.far.as_ref().is_some_and(|(p, _)| *p == doc.path(j)) };
        if (self.anchor, self.focus) == (anchor, focus) && same_far { return; }
        self.anchor = anchor;
        self.focus = focus;
        if j == h {
            self.far = None;
        } else if !same_far {
            self.far = Some((doc.path(j), doc.content(j)));
        }
        if !same_far { self.seal(doc); }
    }

    /// `selection_menu` on A's start.
    fn menu(&mut self, doc: &Doc) {
        let at = 0;
        if MENU_SELECTS_WORD && self.ranges(doc).iter().find(|r| r.0 == T::A).is_none_or(|r| at < r.1 || at >= r.2) {
            let content = doc.content(T::A);
            let (a, b) = unit_at(&content, at, U::Word);
            *self = Dev { path: Some(doc.path(T::A)), content, anchor: a, focus: b, far: None, between: Vec::new(), unit: U::Word,
                origin: (a, b), dragging: false, presses: None, shift: self.shift, pick: self.pick.take() };
            self.seal(doc);
        }
    }

    /// `selection_key` for Cmd+A, then `select_all_key`.
    fn select_all(&mut self, doc: &Doc) {
        let holder = self.holder(doc);
        if holder.is_none() { self.clear(); }
        let target = match holder {
            Some(h) => Some(if grouped(h) { Picked::Group } else { Picked::Text(h) }),
            None => self.pick.as_deref().and_then(|p| {
                if p == doc.group_path() { Some(Picked::Group) } else { doc.find(p).map(Picked::Text) }
            }),
        };
        let (first, last) = match target {
            Some(Picked::Group) => (T::A, T::B),
            Some(Picked::Text(t)) => (t, t),
            None => return,
        };
        // `select_span_ends`.
        let content = doc.content(first);
        let far = (first != last).then(|| (doc.path(last), doc.content(last)));
        let end = far.as_ref().map_or(content.len(), |f| f.1.len());
        self.origin = if far.is_some() { (0, 0) } else { (0, end) };
        (self.path, self.content, self.anchor, self.focus, self.unit, self.far) = (Some(doc.path(first)), content, 0, end, U::Char, far);
        self.dragging = false;
        self.seal(doc);
    }

    /// `rebound`.
    fn rebound(doc: &Doc, path: &str, content: &str, upto: usize) -> Option<T> {
        let mut hits = [T::A, T::M, T::B, T::C].into_iter()
            .filter(|&t| same_keyed_path(path, &doc.path(t)) && kept(content, &doc.content(t), upto));
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    /// `prune_text_selection`, after a commit.
    fn prune(&mut self, doc: &Doc) {
        if self.path.is_none() { return; }
        let mut rebind = false;
        if self.holder(doc).is_none()
            && let Some(old) = self.path.clone()
            && let Some(i) = Self::rebound(doc, &old, &self.content, self.anchor.max(self.focus).max(self.origin.1))
        {
            self.path = Some(doc.path(i));
            rebind = true;
        }
        if let Some(h) = self.holder(doc)
            && self.focus_holder(doc, h).is_none()
            && let Some((far, content)) = self.far.clone()
            && let Some(j) = Self::rebound(doc, &far, &content, self.focus)
        {
            self.far = Some((doc.path(j), content));
            rebind = true;
        }
        if rebind && let Some((h, f)) = self.ends(doc) { self.between = self.between_texts(doc, h, f); }
        let Some((h, f)) = self.ends(doc) else { self.clear(); return };
        if self.far.is_some() && self.between_texts(doc, h, f) != self.between { self.clear(); return; }
        self.path = Some(doc.path(h));
        self.content = doc.content(h);
        if f != h && let Some(far) = &mut self.far { *far = (doc.path(f), doc.content(f)); }
    }

    /// `set_focus` to another element by keyboard.
    fn focus_away(&mut self, doc: &Doc) {
        if self.text_selection(doc).is_some() { self.clear(); }
    }
}

/// The runtime with the four texts, and the reference.
struct Walk {
    rt: Runtime<Doc>,
    dev: Dev,
    /// The reference's copy of the document.
    doc: Doc,
    /// The virtual clock both sides read.
    now: f64,
    /// Where the pointer last went, which a release happens at.
    at: Spot,
}

impl Walk {
    fn new() -> Self {
        let mut rt = Runtime::new(Doc::default(), view, (320.0, 240.0), Env::new(true, 1.0));
        rt.double_click_interval = INTERVAL;
        rt.frame(0.0);
        Self { rt, dev: Dev::new(), doc: Doc::default(), now: 0.0, at: Spot::Blank }
    }

    /// Whatever the last sequence left goes, without stepping: the next starts with nothing
    /// selected on the unedited document.
    fn reset(&mut self) {
        self.rt.cancel_pointer();
        self.rt.selectable = Default::default();
        self.rt.held_mods = Mods::default();
        self.rt.queue.retain(|_| false);
        self.rt.clock = 0.0;
        self.now = 0.0;
        if self.rt.state != Doc::default() {
            let remounted = self.rt.state.remounted;
            self.rt.update(|doc, _| *doc = Doc::default());
            if remounted { self.settle(); } else { self.rt.frame(0.0); }
            self.rt.clock = 0.0;
        }
        self.dev = Dev::new();
        self.doc = Doc::default();
        self.at = Spot::Blank;
    }

    /// Runs frames until the window is still, as a host would after a commit: a remounted
    /// text takes presses once its entrance is drawn. The clock goes back to the walk's.
    fn settle(&mut self) {
        for _ in 0..8 {
            if !self.rt.frame(1.0) { break; }
        }
        self.rt.clock = self.now;
    }

    /// An edit and its commit. Only a remount needs the window to settle; a text that changes
    /// in place takes presses on the next frame.
    fn edit(&mut self, f: fn(&mut Doc)) {
        let remounted = self.doc.remounted;
        f(&mut self.doc);
        self.rt.update(|doc, _| f(doc));
        if self.doc.remounted != remounted { self.settle(); } else { self.rt.frame(0.0); }
        self.rt.clock = self.now;
        self.dev.prune(&self.doc);
    }

    /// Plays one letter on the runtime and the reference, and compares what each left behind.
    fn play(&mut self, letter: Letter) -> Result<(), String> {
        match letter {
            Letter::Press(spot) => {
                let (x, y) = point(spot);
                self.rt.pointer_down(x, y);
                self.dev.down(&self.doc, spot, self.now);
                self.at = spot;
            }
            Letter::Drag(spot) => {
                let (x, y) = point(spot);
                self.rt.pointer_move(x, y);
                self.dev.drag(&self.doc, spot);
                self.at = spot;
            }
            Letter::Up => {
                let (x, y) = point(self.at);
                self.rt.pointer_up(x, y);
                // The release moves the pointer to where it is, which drags there again.
                self.dev.drag(&self.doc, self.at);
                self.dev.dragging = false;
            }
            Letter::Lost => {
                self.rt.cancel_pointer();
                self.dev.dragging = false;
            }
            Letter::Shift => {
                let shift = !self.dev.shift;
                self.rt.physical_input(PhysicalInput::Modifiers(Mods { shift, ..Mods::default() }));
                self.dev.shift = shift;
            }
            Letter::Tick => {
                self.now += 1.0;
                self.rt.clock = self.now;
            }
            Letter::Menu => {
                let (x, y) = point(Spot::Start(T::A));
                self.rt.secondary_down(x, y);
                self.rt.secondary_up(x, y);
                self.rt.popups = Default::default();
                self.dev.menu(&self.doc);
            }
            Letter::SelectAll => {
                // The key waits in the queue for the frame.
                self.rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
                self.rt.frame(0.0);
                self.rt.clock = self.now;
                self.dev.select_all(&self.doc);
            }
            Letter::Focus => {
                self.rt.set_focus(Some("root/button".to_owned()), true);
                self.rt.set_focus(None, false);
                self.dev.focus_away(&self.doc);
            }
            Letter::Append => self.edit(|doc| doc.appended += 1),
            Letter::Rewrite => self.edit(|doc| doc.rewritten = !doc.rewritten),
            Letter::EditM => self.edit(|doc| doc.m_edited = !doc.m_edited),
            Letter::Remount => self.edit(|doc| doc.remounted = !doc.remounted),
        }
        self.compare(letter)
    }

    fn compare(&self, letter: Letter) -> Result<(), String> {
        let dev = &self.dev;
        let ours = &self.rt.selectable;
        if ours.shift != dev.shift { return Err(format!("after {letter:?}: shift {}, dev {}", ours.shift, dev.shift)); }
        match (&ours.phase, &dev.path) {
            (Phase::Empty, None) if !dev.dragging => {}
            (Phase::Placed(sel) | Phase::Dragging { selection: sel, .. }, Some(path)) => {
                let unit = match &ours.phase {
                    Phase::Dragging { unit, .. } => Some(match unit { Unit::Char => U::Char, Unit::Word => U::Word, Unit::Paragraph => U::Paragraph }),
                    _ => None,
                };
                let far = sel.far.as_ref().map(|f| (f.path.clone(), f.content.clone()));
                let ours = (&sel.path, &sel.content, sel.anchor, sel.focus, sel.origin, &far, unit);
                let theirs = (path, &dev.content, dev.anchor, dev.focus, dev.origin, &dev.far, dev.dragging.then_some(dev.unit));
                if ours != theirs { return Err(format!("after {letter:?}: {ours:?}, dev {theirs:?}")); }
                if sel.anchor_end.is_some() || sel.far.as_ref().is_some_and(|f| f.end.is_some()) || !self.rt.selectable.mounted_between.is_empty() {
                    return Err(format!("after {letter:?}: virtual ends outside a virtual list"));
                }
            }
            (phase, _) => {
                let phase = match phase { Phase::Empty => "empty", Phase::Placed(_) => "placed", Phase::Dragging { .. } => "dragging" };
                return Err(format!("after {letter:?}: {phase}, dev path {:?} dragging {}", dev.path, dev.dragging));
            }
        }
        let ours = self.rt.text_selection().map(|(p, r)| (p.to_owned(), r));
        let theirs = dev.text_selection(&self.doc);
        if ours != theirs { return Err(format!("after {letter:?}: text_selection {ours:?}, dev {theirs:?}")); }
        Ok(())
    }
}

/// Plays every sequence that starts with `sequence`, recording the first one that differs.
fn visit(walk: &mut Walk, alphabet: &[Letter], sequence: &mut Vec<Letter>, ran: &mut usize, failure: &mut Option<String>) {
    if failure.is_some() { return; }
    *ran += 1;
    walk.reset();
    for &letter in sequence.iter() {
        if let Err(why) = walk.play(letter) {
            *failure = Some(format!("{sequence:?}: {why}"));
            return;
        }
    }
    if sequence.len() == LENGTH { return; }
    for &letter in alphabet {
        sequence.push(letter);
        visit(walk, alphabet, sequence, ran, failure);
        sequence.pop();
    }
}

fn walk_all(alphabet: &[Letter]) {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut walk = Walk::new();
                let (mut ran, mut failure) = (0, None);
                while let Some(&letter) = alphabet.get(next.fetch_add(1, Ordering::Relaxed)) {
                    visit(&mut walk, alphabet, &mut vec![letter], &mut ran, &mut failure);
                }
                results.lock().unwrap().push((ran, failure));
            });
        }
    });
    let mut total = 0;
    let mut failures = Vec::new();
    for (ran, failure) in results.into_inner().unwrap() {
        total += ran;
        failures.extend(failure);
    }
    println!("{total} sequences up to length {LENGTH}");
    assert!(failures.is_empty(), "sequences differ from dev: {failures:#?}");
    let expected: usize = (1..=LENGTH as u32).map(|n| alphabet.len().pow(n)).sum();
    assert_eq!(total, expected, "every sequence ran");
}

#[test]
fn every_short_sequence_of_presses_drags_and_releases_leaves_what_dev_left() {
    walk_all(&POINTER);
}

#[test]
fn every_short_sequence_of_edits_under_a_selection_leaves_what_dev_left() {
    walk_all(&HELD);
}

/// The facts the reference takes as constants: where a press on each spot lands, and what a
/// drag from C (on its own) reaches at each spot.
#[test]
fn the_reference_knows_where_each_spot_lands() {
    let mut walk = Walk::new();
    for t in [T::A, T::M, T::B, T::C] {
        for spot in [Spot::Start(t), Spot::End(t)] {
            walk.reset();
            let (x, y) = point(spot);
            walk.rt.pointer_down(x, y);
            let at = hit(&walk.doc, t, spot);
            assert_eq!(walk.rt.text_selection(), Some((walk.doc.path(t).as_str(), at..at)), "{spot:?}");
        }
    }
    for spot in [Spot::Start(T::A), Spot::End(T::A), Spot::End(T::B), Spot::Blank] {
        walk.reset();
        let (x, y) = point(Spot::End(T::C));
        walk.rt.pointer_down(x, y);
        let (x, y) = point(spot);
        walk.rt.pointer_move(x, y);
        let len = walk.doc.content(T::C).len();
        let at = hit(&walk.doc, T::C, spot);
        assert_eq!(walk.rt.text_selection(), Some(("root/c", at.min(len)..len)), "drag from C to {spot:?}");
    }
    walk.reset();
    walk.rt.update(|doc, _| doc.remounted = true);
    walk.rt.frame(0.0);
    for t in [T::A, T::M, T::B, T::C] {
        let doc = Doc { remounted: true, ..Doc::default() };
        assert!(walk.rt.nodes.iter().any(|n| n.path == doc.path(t)), "{t:?} at {}", doc.path(t));
    }
    assert!(walk.rt.nodes.iter().any(|n| n.path == "root/#1" && n.selection_group));
}

