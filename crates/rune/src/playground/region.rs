//! Which of the playground's three columns has the keyboard: the story list, the stage or the
//! side column. One at a time, and only it gets keys: the list its arrows, letters and triggers,
//! the side column its controls, variants' digits and `side_keys` hook, the stage the story's
//! own keys. A click in a column, or focus moving into one, gives it the keyboard.
//!
//! [`Keyboard`] is the one place that remembers it: the column, and while it is the side
//! column's, whether the slot that takes the story's keys is what focus is on.

use super::*;

/// A column of the open playground, as the keyboard sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Region {
    #[default]
    List,
    Stage,
    /// The side column, `slot_focused` while focus is on the open story's slot: the one control
    /// whose keys are its `side_keys` hook, whose view marks what they move.
    Side { slot_focused: bool },
}

impl Region {
    /// `column`'s region, the side column's slot taking keys only when `slot_focused`.
    pub(crate) fn of(column: Column, slot_focused: bool) -> Self {
        match column {
            Column::List => Region::List,
            Column::Stage => Region::Stage,
            Column::Side => Region::Side { slot_focused },
        }
    }

    /// Whether focus is on the side column's slot, whose view marks what its keys move.
    pub(crate) fn slot_focused(self) -> bool {
        matches!(self, Region::Side { slot_focused: true })
    }

    /// Whether keys go to the side column's slot, focus being on it when `on_slot`: only while
    /// this region marks it. A click elsewhere can leave focus there and take the keys away.
    pub(crate) fn takes_side_keys(self, on_slot: bool) -> bool {
        on_slot && self.slot_focused()
    }
}

/// The column a press or a key asks for, before the side column's slot is looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Column {
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
    /// Focus moved to `focus`: into `column`, onto the side column's slot when `slot_focused`.
    /// `column` is none when it left every column or only a hover moved it, which takes no keys.
    Focus { focus: Option<String>, column: Option<Column>, slot_focused: bool },
    /// Focus moved onto the side column's slot (`true`) or off it.
    Slot(bool),
    /// The playground closed: the list has the keyboard.
    Closed,
}

/// Which column has the keyboard, and the focus it last took as a move: one that only a hover
/// moved (a row under the pointer) is passed over rather than taken as one.
#[derive(Clone, Default)]
pub(crate) struct Keyboard {
    region: Region,
    seen: Option<String>,
}

impl Keyboard {
    pub(crate) fn region(&self) -> Region {
        self.region
    }

    /// Whether `focus` is the focus it last took, so a look at it has nothing to do.
    pub(crate) fn saw(&self, focus: Option<&str>) -> bool {
        self.seen.as_deref() == focus
    }

    /// Applies `moved`; true when the region changed.
    pub(crate) fn step(&mut self, moved: Moved) -> bool {
        let before = self.region;
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
            Moved::Focus { focus, column, slot_focused } if self.seen != focus => {
                self.seen = focus;
                if let Some(column) = column { self.region = Region::of(column, slot_focused); }
            }
            Moved::Focus { .. } => {}
            // Only the side column's own slot takes its keys: a slot that comes into focus
            // while another column has them leaves that column alone.
            Moved::Slot(slot) => {
                if let Region::Side { slot_focused } = &mut self.region { *slot_focused = slot; }
            }
            Moved::Closed => self.region = Region::List,
        }
        self.region != before
    }
}

impl<S: 'static> Live<S> {
    pub(crate) fn region(&self) -> Region {
        self.keyboard.region()
    }

    /// Whether the keyboard is on the side column's slot, whose view marks what its keys move.
    pub(super) fn side_active(&self) -> bool {
        self.keyboard.region().slot_focused()
    }

    /// Whether `focus` is the focus the keyboard last took.
    pub(crate) fn saw_focus(&self, focus: Option<&str>) -> bool {
        self.keyboard.saw(focus)
    }

    /// Moves the keyboard; true when the column changed, which redraws the columns' focus.
    pub(crate) fn moved(&mut self, moved: Moved) -> bool {
        self.keyboard.step(moved)
    }

    /// The column at window point `x`, in a window `width` wide.
    pub(crate) fn region_at(&self, x: f32, width: f32) -> Column {
        let (sidebar_w, side_w) = self.widths(width);
        if x < sidebar_w + 1.0 {
            Column::List
        } else if x >= width - side_w {
            Column::Side
        } else {
            Column::Stage
        }
    }
}
