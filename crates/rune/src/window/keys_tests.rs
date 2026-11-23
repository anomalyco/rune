use super::*;
use crate::{Cx, El, Env, PhysicalInput, PhysicalKeyCode, Runtime, TextEdit, TextStyle, text_input};

#[derive(Default)]
struct Field { edit: TextEdit, opened: bool }

fn field(s: &Field, _: &mut Cx<Field>) -> El {
    text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("reply").size(200.0, 40.0)
        .on_edit(|s: &mut Field| &mut s.edit)
}

#[test]
fn dvorak_shift_command_o_reaches_the_app_with_a_field_focused() {
    let mut rt = Runtime::new(Field::default(), field, (200.0, 40.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| {
        if ev.key == Key::Command('o') && ev.mods.shift {
            s.opened = true;
            return true;
        }
        false
    });
    rt.frame(0.0);
    let (x, y) = rt.find("reply").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    // Dvorak O lives on ANSI S; the logical chord must win over that position.
    let command = if cfg!(target_os = "macos") { ModifiersState::SUPER } else { ModifiersState::CONTROL };
    let logical = map_key(&WKey::Character("O".into()), command | ModifiersState::SHIFT).unwrap();
    rt.physical_with_fallback(PhysicalInput::Key {
        key: PhysicalKeyCode::KeyS, mods: logical.mods, pressed: true, repeat: false,
    }, Some(logical));
    rt.frame(0.0);
    assert!(rt.state.opened);
    assert!(rt.state.edit.text.is_empty(), "the chord did not type into the field");
}

#[test]
fn dvorak_paste_uses_the_logical_character_not_the_physical_period() {
    let mut rt = Runtime::new(Field::default(), field, (200.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("reply").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    // macOS supplies charactersIgnoringModifiers="v" at ANSI Period (47)
    // on Dvorak, including a synthesized Command-V from a dictation app.
    let command = if cfg!(target_os = "macos") { ModifiersState::SUPER } else { ModifiersState::CONTROL };
    let logical = map_key(&WKey::Character("v".into()), command).unwrap();
    assert_eq!(logical.key, Key::Command('v'));
    rt.physical_with_fallback(PhysicalInput::Key {
        key: PhysicalKeyCode::Period, mods: logical.mods, pressed: true, repeat: false,
    }, Some(logical));
    rt.frame(0.0);
    let target = rt.edit.paste_request.take().expect("Dvorak Command-V requests the clipboard");
    rt.paste_result(target, crate::ClipboardContent::new("Dictated reply"));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "Dictated reply");
}

#[test]
fn space_that_types_zwnj_arrives_as_zwnj_and_inserts_into_a_field() {
    use winit::keyboard::{KeyCode, NamedKey};

    let space = PhysicalKey::Code(KeyCode::Space);
    for windows in [false, true] {
        let ev = map_parts(windows, space, &WKey::Named(NamedKey::Space), Some("\u{200c}"), ModifiersState::SHIFT).unwrap();
        assert_eq!(ev.key, Key::Text("\u{200c}".into()));
        assert!(ev.mods.shift);
    }

    let mut rt = Runtime::new(Field::default(), field, (200.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("reply").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    let zwnj = map_parts(cfg!(target_os = "windows"), space, &WKey::Named(NamedKey::Space), Some("\u{200c}"), ModifiersState::SHIFT).unwrap();
    rt.key(KeyEvent { key: Key::Text("می".into()), mods: Mods::default() });
    rt.key(zwnj);
    rt.key(KeyEvent { key: Key::Text("شود".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "می\u{200c}شود");
}

#[test]
fn plain_space_named_keys_and_command_chords_keep_their_keys() {
    use winit::keyboard::{KeyCode, NamedKey};

    let space = PhysicalKey::Code(KeyCode::Space);
    for text in [Some(" "), None, Some(""), Some("\0")] {
        let ev = map_parts(false, space, &WKey::Named(NamedKey::Space), text, ModifiersState::empty()).unwrap();
        assert_eq!(ev.key, Key::Text(" ".into()), "text={text:?}");
    }
    for (named, text, expected) in [
        (NamedKey::Enter, Some("\r"), Key::Enter),
        (NamedKey::Enter, Some("x"), Key::Enter),
        (NamedKey::Tab, Some("\t"), Key::Tab),
        (NamedKey::Backspace, Some("\u{8}"), Key::Backspace),
    ] {
        let ev = map_parts(false, PhysicalKey::Code(KeyCode::Enter), &WKey::Named(named), text, ModifiersState::empty()).unwrap();
        assert_eq!(ev.key, expected);
    }

    // Super and Ctrl chords on Space stay Space so Cmd+Space / Ctrl+Space match `is_command(' ')`.
    for mods in [
        ModifiersState::SUPER,
        ModifiersState::SUPER | ModifiersState::SHIFT,
        ModifiersState::CONTROL,
        ModifiersState::CONTROL | ModifiersState::SHIFT,
    ] {
        let ev = map_parts(false, space, &WKey::Named(NamedKey::Space), Some("\u{200c}"), mods).unwrap();
        assert_eq!(ev.key, Key::Text(" ".into()), "mods={mods:?}");
    }

    // Option+Space types NBSP on macOS; off macOS Alt+Space stays Space for window menu / chords.
    let alt_space = map_parts(false, space, &WKey::Named(NamedKey::Space), Some("\u{00a0}"), ModifiersState::ALT).unwrap();
    let expected_alt_space = if cfg!(target_os = "macos") { "\u{00a0}" } else { " " };
    assert_eq!(alt_space.key, Key::Text(expected_alt_space.into()));

    // AltGr+Space (Ctrl+Alt) types off macOS; Ctrl+Option+Space is a chord on macOS.
    let ctrl_alt_space = map_parts(
        false,
        space,
        &WKey::Named(NamedKey::Space),
        Some("\u{00a0}"),
        ModifiersState::CONTROL | ModifiersState::ALT,
    ).unwrap();
    let expected_ctrl_alt = if cfg!(target_os = "macos") { " " } else { "\u{00a0}" };
    assert_eq!(ctrl_alt_space.key, Key::Text(expected_ctrl_alt.into()));

    // Alt+letter off macOS stays a chord on its logical key, matching #1471.
    let alt_d = map_parts(false, PhysicalKey::Code(KeyCode::KeyD), &WKey::Character("d".into()), Some("∂"), ModifiersState::ALT).unwrap();
    let expected_alt_d = if cfg!(target_os = "macos") { "∂" } else { "d" };
    assert_eq!(alt_d.key, Key::Text(expected_alt_d.into()));

    // text_with_all_modifiers wins for printable/format text (ZWNJ), while control bytes fall back.
    assert_eq!(pick_typed_text(Some("\u{200c}"), Some(" ")), Some("\u{200c}"));
    assert_eq!(pick_typed_text(Some("\x01"), Some("a")), Some("a"));
    assert_eq!(pick_typed_text(Some("\0"), Some(" ")), Some(" "));
    assert_eq!(pick_typed_text(Some(""), Some(" ")), Some(" "));
    assert_eq!(pick_typed_text(None, Some(" ")), Some(" "));
    assert_eq!(pick_typed_text(None, None), None);
}

#[test]
fn dead_key_sequences_type_their_text_and_insert_into_a_field() {
    use winit::keyboard::{KeyCode, NamedKey};
    let code = PhysicalKey::Code;
    let ch = |s: &str| WKey::Character(s.into());
    let plain = ModifiersState::empty();
    let shift = ModifiersState::SHIFT;
    let altgr = ModifiersState::CONTROL | ModifiersState::ALT;

    // First press of a dead key carries no text and types nothing.
    assert_eq!(map_parts(true, code(KeyCode::Quote), &WKey::Dead(Some('\'')), None, plain), None);

    // Dead key followed by Space, another dead key, a non-combining letter, a combined
    // character, or Right-Alt AltGr (whose modifiers winit clears).
    let cases = [
        (code(KeyCode::Space), WKey::Named(NamedKey::Space), "'", plain),
        (code(KeyCode::Space), WKey::Named(NamedKey::Space), "\"", shift),
        (code(KeyCode::Quote), WKey::Dead(Some('\'')), "''", plain),
        (code(KeyCode::Quote), WKey::Dead(Some('"')), "'\"", shift),
        (code(KeyCode::Digit6), WKey::Dead(None), "^^", shift),
        (code(KeyCode::KeyS), ch("s"), "'s", plain),
        (code(KeyCode::KeyA), ch("á"), "á", plain),
        (code(KeyCode::KeyQ), ch("@"), "'@", plain),
    ];
    for (physical, logical, text, mods) in cases {
        assert_eq!(
            map_parts(true, physical, &logical, Some(text), mods),
            Some(KeyEvent { key: Key::Text(text.into()), mods: map_mods(mods) }),
            "{logical:?} {text:?} {mods:?}",
        );
    }

    // Left Ctrl + Left Alt AltGr dead key twice types off macOS (chord on macOS).
    let altgr_dead = map_parts(true, code(KeyCode::BracketRight), &WKey::Dead(Some('~')), Some("~~"), altgr);
    let expected_altgr_dead = (!cfg!(target_os = "macos")).then(|| KeyEvent { key: Key::Text("~~".into()), mods: map_mods(altgr) });
    assert_eq!(altgr_dead, expected_altgr_dead);

    // Left Ctrl + Left Alt AltGr character after a dead key on Windows.
    if cfg!(target_os = "windows") {
        assert_eq!(
            map_parts(true, code(KeyCode::KeyQ), &ch("@"), Some("'@"), altgr),
            Some(KeyEvent { key: Key::Text("'@".into()), mods: map_mods(altgr) }),
        );
    }

    // Chords and control text on a dead key never type text.
    for mods in [ModifiersState::CONTROL, ModifiersState::SUPER] {
        assert_eq!(map_parts(true, code(KeyCode::Quote), &WKey::Dead(Some('\'')), Some("''"), mods), None, "{mods:?}");
    }
    if !cfg!(target_os = "macos") {
        assert_eq!(map_parts(true, code(KeyCode::Quote), &WKey::Dead(Some('\'')), Some("''"), ModifiersState::ALT), None);
    }
    for text in ["", "\r", "'\n", "\u{1b}"] {
        assert_eq!(map_parts(true, code(KeyCode::Quote), &WKey::Dead(Some('\'')), Some(text), plain), None, "{text:?}");
    }

    // End-to-end into a focused field: ' + Space, ' + ', ' + s, ' + a.
    let mut rt = Runtime::new(Field::default(), field, (200.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("reply").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    let send = |rt: &mut Runtime<Field>, key: KeyCode, logical: WKey, text: Option<&str>, mods: ModifiersState| {
        let ev = map_parts(cfg!(target_os = "windows"), code(key), &logical, text, mods);
        rt.physical_with_fallback(
            PhysicalInput::Key { key, mods: map_mods(mods), pressed: true, repeat: false },
            ev,
        );
        rt.frame(0.0);
    };
    send(&mut rt, KeyCode::Quote, WKey::Dead(Some('\'')), None, plain);
    send(&mut rt, KeyCode::Space, WKey::Named(NamedKey::Space), Some("'"), plain);
    send(&mut rt, KeyCode::Quote, WKey::Dead(Some('\'')), None, plain);
    send(&mut rt, KeyCode::Quote, WKey::Dead(Some('\'')), Some("''"), plain);
    send(&mut rt, KeyCode::Quote, WKey::Dead(Some('\'')), None, plain);
    send(&mut rt, KeyCode::KeyS, ch("s"), Some("'s"), plain);
    send(&mut rt, KeyCode::Quote, WKey::Dead(Some('\'')), None, plain);
    send(&mut rt, KeyCode::KeyA, ch("á"), Some("á"), plain);
    assert_eq!(rt.state.edit.text, "''''sá");
}
