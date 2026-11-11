use super::*;

/// The two caches this replaced, transcribed from the old code: the column, a separate
/// `side_active` flag beside it, and `region_seen`.
#[derive(Clone, Debug)]
struct Old {
    region: Column,
    side_active: bool,
    seen: Option<String>,
}

impl Old {
    fn new() -> Self {
        Self { region: Column::List, side_active: false, seen: None }
    }

    /// `set_region`: the column takes the keyboard, and `side_active` is none of its business.
    fn set_region(&mut self, region: Region) {
        self.region = region.column();
    }

    /// `see_focus`: the focus the region last looked at.
    fn see_focus(&mut self, focus: Option<String>) {
        self.seen = focus;
    }

    /// `sync_region`: a move gives its column the keyboard, unless a hover moved it or focus
    /// left every column.
    fn sync_region(&mut self, focus: Option<&str>, column: Option<Column>, hover: bool) {
        if self.seen.as_deref() == focus { return; }
        self.seen = focus.map(str::to_owned);
        if hover { return; }
        if let Some(column) = column { self.region = column; }
    }

    /// `set_side_active`.
    fn set_side_active(&mut self, active: bool) {
        self.side_active = active;
    }

    /// `Live::close`.
    fn close(&mut self) {
        self.region = Column::List;
    }
}

/// An event as the runtime feeds it, and what the old code did with the same event.
#[derive(Clone, Copy, Debug)]
enum Feed {
    /// A press in `column`, reading `slot` from focus: the side column's slot, or not.
    Press { column: Column, slot: bool },
    /// Focus moved to `focus`, into `column` (`None` when it left them), the side column's slot
    /// focused when `slot`, and `hover` when only a hover moved it.
    Focus { focus: Option<&'static str>, column: Option<Column>, slot: bool, hover: bool },
    /// A focus change reports whether the side column's slot has it.
    Slot(bool),
    /// The playground closed.
    Close,
}

const FEEDS: [Feed; 10] = [
    Feed::Press { column: Column::List, slot: false },
    Feed::Press { column: Column::Stage, slot: true },
    Feed::Press { column: Column::Side, slot: true },
    Feed::Focus { focus: Some("side/slot"), column: Some(Column::Side), slot: true, hover: false },
    Feed::Focus { focus: Some("side/variant"), column: Some(Column::Side), slot: false, hover: false },
    Feed::Focus { focus: Some("stage/field"), column: Some(Column::Stage), slot: false, hover: false },
    Feed::Focus { focus: Some("side/hovered"), column: Some(Column::Side), slot: false, hover: true },
    Feed::Focus { focus: None, column: None, slot: false, hover: false },
    Feed::Slot(true),
    Feed::Slot(false),
];

/// The focus a press in `column` is made with.
fn focus_of(column: Column) -> Option<String> {
    match column {
        Column::List => Some("list/story".into()),
        Column::Stage => None,
        Column::Side => Some("side/control".into()),
    }
}

impl Feed {
    /// Feeds `self` to the keyboard and to the old caches; true when the keyboard's column
    /// changed.
    fn apply(self, keys: &mut Keyboard, old: &mut Old) -> bool {
        match self {
            Feed::Press { column, slot } => {
                let changed = keys.step(Moved::Give { region: Region::of(column, slot), seen: focus_of(column) });
                // Both caches are fed the same press: the column from the press, and
                // `side_active` from the focus it read.
                old.set_region(Region::of(column, slot));
                old.set_side_active(slot);
                old.see_focus(focus_of(column));
                changed
            }
            Feed::Focus { focus, column, slot, hover } => {
                let changed = keys.step(Moved::Focus {
                    focus: focus.map(str::to_owned),
                    column,
                    slot_focused: slot,
                    hover,
                });
                // Every focus change reports the slot too, even one whose path didn't move.
                let slot = slot && !hover;
                let reported = keys.step(Moved::Slot(slot));
                old.sync_region(focus, column, hover);
                old.set_side_active(slot);
                changed || reported
            }
            Feed::Slot(slot) => {
                let changed = keys.step(Moved::Slot(slot));
                old.set_side_active(slot);
                changed
            }
            Feed::Close => {
                let changed = keys.step(Moved::Closed);
                old.close();
                changed
            }
        }
    }
}

/// Feeds every sequence of `FEEDS` up to `depth` long. After each, the keyboard's column is the
/// old `region`, and its slot takes keys only while that region is the side column and the old
/// `side_active` says so: the old code's incoherent pair — the stage with `side_active` set —
/// reads as the stage with no slot, never as a column change.
fn walk(keys: &Keyboard, old: &Old, depth: usize, checked: &mut u64) {
    for feed in FEEDS {
        let (mut next, mut next_old) = (keys.clone(), old.clone());
        let before = next.region();
        let changed = feed.apply(&mut next, &mut next_old);
        *checked += 1;
        assert_eq!(changed, before != next.region(), "{feed:?} says whether the column changed");
        assert_eq!(next.region().column(), next_old.region, "{feed:?}: the column is the last one a press, a move or a route asked for");
        assert_eq!(next.region().slot_focused(), next_old.side_active && next_old.region == Column::Side, "{feed:?}: the side column's slot takes keys only while it has them");
        if depth > 1 { walk(&next, &next_old, depth - 1, checked); }
    }
}

#[test]
fn any_order_of_keyboard_events_leaves_the_column_and_slot_the_old_caches_did() {
    let mut checked = 0;
    walk(&Keyboard::default(), &Old::new(), 5, &mut checked);
    let expected: u64 = (1..=5).map(|d| FEEDS.len().pow(d) as u64).sum();
    assert_eq!(checked, expected);
}

#[test]
fn a_clicked_column_keeps_the_keyboard_until_focus_moves() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::Stage, seen: None });
    // A frame's look at focus, which nothing has: the stage keeps the keyboard.
    keys.step(Moved::Focus { focus: None, column: None, slot_focused: false, hover: false });
    assert_eq!(keys.region(), Region::Stage);
    // A slot coming into focus in the side column doesn't take it: only the column a press or a
    // move asks for does.
    keys.step(Moved::Slot(true));
    assert_eq!(keys.region(), Region::Stage, "a slot elsewhere leaves the stage its keys");
    keys.step(Moved::Focus { focus: Some("list/story".into()), column: Some(Column::List), slot_focused: false, hover: false });
    assert_eq!(keys.region(), Region::List);
}

#[test]
fn only_the_side_columns_slot_takes_its_keys() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::Side { slot_focused: false }, seen: Some("side/variant-1".into()) });
    keys.step(Moved::Slot(true));
    assert_eq!(keys.region(), Region::Side { slot_focused: true }, "a slot focused in the side column takes its keys");
    keys.step(Moved::Slot(false));
    assert_eq!(keys.region(), Region::Side { slot_focused: false }, "and gives them back when focus leaves it");
    // Focus leaving the columns takes the slot's keys with it.
    keys.step(Moved::Slot(true));
    keys.step(Moved::Focus { focus: None, column: None, slot_focused: false, hover: false });
    assert_eq!(keys.region(), Region::Side { slot_focused: false });
    // A hover moves focus onto the slot's row without taking anything.
    keys.step(Moved::Focus { focus: Some("side/slot".into()), column: Some(Column::Side), slot_focused: false, hover: true });
    assert_eq!(keys.region(), Region::Side { slot_focused: false });
}

#[test]
fn a_route_that_moves_focus_gives_the_column_it_landed_in_the_keyboard() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::List, seen: Some("list/story".into()) });
    // A key a step pressed holds the stage, then gives the list back: focus was set aside, so
    // nothing new is seen.
    keys.step(Moved::Hold(Region::Stage));
    keys.step(Moved::Resume { region: Region::List, back: None });
    assert_eq!(keys.region(), Region::List);
    // The stage's key focused a control in the side column: the next look at focus gives it the
    // keyboard, because the route restored nothing.
    keys.step(Moved::Focus { focus: Some("side/variant".into()), column: Some(Column::Side), slot_focused: false, hover: false });
    assert_eq!(keys.region(), Region::Side { slot_focused: false }, "focus where the route left it takes the keyboard");
}

#[test]
fn a_route_that_puts_focus_back_keeps_the_column_it_had() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::Stage, seen: Some("stage/field".into()) });
    keys.step(Moved::Hold(Region::Stage));
    keys.step(Moved::Resume { region: Region::Stage, back: Some("stage/field".into()) });
    // The focus that came back is the one the keyboard saw, so it isn't a move.
    keys.step(Moved::Focus { focus: Some("stage/field".into()), column: Some(Column::List), slot_focused: false, hover: false });
    assert_eq!(keys.region(), Region::Stage);
}
