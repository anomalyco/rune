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
            keyboard_core(core, &mut app, KeyInput {
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
            keyboard_core(core, &mut app, KeyInput {
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
        keyboard_core(core, &mut app, KeyInput {
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
        keyboard_core(core, &mut app, KeyInput {
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
        keyboard_core(core, &mut app, KeyInput {
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

#[test]
fn option_as_alt_matches_left_right_both_and_none_modifier_masks() {
    use super::{NX_DEVICELALTKEYMASK, NX_DEVICERALTKEYMASK, option_as_alt_matches};
    use crate::OptionAsAlt;

    let left = NX_DEVICELALTKEYMASK;
    let right = NX_DEVICERALTKEYMASK;
    let both = left | right;

    // Without Option held, no mode matches (even if stale device bits were present).
    for mode in [OptionAsAlt::None, OptionAsAlt::Left, OptionAsAlt::Right, OptionAsAlt::Both] {
        for bits in [0, left, right, both] {
            assert!(!option_as_alt_matches(mode, false, bits), "{mode:?} with alt=false bits={bits:#x}");
        }
    }

    // None never matches (default text fields always type the layout's Option characters).
    for bits in [0, left, right, both] {
        assert!(!option_as_alt_matches(OptionAsAlt::None, true, bits));
    }

    // Left matches only when NX_DEVICELALTKEYMASK is set.
    assert!(!option_as_alt_matches(OptionAsAlt::Left, true, 0));
    assert!(option_as_alt_matches(OptionAsAlt::Left, true, left));
    assert!(!option_as_alt_matches(OptionAsAlt::Left, true, right));
    assert!(option_as_alt_matches(OptionAsAlt::Left, true, both));

    // Right matches only when NX_DEVICERALTKEYMASK is set.
    assert!(!option_as_alt_matches(OptionAsAlt::Right, true, 0));
    assert!(!option_as_alt_matches(OptionAsAlt::Right, true, left));
    assert!(option_as_alt_matches(OptionAsAlt::Right, true, right));
    assert!(option_as_alt_matches(OptionAsAlt::Right, true, both));

    // Both matches whenever Option is held, even on synthetic events without device-side bits.
    for bits in [0, left, right, both] {
        assert!(option_as_alt_matches(OptionAsAlt::Both, true, bits));
    }
}

#[test]
fn direct_commit_keeps_unselected_option_key_composing_when_left_or_right_is_alt() {
    use super::{NX_DEVICELALTKEYMASK, NX_DEVICERALTKEYMASK, option_as_alt_matches};
    use crate::OptionAsAlt;

    // With OptionAsAlt::Left, left Option acts as Alt (suppresses direct commit) while right Option
    // keeps normal character/IME composition (e.g. German Option+5 -> '[' or IME substitution).
    let left_kd = KeyDown {
        view: 1,
        chars: Some("[".into()),
        chars_ignoring_mods: Some("5".into()),
        ctrl: false,
        cmd: false,
        alt: true,
        option_as_alt: option_as_alt_matches(OptionAsAlt::Left, true, NX_DEVICELALTKEYMASK),
    };
    assert!(!should_direct_commit(&left_kd, false, "「"));

    let right_kd = KeyDown {
        view: 1,
        chars: Some("[".into()),
        chars_ignoring_mods: Some("5".into()),
        ctrl: false,
        cmd: false,
        alt: true,
        option_as_alt: option_as_alt_matches(OptionAsAlt::Left, true, NX_DEVICERALTKEYMASK),
    };
    assert!(!right_kd.option_as_alt);
    assert!(should_direct_commit(&right_kd, false, "「"));
    // Layout character matching NSEvent.characters stays on KeyboardInput (types '[').
    assert!(!should_direct_commit(&right_kd, false, "["));
}

#[cfg(target_os = "macos")]
#[test]
fn winit_option_as_alt_maps_all_modes_and_resets_when_ime_disallowed() {
    use crate::OptionAsAlt;
    use crate::input::host_events::winit_option_as_alt;
    use winit::platform::macos::OptionAsAlt as WinitOpt;

    assert_eq!(winit_option_as_alt(true, OptionAsAlt::None), WinitOpt::None);
    assert_eq!(winit_option_as_alt(true, OptionAsAlt::Left), WinitOpt::OnlyLeft);
    assert_eq!(winit_option_as_alt(true, OptionAsAlt::Right), WinitOpt::OnlyRight);
    assert_eq!(winit_option_as_alt(true, OptionAsAlt::Both), WinitOpt::Both);
    for mode in [OptionAsAlt::None, OptionAsAlt::Left, OptionAsAlt::Right, OptionAsAlt::Both] {
        assert_eq!(winit_option_as_alt(false, mode), WinitOpt::None);
    }
}

#[test]
fn rtl_primary_lang_ids_match_windows_rtl_layouts() {
    use super::is_rtl_primary_lang_id;
    // Primary language IDs extracted via PRIMARYLANGID(LOWORD(hkl)) = (hkl as u16) & 0x03ff.
    for hkl in [0x0401u16, 0x0801, 0x040d, 0x0420, 0x0429, 0x045a, 0x0480] {
        assert!(is_rtl_primary_lang_id(hkl & 0x03ff), "expected RTL for HKL {hkl:#06x}");
    }
    for hkl in [0x0000u16, 0x0409, 0x0809, 0x0407, 0x0419, 0x0422, 0x0411, 0x0804] {
        assert!(!is_rtl_primary_lang_id(hkl & 0x03ff), "expected LTR for HKL {hkl:#06x}");
    }
}
