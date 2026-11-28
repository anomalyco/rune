//! Host-side tests for Android `InputConnection` paragraph-window queries, UTF-16 surrogate-pair
//! conversions, mid-composition queries, `setComposingRegion`, `setSelection`, range-based
//! `deleteSurroundingText` / `deleteSurroundingTextInCodePoints`, `updateSelection`, and
//! paragraph-change `restartInput`.

use crate::edit::{ImeText, TextEdit};
use crate::{Cx, El, Env, Key, KeyEvent, Runtime, TextStyle, col, text_input};

use super::editor;
use super::ime_window::{
    ImeEdit, ImeWindow, SelectionState, WindowSync, apply_ime_edit, byte_to_utf16, utf16_len,
    utf16_to_byte_ceil, utf16_to_byte_floor,
};
use super::input::{KEYCODE_ENTER, TextInputAction, apply_text_input, map_key_event};
use super::machine::{self, Effect, Event, State};

#[test]
fn utf16_conversions_handle_accents_cjk_and_surrogate_pairs_without_splitting() {
    // 'a' (1B, 1u16), 'é' (2B, 1u16), '日' (3B, 1u16), '😀' (4B, 2u16), 'b' (1B, 1u16)
    let s = "aé日😀b";
    assert_eq!(utf16_len(s), 6);

    let b_a = 0;
    let b_e = 1;
    let b_cjk = 3;
    let b_emoji = 6;
    let b_b = 10;
    let b_end = 11;

    assert_eq!(byte_to_utf16(s, b_a), 0);
    assert_eq!(byte_to_utf16(s, b_e), 1);
    assert_eq!(byte_to_utf16(s, b_cjk), 2);
    assert_eq!(byte_to_utf16(s, b_emoji), 3);
    assert_eq!(byte_to_utf16(s, b_b), 5);
    assert_eq!(byte_to_utf16(s, b_end), 6);

    // Exact boundaries round-trip for both floor and ceil.
    for (u16_off, byte_off) in [(0, b_a), (1, b_e), (2, b_cjk), (3, b_emoji), (5, b_b), (6, b_end)]
    {
        assert_eq!(utf16_to_byte_floor(s, u16_off), byte_off, "floor({u16_off})");
        assert_eq!(utf16_to_byte_ceil(s, u16_off), byte_off, "ceil({u16_off})");
    }

    // UTF-16 offset 4 falls right between the high and low surrogates of '😀':
    // floor snaps to start of '😀' (byte 6), ceil snaps to end of '😀' (byte 10).
    assert_eq!(utf16_to_byte_floor(s, 4), b_emoji);
    assert_eq!(utf16_to_byte_ceil(s, 4), b_b);

    // Out-of-bounds clamps to s.len().
    assert_eq!(utf16_to_byte_floor(s, 99), b_end);
    assert_eq!(utf16_to_byte_ceil(s, 99), b_end);

    // ZWJ family emoji: 4 supplementary scalars + 3 ZWJs = 11 UTF-16 code units.
    let family = "👨‍👩‍👧‍👦";
    assert_eq!(utf16_len(family), 11);
    for u in 0..=12 {
        assert!(family.is_char_boundary(utf16_to_byte_floor(family, u)));
        assert!(family.is_char_boundary(utf16_to_byte_ceil(family, u)));
        assert!(utf16_to_byte_floor(family, u) <= utf16_to_byte_ceil(family, u));
    }
}

#[test]
fn cursor_queries_never_split_surrogate_pairs_and_respect_selection() {
    let mut w = ImeWindow::default();
    // "a😀b👋c": bytes a=0..1, 😀=1..5, b=5..6, 👋=6..10, c=10..11
    let text = "a😀b👋c".to_owned();
    let _ = w.sync_after_frame(
        true,
        Some(ImeText {
            text,
            start: 0,
            selection: 5..6, // "b" selected
        }),
        None,
        true,
    );

    assert_eq!(w.selected_text().as_deref(), Some("b"));
    assert_eq!(w.window_text().as_deref(), Some("a😀b👋c"));
    let sel = SelectionState {
        sel_start: 3,
        sel_end: 4,
        composing_start: -1,
        composing_end: -1,
    };
    assert_eq!(w.selection_state(), Some(sel));
    assert_eq!(sel.to_array(), [3, 4, -1, -1]);

    // Before "b" is "a😀" (3 UTF-16 units; 😀 is 2 units).
    assert_eq!(w.text_before_cursor(-1), None);
    assert_eq!(w.text_before_cursor(0).as_deref(), Some(""));
    // Requesting 1 UTF-16 unit before '😀' must NOT split its surrogate pair or exceed 1 unit.
    assert_eq!(w.text_before_cursor(1).as_deref(), Some(""));
    assert_eq!(w.text_before_cursor(2).as_deref(), Some("😀"));
    assert_eq!(w.text_before_cursor(3).as_deref(), Some("a😀"));
    assert_eq!(w.text_before_cursor(100).as_deref(), Some("a😀"));

    // After "b" is "👋c" (3 UTF-16 units; 👋 is 2 units).
    assert_eq!(w.text_after_cursor(-1), None);
    assert_eq!(w.text_after_cursor(0).as_deref(), Some(""));
    assert_eq!(w.text_after_cursor(1).as_deref(), Some(""));
    assert_eq!(w.text_after_cursor(2).as_deref(), Some("👋"));
    assert_eq!(w.text_after_cursor(3).as_deref(), Some("👋c"));
    assert_eq!(w.text_after_cursor(100).as_deref(), Some("👋c"));
}

#[test]
fn mid_composition_queries_answer_from_pre_composition_window() {
    let mut w = ImeWindow::default();
    // Pre-composition snapshot: "hello " with caret at end.
    let sync = w.sync_after_frame(
        true,
        Some(ImeText {
            text: "hello ".into(),
            start: 0,
            selection: 6..6,
        }),
        None,
        true,
    );
    assert_eq!(sync, WindowSync::NONE);

    // Next frame: composition opens with "😀x"; RuntimeCore::ime_text() is None while composing!
    let sync = w.sync_after_frame(true, None, Some(("😀x", Some((5, 5)))), false);
    assert!(w.is_composing());
    assert_eq!(
        sync,
        WindowSync {
            restart_input: false,
            update_selection: Some(SelectionState {
                sel_start: 9,       // 6 + 2 + 1
                sel_end: 9,
                composing_start: 6,
                composing_end: 9,
            }),
        }
    );
    assert_eq!(w.window_text().as_deref(), Some("hello 😀x"));
    assert_eq!(w.text_before_cursor(100).as_deref(), Some("hello 😀x"));
    assert_eq!(w.text_after_cursor(100).as_deref(), Some(""));
    assert_eq!(w.selected_text(), None);

    // Idle frame with same preedit emits no redundant updateSelection.
    let idle = w.sync_after_frame(true, None, Some(("😀x", Some((5, 5)))), false);
    assert_eq!(idle, WindowSync::NONE);
}

struct EditorState {
    edit: TextEdit,
    secret: TextEdit,
    readonly: TextEdit,
}

fn editor_view(s: &EditorState, _: &mut Cx<EditorState>) -> El {
    col()
        .size(320.0, 240.0)
        .child(
            text_input(&s.edit, TextStyle::new(14.0, 400.0))
                .key("edit")
                .label("Editor")
                .on_edit(|st: &mut EditorState| &mut st.edit)
                .on_submit(|st: &mut EditorState, _| {
                    st.edit.insert("\n");
                })
                .size(280.0, 100.0),
        )
        .child(
            text_input(&s.secret, TextStyle::new(14.0, 400.0))
                .key("secret")
                .label("Password")
                .on_edit(|st: &mut EditorState| &mut st.secret)
                .size(280.0, 40.0),
        )
        .child(
            text_input(&s.readonly, TextStyle::new(14.0, 400.0))
                .key("readonly")
                .label("Read only")
                .read_only(true)
                .size(280.0, 40.0),
        )
}

struct TestHost {
    rt: Runtime<EditorState>,
    ime_window: ImeWindow,
    machine: State,
}

impl TestHost {
    fn new(initial: &str) -> Self {
        let edit = TextEdit::new(initial);
        let mut secret = TextEdit::new("top-secret");
        secret.set_secure(true);
        let readonly = TextEdit::new("read-only");
        let state = EditorState {
            edit,
            secret,
            readonly,
        };
        let mut rt = Runtime::new(state, editor_view, (320.0, 240.0), Env::new(true, 2.0));
        rt.frame(0.016);
        let mut host = State::default();
        let _ = machine::step(&mut host, Event::Resumed);
        let _ = machine::step(
            &mut host,
            Event::SurfaceCreated {
                width_px: 640,
                height_px: 480,
                scale: 2.0,
                dark: true,
            },
        );
        Self {
            rt,
            ime_window: ImeWindow::default(),
            machine: host,
        }
    }

    fn focus_at(&mut self, y: f32) -> (Vec<Effect>, WindowSync) {
        self.rt.pointer_down(260.0, y);
        self.rt.pointer_up(260.0, y);
        self.step_frame()
    }

    fn step_frame(&mut self) -> (Vec<Effect>, WindowSync) {
        let _ = machine::step(&mut self.machine, Event::Frame);
        self.rt.frame(0.016);
        let core = self.rt.core_mut();
        let ed = editor::sample_editor(core);
        let ime_allowed = ed.config.is_some();
        let ime_text = if ime_allowed { core.ime_text() } else { None };
        let preedit = if ime_allowed { core.focused_preedit() } else { None };
        let window_sync =
            self.ime_window
                .sync_after_frame(ime_allowed, ime_text, preedit, ed.reset);
        let effects: Vec<_> = machine::step(
            &mut self.machine,
            Event::FrameDone {
                animating: false,
                wake_after_secs: None,
                editor: ed.config,
                ime_restart: ed.reset || window_sync.restart_input,
                field_tap: ed.field_tap,
            },
        )
        .into_iter()
        .collect();
        (effects, window_sync)
    }

    fn ime_edit(&mut self, edit: ImeEdit) -> bool {
        apply_ime_edit(self.rt.core_mut(), &mut self.ime_window, edit)
    }
}

#[test]
fn gboard_suggestion_pick_replaces_composing_word_without_duplication() {
    let mut h = TestHost::new("hi ");
    let (effects, _sync) = h.focus_at(20.0);
    assert!(matches!(effects.as_slice(), [Effect::SetIme { editor: Some(_), restart: true }]));
    assert_eq!(h.ime_window.text_before_cursor(100).as_deref(), Some("hi "));

    // Type 'w', 'o' via setComposingText.
    assert!(h.ime_edit(ImeEdit::SetComposingText {
        text: "w".into(),
        new_cursor_position: 1,
    }));
    // Optimistic query before next frame already sees "hi w".
    assert_eq!(h.ime_window.text_before_cursor(100).as_deref(), Some("hi w"));
    let (effects, sync) = h.step_frame();
    assert!(effects.is_empty());
    assert_eq!(
        sync.update_selection,
        Some(SelectionState {
            sel_start: 4,
            sel_end: 4,
            composing_start: 3,
            composing_end: 4,
        })
    );

    assert!(h.ime_edit(ImeEdit::SetComposingText {
        text: "wo".into(),
        new_cursor_position: 1,
    }));
    let (_, sync) = h.step_frame();
    assert_eq!(
        sync.update_selection,
        Some(SelectionState {
            sel_start: 5,
            sel_end: 5,
            composing_start: 3,
            composing_end: 5,
        })
    );

    // Pick Gboard suggestion "world ": replaces preedit with zero duplication.
    assert!(h.ime_edit(ImeEdit::CommitText("world ".into())));
    assert_eq!(h.ime_window.text_before_cursor(100).as_deref(), Some("hi world "));
    let (effects, sync) = h.step_frame();
    assert!(effects.is_empty());
    assert_eq!(h.rt.state.edit.text, "hi world ");
    assert_eq!(h.rt.state.edit.preedit(), None);
    assert_eq!(
        sync.update_selection,
        Some(SelectionState {
            sel_start: 9,
            sel_end: 9,
            composing_start: -1,
            composing_end: -1,
        })
    );

    // Same-frame batch: setComposingText + finishComposingText updates shadow and commits cleanly
    // without triggering restartInput.
    assert!(h.ime_edit(ImeEdit::SetComposingText {
        text: "ok".into(),
        new_cursor_position: 1,
    }));
    assert!(h.ime_edit(ImeEdit::FinishComposingText));
    assert!(!h.ime_window.is_composing());
    assert_eq!(h.ime_window.text_before_cursor(100).as_deref(), Some("hi world ok"));
    let (effects, sync) = h.step_frame();
    assert!(effects.is_empty(), "finishComposingText must not restartInput");
    assert!(!sync.restart_input);
    assert_eq!(h.rt.state.edit.text, "hi world ok");
    assert_eq!(
        sync.update_selection,
        Some(SelectionState {
            sel_start: 11,
            sel_end: 11,
            composing_start: -1,
            composing_end: -1,
        })
    );
}

#[test]
fn backspace_into_word_recompose_and_replace_keeps_caret_in_place() {
    let mut h = TestHost::new("hello ");
    let _ = h.focus_at(20.0);

    // Batch: Gboard deletes trailing space, reads word before cursor, marks it composing.
    assert!(h.ime_edit(ImeEdit::DeleteSurrounding {
        before: 1,
        after: 0,
        in_code_points: false,
    }));
    assert_eq!(h.ime_window.text_before_cursor(100).as_deref(), Some("hello"));
    assert!(h.ime_edit(ImeEdit::SetComposingRegion { start: 0, end: 5 }));
    let (_, sync) = h.step_frame();
    assert_eq!(h.rt.state.edit.text, "hello");
    assert_eq!(h.rt.state.edit.preedit(), Some(("hello", Some((5, 5)))));
    assert_eq!(
        sync.update_selection,
        Some(SelectionState {
            sel_start: 5,
            sel_end: 5,
            composing_start: 0,
            composing_end: 5,
        })
    );

    // User types 'p' -> Gboard updates composing text to "hellop" then picks "help".
    assert!(h.ime_edit(ImeEdit::SetComposingText {
        text: "hellop".into(),
        new_cursor_position: 1,
    }));
    let _ = h.step_frame();
    assert_eq!(h.rt.state.edit.display().0, "hellop");

    assert!(h.ime_edit(ImeEdit::CommitText("help".into())));
    let _ = h.step_frame();
    assert_eq!(h.rt.state.edit.text, "help");
    assert_eq!(h.rt.state.edit.preedit(), None);
}

#[test]
fn set_composing_region_mid_word_keeps_caret_and_empty_edit_deletes_region() {
    let mut h = TestHost::new("café 😀 world");
    let _ = h.focus_at(20.0);

    // Move caret inside "😀" -> right after "😀" (UTF-16 offset: c=0,a=1,f=2,é=3,' '=4,😀=5..7).
    assert!(h.ime_edit(ImeEdit::SetSelection { start: 7, end: 7 }));
    let _ = h.step_frame();
    assert_eq!(h.ime_window.text_before_cursor(100).as_deref(), Some("café 😀"));
    assert_eq!(h.ime_window.text_after_cursor(100).as_deref(), Some(" world"));

    // Mark "😀" (UTF-16 5..7) as composing region; caret stays at end of "😀".
    assert!(h.ime_edit(ImeEdit::SetComposingRegion { start: 5, end: 7 }));
    let (_, sync) = h.step_frame();
    assert_eq!(h.rt.state.edit.preedit(), Some(("😀", Some((4, 4)))));
    assert_eq!(
        sync.update_selection,
        Some(SelectionState {
            sel_start: 7,
            sel_end: 7,
            composing_start: 5,
            composing_end: 7,
        })
    );

    // Backspacing the last char of a recomposed region sends empty setComposingText: must delete "😀"!
    assert!(h.ime_edit(ImeEdit::SetComposingText {
        text: String::new(),
        new_cursor_position: 1,
    }));
    let _ = h.step_frame();
    assert_eq!(h.rt.state.edit.text, "café  world");
    assert_eq!(h.rt.state.edit.preedit(), None);

    // Move caret into the middle of "café" (UTF-16 offset 2: "ca|fé"), then setComposingRegion(0, 4).
    assert!(h.ime_edit(ImeEdit::SetSelection { start: 2, end: 2 }));
    let _ = h.step_frame();
    assert!(h.ime_edit(ImeEdit::SetComposingRegion { start: 0, end: 4 }));
    let (_, sync) = h.step_frame();
    assert_eq!(h.rt.state.edit.preedit(), Some(("café", Some((2, 2)))));
    assert_eq!(
        sync.update_selection,
        Some(SelectionState {
            sel_start: 2,
            sel_end: 2,
            composing_start: 0,
            composing_end: 4,
        })
    );

    // Back-to-back setComposingRegion(0, 2) finishes previous region first so Ime::Select runs cleanly.
    assert!(h.ime_edit(ImeEdit::SetComposingRegion { start: 0, end: 2 }));
    let _ = h.step_frame();
    assert_eq!(h.rt.state.edit.preedit(), Some(("ca", Some((2, 2)))));

    // Empty commitText deletes the recomposed region "ca".
    assert!(h.ime_edit(ImeEdit::CommitText(String::new())));
    let _ = h.step_frame();
    assert_eq!(h.rt.state.edit.text, "fé  world");
}

#[test]
fn delete_surrounding_handles_utf16_emoji_code_points_and_selection_exclusion() {
    let mut h = TestHost::new("a😀b👋c");
    let _ = h.focus_at(20.0);

    // Caret after "a😀" (UTF-16 offset 3): deleteSurroundingText(1, 0) tries to split surrogate ->
    // expands to take whole "😀" cleanly instead of getting stuck.
    assert!(h.ime_edit(ImeEdit::SetSelection { start: 3, end: 3 }));
    let _ = h.step_frame();
    assert!(h.ime_edit(ImeEdit::DeleteSurrounding {
        before: 1,
        after: 0,
        in_code_points: false,
    }));
    let _ = h.step_frame();
    assert_eq!(h.rt.state.edit.text, "ab👋c");

    // Caret after "ab" (UTF-16 offset 2): deleteSurroundingTextInCodePoints(0, 1) deletes "👋" (1 scalar).
    assert!(h.ime_edit(ImeEdit::SetSelection { start: 2, end: 2 }));
    let _ = h.step_frame();
    assert!(h.ime_edit(ImeEdit::DeleteSurrounding {
        before: 0,
        after: 1,
        in_code_points: true,
    }));
    let _ = h.step_frame();
    assert_eq!(h.rt.state.edit.text, "abc");

    // Non-empty selection "b" (1..2) in "abc": deleteSurroundingText(1, 1) deletes "a" and "c",
    // preserving the selected "b" per Android's InputConnection contract.
    assert!(h.ime_edit(ImeEdit::SetSelection { start: 1, end: 2 }));
    let _ = h.step_frame();
    assert_eq!(h.ime_window.selected_text().as_deref(), Some("b"));
    assert!(h.ime_edit(ImeEdit::DeleteSurrounding {
        before: 1,
        after: 1,
        in_code_points: false,
    }));
    let _ = h.step_frame();
    assert_eq!(h.rt.state.edit.text, "b");
    assert_eq!(h.ime_window.selected_text().as_deref(), Some("b"));
}

#[test]
fn paragraph_change_restarts_input_and_col0_delete_crosses_newline() {
    let mut h = TestHost::new("line1");
    let _ = h.focus_at(20.0);
    assert_eq!(h.ime_window.window_text().as_deref(), Some("line1"));

    // Press Enter -> moves to paragraph 2 (ImeText::start shifts 0 -> 6) -> triggers restartInput!
    let enter = map_key_event(KEYCODE_ENTER, 0, 0, 0).unwrap();
    assert!(apply_text_input(
        h.rt.core_mut(),
        TextInputAction::KeyDown {
            event: enter,
            repeat: false,
        },
    ));
    let (effects, sync) = h.step_frame();
    assert!(sync.restart_input, "moving to a new paragraph must request restartInput");
    assert_eq!(sync.update_selection, None, "restartInput re-reads initialSelStart/End");
    assert!(matches!(effects.as_slice(), [Effect::SetIme { editor: Some(_), restart: true }]));
    assert_eq!(h.ime_window.window_text().as_deref(), Some(""));
    assert_eq!(
        h.ime_window.selection_state(),
        Some(SelectionState {
            sel_start: 0,
            sel_end: 0,
            composing_start: -1,
            composing_end: -1,
        })
    );

    // Type "line2" on paragraph 2 -> offsets stay local to paragraph 2, no restartInput.
    assert!(h.ime_edit(ImeEdit::CommitText("line2".into())));
    let (effects, sync) = h.step_frame();
    assert!(effects.is_empty());
    assert!(!sync.restart_input);
    assert_eq!(h.ime_window.window_text().as_deref(), Some("line2"));
    assert_eq!(
        sync.update_selection,
        Some(SelectionState {
            sel_start: 5,
            sel_end: 5,
            composing_start: -1,
            composing_end: -1,
        })
    );

    // Move to column 0 of line 2 and deleteSurroundingText(1, 0): leaves the paragraph window,
    // falls back to Key::Backspace to remove '\n', merges into line 1, and triggers restartInput.
    assert!(h.ime_edit(ImeEdit::SetSelection { start: 0, end: 0 }));
    let _ = h.step_frame();
    assert!(h.ime_edit(ImeEdit::DeleteSurrounding {
        before: 1,
        after: 0,
        in_code_points: false,
    }));
    let (effects, sync) = h.step_frame();
    assert_eq!(h.rt.state.edit.text, "line1line2");
    assert!(sync.restart_input, "merging paragraphs shifts ImeText::start back to 0");
    assert!(matches!(effects.as_slice(), [Effect::SetIme { editor: Some(_), restart: true }]));
    assert_eq!(h.ime_window.window_text().as_deref(), Some("line1line2"));
    assert_eq!(
        h.ime_window.selection_state(),
        Some(SelectionState {
            sel_start: 5,
            sel_end: 5,
            composing_start: -1,
            composing_end: -1,
        })
    );
}

#[test]
fn secure_and_read_only_fields_never_expose_ime_window_text() {
    let mut h = TestHost::new("plain");
    let _ = h.focus_at(20.0);
    assert!(h.ime_window.is_active());

    // Focus secure password field (second child, y = 120).
    let _ = h.focus_at(120.0);
    assert!(!h.ime_window.is_active());
    assert_eq!(h.ime_window.window_text(), None);
    assert_eq!(h.ime_window.text_before_cursor(100), None);
    assert_eq!(h.ime_window.text_after_cursor(100), None);
    assert_eq!(h.ime_window.selected_text(), None);
    assert_eq!(h.ime_window.selection_state(), None);

    // Focus read-only field (third child, y = 160).
    let _ = h.focus_at(160.0);
    assert!(!h.ime_window.is_active());
    assert_eq!(h.ime_window.window_text(), None);

    // Blurring clears window state cleanly.
    h.rt.key(KeyEvent::from(Key::Escape));
    let _ = h.step_frame();
    assert!(!h.ime_window.is_active());
}
