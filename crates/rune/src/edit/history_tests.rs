use super::*;

#[test]
fn a_group_opened_on_a_secure_field_records_from_when_it_turns_plain() {
    let mut e = TextEdit::new("pw").secure();
    e.undo_group(|e| {
        e.insert("1");
        assert!(matches!(e.group, Group::Unrecorded));
        e.set_secure(false);
        e.insert("a");
        e.set_secure(false);
        e.insert("b");
    });
    assert_eq!(e.text, "pw1ab");
    e.undo();
    assert_eq!(e.text, "pw1", "one step, from when the field turned plain, not moved by a second set_secure(false)");
    e.undo();
    assert_eq!(e.text, "pw1");
}

#[test]
fn a_group_is_closed_in_clones_and_holds_undo_until_it_closes() {
    let mut e = TextEdit::new("a");
    e.undo_group(|e| {
        e.insert("b");
        assert!(e.group.is_open() && !e.clone().group.is_open());
        e.undo();
        assert_eq!(e.text, "ab", "undo waits for the group to close");
        e.set_secure(true);
        assert!(matches!(e.group, Group::Unrecorded), "turning secure drops the baseline's text");
        e.set_secure(false);
        e.replace_text("c");
    });
    assert!(!e.group.is_open());
    e.undo();
    assert_eq!(e.text, "c", "replace_text in the group discards the history before it");
}
