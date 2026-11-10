use super::*;
use crate::{MarkRange, TextEdit};

#[test]
fn armed_marks_count_only_at_the_selection_they_were_armed_at() {
    let mut t = Typing::Armed { at: (3, 3), marks: vec![Mark::Bold] };
    assert_eq!(t.at((3, 3)), Some(&[Mark::Bold][..]));
    assert_eq!(t.at((4, 4)), None);
    assert_eq!(Typing::Inherited.at((3, 3)), None);
    assert_eq!(t.take_at((4, 4)), None);
    assert_eq!(t, Typing::Inherited, "a stale style is dropped when text is typed elsewhere");
    let mut t = Typing::Armed { at: (3, 3), marks: vec![Mark::Bold] };
    assert_eq!(t.take_at((3, 3)), Some(vec![Mark::Bold]));
    assert_eq!(t, Typing::Inherited, "typed text uses the style up");
}

#[test]
fn a_caret_moved_through_the_public_fields_and_back_still_types_the_armed_style() {
    let mut e = TextEdit::new("ab").rich();
    e.toggle_mark(Mark::Bold);
    e.anchor = 0;
    e.focus = 0;
    assert!(!e.has_mark(&Mark::Bold), "armed at the end, so not at the start");
    e.anchor = 2;
    e.focus = 2;
    e.insert("c");
    assert_eq!(e.marks(), [MarkRange { start: 2, end: 3, mark: Mark::Bold }]);
    e.set_caret(3);
    e.toggle_mark(Mark::Italic);
    e.set_caret(3);
    e.insert("d");
    assert_eq!(e.marks(), [MarkRange { start: 2, end: 4, mark: Mark::Bold }], "placing the caret, even where it was, disarms");
}
