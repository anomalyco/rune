//! Dragging a list's rows into a new order.

use crate::grip::Grip;

/// Points the pointer travels before a press on a row becomes a drag.
const SLOP: f32 = 4.0;

/// How a dragged row's pointer offset chooses its landing slot.
///
/// The default is [`ReorderLanding::Nearest`]. Pass [`ReorderLanding::HalfRow`] to
/// [`Reorder::landing`] for desktop sidebar semantics (GTK, Finder): dragging from a row's middle
/// lands before or after each neighbour according to which half the pointer is in, and into the
/// gap between rows.
///
/// ```
/// use rune_kit::shell::{Reorder, ReorderLanding};
///
/// let reorder = Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow);
/// assert_eq!(reorder.dragging(), None);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReorderLanding {
    /// Round `dy / pitch` to the nearest slot (default).
    #[default]
    Nearest,
    /// Truncate `dy / pitch` toward zero, stepping once the pointer crosses each neighbour's
    /// middle from the dragged row's middle.
    HalfRow,
}

/// A list's drag to reorder, owned by the app: `Reorder::new().gap(4.0)`. Pass it to each
/// row's `SidebarRow::reorder` or `SidebarRow::reorder_tx`. Rows are taken to share one height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reorder {
    grip: Grip<Lift>,
    gap: f32,
    landing: ReorderLanding,
    cancel_x: Option<f32>,
}

/// A row held by a press: still a press inside the slop, then a drag that carries it from its
/// place `from` to `to` (`None` when outside [`Reorder::cancel_outside_x`] or in an empty list),
/// `dy` from where it was pressed, rows `pitch` apart.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lift {
    Pressed { index: usize },
    Dragging { from: usize, to: Option<usize>, dy: f32, pitch: f32 },
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
    /// Sets how the dragged row's vertical offset chooses its landing slot (default
    /// [`ReorderLanding::Nearest`]).
    ///
    /// ```
    /// use rune_kit::shell::{Reorder, ReorderLanding};
    ///
    /// let reorder = Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow);
    /// assert_eq!(reorder.target(), None);
    /// ```
    pub fn landing(mut self, landing: ReorderLanding) -> Self {
        self.landing = landing;
        self
    }
    /// Cancels the reorder while the pointer is farther than `max_dx` points horizontally from
    /// where the press began (`dx.abs() > max_dx`). While outside the margin, [`Self::target`]
    /// returns `None`, neighbouring rows' [`Self::shift`] returns to `0.0`, and releasing cancels
    /// the drop instead of clamping to an end of the list. Negative values clamp to `0.0`.
    /// By default there is no horizontal cancel margin.
    ///
    /// ```
    /// use rune_kit::shell::Reorder;
    ///
    /// let reorder = Reorder::new().gap(2.0).cancel_outside_x(80.0);
    /// assert_eq!(reorder.target(), None);
    /// ```
    pub fn cancel_outside_x(mut self, max_dx: f32) -> Self {
        self.cancel_x = Some(max_dx.max(0.0));
        self
    }
    /// The row being dragged, by its index before the drag.
    pub fn dragging(&self) -> Option<usize> {
        match self.grip.get() {
            Some(Lift::Dragging { from, .. }) => Some(*from),
            _ => None,
        }
    }
    /// Where the dragged row would land if dropped now, or `None` when not dragging or while
    /// outside [`Self::cancel_outside_x`].
    pub fn target(&self) -> Option<usize> {
        match self.grip.get() {
            Some(Lift::Dragging { to, .. }) => *to,
            _ => None,
        }
    }
    /// The active drag's vertical offset (`dy`) from where the press began, in points, or `None`
    /// when idle or still inside the press slop.
    ///
    /// ```
    /// use rune_kit::shell::Reorder;
    ///
    /// let reorder = Reorder::new();
    /// assert_eq!(reorder.offset(), None);
    /// ```
    pub fn offset(&self) -> Option<f32> {
        match self.grip.get() {
            Some(Lift::Dragging { dy, .. }) => Some(*dy),
            _ => None,
        }
    }
    /// The active drag's row pitch (row height plus gap, at least 1 point), or `None` when idle
    /// or still inside the press slop.
    ///
    /// ```
    /// use rune_kit::shell::Reorder;
    ///
    /// let reorder = Reorder::new();
    /// assert_eq!(reorder.pitch(), None);
    /// ```
    pub fn pitch(&self) -> Option<f32> {
        match self.grip.get() {
            Some(Lift::Dragging { pitch, .. }) => Some(*pitch),
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

    /// The row's own drag moved to `at` in window points, `(dx, dy)` from the press on row
    /// `index`.
    pub(super) fn drag(&mut self, index: usize, count: usize, at: [f32; 2], dx: f32, dy: f32, h: f32) {
        self.grip.moved(at);
        self.follow(index, count, dx, dy, h);
    }

    /// The pointer moved to `at`, seen from any row. Returns whether that showed the drag's
    /// release was lost, so the list let go of its row.
    pub(super) fn pointer(&mut self, at: [f32; 2]) -> bool {
        self.grip.pointer(at).is_some()
    }

    /// The pointer moved `(dx, dy)` since the press on row `index`, which is `h` tall.
    pub(super) fn follow(&mut self, index: usize, count: usize, dx: f32, dy: f32, h: f32) {
        if !dy.is_finite() {
            return;
        }
        let (from, pitch) = match self.grip.get() {
            Some(&Lift::Dragging { from, pitch, .. }) => (from, pitch),
            _ if dy.abs() < SLOP => return,
            Some(&Lift::Pressed { index }) => (index, (h + self.gap).max(1.0)),
            None => (index, (h + self.gap).max(1.0)),
        };
        let outside = self.cancel_x.is_some_and(|max| dx.abs() > max);
        let to = if outside || count == 0 {
            None
        } else {
            let steps = match self.landing {
                ReorderLanding::Nearest => (dy / pitch).round(),
                ReorderLanding::HalfRow => (dy / pitch).trunc(),
            };
            Some((from as f32 + steps).clamp(0.0, (count - 1) as f32) as usize)
        };
        self.grip.hold(Lift::Dragging { from, to, dy, pitch });
    }

    /// The drag ended: `(from, to)` for a drop that moves a row, `None` for a cancel or a drop
    /// in place.
    pub(super) fn end(&mut self, cancelled: bool) -> Option<(usize, usize)> {
        match self.grip.release()? {
            Lift::Dragging { from, to: Some(to), .. } if !cancelled && from != to => Some((from, to)),
            _ => None,
        }
    }

    /// How far row `index` sits from its resting place, in points: the dragged row follows the
    /// pointer (`dy`), rows between its starting slot and [`Self::target`] step aside by one pitch
    /// toward the gap it left, and all other rows sit at `0.0` (including every neighbour while
    /// outside [`Self::cancel_outside_x`]).
    ///
    /// ```
    /// use rune_kit::shell::Reorder;
    ///
    /// let reorder = Reorder::new();
    /// assert_eq!(reorder.shift(0), 0.0);
    /// ```
    pub fn shift(&self, index: usize) -> f32 {
        let Some(&Lift::Dragging { from, to, dy, pitch }) = self.grip.get() else { return 0.0 };
        if index == from {
            dy
        } else if let Some(to) = to {
            if from < index && index <= to {
                -pitch
            } else if to <= index && index < from {
                pitch
            } else {
                0.0
            }
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
        r.follow(1, 5, 0.0, 3.0, 45.0);
        assert_eq!(r.dragging(), None, "inside the slop it's still a press");
        r.follow(1, 5, 0.0, 100.0, 45.0);
        assert_eq!((r.dragging(), r.target()), (Some(1), Some(3)));
        let shifts: Vec<f32> = (0..5).map(|i| r.shift(i)).collect();
        assert_eq!(shifts, [0.0, 100.0, -49.0, -49.0, 0.0]);
        r.follow(1, 5, 0.0, -500.0, 45.0);
        assert_eq!(r.target(), Some(0), "clamped to the list");
        assert_eq!(r.shift(0), 49.0);
        assert_eq!(r.end(false), Some((1, 0)));
        assert_eq!(r.shift(0), 0.0);
        r.follow(2, 5, 0.0, 60.0, 45.0);
        assert_eq!(r.end(true), None, "a cancel moves nothing");
    }

    #[test]
    fn a_press_after_a_lost_release_drags_its_own_row() {
        let mut r = Reorder::new();
        r.press(1, [0.0, 0.0]);
        r.drag(1, 5, [0.0, 100.0], 0.0, 100.0, 45.0);
        assert_eq!(r.dragging(), Some(1));
        // Row 1's release never came. Row 3 is pressed and dragged down a row.
        r.press(3, [0.0, 200.0]);
        assert_eq!(r.dragging(), None, "the stale drag let go");
        r.drag(3, 5, [0.0, 250.0], 0.0, 50.0, 45.0);
        assert_eq!(r.end(false), Some((3, 4)));
    }

    #[test]
    fn moves_the_drag_never_hears_put_the_row_back() {
        let mut r = Reorder::new();
        r.press(1, [0.0, 0.0]);
        r.drag(1, 5, [0.0, 100.0], 0.0, 100.0, 45.0);
        assert!(!r.pointer([0.0, 100.0]), "the drag heard it");
        assert!(!r.pointer([0.0, 110.0]), "one it hasn't yet");
        assert!(r.pointer([0.0, 120.0]), "a second: the release was lost");
        assert_eq!((r.dragging(), r.shift(2), r.end(false)), (None, 0.0, None));
    }

    #[test]
    fn nearest_and_half_row_landing_rules() {
        // Issue #1396 repro: 3 rows, 24 pt tall + 2 pt gap (26 pt pitch), dragged 39 pt down
        // (1.5 pitches, into the gap below row 1).
        let mut nearest = Reorder::new().gap(2.0);
        nearest.follow(0, 3, 0.0, 39.0, 24.0);
        assert_eq!(nearest.target(), Some(2));
        assert_eq!(nearest.end(false), Some((0, 2)));

        let mut half = Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow);
        half.follow(0, 3, 0.0, 25.0, 24.0);
        assert_eq!(half.target(), Some(0), "upper half of row 1 stays at 0");
        half.follow(0, 3, 0.0, 26.0, 24.0);
        assert_eq!(half.target(), Some(1), "crossing row 1 middle lands at 1");
        half.follow(0, 3, 0.0, 39.0, 24.0);
        assert_eq!(half.target(), Some(1), "gap below row 1 stays at 1");
        half.follow(0, 3, 0.0, 51.0, 24.0);
        assert_eq!(half.target(), Some(1), "upper half of row 2 stays at 1");
        half.follow(0, 3, 0.0, 52.0, 24.0);
        assert_eq!(half.target(), Some(2), "crossing row 2 middle lands at 2");
        half.follow(0, 3, 0.0, 39.0, 24.0);
        assert_eq!(half.end(false), Some((0, 1)));

        // Dragging upward mirrors the rule toward zero.
        let mut up_nearest = Reorder::new().gap(2.0);
        up_nearest.follow(2, 3, 0.0, -39.0, 24.0);
        assert_eq!(up_nearest.end(false), Some((2, 0)));

        let mut up_half = Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow);
        up_half.follow(2, 3, 0.0, -25.0, 24.0);
        assert_eq!(up_half.target(), Some(2));
        up_half.follow(2, 3, 0.0, -39.0, 24.0);
        assert_eq!(up_half.target(), Some(1));
        assert_eq!(up_half.end(false), Some((2, 1)));
    }

    #[test]
    fn cancel_outside_x_clears_target_resets_neighbors_and_cancels_drop() {
        // Default has no horizontal cancel margin: large dx still lands.
        let mut default_r = Reorder::new().gap(2.0);
        default_r.follow(0, 3, 500.0, 39.0, 24.0);
        assert_eq!(default_r.target(), Some(2));
        assert_eq!(default_r.end(false), Some((0, 2)));

        let mut r = Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow).cancel_outside_x(80.0);
        // Exact boundary (both sides) stays inside.
        r.follow(0, 3, 80.0, 39.0, 24.0);
        assert_eq!((r.dragging(), r.target()), (Some(0), Some(1)));
        assert_eq!((r.shift(0), r.shift(1), r.shift(2)), (39.0, -26.0, 0.0));
        r.follow(0, 3, -80.0, 39.0, 24.0);
        assert_eq!(r.target(), Some(1));

        // Past the margin (right and left): target is None, neighbours return to 0, dragged row keeps dy.
        r.follow(0, 3, 81.0, 39.0, 24.0);
        assert_eq!((r.dragging(), r.target(), r.offset(), r.pitch()), (Some(0), None, Some(39.0), Some(26.0)));
        assert_eq!((r.shift(0), r.shift(1), r.shift(2)), (39.0, 0.0, 0.0));

        r.follow(0, 3, -81.0, 39.0, 24.0);
        assert_eq!(r.target(), None);
        assert_eq!((r.shift(0), r.shift(1), r.shift(2)), (39.0, 0.0, 0.0));

        // Returning inside restores target and neighbour shifts.
        r.follow(0, 3, 20.0, 39.0, 24.0);
        assert_eq!(r.target(), Some(1));
        assert_eq!((r.shift(0), r.shift(1), r.shift(2)), (39.0, -26.0, 0.0));

        // Releasing outside cancels the drop and resets state.
        r.follow(0, 3, 120.0, 39.0, 24.0);
        assert_eq!(r.end(false), None);
        assert_eq!((r.dragging(), r.target(), r.offset(), r.pitch()), (None, None, None, None));
        assert_eq!((r.shift(0), r.shift(1), r.shift(2)), (0.0, 0.0, 0.0));
    }

    #[test]
    fn offset_pitch_and_shift_getters_track_active_drag() {
        let mut r = Reorder::new().gap(2.0);
        assert_eq!((r.offset(), r.pitch(), r.shift(0)), (None, None, 0.0));

        r.press(0, [10.0, 12.0]);
        assert_eq!((r.offset(), r.pitch(), r.shift(0)), (None, None, 0.0), "pressed inside slop");

        r.drag(0, 3, [10.0, 15.0], 0.0, 3.0, 24.0);
        assert_eq!((r.offset(), r.pitch()), (None, None), "still inside slop");

        r.drag(0, 3, [10.0, 51.0], 0.0, 39.0, 24.0);
        assert_eq!((r.offset(), r.pitch()), (Some(39.0), Some(26.0)));
        assert_eq!((r.shift(0), r.shift(1), r.shift(2)), (39.0, -26.0, -26.0));

        assert_eq!(r.end(false), Some((0, 2)));
        assert_eq!((r.offset(), r.pitch(), r.shift(0)), (None, None, 0.0));
    }

    #[test]
    fn edges_empty_single_zero_height_negative_cancel_and_huge_dy() {
        let mut empty = Reorder::new().gap(2.0);
        empty.follow(0, 0, 0.0, 30.0, 24.0);
        assert_eq!((empty.dragging(), empty.target(), empty.end(false)), (Some(0), None, None));

        let mut single = Reorder::new().gap(2.0);
        single.follow(0, 1, 0.0, 30.0, 24.0);
        assert_eq!((single.dragging(), single.target(), single.end(false)), (Some(0), Some(0), None));

        let mut zero_h = Reorder::new().gap(0.0);
        zero_h.follow(0, 3, 0.0, 10.0, 0.0);
        assert_eq!(zero_h.pitch(), Some(1.0));
        assert_eq!(zero_h.target(), Some(2));

        let mut neg_cancel = Reorder::new().cancel_outside_x(-10.0);
        neg_cancel.follow(0, 3, 0.0, 30.0, 24.0);
        assert_eq!(neg_cancel.target(), Some(1), "negative max_dx clamps to 0 so vertical drags stay valid");
        neg_cancel.follow(0, 3, 1.0, 30.0, 24.0);
        assert_eq!(neg_cancel.target(), None);

        let mut huge = Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow);
        huge.follow(1, 1000, 0.0, f32::NAN, 24.0);
        assert_eq!(huge.dragging(), None, "non-finite dy is ignored");
        huge.follow(1, 1000, 0.0, 1e9, 24.0);
        assert_eq!(huge.target(), Some(999));
        huge.follow(1, 1000, 0.0, -1e9, 24.0);
        assert_eq!(huge.target(), Some(0));
    }
}
