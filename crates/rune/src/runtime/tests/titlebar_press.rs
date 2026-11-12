//! Drives every short sequence of titlebar presses through the runtime and checks the press
//! state, the request handed to the shell and the window menu against a literal transcription of
//! the code before `TitlebarPress` replaced it: `action: Option<TitlebarAction>`, a `PressRun`
//! for the pair, `secondary_on_region: bool` and `window_menu`.
//!
//! The reference sees only the letters. It keeps its own run of presses (with `PressRun`'s count,
//! as it was), its own band, its own secondary flag and its own request slot, and the runtime's
//! answers are compared with it after every letter, on both a custom titlebar that draws caption
//! buttons and one that draws none.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// Sequences of up to this many letters.
const LENGTH: usize = 5;
/// The double-click interval both sides run with.
const INTERVAL: f64 = 0.3;
/// `CLICK_SLOP`, as the reference transcribes it.
const SLOP: f32 = 4.0;
/// The native titlebar band both sides run with.
const BAND: f32 = 28.0;

/// A press on the bare `.window_drag()` region.
const REGION: (f32, f32) = (100.0, 50.0);
/// A press on the same region, past the click slop of `REGION`.
const REGION_FAR: (f32, f32) = (140.0, 50.0);
/// A press on the button inside the region.
const BUTTON: (f32, f32) = (20.0, 50.0);
/// A press on nothing, in the native band and clear of the caption buttons.
const EMPTY_BAND: (f32, f32) = (20.0, 10.0);
/// A press on nothing, below the band.
const EMPTY_BELOW: (f32, f32) = (100.0, 160.0);

#[derive(Clone, Copy, Debug)]
enum Letter {
    Region,
    RegionFar,
    Button,
    EmptyBand,
    EmptyBelow,
    SecDownRegion,
    SecDownButton,
    SecUpRegion,
    SecUpButton,
    Clear,
    Tick,
    TakeAction,
    TakeMenu,
}

const ALPHABET: [Letter; 13] = [
    Letter::Region, Letter::RegionFar, Letter::Button, Letter::EmptyBand, Letter::EmptyBelow,
    Letter::SecDownRegion, Letter::SecDownButton, Letter::SecUpRegion, Letter::SecUpButton,
    Letter::Clear, Letter::Tick, Letter::TakeAction, Letter::TakeMenu,
];

/// Whether a press at `point` is this press's second: `press_run::follows`, transcribed.
fn follows(then: f64, was: (f32, f32), now: f64, point: (f32, f32), interval: f64) -> bool {
    now - then <= interval && (point.0 - was.0).abs() <= SLOP && (point.1 - was.1).abs() <= SLOP
}

/// `press_run::PressRun`'s last press: its element (always the titlebar's `""`), time, point and
/// count.
struct Run {
    at: f64,
    point: (f32, f32),
    count: u32,
}

/// `Titlebar` and the runtime's two entry points before `TitlebarPress`, field for field.
struct Dev {
    action: Option<TitlebarAction>,
    run: Option<Run>,
    native_band: f32,
    secondary_on_region: bool,
    window_menu: Option<WindowMenu>,
}

impl Dev {
    fn new(native_band: f32) -> Self {
        Self { action: None, run: None, native_band, secondary_on_region: false, window_menu: None }
    }

    fn press_region(&mut self, now: f64, x: f32, y: f32, interval: f64) {
        self.action = Some(if self.double(now, x, y, interval) { TitlebarAction::DoubleClick } else { TitlebarAction::Drag });
    }

    fn press_empty(&mut self, now: f64, x: f32, y: f32, interval: f64) {
        if y < self.native_band && self.double(now, x, y, interval) {
            self.action = Some(TitlebarAction::DoubleClick);
        }
    }

    fn clear(&mut self) {
        self.run = None;
        // differs from dev: a replaced stage no longer owns a secondary press either.
        self.secondary_on_region = false;
    }

    /// `PressRun::press("", ..) >= 2`, after which the next press starts a new pair.
    fn double(&mut self, now: f64, x: f32, y: f32, interval: f64) -> bool {
        let count = match &mut self.run {
            Some(last) if follows(last.at, last.point, now, (x, y), interval) => {
                (last.at, last.point) = (now, (x, y));
                last.count += 1;
                last.count
            }
            _ => {
                self.run = Some(Run { at: now, point: (x, y), count: 1 });
                1
            }
        };
        if count >= 2 { self.run = None; }
        count >= 2
    }

    fn secondary_down(&mut self, region: bool) {
        self.secondary_on_region = region;
    }

    fn secondary_up(&mut self, region: bool) {
        if std::mem::take(&mut self.secondary_on_region) && region { self.window_menu = Some(WindowMenu::Pointer); }
    }

    fn take_action(&mut self) -> Option<TitlebarAction> { self.action.take() }

    fn take_menu(&mut self) -> Option<WindowMenu> { self.window_menu.take() }

    /// The press the next one would pair with, and a secondary press that still owes its release.
    fn state(&self) -> (Option<(f64, (f32, f32))>, bool) {
        (self.run.as_ref().map(|run| (run.at, run.point)), self.secondary_on_region)
    }
}

/// The same two facts, read out of the branch's press.
fn state_of(press: titlebar::TitlebarPress) -> (Option<(f64, (f32, f32))>, bool) {
    match press {
        titlebar::TitlebarPress::Rest => (None, false),
        titlebar::TitlebarPress::Armed(pair) => (Some((pair.at, pair.point)), false),
        titlebar::TitlebarPress::SecondaryOnRegion(pair) => (pair.map(|pair| (pair.at, pair.point)), true),
    }
}

/// The runtime with a bare drag region, a button in it and space around it, and the reference.
struct Walk {
    rt: Runtime<()>,
    dev: Dev,
    /// The virtual clock both sides read, as the host's would tick on.
    now: f64,
    /// Whether the custom titlebar draws caption buttons, which the secondary press needs.
    caption: bool,
}

impl Walk {
    fn new(caption: bool) -> Self {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            col().size(200.0, 200.0)
                // The band the system drags, and room for the caption buttons a custom titlebar
                // draws over the top right.
                .child(el().key("top").size(200.0, 32.0))
                .child(crate::el::row().key("titlebar").size(200.0, 40.0).window_drag()
                    .child(el().key("button").size(40.0, 40.0).on_press(|_: &mut ()| {})))
                .child(el().key("body").size(200.0, 40.0))
        }
        let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.double_click_interval = INTERVAL;
        rt.set_native_titlebar_band(BAND);
        if caption { rt.set_caption(Some(crate::titlebar::Caption::new(BAND))); }
        rt.frame(0.0);
        Self { rt, dev: Dev::new(BAND), now: 0.0, caption }
    }

    /// Whatever the last sequence left goes, without stepping: the next starts at `Rest`.
    fn reset(&mut self) {
        self.rt.cancel_pointer();
        self.rt.titlebar.press = titlebar::TitlebarPress::Rest;
        self.rt.titlebar.action = None;
        self.rt.take_window_menu();
        self.rt.queue.retain(|_| false);
        self.rt.clock = 0.0;
        self.dev = Dev::new(BAND);
        self.now = 0.0;
    }

    /// Whether a secondary press at `point` lands on the bare region: a custom titlebar that
    /// draws caption buttons, and a point inside its drag row clear of the button.
    fn on_region(&self, point: (f32, f32)) -> bool {
        self.caption && (point == REGION || point == REGION_FAR)
    }

    /// A primary press and release at `point`; `ask` is what the reference's press asked for,
    /// after `pointer_down` dropped the request nobody took.
    fn press(&mut self, point: (f32, f32), ask: impl FnOnce(&mut Dev)) {
        self.dev.action = None;
        self.rt.pointer_down(point.0, point.1);
        self.rt.pointer_up(point.0, point.1);
        ask(&mut self.dev);
    }

    /// Plays one letter on the runtime and the reference, and compares what each left behind.
    fn play(&mut self, letter: Letter) -> Result<(), String> {
        let now = self.now;
        match letter {
            Letter::Region => self.press(REGION, |dev| dev.press_region(now, REGION.0, REGION.1, INTERVAL)),
            Letter::RegionFar => self.press(REGION_FAR, |dev| dev.press_region(now, REGION_FAR.0, REGION_FAR.1, INTERVAL)),
            Letter::Button => self.press(BUTTON, |_| {}),
            Letter::EmptyBand => self.press(EMPTY_BAND, |dev| dev.press_empty(now, EMPTY_BAND.0, EMPTY_BAND.1, INTERVAL)),
            Letter::EmptyBelow => self.press(EMPTY_BELOW, |dev| dev.press_empty(now, EMPTY_BELOW.0, EMPTY_BELOW.1, INTERVAL)),
            Letter::SecDownRegion | Letter::SecDownButton => {
                let point = if matches!(letter, Letter::SecDownRegion) { REGION } else { BUTTON };
                self.rt.secondary_down(point.0, point.1);
                let region = self.on_region(point);
                self.dev.secondary_down(region);
            }
            Letter::SecUpRegion | Letter::SecUpButton => {
                let point = if matches!(letter, Letter::SecUpRegion) { REGION } else { BUTTON };
                self.rt.secondary_up(point.0, point.1);
                let region = self.on_region(point);
                self.dev.secondary_up(region);
            }
            Letter::Clear => {
                self.rt.titlebar.clear();
                self.dev.clear();
            }
            Letter::Tick => {
                self.now += 0.5;
                self.rt.clock = self.now;
            }
            Letter::TakeAction => {
                let (ours, reference) = (self.rt.take_titlebar_action(), self.dev.take_action());
                if ours != reference { return Err(format!("take action {ours:?}, dev {reference:?}")); }
            }
            Letter::TakeMenu => {
                let (ours, reference) = (self.rt.take_window_menu(), self.dev.take_menu());
                if ours != reference { return Err(format!("take menu {ours:?}, dev {reference:?}")); }
            }
        }
        self.compare(letter)
    }

    fn compare(&self, letter: Letter) -> Result<(), String> {
        if self.rt.titlebar.action != self.dev.action {
            return Err(format!("after {letter:?}: action {:?}, dev {:?}, press {:?}", self.rt.titlebar.action,
                self.dev.action, self.rt.titlebar.press));
        }
        let (ours, reference) = (state_of(self.rt.titlebar.press), self.dev.state());
        if ours != reference { return Err(format!("after {letter:?}: press {ours:?}, dev {reference:?}")); }
        Ok(())
    }
}

#[test]
fn focus_loss_cancels_titlebar_press() {
    let mut rt = Walk::new(true).rt;
    let (x, y) = REGION;
    rt.pointer_down(x, y);
    rt.secondary_down(x, y);
    rt.window_focus(false);
    rt.window_focus(true);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag));
    rt.secondary_up(x, y);
    assert_eq!(rt.take_window_menu(), None);
    rt.pointer_down(x, y);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag));
    rt.pointer_down(x, y);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::DoubleClick));
}

/// Plays every sequence that starts with `sequence`, recording the first one that differs.
fn visit(walk: &mut Walk, sequence: &mut Vec<Letter>, ran: &mut usize, failure: &mut Option<String>) {
    if failure.is_some() { return; }
    *ran += 1;
    walk.reset();
    for &letter in sequence.iter() {
        if let Err(why) = walk.play(letter) {
            *failure = Some(format!("cap={} {sequence:?}: {why}", walk.caption));
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
fn every_short_sequence_of_titlebar_presses_leaves_what_dev_left() {
    let jobs: Vec<(bool, Letter)> = [false, true].into_iter().flat_map(|caption| ALPHABET.map(|letter| (caption, letter))).collect();
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut walks = [Walk::new(false), Walk::new(true)];
                let (mut ran, mut failure) = (0, None);
                while let Some(&(caption, letter)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    visit(&mut walks[usize::from(caption)], &mut vec![letter], &mut ran, &mut failure);
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
    assert!(total > 100_000, "only {total} sequences ran");
}
