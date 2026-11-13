//! `El::pixel_snap`. The snapping itself is in `runtime/paint/pixel_snap.rs`.
use crate::el::El;

impl El {
    /// Whether this element and its children snap their fills, borders and clips to whole
    /// device pixels when they paint. On by default: a box laid out at a fractional position
    /// (a 1.1 pt gap at 2×) still draws hard edges, and boxes that share an edge still meet
    /// with no seam. Layout, hit testing and [`Cx::layout_size`](crate::Cx::layout_size) keep
    /// the exact fractional rects either way.
    ///
    /// While an element animates, its correction eases toward the pixels for where it comes to
    /// rest, in step with the distance left, so motion stays smooth and starts and lands
    /// without a jump. Content under an `El::scale` (a zoomed canvas) draws at its exact
    /// sub-pixel edges while the scale moves and eases onto whole pixels once it holds still.
    /// Turn it off for an element an app moves itself frame by frame at sub-pixel speeds
    /// (`El::offset` from a value it animates), so it glides instead of stepping a pixel at a
    /// time.
    pub fn pixel_snap(mut self, on: bool) -> Self {
        self.vis_flags.set(crate::el::VisualFlags::NO_PIXEL_SNAP, !on);
        self
    }

    /// Snaps this element's children as Blink does inside a box with paint containment
    /// (`contain: paint`, `content` or `strict`) whose top-left is `origin`, in window points:
    /// that box's origin rounds to a whole device pixel on its own, and each child's position
    /// in it rounds again, with no carry between the two. At 1.875x a box at 44 pt (82.5 px)
    /// starts at 83, so a child 17 pt into it (31.875 px) lands on 115, not on 114 where its
    /// own position would round. Pass the containing box's origin even when this element
    /// isn't it (a panel inset inside a `main` that has the containment).
    pub fn paint_contained(mut self, origin: [f32; 2]) -> Self {
        self.vis.cold_mut().paint_contain = Some(origin);
        self
    }
}