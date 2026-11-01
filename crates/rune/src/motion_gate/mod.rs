//! A motion gate for an app's own tests: drive the real app headless on the virtual clock, record
//! every element's presented rect, visible part and opacity each frame, and list what a person
//! would see go wrong (see [`check`] and [`RULES`]).
//!
//! An app lists cells, one scenario in one circumstance each, and runs them in a test:
//!
//! ```ignore
//! use rune::motion_gate::{self, Allow, Job, Run};
//!
//! const ALLOW: &[Allow] = &[("fade-snap", "badge", "", "the badge swaps at once, by design")];
//!
//! fn add_row() -> motion_gate::Recording {
//!     let mut r = Run::new(Runtime::new(State::default(), view, (800.0, 600.0), Env::new(true, 1.0)), 120.0);
//!     r.idle(1.0);
//!     r.step(); // frame 0: the settled start
//!     r.act("tap add", |rt| Run::tap(rt, "add"));
//!     r.play(1.0);
//!     r.rec
//! }
//!
//! #[test]
//! fn motion_gate() {
//!     motion_gate::gate(vec![("add-row/wide".into(), Box::new(add_row) as Job)], ALLOW);
//! }
//! ```
//!
//! [`gate`] reads its settings from the environment, which `scripts/motion-audit/gate` sets:
//! `MOTION_GATE` keeps cells whose name contains it, `MOTION_GATE_OUT` names a file for the full
//! report, `MOTION_GATE_ALLOWED` lists allowed findings too, `MOTION_GATE_DUMP=<cell>|<path
//! part>[|from|to]` writes those tracks' frames to `MOTION_GATE_OUT`.dump, and
//! `MOTION_GATE_SHEET=<cell>|<frame>[|<path part>]` writes a contact strip of the frames around
//! `frame` to `MOTION_GATE_OUT`.sheet.png.

pub mod check;
mod gate;

pub use check::{Finding, RULES, Recording, Rule, Snap, Track, check, dump, rule, sheet, short};
pub use gate::{Allow, Cell, Job, Report, allowed, gate, report, run};

use crate::{ElementInfo, Runtime};

/// A runtime being recorded: every [`step`](Run::step) frames it on the virtual clock and samples
/// its element tree into `rec`.
pub struct Run<S: 'static> {
    pub rt: Runtime<S>,
    pub rec: Recording,
    /// Seconds per frame.
    pub dt: f64,
}

impl<S: 'static> Run<S> {
    /// Records `rt` at `hz` frames a second, after its first frame. Settle it with
    /// [`idle`](Run::idle) and set the scene up, then the first [`step`](Run::step) is the settled
    /// start the first action changes.
    pub fn new(mut rt: Runtime<S>, hz: f64) -> Self {
        rt.frame(0.0);
        let rec = Recording::new(rt.viewport, hz);
        Run { rt, rec, dt: 1.0 / hz }
    }

    /// Steps `secs` unrecorded (setting up, before the run).
    pub fn idle(&mut self, secs: f64) {
        for _ in 0..(secs / self.dt).round() as usize {
            self.rt.frame(self.dt);
        }
    }

    /// Records one frame.
    pub fn step(&mut self) {
        self.rt.commit_why = 0;
        let awake = self.rt.frame(self.dt);
        self.rec.awake.push(awake);
        let why = std::mem::take(&mut self.rt.commit_why);
        self.rec.commits(self.rt.scrolling(), why);
        let els: Vec<ElementInfo> = self.rt.elements();
        self.rec.sample(&els);
    }

    pub fn step_n(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Records `secs` of frames.
    pub fn play(&mut self, secs: f64) {
        self.step_n((secs / self.dt).round() as usize);
    }

    /// An input on the next frame: `f` runs, then the frame it lands on is recorded. Motion
    /// retargeted by an input isn't held to one path.
    pub fn act(&mut self, what: &str, f: impl FnOnce(&mut Runtime<S>)) {
        self.rec.input(what);
        f(&mut self.rt);
        self.step();
    }

    /// Marks the cell's motion as over from the next frame: frames asked for well after it are
    /// `wakes-after-end`.
    pub fn quiet(&mut self) {
        self.rec.quiet_from = Some(self.rec.frames);
    }

    /// A click at the middle of the element keyed `key`.
    pub fn tap(rt: &mut Runtime<S>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    }

    /// What the checker finds in the recording.
    pub fn finish(&self) -> Vec<Finding> {
        check(&self.rec)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod loop_tests;
