use super::*;

fn key(k: Key) -> KeyEvent { KeyEvent::from(k) }
fn shift(k: Key) -> KeyEvent { KeyEvent { key: k, mods: Mods { shift: true, ..Mods::default() } } }
fn cmd(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() } }
}

#[test]
fn add_carets_deduplicates_merges_overlaps_and_keeps_abutting_selections() {
    let mut e = TextEdit::new("aaaa bbbb");
    e.set_caret(0);
    assert!(!e.has_extra_carets());
    assert!(!e.add_caret(0), "duplicate primary caret does not change set");
    assert!(e.add_caret(5));
    assert!(e.has_extra_carets());
    assert_eq!(e.carets(), vec![Caret::caret(0), Caret::caret(5)]);
    assert!(!e.add_caret(5), "duplicate secondary caret ignored");

    // Overlapping selection merges with primary and preserves primary identity.
    e.set_selection(1, 3);
    assert!(e.add_selection(2, 4));
    assert_eq!(e.carets(), vec![Caret::selection(1, 4)]);
    assert!(!e.has_extra_carets());

    // Abutting non-empty selections stay distinct (e.g. Ctrl+D on "aaaa").
    e.set_selection(0, 2);
    assert!(e.add_selection(2, 4));
    assert_eq!(e.carets(), vec![Caret::selection(0, 2), Caret::selection(2, 4)]);

    // Collapsed caret inside a selection merges into that selection.
    assert!(!e.add_caret(1));
    assert_eq!(e.carets(), vec![Caret::selection(0, 2), Caret::selection(2, 4)]);
}

#[test]
fn add_caret_above_below_and_simultaneous_typing_backspace_delete_undo_redo() {
    let mut e = TextEdit::new("alpha
beta
gamma");
    e.set_caret(2); // "al|pha"
    assert!(!e.add_caret_above(None), "already on first line");
    assert!(e.add_caret_below(None)); // "be|ta" -> offset 6+2 = 8
    assert!(e.add_caret_below(None)); // "ga|mma" -> offset 11+2 = 13
    assert!(!e.add_caret_below(None), "already on last line");
    assert_eq!(e.carets(), vec![Caret::caret(2), Caret::caret(8), Caret::caret(13)]);

    // Simultaneous typing joins into one undo run.
    e.key(&key(Key::Text("X".into())), None);
    e.key(&key(Key::Text("Y".into())), None);
    assert_eq!(e.text, "alXYpha
beXYta
gaXYmma");
    assert_eq!(e.carets(), vec![Caret::caret(4), Caret::caret(12), Caret::caret(19)]);

    // Single undo restores both text AND all 3 caret positions.
    e.undo();
    assert_eq!(e.text, "alpha
beta
gamma");
    assert_eq!(e.carets(), vec![Caret::caret(2), Caret::caret(8), Caret::caret(13)]);

    // Redo restores text AND all 3 caret positions.
    e.redo();
    assert_eq!(e.text, "alXYpha
beXYta
gaXYmma");
    assert_eq!(e.carets(), vec![Caret::caret(4), Caret::caret(12), Caret::caret(19)]);

    // Simultaneous Backspace and Delete.
    e.key(&key(Key::Backspace), None);
    assert_eq!(e.text, "alXpha
beXta
gaXmma");
    e.key(&key(Key::Delete), None);
    assert_eq!(e.text, "alXha
beXa
gaXma");

    // History JSON roundtrip preserves secondary carets.
    let json = e.history_json(128, 1024 * 1024).expect("history json");
    let mut restored = TextEdit::new(&e.text);
    restored.set_carets(Caret::caret(3), [Caret::caret(9), Caret::caret(14)]);
    assert!(restored.restore_history_json(&json));
    restored.undo();
    assert_eq!(restored.text, "alXYpha
beXYta
gaXYmma");
    assert_eq!(restored.carets(), vec![Caret::caret(4), Caret::caret(12), Caret::caret(19)]);
}

#[test]
fn add_next_occurrence_copy_cut_paste_and_escape_collapse() {
    let mut e = TextEdit::new("foo bar foo baz foo");
    e.set_caret(1); // inside first "foo"
    assert!(e.add_next_occurrence());
    assert_eq!(e.carets(), vec![Caret::selection(0, 3)]);
    assert!(e.add_next_occurrence());
    assert_eq!(e.carets(), vec![Caret::selection(0, 3), Caret::selection(8, 11)]);
    assert!(e.add_next_occurrence());
    assert_eq!(e.carets(), vec![Caret::selection(0, 3), Caret::selection(8, 11), Caret::selection(16, 19)]);
    assert!(!e.add_next_occurrence(), "all occurrences already selected");

    // Copy joins non-empty selections in document order with newline.
    let out = e.key(&cmd('c'), None);
    assert_eq!(out.copy.as_deref(), Some("foo
foo
foo"));

    // Line-matched paste distributes one line per caret.
    e.paste("one
two
three");
    assert_eq!(e.text, "one bar two baz three");
    assert_eq!(e.carets(), vec![Caret::caret(3), Caret::caret(11), Caret::caret(21)]);

    // Non-matching paste inserts full string at every caret.
    e.paste("!");
    assert_eq!(e.text, "one! bar two! baz three!");

    // Cut copies all selections and deletes them in one undo step.
    e.set_carets(Caret::selection(0, 3), [Caret::selection(9, 12)]);
    let cut = e.key(&cmd('x'), None);
    assert_eq!(cut.copy.as_deref(), Some("one
two"));
    assert_eq!(e.text, "! bar ! baz three!");
    e.undo();
    assert_eq!(e.text, "one! bar two! baz three!");

    // First Escape collapses extra carets without blurring; second Escape blurs.
    assert!(e.has_extra_carets());
    let esc1 = e.key(&key(Key::Escape), None);
    assert!(!esc1.blur);
    assert!(!e.has_extra_carets());
    let esc2 = e.key(&key(Key::Escape), None);
    assert!(esc2.blur);
}

#[test]
fn multi_navigation_merges_collisions_and_ime_commit_and_secure_guard() {
    let mut e = TextEdit::new("ab
cd");
    e.set_caret(1);
    e.add_caret(4);
    e.key(&shift(Key::Right), None);
    assert_eq!(e.carets(), vec![Caret::selection(1, 2), Caret::selection(4, 5)]);

    // IME commit replaces at every active caret.
    e.ime(&Ime::Commit("Z".into()));
    assert_eq!(e.text, "aZ
cZ");

    // Moving all carets to document start merges them into one caret at 0.
    for _ in 0..5 {
        e.key(&key(Key::Left), None);
    }
    assert_eq!(e.carets(), vec![Caret::caret(0)]);
    assert!(!e.has_extra_carets());

    // Secure fields refuse and clear extra carets.
    e.add_caret(2);
    assert!(e.has_extra_carets());
    e.set_secure(true);
    assert!(!e.has_extra_carets());
    assert!(!e.add_caret(1));
}

#[test]
fn browser_keys_are_ignored_by_text_edit() {
    let mut e = TextEdit::new("hello");
    e.set_caret(2);
    for k in [Key::BrowserBack, Key::BrowserForward] {
        let out = e.key(&key(k), None);
        assert_eq!(out, EditOutcome::default());
        assert_eq!(e.text, "hello");
        assert_eq!((e.anchor, e.focus), (2, 2));
    }
}
