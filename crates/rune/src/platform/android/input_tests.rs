//! Host-side tests for Android keycode mapping, modifier translation, editor actions,
//! and end-to-end `InputConnection` text editing through `Runtime` and `ImeSync`.

use crate::edit::{Ime, TextEdit};
use crate::input::host_events::{ImeSync, ime_sync_step};
use crate::{Cx, El, Env, Key, KeyEvent, MediaKey, Mods, Runtime, TextStyle, col, text_input};

use super::input::*;
use super::machine::{self, Effect, Event, State};

const KEYCODE_BACK: i32 = 4;
const KEYCODE_HOME: i32 = 3;
const KEYCODE_VOLUME_UP: i32 = 24;
const KEYCODE_SHIFT_LEFT: i32 = 59;

#[test]
fn modifier_bits_map_ctrl_as_primary_and_meta_as_logo() {
    assert_eq!(map_mods(0), Mods::default());

    let shift = map_mods(0x01);
    assert!(shift.shift && !shift.cmd && !shift.ctrl && !shift.alt && !shift.logo);

    let ctrl_left = map_mods(0x2000);
    assert!(ctrl_left.ctrl && ctrl_left.cmd && !ctrl_left.logo && !ctrl_left.shift && !ctrl_left.alt);

    let alt_right = map_mods(0x20);
    assert!(alt_right.alt && !alt_right.cmd && !alt_right.ctrl);

    let meta = map_mods(0x10000);
    assert!(meta.logo && !meta.cmd && !meta.ctrl);

    let all = map_mods(META_SHIFT_MASK | META_ALT_MASK | META_CTRL_MASK | META_META_MASK);
    assert!(all.shift && all.alt && all.ctrl && all.cmd && all.logo);
}

#[test]
fn named_navigation_editing_function_and_media_keycodes_map_accurately() {
    let cases = [
        (KEYCODE_DPAD_UP, Key::Up),
        (KEYCODE_DPAD_DOWN, Key::Down),
        (KEYCODE_DPAD_LEFT, Key::Left),
        (KEYCODE_DPAD_RIGHT, Key::Right),
        (KEYCODE_TAB, Key::Tab),
        (KEYCODE_ENTER, Key::Enter),
        (KEYCODE_NUMPAD_ENTER, Key::Enter),
        (KEYCODE_DEL, Key::Backspace),
        (KEYCODE_FORWARD_DEL, Key::Delete),
        (KEYCODE_MOVE_HOME, Key::Home),
        (KEYCODE_MOVE_END, Key::End),
        (KEYCODE_PAGE_UP, Key::PageUp),
        (KEYCODE_PAGE_DOWN, Key::PageDown),
        (KEYCODE_ESCAPE, Key::Escape),
        (KEYCODE_INSERT, Key::Insert),
        (KEYCODE_MENU, Key::ContextMenu),
        (KEYCODE_SYSRQ, Key::PrintScreen),
        (KEYCODE_SCROLL_LOCK, Key::ScrollLock),
        (KEYCODE_BREAK, Key::Pause),
        (KEYCODE_F1, Key::Function(1)),
        (KEYCODE_F1 + 4, Key::Function(5)),
        (KEYCODE_F12, Key::Function(12)),
        (KEYCODE_MEDIA_PLAY_PAUSE, Key::Media(MediaKey::PlayPause)),
        (KEYCODE_MEDIA_PLAY, Key::Media(MediaKey::Play)),
        (KEYCODE_MEDIA_PAUSE, Key::Media(MediaKey::Pause)),
        (KEYCODE_MEDIA_NEXT, Key::Media(MediaKey::Next)),
        (KEYCODE_MEDIA_PREVIOUS, Key::Media(MediaKey::Previous)),
        (KEYCODE_MEDIA_STOP, Key::Media(MediaKey::Stop)),
        (KEYCODE_MEDIA_FAST_FORWARD, Key::Media(MediaKey::FastForward)),
        (KEYCODE_MEDIA_REWIND, Key::Media(MediaKey::Rewind)),
    ];
    for (code, expected) in cases {
        let ev = map_key_event(code, 0, 0, 0).unwrap_or_else(|| panic!("keycode {code} unmapped"));
        assert_eq!(ev.key, expected, "keycode {code}");
        assert_eq!(ev.mods, Mods::default());
    }
}

#[test]
fn system_keys_modifiers_and_dead_keys_return_none() {
    assert_eq!(map_key_event(KEYCODE_BACK, 0, 0, 0), None);
    assert_eq!(map_key_event(KEYCODE_HOME, 0, 0, 0), None);
    assert_eq!(map_key_event(KEYCODE_VOLUME_UP, 0, 0, 0), None);
    assert_eq!(map_key_event(KEYCODE_SHIFT_LEFT, 0x01, 0, 0), None);

    // Dead key (combining accent bit set): must not emit a character yet.
    let dead_grave = (COMBINING_ACCENT | ('`' as u32)) as i32;
    assert_eq!(map_key_event(KEYCODE_GRAVE, 0, dead_grave, '`' as i32), None);
}

#[test]
fn printable_unicode_space_and_synthetic_key_fallbacks() {
    // Direct Unicode character (including accented and supplementary emoji).
    let ev = map_key_event(KEYCODE_A, 0x01, 'É' as i32, 'é' as i32).unwrap();
    assert_eq!(ev.key, Key::Text("É".into()));
    assert!(ev.mods.shift);

    let emoji = map_key_event(0, 0, '😀' as i32, 0).unwrap();
    assert_eq!(emoji.key, Key::Text("😀".into()));

    // Plain space and layout-overridden Shift+Space (ZWNJ U+200C).
    let space = map_key_event(KEYCODE_SPACE, 0, ' ' as i32, ' ' as i32).unwrap();
    assert_eq!(space.key, Key::Text(" ".into()));

    let zwnj = map_key_event(KEYCODE_SPACE, 0x01, 0x200C, ' ' as i32).unwrap();
    assert_eq!(zwnj.key, Key::Text("‌".into()));

    // Alt chord where Android reports unicodeChar == 0 falls back to unmodifiedChar.
    let alt_d = map_key_event(KEYCODE_A + 3, 0x02, 0, 'd' as i32).unwrap();
    assert_eq!(alt_d.key, Key::Text("d".into()));
    assert!(alt_d.mods.alt);

    // Synthetic KeyEvent with unicodeChar == 0 falls back to US keycode table.
    let synth_a = map_key_event(KEYCODE_A, 0, 0, 0).unwrap();
    assert_eq!(synth_a.key, Key::Text("a".into()));
    let synth_shift_a = map_key_event(KEYCODE_A, 0x01, 0, 0).unwrap();
    assert_eq!(synth_shift_a.key, Key::Text("A".into()));
}

#[test]
fn ctrl_chords_map_to_command_with_layout_and_non_ascii_remapping() {
    // Ctrl+C on QWERTY (Android returns unicodeChar == 0 under Ctrl).
    let ctrl_c = map_key_event(KEYCODE_A + 2, 0x1000, 0, 'c' as i32).unwrap();
    assert_eq!(ctrl_c.key, Key::Command('c'));
    assert!(ctrl_c.mods.cmd && ctrl_c.mods.ctrl);

    // Ctrl+Shift+Z on QWERTZ where Z lives on KEYCODE_Y: honors unmodified ASCII 'z'.
    let qwertz_z = map_key_event(KEYCODE_A + 24, 0x1000 | 0x01, 0, 'z' as i32).unwrap();
    assert_eq!(qwertz_z.key, Key::Command('z'));
    assert!(qwertz_z.mods.cmd && qwertz_z.mods.shift);

    // Ctrl+М on Cyrillic at KEYCODE_V: non-ASCII unmodified falls back to US 'v'.
    let cyrillic_v = map_key_event(KEYCODE_A + 21, 0x1000, 0, 'м' as i32).unwrap();
    assert_eq!(cyrillic_v.key, Key::Command('v'));

    // Ctrl+Space stays Key::Text(" ") with cmd set (matching KeyEvent::is_command(' ')).
    let ctrl_space = map_key_event(KEYCODE_SPACE, 0x1000, 0, ' ' as i32).unwrap();
    assert_eq!(ctrl_space.key, Key::Text(" ".into()));
    assert!(ctrl_space.is_command(' '));
}

#[test]
fn editor_actions_map_to_enter_and_tab_navigation() {
    assert_eq!(map_editor_action(IME_ACTION_NONE), None);
    assert_eq!(map_editor_action(99), None);
    for code in [
        IME_ACTION_UNSPECIFIED,
        IME_ACTION_GO,
        IME_ACTION_SEARCH,
        IME_ACTION_SEND,
        IME_ACTION_DONE,
    ] {
        assert_eq!(map_editor_action(code), Some(KeyEvent::from(Key::Enter)));
    }
    assert_eq!(map_editor_action(IME_ACTION_NEXT), Some(KeyEvent::from(Key::Tab)));
    assert_eq!(
        map_editor_action(IME_ACTION_PREVIOUS),
        Some(KeyEvent {
            key: Key::Tab,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        }),
    );
}

#[test]
fn composing_commit_and_delete_helpers_handle_boundaries() {
    assert_eq!(
        composing_text_event("hello".into(), 1),
        Ime::Preedit("hello".into(), Some((5, 5)), Vec::new()),
    );
    assert_eq!(
        composing_text_event("hello".into(), 0),
        Ime::Preedit("hello".into(), Some((0, 0)), Vec::new()),
    );
    assert_eq!(
        composing_text_event("hello".into(), -3),
        Ime::Preedit("hello".into(), Some((0, 0)), Vec::new()),
    );
    assert_eq!(
        composing_text_event(String::new(), 1),
        Ime::Preedit(String::new(), None, Vec::new()),
    );
    assert_eq!(commit_text_event("👋".into()), Ime::Commit("👋".into()));

    assert_eq!(clamp_delete_count(-5), 0);
    assert_eq!(clamp_delete_count(0), 0);
    assert_eq!(clamp_delete_count(3), 3);
    assert_eq!(clamp_delete_count(100_000), MAX_DELETE_SURROUNDING);
}

struct FormState {
    first: TextEdit,
    second: TextEdit,
    submitted: Vec<String>,
    media_seen: Vec<MediaKey>,
}

fn form_view(s: &FormState, _: &mut Cx<FormState>) -> El {
    col()
        .size(320.0, 200.0)
        .child(
            text_input(&s.first, TextStyle::new(14.0, 400.0))
                .key("first")
                .on_edit(|st: &mut FormState| &mut st.first)
                .on_submit(|st: &mut FormState, _| st.submitted.push(st.first.text.clone()))
                .size(240.0, 36.0),
        )
        .child(
            text_input(&s.second, TextStyle::new(14.0, 400.0))
                .key("second")
                .on_edit(|st: &mut FormState| &mut st.second)
                .size(240.0, 36.0),
        )
}

fn make_form_runtime() -> Runtime<FormState> {
    let mut first = TextEdit::new("");
    first.set_single_line(true);
    let mut second = TextEdit::new("");
    second.set_single_line(true);
    let state = FormState {
        first,
        second,
        submitted: Vec::new(),
        media_seen: Vec::new(),
    };
    let mut rt = Runtime::new(state, form_view, (320.0, 200.0), Env::new(true, 2.0));
    rt.on_key = Some(|st, ev, _| {
        if let Key::Media(m) = ev.key {
            st.media_seen.push(m);
            return true;
        }
        false
    });
    rt.frame(0.016);
    rt
}

#[test]
fn end_to_end_input_connection_flow_and_ime_sync_on_host() {
    let mut rt = make_form_runtime();
    let mut ime_sync = ImeSync::default();
    let mut host = State::default();
    let _ = machine::step(&mut host, Event::Resumed);
    let _ = machine::step(
        &mut host,
        Event::SurfaceCreated {
            width_px: 640,
            height_px: 400,
            scale: 2.0,
            dark: true,
        },
    );
    let _ = machine::step(&mut host, Event::Frame);

    // Before focusing a field, ImeSync keeps IME disabled.
    let step = ime_sync_step(rt.core_mut(), &mut ime_sync, host.resumed);
    assert_eq!(step.set_allowed, None);
    assert!(!host.ime_enabled);

    // Focus the first text field -> ImeSync enables IME and host state machine shows keyboard.
    rt.pointer_down(20.0, 18.0);
    rt.pointer_up(20.0, 18.0);
    rt.frame(0.016);
    let step = ime_sync_step(rt.core_mut(), &mut ime_sync, host.resumed);
    assert_eq!(step.set_allowed, Some(true));
    let effects: Vec<_> = machine::step(
        &mut host,
        Event::FrameDone {
            animating: false,
            wake_after_secs: None,
            ime_allowed: step.set_allowed,
            ime_restart: step.cancel,
        },
    )
    .into_iter()
    .collect();
    assert_eq!(effects, [Effect::SetImeEnabled { enabled: true, restart: true }]);
    assert!(host.ime_enabled);

    // 1. setComposingText -> preedit appears without committing yet.
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::Ime(composing_text_event("hel".into(), 1)),
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.first.text, "");
    assert_eq!(rt.state.first.preedit(), Some(("hel", Some((3, 3)))));

    // 2. commitText replaces active preedit with committed text (including emoji).
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::Ime(commit_text_event("hello 👋".into())),
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.first.text, "hello 👋");
    assert_eq!(rt.state.first.preedit(), None);

    // 3. Same-frame batch: setComposingText followed by finishComposingText commits in order
    //    WITHOUT setting ime_reset (so no spurious restartInput fires).
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::Ime(composing_text_event("!".into(), 1)),
    ));
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::FinishCompositionFromIme,
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.first.text, "hello 👋!");
    assert_eq!(rt.state.first.preedit(), None);
    let step = ime_sync_step(rt.core_mut(), &mut ime_sync, host.resumed);
    assert!(!step.cancel, "finishComposingText from IME must not request restartInput");
    assert_eq!(step.set_allowed, None);

    // 4. deleteSurroundingText removes characters before and after the caret.
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::DeleteSurrounding { before: 2, after: 0 },
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.first.text, "hello ");

    // Move caret left twice then delete 1 before and 1 after.
    let left = map_key_event(KEYCODE_DPAD_LEFT, 0, 0, 0).unwrap();
    assert!(apply_text_input(rt.core_mut(), TextInputAction::KeyDown { event: left.clone(), repeat: false }));
    assert!(apply_text_input(rt.core_mut(), TextInputAction::KeyDown { event: left, repeat: false }));
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::DeleteSurrounding { before: 1, after: 1 },
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.first.text, "hel ");

    // 5. sendKeyEvent Backspace (KEYCODE_DEL) deletes one more character.
    let del = map_key_event(KEYCODE_DEL, 0, 0, 0).unwrap();
    assert!(apply_text_input(rt.core_mut(), TextInputAction::KeyDown { event: del, repeat: false }));
    rt.frame(0.016);
    assert_eq!(rt.state.first.text, "he ");

    // 6. Editor action DONE submits the single-line field.
    let done = map_editor_action(IME_ACTION_DONE).unwrap();
    assert!(apply_text_input(rt.core_mut(), TextInputAction::KeyDown { event: done, repeat: false }));
    rt.frame(0.016);
    assert_eq!(rt.state.submitted, ["he "]);

    // 7. Editor action NEXT tabs focus to the second field, where commitText lands.
    let next = map_editor_action(IME_ACTION_NEXT).unwrap();
    assert!(apply_text_input(rt.core_mut(), TextInputAction::KeyDown { event: next, repeat: false }));
    rt.frame(0.016);
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::Ime(commit_text_event("second".into())),
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.second.text, "second");

    // 8. Blurring (Escape or tapping outside) closes the keyboard via ImeSync.
    rt.key(KeyEvent::from(Key::Escape));
    rt.frame(0.016);
    let step = ime_sync_step(rt.core_mut(), &mut ime_sync, host.resumed);
    assert_eq!(step.set_allowed, Some(false));
    let effects: Vec<_> = machine::step(
        &mut host,
        Event::FrameDone {
            animating: false,
            wake_after_secs: None,
            ime_allowed: step.set_allowed,
            ime_restart: step.cancel,
        },
    )
    .into_iter()
    .collect();
    assert_eq!(effects, [Effect::SetImeEnabled { enabled: false, restart: true }]);
    assert!(!host.ime_enabled);
}

#[test]
fn media_keys_respect_runtime_opt_in() {
    let mut rt = make_form_runtime();
    let play_pause = map_key_event(KEYCODE_MEDIA_PLAY_PAUSE, 0, 0, 0).unwrap();

    // Off by default: unconsumed so Android system media controls still receive it.
    assert!(!apply_text_input(
        rt.core_mut(),
        TextInputAction::KeyDown {
            event: play_pause.clone(),
            repeat: false,
        },
    ));
    rt.frame(0.016);
    assert!(rt.state.media_seen.is_empty());

    // Opted in: delivered to on_key.
    rt.set_media_keys(true);
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::KeyDown {
            event: play_pause,
            repeat: false,
        },
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.media_seen, [MediaKey::PlayPause]);
}
