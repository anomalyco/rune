use rune_core::tok::TextStyle;
use rune_text::{TextFlow, TextSystem};

use super::*;

fn key(key: Key, mods: Mods) -> KeyEvent {
    KeyEvent { key, mods }
}

fn plain(k: Key) -> KeyEvent {
    KeyEvent::from(k)
}

#[test]
fn line_home_end_defaults_false_and_survives_replace_text() {
    let mut e = TextEdit::new("first\nsecond");
    assert!(!e.line_home_end());
    e.set_line_home_end(true);
    assert!(e.line_home_end());
    e.replace_text("alpha\nbeta");
    assert!(e.line_home_end(), "replace_text preserves line_home_end");
    e.set_line_home_end(false);
    assert!(!e.line_home_end());

    let built = TextEdit::new("one\ntwo").with_line_home_end(true);
    assert!(built.line_home_end());
}

#[test]
fn home_end_target_respects_platform_flag_and_modifiers() {
    let mut e = TextEdit::new("first\nsecond\nthird");
    // Caret in the middle of the second line ("sec|ond", byte 9).
    e.set_caret(9);
    let none = Mods::default();
    let shift = Mods { shift: true, ..Mods::default() };
    let cmd = Mods { cmd: true, ..Mods::default() };
    let cmd_shift = Mods { cmd: true, shift: true, ..Mods::default() };
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    let ctrl_cmd = Mods { ctrl: true, cmd: true, ..Mods::default() };

    // Default (line_home_end == false): macOS jumps to document ends; non-macOS jumps to line edges.
    assert_eq!(e.home_end_target(None, false, none, true), 0);
    assert_eq!(e.home_end_target(None, true, none, true), e.text.len());
    assert_eq!(e.home_end_target(None, false, shift, true), 0);
    assert_eq!(e.home_end_target(None, true, shift, true), e.text.len());
    assert_eq!(e.home_end_target(None, false, none, false), 6);
    assert_eq!(e.home_end_target(None, true, none, false), 12);
    assert_eq!(e.home_end_target(None, false, ctrl_cmd, false), 0);
    assert_eq!(e.home_end_target(None, true, ctrl_cmd, false), e.text.len());

    // Opt-in (line_home_end == true): macOS plain/Shift Home/End jump to current line edges,
    // while Cmd+Home/End and Ctrl+Home/End still jump to document ends.
    e.set_line_home_end(true);
    assert_eq!(e.home_end_target(None, false, none, true), 6);
    assert_eq!(e.home_end_target(None, true, none, true), 12);
    assert_eq!(e.home_end_target(None, false, shift, true), 6);
    assert_eq!(e.home_end_target(None, true, shift, true), 12);
    assert_eq!(e.home_end_target(None, false, cmd, true), 0);
    assert_eq!(e.home_end_target(None, true, cmd, true), e.text.len());
    assert_eq!(e.home_end_target(None, false, cmd_shift, true), 0);
    assert_eq!(e.home_end_target(None, true, cmd_shift, true), e.text.len());
    assert_eq!(e.home_end_target(None, false, ctrl, true), 0);
    assert_eq!(e.home_end_target(None, true, ctrl, true), e.text.len());
    // Off macOS, line_home_end(true) keeps the same line-edge behavior and Ctrl document ends.
    assert_eq!(e.home_end_target(None, false, none, false), 6);
    assert_eq!(e.home_end_target(None, true, none, false), 12);
    assert_eq!(e.home_end_target(None, false, ctrl_cmd, false), 0);
    assert_eq!(e.home_end_target(None, true, ctrl_cmd, false), e.text.len());
}

#[test]
fn home_end_target_uses_visual_wrapped_lines_and_rtl_layout() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);

    // Wrapped single paragraph: verify visual line edges differ from document ends.
    let wrapped = "alpha bravo charlie delta echo foxtrot golf hotel";
    let layout = ts.layout_rich(wrapped, &[], style, 1.0, Some(90.0));
    let lines: Vec<std::ops::Range<usize>> = layout.lines().map(|l| l.text_range()).collect();
    assert!(lines.len() >= 3, "fixture must wrap into at least 3 visual lines: {lines:?}");

    let mid_line = &lines[1];
    let mid_pos = (mid_line.start + mid_line.end) / 2;
    let mut e = TextEdit::new(wrapped).with_line_home_end(true);
    e.set_caret(mid_pos);
    let flow = Some(TextFlow::Single(&layout));
    let start = e.home_end_target(flow, false, Mods::default(), true);
    let end = e.home_end_target(flow, true, Mods::default(), true);
    assert!(start > 0 && start < mid_pos, "visual line start {start} should be inside document before {mid_pos}");
    assert!(end > mid_pos && end < wrapped.len(), "visual line end {end} should be inside document after {mid_pos}");
    assert_eq!(e.home_end_target(flow, false, Mods { cmd: true, ..Mods::default() }, true), 0);
    assert_eq!(e.home_end_target(flow, true, Mods { cmd: true, ..Mods::default() }, true), wrapped.len());

    // RTL multiline text: plain Home/End stay on the current RTL line.
    let rtl = "مرحبا\nبالعالم";
    let line2_start = rtl.find('\n').unwrap() + 1;
    let mut rtl_edit = TextEdit::new(rtl).with_line_home_end(true);
    let rtl_mid = rtl[line2_start..].char_indices().nth(2).unwrap().0 + line2_start;
    rtl_edit.set_caret(rtl_mid);
    assert_eq!(rtl_edit.home_end_target(None, false, Mods::default(), true), line2_start);
    assert_eq!(rtl_edit.home_end_target(None, true, Mods::default(), true), rtl.len());
}

#[test]
fn home_end_handles_empty_pills_and_hidden_secure_edges() {
    // Empty text never panics.
    let empty = TextEdit::new("").with_line_home_end(true);
    assert_eq!(empty.home_end_target(None, false, Mods::default(), true), 0);
    assert_eq!(empty.home_end_target(None, true, Mods::default(), true), 0);

    // Hidden multi-line secure field keeps its fixed-mask end-pinned navigation even with line_home_end.
    let mut sec = TextEdit::new("secret1\nsecret2").secure().with_line_home_end(true);
    sec.set_single_line(false);
    sec.key(&plain(Key::Home), None);
    assert_eq!((sec.anchor, sec.focus), (sec.text.len(), sec.text.len()));
    sec.key(&key(Key::Home, Mods { shift: true, ..Mods::default() }), None);
    assert_eq!((sec.anchor, sec.focus), (0, sec.text.len()));

    // Pill at line end stays intact when selecting with Shift+End.
    let mut pill = TextEdit::new("hi @ada\nnext").with_line_home_end(true);
    pill.set_marks(vec![MarkRange::new(3..7, Mark::Pill("u1".into()))]);
    pill.set_caret(0);
    let end = pill.home_end_target(None, true, Mods { shift: true, ..Mods::default() }, true);
    pill.set(end, true);
    assert_eq!(pill.selected(), "hi @ada");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_key_home_end_matches_issue_1682_repro_and_default() {
    // Default (line_home_end == false): preserves native AppKit document-end navigation.
    let mut native = TextEdit::new("first\nsecond");
    native.set_caret(2);
    native.key(&plain(Key::End), None);
    native.key(&plain(Key::Text("!".into())), None);
    assert_eq!(native.text, "first\nsecond!");

    // Opt-in (line_home_end == true): plain End stays on line 1, Shift selects within the line,
    // and Cmd+Home/End plus Cmd+Up/Down still jump to document ends.
    let mut code = TextEdit::new("first\nsecond").with_line_home_end(true);
    code.set_caret(2);
    code.key(&plain(Key::End), None);
    code.key(&plain(Key::Text("!".into())), None);
    assert_eq!(code.text, "first!\nsecond");

    // Shift+Home selects back to the start of line 1 only.
    code.key(&key(Key::Home, Mods { shift: true, ..Mods::default() }), None);
    assert_eq!(code.selected(), "first!");

    // On line 2, plain Home goes to line 2 start (7), Shift+End selects line 2.
    code.set_caret(9);
    code.key(&plain(Key::Home), None);
    assert_eq!(code.focus, 7);
    code.key(&key(Key::End, Mods { shift: true, ..Mods::default() }), None);
    assert_eq!(code.selected(), "second");

    // Cmd+Home / Cmd+End and Cmd+Up / Cmd+Down still jump to document start/end.
    code.key(&key(Key::Home, Mods { cmd: true, ..Mods::default() }), None);
    assert_eq!(code.focus, 0);
    code.key(&key(Key::End, Mods { cmd: true, ..Mods::default() }), None);
    assert_eq!(code.focus, code.text.len());
    code.key(&key(Key::Up, Mods { cmd: true, ..Mods::default() }), None);
    assert_eq!(code.focus, 0);
    code.key(&key(Key::Down, Mods { cmd: true, ..Mods::default() }), None);
    assert_eq!(code.focus, code.text.len());
}

#[test]
fn key_in_read_only_navigates_selects_and_copies_while_blocking_all_mutations() {
    let shift = Mods { shift: true, ..Mods::default() };
    let cmd = Mods { cmd: true, ..Mods::default() };
    let cmd_shift = Mods { cmd: true, shift: true, ..Mods::default() };
    let word_mods = if cfg!(target_os = "macos") {
        Mods { alt: true, ..Mods::default() }
    } else {
        Mods { ctrl: true, cmd: true, ..Mods::default() }
    };
    let word_shift = Mods { shift: true, ..word_mods };

    let mut e = TextEdit::new("alpha beta\ngamma delta").rich();
    // Seed undo history so Undo/Redo attempts would mutate if unblocked.
    e.set_caret(e.text.len());
    e.insert("!");
    assert_eq!(e.text, "alpha beta\ngamma delta!");
    e.set_caret(0);

    // Character, word, line, and Shift+selection navigation all work.
    let _ = e.key_in_read_only(&plain(Key::Right), None);
    assert_eq!((e.anchor, e.focus), (1, 1));
    let _ = e.key_in_read_only(&key(Key::Right, word_shift), None);
    assert_eq!(e.selected(), if cfg!(target_os = "macos") { "lpha" } else { "lpha " });

    let _ = e.key_in_read_only(&plain(Key::Home), None);
    assert_eq!(e.focus, 0);
    let _ = e.key_in_read_only(&key(Key::Right, shift), None);
    assert_eq!(e.selected(), "a");

    // Cmd/Ctrl+A selects all; Cmd/Ctrl+C copies without mutating.
    let _ = e.key_in_read_only(&key(Key::Command('a'), cmd), None);
    assert_eq!(e.selected(), "alpha beta\ngamma delta!");
    let copy = e.key_in_read_only(&key(Key::Command('c'), cmd), None);
    assert_eq!(copy.copy.as_deref(), Some("alpha beta\ngamma delta!"));

    // Mutating keys all leave text, marks, and blocks untouched and do not submit or cut.
    let before = e.text.clone();
    let blocked = [
        plain(Key::Text("x".into())),
        plain(Key::Backspace),
        plain(Key::Delete),
        plain(Key::Enter),
        key(Key::Enter, shift),
        plain(Key::Tab),
        key(Key::Tab, shift),
        key(Key::Command('x'), cmd),
        key(Key::Command('z'), cmd),
        key(Key::Command('z'), cmd_shift),
        key(Key::Command('y'), cmd),
        key(Key::Command('b'), cmd),
        key(Key::Command('i'), cmd),
        key(Key::Command('u'), cmd),
    ];
    for ev in &blocked {
        let out = e.key_in_read_only(ev, None);
        assert_eq!(e.text, before, "read-only key {ev:?} must not mutate text");
        assert!(e.marks().is_empty(), "read-only key {ev:?} must not add marks");
        assert!(!out.submit, "read-only key {ev:?} must not submit");
        assert!(out.copy.is_none(), "read-only key {ev:?} must not cut/copy");
    }

    // Escape still requests blur.
    assert!(e.key_in_read_only(&plain(Key::Escape), None).blur);

    // Right/Down at the end of a trailing Code or Quote block must not insert a paragraph.
    let mut code = TextEdit::new("let x = 1;").rich();
    code.set_block(Block::Code);
    code.set_caret(code.text.len());
    let _ = code.key_in_read_only(&plain(Key::Right), None);
    let _ = code.key_in_read_only(&plain(Key::Down), None);
    assert_eq!(code.text, "let x = 1;");
    assert_eq!(code.blocks(), &[Block::Code]);

    // RTL text navigates and selects without mutating.
    let mut rtl = TextEdit::new("مرحبا بالعالم");
    rtl.set_caret(0);
    let _ = rtl.key_in_read_only(&key(Key::End, shift), None);
    assert_eq!(rtl.selected(), "مرحبا بالعالم");
    let _ = rtl.key_in_read_only(&plain(Key::Backspace), None);
    assert_eq!(rtl.text, "مرحبا بالعالم");

    // Empty read-only field handles navigation, selection, copy, and blocked edits safely.
    let mut empty = TextEdit::new("");
    let _ = empty.key_in_read_only(&plain(Key::Left), None);
    let _ = empty.key_in_read_only(&plain(Key::Right), None);
    let _ = empty.key_in_read_only(&key(Key::Command('a'), cmd), None);
    let _ = empty.key_in_read_only(&plain(Key::Backspace), None);
    let _ = empty.key_in_read_only(&plain(Key::Text("a".into())), None);
    assert_eq!(empty.text, "");
}

#[test]
fn media_keys_are_ignored_by_editable_and_read_only_text_edits() {
    let all = [
        MediaKey::PlayPause,
        MediaKey::Play,
        MediaKey::Pause,
        MediaKey::Next,
        MediaKey::Previous,
        MediaKey::Stop,
        MediaKey::FastForward,
        MediaKey::Rewind,
    ];
    let mut e = TextEdit::new("hello world");
    e.set_selection(2, 5);
    let snap = e.clone();
    for mk in all {
        assert_eq!(e.key(&plain(Key::Media(mk)), None), EditOutcome::default());
        assert_eq!(e.key_in(&plain(Key::Media(mk)), None), EditOutcome::default());
        assert_eq!(e.key_in_read_only(&plain(Key::Media(mk)), None), EditOutcome::default());
        assert_eq!(e, snap, "media key {mk:?} must leave TextEdit untouched");
    }
}
