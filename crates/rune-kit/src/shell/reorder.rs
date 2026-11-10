//! Dragging a list's rows into a new order.

use crate::grip::Grip;

/// Points the pointer travels before a press on a row becomes a drag.
const SLOP: f32 = 4.0;

/// A list's drag to reorder, owned by the app: `Reorder::new().gap(4.0)`. Pass it to each
/// row's `SidebarRow::reorder`. Rows are taken to share one height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reorder {
    grip: Grip<Lift>,
    gap: f32,
}

/// A row held by a press: still a press inside the slop, then a drag that carries it from its
/// place `from` to `to`, `dy` from where it was pressed, rows `pitch` apart.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lift {
    Pressed { index: usize },
    Dragging { from: usize, to: usize, dy: f32, pitch: f32 },
}

impl Reorder {
    pub fn new() -> Reorder {
        Reorder::default()
    }
    /// Points between the rows (the list's gap), so a row steps aside by its height plus this.
    pub fn gap(mut self, points: f32) -> Self {
        self.gap = points.max(0.0);
        self
    }
    /// The row being dragged, by its index before the drag.
    pub fn dragging(&self) -> Option<usize> {
        match self.grip.get() {
            Some(Lift::Dragging { from, .. }) => Some(*from),
            _ => None,
        }
    }
    /// Where the dragged row would land if dropped now.
    pub fn target(&self) -> Option<usize> {
        match self.grip.get() {
            Some(Lift::Dragging { to, .. }) => Some(*to),
            _ => None,
        }
    }

    /// Whether a press on a row holds the list (pressed or dragging).
    pub(super) fn held(&self) -> bool {
        self.grip.is_held()
    }

    /// The primary button went down on row `index`, at `at` in window points. A drag whose
    /// release was lost ends here, moving nothing.
    pub(super) fn press(&mut self, index: usize, at: [f32; 2]) {
        self.grip.press(Lift::Pressed { index }, Some(at));
    }

    /// The row's own drag moved to `at` in window points, `dy` from the press on row `index`.
    pub(super) fn drag(&mut self, index: usize, count: usize, at: [f32; 2], dy: f32, h: f32) {
        self.grip.moved(at);
        self.follow(index, count, dy, h);
    }

    /// The pointer moved to `at`, seen from any row. Returns whether that showed the drag's
    /// release was lost, so the list let go of its row.
    pub(super) fn pointer(&mut self, at: [f32; 2]) -> bool {
        self.grip.pointer(at).is_some()
    }

    /// The pointer moved `dy` since the press on row `index`, which is `h` tall.
    pub(super) fn follow(&mut self, index: usize, count: usize, dy: f32, h: f32) {
        let (from, pitch) = match self.grip.get() {
            Some(&Lift::Dragging { from, pitch, .. }) => (from, pitch),
            _ if dy.abs() < SLOP => return,
            Some(&Lift::Pressed { index }) => (index, (h + self.gap).max(1.0)),
            None => (index, (h + self.gap).max(1.0)),
        };
        let to = (from as f32 + (dy / pitch).round()).clamp(0.0, count.saturating_sub(1) as f32) as usize;
        self.grip.hold(Lift::Dragging { from, to, dy, pitch });
    }

    /// The drag ended: `(from, to)` for a drop that moves a row, `None` for a cancel or a drop
    /// in place.
    pub(super) fn end(&mut self, cancelled: bool) -> Option<(usize, usize)> {
        match self.grip.release()? {
            Lift::Dragging { from, to, .. } if !cancelled && from != to => Some((from, to)),
            _ => None,
        }
    }

    /// How far row `index` sits from its place: the dragged row with the pointer, and the rows
    /// between its place and its target a step toward the gap it left.
    pub(super) fn shift(&self, index: usize) -> f32 {
        let Some(&Lift::Dragging { from, to, dy, pitch }) = self.grip.get() else { return 0.0 };
        if index == from {
            dy
        } else if from < index && index <= to {
            -pitch
        } else if to <= index && index < from {
            pitch
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_between_the_place_and_the_target_step_aside() {
        let mut r = Reorder::new().gap(4.0);
        r.follow(1, 5, 3.0, 45.0);
        assert_eq!(r.dragging(), None, "inside the slop it's still a press");
        r.follow(1, 5, 100.0, 45.0);
        assert_eq!((r.dragging(), r.target()), (Some(1), Some(3)));
        let shifts: Vec<f32> = (0..5).map(|i| r.shift(i)).collect();
        assert_eq!(shifts, [0.0, 100.0, -49.0, -49.0, 0.0]);
        r.follow(1, 5, -500.0, 45.0);
        assert_eq!(r.target(), Some(0), "clamped to the list");
        assert_eq!(r.shift(0), 49.0);
        assert_eq!(r.end(false), Some((1, 0)));
        assert_eq!(r.shift(0), 0.0);
        r.follow(2, 5, 60.0, 45.0);
        assert_eq!(r.end(true), None, "a cancel moves nothing");
    }

    #[test]
    fn a_press_after_a_lost_release_drags_its_own_row() {
        let mut r = Reorder::new();
        r.press(1, [0.0, 0.0]);
        r.drag(1, 5, [0.0, 100.0], 100.0, 45.0);
        assert_eq!(r.dragging(), Some(1));
        // Row 1's release never came. Row 3 is pressed and dragged down a row.
        r.press(3, [0.0, 200.0]);
        assert_eq!(r.dragging(), None, "the stale drag let go");
        r.drag(3, 5, [0.0, 250.0], 50.0, 45.0);
        assert_eq!(r.end(false), Some((3, 4)));
    }

    #[test]
    fn moves_the_drag_never_hears_put_the_row_back() {
        let mut r = Reorder::new();
        r.press(1, [0.0, 0.0]);
        r.drag(1, 5, [0.0, 100.0], 100.0, 45.0);
        assert!(!r.pointer([0.0, 100.0]), "the drag heard it");
        assert!(!r.pointer([0.0, 110.0]), "one it hasn't yet");
        assert!(r.pointer([0.0, 120.0]), "a second: the release was lost");
        assert_eq!((r.dragging(), r.shift(2), r.end(false)), (None, 0.0, None));
    }
}
