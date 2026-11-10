//! Counting a run of presses, as AppKit's `clickCount` does: one rule for an element's
//! [`Drag::clicks`], a text field's and selectable text's word and line clicks, the titlebar's
//! double click and the first click an `on_double_press` pairs with.
use super::*;

/// `b` is within the click slop of `a` on each axis.
pub(super) fn within_slop(a: (f32, f32), b: (f32, f32)) -> bool {
    (b.0 - a.0).abs() <= CLICK_SLOP && (b.1 - a.1).abs() <= CLICK_SLOP
}

/// A press at `point` at `now` (clock seconds) carries on a run whose last press went down at
/// `then` at `was`: within `interval` of it and within the click slop.
pub(super) fn follows(then: f64, was: (f32, f32), now: f64, point: (f32, f32), interval: f64) -> bool {
    now - then <= interval && within_slop(was, point)
}

/// The run's last press: its element, time (clock seconds), window point and count.
struct Last {
    path: String,
    at: f64,
    point: (f32, f32),
    count: u32,
}

#[derive(Default)]
pub(super) struct PressRun {
    last: Option<Last>,
}

impl PressRun {
    /// Counts a press on `path` at `now`: one more than the last press when that was on the same
    /// element and the press [`follows`] it, else 1.
    pub(super) fn press(&mut self, path: &str, now: f64, point: (f32, f32), interval: f64) -> u32 {
        match &mut self.last {
            Some(last) if last.path == path && follows(last.at, last.point, now, point, interval) => {
                (last.at, last.point) = (now, point);
                last.count += 1;
                last.count
            }
            _ => {
                self.last = Some(Last { path: path.to_owned(), at: now, point, count: 1 });
                1
            }
        }
    }

    /// A press that dragged, another button's press, a press elsewhere or a reset ends the run.
    pub(super) fn clear(&mut self) { self.last = None; }

    /// Ends the run unless its last press was on `path`.
    pub(super) fn keep_on(&mut self, path: Option<&str>) {
        if self.last.as_ref().is_some_and(|l| Some(l.path.as_str()) != path) { self.last = None; }
    }
}

#[cfg(test)]
#[path = "press_run_tests.rs"]
mod tests;
