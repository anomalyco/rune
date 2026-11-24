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

#[test]
fn web_key_and_base_table_matches_chrome_and_native_across_layouts() {
    use crate::{Chord, KeyPlatform};
    use winit::keyboard::{KeyCode, NamedKey, NativeKey};

    struct Case {
        label: &'static str,
        dom_key: &'static str,
        dom_code: &'static str,
        physical: PhysicalKeyCode,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
        platform: KeyPlatform,
        expected_key: Option<Key>,
        expected_base: Option<char>,
        /// Whether `bridge.js` forwards this event to `Runtime::web_key` from a focused field.
        via_web_key: bool,
        match_chords: &'static [&'static str],
        reject_chords: &'static [&'static str],
    }

    let cases = [
        // Non-ASCII letters on non-US layouts (Ctrl / Cmd, plain and shifted)
        Case {
            label: "German Ctrl+Ö on Semicolon",
            dom_key: "ö", dom_code: "Semicolon", physical: KeyCode::Semicolon,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command(';')), expected_base: Some('ö'), via_web_key: true,
            match_chords: &["ctrl+ö", "ctrl+;"], reject_chords: &["ctrl+o", "ctrl+shift+ö"],
        },
        Case {
            label: "German Ctrl+Shift+Ö on Semicolon",
            dom_key: "Ö", dom_code: "Semicolon", physical: KeyCode::Semicolon,
            shift: true, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command(';')), expected_base: Some('ö'), via_web_key: true,
            match_chords: &["ctrl+shift+ö", "ctrl+shift+;"], reject_chords: &["ctrl+ö", "ctrl+shift+o"],
        },
        Case {
            label: "German Ctrl+Ö with CapsLock (uppercase key without Shift)",
            dom_key: "Ö", dom_code: "Semicolon", physical: KeyCode::Semicolon,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command(';')), expected_base: Some('ö'), via_web_key: true,
            match_chords: &["ctrl+ö", "ctrl+;"], reject_chords: &["ctrl+o"],
        },
        Case {
            label: "German Cmd+Ö on macOS web",
            dom_key: "ö", dom_code: "Semicolon", physical: KeyCode::Semicolon,
            shift: false, ctrl: false, alt: false, meta: true, platform: KeyPlatform::Mac,
            expected_key: Some(Key::Command(';')), expected_base: Some('ö'), via_web_key: true,
            match_chords: &["mod+ö", "mod+;"], reject_chords: &["mod+o"],
        },
        Case {
            label: "German Cmd+Shift+ö on macOS web (Chrome keeps lowercase letter under Meta)",
            dom_key: "ö", dom_code: "Semicolon", physical: KeyCode::Semicolon,
            shift: true, ctrl: false, alt: false, meta: true, platform: KeyPlatform::Mac,
            expected_key: Some(Key::Command(';')), expected_base: Some('ö'), via_web_key: true,
            match_chords: &["mod+shift+ö", "mod+shift+;"], reject_chords: &["mod+ö"],
        },
        Case {
            label: "Russian Ctrl+М on KeyV",
            dom_key: "м", dom_code: "KeyV", physical: KeyCode::KeyV,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('v')), expected_base: Some('м'), via_web_key: true,
            match_chords: &["ctrl+v", "ctrl+м"], reject_chords: &["ctrl+m", "ctrl+c"],
        },
        Case {
            label: "Russian Ctrl+Shift+М on KeyV",
            dom_key: "М", dom_code: "KeyV", physical: KeyCode::KeyV,
            shift: true, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('v')), expected_base: Some('м'), via_web_key: true,
            match_chords: &["ctrl+shift+v", "ctrl+shift+м"], reject_chords: &["ctrl+v", "ctrl+м"],
        },
        Case {
            label: "Greek Ctrl+β on KeyB",
            dom_key: "β", dom_code: "KeyB", physical: KeyCode::KeyB,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Linux,
            expected_key: Some(Key::Command('b')), expected_base: Some('β'), via_web_key: true,
            match_chords: &["ctrl+b", "ctrl+β"], reject_chords: &["ctrl+v"],
        },
        Case {
            label: "French AZERTY Ctrl+é on Digit2",
            dom_key: "é", dom_code: "Digit2", physical: KeyCode::Digit2,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('2')), expected_base: Some('é'), via_web_key: true,
            match_chords: &["ctrl+é", "ctrl+2"], reject_chords: &["ctrl+e"],
        },
        Case {
            label: "German Ctrl+ß on Minus",
            dom_key: "ß", dom_code: "Minus", physical: KeyCode::Minus,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('-')), expected_base: Some('ß'), via_web_key: true,
            match_chords: &["ctrl+ß", "ctrl+-"], reject_chords: &["ctrl+s"],
        },
        // ASCII letters on US and non-US layouts
        Case {
            label: "US Ctrl+A",
            dom_key: "a", dom_code: "KeyA", physical: KeyCode::KeyA,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('a')), expected_base: Some('a'), via_web_key: true,
            match_chords: &["ctrl+a"], reject_chords: &["ctrl+shift+a"],
        },
        Case {
            label: "US Ctrl+Shift+A",
            dom_key: "A", dom_code: "KeyA", physical: KeyCode::KeyA,
            shift: true, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('a')), expected_base: Some('a'), via_web_key: true,
            match_chords: &["ctrl+shift+a"], reject_chords: &["ctrl+a"],
        },
        Case {
            label: "French AZERTY Ctrl+A on KeyQ",
            dom_key: "a", dom_code: "KeyQ", physical: KeyCode::KeyQ,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('a')), expected_base: Some('a'), via_web_key: true,
            match_chords: &["ctrl+a"], reject_chords: &["ctrl+q"],
        },
        Case {
            label: "French AZERTY Ctrl+M on Semicolon",
            dom_key: "m", dom_code: "Semicolon", physical: KeyCode::Semicolon,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('m')), expected_base: Some('m'), via_web_key: true,
            match_chords: &["ctrl+m"], reject_chords: &["ctrl+;"],
        },
        // Layouts where the same physical key gives punctuation (unshifted)
        Case {
            label: "German Ctrl++ on BracketRight",
            dom_key: "+", dom_code: "BracketRight", physical: KeyCode::BracketRight,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('+')), expected_base: Some('+'), via_web_key: true,
            match_chords: &["ctrl+plus"], reject_chords: &["ctrl+]"],
        },
        Case {
            label: "German Ctrl+- on Slash",
            dom_key: "-", dom_code: "Slash", physical: KeyCode::Slash,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('-')), expected_base: Some('-'), via_web_key: true,
            match_chords: &["ctrl+minus"], reject_chords: &["ctrl+/"],
        },
        Case {
            label: "Dvorak Ctrl+' on KeyQ (US letter key gives punctuation)",
            dom_key: "'", dom_code: "KeyQ", physical: KeyCode::KeyQ,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('\'')), expected_base: Some('\''), via_web_key: true,
            match_chords: &["ctrl+'"], reject_chords: &["ctrl+q"],
        },
        Case {
            label: "French AZERTY Ctrl+, on KeyM (US letter key gives punctuation)",
            dom_key: ",", dom_code: "KeyM", physical: KeyCode::KeyM,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command(',')), expected_base: Some(','), via_web_key: true,
            match_chords: &["ctrl+comma"], reject_chords: &["ctrl+m"],
        },
        Case {
            label: "Russian Ctrl+. on Slash (punctuation moved from Period)",
            dom_key: ".", dom_code: "Slash", physical: KeyCode::Slash,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('.')), expected_base: Some('.'), via_web_key: true,
            match_chords: &["ctrl+period"], reject_chords: &["ctrl+/"],
        },
        Case {
            label: "Spanish Ctrl+¡ on Equal (unshifted non-ASCII punctuation)",
            dom_key: "¡", dom_code: "Equal", physical: KeyCode::Equal,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('=')), expected_base: Some('¡'), via_web_key: true,
            match_chords: &["ctrl+¡", "ctrl+="], reject_chords: &["ctrl+!"],
        },
        // Shifted punctuation and digits must NOT misreport the shifted glyph as key_base
        Case {
            label: "US Ctrl+Shift+= produces '+' with key_base None",
            dom_key: "+", dom_code: "Equal", physical: KeyCode::Equal,
            shift: true, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('+')), expected_base: None, via_web_key: true,
            match_chords: &["ctrl+plus", "ctrl+shift+="], reject_chords: &["ctrl+="],
        },
        Case {
            label: "US Ctrl+Shift+[ produces '{' with key_base None",
            dom_key: "{", dom_code: "BracketLeft", physical: KeyCode::BracketLeft,
            shift: true, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('{')), expected_base: None, via_web_key: true,
            match_chords: &["ctrl+shift+[", "ctrl+{"], reject_chords: &["ctrl+["],
        },
        Case {
            label: "German Ctrl+Shift+7 produces '/' with key_base None",
            dom_key: "/", dom_code: "Digit7", physical: KeyCode::Digit7,
            shift: true, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('/')), expected_base: None, via_web_key: true,
            match_chords: &["ctrl+shift+/", "ctrl+/"], reject_chords: &["ctrl+7"],
        },
        Case {
            label: "Spanish Ctrl+Shift+¡ produces '¿' with key_base None",
            dom_key: "¿", dom_code: "Equal", physical: KeyCode::Equal,
            shift: true, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('=')), expected_base: None, via_web_key: true,
            match_chords: &["ctrl+shift+="], reject_chords: &["ctrl+shift+¿", "ctrl+¡"],
        },
        // Alt / Option / AltGr composed characters must leave key_base None
        Case {
            label: "German AltGr+Q ('@') leaves key_base None",
            dom_key: "@", dom_code: "KeyQ", physical: KeyCode::KeyQ,
            shift: false, ctrl: true, alt: true, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Command('@')), expected_base: None, via_web_key: true,
            match_chords: &["ctrl+alt+@"], reject_chords: &["ctrl+alt+q"],
        },
        Case {
            label: "macOS Cmd+Option+A ('å') leaves key_base None",
            dom_key: "å", dom_code: "KeyA", physical: KeyCode::KeyA,
            shift: false, ctrl: false, alt: true, meta: true, platform: KeyPlatform::Mac,
            expected_key: Some(Key::Command('a')), expected_base: None, via_web_key: true,
            match_chords: &["mod+alt+a"], reject_chords: &["mod+alt+å"],
        },
        Case {
            label: "macOS Option+A ('å') without Cmd leaves key_base None",
            dom_key: "å", dom_code: "KeyA", physical: KeyCode::KeyA,
            shift: false, ctrl: false, alt: true, meta: false, platform: KeyPlatform::Mac,
            expected_key: Some(Key::Text("å".into())), expected_base: None, via_web_key: false,
            match_chords: &["alt+å", "alt+a"], reject_chords: &["alt+b"],
        },
        // Dead keys: Chrome reports key="Dead" with no character -> no logical event, key_base None
        Case {
            label: "German Ctrl+Dead on Equal (acute accent)",
            dom_key: "Dead", dom_code: "Equal", physical: KeyCode::Equal,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: None, expected_base: None, via_web_key: true,
            match_chords: &[], reject_chords: &[],
        },
        Case {
            label: "German Ctrl+Dead on Backquote (circumflex)",
            dom_key: "Dead", dom_code: "Backquote", physical: KeyCode::Backquote,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: None, expected_base: None, via_web_key: true,
            match_chords: &[], reject_chords: &[],
        },
        Case {
            label: "French Ctrl+Dead on BracketLeft (circumflex)",
            dom_key: "Dead", dom_code: "BracketLeft", physical: KeyCode::BracketLeft,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: None, expected_base: None, via_web_key: true,
            match_chords: &[], reject_chords: &[],
        },
        Case {
            label: "US-International plain Dead on Quote",
            dom_key: "Dead", dom_code: "Quote", physical: KeyCode::Quote,
            shift: false, ctrl: false, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: None, expected_base: None, via_web_key: true,
            match_chords: &[], reject_chords: &[],
        },
        // Space and named keys
        Case {
            label: "Ctrl+Space",
            dom_key: " ", dom_code: "Space", physical: KeyCode::Space,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Text(" ".into())), expected_base: Some(' '), via_web_key: true,
            match_chords: &["ctrl+space"], reject_chords: &["ctrl+shift+space"],
        },
        Case {
            label: "Ctrl+Shift+Space",
            dom_key: " ", dom_code: "Space", physical: KeyCode::Space,
            shift: true, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Text(" ".into())), expected_base: Some(' '), via_web_key: true,
            match_chords: &["ctrl+shift+space"], reject_chords: &["ctrl+space"],
        },
        Case {
            label: "Ctrl+Enter",
            dom_key: "Enter", dom_code: "Enter", physical: KeyCode::Enter,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Enter), expected_base: None, via_web_key: true,
            match_chords: &["ctrl+enter"], reject_chords: &["ctrl+e"],
        },
        Case {
            label: "Unmodified F8",
            dom_key: "F8", dom_code: "F8", physical: KeyCode::F8,
            shift: false, ctrl: false, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: Some(Key::Function(8)), expected_base: None, via_web_key: true,
            match_chords: &["f8"], reject_chords: &["ctrl+f8"],
        },
        Case {
            label: "Unidentified key with Ctrl",
            dom_key: "Unidentified", dom_code: "KeyA", physical: KeyCode::KeyA,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: None, expected_base: None, via_web_key: true,
            match_chords: &[], reject_chords: &[],
        },
        Case {
            label: "Control byte under Ctrl is ignored",
            dom_key: "\x03", dom_code: "KeyC", physical: KeyCode::KeyC,
            shift: false, ctrl: true, alt: false, meta: false, platform: KeyPlatform::Windows,
            expected_key: None, expected_base: None, via_web_key: true,
            match_chords: &[], reject_chords: &[],
        },
    ];

    fn dom_to_winit(s: &str) -> (WKey, Option<&str>) {
        match s {
            "Dead" => (WKey::Dead(None), None),
            "Unidentified" => (WKey::Unidentified(NativeKey::Unidentified), None),
            " " => (WKey::Named(NamedKey::Space), Some(" ")),
            "Enter" => (WKey::Named(NamedKey::Enter), Some("\r")),
            "F8" => (WKey::Named(NamedKey::F8), None),
            other => (WKey::Character(other.into()), Some(other)),
        }
    }

    // On native test builds, `held(ev.mods, platform)` folds `ev.mods.cmd` into the host build's
    // own primary key (Super on macOS, Ctrl elsewhere) whereas on wasm32 it reads `(ctrl, logo)`
    // directly. Clearing `cmd` when matching against an explicit `KeyPlatform` evaluates the exact
    // `(ctrl, logo)` bits that wasm32 sees, on every host OS.
    fn check_chords(ev: &KeyEvent, tx: &crate::Tx, p: KeyPlatform, specs: &[&str]) -> Vec<bool> {
        let web_ev = KeyEvent { key: ev.key.clone(), mods: Mods { cmd: false, ..ev.mods } };
        specs.iter().map(|c| {
            let chord = Chord::parse(c).unwrap();
            let ok = chord.matches_tx_on(&web_ev, tx, p);
            if p == KeyPlatform::current() {
                assert_eq!(chord.matches_tx(ev, tx), ok, "host matches_tx diverged for {c:?}");
            }
            ok
        }).collect()
    }

    #[derive(Default)]
    struct Seen {
        last: Option<(Key, Option<PhysicalKeyCode>, Option<char>)>,
        matched: Vec<bool>,
        rejected: Vec<bool>,
        platform: Option<KeyPlatform>,
        match_chords: &'static [&'static str],
        reject_chords: &'static [&'static str],
    }
    fn empty_view(_: &Seen, _: &mut Cx<Seen>) -> El { crate::el() }

    for case in &cases {
        let (wkey, text) = dom_to_winit(case.dom_key);
        let (canvas_ev, canvas_base) = map_web_winit_key(
            PhysicalKey::Code(case.physical),
            &wkey,
            text,
            case.shift,
            case.ctrl,
            case.alt,
            case.meta,
        );
        assert_eq!(canvas_base, case.expected_base, "{} (canvas key_base)", case.label);
        if case.dom_key != "\x03" {
            assert_eq!(
                canvas_ev.as_ref().map(|e| &e.key),
                case.expected_key.as_ref(),
                "{} (canvas key)",
                case.label,
            );
        }

        // Verify web_key_base_str agrees with web_key_base on every case.
        let mods = crate::input::key_table::web_mods(case.shift, case.ctrl, case.alt, case.meta);
        assert_eq!(web_key_base_str(case.dom_key, mods), case.expected_base, "{} (web_key_base_str)", case.label);

        // Deliver canvas result into Runtime and check Tx + Chord::matches_tx_on.
        if let Some(ev) = canvas_ev.filter(|_| case.dom_key != "\x03") {
            let mut rt = Runtime::new(
                Seen {
                    platform: Some(case.platform),
                    match_chords: case.match_chords,
                    reject_chords: case.reject_chords,
                    ..Seen::default()
                },
                empty_view,
                (100.0, 100.0),
                Env::default(),
            );
            rt.on_key = Some(|s, ev, tx| {
                let p = s.platform.unwrap();
                s.last = Some((ev.key.clone(), tx.physical_key(), tx.key_base()));
                s.matched = check_chords(ev, tx, p, s.match_chords);
                s.rejected = check_chords(ev, tx, p, s.reject_chords);
                true
            });
            rt.frame(0.0);
            rt.physical_key_input(
                PhysicalInput::Key { key: case.physical, mods: ev.mods, pressed: true, repeat: false },
                ev,
                canvas_base,
            );
            rt.frame(0.0);
            assert_eq!(
                rt.state.last,
                Some((case.expected_key.clone().unwrap(), Some(case.physical), case.expected_base)),
                "{} (canvas runtime delivery)",
                case.label,
            );
            for (spec, ok) in case.match_chords.iter().zip(&rt.state.matched) {
                assert!(*ok, "{} (canvas should match {spec:?})", case.label);
            }
            for (spec, ok) in case.reject_chords.iter().zip(&rt.state.rejected) {
                assert!(!*ok, "{} (canvas must not match {spec:?})", case.label);
            }
        }

        // Verify focused-field bridge path (Runtime::web_key) agrees with canvas path.
        if case.via_web_key {
            let mut rt = Runtime::new(
                Seen {
                    platform: Some(case.platform),
                    match_chords: case.match_chords,
                    reject_chords: case.reject_chords,
                    ..Seen::default()
                },
                empty_view,
                (100.0, 100.0),
                Env::default(),
            );
            rt.on_key = Some(|s, ev, tx| {
                let p = s.platform.unwrap();
                s.last = Some((ev.key.clone(), tx.physical_key(), tx.key_base()));
                s.matched = check_chords(ev, tx, p, s.match_chords);
                s.rejected = check_chords(ev, tx, p, s.reject_chords);
                true
            });
            rt.frame(0.0);
            rt.web_key(&serde_json::json!({
                "kind": "key",
                "key": case.dom_key,
                "code": case.dom_code,
                "shift": case.shift,
                "ctrl": case.ctrl,
                "alt": case.alt,
                "meta": case.meta,
                "repeat": false,
            }));
            rt.frame(0.0);
            let expected = case.expected_key.as_ref().map(|k| (k.clone(), Some(case.physical), case.expected_base));
            assert_eq!(rt.state.last, expected, "{} (web_key delivery)", case.label);
            for (spec, ok) in case.match_chords.iter().zip(&rt.state.matched) {
                assert!(*ok, "{} (web_key should match {spec:?})", case.label);
            }
            for (spec, ok) in case.reject_chords.iter().zip(&rt.state.rejected) {
                assert!(!*ok, "{} (web_key must not match {spec:?})", case.label);
            }
        }
    }
}
