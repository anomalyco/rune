//! The edge scroll's walk: from each state a selection can be in when the pointer reaches the
//! edge of its virtual list (no press, a press by character, word or paragraph, a press whose
//! anchor row has scrolled out, a press at the list's end), every short sequence of the pointer
//! leaving below or above the list or coming back inside, frames, release, Escape and a lost
//! press, checked after every letter against a reference: `selection_phase_virtual`'s
//! transcription of the code before `selectable::phase`, on forty rows that never change, with
//! the list's offset kept as a number and the rows each spot is over worked out from it. The edge
//! scroll itself and Escape ending a selection drag are new, marked `differs from dev`.
//! Both sides' selection, virtual ends, offset and whether a step is asked for are compared.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::runtime::selectable::Unit;
use crate::runtime::selectable::phase::Phase;
use crate::{GrowthSpec, Key, KeyEvent, Mods, ScrollSpec, TextStyle, text};

/// Sequences of up to this many letters after each seed.
const LENGTH: usize = 4;
const ROWS: usize = 40;
const ROW_H: f64 = 40.0;
const VIEW_H: f64 = 120.0;
/// How far the list scrolls.
const MAX: f64 = ROWS as f64 * ROW_H - VIEW_H;
const LIST: &str = "root/scroll/list";
const LEN: usize = 6;
const SLOP: f32 = 4.0;
/// One frame, and the speed of a pointer 301 points past an edge (`edge_scroll`'s rate of ten
/// points a second per point, counting from a point inside the edge).
const FRAME: f64 = 1.0 / 60.0;
const SPEED: f32 = 3010.0;

fn content(row: usize) -> String { format!("row {row:02}") }

fn path(row: usize) -> String { format!("{LIST}/r{row}") }

fn row_of(path: &str) -> usize { path.rsplit_once("/r").and_then(|(_, r)| r.parse().ok()).expect("a row path") }

#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot { HiStart, LoEnd, Below, Above }

fn point(spot: Spot) -> (f32, f32) {
    match spot {
        Spot::HiStart => (2.0, 20.0),
        Spot::LoEnd => (296.0, 100.0),
        Spot::Below => (150.0, 420.0),
        Spot::Above => (150.0, -300.0),
    }
}

/// The row under `spot` with the list scrolled to `offset`, and the byte a press or drag there
/// lands on: inside, the row at its height; below the list, the end of its last row that shows;
/// above it, the start of its first.
fn under(spot: Spot, offset: f64) -> (usize, usize) {
    let row = |y: f64| ((y / ROW_H).floor() as usize).min(ROWS - 1);
    match spot {
        Spot::HiStart => (row(offset + 20.0), 0),
        Spot::LoEnd => (row(offset + 100.0), LEN),
        Spot::Below => (((offset + VIEW_H) / ROW_H).ceil() as usize - 1, LEN),
        Spot::Above => (row(offset), 0),
    }
}

#[derive(Clone, Copy, Debug)]
enum Letter { Move(Spot), Tick, Ticks, Up, Escape, Lost }

const ALPHABET: [Letter; 8] = [
    Letter::Move(Spot::Below), Letter::Move(Spot::Above), Letter::Move(Spot::LoEnd),
    Letter::Tick, Letter::Ticks, Letter::Up, Letter::Escape, Letter::Lost,
];

#[derive(Clone, Copy, Debug)]
enum Seed { None, Char, Word, Paragraph, Offscreen, AtEnd }

const SEEDS: [Seed; 6] = [Seed::None, Seed::Char, Seed::Word, Seed::Paragraph, Seed::Offscreen, Seed::AtEnd];

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().key("root").w(300.0).h(VIEW_H as f32).child(
        col().key("scroll").w_full().h(VIEW_H as f32).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("list").w_full().growth(GrowthSpec::default()).selection_group(true).virtual_rows(
                ROWS,
                ROW_H as f32,
                |_: &(), i| format!("r{i}").into(),
                |_: &(), i| text(content(i)).font(TextStyle::new(14.0, 400.0)).w_full().h(ROW_H as f32).selectable(true),
            ),
        ),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum U { Char, Word, Paragraph }

fn unit_at(i: usize, unit: U) -> (usize, usize) {
    let i = if i == LEN && unit != U::Char { i - 1 } else { i };
    match unit {
        U::Char => (i, i),
        U::Paragraph => (0, LEN),
        U::Word => match i { 0..=2 => (0, 3), 3 => (3, 4), _ => (4, LEN) },
    }
}

fn extend(origin: (usize, usize), at: (usize, usize)) -> (usize, usize) {
    if at.0 < origin.0 { (origin.1, at.0) } else { (origin.0, at.1.max(origin.1)) }
}

struct Run {
    path: String,
    point: (f32, f32),
    count: u32,
}

/// differs from dev: the edge scroll, which dev doesn't have.
#[derive(Clone, Copy, Debug, PartialEq)]
enum EdgeRef { Idle, Running }

/// The selection as dev kept it (`selection_phase_virtual`'s `Dev`), the list's offset, and the
/// edge scroll.
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
    offset: f64,
    at: Spot,
    edge: EdgeRef,
}

impl Dev {
    fn new() -> Self {
        Self {
            path: None, content: String::new(), anchor: 0, focus: 0, far: None, unit: U::Char, origin: (0, 0), dragging: false,
            presses: None, offset: 0.0, at: Spot::HiStart, edge: EdgeRef::Idle,
        }
    }

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

    fn down(&mut self, spot: Spot) {
        let (row, at) = under(spot, self.offset);
        let clicks = self.press_count(&path(row), point(spot)).min(3);
        let unit = match clicks { 1 => U::Char, 2 => U::Word, _ => U::Paragraph };
        let origin = unit_at(at, unit);
        (self.unit, self.origin) = (unit, origin);
        (self.anchor, self.focus) = origin;
        self.path = Some(path(row));
        self.content = content(row);
        self.far = None;
        self.dragging = true;
        self.at = spot;
    }

    /// `selection_drag` to `self.at` over the list at its offset.
    fn drag(&mut self) {
        if !self.dragging { return; }
        let Some(h) = self.path.as_deref().map(row_of) else { return };
        let (j, at) = under(self.at, self.offset);
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

    /// differs from dev: how fast the held pointer scrolls the list, and the room left that way.
    fn edge_at(&self) -> Option<(f32, f64)> {
        if !self.dragging { return None; }
        let e = match self.at {
            Spot::Below => (SPEED, MAX - self.offset),
            Spot::Above => (-SPEED, self.offset),
            _ => return None,
        };
        (e.1 > 0.0).then_some(e)
    }

    /// The held pointer moved to `spot`.
    fn moved(&mut self, spot: Spot) {
        self.at = spot;
        self.drag();
        // differs from dev: at an edge with room it steps (on from a step it was taking);
        // anywhere else it stops.
        self.edge = if self.edge_at().is_some() { EdgeRef::Running } else { EdgeRef::Idle };
    }

    /// differs from dev: a frame takes the step due, scrolling and then extending over the
    /// list at its new offset; the step that reaches the end is the last.
    fn tick(&mut self) {
        match self.edge {
            EdgeRef::Idle => {}
            EdgeRef::Running => {
                let Some((speed, room)) = self.edge_at() else {
                    self.edge = EdgeRef::Idle;
                    return;
                };
                let by = (f64::from(speed) * FRAME).clamp(-room, room);
                let landing = room - by.abs() <= 1e-3;
                let by = if landing { room.copysign(f64::from(speed)) } else { by };
                self.offset = (self.offset + by).clamp(0.0, MAX);
                self.drag();
                self.edge = if landing { EdgeRef::Idle } else { EdgeRef::Running };
            }
        }
    }

    /// The press ends without a click.
    fn let_go(&mut self) {
        self.dragging = false;
        self.edge = EdgeRef::Idle;
    }

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
    /// Frames the letters ran.
    frames: usize,
}

impl Walk {
    fn new() -> Self {
        let mut rt = Runtime::new((), view, (300.0, VIEW_H as f32), Env::new(true, 1.0));
        rt.double_click_interval = 0.3;
        let mut walk = Self { rt, dev: Dev::new(), frames: 0 };
        walk.settle();
        walk
    }

    fn settle(&mut self) {
        for _ in 0..60 {
            if !self.rt.frame(FRAME) { break; }
        }
        self.rt.clock = 0.0;
    }

    fn offset(&self) -> f64 { self.rt.scrolls.values().next().expect("one scroll").offset }

    fn to_end(&mut self) {
        self.rt.wheel(150.0, 60.0, -10_000.0);
        self.settle();
        self.dev.offset = MAX;
    }

    fn press(&mut self, spot: Spot) {
        let (x, y) = point(spot);
        self.rt.pointer_move(x, y);
        self.rt.pointer_down(x, y);
        self.dev.down(spot);
    }

    fn up(&mut self) {
        let (x, y) = point(self.dev.at);
        self.rt.pointer_up(x, y);
        self.dev.drag();
        self.dev.let_go();
    }

    fn reset(&mut self, seed: Seed) {
        self.rt.cancel_pointer();
        self.rt.selectable = Default::default();
        self.rt.sync_selection_pins();
        self.rt.queue.retain(|_| false);
        self.rt.wake_deadlines.clear();
        if self.offset() != 0.0 {
            self.rt.wheel(150.0, 60.0, 10_000.0);
            self.settle();
        }
        self.rt.clock = 0.0;
        self.dev = Dev::new();
        match seed {
            Seed::None => {}
            Seed::Char => self.press(Spot::HiStart),
            Seed::Word | Seed::Paragraph => {
                for _ in 0..if matches!(seed, Seed::Word) { 1 } else { 2 } {
                    self.press(Spot::HiStart);
                    self.up();
                }
                self.press(Spot::HiStart);
            }
            Seed::Offscreen => {
                self.press(Spot::HiStart);
                self.to_end();
            }
            Seed::AtEnd => {
                self.to_end();
                self.press(Spot::LoEnd);
            }
        }
    }

    fn frame(&mut self) {
        self.frames += 1;
        self.rt.frame(FRAME);
        self.dev.tick();
    }

    fn play(&mut self, letter: Letter) -> Result<(), String> {
        let frames = self.frames;
        match letter {
            Letter::Move(spot) => {
                let (x, y) = point(spot);
                self.rt.pointer_move(x, y);
                self.dev.moved(spot);
            }
            Letter::Tick => self.frame(),
            Letter::Ticks => {
                for _ in 0..100 {
                    if self.dev.edge == EdgeRef::Idle { break; }
                    self.frame();
                }
            }
            Letter::Up => self.up(),
            Letter::Escape => {
                self.rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
                // differs from dev: Escape ends a selection drag where it stands, without a click.
                self.dev.let_go();
            }
            Letter::Lost => {
                self.rt.cancel_pointer();
                self.dev.let_go();
            }
        }
        self.compare(letter, self.frames != frames)
    }

    /// `framed`: a frame ran since the last letter, so every step asked for before it was taken.
    fn compare(&self, letter: Letter, framed: bool) -> Result<(), String> {
        let dev = &self.dev;
        if (self.offset() - dev.offset).abs() > 1e-6 {
            return Err(format!("after {letter:?}: offset {}, dev {}", self.offset(), dev.offset));
        }
        let running = dev.edge != EdgeRef::Idle;
        if self.rt.selectable.edge.running() != running {
            return Err(format!("after {letter:?}: edge running {}, dev {:?}", self.rt.selectable.edge.running(), dev.edge));
        }
        if framed && self.rt.wake_deadline().is_some() != running {
            return Err(format!("after {letter:?}: wake {:?}, dev {:?}", self.rt.wake_deadline(), dev.edge));
        }
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
                if let Some(k) = self.rt.selectable.mounted_between.keys().find(|k| {
                    let r: usize = k[1..].parse().unwrap_or(usize::MAX);
                    r <= h.min(f) || r >= h.max(f)
                }) {
                    return Err(format!("after {letter:?}: row {k} hashed as between rows {h} and {f}"));
                }
            }
            (phase, _) => {
                let phase = match phase { Phase::Empty => "empty", Phase::Placed(_) => "placed", Phase::Dragging { .. } => "dragging" };
                return Err(format!("after {letter:?}: {phase}, dev path {:?} dragging {}", dev.path, dev.dragging));
            }
        }
        let ours = self.rt.text_selection().map(|(p, r)| (p.to_owned(), r));
        if ours != dev.text_selection() { return Err(format!("after {letter:?}: text_selection {ours:?}, dev {:?}", dev.text_selection())); }
        Ok(())
    }
}

fn visit(walk: &mut Walk, seed: Seed, sequence: &mut Vec<Letter>, ran: &mut usize, failure: &mut Option<String>) {
    if failure.is_some() { return; }
    *ran += 1;
    walk.reset(seed);
    if let Err(why) = walk.compare(Letter::Up, false) {
        *failure = Some(format!("{seed:?} seeded: {why}"));
        return;
    }
    for &letter in sequence.iter() {
        if let Err(why) = walk.play(letter) {
            *failure = Some(format!("{seed:?} {sequence:?}: {why}"));
            return;
        }
    }
    if sequence.len() == LENGTH { return; }
    for letter in ALPHABET {
        sequence.push(letter);
        visit(walk, seed, sequence, ran, failure);
        sequence.pop();
    }
}

#[test]
fn every_short_edge_sequence_leaves_what_the_reference_leaves() {
    let jobs: Vec<(Seed, Letter)> = SEEDS.iter().flat_map(|&s| ALPHABET.iter().map(move |&l| (s, l))).collect();
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut walk = Walk::new();
                let (mut ran, mut failure) = (0, None);
                while let Some(&(seed, letter)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    visit(&mut walk, seed, &mut vec![letter], &mut ran, &mut failure);
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
    assert!(failures.is_empty(), "sequences differ from the reference: {failures:#?}");
    let expected: usize = SEEDS.len() * (1..=LENGTH as u32).map(|n| ALPHABET.len().pow(n)).sum::<usize>();
    assert_eq!(total, expected, "every sequence ran");
}

/// The facts the reference takes as constants: how far the list scrolls, which row each spot is
/// over at either end, and that a seed's anchor row scrolls out.
#[test]
fn the_edge_reference_knows_the_list() {
    let mut walk = Walk::new();
    walk.to_end();
    assert_eq!(walk.offset(), MAX);
    walk.reset(Seed::Offscreen);
    assert!(!walk.rt.virtuals[LIST].range.contains(&0), "{:?}", walk.rt.virtuals[LIST].range);
    for (seed, at) in [(Seed::Char, (0, 0)), (Seed::AtEnd, (ROWS - 1, LEN))] {
        walk.reset(seed);
        assert_eq!(walk.rt.text_selection(), Some((path(at.0).as_str(), at.1..at.1)), "{seed:?}");
    }
    assert_eq!(under(Spot::Below, 0.0), (2, LEN));
    assert_eq!(under(Spot::Below, MAX), (ROWS - 1, LEN));
    assert_eq!(under(Spot::Above, MAX), (ROWS - 3, 0));
}
