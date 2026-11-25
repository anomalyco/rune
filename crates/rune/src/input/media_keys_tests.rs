use super::*;
use crate::{Chord, Cx, El, Env, Key, KeyEvent, KeyPlatform, Keybind, MediaKey, Mods, Runtime, TextEdit, TextStyle, col, el, text_input};

const fn pack_data1(key_code: u32, key_state: u32, repeat: bool) -> isize {
    let flags = (key_state << 8) | if repeat { 1 } else { 0 };
    ((key_code << 16) | flags) as isize
}

#[test]
fn decodes_nx_media_key_press_release_and_repeat_bitfields() {
    let expected = [
        (NX_KEYTYPE_PLAY, MediaKey::PlayPause),
        (NX_KEYTYPE_NEXT, MediaKey::Next),
        (NX_KEYTYPE_PREVIOUS, MediaKey::Previous),
        (NX_KEYTYPE_FAST, MediaKey::FastForward),
        (NX_KEYTYPE_REWIND, MediaKey::Rewind),
    ];
    for (code, key) in expected {
        assert_eq!(
            decode_system_defined(SUBTYPE_AUX_CONTROL_BUTTONS, pack_data1(code, NX_KEYDOWN, false)),
            Some(DecodedMediaKey { key, pressed: true, repeat: false }),
            "press for code {code}",
        );
        assert_eq!(
            decode_system_defined(SUBTYPE_AUX_CONTROL_BUTTONS, pack_data1(code, NX_KEYDOWN, true)),
            Some(DecodedMediaKey { key, pressed: true, repeat: true }),
            "repeat press for code {code}",
        );
        assert_eq!(
            decode_system_defined(SUBTYPE_AUX_CONTROL_BUTTONS, pack_data1(code, NX_KEYUP, false)),
            Some(DecodedMediaKey { key, pressed: false, repeat: false }),
            "release for code {code}",
        );
        assert_eq!(
            decode_system_defined(SUBTYPE_AUX_CONTROL_BUTTONS, pack_data1(code, NX_KEYUP, true)),
            Some(DecodedMediaKey { key, pressed: false, repeat: false }),
            "release with repeat bit for code {code}",
        );
    }
}

#[test]
fn ignores_non_media_aux_keys_other_subtypes_and_unknown_states() {
    for subtype in [0, 1, 7, 9, -1] {
        assert_eq!(
            decode_system_defined(subtype, pack_data1(NX_KEYTYPE_PLAY, NX_KEYDOWN, false)),
            None,
            "subtype {subtype} must be ignored",
        );
    }
    for code in [0, 1, 2, 3, 7, 14, 15, 21, 22, 99, 0xFFFF] {
        assert_eq!(
            decode_system_defined(SUBTYPE_AUX_CONTROL_BUTTONS, pack_data1(code, NX_KEYDOWN, false)),
            None,
            "aux keycode {code} must pass through",
        );
    }
    for state in [0x00, 0x09, 0x0C, 0xFF] {
        assert_eq!(
            decode_system_defined(SUBTYPE_AUX_CONTROL_BUTTONS, pack_data1(NX_KEYTYPE_PLAY, state, false)),
            None,
            "state {state:#x} must be ignored",
        );
    }
}

#[test]
fn decodes_macos_modifier_flags() {
    assert_eq!(mods_from_macos_flags(0), Mods::default());
    let shift = mods_from_macos_flags(1 << 17);
    assert!(shift.shift && !shift.ctrl && !shift.alt && !shift.logo && !shift.cmd);
    let ctrl = mods_from_macos_flags(1 << 18);
    assert!(ctrl.ctrl && !ctrl.cmd && !ctrl.logo);
    let alt = mods_from_macos_flags(1 << 19);
    assert!(alt.alt && !alt.cmd);
    let cmd = mods_from_macos_flags(1 << 20);
    assert!(cmd.cmd && cmd.logo && !cmd.ctrl);
    let all = mods_from_macos_flags((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20));
    assert_eq!(all, Mods { shift: true, cmd: true, alt: true, ctrl: true, logo: true });
}

#[derive(Default)]
struct PlayerState {
    edit: TextEdit,
    ro_edit: TextEdit,
    seen: Vec<(Key, bool, bool)>,
    up: Vec<Key>,
    preview: Vec<Key>,
    chord_hits: usize,
    disabled_hits: usize,
    inert_hits: usize,
}

fn player_view(s: &PlayerState, _: &mut Cx<PlayerState>) -> El {
    col().size(320.0, 240.0)
        .on_key_preview(|s: &mut PlayerState, ev, _| {
            if matches!(ev.key, Key::Media(_)) {
                s.preview.push(ev.key.clone());
            }
            false
        })
        .on_key_up(|s: &mut PlayerState, ev, _| {
            if matches!(ev.key, Key::Media(_)) {
                s.up.push(ev.key.clone());
                return true;
            }
            false
        })
        .child(
            text_input(&s.edit, TextStyle::new(14.0, 400.0))
                .single_line()
                .key("search")
                .size(200.0, 32.0)
                .on_edit(|s: &mut PlayerState| &mut s.edit),
        )
        .child(
            text_input(&s.ro_edit, TextStyle::new(14.0, 400.0))
                .single_line()
                .read_only(true)
                .key("readonly")
                .size(200.0, 32.0)
                .on_edit(|s: &mut PlayerState| &mut s.ro_edit),
        )
        .child(
            el().key("disabled-target")
                .size(60.0, 24.0)
                .focusable(true)
                .disabled(true)
                .on_key(|s: &mut PlayerState, _, _| {
                    s.disabled_hits += 1;
                    true
                }),
        )
        .child(
            el().key("inert-wrap")
                .inert()
                .child(
                    el().key("inert-target")
                        .size(60.0, 24.0)
                        .focusable(true)
                        .on_key(|s: &mut PlayerState, _, _| {
                            s.inert_hits += 1;
                            true
                        }),
                ),
        )
}

fn player_runtime(enabled: bool) -> Runtime<PlayerState> {
    let state = PlayerState {
        edit: TextEdit::new("track"),
        ro_edit: TextEdit::new("fixed"),
        ..PlayerState::default()
    };
    let mut rt = Runtime::new(state, player_view, (320.0, 240.0), Env::new(true, 1.0));
    rt.set_media_keys(enabled);
    rt.on_key = Some(|s, ev, tx| {
        if matches!(ev.key, Key::Media(_)) {
            if Keybind::new("playpause").matches(ev) {
                s.chord_hits += 1;
            }
            s.seen.push((ev.key.clone(), tx.key_repeat(), ev.mods.shift));
            return true;
        }
        false
    });
    rt.frame(0.0);
    rt
}

#[test]
fn off_by_default_apps_receive_no_media_keys_and_no_redraws() {
    let mut rt = player_runtime(false);
    assert!(!rt.media_keys());
    rt.frame(0.1);

    let decoded = decode_system_defined(SUBTYPE_AUX_CONTROL_BUTTONS, pack_data1(NX_KEYTYPE_PLAY, NX_KEYDOWN, false)).unwrap();
    let redraw = {
        let (core, mut app) = rt.split_mut();
        deliver_decoded(core, &mut app, decoded, Mods::default())
    };
    assert!(!redraw, "opt-out runtime must not request a redraw for decoded media keys");
    rt.key(KeyEvent::from(Key::Media(MediaKey::PlayPause)));
    rt.repeat_key(KeyEvent::from(Key::Media(MediaKey::Next)));
    assert!(!rt.frame(0.2), "idle runtime with media_keys off stays idle");
    assert!(rt.state.seen.is_empty());
    assert!(rt.state.up.is_empty());
    assert!(rt.state.preview.is_empty());
}

#[test]
fn opted_in_media_keys_reach_on_key_chord_key_up_and_bypass_text_fields() {
    let mut rt = player_runtime(true);
    assert!(rt.media_keys());

    for (data1, mods) in [
        (pack_data1(NX_KEYTYPE_PLAY, NX_KEYDOWN, false), Mods::default()),
        (pack_data1(NX_KEYTYPE_NEXT, NX_KEYDOWN, true), Mods { shift: true, ..Mods::default() }),
        (pack_data1(NX_KEYTYPE_PLAY, NX_KEYUP, false), Mods::default()),
    ] {
        let decoded = decode_system_defined(SUBTYPE_AUX_CONTROL_BUTTONS, data1).unwrap();
        let redraw = {
            let (core, mut app) = rt.split_mut();
            deliver_decoded(core, &mut app, decoded, mods)
        };
        assert!(redraw);
        rt.frame(0.0);
    }
    assert_eq!(
        rt.state.seen,
        [
            (Key::Media(MediaKey::PlayPause), false, false),
            (Key::Media(MediaKey::Next), true, true),
        ],
    );
    assert_eq!(rt.state.up, [Key::Media(MediaKey::PlayPause)]);
    assert_eq!(rt.state.chord_hits, 1);

    let (x, y) = rt.find("search").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::Media(MediaKey::Previous)));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "track");
    assert_eq!(rt.state.seen.last(), Some(&(Key::Media(MediaKey::Previous), false, false)));
    assert_eq!(rt.state.preview.last(), Some(&Key::Media(MediaKey::Previous)));

    let (x, y) = rt.find("readonly").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::Media(MediaKey::Stop)));
    rt.frame(0.0);
    assert_eq!(rt.state.ro_edit.text, "fixed");
    assert_eq!(rt.state.seen.last(), Some(&(Key::Media(MediaKey::Stop), false, false)));

    let (x, y) = rt.find("disabled-target").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::Media(MediaKey::FastForward)));
    rt.frame(0.0);
    assert_eq!(rt.state.disabled_hits, 0);
    assert_eq!(rt.state.inert_hits, 0);
    assert_eq!(rt.state.seen.last(), Some(&(Key::Media(MediaKey::FastForward), false, false)));

    let chord = Chord::parse("playpause").unwrap();
    assert_eq!(chord.to_string(), "mediaplaypause");
    assert_eq!(chord.label_on(KeyPlatform::Mac), "⏯");
    assert_eq!(chord.label_on(KeyPlatform::Windows), "Play/Pause");
    assert!(chord.matches(&KeyEvent::from(Key::Media(MediaKey::PlayPause))));
    assert!(!chord.matches(&KeyEvent::from(Key::Media(MediaKey::Play))));
}

#[test]
fn host_keyboard_core_gates_media_keys_on_opt_in_and_delivers_physical_code() {
    use crate::input::host_events::{KeyInput, keyboard_core};
    use crate::PhysicalKeyCode;

    type Seen = Vec<(Key, Option<PhysicalKeyCode>)>;
    fn view(_: &Seen, _: &mut Cx<Seen>) -> El { el().size(100.0, 100.0) }
    let mut rt = Runtime::new(Seen::new(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, tx| { s.push((ev.key.clone(), tx.physical_key())); true });
    rt.frame(0.0);

    let media_ev = KeyEvent::from(Key::Media(MediaKey::PlayPause));
    let redraw = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, KeyInput {
            physical: None,
            logical: Some(media_ev.clone()),
            base: None,
            mods: Mods::default(),
            pressed: true,
            repeat: false,
            unhandled: None,
        })
    };
    assert!(!redraw);
    assert!(!rt.frame(0.0));
    assert!(rt.state.is_empty());

    rt.set_media_keys(true);
    let redraw = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, KeyInput {
            physical: Some(PhysicalKeyCode::MediaPlayPause),
            logical: Some(media_ev),
            base: None,
            mods: Mods::default(),
            pressed: true,
            repeat: false,
            unhandled: None,
        })
    };
    assert!(redraw);
    rt.frame(0.0);
    assert_eq!(rt.state, [(Key::Media(MediaKey::PlayPause), Some(PhysicalKeyCode::MediaPlayPause))]);
}
