use super::{DirectCommit, KeyDown, should_direct_commit};
use crate::edit::Ime;
use crate::input::host_events::{KeyInput, ime_core, keyboard_core};
use crate::{Cx, El, Env, Key, KeyEvent, Mods, PhysicalKeyCode, Runtime, TextEdit, TextStyle, text_input};

fn key(chars: Option<&str>) -> KeyDown {
    KeyDown {
        view: 1,
        chars: chars.map(str::to_owned),
        chars_ignoring_mods: chars.map(str::to_owned),
        ctrl: false,
        cmd: false,
        alt: false,
        option_as_alt: false,
    }
}

#[test]
fn full_width_punctuation_without_marked_text_commits() {
    for (ascii, fullwidth) in [
        (",", "，"),
        (".", "。"),
        ("?", "？"),
        ("!", "！"),
        (":", "："),
        (";", "；"),
        ("\"", "“"),
        ("'", "‘"),
        ("^", "……"),
        ("_", "——"),
        ("\\", "、"),
        ("[", "【"),
        (" ", "\u{3000}"),
    ] {
        assert!(
            should_direct_commit(&key(Some(ascii)), false, fullwidth),
            "expected {ascii:?} -> {fullwidth:?} to direct-commit"
        );
    }
}

#[test]
fn plain_typing_and_marked_text_and_shortcuts_stay_unchanged() {
    // Plain Latin and half-width punctuation match the key event's characters.
    for ch in ["a", "A", ",", ".", "?", " ", "ü", "й"] {
        assert!(!should_direct_commit(&key(Some(ch)), false, ch), "{ch:?}");
    }
    // Marked text is already committed by winit's own insertText: path.
    assert!(!should_direct_commit(&key(Some(",")), true, "，"));
    // Empty and control characters are never direct commits.
    for bad in ["", "\r", "\n", "\t", "\u{1b}", "\u{7f}", "，\n"] {
        assert!(!should_direct_commit(&key(Some(",")), false, bad), "{bad:?}");
    }
    // Cmd and Ctrl chords never turn into IME commits.
    let mut cmd = key(Some("c"));
    cmd.cmd = true;
    assert!(!should_direct_commit(&cmd, false, "ç"));
    let mut ctrl = key(Some("c"));
    ctrl.ctrl = true;
    assert!(!should_direct_commit(&ctrl, false, "ç"));
    // Option text whose characters already match the NSEvent stays on KeyboardInput.
    let mut opt = KeyDown {
        view: 1,
        chars: Some("å".into()),
        chars_ignoring_mods: Some("a".into()),
        ctrl: false,
        cmd: false,
        alt: true,
        option_as_alt: false,
    };
    assert!(!should_direct_commit(&opt, false, "å"));
    assert!(!should_direct_commit(&opt, false, "a"));
    // With Option-as-Alt enabled, Option chords stay on KeyboardInput even if IME inserts text.
    opt.option_as_alt = true;
    assert!(!should_direct_commit(&opt, false, "α"));
}

#[test]
fn direct_commit_scopes_to_active_view_and_clears_on_end() {
    let mut dc = DirectCommit::new();

    // Outside keyDown:, insertText: is ignored and never leaks to a later key.
    dc.record_insert(1, false, "，");
    assert_eq!(dc.take(1), None);

    // Inside keyDown: on view 1, a substituted insertText: is taken once by view 1 only.
    dc.begin(key(Some(",")));
    dc.record_insert(2, false, "。"); // wrong view
    assert_eq!(dc.take(1), None);
    dc.record_insert(1, false, "，");
    assert_eq!(dc.take(2), None);
    assert_eq!(dc.take(1), Some("，".into()));
    assert_eq!(dc.take(1), None);
    dc.end();

    // Unconsumed commits are dropped when keyDown: exits.
    dc.begin(key(Some(".")));
    dc.record_insert(1, false, "。");
    dc.end();
    assert_eq!(dc.take(1), None);

    // Multiple substituted inserts in one keyDown: concatenate in order.
    dc.begin(key(Some("[")));
    dc.record_insert(1, false, "「");
    dc.record_insert(1, false, "」");
    assert_eq!(dc.take(1), Some("「」".into()));
    dc.end();
}

#[test]
fn direct_commit_inserts_fullwidth_punctuation_into_focused_text_input() {
    #[derive(Default)]
    struct Form {
        edit: TextEdit,
        keys: Vec<Key>,
        ups: Vec<Key>,
    }
    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        text_input(&s.edit, TextStyle::new(14.0, 400.0))
            .key("f")
            .label("Field")
            .size(200.0, 40.0)
            .on_edit(|s: &mut Form| &mut s.edit)
            .on_key_up(|s: &mut Form, ev, _| { s.ups.push(ev.key.clone()); true })
    }
    let mut edit = TextEdit::new("");
    edit.focused = true;
    let mut rt = Runtime::new(Form { edit, keys: Vec::new(), ups: Vec::new() }, view, (200.0, 40.0), Env::default());
    rt.on_key = Some(|s, ev, _| { s.keys.push(ev.key.clone()); false });
    rt.frame(0.0);

    let clipboard = crate::clipboard::Client::default();
    let mut dc = DirectCommit::new();

    // Simulate macOS Pinyin typing ',', '.', '?' without marked text: insertText: provides
    // full-width punctuation, so the matching KeyboardInput press routes as Ime::Commit and the
    // release of the key is suppressed.
    for (ascii, fullwidth, phys) in [
        (",", "，", PhysicalKeyCode::Comma),
        (".", "。", PhysicalKeyCode::Period),
        ("?", "？", PhysicalKeyCode::Slash),
    ] {
        dc.begin(key(Some(ascii)));
        dc.record_insert(1, false, fullwidth);
        if let Some(text) = dc.take(1) {
            ime_core(rt.core_mut(), None, Ime::Commit(text));
        } else {
            let (core, mut app) = rt.split_mut();
            keyboard_core(core, &mut app, &clipboard, KeyInput {
                physical: Some(phys),
                logical: Some(KeyEvent { key: Key::Text(ascii.into()), mods: Mods::default() }),
                base: ascii.chars().next(),
                mods: Mods::default(),
                pressed: true,
                repeat: false,
                unhandled: None,
            });
        }
        dc.end();
        rt.frame(0.0);
        // Matching keyUp arrives after keyDown:.
        {
            let (core, mut app) = rt.split_mut();
            keyboard_core(core, &mut app, &clipboard, KeyInput {
                physical: Some(phys),
                logical: Some(KeyEvent { key: Key::Text(ascii.into()), mods: Mods::default() }),
                base: ascii.chars().next(),
                mods: Mods::default(),
                pressed: false,
                repeat: false,
                unhandled: None,
            });
        }
        rt.frame(0.0);
    }

    // Plain ASCII 'a' (insertText: matches keyDown:) continues through KeyboardInput, typing
    // into the field and delivering its matching keyUp.
    dc.begin(key(Some("a")));
    dc.record_insert(1, false, "a");
    assert_eq!(dc.take(1), None);
    {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, &clipboard, KeyInput {
            physical: Some(PhysicalKeyCode::KeyA),
            logical: Some(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() }),
            base: Some('a'),
            mods: Mods::default(),
            pressed: true,
            repeat: false,
            unhandled: None,
        });
    }
    dc.end();
    rt.frame(0.0);
    {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, &clipboard, KeyInput {
            physical: Some(PhysicalKeyCode::KeyA),
            logical: Some(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() }),
            base: Some('a'),
            mods: Mods::default(),
            pressed: false,
            repeat: false,
            unhandled: None,
        });
    }
    rt.frame(0.0);

    // Cmd shortcut never direct-commits and still reaches on_key.
    let mut cmd_k = key(Some("k"));
    cmd_k.cmd = true;
    dc.begin(cmd_k);
    dc.record_insert(1, false, "˚");
    assert_eq!(dc.take(1), None);
    {
        let cmd_mods = Mods { cmd: true, ..Mods::default() };
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, &clipboard, KeyInput {
            physical: Some(PhysicalKeyCode::KeyK),
            logical: Some(KeyEvent { key: Key::Command('k'), mods: cmd_mods }),
            base: Some('k'),
            mods: cmd_mods,
            pressed: true,
            repeat: false,
            unhandled: None,
        });
    }
    dc.end();
    rt.frame(0.0);

    assert_eq!(rt.state.edit.text, "，。？a");
    assert_eq!(rt.state.keys, [Key::Command('k')]);
    assert_eq!(rt.state.ups, [Key::Text("a".into())], "only non-IME key releases reach on_key_up");
}
