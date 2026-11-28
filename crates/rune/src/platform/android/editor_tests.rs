//! Host-side tests for Android `EditorInfo` mapping, secret-field soft-keyboard entry,
//! `InputMode::None` suppression, keyboard switching/restart, re-tap keyboard restore, and
//! `preserve_focus_on_press(true)` buttons.

use crate::edit::{Ime, TextEdit};
use crate::{Cx, El, Env, InputMode, Key, KeyEvent, Runtime, TextStyle, TouchPhase, col, el, text_input};

use super::editor::*;
use super::input::{IME_ACTION_DONE, TextInputAction, apply_text_input, commit_text_event, composing_text_event, map_editor_action};
use super::machine::{self, Effect, Event, State};

fn base_options() -> i32 {
    IME_FLAG_NO_EXTRACT_UI | IME_FLAG_NO_FULLSCREEN
}

fn attach_resumed_host() -> State {
    let mut host = State::default();
    let _ = machine::step(&mut host, Event::Resumed);
    let _ = machine::step(
        &mut host,
        Event::SurfaceCreated {
            width_px: 1080,
            height_px: 2400,
            scale: 3.0,
            dark: true,
        },
    );
    let _ = machine::step(&mut host, Event::Frame);
    host
}

fn sync_host<S: 'static>(rt: &mut Runtime<S>, host: &mut State) -> Vec<Effect> {
    let snap = sample_editor(rt.core_mut());
    machine::step(
        host,
        Event::FrameDone {
            animating: false,
            wake_after_secs: None,
            editor: snap.config,
            ime_restart: snap.reset,
            field_tap: snap.field_tap,
        },
    )
    .into_iter()
    .collect()
}

fn tap_key<S: 'static>(rt: &mut Runtime<S>, id: u64, key: &str, t: f64) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key {key:?}"));
    rt.touch(id, TouchPhase::Started, x, y, t);
    rt.frame(0.016);
    rt.touch(id, TouchPhase::Ended, x, y, t + 0.04);
    rt.frame(0.016);
}

#[test]
fn editor_config_maps_every_input_mode_single_line_and_secure_combination() {
    let base = base_options();

    // Default multi-line text field keeps multi-line Return (no IME_ACTION_DONE).
    assert_eq!(
        editor_config(InputMode::Text, false, false),
        Some(EditorConfig {
            input_type: 0x2_0001,
            ime_options: base,
        }),
    );
    // Single-line text field gets IME_ACTION_DONE.
    assert_eq!(
        editor_config(InputMode::Text, true, false),
        Some(EditorConfig {
            input_type: 0x01,
            ime_options: base | IME_ACTION_DONE,
        }),
    );

    // Semantic input modes match Android InputType hex values.
    assert_eq!(
        editor_config(InputMode::Email, true, false),
        Some(EditorConfig {
            input_type: 0x21,
            ime_options: base | IME_ACTION_DONE,
        }),
    );
    assert_eq!(
        editor_config(InputMode::Url, true, false),
        Some(EditorConfig {
            input_type: 0x11,
            ime_options: base | IME_ACTION_DONE,
        }),
    );
    assert_eq!(
        editor_config(InputMode::Numeric, true, false),
        Some(EditorConfig {
            input_type: 0x02,
            ime_options: base | IME_ACTION_DONE,
        }),
    );
    assert_eq!(
        editor_config(InputMode::Decimal, true, false),
        Some(EditorConfig {
            input_type: 0x2002,
            ime_options: base | IME_ACTION_DONE,
        }),
    );
    assert_eq!(
        editor_config(InputMode::Tel, true, false),
        Some(EditorConfig {
            input_type: 0x03,
            ime_options: base | IME_ACTION_DONE,
        }),
    );
    // Search always uses IME_ACTION_SEARCH (whether single_line is set or not).
    assert_eq!(
        editor_config(InputMode::Search, true, false),
        Some(EditorConfig {
            input_type: 0x01,
            ime_options: base | IME_ACTION_SEARCH,
        }),
    );
    assert_eq!(
        editor_config(InputMode::Search, false, false),
        Some(EditorConfig {
            input_type: 0x01,
            ime_options: base | IME_ACTION_SEARCH,
        }),
    );

    // Password (single-line) uses TYPE_TEXT_VARIATION_PASSWORD + NO_PERSONALIZED_LEARNING + DONE.
    assert_eq!(
        editor_config(InputMode::Text, true, true),
        Some(EditorConfig {
            input_type: 0x81,
            ime_options: base | IME_FLAG_NO_PERSONALIZED_LEARNING | IME_ACTION_DONE,
        }),
    );
    // Multi-line secret note/key preserves multi-line + password + NO_PERSONALIZED_LEARNING.
    assert_eq!(
        editor_config(InputMode::Text, false, true),
        Some(EditorConfig {
            input_type: 0x2_0081,
            ime_options: base | IME_FLAG_NO_PERSONALIZED_LEARNING,
        }),
    );
    // Secret numeric PIN pad uses TYPE_CLASS_NUMBER | TYPE_NUMBER_VARIATION_PASSWORD (0x12).
    assert_eq!(
        editor_config(InputMode::Numeric, true, true),
        Some(EditorConfig {
            input_type: 0x12,
            ime_options: base | IME_FLAG_NO_PERSONALIZED_LEARNING | IME_ACTION_DONE,
        }),
    );
    // Secret decimal preserves both number password and decimal flag (0x2012).
    assert_eq!(
        editor_config(InputMode::Decimal, true, true),
        Some(EditorConfig {
            input_type: 0x2012,
            ime_options: base | IME_FLAG_NO_PERSONALIZED_LEARNING | IME_ACTION_DONE,
        }),
    );

    // InputMode::None raises no soft keyboard (plain or secure).
    assert_eq!(editor_config(InputMode::None, true, false), None);
    assert_eq!(editor_config(InputMode::None, false, false), None);
    assert_eq!(editor_config(InputMode::None, true, true), None);
}

struct Form {
    notes: TextEdit,
    email: TextEdit,
    url: TextEdit,
    search: TextEdit,
    numeric: TextEdit,
    decimal: TextEdit,
    tel: TextEdit,
    password: TextEdit,
    pin: TextEdit,
    none_mode: TextEdit,
    read_only: TextEdit,
    submitted: Vec<String>,
    preserve_taps: usize,
    regular_taps: usize,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            notes: TextEdit::new(""),
            email: TextEdit::new(""),
            url: TextEdit::new(""),
            search: TextEdit::new(""),
            numeric: TextEdit::new(""),
            decimal: TextEdit::new(""),
            tel: TextEdit::new(""),
            password: TextEdit::new("").secure(),
            pin: TextEdit::new("").secure(),
            none_mode: TextEdit::new(""),
            read_only: TextEdit::new("fixed"),
            submitted: Vec::new(),
            preserve_taps: 0,
            regular_taps: 0,
        }
    }
}

fn form_view(s: &Form, _: &mut Cx<Form>) -> El {
    let st = TextStyle::new(14.0, 400.0);
    col()
        .size(360.0, 800.0)
        .child(
            text_input(&s.notes, st)
                .key("notes")
                .size(280.0, 36.0)
                .on_edit(|f: &mut Form| &mut f.notes),
        )
        .child(
            text_input(&s.email, st)
                .key("email")
                .single_line()
                .input_mode(InputMode::Email)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.email),
        )
        .child(
            text_input(&s.url, st)
                .key("url")
                .single_line()
                .input_mode(InputMode::Url)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.url),
        )
        .child(
            text_input(&s.search, st)
                .key("search")
                .single_line()
                .input_mode(InputMode::Search)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.search),
        )
        .child(
            text_input(&s.numeric, st)
                .key("numeric")
                .single_line()
                .input_mode(InputMode::Numeric)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.numeric),
        )
        .child(
            text_input(&s.decimal, st)
                .key("decimal")
                .single_line()
                .input_mode(InputMode::Decimal)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.decimal),
        )
        .child(
            text_input(&s.tel, st)
                .key("tel")
                .single_line()
                .input_mode(InputMode::Tel)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.tel),
        )
        .child(
            text_input(&s.password, st)
                .key("password")
                .single_line()
                .secure()
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.password)
                .on_submit(|f: &mut Form, _| f.submitted.push(f.password.text.clone())),
        )
        .child(
            text_input(&s.pin, st)
                .key("pin")
                .single_line()
                .secure()
                .input_mode(InputMode::Numeric)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.pin),
        )
        .child(
            text_input(&s.none_mode, st)
                .key("none_mode")
                .single_line()
                .input_mode(InputMode::None)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.none_mode),
        )
        .child(
            text_input(&s.read_only, st)
                .key("read_only")
                .single_line()
                .read_only(true)
                .size(280.0, 32.0)
                .on_edit(|f: &mut Form| &mut f.read_only),
        )
        .child(
            el().key("keep_btn")
                .role("button")
                .focusable(true)
                .preserve_focus_on_press(true)
                .size(120.0, 36.0)
                .on_press(|f: &mut Form| f.preserve_taps += 1),
        )
        .child(
            el().key("regular_btn")
                .role("button")
                .focusable(true)
                .size(120.0, 36.0)
                .on_press(|f: &mut Form| f.regular_taps += 1),
        )
}

fn make_form() -> Runtime<Form> {
    let mut rt = Runtime::new(Form::default(), form_view, (360.0, 800.0), Env::new(true, 3.0));
    rt.frame(0.0);
    rt
}

#[test]
fn password_field_raises_password_keyboard_accepts_committed_text_and_drops_preedit() {
    let mut rt = make_form();
    let mut host = attach_resumed_host();
    assert!(sync_host(&mut rt, &mut host).is_empty());

    // Tap the password field -> Android raises password keyboard with NO_PERSONALIZED_LEARNING,
    // while desktop rt.ime_area stays None.
    tap_key(&mut rt, 1, "password", 0.0);
    assert!(rt.has_secure_focus());
    assert!(rt.ime_area.is_none(), "desktop ime_area must stay None on secure fields");

    let expected_pw = EditorConfig {
        input_type: 0x81,
        ime_options: base_options() | IME_FLAG_NO_PERSONALIZED_LEARNING | IME_ACTION_DONE,
    };
    assert_eq!(
        sync_host(&mut rt, &mut host),
        [Effect::SetIme {
            editor: Some(expected_pw),
            restart: true,
        }],
    );

    // Proof that raw Ime::Commit directly into TextEdit::ime drops on a secure field.
    rt.ime(Ime::Commit("dropped".into()));
    rt.frame(0.016);
    assert_eq!(rt.state.password.text, "");

    // Through Android InputConnection commitText, committed text routes as Key::Text into the
    // secure field.
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::Ime(commit_text_event("s3cr".into())),
    ));
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::Ime(commit_text_event("et!".into())),
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.password.text, "s3cret!");

    // Stray setComposingText / finishComposingText from a keyboard are ignored on secure fields.
    assert!(!apply_text_input(
        rt.core_mut(),
        TextInputAction::Ime(composing_text_event("abc".into(), 1)),
    ));
    assert!(!apply_text_input(
        rt.core_mut(),
        TextInputAction::FinishCompositionFromIme,
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.password.text, "s3cret!");
    assert_eq!(rt.state.password.preedit(), None);

    // deleteSurroundingText removes characters from the password field.
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::DeleteSurrounding { before: 1, after: 0 },
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.password.text, "s3cret");

    // IME_ACTION_DONE submits the password field as Return.
    let done = map_editor_action(IME_ACTION_DONE).unwrap();
    assert!(apply_text_input(
        rt.core_mut(),
        TextInputAction::KeyDown { event: done, repeat: false },
    ));
    rt.frame(0.016);
    assert_eq!(rt.state.submitted, ["s3cret"]);
}

#[test]
fn switching_between_field_kinds_restarts_input_and_none_mode_hides_keyboard() {
    let mut rt = make_form();
    let mut host = attach_resumed_host();
    let base = base_options();

    let sequence: [(&str, i32, i32); 8] = [
        ("notes", 0x2_0001, base),
        ("email", 0x21, base | IME_ACTION_DONE),
        ("url", 0x11, base | IME_ACTION_DONE),
        ("search", 0x01, base | IME_ACTION_SEARCH),
        ("numeric", 0x02, base | IME_ACTION_DONE),
        ("decimal", 0x2002, base | IME_ACTION_DONE),
        ("tel", 0x03, base | IME_ACTION_DONE),
        ("pin", 0x12, base | IME_FLAG_NO_PERSONALIZED_LEARNING | IME_ACTION_DONE),
    ];

    for (idx, (key, expected_type, expected_opts)) in sequence.into_iter().enumerate() {
        tap_key(&mut rt, (idx + 1) as u64, key, idx as f64 * 0.2);
        let expected = EditorConfig {
            input_type: expected_type,
            ime_options: expected_opts,
        };
        assert_eq!(
            sync_host(&mut rt, &mut host),
            [Effect::SetIme {
                editor: Some(expected),
                restart: true,
            }],
            "field {key:?} must restart input with expected EditorInfo",
        );
        // Steady frame emits no redundant SetIme.
        rt.frame(0.016);
        assert!(sync_host(&mut rt, &mut host).is_empty(), "steady frame on {key:?}");
    }

    // Switching to InputMode::None closes the soft keyboard, while hardware keys still type.
    tap_key(&mut rt, 20, "none_mode", 4.0);
    assert_eq!(
        sync_host(&mut rt, &mut host),
        [Effect::SetIme {
            editor: None,
            restart: true,
        }],
    );
    assert_eq!(host.ime_editor, None);

    // Re-tapping InputMode::None keeps the keyboard away.
    tap_key(&mut rt, 21, "none_mode", 4.2);
    assert!(sync_host(&mut rt, &mut host).is_empty());

    // Hardware key still reaches the InputMode::None field.
    rt.key(KeyEvent::from(Key::Text("9".into())));
    rt.frame(0.016);
    assert_eq!(rt.state.none_mode.text, "9");

    // Read-only field also keeps the soft keyboard away.
    tap_key(&mut rt, 22, "read_only", 4.5);
    assert!(sync_host(&mut rt, &mut host).is_empty());
}

#[test]
fn tapping_already_focused_field_reshows_keyboard_without_restarting() {
    let mut rt = make_form();
    let mut host = attach_resumed_host();

    // First tap focuses email and opens keyboard with restart = true.
    tap_key(&mut rt, 1, "email", 0.0);
    let email_cfg = EditorConfig {
        input_type: 0x21,
        ime_options: base_options() | IME_ACTION_DONE,
    };
    assert_eq!(
        sync_host(&mut rt, &mut host),
        [Effect::SetIme {
            editor: Some(email_cfg),
            restart: true,
        }],
    );

    // Simulate Back hiding the soft keyboard while the field keeps focus in Rune
    // (Android's IME consumes the first Back press; keyboard_inset drops to 0).
    let _ = machine::step(
        &mut host,
        Event::InsetsChanged {
            safe_area_px: [0; 4],
            keyboard_px: 0,
            scale: 3.0,
        },
    );
    assert!(rt.state.email.focused);

    // Tapping the same focused field again reports field_tap -> re-shows keyboard with restart=false.
    tap_key(&mut rt, 2, "email", 0.5);
    assert_eq!(
        sync_host(&mut rt, &mut host),
        [Effect::SetIme {
            editor: Some(email_cfg),
            restart: false,
        }],
    );

    // Next steady frame emits nothing (field_tap was consumed).
    rt.frame(0.016);
    assert!(sync_host(&mut rt, &mut host).is_empty());

    // Same re-tap behaviour works for a focused password field.
    tap_key(&mut rt, 3, "password", 1.0);
    let pw_cfg = EditorConfig {
        input_type: 0x81,
        ime_options: base_options() | IME_FLAG_NO_PERSONALIZED_LEARNING | IME_ACTION_DONE,
    };
    assert_eq!(
        sync_host(&mut rt, &mut host),
        [Effect::SetIme {
            editor: Some(pw_cfg),
            restart: true,
        }],
    );
    tap_key(&mut rt, 4, "password", 1.5);
    assert_eq!(
        sync_host(&mut rt, &mut host),
        [Effect::SetIme {
            editor: Some(pw_cfg),
            restart: false,
        }],
    );
}

#[test]
fn preserve_focus_on_press_keeps_keyboard_open_while_regular_button_closes_it() {
    let mut rt = make_form();
    let mut host = attach_resumed_host();

    tap_key(&mut rt, 1, "notes", 0.0);
    let notes_cfg = EditorConfig::default_multiline();
    assert_eq!(
        sync_host(&mut rt, &mut host),
        [Effect::SetIme {
            editor: Some(notes_cfg),
            restart: true,
        }],
    );

    // Tapping a button with preserve_focus_on_press(true) fires on_press and leaves focus +
    // soft keyboard untouched with zero SetIme chatter.
    tap_key(&mut rt, 2, "keep_btn", 0.4);
    assert_eq!(rt.state.preserve_taps, 1);
    assert!(rt.state.notes.focused);
    assert!(sync_host(&mut rt, &mut host).is_empty());
    assert_eq!(host.ime_editor, Some(notes_cfg));

    // Tapping a normal focusable button follows Rune's cross-platform focus rule: moves focus
    // to the button and closes the soft keyboard.
    tap_key(&mut rt, 3, "regular_btn", 0.8);
    assert_eq!(rt.state.regular_taps, 1);
    assert!(!rt.state.notes.focused);
    assert_eq!(
        sync_host(&mut rt, &mut host),
        [Effect::SetIme {
            editor: None,
            restart: true,
        }],
    );
    assert_eq!(host.ime_editor, None);
}

#[test]
fn app_owned_ime_cursor_still_gets_default_multiline_keyboard() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .size(360.0, 200.0)
            .child(el().key("caret").size(2.0, 18.0).ime_cursor())
    }

    let mut rt = Runtime::new((), view, (360.0, 200.0), Env::new(true, 3.0));
    rt.frame(0.0);
    assert!(rt.ime_area.is_some());
    assert_eq!(
        active_editor_config(rt.core()),
        Some(EditorConfig::default_multiline()),
    );
}
