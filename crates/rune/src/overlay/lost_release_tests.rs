//! Drives every short sequence of native and winit pointer events through the overlay host and
//! the panel's `Hold`, and checks what the runtime's drags hear against a reference.
//!
//! The reference sees only the inputs. It keeps its own clock and history, predicts each letter's
//! callbacks, capture and deadline before production runs it, and production's answers are
//! compared with those predictions. Each point carries the press it belongs to, so the
//! invariants can name a position reaching an earlier press.
//!
//! winit can only dispatch a click the monitor saw, in order; a Down the panel gave up on never
//! arrives, but an Up may still arrive late. A press the monitor never saw is out of scope, and
//! the reference expects a winit move during the grace to reach the drag, as it always has.

use super::*;
use crate::platform::macos::PointerState;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::AtomicUsize;

type Point = (f32, f32);

const LENGTH: usize = 6;
const SLOP: f32 = 4.0;
const UNSEEN_UP_GRACE: Duration = Duration::from_millis(100);
const QUEUED_UP_GRACE: Duration = Duration::from_secs(2);

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

/// What a drag's handlers hear.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Heard { Down(Point, u32), Move(Point), Up(Point), Cancel(Point), Press }

#[derive(Clone, Copy)]
struct Queued { down: bool, at: Point, press: usize, /// The panel gave up on it, so it arrives with no point.
    forgotten: bool, /// A Down whose press before it never got its Up.
    owes: bool }

/// The runtime's held drag, as predicted.
#[derive(Clone, Copy)]
struct Predicted { press: usize, down: Point, moved: bool }

/// What one letter is, for the driver to play and the invariants to read.
struct Played { at: Option<Point>, reading: Option<(usize, Point, Duration)>, dispatched: Option<Queued>, expect: Vec<Heard> }

struct Reference {
    plan: Plan,
    clock: Duration,
    queue: VecDeque<Queued>,
    seen: usize,
    /// The press each event's point belongs to.
    owner: Vec<usize>,
    /// By press: its own Up, where the button first read up, where it was last read down.
    ups: Vec<Option<Point>>,
    rose: Vec<Option<Point>>,
    latest: Vec<Point>,
    buttons: usize,
    at: Point,
    /// The button read up then down with no Down seen: readings belong to the next press.
    next_press_under_way: bool,
    last_read: Option<Point>,
    pending: Option<Point>,
    holding: bool,
    /// When the button read up, while a press is held.
    rising: Option<Duration>,
    rebounded: bool,
    /// A release was given up on and the runtime hasn't heard.
    owed: bool,
    given_up: usize,
    occluded: bool,
    drag: Option<Predicted>,
    runtime_at: Point,
    /// The run of clicks: its last press and count.
    run: Option<(Point, u32)>,
    // The invariants' own view of what was heard.
    open: Option<Predicted>,
    last_press: Option<(Point, bool, u32)>,
}

enum Outcome { Illegal, Passed, Failed(&'static str, String) }

impl Reference {
    fn new(plan: Plan, runtime_at: Point) -> Self {
        Self { plan, clock: Duration::ZERO, queue: VecDeque::new(), seen: 0, owner: vec![0], ups: vec![None], rose: vec![None], latest: vec![point(0, false)],
            buttons: 0, at: point(0, false), next_press_under_way: false, last_read: None, pending: None, holding: false, rising: None, rebounded: false,
            owed: false, given_up: 0, occluded: false, drag: None, runtime_at, run: None, open: None, last_press: None }
    }

    fn event(&mut self, far: bool, owner: usize) -> Point {
        let index = self.owner.len();
        self.owner.push(owner);
        point(index, far)
    }

    fn reading_owner(&self) -> usize { self.seen + usize::from(self.next_press_under_way) }

    fn live(&self) -> impl DoubleEndedIterator<Item = &Queued> { self.queue.iter().filter(|click| !click.forgotten) }

    fn deadline(&self) -> Option<Duration> {
        let since = self.rising.filter(|_| self.holding)?;
        Some(since + if self.live().any(|click| !click.down) { QUEUED_UP_GRACE } else { UNSEEN_UP_GRACE })
    }

    /// One sample of the current button and point.
    fn read(&mut self) -> (usize, Point, Duration) {
        if self.holding {
            if self.buttons != 0 {
                if self.rising.take().is_some() { self.rebounded = true; } else if !self.rebounded { self.latest[self.seen] = self.at; }
            } else if self.rising.is_none() {
                self.rising = Some(self.clock);
                self.rebounded = false;
                if self.rose[self.seen].is_none() { self.rose[self.seen] = Some(self.at); }
            } else if self.deadline().is_some_and(|due| self.clock >= due) {
                self.give_up();
                for click in &mut self.queue { click.forgotten = true; }
                self.queue.retain(|click| !click.down);
            }
        }
        if self.last_read != Some(self.at) {
            self.last_read = Some(self.at);
            self.pending = Some(self.at);
        }
        (self.buttons, self.at, self.clock)
    }

    fn give_up(&mut self) {
        self.holding = false;
        self.rising = None;
        self.rebounded = false;
        self.given_up = self.seen;
        self.owed |= self.drag.is_some();
    }

    /// Where a press that lost its release let go.
    fn expected_cancel(&self, press: usize) -> Point {
        self.ups[press].or(self.rose[press]).unwrap_or(self.latest[press])
    }

    fn land_lost(&mut self, expect: &mut Vec<Heard>) {
        if !std::mem::take(&mut self.owed) { return; }
        let Some(drag) = self.drag.take() else { return };
        let at = self.expected_cancel(drag.press);
        expect.push(Heard::Cancel(at));
        if drag.moved || !within_slop(drag.down, at) { self.run = None; }
        self.runtime_at = at;
    }

    fn before_position(&mut self, expect: &mut Vec<Heard>) {
        if let Some(front) = self.queue.iter_mut().find(|click| !click.forgotten) && std::mem::take(&mut front.owes) { self.owed = true; }
        self.land_lost(expect);
    }

    fn position(&mut self, at: Point, expect: &mut Vec<Heard>) {
        if self.runtime_at == at { return; }
        self.runtime_at = at;
        if let Some(drag) = &mut self.drag {
            expect.push(Heard::Move(at));
            drag.moved |= !within_slop(drag.down, at);
        }
    }

    /// Predicts one letter. `None` when winit couldn't have delivered it.
    fn play(&mut self, letter: Letter) -> Option<Played> {
        let (mut at, mut reading, mut dispatched, mut expect) = (None, None, None, Vec::new());
        match letter {
            Letter::SeenDown => {
                let owes = self.holding && self.live().next_back().is_none_or(|click| click.down);
                self.seen += 1;
                self.next_press_under_way = false;
                let point = self.event(self.plan.far_presses, self.seen);
                self.ups.push(None);
                self.rose.push(None);
                self.latest.push(point);
                self.queue.push_back(Queued { down: true, at: point, press: self.seen, forgotten: false, owes });
                self.holding = true;
                self.rising = None;
                self.rebounded = false;
                (self.buttons, self.at, at) = (1, point, Some(point));
            }
            Letter::SeenUp => {
                let point = self.event(self.plan.far_readings, self.seen);
                if self.ups[self.seen].is_none() && self.seen > self.given_up { self.ups[self.seen] = Some(point); }
                self.queue.push_back(Queued { down: false, at: point, press: self.seen, forgotten: false, owes: false });
                (self.buttons, self.at, at) = (0, point, Some(point));
            }
            Letter::ReadUp => {
                self.at = self.event(self.plan.far_readings, self.reading_owner());
                self.buttons = 0;
                reading = Some(self.read());
            }
            Letter::ReadDown => {
                if self.buttons == 0 && self.seen > 0 { self.next_press_under_way = true; }
                self.at = self.event(self.plan.far_readings, self.reading_owner());
                self.buttons = 1;
                reading = Some(self.read());
            }
            Letter::CursorMoved => {
                // winit reports a click's position just before the click.
                let point = match self.queue.front() { Some(click) => click.at, None => self.event(self.plan.far_readings, self.seen) };
                self.before_position(&mut expect);
                self.position(point, &mut expect);
                at = Some(point);
            }
            Letter::MouseDown => {
                let click = self.queue.pop_front().filter(|click| click.down);
                let Some(click) = click else { return None };
                self.owed |= click.owes;
                self.land_lost(&mut expect);
                let clicks = match self.run { Some((was, count)) if within_slop(was, click.at) => count + 1, _ => 1 };
                self.run = Some((click.at, clicks));
                self.runtime_at = click.at;
                self.drag = Some(Predicted { press: click.press, down: click.at, moved: false });
                expect.push(Heard::Down(click.at, clicks));
                dispatched = Some(click);
            }
            Letter::MouseUp => {
                if !self.queue.front().is_some_and(|click| !click.down) { return None; }
                let click = self.queue.pop_front()?;
                self.land_lost(&mut expect);
                let released = if click.forgotten { self.runtime_at } else { click.at };
                self.position(released, &mut expect);
                if let Some(drag) = self.drag.take() {
                    expect.extend([Heard::Up(released), Heard::Press]);
                    if drag.moved { self.run = None; }
                }
                let holding = self.live().any(|click| click.down);
                self.holding = holding;
                if !holding { (self.rising, self.rebounded) = (None, false); }
                reading = Some(self.read());
                dispatched = Some(click);
            }
            Letter::Turn => {
                reading = Some(self.read());
                self.before_position(&mut expect);
                let ahead = self.holding && (self.rising.is_some() || self.rebounded || self.live().next().is_some());
                if !ahead && let Some(point) = self.pending.take() { self.position(point, &mut expect); }
            }
            Letter::Grace => if let Some(due) = self.deadline() {
                self.clock = due;
                reading = Some(self.read());
            },
            Letter::Occlude => self.occluded = !self.occluded,
            Letter::ViewLost => {
                // With nothing newer read, the held press ends where it was last known.
                self.give_up();
                self.queue.clear();
            }
        }
        Some(Played { at, reading, dispatched, expect })
    }

    /// The invariants, on what one letter made the runtime hear.
    fn check(&mut self, letter: Letter, dispatched: Option<Queued>, heard: &[Heard]) -> Result<(), (&'static str, String)> {
        let mut dropped = false;
        for item in heard {
            let (at, up) = match *item {
                Heard::Press => {
                    if !std::mem::take(&mut dropped) { return Err(("no-drop-after-cancel", "a press fired without a normal release".into())); }
                    continue;
                }
                Heard::Down(down, clicks) => {
                    dropped = false;
                    if self.open.is_some() { return Err(("ends-exactly-once", "a press replaced a drag that hadn't ended".into())); }
                    let Some(click) = dispatched.filter(|click| click.down && letter == Letter::MouseDown) else {
                        return Err(("ends-exactly-once", "a drag began without a dispatched press".into()));
                    };
                    if down != click.at { return Err(("own-point", format!("press at {:?} began at {down:?}", click.at))); }
                    let want = match self.last_press { Some((was, false, count)) if within_slop(was, down) => count + 1, _ => 1 };
                    if clicks != want { return Err(("click-counts", format!("{clicks} clicks, reference {want}"))); }
                    self.last_press = Some((down, false, clicks));
                    self.open = Some(Predicted { press: click.press, down, moved: false });
                    continue;
                }
                Heard::Move(at) => (at, false),
                Heard::Up(at) | Heard::Cancel(at) => (at, true),
            };
            dropped = false;
            let Some(open) = &mut self.open else { return Err(("ends-exactly-once", format!("{item:?} on a drag already ended"))); };
            let owner = self.owner[event_of(at)];
            if owner > open.press { return Err(("no-later-press-position", format!("press {} heard {item:?}, from press {owner}", open.press))); }
            open.moved |= !within_slop(open.down, at);
            let (press, moved) = (open.press, open.moved);
            if let Some(last) = &mut self.last_press { last.1 = moved; }
            if !up { continue; }
            if matches!(item, Heard::Cancel(_)) {
                let want = self.expected_cancel(press);
                if at != want { return Err(("own-point", format!("press {press} cancelled at {at:?}, reference {want:?}"))); }
            } else {
                let own = dispatched.is_some_and(|click| !click.down && click.press == press) && letter == Letter::MouseUp;
                if !own || press <= self.given_up { return Err(("no-drop-after-cancel", format!("press {press} ended normally after losing its release"))); }
                dropped = true;
            }
            self.open = None;
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

/// Plays a letter on production: the only place the test touches it.
fn drive(overlay: &mut Overlay<u32>, pointer: &PointerState, letter: Letter, played: &Played, base: Instant) -> Vec<Heard> {
    let read = |pointer: &PointerState| if let Some((buttons, at, clock)) = played.reading { pointer.read(buttons, base + clock, at, true, false); };
    let at = played.at.unwrap_or_default();
    match letter {
        Letter::SeenDown => pointer.record_click(true, at.0, at.1),
        Letter::SeenUp => pointer.record_click(false, at.0, at.1),
        Letter::ReadUp | Letter::ReadDown | Letter::Grace => read(pointer),
        Letter::CursorMoved => {
            overlay.before_position(pointer);
            overlay.cursor_moved(at.0, at.1);
        }
        Letter::MouseDown => overlay.native_left(pointer, true),
        Letter::MouseUp => {
            overlay.native_left(pointer, false);
            pointer.released();
            read(pointer);
        }
        Letter::Turn => {
            read(pointer);
            if let Some((x, y)) = overlay.turn_position(pointer) && overlay.rt.pointer() != Some((x, y)) { overlay.observe_pointer(x, y); }
        }
        Letter::Occlude => {}
        Letter::ViewLost => pointer.view_lost(),
    }
    overlay.rt.take_queued_pointer().into_iter().map(|item| match item {
        None => Heard::Press,
        Some(drag) => {
            let at = (drag.window[0], drag.window[1]);
            match drag.phase {
                crate::Phase::Down => Heard::Down(at, drag.clicks),
                crate::Phase::Up if drag.cancelled() => Heard::Cancel(at),
                crate::Phase::Up => Heard::Up(at),
                _ => Heard::Move(at),
            }
        }
    }).collect()
}

fn run(overlay: &mut Overlay<u32>, plan: Plan, sequence: &[Letter]) -> Outcome {
    let start = (5.0, 5.0);
    overlay.rt.cancel_pointer();
    overlay.rt.end_click_run();
    overlay.rt.pointer_move(start.0, start.1);
    overlay.rt.take_queued_pointer();
    let pointer = PointerState::new(OverlayInput::All, false);
    let base = Instant::now();
    let mut model = Reference::new(plan, start);
    let (buttons, at, clock) = model.read();
    pointer.read(buttons, base + clock, at, true, false);
    // Then the button comes to rest and the grace runs out, however the sequence left things.
    for &letter in sequence.iter().chain(&[Letter::ReadUp, Letter::Grace, Letter::Turn]) {
        let Some(played) = model.play(letter) else { return Outcome::Illegal };
        let heard = drive(overlay, &pointer, letter, &played, base);
        if let Err((invariant, why)) = model.check(letter, played.dispatched, &heard) { return Outcome::Failed(invariant, why); }
        if heard != played.expect { return Outcome::Failed("predicted-callbacks", format!("after {letter:?} heard {heard:?}, predicted {:?}", played.expect)); }
        if pointer.captured() != model.holding { return Outcome::Failed("predicted-capture", format!("after {letter:?} captured {}, predicted {}", pointer.captured(), model.holding)); }
        let (deadline, predicted) = (pointer.deadline(None, model.occluded), model.deadline().map(|due| base + due));
        if deadline != predicted { return Outcome::Failed("predicted-deadline", format!("after {letter:?} {deadline:?}, predicted {predicted:?}")); }
    }
    if model.open.is_some() || model.drag.is_some() { return Outcome::Failed("ends-exactly-once", "the drag never ended".into()); }
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
