use rune::accesskit::{Action as Ak, ActionData};
use rune::{El, Env, Key, KeyEvent, Mods, PopupEntry, Runtime, TextEdit, col};
use rune_kit::{Kit, TextArea, TextInput};

fn plain(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn shift(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { shift: true, ..Mods::default() } }
}

fn cmd(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() } }
}

#[test]
fn kit_text_input_and_text_area_read_only_forward_full_readonly_semantics() {
    struct State {
        single: TextEdit,
        multi: TextEdit,
        sel_events: usize,
        changes: usize,
    }

    fn view(s: &State, _: &mut rune::Cx<State>) -> El {
        let k = Kit::neutral(true);
        col()
            .w_full()
            .h_full()
            .padding(12.0)
            .gap(12.0)
            .child(
                TextInput::new(&s.single)
                    .label("Endpoint")
                    .read_only(true)
                    .width(Some(300.0))
                    .on_edit(|s: &mut State| &mut s.single)
                    .on_change(|s: &mut State, _| s.changes += 1)
                    .on_input_selection(|s: &mut State, _| s.sel_events += 1)
                    .build(&k)
                    .key("ro-single"),
            )
            .child(
                TextArea::new(&s.multi)
                    .label("Preview")
                    .read_only(true)
                    .width(Some(300.0))
                    .rows(3)
                    .on_edit(|s: &mut State| &mut s.multi)
                    .on_change(|s: &mut State, _| s.changes += 1)
                    .on_input_selection(|s: &mut State, _| s.sel_events += 1)
                    .build(&k)
                    .key("ro-multi"),
            )
    }

    let board = rune::clipboard::memory();
    let mut rt = Runtime::new(
        State {
            single: TextEdit::new("https://example.com/api"),
            multi: TextEdit::new("line one
line two"),
            sel_events: 0,
            changes: 0,
        },
        view,
        (360.0, 220.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // Click focuses TextInput, moves caret, selects with Shift+Right, copies with Cmd/Ctrl+C, blocks typing/Cut/Paste.
    let (x, y) = rt.find("ro-single").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert!(rt.state.single.focused);
    assert!(rt.ime_area.is_none());

    rt.key(plain(Key::Home));
    rt.frame(0.0);
    for _ in 0..5 {
        rt.key(shift(Key::Right));
        rt.frame(0.0);
    }
    assert_eq!(rt.state.single.selected(), "https");
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(board.content().unwrap().text(), "https");

    rt.key(plain(Key::Text("X".into())));
    rt.frame(0.0);
    rt.key(plain(Key::Backspace));
    rt.frame(0.0);
    rt.key(cmd('x'));
    rt.frame(0.0);
    rt.paste("overwrite");
    rt.frame(0.0);
    assert_eq!(rt.state.single.text, "https://example.com/api");
    assert_eq!(rt.state.changes, 0);
    assert!(rt.state.sel_events > 0);

    // Tab moves focus to read-only TextArea; Cmd/Ctrl+A + Cmd/Ctrl+C works while edits are blocked.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(rt.state.multi.focused);
    rt.key(cmd('a'));
    rt.frame(0.0);
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(board.content().unwrap().text(), "line one
line two");
    rt.key(plain(Key::Enter));
    rt.frame(0.0);
    rt.key(plain(Key::Delete));
    rt.frame(0.0);
    assert_eq!(rt.state.multi.text, "line one
line two");

    // Field context menu on read-only TextArea has Copy enabled and Cut/Paste disabled.
    rt.key(plain(Key::ContextMenu));
    rt.frame(0.0);
    let menu = rt.popup_menu().expect("field menu opens on read-only TextArea").clone();
    assert!(menu.find("copy").is_some_and(|i| i.is_enabled()));
    assert!(menu.find("cut").is_some_and(|i| !i.is_enabled()));
    assert!(menu.find("paste").is_some_and(|i| !i.is_enabled()));
    let ids: Vec<&str> = menu.entries().iter().filter_map(|e| match e {
        PopupEntry::Item(i) => Some(i.id()),
        _ => None,
    }).collect();
    assert!(!ids.contains(&"spellcheck"));

    // AccessKit reports read-only and refuses SetValue on both kit controls.
    let tree = rt.accessibility_tree();
    let ro_inputs: Vec<_> = tree
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.label(), Some("Endpoint" | "Preview")))
        .collect();
    assert_eq!(ro_inputs.len(), 2);
    for (_, node) in ro_inputs {
        assert!(node.is_read_only());
        assert!(node.supports_action(Ak::Focus));
        assert!(node.supports_action(Ak::SetTextSelection));
        assert!(!node.supports_action(Ak::SetValue));
        assert!(!node.supports_action(Ak::ReplaceSelectedText));
    }
    assert!(!rt.accessibility_action("input", Ak::SetValue, Some(ActionData::Value("no".into()))));
}
