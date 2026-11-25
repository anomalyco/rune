use super::*;
use crate::{Cx, El, Env, KeySide, TextEdit, TextStyle, col, el, text_input};

#[derive(Default)]
struct Keys { edit: TextEdit, app: Vec<KeyEvent>, field: Vec<KeyEvent>, field_takes: bool, chords: usize,
    recording: bool, recorded: Vec<PhysicalInput>, focus_lost: usize }
fn view(s: &Keys, _: &mut Cx<Keys>) -> El {
    col().size(300.0, 200.0)
        .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).single_line().key("field").size(200.0, 32.0)
            .on_edit(|s: &mut Keys| &mut s.edit)
            .on_input_key(|s: &mut Keys, ev, _| {
                if !matches!(ev.key, Key::Modifier { .. }) { return false; }
                s.field.push(ev.clone());
                s.field_takes
            }))
        .child(el().key("blank").size(100.0, 40.0))
        // A shortcut recorder: it takes every key while recording.
        .child(el().key("recorder").size(100.0, 40.0).focusable(true)
            .on_physical_input(|s: &mut Keys, ev, _| {
                if matches!(ev, PhysicalInput::FocusLost) { s.focus_lost += 1; }
                if s.recording { s.recorded.push(ev.clone()); }
                s.recording
            }))
}
fn runtime(enabled: bool) -> Runtime<Keys> {
    let mut rt = Runtime::new(Keys::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.set_modifier_keys(enabled);
    rt.on_key = Some(|s, ev, _| {
        // A typical chord binding: Cmd with any key.
        if ev.mods.cmd && matches!(ev.key, Key::Text(_) | Key::Command(_)) { s.chords += 1; }
        s.app.push(ev.clone());
        true
    });
    rt.frame(0.0);
    rt
}
fn click(rt: &mut Runtime<Keys>, key: &str) {
    let (x, y) = rt.find(key).unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
}
/// A physical modifier key, as hosts send it: `mods` from before the change, like macOS.
fn modifier(rt: &mut Runtime<Keys>, code: Code, pressed: bool, mods: Mods) -> bool {
    let redraw = rt.physical_with_fallback(PhysicalInput::Key { key: code, mods, pressed, repeat: false, text: None, consumed_mods: Default::default() }, None);
    rt.frame(0.0);
    redraw
}
fn flags(rt: &mut Runtime<Keys>, mods: Mods) -> bool {
    let redraw = rt.physical_with_fallback(PhysicalInput::Modifiers(mods), None);
    rt.frame(0.0);
    redraw
}
fn modifiers(log: &[KeyEvent]) -> Vec<(Modifier, bool)> {
    log.iter().filter_map(|ev| match ev.key { Key::Modifier { modifier, pressed } => Some((modifier, pressed)), _ => None }).collect()
}
const SHIFT: Mods = Mods { shift: true, ctrl: false, alt: false, logo: false, cmd: false };
const ALT: Mods = Mods { shift: false, ctrl: false, alt: true, logo: false, cmd: false };

#[test]
fn shift_alone_sends_press_and_release_with_side_and_mods_after_the_change() {
    let mut rt = runtime(true);
    assert!(modifier(&mut rt, Code::ShiftLeft, true, Mods::default()), "a press needs a frame");
    assert!(!flags(&mut rt, SHIFT), "the flags of a press already sent add nothing");
    assert!(modifier(&mut rt, Code::ShiftLeft, false, SHIFT));
    assert!(!flags(&mut rt, Mods::default()));
    assert_eq!(modifiers(&rt.state.app), [(Modifier::ShiftLeft, true), (Modifier::ShiftLeft, false)]);
    assert_eq!((rt.state.app[0].mods, rt.state.app[1].mods), (SHIFT, Mods::default()));
    assert_eq!(Modifier::ShiftLeft.side(), Some(KeySide::Left));
    assert!(!rt.kbd.focus.keyboard(), "a modifier tap shows no focus ring");
}

#[test]
fn right_option_is_told_apart_from_left_option() {
    let mut rt = runtime(true);
    modifier(&mut rt, Code::AltRight, true, Mods::default());
    modifier(&mut rt, Code::AltLeft, true, ALT);
    modifier(&mut rt, Code::AltRight, false, ALT);
    modifier(&mut rt, Code::AltLeft, false, ALT);
    assert_eq!(modifiers(&rt.state.app), [(Modifier::AltRight, true), (Modifier::AltLeft, true),
        (Modifier::AltRight, false), (Modifier::AltLeft, false)]);
    let alt: Vec<bool> = rt.state.app.iter().map(|ev| ev.mods.alt).collect();
    assert_eq!(alt, [true, true, true, false], "Option stays on while the other side is down");
    assert_eq!((Modifier::AltRight.side(), Modifier::Fn.side()), (Some(KeySide::Right), None));
}

#[test]
fn a_held_modifier_then_a_letter_is_the_usual_chord() {
    let mut rt = runtime(true);
    let cmd = Mods { cmd: true, ..Mods::default() };
    let code = if cfg!(target_os = "macos") { Code::SuperLeft } else { Code::ControlLeft };
    modifier(&mut rt, code, true, Mods::default());
    assert_eq!(rt.state.app[0].mods.cmd, true);
    assert_eq!(rt.state.chords, 0, "the modifier alone is no chord");
    rt.physical_with_fallback(PhysicalInput::Key { key: Code::KeyK, mods: cmd, pressed: true, repeat: false, text: None, consumed_mods: Default::default() },
        Some(KeyEvent { key: Key::Command('k'), mods: cmd }));
    rt.frame(0.0);
    assert_eq!(rt.state.app[1], KeyEvent { key: Key::Command('k'), mods: cmd });
    assert_eq!(rt.state.chords, 1);
    // Shift held types capitals in a field as before.
    click(&mut rt, "field");
    modifier(&mut rt, Code::ShiftRight, true, Mods::default());
    rt.physical_with_fallback(PhysicalInput::Key { key: Code::KeyA, mods: SHIFT, pressed: true, repeat: false, text: None, consumed_mods: Default::default() },
        Some(KeyEvent { key: Key::Text("A".into()), mods: SHIFT }));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "A");
}

#[test]
fn modifier_keys_never_fire_chords_insert_text_or_repeat() {
    let mut rt = runtime(true);
    click(&mut rt, "field");
    rt.state.app.clear();
    for code in [Code::SuperLeft, Code::ControlLeft, Code::AltLeft, Code::ShiftLeft, Code::Fn] {
        modifier(&mut rt, code, true, Mods::default());
        rt.physical_with_fallback(PhysicalInput::Key { key: code, mods: Mods::default(), pressed: true, repeat: true, text: None, consumed_mods: Default::default() }, None);
        rt.frame(0.0);
        modifier(&mut rt, code, false, Mods::default());
    }
    assert_eq!(rt.state.chords, 0);
    assert_eq!(rt.state.edit.text, "");
    assert_eq!(rt.state.app.len(), 10, "one press and one release each, no repeats");
    assert!(rt.state.app.iter().all(|ev| matches!(ev.key, Key::Modifier { .. })));
    // A KeyEvent sent straight to a field's editor changes nothing either.
    let ev = KeyEvent { key: Key::Modifier { modifier: Modifier::ShiftLeft, pressed: true }, mods: SHIFT };
    assert_eq!(rt.state.edit.key(&ev, None), crate::edit::EditOutcome::default());
}

#[test]
fn a_focused_field_hears_modifiers_before_the_window_unless_nothing_is_focused() {
    let mut rt = runtime(true);
    modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
    assert_eq!((rt.state.app.len(), rt.state.field.len()), (1, 0), "nothing focused: the window's on_key");
    modifier(&mut rt, Code::ShiftLeft, false, SHIFT);
    click(&mut rt, "field");
    rt.state.app.clear();
    rt.state.field_takes = true;
    modifier(&mut rt, Code::ControlRight, true, Mods::default());
    assert_eq!((rt.state.app.len(), rt.state.field.len()), (0, 1), "the field took it");
    rt.state.field_takes = false;
    modifier(&mut rt, Code::ControlRight, false, Mods { ctrl: true, ..Mods::default() });
    assert_eq!((rt.state.app.len(), rt.state.field.len()), (1, 2), "the field passed it on");
    assert_eq!(rt.state.app[0].key, Key::Modifier { modifier: Modifier::ControlRight, pressed: false });
}

#[test]
fn off_by_default_existing_handlers_see_no_modifier_keys() {
    let mut rt = runtime(false);
    assert!(!rt.modifier_keys());
    modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
    assert!(!modifier(&mut rt, Code::ShiftLeft, false, SHIFT), "an unused release needs no frame");
    assert!(!flags(&mut rt, Mods::default()));
    rt.physical_input(PhysicalInput::FocusLost);
    rt.frame(0.0);
    assert!(rt.state.app.is_empty());
}

#[test]
fn a_key_up_the_platform_never_sent_comes_from_the_flags_or_focus_loss() {
    let mut rt = runtime(true);
    let both = Mods { shift: true, alt: true, ..Mods::default() };
    modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
    flags(&mut rt, SHIFT);
    modifier(&mut rt, Code::AltRight, true, SHIFT);
    flags(&mut rt, both);
    modifier(&mut rt, Code::Fn, true, both);
    assert!(flags(&mut rt, ALT), "Shift went off without its key-up");
    assert_eq!(modifiers(&rt.state.app)[3..], [(Modifier::ShiftLeft, false)]);
    rt.physical_input(PhysicalInput::FocusLost);
    rt.frame(0.0);
    assert_eq!(modifiers(&rt.state.app)[4..], [(Modifier::AltRight, false), (Modifier::Fn, false)]);
    assert!(rt.state.app[4..].iter().all(|ev| ev.mods == Mods::default()));
    assert!(!modifier(&mut rt, Code::AltRight, false, Mods::default()), "a late key-up is not sent twice");
    assert_eq!(rt.state.app.len(), 6);
}

#[test]
fn altgr_held_without_the_alt_flag_stays_down_until_its_key_up() {
    // X11 and Wayland report AltGr (Right Alt on many layouts) as Level 3, not `alt`.
    let mut rt = runtime(true);
    modifier(&mut rt, Code::AltRight, true, Mods::default());
    flags(&mut rt, Mods::default());
    flags(&mut rt, SHIFT);
    assert_eq!(modifiers(&rt.state.app), [(Modifier::AltRight, true)]);
    modifier(&mut rt, Code::AltRight, false, SHIFT);
    assert_eq!(modifiers(&rt.state.app), [(Modifier::AltRight, true), (Modifier::AltRight, false)]);
}

#[test]
fn a_recorder_takes_modifiers_first_and_on_key_never_hears_half_of_what_it_took() {
    let mut rt = runtime(true);
    click(&mut rt, "recorder");
    rt.state.recording = true;
    // Recording ⌥K: the recorder takes every key, and on_key hears none of it.
    modifier(&mut rt, Code::AltLeft, true, Mods::default());
    flags(&mut rt, ALT);
    rt.physical_with_fallback(PhysicalInput::Key { key: Code::KeyK, mods: ALT, pressed: true, repeat: false, text: None, consumed_mods: Default::default() },
        Some(KeyEvent { key: Key::Text("k".into()), mods: ALT }));
    rt.frame(0.0);
    rt.state.recording = false;
    assert!(!flags(&mut rt, Mods::default()), "Option's flag off is no release: its press was taken");
    assert!(!modifier(&mut rt, Code::AltLeft, false, ALT), "nor is its key-up");
    assert_eq!(rt.state.recorded.len(), 3);
    assert!(rt.state.app.is_empty());
    // A release the recorder takes ends the key silently, so the next press is heard.
    modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
    rt.state.recording = true;
    modifier(&mut rt, Code::ShiftLeft, false, SHIFT);
    modifier(&mut rt, Code::AltRight, true, Mods::default());
    rt.state.recording = false;
    modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
    assert_eq!(modifiers(&rt.state.app), [(Modifier::ShiftLeft, true), (Modifier::ShiftLeft, true)]);
    // A focus loss the recorder takes ends held keys silently, releases included.
    rt.state.recording = true;
    rt.physical_input(PhysicalInput::FocusLost);
    rt.frame(0.0);
    rt.state.recording = false;
    assert!(matches!(rt.state.recorded.last(), Some(PhysicalInput::FocusLost)));
    assert_eq!(rt.state.app.len(), 2);
    assert!(!modifier(&mut rt, Code::ShiftLeft, false, SHIFT), "Shift was already ended");
    modifier(&mut rt, Code::AltRight, true, Mods::default());
    assert_eq!(modifiers(&rt.state.app)[2..], [(Modifier::AltRight, true)], "a key the recorder took is heard next time");
}

#[test]
fn a_blur_or_a_hide_releases_held_keys_once() {
    let mut rt = runtime(true);
    click(&mut rt, "recorder");
    rt.window_focus(true);
    modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
    // How `--desktop` and headless scripts blur a window.
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(modifiers(&rt.state.app), [(Modifier::ShiftLeft, true), (Modifier::ShiftLeft, false)]);
    assert_eq!(rt.state.focus_lost, 1, "the recorder hears the blur too");
    // A hide, then the blur it may or may not bring.
    rt.window_focus(true);
    modifier(&mut rt, Code::AltRight, true, Mods::default());
    rt.keys_lost();
    assert!(rt.keys_queued(), "a hidden host steps a frame to deliver it");
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(modifiers(&rt.state.app)[2..], [(Modifier::AltRight, true), (Modifier::AltRight, false)]);
    assert_eq!(rt.state.focus_lost, 2);
    rt.window_focus(true);
    rt.keys_lost();
    rt.frame(0.0);
    assert_eq!(rt.state.focus_lost, 3, "a hide without a blur, after focus came back");
}

#[test]
fn a_focus_move_inside_the_window_keeps_modifiers_held() {
    let mut rt = runtime(true);
    modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
    click(&mut rt, "recorder");
    click(&mut rt, "field");
    modifier(&mut rt, Code::ShiftLeft, false, SHIFT);
    assert_eq!(modifiers(&rt.state.app), [(Modifier::ShiftLeft, true), (Modifier::ShiftLeft, false)]);
}

#[test]
fn windows_altgr_clearing_control_and_alt_is_no_release() {
    let key = |key, pressed, mods| PhysicalInput::Key { key, mods, pressed, repeat: false, text: None, consumed_mods: Default::default() };
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    let sent = |out: Vec<KeyEvent>| modifiers(&out);
    for altgr_clears in [true, false] {
        let mut keys = ModifierKeys { enabled: true, ..ModifierKeys::default() };
        keys.apply(&key(Code::ControlLeft, true, Mods::default()), false, altgr_clears);
        keys.apply(&PhysicalInput::Modifiers(ctrl), false, altgr_clears);
        keys.apply(&key(Code::AltRight, true, ctrl), false, altgr_clears);
        // Windows clears both flags while AltGr is down; X11 and Wayland leave Control on.
        let cleared = sent(keys.apply(&PhysicalInput::Modifiers(Mods::default()), false, altgr_clears));
        if !altgr_clears {
            assert_eq!(cleared, [(Modifier::ControlLeft, false)], "elsewhere a flag off is a key-up");
            continue;
        }
        assert!(cleared.is_empty(), "Left Control is still down");
        assert_eq!(sent(keys.apply(&key(Code::AltRight, false, Mods::default()), false, true)), [(Modifier::AltRight, false)]);
        assert!(sent(keys.apply(&PhysicalInput::Modifiers(ctrl), false, true)).is_empty());
        assert_eq!(sent(keys.apply(&key(Code::ControlLeft, false, ctrl), false, true)), [(Modifier::ControlLeft, false)]);
    }
}

#[test]
fn ctrl_shift_direction_chord_requires_windows_rtl_layout_and_disarms_on_other_keys_or_blur() {
    let ctrl = Mods { ctrl: true, cmd: !cfg!(target_os = "macos"), ..Mods::default() };
    let ctrl_shift = Mods { shift: true, ..ctrl };
    let ctrl_alt = Mods { alt: true, ..ctrl };
    let ctrl_alt_shift = Mods { shift: true, ..ctrl_alt };

    // Without an RTL keyboard layout installed, Ctrl+Shift never forces direction or redraws.
    let mut rt = runtime(false);
    rt.kbd.modifier_keys.rtl_layout_override = Some(false);
    click(&mut rt, "field");
    modifier(&mut rt, Code::ControlLeft, true, Mods::default());
    modifier(&mut rt, Code::ShiftRight, true, ctrl);
    assert!(!modifier(&mut rt, Code::ShiftRight, false, ctrl_shift), "no RTL layout: no redraw on release");
    modifier(&mut rt, Code::ControlLeft, false, ctrl);
    assert_eq!(rt.state.edit.forced_dir(), None);

    // With an RTL keyboard layout installed: fires on Windows only, and clear_dir restores natural.
    rt.kbd.modifier_keys.rtl_layout_override = Some(true);
    modifier(&mut rt, Code::ControlLeft, true, Mods::default());
    modifier(&mut rt, Code::ShiftRight, true, ctrl);
    // Repeat on held Shift does not disarm the chord.
    rt.physical_with_fallback(PhysicalInput::Key { key: Code::ShiftRight, mods: ctrl_shift, pressed: true, repeat: true, text: None, consumed_mods: Default::default() }, None);
    rt.frame(0.0);
    let redraw = modifier(&mut rt, Code::ShiftRight, false, ctrl_shift);
    modifier(&mut rt, Code::ControlLeft, false, ctrl);
    if cfg!(target_os = "windows") {
        assert!(redraw);
        assert_eq!(rt.state.edit.forced_dir(), Some(crate::el::Flow::Rtl));
        // Ctrl+Left Shift forces LTR on Ctrl release too.
        modifier(&mut rt, Code::ControlLeft, true, Mods::default());
        modifier(&mut rt, Code::ShiftLeft, true, ctrl);
        modifier(&mut rt, Code::ControlLeft, false, ctrl_shift);
        modifier(&mut rt, Code::ShiftLeft, false, SHIFT);
        assert_eq!(rt.state.edit.forced_dir(), Some(crate::el::Flow::Ltr));
        // App can clear forced direction back to natural.
        rt.state.edit.clear_dir();
        assert_eq!(rt.state.edit.forced_dir(), None);
    } else {
        assert!(!redraw);
        assert_eq!(rt.state.edit.forced_dir(), None, "non-Windows platforms have no Ctrl+Shift direction chord");
    }

    // Alt held before or during Ctrl+Shift disarms it.
    modifier(&mut rt, Code::AltLeft, true, Mods::default());
    modifier(&mut rt, Code::ControlLeft, true, ALT);
    modifier(&mut rt, Code::ShiftRight, true, ctrl_alt);
    modifier(&mut rt, Code::ShiftRight, false, ctrl_alt_shift);
    modifier(&mut rt, Code::ControlLeft, false, ctrl_alt);
    modifier(&mut rt, Code::AltLeft, false, ALT);
    assert_eq!(rt.state.edit.forced_dir(), None, "Alt held before Ctrl+Shift must disarm");

    modifier(&mut rt, Code::ControlLeft, true, Mods::default());
    modifier(&mut rt, Code::AltLeft, true, ctrl);
    modifier(&mut rt, Code::ShiftRight, true, ctrl_alt);
    modifier(&mut rt, Code::ShiftRight, false, ctrl_alt_shift);
    modifier(&mut rt, Code::AltLeft, false, ctrl_alt);
    modifier(&mut rt, Code::ControlLeft, false, ctrl);
    assert_eq!(rt.state.edit.forced_dir(), None, "Alt pressed during Ctrl+Shift must disarm");

    // Any non-modifier key between press and release disarms it.
    modifier(&mut rt, Code::ControlLeft, true, Mods::default());
    modifier(&mut rt, Code::ShiftRight, true, ctrl);
    rt.physical_with_fallback(
        PhysicalInput::Key { key: Code::KeyP, mods: ctrl_shift, pressed: true, repeat: false, text: None, consumed_mods: Default::default() },
        Some(KeyEvent { key: Key::Command('p'), mods: ctrl_shift }),
    );
    rt.frame(0.0);
    modifier(&mut rt, Code::ShiftRight, false, ctrl_shift);
    modifier(&mut rt, Code::ControlLeft, false, ctrl);
    assert_eq!(rt.state.edit.forced_dir(), None, "Ctrl+Shift+P must disarm");

    // FocusLost disarms a pending chord.
    modifier(&mut rt, Code::ControlLeft, true, Mods::default());
    modifier(&mut rt, Code::ShiftRight, true, ctrl);
    rt.physical_input(PhysicalInput::FocusLost);
    rt.frame(0.0);
    modifier(&mut rt, Code::ShiftRight, false, ctrl_shift);
    modifier(&mut rt, Code::ControlLeft, false, ctrl);
    assert_eq!(rt.state.edit.forced_dir(), None, "FocusLost must disarm a pending chord");

    // Moving focus to another element while armed disarms the chord.
    modifier(&mut rt, Code::ControlLeft, true, Mods::default());
    modifier(&mut rt, Code::ShiftRight, true, ctrl);
    click(&mut rt, "recorder");
    click(&mut rt, "field");
    modifier(&mut rt, Code::ShiftRight, false, ctrl_shift);
    modifier(&mut rt, Code::ControlLeft, false, ctrl);
    assert_eq!(rt.state.edit.forced_dir(), None, "focus move must disarm a pending chord");
}

#[test]
fn on_focus_lost_observes_without_ending_held_modifiers_silently() {
    #[derive(Default)]
    struct S { app: Vec<KeyEvent>, lost: usize, physical: usize }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(300.0, 200.0)
            .child(el().key("field").size(100.0, 40.0).focusable(true)
                .on_physical_input(|s: &mut S, ev, _| { if matches!(ev, PhysicalInput::FocusLost) { s.physical += 1; } false })
                .on_focus_lost(|s: &mut S, _| s.lost += 1))
            .child(crate::text(format!("lost {}", s.lost)).key("count"))
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.set_modifier_keys(true);
    rt.on_key = Some(|s, ev, _| { s.app.push(ev.clone()); true });
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("field", false));
    rt.frame(0.0);
    rt.physical_with_fallback(PhysicalInput::Key { key: Code::AltLeft, mods: Mods::default(), pressed: true, repeat: false, text: None, consumed_mods: Default::default() }, None);
    rt.frame(0.0);
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.lost, 1);
    assert_eq!(rt.state.physical, 1, "the physical handler still hears FocusLost");
    assert_eq!(modifiers(&rt.state.app), [(Modifier::AltLeft, true), (Modifier::AltLeft, false)]);
    assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("lost 1")), "the view builds again");
}

#[test]
fn on_focus_lost_order_with_physical_and_focus_change_and_no_unused_cost() {
    #[derive(Default)]
    struct S { log: Vec<&'static str>, take_physical: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let take = s.take_physical;
        col().size(300.0, 200.0)
            .child(
                el().key("a").size(100.0, 40.0).focusable(true)
                    .on_physical_input(move |s: &mut S, ev, _| {
                        if matches!(ev, PhysicalInput::FocusLost) {
                            s.log.push("physical");
                            return take;
                        }
                        false
                    })
                    .on_focus_lost(|s: &mut S, _| s.log.push("focus_lost"))
                    .on_focus_change(|s: &mut S, focused, _| {
                        s.log.push(if focused { "focus_in" } else { "focus_out" });
                    }),
            )
            .child(el().key("b").size(100.0, 40.0).focusable(true))
    }
    let mut rt = Runtime::new(S { log: Vec::new(), take_physical: true }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Unused FocusSpec::EMPTY keeps focus_lost None without allocating.
    assert!(<crate::el::FocusSpec as crate::cold::Empty>::EMPTY.focus_lost.is_none());

    rt.update(|_, tx| tx.focus("a", false));
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["focus_in"]);
    rt.state.log.clear();

    // Window deactivation: physical -> focus_lost (even when physical returned true); no focus_change.
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["physical", "focus_lost"]);
    rt.state.log.clear();

    // Window returns: no blur; focus is still on "a".
    rt.window_focus(true);
    rt.frame(0.0);
    assert!(rt.state.log.is_empty());

    // Moving focus inside the window: physical -> focus_lost -> focus_out.
    rt.update(|_, tx| tx.focus("b", false));
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["physical", "focus_lost", "focus_out"]);
}
