use rune::{Cx, El, Env, Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode as Code, Runtime, ShortcutError};
use rune_kit::{Kit, ShortcutRecorder, ShortcutRecorderState};

struct State { recorder: ShortcutRecorderState, reject: bool, disabled: bool, changes: usize, worded: bool, unmodified: bool }
fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut recorder = ShortcutRecorder::new("shortcut", &s.recorder, |s: &mut State| &mut s.recorder)
        .disabled(s.disabled)
        .on_change(|s, _, _| { s.changes += 1; if s.reject { Err(ShortcutError::Conflict) } else { Ok(()) } });
    if s.unmodified { recorder = recorder.allow_unmodified(); }
    if s.worded {
        recorder = recorder.error_message(|error, chord| format!("{error:?}: {}", chord.map(|c| c.to_string()).unwrap_or_default()));
    }
    rune::col().size(400.0, 200.0).child(recorder.build(&Kit::neutral(true)))
        .child(rune::el().key("other").size(100.0, 32.0).focusable(true))
}
fn runtime() -> Runtime<State> {
    let mut rt = Runtime::new(State { recorder: ShortcutRecorderState::new(Some("ctrl+k".parse().unwrap())), reject: false, disabled: false, changes: 0, worded: false, unmodified: false },
        view, (400.0, 200.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0); rt
}
fn shows(rt: &Runtime<State>, s: &str) -> bool { rt.elements().iter().any(|e| e.text.as_deref() == Some(s)) }
fn click(rt: &mut Runtime<State>, name: &str) {
    let (x, y) = rt.find(name).unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y);
}
fn physical(rt: &mut Runtime<State>, key: Code, mods: Mods, pressed: bool, repeat: bool) {
    rt.physical_input(PhysicalInput::Key { key, mods, pressed, repeat }); rt.frame(0.0);
}

#[test]
fn click_and_chord_in_one_frame_capture_physical_key_and_ignore_repeat() {
    let mut rt = runtime(); click(&mut rt, "shortcut");
    physical(&mut rt, Code::KeyQ, Mods { ctrl: true, ..Mods::default() }, true, false);
    assert_eq!(rt.state.recorder.shortcut().unwrap().to_string(), "ctrl+q");
    assert!(!rt.state.recorder.is_recording());
    physical(&mut rt, Code::KeyQ, Mods::default(), true, true);
    assert_eq!(rt.state.changes, 1);
}

#[test]
fn held_modifiers_preview_release_cancel_and_clear() {
    let mut rt = runtime(); click(&mut rt, "shortcut");
    rt.physical_input(PhysicalInput::Modifiers(Mods { ctrl: true, shift: true, ..Mods::default() })); rt.frame(0.0);
    assert_eq!(rt.state.recorder.keycaps().len(), 2);
    physical(&mut rt, Code::ShiftLeft, Mods { ctrl: true, ..Mods::default() }, false, false);
    assert_eq!(rt.state.recorder.keycaps().len(), 1);
    assert_eq!(rt.state.changes, 0);
    physical(&mut rt, Code::Escape, Mods::default(), true, false);
    assert!(!rt.state.recorder.is_recording());
    assert_eq!(rt.state.recorder.shortcut().unwrap().to_string(), "ctrl+k");
    click(&mut rt, "shortcut");
    physical(&mut rt, Code::Backspace, Mods::default(), true, false);
    assert!(rt.state.recorder.shortcut().is_none());
    assert_eq!(rt.state.changes, 1);
}

#[test]
fn conflicts_keep_old_binding_and_unchanged_chords_skip_registration() {
    let mut rt = runtime(); rt.state.reject = true; click(&mut rt, "shortcut");
    physical(&mut rt, Code::KeyJ, Mods { ctrl: true, ..Mods::default() }, true, false);
    assert_eq!(rt.state.recorder.error(), Some(&ShortcutError::Conflict));
    assert_eq!(rt.state.recorder.shortcut().unwrap().to_string(), "ctrl+k");
    assert!(rt.state.recorder.is_recording());
    physical(&mut rt, Code::KeyK, Mods { ctrl: true, ..Mods::default() }, true, false);
    assert!(rt.state.recorder.error().is_none());
    assert!(!rt.state.recorder.is_recording());
    assert_eq!(rt.state.changes, 1);
    click(&mut rt, "shortcut");
    physical(&mut rt, Code::Backspace, Mods::default(), true, false);
    assert_eq!(rt.state.recorder.error(), Some(&ShortcutError::Conflict));
    assert!(rt.state.recorder.shortcut().is_some());
    physical(&mut rt, Code::Escape, Mods::default(), true, false);
    assert!(rt.state.recorder.error().is_none());
    assert!(!rt.state.recorder.is_recording());
    assert_eq!(rt.state.recorder.shortcut().unwrap().to_string(), "ctrl+k");
}

#[test]
fn keyboard_activation_and_both_kinds_of_blur() {
    let mut rt = runtime();
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() }); rt.frame(0.0);
    assert!(rt.state.recorder.is_recording());
    click(&mut rt, "other"); rt.frame(0.0);
    assert!(!rt.state.recorder.is_recording());
    click(&mut rt, "shortcut"); rt.physical_input(PhysicalInput::FocusLost); rt.frame(0.0);
    assert!(!rt.state.recorder.is_recording());
    assert_eq!(rt.state.changes, 0);
}

#[test]
fn disabling_a_focused_recorder_cancels_capture() {
    let mut rt = runtime(); click(&mut rt, "shortcut"); rt.frame(0.0);
    assert!(rt.state.recorder.is_recording());
    rt.state.disabled = true; rt.invalidate(); rt.frame(0.0);
    assert!(!rt.state.recorder.is_recording());
    physical(&mut rt, Code::KeyJ, Mods::default(), true, false);
    assert_eq!(rt.state.changes, 0);
}

#[test]
fn a_refused_chord_stays_on_the_keycaps_with_a_message_naming_it() {
    let mut rt = runtime(); rt.state.reject = true; click(&mut rt, "shortcut");
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    physical(&mut rt, Code::KeyJ, ctrl, true, false);
    let refused: rune::Shortcut = "ctrl+j".parse().unwrap();
    assert_eq!(rt.state.recorder.refused(), Some(&refused));
    // Releasing the key and its modifier keeps showing what was refused.
    physical(&mut rt, Code::KeyJ, ctrl, false, false);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default())); rt.frame(0.0);
    assert_eq!(rt.state.recorder.keycaps(), refused.keycaps());
    let chord = refused.keycaps().join(if cfg!(target_os = "macos") { "" } else { "+" });
    assert!(shows(&rt, &format!("{chord} is already in use.")));
    rt.state.worded = true; rt.invalidate(); rt.frame(0.0);
    assert!(shows(&rt, "Conflict: ctrl+j"));
    // A new modifier starts the next chord.
    rt.physical_input(PhysicalInput::Modifiers(Mods { alt: true, ..Mods::default() })); rt.frame(0.0);
    assert!(rt.state.recorder.refused().is_none() && rt.state.recorder.error().is_none());
    assert_eq!(rt.state.recorder.keycaps().len(), 1);
    assert!(rt.state.recorder.is_recording());
}

#[test]
fn a_chord_without_a_modifier_is_refused_before_on_change_but_f_keys_pass() {
    let add = if cfg!(target_os = "macos") { "Add ⌘, ⌃ or ⌥." } else { "Add Ctrl, Alt or Super." };
    let mut rt = runtime(); click(&mut rt, "shortcut");
    for (key, mods) in [(Code::Space, Mods::default()), (Code::Enter, Mods::default()), (Code::KeyA, Mods { shift: true, ..Mods::default() })] {
        rt.physical_input(PhysicalInput::Modifiers(Mods { alt: true, ..Mods::default() })); rt.frame(0.0);
        rt.physical_input(PhysicalInput::Modifiers(Mods::default())); rt.frame(0.0);
        physical(&mut rt, key, mods, true, false);
        physical(&mut rt, key, mods, false, false);
        assert_eq!(rt.state.changes, 0, "{key:?} never reaches on_change");
        assert!(matches!(rt.state.recorder.error(), Some(ShortcutError::InvalidAccelerator(_))), "{key:?}");
        assert_eq!(rt.state.recorder.refused().map(|s| s.physical_key()), Some(key));
        assert!(shows(&rt, add), "{key:?}");
        assert!(rt.state.recorder.is_recording());
    }
    assert_eq!(rt.state.recorder.shortcut().unwrap().to_string(), "ctrl+k");
    physical(&mut rt, Code::F5, Mods::default(), true, false);
    assert_eq!(rt.state.recorder.shortcut().unwrap().to_string(), "f5");
    assert_eq!(rt.state.changes, 1);
    rt.state.unmodified = true; rt.invalidate(); rt.frame(0.0);
    click(&mut rt, "shortcut");
    physical(&mut rt, Code::Space, Mods::default(), true, false);
    assert_eq!(rt.state.recorder.shortcut().unwrap().to_string(), "space");
    assert_eq!(rt.state.changes, 2);
}

#[test]
fn recording_the_same_chord_retries_it_after_mark_unregistered() {
    let mut rt = runtime(); rt.state.reject = true;
    rt.state.recorder.mark_unregistered();
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    click(&mut rt, "shortcut");
    physical(&mut rt, Code::KeyK, ctrl, true, false);
    assert_eq!(rt.state.changes, 1, "an unregistered chord is proposed again");
    assert_eq!(rt.state.recorder.error(), Some(&ShortcutError::Conflict));
    physical(&mut rt, Code::KeyK, ctrl, false, false);
    rt.state.reject = false;
    rt.physical_input(PhysicalInput::Modifiers(Mods::default())); rt.frame(0.0);
    physical(&mut rt, Code::KeyK, ctrl, true, false);
    physical(&mut rt, Code::KeyK, ctrl, false, false);
    assert_eq!(rt.state.changes, 2);
    assert!(!rt.state.recorder.is_recording() && rt.state.recorder.error().is_none());
    // Registered again: the same chord is a no-op once more.
    click(&mut rt, "shortcut");
    physical(&mut rt, Code::KeyK, ctrl, true, false);
    assert_eq!(rt.state.changes, 2);
    assert_eq!(rt.state.recorder.shortcut().unwrap().to_string(), "ctrl+k");
}
