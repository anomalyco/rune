//! Headless Runtime tests for read-only `text_input` (`El::read_only(true)`), matching
//! HTML/Chrome `<input readonly>` and `<textarea readonly>` semantics.

use accesskit::{Action as Ak, ActionData, TextPosition, TextSelection};

use crate::{
    Block, ClipboardContent, Color, Cx, El, Env, Ime, Key, KeyEvent, Mods, PopupEntry, Runtime,
    TextEdit, TextStyle, col, row, text_input,
};

fn style() -> TextStyle {
    TextStyle::new(14.0, 400.0).lh(20.0)
}

fn plain(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn shift(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { shift: true, ..Mods::default() } }
}

fn cmd(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() } }
}

fn cmd_shift(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), shift: true, ..Mods::default() } }
}

fn click_key<S: 'static>(rt: &mut Runtime<S>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing element {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

fn caret_of<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.caret)
}

#[test]
fn read_only_field_has_caret_navigation_selection_and_copy_while_refusing_edits_and_ime() {
    struct State {
        ro: TextEdit,
        editable: TextEdit,
        changes: usize,
        selections: usize,
        submits: usize,
        pastes: usize,
        copy_calls: Vec<bool>,
        app_keys: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(12.0)
            .gap(12.0)
            .child(
                text_input(&s.ro, style())
                    .key("ro")
                    .label("Read-only preview")
                    .w(320.0)
                    .h(80.0)
                    .color(Color::WHITE)
                    .caret(Color::WHITE, Color { r: 0.2, g: 0.5, b: 1.0, a: 0.4 })
                    .read_only(true)
                    .spellcheck(true)
                    .on_edit(|s: &mut State| &mut s.ro)
                    .on_input_change(|s: &mut State, _| s.changes += 1)
                    .on_input_selection(|s: &mut State, _| s.selections += 1)
                    .on_submit(|s: &mut State, _| s.submits += 1)
                    .on_input_paste(|s: &mut State, _, _| {
                        s.pastes += 1;
                        true
                    })
                    .on_input_copy(|s: &mut State, cut, _| {
                        s.copy_calls.push(cut);
                        None
                    }),
            )
            .child(
                text_input(&s.editable, style())
                    .key("editable")
                    .label("Editable field")
                    .single_line()
                    .w(320.0)
                    .h(30.0)
                    .color(Color::WHITE)
                    .on_edit(|s: &mut State| &mut s.editable),
            )
    }

    let mut ro = TextEdit::new("Read-only").rich();
    // Seed undo history so Cmd/Ctrl+Z would mutate if unblocked.
    ro.set_caret(ro.text.len());
    ro.insert("
second line");
    ro.set_caret(0);

    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        State {
            ro,
            editable: TextEdit::new("normal"),
            changes: 0,
            selections: 0,
            submits: 0,
            pastes: 0,
            copy_calls: Vec::new(),
            app_keys: 0,
        },
        view,
        (380.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.on_key = Some(|s, _, _| {
        s.app_keys += 1;
        true
    });
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // Click focuses the read-only field, renders a caret, and keeps IME off.
    click_key(&mut rt, "ro");
    assert!(rt.state.ro.focused, "read-only field takes focus on click");
    assert!(caret_of(&rt, "ro").is_some(), "focused read-only field renders a caret");
    assert!(rt.ime_area.is_none(), "read-only field must not publish an IME area");
    assert!(rt.core().ime_text().is_none(), "read-only field must not publish IME surrounding text");
    assert!(rt.paste_target().is_none(), "read-only field must not be a paste target");

    // Issue #1882 repro sequence: End, Backspace, 'x', Cmd/Ctrl+A -> text intact, all selected, focused.
    let original = rt.state.ro.text.clone();
    let sel_before = rt.state.selections;
    rt.key(plain(Key::End));
    rt.frame(0.0);
    rt.key(plain(Key::Backspace));
    rt.frame(0.0);
    rt.key(plain(Key::Text("x".into())));
    rt.frame(0.0);
    rt.key(cmd('a'));
    rt.frame(0.0);
    assert_eq!(rt.state.ro.text, original);
    assert_eq!(rt.state.ro.selected(), original);
    assert!(rt.state.ro.focused);
    assert!(rt.state.selections > sel_before, "navigation and Select All fire on_input_selection");
    assert_eq!(rt.state.changes, 0, "read-only field never fires on_input_change");
    assert_eq!(rt.state.app_keys, 0, "blocked typing keys stay consumed by the focused read-only field");

    // Copy works via Cmd/Ctrl+C (invoking on_input_copy with cut = false); Cut does nothing.
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(board.content().unwrap().text(), original);
    assert_eq!(rt.state.copy_calls, [false]);

    board.set_content(ClipboardContent::new("sentinel"));
    rt.key(cmd('x'));
    rt.frame(0.0);
    assert_eq!(rt.state.ro.text, original, "Cut must not delete text");
    assert_eq!(board.content().unwrap().text(), "sentinel", "Cut must not overwrite clipboard");
    assert_eq!(rt.state.copy_calls, [false], "Cut must not invoke on_input_copy");

    // Keyboard navigation: Home, Shift+Right, Shift+word, Down, Up.
    rt.key(plain(Key::Home));
    rt.frame(0.0);
    rt.key(plain(Key::Up));
    rt.frame(0.0);
    rt.key(plain(Key::Home));
    rt.frame(0.0);
    assert_eq!((rt.state.ro.anchor, rt.state.ro.focus), (0, 0));
    for _ in 0..4 {
        rt.key(shift(Key::Right));
        rt.frame(0.0);
    }
    assert_eq!(rt.state.ro.selected(), "Read");
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(board.content().unwrap().text(), "Read");

    // Blocked mutating actions: Delete, Enter, Shift+Enter, Undo, Redo, Bold/Italic/Underline,
    // Cmd/Ctrl+V, Shift+Insert, direct Runtime::paste, and IME Preedit/Commit.
    let blocked_keys = [
        plain(Key::Delete),
        plain(Key::Enter),
        shift(Key::Enter),
        cmd('z'),
        cmd_shift('z'),
        cmd('y'),
        cmd('b'),
        cmd('i'),
        cmd('u'),
        cmd('v'),
        KeyEvent { key: Key::Insert, mods: Mods { shift: true, ..Mods::default() } },
    ];
    for ev in blocked_keys {
        rt.key(ev);
        rt.frame(0.0);
    }
    rt.paste("pasted");
    rt.frame(0.0);
    rt.paste_content(ClipboardContent::new("pasted-rich").with_html("<b>pasted-rich</b>"));
    rt.frame(0.0);
    rt.ime(Ime::Preedit("かな".into(), Some((0, 6)), Vec::new()));
    rt.frame(0.0);
    rt.ime(Ime::Commit("仮名".into()));
    rt.frame(0.0);

    assert_eq!(rt.state.ro.text, original);
    assert!(rt.state.ro.marks().is_empty(), "rich formatting chords must not add marks");
    assert!(rt.state.ro.preedit().is_none(), "IME must not open preedit");
    assert_eq!(rt.state.changes, 0);
    assert_eq!(rt.state.submits, 0);
    assert_eq!(rt.state.pastes, 0);

    // Tab moves focus out to the editable field, which still edits normally.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(!rt.state.ro.focused && rt.state.editable.focused, "Tab leaves read-only field for next control");
    assert!(rt.ime_area.is_some(), "editable field still publishes IME area");
    rt.key(plain(Key::Text("!".into())));
    rt.frame(0.0);
    assert_eq!(rt.state.editable.text, "normal!");
}

#[test]
fn read_only_field_in_code_block_tabs_focus_instead_of_inserting_tab() {
    struct State {
        code: TextEdit,
        after: TextEdit,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .gap(8.0)
            .child(
                text_input(&s.code, style())
                    .key("code")
                    .w(260.0)
                    .h(60.0)
                    .read_only(true)
                    .on_edit(|s: &mut State| &mut s.code),
            )
            .child(
                text_input(&s.after, style())
                    .key("after")
                    .single_line()
                    .w(260.0)
                    .h(30.0)
                    .on_edit(|s: &mut State| &mut s.after),
            )
    }

    let mut code = TextEdit::new("let x = 1;").rich();
    code.set_block(Block::Code);
    code.set_caret(code.text.len());
    code.focused = true;
    let mut rt = Runtime::new(
        State { code, after: TextEdit::new("") },
        view,
        (320.0, 140.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Right at end of trailing code block must not append a paragraph.
    rt.key(plain(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.state.code.text, "let x = 1;");

    // Tab inside a read-only code block moves focus instead of inserting '\t'.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert_eq!(rt.state.code.text, "let x = 1;");
    assert!(rt.state.after.focused);
}

#[test]
fn read_only_field_menu_disables_cut_and_paste_omits_spellcheck_and_keeps_copy() {
    struct State {
        ro: TextEdit,
        spellcheck: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .on_spellcheck(s.spellcheck, |s: &mut State, on, _| s.spellcheck = on)
            .child(
                text_input(&s.ro, style())
                    .key("ro")
                    .w(260.0)
                    .h(36.0)
                    .read_only(true)
                    .spellcheck(true)
                    .on_edit(|s: &mut State| &mut s.ro),
            )
    }

    let board = crate::clipboard::memory();
    let mut ro = TextEdit::new("teh value");
    ro.focused = true;
    ro.set_selection(0, 3);
    let mut rt = Runtime::new(
        State { ro, spellcheck: true },
        view,
        (300.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // Open field menu via ContextMenu key.
    rt.key(plain(Key::ContextMenu));
    rt.frame(0.0);
    let menu = rt.popup_menu().expect("read-only field still opens field context menu").clone();
    let ids: Vec<&str> = menu
        .entries()
        .iter()
        .filter_map(|e| match e {
            PopupEntry::Item(i) => Some(i.id()),
            _ => None,
        })
        .collect();

    assert!(menu.find("copy").is_some_and(|i| i.is_enabled()), "Copy is enabled for selection: {ids:?}");
    assert!(menu.find("cut").is_some_and(|i| !i.is_enabled()), "Cut is disabled on read-only field: {ids:?}");
    assert!(menu.find("paste").is_some_and(|i| !i.is_enabled()), "Paste is disabled on read-only field: {ids:?}");
    if !cfg!(target_os = "macos") {
        assert!(menu.find("select-all").is_some_and(|i| i.is_enabled()), "Select All is enabled: {ids:?}");
    }
    assert!(!ids.contains(&"spellcheck"), "Spellcheck toggle is omitted on read-only field: {ids:?}");
    assert!(!ids.contains(&"learn-spelling"), "Learn Spelling is omitted on read-only field: {ids:?}");
    assert!(!ids.iter().any(|id| id.starts_with("suggestion-")), "Suggestions are omitted: {ids:?}");

    // Picking Copy writes the selection to clipboard without modifying text.
    rt.resolve_popup(Some("copy")).unwrap();
    rt.frame(0.0);
    assert_eq!(board.content().unwrap().text(), "teh");
    assert_eq!(rt.state.ro.text, "teh value");
}

#[test]
fn read_only_a11y_node_and_actions_allow_focus_and_selection_but_block_value_edits() {
    struct State {
        direct: TextEdit,
        wrapped: TextEdit,
        disabled_ro: TextEdit,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .gap(8.0)
            .child(
                text_input(&s.direct, style())
                    .key("direct")
                    .label("Direct read-only")
                    .w(260.0)
                    .h(30.0)
                    .read_only(true)
                    .on_edit(|s: &mut State| &mut s.direct),
            )
            .child(
                row()
                    .key("wrap")
                    .role("textbox")
                    .read_only(true)
                    .w(260.0)
                    .h(30.0)
                    .child(
                        text_input(&s.wrapped, style())
                            .key("wrapped")
                            .label("Wrapped read-only")
                            .w(240.0)
                            .h(30.0)
                            .on_edit(|s: &mut State| &mut s.wrapped),
                    ),
            )
            .child(
                text_input(&s.disabled_ro, style())
                    .key("disabled-ro")
                    .label("Disabled read-only")
                    .w(260.0)
                    .h(30.0)
                    .read_only(true)
                    .disabled(true),
            )
    }

    let mut rt = Runtime::new(
        State {
            direct: TextEdit::new("hello world"),
            wrapped: TextEdit::new("wrapped text"),
            disabled_ro: TextEdit::new("locked"),
        },
        view,
        (300.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    for key in ["direct", "wrapped"] {
        let (_, node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).unwrap();
        assert!(node.is_read_only(), "{key} must report is_read_only");
        assert!(node.supports_action(Ak::Focus), "{key} must support Focus");
        assert!(node.supports_action(Ak::Click), "{key} must support Click");
        assert!(node.supports_action(Ak::SetTextSelection), "{key} must support SetTextSelection");
        assert!(!node.supports_action(Ak::SetValue), "{key} must NOT advertise SetValue");
        assert!(!node.supports_action(Ak::ReplaceSelectedText), "{key} must NOT advertise ReplaceSelectedText");
    }

    // Disabled + read-only stays disabled and unfocusable.
    let (_, dis_node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("disabled-ro")).unwrap();
    assert!(dis_node.is_disabled());
    assert!(dis_node.is_read_only());
    assert!(!dis_node.supports_action(Ak::Focus));

    // SetValue and ReplaceSelectedText are refused on both direct and ancestor-wrapped read-only fields.
    assert!(!rt.accessibility_action("direct", Ak::SetValue, Some(ActionData::Value("nope".into()))));
    assert!(!rt.accessibility_action("direct", Ak::ReplaceSelectedText, Some(ActionData::Value("nope".into()))));
    assert!(!rt.accessibility_action("wrapped", Ak::SetValue, Some(ActionData::Value("nope".into()))));
    assert!(!rt.accessibility_action("wrapped", Ak::ReplaceSelectedText, Some(ActionData::Value("nope".into()))));
    rt.frame(0.0);
    assert_eq!(rt.state.direct.text, "hello world");
    assert_eq!(rt.state.wrapped.text, "wrapped text");

    // Focus and SetTextSelection succeed on a read-only field.
    assert!(rt.accessibility_action("direct", Ak::Focus, None));
    rt.frame(0.0);
    assert!(rt.state.direct.focused);

    let tree = rt.accessibility_tree();
    let (direct_id, direct_node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("direct")).unwrap();
    let line_id = direct_node.children().first().copied().unwrap_or(*direct_id);
    assert!(rt.accessibility_action(
        "direct",
        Ak::SetTextSelection,
        Some(ActionData::SetTextSelection(TextSelection {
            anchor: TextPosition { node: line_id, character_index: 0 },
            focus: TextPosition { node: line_id, character_index: 5 },
        })),
    ));
    rt.frame(0.0);
    assert_eq!(rt.state.direct.selected(), "hello");
    assert_eq!(rt.state.direct.text, "hello world");
}

#[test]
fn read_only_field_keeps_web_editing_proxy_inactive_and_rejects_web_edits() {
    struct State {
        ro: TextEdit,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().h_full().child(
            text_input(&s.ro, style())
                .key("ro")
                .w(240.0)
                .h(32.0)
                .read_only(true)
                .on_edit(|s: &mut State| &mut s.ro),
        )
    }

    let mut ro = TextEdit::new("immutable");
    ro.focused = true;
    let mut rt = Runtime::new(State { ro }, view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    assert!(!rt.core().web_has_editable_focus(), "read-only field must not claim editable web focus");
    let mut mirror = crate::runtime::web_runtime::Mirror::default();
    mirror.key = "stale".into();
    assert!(rt.web_context(&mut mirror), "web_context clears mirror key for read-only field");
    assert!(mirror.key.is_empty());
    assert_eq!(rt.web_edit_context(&mut mirror), None);

    // Even if a browser edit event arrives for its path, web_edit rejects it.
    let path = rt.core().kbd.focus.owned().unwrap();
    let ev = serde_json::json!({
        "kind": "edit",
        "key": path,
        "base": 0,
        "expected": "immutable",
        "start": 0,
        "end": 9,
        "text": "hacked",
        "anchor": 6,
        "focus": 6,
    });
    rt.web_edit(&ev, &mut mirror);
    rt.frame(0.0);
    assert_eq!(rt.state.ro.text, "immutable");
}

#[test]
fn read_only_field_supports_pointer_drag_double_click_empty_edge_and_zero_commits_on_blocked_keys() {
    struct State {
        ro: TextEdit,
        empty: TextEdit,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(10.0)
            .gap(10.0)
            .child(
                text_input(&s.ro, style())
                    .key("ro")
                    .single_line()
                    .w(260.0)
                    .h(30.0)
                    .read_only(true)
                    .on_edit(|s: &mut State| &mut s.ro),
            )
            .child(
                text_input(&s.empty, style())
                    .key("empty")
                    .single_line()
                    .w(260.0)
                    .h(30.0)
                    .read_only(true)
                    .on_edit(|s: &mut State| &mut s.empty),
            )
    }

    let mut rt = Runtime::new(
        State {
            ro: TextEdit::new("alpha beta gamma"),
            empty: TextEdit::new(""),
        },
        view,
        (300.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Drag across the read-only field selects a range without mutating text.
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("ro")).unwrap().rect;
    let y = r[1] + r[3] * 0.5;
    rt.pointer_down(r[0] + 2.0, y);
    rt.frame(0.0);
    rt.pointer_move(r[0] + 50.0, y);
    rt.frame(0.0);
    rt.pointer_up(r[0] + 50.0, y);
    rt.frame(0.0);
    assert!(rt.state.ro.focused);
    assert!(!rt.state.ro.selected().is_empty(), "pointer drag selects text in read-only field");
    assert_eq!(rt.state.ro.text, "alpha beta gamma");

    // Blocked typing keys on a read-only field do not trigger redundant view commits.
    let commits_before = rt.commit.commits;
    rt.key(plain(Key::Text("z".into())));
    rt.frame(0.0);
    rt.key(plain(Key::Backspace));
    rt.frame(0.0);
    rt.key(plain(Key::Delete));
    rt.frame(0.0);
    rt.key(plain(Key::Enter));
    rt.frame(0.0);
    assert_eq!(rt.state.ro.text, "alpha beta gamma");
    assert_eq!(rt.commit.commits, commits_before, "blocked keys on a read-only field trigger zero view rebuilds");

    // Empty read-only field focuses, draws a caret, and ignores navigation/edit/copy safely.
    click_key(&mut rt, "empty");
    assert!(rt.state.empty.focused);
    assert!(caret_of(&rt, "empty").is_some());
    rt.key(plain(Key::Left));
    rt.frame(0.0);
    rt.key(plain(Key::Backspace));
    rt.frame(0.0);
    rt.key(plain(Key::Text("a".into())));
    rt.frame(0.0);
    assert_eq!(rt.state.empty.text, "");
}
