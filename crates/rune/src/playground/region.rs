//! Which of the playground's three columns has the keyboard: the story list, the stage or the
//! side column. One at a time, and only it gets keys: the list its arrows, letters and triggers,
//! the side column its controls, variants' digits and `side_keys` hook, the stage the story's
//! own keys. A click in a column, or focus moving into one, gives it the keyboard.
//!
//! [`Keyboard`] is the one place that remembers it: the column, whether focus is on the slot
//! that takes the story's keys, and the focus it last took.

use super::*;

/// A column of the open playground, as the keyboard sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Region {
    #[default]
    List,
    Stage,
    Side,
}

/// What gives a column the keyboard.
pub(crate) enum Moved {
    /// A press, or a key that asks for a column: `region` takes the keyboard, and `seen` is the
    /// focus it was made with (`None` when nothing is focused), so the column holds until focus
    /// moves again.
    Give { region: Region, seen: Option<String> },
    /// A key the stage's steps pressed holds `region` for its route. Focus was set aside for
    /// it, so the focus it last saw stays as it was.
    Hold(Region),
    /// The route is over: `region` takes the keyboard again, and `back` is the focus it puts
    /// back. `None` when the key left focus where its handler chose: the next look at focus
    /// gives the keyboard to the column it landed in.
    Resume { region: Region, back: Option<String> },
    /// Focus moved to `focus`, into `region`: none when it left every column or only a hover
    /// moved it, which takes no keys.
    Focus { focus: Option<String>, region: Option<Region> },
    /// Focus moved onto the open story's slot (`true`) or off it.
    Slot(bool),
    /// The playground closed: the list has the keyboard.
    Closed,
}

/// Which column has the keyboard, whether focus is on the open story's slot, and the focus it
/// last took as a move: one that only a hover moved (a row under the pointer) is passed over
/// rather than taken as one. The slot goes by focus alone: a click elsewhere that leaves focus
/// on it, as a link's does, leaves it its keys and its mark.
#[derive(Clone, Default)]
pub(crate) struct Keyboard {
    region: Region,
    slot_focused: bool,
    seen: Option<String>,
}

impl Keyboard {
    pub(crate) fn region(&self) -> Region {
        self.region
    }

    /// Whether focus is on the open story's slot, whose view marks what its keys move.
    pub(crate) fn slot_focused(&self) -> bool {
        self.slot_focused
    }

    /// Whether `focus` is the focus it last took, so a look at it has nothing to do.
    pub(crate) fn saw(&self, focus: Option<&str>) -> bool {
        self.seen.as_deref() == focus
    }

    /// Applies `moved`; true when the column or the slot changed.
    pub(crate) fn step(&mut self, moved: Moved) -> bool {
        let before = (self.region, self.slot_focused);
        match moved {
            Moved::Give { region, seen } => {
                self.region = region;
                self.seen = seen;
            }
            Moved::Hold(region) => self.region = region,
            Moved::Resume { region, back } => {
                self.region = region;
                if back.is_some() { self.seen = back; }
            }
            // Focus leaving every column keeps the one that has the keyboard, and a hover takes
            // no keys: neither moves a column.
            Moved::Focus { focus, region } if self.seen != focus => {
                self.seen = focus;
                if let Some(region) = region { self.region = region; }
            }
            Moved::Focus { .. } => {}
            Moved::Slot(slot) => self.slot_focused = slot,
            Moved::Closed => self.region = Region::List,
        }
        (self.region, self.slot_focused) != before
    }
}

impl<S: 'static> Live<S> {
    pub(crate) fn region(&self) -> Region {
        self.keyboard.region()
    }

    /// Whether the keyboard is on the side column's slot, whose view marks what its keys move.
    pub(super) fn side_active(&self) -> bool {
        self.keyboard.slot_focused()
    }

    /// Whether `focus` is the focus the keyboard last took.
    pub(crate) fn saw_focus(&self, focus: Option<&str>) -> bool {
        self.keyboard.saw(focus)
    }

    /// Moves the keyboard; true when that changed what the columns draw.
    pub(crate) fn moved(&mut self, moved: Moved) -> bool {
        self.keyboard.step(moved)
    }

    /// The column at window point `x`, in a window `width` wide.
    pub(crate) fn region_at(&self, x: f32, width: f32) -> Region {
        let (sidebar_w, side_w) = self.widths(width);
        if x < sidebar_w + 1.0 {
            Region::List
        } else if x >= width - side_w {
            Region::Side
        } else {
            Region::Stage
        }
    }
}
