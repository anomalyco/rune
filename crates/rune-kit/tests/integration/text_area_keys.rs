//! `TextArea::line_home_end(bool)` and `Shift+Insert` paste integration tests (including
//! wrapped lines, RTL text, disabled state, `on_input_paste`, and accessibility tree checks).

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, col};
use rune_kit::{Kit, TextArea, TextInput};

struct CodeApp {
    code: TextEdit,
    native: TextEdit,
    disabled: TextEdit,
    single: TextEdit,
    line_home_end: bool,
    intercepted: Vec<String>,
}

fn view(s: &CodeApp, _: &mut Cx<CodeApp>) -> El {
    let k = Kit::neutral(true);
    col()
        .size(320.0, 360.0)
        .padding(8.0)
        .gap(8.0)
        .child(
            TextArea::new(&s.code)
                .label("Code editor")
                .description("Line-based Home and End")
                .width(Some(140.0))
                .rows(4)
                .line_home_end(s.line_home_end)
                .on_edit(|s: &mut CodeApp| &mut s.code)
                .build(&k)
                .key("code"),
        )
        .child(
            TextArea::new(&s.native)
                .label("Notes")
                .width(Some(140.0))
                .rows(3)
                .on_edit(|s: &mut CodeApp| &mut s.native)
                .build(&k)
                .key("native"),
        )
        .child(
            TextArea::new(&s.disabled)
                .label("Disabled editor")
                .width(Some(140.0))
                .rows(2)
                .line_home_end(true)
                .disabled(true)
                .on_edit(|s: &mut CodeApp| &mut s.disabled)
                .build(&k)
                .key("disabled"),
        )
        .child(
            TextInput::new(&s.single)
                .label("Filter")
                .width(Some(140.0))
                .on_edit(|s: &mut CodeApp| &mut s.single)
                .on_input_paste(|s: &mut CodeApp, c, _| {
                    s.intercepted.push(c.text().to_owned());
                    false
                })
                .build(&k)
                .key("single"),
        )
}

fn find_node<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if v["key"].as_str() == Some(key) {
        return Some(v);
    }
    v["children"].as_array()?.iter().find_map(|c| find_node(c, key))
}

#[test]
fn text_area_line_home_end_navigation_selection_and_accessibility() {
    let mut code = TextEdit::new("first\nsecond");
    code.set_caret(2);
    code.focused = true;
    let mut native = TextEdit::new("first\nsecond");
    native.set_caret(2);
    let disabled = TextEdit::new("locked\nline");
    let mut rt = Runtime::new(
        CodeApp {
            code,
            native,
            disabled,
            single: TextEdit::default(),
            line_home_end: true,
            intercepted: Vec::new(),
        },
        view,
        (320.0, 360.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Accessibility tree keeps textbox role, multiline state, label, and description intact.
    let tree = rt.accessibility_json();
    let code_box = find_node(&tree, "code").expect("code TextArea in a11y tree");
    let code_input = find_node(code_box, "input").expect("inner textbox in a11y tree");
    assert_eq!(code_input["role"].as_str(), Some("multiline-text-input"));
    assert_eq!(code_input["name"].as_str(), Some("Code editor"));
    assert_eq!(code_input["description"].as_str(), Some("Line-based Home and End"));

    assert!(rt.state.code.line_home_end());
    assert!(!rt.state.native.line_home_end(), "default TextArea keeps line_home_end false");

    // Plain End on line 1 moves to end of "first", then typing '!' gives "first!\nsecond".
    rt.key(KeyEvent::from(Key::End));
    rt.key(KeyEvent::from(Key::Text("!".into())));
    rt.frame(0.0);
    assert_eq!(rt.state.code.text, "first!\nsecond");

    // Shift+Home selects line 1 only.
    rt.key(KeyEvent { key: Key::Home, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.code.selected(), "first!");

    // Cmd+Down / Cmd+Up (macOS) or Ctrl+End / Ctrl+Home still reach document ends.
    let doc_mods = if cfg!(target_os = "macos") {
        Mods { cmd: true, ..Mods::default() }
    } else {
        Mods { ctrl: true, cmd: true, ..Mods::default() }
    };
    rt.key(KeyEvent { key: Key::End, mods: doc_mods });
    rt.frame(0.0);
    assert_eq!(rt.state.code.focus, rt.state.code.text.len());
    rt.key(KeyEvent { key: Key::Home, mods: doc_mods });
    rt.frame(0.0);
    assert_eq!(rt.state.code.focus, 0);

    // Default TextArea without line_home_end preserves platform default.
    rt.update(|s, _| {
        s.code.focused = false;
        s.native.focused = true;
        s.native.set_caret(2);
    });
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::End));
    rt.frame(0.0);
    if cfg!(target_os = "macos") {
        assert_eq!(rt.state.native.focus, rt.state.native.text.len());
    } else {
        assert_eq!(rt.state.native.focus, 5);
    }

    // Disabled TextArea with line_home_end(true) does not accept focus or edits.
    rt.update(|s, _| {
        s.native.focused = false;
        s.disabled.focused = true;
    });
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::End));
    rt.key(KeyEvent::from(Key::Text("x".into())));
    rt.frame(0.0);
    assert_eq!(rt.state.disabled.text, "locked\nline");
}

#[test]
fn shift_insert_pastes_into_kit_text_area_and_text_input_off_macos() {
    let mut code = TextEdit::new("hi ");
    code.focused = true;
    let m = rune::clipboard::memory();
    m.set_text("🎉");
    let mut rt = Runtime::new(
        CodeApp {
            code,
            native: TextEdit::default(),
            disabled: TextEdit::new("locked"),
            single: TextEdit::default(),
            line_home_end: true,
            intercepted: Vec::new(),
        },
        view,
        (320.0, 360.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&m);
    rt.frame(0.0);

    let shift_ins = KeyEvent { key: Key::Insert, mods: Mods { shift: true, ..Mods::default() } };
    rt.key(shift_ins.clone());
    rt.frame(0.0);
    rt.frame(0.0);
    if cfg!(target_os = "macos") {
        assert_eq!(rt.state.code.text, "hi ");
    } else {
        assert_eq!(rt.state.code.text, "hi 🎉");
    }

    // Also works in TextInput and routes through on_input_paste.
    rt.update(|s, _| {
        s.code.focused = false;
        s.single.focused = true;
    });
    rt.frame(0.0);
    rt.key(shift_ins);
    rt.frame(0.0);
    rt.frame(0.0);
    if cfg!(target_os = "macos") {
        assert_eq!(rt.state.single.text, "");
        assert!(rt.state.intercepted.is_empty());
    } else {
        assert_eq!(rt.state.single.text, "🎉");
        assert_eq!(rt.state.intercepted, ["🎉"]);
    }
}
