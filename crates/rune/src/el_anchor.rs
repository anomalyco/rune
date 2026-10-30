//! Anchored overlays: `El::anchored`, `El::anchored_fit` and `El::above`. Where they go is
//! worked out in `runtime/anchor.rs`.
use taffy::prelude::*;

use crate::el::{Align, El};

/// Which side of its anchor an [`El::anchored`] overlay sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub key: String,
    pub side: Side,
    pub align: Align,
    pub gap: f32,
    /// [`El::anchored_fit`]: at most as long as the room on the side it goes to.
    pub fit: bool,
    /// [`El::anchored_unscaled`]: at window scale, not its anchor's zoom.
    pub unscaled: bool,
}

impl El {
    /// Position an out-of-flow overlay above an earlier keyed element, clamped to the window.
    /// Attach the overlay at the root to escape the anchor's clipping ancestors.
    pub fn above(self, key: impl Into<String>, gutter: f32) -> Self {
        self.anchored(key, Side::Top, Align::Start, gutter)
    }
    /// Position an out-of-flow overlay on `side` of an earlier keyed element, aligned along that
    /// side, `gap` points away. It flips to the opposite side when it doesn't fit, and stays
    /// 8 points inside the window. Attach it at the root (after the anchor) so it draws above
    /// later siblings and escapes clipping ancestors.
    ///
    /// It draws at its anchor's zoom: inside a page drawn at [`El::scale`]`(1.6)`, the overlay
    /// and its text draw 1.6 times their size too, growing away from the anchor. Its gap zooms
    /// with it, and flipping, fitting and the 8-point window margin use its drawn size. Its own
    /// [`El::scale`] replaces the inherited zoom; [`El::anchored_unscaled`] keeps it at window
    /// scale.
    pub fn anchored(mut self, key: impl Into<String>, side: Side, align: Align, gap: f32) -> Self {
        self.style.position = Position::Absolute;
        self.above = Some(Anchor { key: key.into(), side, align, gap, fit: false, unscaled: false });
        self
    }
    /// [`El::anchored`], sized to the window as Floating UI's `size` middleware sizes a
    /// dropdown: when the overlay fits on neither side, it goes to the side with more room and
    /// is at most that room long (tall beside a Top or Bottom anchor, wide beside Left or
    /// Right), 8 points inside the window. Let a scroll viewport inside it shrink (`.shrink()`
    /// and `.min_h(0.0)` on it and on the columns around it), so a long list scrolls instead of
    /// running off the window.
    pub fn anchored_fit(self, key: impl Into<String>, side: Side, align: Align, gap: f32) -> Self {
        let mut e = self.anchored(key, side, align, gap);
        if let Some(a) = &mut e.above {
            a.fit = true;
        }
        e
    }
    /// Keep this [`El::anchored`] overlay at window scale instead of its anchor's zoom, for an
    /// overlay that zooms its own content (or must not zoom). Call it after `anchored`.
    pub fn anchored_unscaled(mut self) -> Self {
        if let Some(a) = &mut self.above {
            a.unscaled = true;
        }
        self
    }
}
