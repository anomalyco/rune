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
//!
//! Beside geometry and opacity, each element's painted properties are followed frame by frame
//! (its colours in OKLab, corners, blurs, shadow, effect payload, image and place in the paint
//! order), and the composited window as a grid of cells a few points across: a CPU raster of
//! the painter stream, or a GPU frame through [`Run::pixels`]. `snap`, `flash` and
//! `changes-twice` find a property or a region that steps, blinks or changes twice. An input
//! ([`Run::act`]), a `Tx::instant` commit, `El::instant_changes` and `El::motion_intent` marks
//! keep a deliberate change from counting.

pub mod check;
mod gate;

pub use check::{Finding, RULES, Recording, Rule, Snap, Track, check, dump, rule, sheet, short};
pub use gate::{Allow, AllowRow, Cell, Job, Report, allowed, gate, gate_rows, report, report_rows, run};

use std::sync::Arc;

use crate::{ElementInfo, Runtime};

/// A frame's pixels, for [`Run::pixels`]: width, height and 8-bit sRGB RGBA rows.
pub type Pixels = (u32, u32, Vec<u8>);

/// A runtime being recorded: every [`step`](Run::step) frames it on the virtual clock and samples
/// its element tree, what each element paints and the composited window into `rec`.
pub struct Run<S: 'static> {
    pub rt: Runtime<S>,
    pub rec: Recording,
    /// Seconds per frame.
    pub dt: f64,
    /// Reads a drawn frame for the region checks in place of the CPU raster ([`Run::pixels`]).
    draw: Option<Box<dyn FnMut(&mut Runtime<S>) -> Option<Pixels>>>,
    /// The last frame's painter stream (hashed) and its raster, reused while it's unchanged.
    last: Option<(u64, Arc<check::raster::Grid>)>,
}

impl<S: 'static> Run<S> {
    /// Records `rt` at `hz` frames a second, after its first frame. Settle it with
    /// [`idle`](Run::idle) and set the scene up, then the first [`step`](Run::step) is the settled
    /// start the first action changes.
    pub fn new(mut rt: Runtime<S>, hz: f64) -> Self {
        rt.paint_rank = Some(Vec::new());
        rt.frame(0.0);
        let rec = Recording::new(rt.viewport, hz);
        Run { rt, rec, dt: 1.0 / hz, draw: None, last: None }
    }

    /// Reads each recorded frame's pixels with `draw` (a GPU render of the runtime) for the
    /// region checks, in place of rasterizing its painter stream on the CPU. `None` from it
    /// falls back to the raster for that frame.
    pub fn pixels(&mut self, draw: impl FnMut(&mut Runtime<S>) -> Option<Pixels> + 'static) {
        self.draw = Some(Box::new(draw));
    }

    fn grid(&mut self) -> Arc<check::raster::Grid> {
        if let Some(draw) = self.draw.as_mut()
            && let Some((w, h, rgba)) = draw(&mut self.rt)
        {
            return Arc::new(check::raster::from_pixels(&rgba, w, h, self.rec.size));
        }
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytemuck::cast_slice::<_, u8>(&self.rt.instances).hash(&mut hasher);
        let key = hasher.finish();
        if let Some((k, g)) = &self.last && *k == key { return g.clone(); }
        let g = Arc::new(check::raster::rasterize(&self.rt.instances, self.rec.size, self.rt.env.scale));
        self.last = Some((key, g.clone()));
        g
    }

    /// Steps `secs` unrecorded (setting up, before the run).
    pub fn idle(&mut self, secs: f64) {
        for _ in 0..(secs / self.dt).round() as usize {
            self.rt.frame(self.dt);
        }
    }

    /// Records one frame. A key a playground story's step pressed on it (`Play::press`) is an
    /// input there, as [`act`](Run::act)'s are.
    pub fn step(&mut self) {
        self.rt.commit_why = 0;
        let played = self.rt.played_keys();
        let awake = self.rt.frame(self.dt);
        if self.rt.played_keys() > played { self.rec.input("a story step's key"); }
        self.rec.awake.push(awake);
        let why = std::mem::take(&mut self.rt.commit_why);
        self.rec.commits(self.rt.scrolling(), why);
        let els: Vec<ElementInfo> = self.rt.elements();
        let paints = self.rt.paints();
        self.rec.sample_with(&els, Some(&paints));
        let grid = self.grid();
        self.rec.grid(Some(grid));
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
