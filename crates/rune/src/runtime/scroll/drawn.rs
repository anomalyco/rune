//! Where a scroll viewport drew its content last frame, and the hold that keeps a pinned end's
//! pixel through a sub-pixel reversal.
//!
//! As a pinned growth stack's rows and slack settle, its offset can creep back and forth by a
//! fraction of a rounding quantum, which the drawn offset would otherwise follow between two
//! values and re-rasterize the rows under it. A step straight back by less than one quantum
//! holds the pixel it drew; a step one way rounds as ever.
use super::*;

/// Where a scroll viewport drew its content's vertical offset last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum PixelHold {
    /// Nothing drawn yet.
    #[default]
    Unseen,
    /// Drawn at `drawn`, stepping `dir` (-1, 0 or 1), with `pinned` the mode it stepped in.
    Stepping { drawn: f32, dir: i8, pinned: bool },
    /// Held at `drawn` through a reversal at a pinned end, last stepping `dir`.
    Holding { drawn: f32, dir: i8 },
}

impl PixelHold {
    /// The vertical offset the last frame drew at, in points.
    pub(super) fn y(self) -> f32 {
        match self {
            PixelHold::Unseen => 0.0,
            PixelHold::Stepping { drawn, .. } | PixelHold::Holding { drawn, .. } => drawn,
        }
    }

    /// The offset to draw this frame's `exact` content offset at, and the hold after it.
    /// `offset` is the viewport's layout offset, which a held step never draws away from.
    pub(super) fn draw(self, exact: f32, offset: f32, pinned: bool, s: f32) -> (f32, PixelHold) {
        // `grid` rounds to 1/64 of a device pixel, so a hold spanning under one quantum only
        // absorbs rounding noise. Real travel always lands at the layout offset.
        let rounded = grid(exact, s);
        let (drawn, dir, stepped_pinned) = match self {
            PixelHold::Unseen => (0.0, 0, false),
            PixelHold::Stepping { drawn, dir, pinned } => (drawn, dir, pinned),
            PixelHold::Holding { drawn, dir } => (drawn, dir, true),
        };
        let step = (rounded - drawn).signum() as i8;
        // Only a step drawn at a pinned end may be reversed into a hold. A direction a gesture
        // or glide left behind is not the settle's, and holding it would swallow the first real
        // step after the end pins.
        let back = pinned
            && stepped_pinned
            && rounded != grid(offset, s)
            && step == -dir
            && ((exact - drawn) * s).abs() < 1.0 / 64.0;
        if back {
            return (drawn, PixelHold::Holding { drawn, dir });
        }
        let dir = if rounded == drawn { dir } else { step };
        // A frame that drew no step keeps the mode that last stepped: pinning while the end
        // holds still doesn't make the gesture's direction the settle's.
        let stepped_pinned = if rounded == drawn { stepped_pinned } else { pinned };
        (rounded, PixelHold::Stepping { drawn: rounded, dir, pinned: stepped_pinned })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drawn at 100 pt at 2× (`grid` is 1/128 pt there), last stepping `dir` at a pinned end.
    fn drawn(dir: i8) -> PixelHold {
        PixelHold::Stepping { drawn: 100.0, dir, pinned: true }
    }

    #[test]
    fn a_pinned_end_holds_its_pixel_through_a_reversal() {
        // The content creeps back by 0.012 device pixels, across a rounding boundary.
        let (at, next) = drawn(-1).draw(100.006, 100.0, true, 2.0);
        assert_eq!(at, 100.0, "held where it drew");
        assert_eq!(next, PixelHold::Holding { drawn: 100.0, dir: -1 });
    }

    #[test]
    fn a_direction_from_another_mode_does_not_hold() {
        // A gesture or glide drew the last step, then the end pinned. Its direction isn't the
        // settle's: the first step back is real motion and draws.
        let carried = PixelHold::Stepping { drawn: 100.0, dir: -1, pinned: false };
        let (at, next) = carried.draw(100.006, 100.0, true, 2.0);
        assert_eq!(at, grid(100.006, 2.0), "the step drew, not held");
        assert_eq!(next, PixelHold::Stepping { drawn: grid(100.006, 2.0), dir: 1, pinned: true });
    }

    #[test]
    fn a_stationary_pinned_frame_keeps_the_mode_that_stepped() {
        // The end pins and holds still for a frame before the content moves. That frame drew no
        // step, so it can't relabel the gesture's direction as the settle's.
        let carried = PixelHold::Stepping { drawn: 100.0, dir: -1, pinned: false };
        let (still, next) = carried.draw(100.0, 100.0, true, 2.0);
        assert_eq!(still, 100.0);
        assert_eq!(next, PixelHold::Stepping { drawn: 100.0, dir: -1, pinned: false });
        let (at, next) = next.draw(100.006, 100.0, true, 2.0);
        assert_eq!(at, grid(100.006, 2.0), "the first real step drew, not held");
        assert_eq!(next, PixelHold::Stepping { drawn: grid(100.006, 2.0), dir: 1, pinned: true });
    }

    #[test]
    fn motion_one_way_rounds_as_before() {
        let (at, next) = drawn(1).draw(100.013, 100.0, true, 2.0);
        assert_eq!(at, grid(100.013, 2.0));
        assert_eq!(next, PixelHold::Stepping { drawn: grid(100.013, 2.0), dir: 1, pinned: true });
    }

    #[test]
    fn the_layout_offset_always_lands() {
        // No slack or stuck offset: the exact offset is the layout offset, so it draws there.
        let (at, _) = drawn(-1).draw(100.006, 100.006, true, 2.0);
        assert_eq!(at, grid(100.006, 2.0));
    }

    #[test]
    fn a_held_pixel_let_go_lands_where_the_content_is() {
        let held = PixelHold::Holding { drawn: 100.0, dir: 1 };
        let (at, next) = held.draw(100.02, 100.0, true, 2.0);
        assert_eq!(at, grid(100.02, 2.0), "past one quantum, it steps");
        assert_eq!(next, PixelHold::Stepping { drawn: grid(100.02, 2.0), dir: 1, pinned: true });
    }
}
