//! One line-selection gesture on a code or diff view, for every view that selects lines: a
//! press on a line's number (Pierre's own gesture, `@pierre/diffs` `InteractionManager`:
//! `selecting`, `pendingSingleLineUnselect`, Shift extending from the end it keeps) or on its
//! code (Desktop's `session-ui/src/pierre/selection-machine.ts`, the `text` column: a click
//! toggles the line, a drag selects the lines it crosses).
//!
//! The view keeps a [`LineSelection`] and feeds it [`LineEvent`]s from its pointer handlers;
//! [`LineSelection::step`] returns [`LineEffect::Ended`] when a gesture leaves a selection the
//! app should hear about.

use rune::machine::Effects;

/// A point a selection runs from or to: a line of a file, or of one side of a diff.
pub trait LinePoint: Copy + PartialEq {
    /// Whether one selection may run from `self` to `other`: the same file (a diff's two sides
    /// may mix).
    fn joins(&self, other: &Self) -> bool;
}

/// A line of the one file a code view shows, 1-based.
impl LinePoint for u32 {
    fn joins(&self, _: &u32) -> bool {
        true
    }
}

/// Selected lines, from where the gesture started to where it ends, either way round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lines<P> {
    pub start: P,
    pub end: P,
}

impl<P: Copy> Lines<P> {
    /// One line.
    pub fn line(at: P) -> Lines<P> {
        Lines { start: at, end: at }
    }
}

/// Where a press landed: the line numbers, or the code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Numbers,
    Text,
}

/// The selection and the gesture making it. `before` is the selection the press found, which a
/// cancelled gesture puts back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineSelection<P> {
    /// No press held; `selected` is what the view shows.
    Idle { selected: Option<Lines<P>> },
    /// A press on the one selected line's number: releasing clears it, and moving to another
    /// line selects from it (Pierre's `pendingSingleLineUnselect`).
    Unselecting { line: Lines<P> },
    /// A press on the numbers selecting from `selected.start` to the line under the pointer
    /// (Pierre's `selecting`).
    Selecting { selected: Lines<P>, before: Option<Lines<P>> },
    /// A press on the code that hasn't left its line: releasing toggles that line
    /// (`selection-machine.ts` `pressing`, `text`).
    Pressing { origin: P, before: Option<Lines<P>> },
    /// A press on the code dragged to another line: the lines between its ends are selected
    /// (`dragging`, `text`).
    Dragging { selected: Lines<P>, before: Option<Lines<P>> },
}

impl<P> Default for LineSelection<P> {
    fn default() -> LineSelection<P> {
        LineSelection::Idle { selected: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEvent<P> {
    /// The primary button went down on line `at`, with Shift held or not.
    Press { at: P, column: Column, shift: bool },
    /// The held pointer is over line `at` (a row without a line, such as a fold, sends none).
    Move { at: P },
    /// The button came up.
    Release,
    /// The press ended without a release (the pointer capture was lost).
    Cancel,
    /// The app set the selection (`selection.controlled`); a held press ends.
    Set(Option<Lines<P>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEffect<P> {
    /// A gesture ended with this selection (`notify-selection-end`).
    Ended(Option<Lines<P>>),
}

impl<P: LinePoint> LineSelection<P> {
    /// The lines the view shows selected.
    pub fn selected(&self) -> Option<Lines<P>> {
        match *self {
            LineSelection::Idle { selected } => selected,
            LineSelection::Unselecting { line } => Some(line),
            LineSelection::Selecting { selected, .. } | LineSelection::Dragging { selected, .. } => Some(selected),
            LineSelection::Pressing { before, .. } => before,
        }
    }

    /// Whether a press is held (pointer moves can change the selection).
    pub fn tracking(&self) -> bool {
        !matches!(self, LineSelection::Idle { .. })
    }

    /// Steps the gesture. `row` gives a line's row in the view (`None` when it has none), which
    /// Shift uses to pick the end of the selection it keeps.
    pub fn step(&mut self, event: LineEvent<P>, row: impl Fn(&P) -> Option<u32>) -> Effects<LineEffect<P>> {
        match event {
            LineEvent::Set(selected) => {
                *self = LineSelection::Idle { selected };
                Effects::none()
            }
            LineEvent::Press { at, column, shift } => {
                let current = self.selected();
                *self = match column {
                    Column::Numbers => press_number(current, at, shift, row),
                    Column::Text => LineSelection::Pressing { origin: at, before: current },
                };
                Effects::none()
            }
            LineEvent::Move { at } => {
                *self = match *self {
                    LineSelection::Unselecting { line } if line.start != at && line.start.joins(&at) => {
                        LineSelection::Selecting { selected: Lines { start: line.start, end: at }, before: Some(line) }
                    }
                    LineSelection::Selecting { selected, before } if selected.start.joins(&at) => {
                        LineSelection::Selecting { selected: Lines { end: at, ..selected }, before }
                    }
                    LineSelection::Pressing { origin, before } if origin != at && origin.joins(&at) => {
                        LineSelection::Dragging { selected: Lines { start: origin, end: at }, before }
                    }
                    LineSelection::Dragging { selected, before } if selected.start.joins(&at) => {
                        LineSelection::Dragging { selected: Lines { end: at, ..selected }, before }
                    }
                    ref same => same.clone(),
                };
                Effects::none()
            }
            LineEvent::Release => {
                let selected = match *self {
                    LineSelection::Idle { .. } => return Effects::none(),
                    LineSelection::Unselecting { .. } => None,
                    LineSelection::Selecting { selected, .. } | LineSelection::Dragging { selected, .. } => Some(selected),
                    // A click on the code selects its line, or clears it when it was the selection.
                    LineSelection::Pressing { origin, before } => {
                        let click = Lines::line(origin);
                        (before != Some(click)).then_some(click)
                    }
                };
                *self = LineSelection::Idle { selected };
                Effects::one(LineEffect::Ended(selected))
            }
            LineEvent::Cancel => {
                let before = match *self {
                    LineSelection::Idle { .. } => return Effects::none(),
                    LineSelection::Unselecting { line } => Some(line),
                    LineSelection::Selecting { before, .. } | LineSelection::Pressing { before, .. } | LineSelection::Dragging { before, .. } => {
                        before
                    }
                };
                *self = LineSelection::Idle { selected: before };
                Effects::none()
            }
        }
    }
}

/// A press on line `at`'s number. With Shift and a selection in the same file, the selection
/// runs from the end of it the press leaves in place: its start, unless the press is above the
/// selection's top (Pierre's `startLineSelectionFromPointerDown`); a selection whose rows aren't
/// drawn ignores the press, as Pierre does. Pressing the one selected line waits to see whether
/// the press drags.
fn press_number<P: LinePoint>(current: Option<Lines<P>>, at: P, shift: bool, row: impl Fn(&P) -> Option<u32>) -> LineSelection<P> {
    match current {
        Some(s) if shift && s.start.joins(&at) => {
            let (Some(start), Some(end), Some(here)) = (row(&s.start), row(&s.end), row(&at)) else {
                return LineSelection::Idle { selected: current };
            };
            let keep_start = if start <= end { here >= start } else { here <= end };
            let anchor = if keep_start { s.start } else { s.end };
            LineSelection::Selecting { selected: Lines { start: anchor, end: at }, before: current }
        }
        Some(s) if s == Lines::line(at) => LineSelection::Unselecting { line: s },
        _ => LineSelection::Selecting { selected: Lines::line(at), before: current },
    }
}
