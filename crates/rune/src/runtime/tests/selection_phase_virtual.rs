//! `selection_phase`'s walk over the rows of a virtual list: a selection whose anchor row
//! scrolls out of the mounted window and back, a drag while the list scrolls, Select All over
//! rows never built, a handler clearing it. The reference is the same transcription of the code
//! before `selectable::phase`, on a list of forty rows that never change, so every text the
//! selection names stays resolvable (its rows are pinned) and only the pointer's row depends on
//! the scroll. Both sides' virtual ends are compared after every letter: the anchor's row and
//! offset, the focus's when it's in another row, and that the mounted rows hashed between them
//! lie strictly between them.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::runtime::selectable::Unit;
use crate::runtime::selectable::phase::Phase;
use crate::{GrowthSpec, Key, KeyEvent, Mods, ScrollSpec, TextStyle, text};

/// Sequences of up to this many letters.
const LENGTH: usize = 5;
const ROWS: usize = 40;
const ROW_H: f32 = 40.0;
const LIST: &str = "root/scroll/list";
/// What each row reads: every row the same length.
const LEN: usize = 6;
/// `CLICK_SLOP`, as the reference transcribes it.
const SLOP: f32 = 4.0;

fn content(row: usize) -> String { format!("row {row:02}") }

fn path(row: usize) -> String { format!("{LIST}/r{row}") }

fn row_of(path: &str) -> usize { path.rsplit_once("/r").and_then(|(_, r)| r.parse().ok()).expect("a row path") }

/// The top line of the viewport at its start, or its bottom line past the end.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot { HiStart, LoEnd }

fn point(spot: Spot) -> (f32, f32) {
    match spot { Spot::HiStart => (2.0, 20.0), Spot::LoEnd => (296.0, 100.0) }
}

/// The row under `spot`, with the list scrolled to its top or its end, and the byte a press
/// there lands on.
fn under(spot: Spot, top: bool) -> (usize, usize) {
    match (spot, top) {
        (Spot::HiStart, true) => (0, 0),
        (Spot::LoEnd, true) => (2, LEN),
        (Spot::HiStart, false) => (ROWS - 3, 0),
        (Spot::LoEnd, false) => (ROWS - 1, LEN),
    }
}

#[derive(Clone, Copy, Debug)]
enum Letter { Press(Spot), Drag(Spot), Up, Lost, Down, Top, SelectAll, Clear }

const ALPHABET: [Letter; 10] = [
    Letter::Press(Spot::HiStart), Letter::Press(Spot::LoEnd), Letter::Drag(Spot::HiStart), Letter::Drag(Spot::LoEnd),
    Letter::Up, Letter::Lost, Letter::Down, Letter::Top, Letter::SelectAll, Letter::Clear,
];

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().key("root").w(300.0).h(120.0).child(
        col().key("scroll").w_full().h(120.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("list").w_full().growth(GrowthSpec::default()).selection_group(true).virtual_rows(
                ROWS,
                ROW_H,
                |_: &(), i| format!("r{i}").into(),
                |_: &(), i| text(content(i)).font(TextStyle::new(14.0, 400.0)).w_full().h(ROW_H).selectable(true),
            ),
        ),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum U { Char, Word, Paragraph }

/// `unit_at` on "row NN".
fn unit_at(i: usize, unit: U) -> (usize, usize) {
    let i = if i == LEN && unit != U::Char { i - 1 } else { i };
    match unit {
        U::Char => (i, i),
        U::Paragraph => (0, LEN),
        U::Word => match i { 0..=2 => (0, 3), 3 => (3, 4), _ => (4, LEN) },
    }
}

/// `extend`, transcribed.
fn extend(origin: (usize, usize), at: (usize, usize)) -> (usize, usize) {
    if at.0 < origin.0 { (origin.1, at.0) } else { (origin.0, at.1.max(origin.1)) }
}

struct Run {
    path: String,
    point: (f32, f32),
    count: u32,
}

/// `selectable::Held` before `selectable::phase`, on rows that never change: the anchor's and
/// the focus's texts always resolve, and resolve to the rows their paths name.
struct Dev {
    path: Option<String>,
    content: String,
    anchor: usize,
    focus: usize,
    far: Option<(String, String)>,
    unit: U,
    origin: (usize, usize),
    dragging: bool,
    presses: Option<Run>,
}

impl Dev {
    fn new() -> Self {
        Self { path: None, content: String::new(), anchor: 0, focus: 0, far: None, unit: U::Char, origin: (0, 0), dragging: false, presses: None }
    }

    /// `PressRun::press` with the clock standing still.
    fn press_count(&mut self, path: &str, point: (f32, f32)) -> u32 {
        match &mut self.presses {
            Some(last) if last.path == path && (point.0 - last.point.0).abs() <= SLOP && (point.1 - last.point.1).abs() <= SLOP => {
                last.point = point;
                last.count += 1;
                last.count
            }
            _ => {
                self.presses = Some(Run { path: path.to_owned(), point, count: 1 });
                1
            }
        }
    }

    fn clear(&mut self) {
        self.path = None;
        self.far = None;
        self.dragging = false;
    }

    /// `selection_down` on the row under `spot`: no Shift, so every press starts a selection.
    fn down(&mut self, spot: Spot, top: bool) {
        let (row, at) = under(spot, top);
        let clicks = self.press_count(&path(row), point(spot)).min(3);
        let unit = match clicks { 1 => U::Char, 2 => U::Word, _ => U::Paragraph };
        let origin = unit_at(at, unit);
        (self.unit, self.origin) = (unit, origin);
        (self.anchor, self.focus) = origin;
        self.path = Some(path(row));
        self.content = content(row);
        self.far = None;
        self.dragging = true;
    }

    /// `selection_drag`: the group reaches the row the pointer is over.
    fn drag(&mut self, spot: Spot, top: bool) {
        if !self.dragging { return; }
        let Some(h) = self.path.as_deref().map(row_of) else { self.clear(); return };
        let (j, at) = under(spot, top);
        let at = unit_at(at, self.unit);
        let (anchor, focus) = if j == h { extend(self.origin, at) } else if j > h { (self.origin.0, at.1) } else { (self.origin.1, at.0) };
        let same_far = if j == h { self.far.is_none() } else { self.far.as_ref().is_some_and(|(p, _)| *p == path(j)) };
        if (self.anchor, self.focus) == (anchor, focus) && same_far { return; }
        (self.anchor, self.focus) = (anchor, focus);
        if j == h {
            self.far = None;
        } else if !same_far {
            self.far = Some((path(j), content(j)));
        }
    }

    /// `selection_key`'s Cmd+A: the group is the list, so it selects every row.
    fn select_all(&mut self) {
        let far = (path(ROWS - 1), content(ROWS - 1));
        (self.path, self.content, self.anchor, self.focus, self.origin, self.unit) = (Some(path(0)), content(0), 0, LEN, (0, 0), U::Char);
        self.far = Some(far);
        self.dragging = false;
    }

    /// `Runtime::text_selection`.
    fn text_selection(&self) -> Option<(String, std::ops::Range<usize>)> {
        let path = self.path.clone()?;
        let h = row_of(&path);
        let range = match &self.far {
            None => self.anchor.min(self.focus)..self.anchor.max(self.focus),
            Some((far, _)) => {
                let f = row_of(far);
                if f == h {
                    self.anchor.min(self.focus)..self.anchor.max(self.focus)
                } else {
                    let (a, b) = if h < f { (self.anchor.min(LEN), LEN) } else { (0, self.anchor.min(LEN)) };
                    if a < b { a..b } else { self.anchor..self.anchor }
                }
            }
        };
        Some((path, range))
    }
}

struct Walk {
    rt: Runtime<()>,
    dev: Dev,
    /// The list is scrolled to its top (else to its end).
    top: bool,
    at: Spot,
}

impl Walk {
    fn new() -> Self {
        let mut rt = Runtime::new((), view, (300.0, 120.0), Env::new(true, 1.0));
        rt.double_click_interval = 0.3;
        let mut walk = Self { rt, dev: Dev::new(), top: true, at: Spot::HiStart };
        walk.settle();
        walk
    }

    fn settle(&mut self) {
        for _ in 0..60 {
            if !self.rt.frame(1.0 / 60.0) { break; }
        }
        self.rt.clock = 0.0;
    }

    fn scroll(&mut self, top: bool) {
        self.rt.wheel(150.0, 60.0, if top { 10_000.0 } else { -10_000.0 });
        self.settle();
        self.top = top;
    }

    fn reset(&mut self) {
        self.rt.cancel_pointer();
        self.rt.selectable = Default::default();
        self.rt.sync_selection_pins();
        self.rt.queue.retain(|_| false);
        if !self.top { self.scroll(true); }
        self.rt.clock = 0.0;
        self.dev = Dev::new();
        self.at = Spot::HiStart;
    }

    fn play(&mut self, letter: Letter) -> Result<(), String> {
        match letter {
            Letter::Press(spot) => {
                let (x, y) = point(spot);
                self.rt.pointer_down(x, y);
                self.dev.down(spot, self.top);
                self.at = spot;
            }
            Letter::Drag(spot) => {
                let (x, y) = point(spot);
                self.rt.pointer_move(x, y);
                self.dev.drag(spot, self.top);
                self.at = spot;
            }
            Letter::Up => {
                let (x, y) = point(self.at);
                self.rt.pointer_up(x, y);
                self.dev.drag(self.at, self.top);
                self.dev.dragging = false;
            }
            Letter::Lost => {
                self.rt.cancel_pointer();
                self.dev.dragging = false;
            }
            Letter::Down => self.scroll(false),
            Letter::Top => self.scroll(true),
            Letter::SelectAll => {
                self.rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
                self.settle();
                self.dev.select_all();
            }
            Letter::Clear => {
                self.rt.update(|_, tx| tx.clear_text_selection());
                self.settle();
                self.dev.clear();
            }
        }
        self.compare(letter)
    }

    fn compare(&self, letter: Letter) -> Result<(), String> {
        let dev = &self.dev;
        match (&self.rt.selectable.phase, &dev.path) {
            (Phase::Empty, None) if !dev.dragging && self.rt.selectable.mounted_between.is_empty() => {}
            (phase @ (Phase::Placed(sel) | Phase::Dragging { selection: sel, .. }), Some(path)) => {
                let unit = match phase {
                    Phase::Dragging { unit, .. } => Some(match unit { Unit::Char => U::Char, Unit::Word => U::Word, Unit::Paragraph => U::Paragraph }),
                    _ => None,
                };
                let far = sel.far.as_ref().map(|f| (f.path.clone(), f.content.clone()));
                let ours = (&sel.path, &sel.content, sel.anchor, sel.focus, sel.origin, &far, unit);
                let theirs = (path, &dev.content, dev.anchor, dev.focus, dev.origin, &dev.far, dev.dragging.then_some(dev.unit));
                if ours != theirs { return Err(format!("after {letter:?}: {ours:?}, dev {theirs:?}")); }
                // `sync_selection_pins` names the rows of both ends, at their offsets.
                let end = |e: &Option<crate::runtime::selectable::virtual_selection::VirtualEnd>| {
                    e.as_ref().map(|e| (e.list_path.clone(), e.row_key.clone(), e.row_index, e.rel_path.clone(), e.offset))
                };
                let row = |p: &str| { let r = row_of(p); (LIST.to_owned(), format!("r{r}"), r, String::new()) };
                let (r, k, i, rel) = row(path);
                if end(&sel.anchor_end) != Some((r, k, i, rel, dev.anchor)) {
                    return Err(format!("after {letter:?}: anchor end {:?}", sel.anchor_end));
                }
                if let (Some(far), Some((fp, _))) = (&sel.far, &dev.far) {
                    let (r, k, i, rel) = row(fp);
                    if end(&far.end) != Some((r, k, i, rel, dev.focus)) { return Err(format!("after {letter:?}: focus end {:?}", far.end)); }
                }
                let (h, f) = (row_of(path), dev.far.as_ref().map_or(row_of(path), |(p, _)| row_of(p)));
                let between: HashSet<String> = (h.min(f) + 1..h.max(f)).map(|r| format!("r{r}")).collect();
                if let Some(k) = self.rt.selectable.mounted_between.keys().find(|k| !between.contains(*k)) {
                    return Err(format!("after {letter:?}: row {k} hashed as between rows {h} and {f}"));
                }
            }
            (phase, _) => {
                let phase = match phase { Phase::Empty => "empty", Phase::Placed(_) => "placed", Phase::Dragging { .. } => "dragging" };
                return Err(format!("after {letter:?}: {phase}, dev path {:?} dragging {}", dev.path, dev.dragging));
            }
        }
        let ours = self.rt.text_selection().map(|(p, r)| (p.to_owned(), r));
        let theirs = dev.text_selection();
        if ours != theirs { return Err(format!("after {letter:?}: text_selection {ours:?}, dev {theirs:?}")); }
        Ok(())
    }
}

fn visit(walk: &mut Walk, sequence: &mut Vec<Letter>, ran: &mut usize, failure: &mut Option<String>) {
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
    for letter in ALPHABET {
        sequence.push(letter);
        visit(walk, sequence, ran, failure);
        sequence.pop();
    }
}

#[test]
fn every_short_sequence_over_virtual_rows_leaves_what_dev_left() {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut walk = Walk::new();
                let (mut ran, mut failure) = (0, None);
                while let Some(&letter) = ALPHABET.get(next.fetch_add(1, Ordering::Relaxed)) {
                    visit(&mut walk, &mut vec![letter], &mut ran, &mut failure);
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
    let expected: usize = (1..=LENGTH as u32).map(|n| ALPHABET.len().pow(n)).sum();
    assert_eq!(total, expected, "every sequence ran");
}

/// The facts the reference takes as constants: the row under each spot at either end of the
/// list, and that the anchor's row leaves the mounted window when the list scrolls to its end.
#[test]
fn the_virtual_reference_knows_which_row_each_spot_is_on() {
    let mut walk = Walk::new();
    for top in [true, false] {
        for spot in [Spot::HiStart, Spot::LoEnd] {
            walk.reset();
            if !top { walk.scroll(false); }
            let (x, y) = point(spot);
            walk.rt.pointer_down(x, y);
            let (row, at) = under(spot, top);
            assert_eq!(walk.rt.text_selection(), Some((path(row).as_str(), at..at)), "{spot:?} top {top}");
        }
    }
    walk.reset();
    assert!(walk.rt.virtuals[LIST].range.contains(&0));
    walk.scroll(false);
    assert!(!walk.rt.virtuals[LIST].range.contains(&0) && !walk.rt.virtuals[LIST].range.contains(&2), "{:?}", walk.rt.virtuals[LIST].range);
    walk.scroll(true);
    assert!(!walk.rt.virtuals[LIST].range.contains(&(ROWS - 1)), "{:?}", walk.rt.virtuals[LIST].range);
}

/// A selection made again over the same rows reuses the buckets of the map that hashes the
/// mounted rows between its ends: a press, a handler's clear and the next drag clear it in place.
#[test]
fn a_repeated_drag_across_virtual_rows_keeps_the_mounted_row_map() {
    let mut walk = Walk::new();
    let capacity = |walk: &Walk| walk.rt.selectable.mounted_between.capacity();
    for letter in [Letter::Press(Spot::HiStart), Letter::Drag(Spot::LoEnd), Letter::Up] { walk.play(letter).unwrap(); }
    assert!(walk.rt.selectable.mounted_between.contains_key("r1"), "row 1 lies between rows 0 and 2");
    let warm = capacity(&walk);
    assert!(warm > 0);
    for letter in [Letter::Press(Spot::HiStart), Letter::Drag(Spot::LoEnd), Letter::Up, Letter::Clear, Letter::Press(Spot::HiStart), Letter::Drag(Spot::LoEnd)] {
        walk.play(letter).unwrap();
        assert_eq!(capacity(&walk), warm, "after {letter:?}");
    }
    assert!(walk.rt.selectable.mounted_between.contains_key("r1"));
}
