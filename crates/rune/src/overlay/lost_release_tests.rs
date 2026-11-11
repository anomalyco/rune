//! Drives every short sequence of native and winit pointer events through the overlay host and
//! the panel's `Hold`, and checks what the runtime's drags hear against a simple reference.
//!
//! The reference knows only the order things happened in. Each point carries the press it
//! belongs to, so a position reaching an earlier press shows up without restating the machine.
//! winit can only dispatch a click the monitor saw, in order; a Down the panel gave up on never
//! arrives, but an Up may still arrive late.

use super::*;
use crate::platform::macos::PointerState;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::AtomicUsize;

type Point = (f32, f32);

const LENGTH: usize = 6;
const SLOP: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Letter { SeenDown, SeenUp, ReadUp, ReadDown, CursorMoved, MouseDown, MouseUp, Turn, Grace, Occlude, ViewLost }

const ALPHABET: [Letter; 11] = [Letter::SeenDown, Letter::SeenUp, Letter::ReadUp, Letter::ReadDown, Letter::CursorMoved,
    Letter::MouseDown, Letter::MouseUp, Letter::Turn, Letter::Grace, Letter::Occlude, Letter::ViewLost];

/// Whether presses, and readings and moves, land within the click slop of the last or far off.
#[derive(Clone, Copy, Debug)]
struct Plan { far_presses: bool, far_readings: bool }

const PLANS: [Plan; 3] = [Plan { far_presses: false, far_readings: false }, Plan { far_presses: false, far_readings: true }, Plan { far_presses: true, far_readings: true }];

/// Unique per event: `y` stays inside the slop and names the event.
fn point(index: usize, far: bool) -> Point {
    (if far { 100.0 + 30.0 * index as f32 } else { 10.0 }, 10.0 + 0.125 * index as f32)
}

fn event_of(point: Point) -> usize { ((point.1 - 10.0) / 0.125).round() as usize }

fn within_slop(a: Point, b: Point) -> bool { (a.0 - b.0).abs() <= SLOP && (a.1 - b.1).abs() <= SLOP }

struct OpenDrag { press: usize, down: Point, moved: bool }

struct Reference {
    plan: Plan,
    /// Clicks the monitor saw that winit hasn't dispatched: (down, point, press).
    queue: VecDeque<(bool, Point, usize)>,
    /// Presses seen so far; press `n` is the `n`th Down.
    seen: usize,
    /// The press each event's point belongs to.
    owner: Vec<usize>,
    /// By press: the last reading before its Down, its own Up, where the button first read up.
    before: Vec<Option<Point>>,
    ups: Vec<Option<Point>>,
    rose: Vec<Option<Point>>,
    /// The button read up then down with no Down seen: readings belong to the next press.
    rebound: bool,
    buttons: usize,
    at: Point,
    last_read: Point,
    now: Instant,
    occluded: bool,
    /// Presses up to this one were given up on.
    given_up: usize,
    open: Option<OpenDrag>,
    /// The last press: where, whether it moved, its count.
    last_press: Option<(Point, bool, u32)>,
}

enum Outcome { Illegal, Passed, Failed(&'static str, String) }

impl Reference {
    fn new(plan: Plan) -> Self {
        Self { plan, queue: VecDeque::new(), seen: 0, owner: vec![0], before: vec![None], ups: vec![None], rose: vec![None], rebound: false,
            buttons: 0, at: point(0, false), last_read: point(0, false), now: Instant::now(), occluded: false, given_up: 0, open: None, last_press: None }
    }

    fn event(&mut self, far: bool, owner: usize) -> Point {
        let index = self.owner.len();
        self.owner.push(owner);
        point(index, far)
    }

    fn reading_owner(&self) -> usize { self.seen + usize::from(self.rebound) }

    /// One sample of the current button and point.
    fn read(&mut self, pointer: &PointerState) {
        let held = pointer.captured();
        self.last_read = self.at;
        if self.buttons == 0 && held && self.rose[self.seen].is_none() { self.rose[self.seen] = Some(self.at); }
        pointer.read(self.buttons, self.now, self.at, true, false);
        if held && !pointer.captured() { self.give_up(); }
    }

    fn give_up(&mut self) {
        self.queue.retain(|click| !click.0);
        self.given_up = self.seen;
    }

    /// Where a press that lost its release must have let go.
    fn expected_cancel(&self, press: usize) -> Option<Point> {
        self.ups[press].or(self.rose[press]).or(self.before.get(press + 1).copied().flatten()).or(Some(self.last_read))
    }

    /// Plays one letter. `None` when winit couldn't have delivered it.
    fn apply(&mut self, overlay: &mut Overlay<u32>, pointer: &PointerState, letter: Letter) -> Option<Option<(bool, Point, usize)>> {
        let mut dispatched = None;
        match letter {
            Letter::SeenDown => {
                self.seen += 1;
                self.rebound = false;
                let at = self.event(self.plan.far_presses, self.seen);
                self.before.push(Some(self.last_read));
                self.ups.push(None);
                self.rose.push(None);
                self.queue.push_back((true, at, self.seen));
                (self.buttons, self.at) = (1, at);
                pointer.record_click(true, at.0, at.1);
            }
            Letter::SeenUp => {
                let at = self.event(self.plan.far_readings, self.seen);
                if self.ups[self.seen].is_none() && self.seen > self.given_up { self.ups[self.seen] = Some(at); }
                self.queue.push_back((false, at, self.seen));
                (self.buttons, self.at) = (0, at);
                pointer.record_click(false, at.0, at.1);
            }
            Letter::ReadUp => {
                self.at = self.event(self.plan.far_readings, self.reading_owner());
                self.buttons = 0;
                self.read(pointer);
            }
            Letter::ReadDown => {
                if self.buttons == 0 && self.seen > 0 { self.rebound = true; }
                self.at = self.event(self.plan.far_readings, self.reading_owner());
                self.buttons = 1;
                self.read(pointer);
            }
            Letter::CursorMoved => {
                // winit reports a click's position just before the click.
                let at = match self.queue.front() { Some(click) => click.1, None => self.event(self.plan.far_readings, self.seen) };
                overlay.before_position(pointer);
                overlay.cursor_moved(at.0, at.1);
            }
            Letter::MouseDown => {
                if !self.queue.front().is_some_and(|click| click.0) { return None; }
                dispatched = self.queue.pop_front();
                overlay.native_left(pointer, true);
            }
            Letter::MouseUp => {
                if !self.queue.front().is_some_and(|click| !click.0) { return None; }
                dispatched = self.queue.pop_front();
                overlay.native_left(pointer, false);
                pointer.released();
                self.read(pointer);
            }
            Letter::Turn => {
                self.read(pointer);
                if let Some((x, y)) = overlay.turn_position(pointer) && overlay.rt.pointer() != Some((x, y)) { overlay.observe_pointer(x, y); }
            }
            Letter::Grace => if let Some(due) = pointer.deadline(None, self.occluded) {
                self.now = due;
                self.read(pointer);
            },
            Letter::Occlude => self.occluded = !self.occluded,
            Letter::ViewLost => {
                // With nothing better known, the held press ends at the last reading, as of now.
                if let Some(press) = self.open.as_ref().map(|open| open.press) && self.before.get(press + 1).is_none() && self.rose[press].is_none() {
                    self.rose[press] = Some(self.last_read);
                }
                pointer.view_lost();
                self.queue.clear();
                self.given_up = self.seen;
            }
        }
        Some(dispatched)
    }

    /// Checks what one letter made the runtime hear.
    fn check(&mut self, letter: Letter, dispatched: Option<(bool, Point, usize)>, heard: Vec<Option<crate::Drag>>) -> Result<(), (&'static str, String)> {
        let mut dropped = false;
        for item in heard {
            let Some(drag) = item else {
                if !std::mem::take(&mut dropped) { return Err(("no-drop-after-cancel", "a press fired without a normal release".into())); }
                continue;
            };
            let at = (drag.window[0], drag.window[1]);
            dropped = false;
            match drag.phase {
                crate::Phase::Down => {
                    if self.open.is_some() { return Err(("ends-exactly-once", "a press replaced a drag that hadn't ended".into())); }
                    let Some((true, down, press)) = dispatched.filter(|_| letter == Letter::MouseDown) else {
                        return Err(("ends-exactly-once", "a drag began without a dispatched press".into()));
                    };
                    if at != down { return Err(("own-point", format!("press at {down:?} began at {at:?}"))); }
                    let clicks = match self.last_press { Some((was, false, count)) if within_slop(was, down) => count + 1, _ => 1 };
                    if drag.clicks != clicks { return Err(("click-counts", format!("{} clicks, reference {clicks}", drag.clicks))); }
                    self.last_press = Some((down, false, clicks));
                    self.open = Some(OpenDrag { press, down, moved: false });
                }
                crate::Phase::Move | crate::Phase::Up => {
                    let up = drag.phase == crate::Phase::Up;
                    let Some(open) = &mut self.open else { return Err(("ends-exactly-once", format!("{:?} on a drag already ended", drag.phase))); };
                    let owner = self.owner[event_of(at)];
                    if owner > open.press { return Err(("no-later-press-position", format!("press {} heard {:?} at {at:?}, from press {owner}", open.press, drag.phase))); }
                    open.moved |= !within_slop(open.down, at);
                    let (press, moved) = (open.press, open.moved);
                    if let Some(last) = &mut self.last_press { last.1 = moved; }
                    if !up { continue; }
                    if drag.cancelled() {
                        let want = self.expected_cancel(press);
                        if Some(at) != want { return Err(("own-point", format!("press {press} cancelled at {at:?}, reference {want:?}"))); }
                    } else {
                        let own = dispatched.is_some_and(|(down, _, owner)| !down && owner == press) && letter == Letter::MouseUp;
                        if !own || press <= self.given_up { return Err(("no-drop-after-cancel", format!("press {press} ended normally after losing its release"))); }
                        dropped = true;
                    }
                    self.open = None;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn overlay() -> Overlay<u32> {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        crate::el().w_full().h_full().hit().on_drag(|_: &mut u32, _, _| {}).on_press(|_: &mut u32| {})
    }
    let app = AppSpec {
        title: "sequences", size: (1000.0, 100.0), min_size: None, init: || 0, view,
        scenes: &[], fonts: &[], families: ("sans-serif", "monospace"), setup: None, poll: None, on_key: None,
    };
    let spec = OverlaySpec::new("sequences", app.size, view).options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
    let (mut overlay, _) = Overlay::new(spec, 0, &app);
    overlay.rt.frame(0.0);
    overlay
}

fn run(overlay: &mut Overlay<u32>, plan: Plan, sequence: &[Letter]) -> Outcome {
    overlay.rt.cancel_pointer();
    overlay.rt.end_click_run();
    overlay.rt.pointer_move(5.0, 5.0);
    overlay.rt.take_queued_pointer();
    let pointer = PointerState::new(OverlayInput::All, false);
    let mut model = Reference::new(plan);
    model.read(&pointer);
    // Then the button comes to rest and the grace runs out, however the sequence left things.
    for &letter in sequence.iter().chain(&[Letter::ReadUp, Letter::Grace, Letter::Turn]) {
        let Some(dispatched) = model.apply(overlay, &pointer, letter) else { return Outcome::Illegal };
        let heard = overlay.rt.take_queued_pointer();
        if let Err((invariant, why)) = model.check(letter, dispatched, heard) { return Outcome::Failed(invariant, why); }
    }
    if model.open.is_some() { return Outcome::Failed("ends-exactly-once", "the drag never ended".into()); }
    if pointer.captured() { return Outcome::Failed("ends-exactly-once", "the panel still holds the pointer".into()); }
    Outcome::Passed
}

#[derive(Default)]
struct Tally { ran: usize, failures: BTreeMap<&'static str, (usize, String)> }

fn visit(overlay: &mut Overlay<u32>, plan: Plan, sequence: &mut Vec<Letter>, tally: &mut Tally) {
    match run(overlay, plan, sequence) {
        Outcome::Illegal => return,
        Outcome::Passed => tally.ran += 1,
        Outcome::Failed(invariant, why) => {
            tally.ran += 1;
            let entry = tally.failures.entry(invariant).or_insert_with(|| (0, format!("{plan:?} {sequence:?}: {why}")));
            entry.0 += 1;
        }
    }
    if sequence.len() == LENGTH { return; }
    for letter in ALPHABET {
        sequence.push(letter);
        visit(overlay, plan, sequence, tally);
        sequence.pop();
    }
}

#[test]
fn every_short_sequence_of_pointer_events_keeps_the_drag_invariants() {
    let jobs: Vec<(Plan, Letter)> = PLANS.iter().flat_map(|plan| ALPHABET.iter().map(|letter| (*plan, *letter))).collect();
    let next = AtomicUsize::new(0);
    let total = Mutex::new(Tally::default());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| for _ in 0..workers {
        scope.spawn(|| {
            let mut overlay = overlay();
            let mut tally = Tally::default();
            while let Some((plan, letter)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                visit(&mut overlay, *plan, &mut vec![*letter], &mut tally);
            }
            let mut total = total.lock().unwrap();
            total.ran += tally.ran;
            for (invariant, (count, first)) in tally.failures {
                let entry = total.failures.entry(invariant).or_insert((0, first));
                entry.0 += count;
            }
        });
    });
    let total = total.into_inner().unwrap();
    println!("{} sequences up to length {LENGTH}", total.ran);
    assert!(total.ran > 100_000, "only {} sequences were legal", total.ran);
    assert!(total.failures.is_empty(), "{} sequences, failures: {:#?}", total.ran, total.failures);
}
