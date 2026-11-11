use super::*;

/// Every kind of event the keyboard is fed: a press in a column, focus moving (onto the side
/// column's slot, another control there, the stage or the list, or nowhere), a hover, the
/// slot's focus coming or going, and the playground closing.
#[derive(Clone, Copy, Debug)]
enum Feed {
    PressList,
    PressStage,
    PressSide,
    FocusSlot,
    FocusSide,
    FocusStage,
    FocusList,
    HoverSide,
    FocusAway,
    Slot(bool),
    Close,
}

const FEEDS: [Feed; 11] = [
    Feed::PressList,
    Feed::PressStage,
    Feed::PressSide,
    Feed::FocusSlot,
    Feed::FocusSide,
    Feed::FocusStage,
    Feed::FocusList,
    Feed::HoverSide,
    Feed::FocusAway,
    Feed::Slot(true),
    Feed::Close,
];

impl Feed {
    fn moved(self) -> Moved {
        let path = |p: &str| Some(p.to_owned());
        match self {
            Feed::PressList => Moved::Give { region: Region::List, seen: path("list/story") },
            Feed::PressStage => Moved::Give { region: Region::Stage, seen: None },
            Feed::PressSide => Moved::Give { region: Region::Side { slot_focused: false }, seen: path("side/variant-1") },
            Feed::FocusSlot => Moved::Focus { focus: path("side/slot"), column: Some(Column::Side), slot_focused: true, hover: false },
            Feed::FocusSide => Moved::Focus { focus: path("side/variant-1"), column: Some(Column::Side), slot_focused: false, hover: false },
            Feed::FocusStage => Moved::Focus { focus: path("stage/field"), column: Some(Column::Stage), slot_focused: false, hover: false },
            Feed::FocusList => Moved::Focus { focus: path("list/story"), column: Some(Column::List), slot_focused: false, hover: false },
            Feed::HoverSide => Moved::Focus { focus: path("side/hovered-row"), column: Some(Column::Side), slot_focused: false, hover: true },
            Feed::FocusAway => Moved::Focus { focus: None, column: None, slot_focused: false, hover: false },
            Feed::Slot(on) => Moved::Slot(on),
            Feed::Close => Moved::Closed,
        }
    }
}

/// Feeds every sequence of `FEEDS` up to `depth` long, checking at each step that the column
/// changes only when an event asks for one, and that the same event twice is the same as once.
fn walk(keys: &Keyboard, depth: usize, checked: &mut u64) {
    for feed in FEEDS {
        let before = keys.region();
        let mut next = keys.clone();
        let changed = next.step(feed.moved());
        *checked += 1;
        assert_eq!(changed, before != next.region(), "{feed:?} says whether the column changed");
        match feed {
            Feed::PressList | Feed::PressStage | Feed::PressSide => {
                let asked = match feed {
                    Feed::PressList => Region::List,
                    Feed::PressStage => Region::Stage,
                    _ => Region::Side { slot_focused: false },
                };
                assert_eq!(next.region(), asked, "a press gives its column the keyboard");
            }
            Feed::Slot(false) => assert_eq!(next.region().column(), before.column(), "a slot coming or going never moves the column"),
            Feed::HoverSide => assert_eq!(next.region().column(), before.column(), "a hover takes no keys, so it never moves the column"),
            Feed::Close => assert_eq!(next.region(), Region::List, "closing gives the list the keyboard"),
            _ => {}
        }
        let region = next.region();
        assert!(!next.step(feed.moved()), "{feed:?} twice changes nothing");
        assert_eq!(next.region(), region, "{feed:?} twice keeps its column");
        if depth > 1 { walk(&next, depth - 1, checked); }
    }
}

#[test]
fn any_order_of_keyboard_events_leaves_one_coherent_column() {
    let mut checked = 0;
    walk(&Keyboard::default(), 5, &mut checked);
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
    // Focus moving away from the list's row gives the stage up to where focus went.
    keys.step(Moved::Focus { focus: Some("list/story".into()), column: Some(Column::List), slot_focused: false, hover: false });
    assert_eq!(keys.region(), Region::List);
}

#[test]
fn only_the_slots_focus_takes_the_side_columns_keys() {
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
