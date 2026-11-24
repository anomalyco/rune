use std::hash::{DefaultHasher, Hash, Hasher};

use rune_text::Flow;

use super::{DirChord, step_dir_chord, step_dir_chord_win};
use crate::{Key, KeyEvent, KeySide, Modifier, Mods, TextEdit};

fn hash_of(edit: &TextEdit) -> u64 {
    let mut h = DefaultHasher::new();
    edit.hash(&mut h);
    h.finish()
}

#[test]
fn ctrl_then_shift_release_sets_ltr_or_rtl() {
    let (s, out) = step_dir_chord_win(DirChord::Idle, Modifier::ControlLeft, true, true, false);
    assert_eq!((s, out), (DirChord::Ctrl, None));
    let (s, out) = step_dir_chord_win(s, Modifier::ShiftRight, true, true, true);
    assert_eq!((s, out), (DirChord::Armed(KeySide::Right), None));
    let (s, out) = step_dir_chord_win(s, Modifier::ShiftRight, false, true, false);
    assert_eq!((s, out), (DirChord::Idle, Some(Flow::Rtl)));

    let (s, _) = step_dir_chord_win(DirChord::Idle, Modifier::ControlRight, true, true, false);
    let (s, _) = step_dir_chord_win(s, Modifier::ShiftLeft, true, true, true);
    let (s, out) = step_dir_chord_win(s, Modifier::ControlRight, false, false, true);
    assert_eq!((s, out), (DirChord::Idle, Some(Flow::Ltr)));
}

#[test]
fn shift_first_or_extra_modifiers_disarm_the_chord() {
    // Shift first, then Ctrl, does not arm.
    let (s, out) = step_dir_chord_win(DirChord::Idle, Modifier::ShiftRight, true, false, true);
    assert_eq!((s, out), (DirChord::Idle, None));
    let (s, out) = step_dir_chord_win(s, Modifier::ControlLeft, true, true, true);
    assert_eq!((s, out), (DirChord::Idle, None));

    // Alt, Super or Fn pressed while Ctrl is held disarms before Shift arrives.
    for other in [Modifier::AltLeft, Modifier::AltRight, Modifier::SuperLeft, Modifier::Fn] {
        let (s, _) = step_dir_chord_win(DirChord::Idle, Modifier::ControlLeft, true, true, false);
        let (s, out) = step_dir_chord_win(s, other, true, true, false);
        assert_eq!((s, out), (DirChord::Idle, None), "{other:?} pressed after Ctrl must disarm");
        let (s, _) = step_dir_chord_win(s, Modifier::ShiftRight, true, true, true);
        let (s, out) = step_dir_chord_win(s, Modifier::ShiftRight, false, true, false);
        assert_eq!((s, out), (DirChord::Idle, None), "chord must stay idle after {other:?}");
    }

    // Pressing a second Shift or Alt while armed disarms it.
    for other in [Modifier::ShiftLeft, Modifier::AltLeft, Modifier::SuperRight] {
        let (s, _) = step_dir_chord_win(DirChord::Idle, Modifier::ControlLeft, true, true, false);
        let (s, _) = step_dir_chord_win(s, Modifier::ShiftRight, true, true, true);
        let (s, out) = step_dir_chord_win(s, other, true, true, true);
        assert_eq!((s, out), (DirChord::Idle, None), "{other:?} while armed must disarm");
    }
}

#[test]
fn step_dir_chord_is_windows_only() {
    let (s, _) = step_dir_chord(DirChord::Idle, Modifier::ControlLeft, true, true, false);
    let (s, _) = step_dir_chord(s, Modifier::ShiftRight, true, true, true);
    let (_, out) = step_dir_chord(s, Modifier::ShiftRight, false, true, false);
    if cfg!(target_os = "windows") {
        assert_eq!(out, Some(Flow::Rtl));
    } else {
        assert_eq!(out, None);
    }
}

#[test]
fn clear_dir_resets_forced_direction_without_losing_text_or_history() {
    let mut edit = TextEdit::new("");
    edit.key(&KeyEvent { key: Key::Text("hello".into()), mods: Mods::default() }, None);
    assert!(edit.has_history());
    let natural_hash = hash_of(&edit);

    edit.force_dir(Flow::Rtl);
    assert_eq!(edit.forced_dir(), Some(Flow::Rtl));
    assert_ne!(hash_of(&edit), natural_hash);

    // Clearing text keeps the forced direction (matching HTML input.value = "").
    let mut cleared = edit.clone();
    cleared.clear();
    assert_eq!(cleared.forced_dir(), Some(Flow::Rtl));
    cleared.clear_dir();
    assert_eq!(cleared.forced_dir(), None);

    // Clearing direction alone restores natural direction, keeps text and undo history.
    edit.clear_dir();
    assert_eq!(edit.forced_dir(), None);
    assert_eq!(edit.text, "hello");
    assert!(edit.has_history());
    assert_eq!(hash_of(&edit), natural_hash);
}
