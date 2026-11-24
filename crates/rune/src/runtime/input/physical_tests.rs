use super::*;
use crate::{Cx, El, Env, Key, Mods, PhysicalKeyCode, TextEdit, TextStyle, col, el, text_input};

#[derive(Default)]
struct State { capture: bool, physical: usize, logical: usize, dismissed: usize, edit: TextEdit, gone: bool }
fn view(s: &State, _: &mut Cx<State>) -> El {
    let recorder = el().key("recorder").size(100.0, 40.0).focusable(true)
        .on_press(|s: &mut State| s.capture = true)
        .on_physical_input(|s: &mut State, _, _| {
            if s.capture { s.physical += 1; true } else { false }
        });
    col().key("modal").size(300.0, 200.0).modal(false, |s: &mut State| s.dismissed += 1)
        .children((!s.gone).then_some(recorder))
        .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("edit").size(200.0, 40.0)
            .on_edit(|s: &mut State| &mut s.edit))
}
#[test]
fn physical_dispatch_precedes_modal_app_and_paste_after_same_frame_click() {
    let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env { system_dark: true, scale: 1.0 });
    rt.on_key = Some(|s, _, _| { s.logical += 1; true });
    rt.frame(0.0);
    let (x, y) = rt.find("recorder").unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y);
    let mods = Mods { cmd: true, ..Mods::default() };
    rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::KeyV, mods, pressed: true, repeat: false },
        Some(KeyEvent { key: Key::Command('v'), mods }));
    rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Escape, mods: Mods::default(), pressed: true, repeat: false },
        Some(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    rt.frame(0.0);
    assert_eq!((rt.state.physical, rt.state.logical, rt.state.dismissed), (2, 0, 0));
    assert_eq!(rt.state.edit.text, "");
    assert!(rt.edit.paste_request.is_none(), "capture must not read the clipboard");
    rt.state.capture = false;
    let (x, y) = rt.find("edit").unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y);
    rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::KeyV, mods, pressed: true, repeat: false },
        Some(KeyEvent { key: Key::Command('v'), mods }));
    rt.frame(0.0);
    let target = rt.edit.paste_request.take().expect("paste requested");
    rt.paste_result(target, crate::ClipboardContent::new("paste"));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "paste");
    rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Escape, mods: Mods::default(), pressed: true, repeat: false },
        Some(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    rt.frame(0.0);
    assert_eq!(rt.state.dismissed, 1);
}

#[test]
fn modifier_and_release_redraws_require_consumption_and_keep_queued_focus_order() {
    let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let modifiers = PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() });
    let release = PhysicalInput::Key { key: PhysicalKeyCode::ShiftLeft, mods: Mods::default(), pressed: false, repeat: false };
    for event in [modifiers, release] {
        assert!(!rt.physical_with_fallback(event, None));
        assert!(rt.frame.queue.is_empty() && !rt.commit.dirty, "unused input must not schedule work");
    }
    click(&mut rt, "recorder");
    // The click already requested a frame. Do not deliver modifiers to the old
    // focus or request another frame before that queued click starts capture.
    assert!(!rt.physical_with_fallback(modifiers, None));
    assert_eq!(rt.state.physical, 0);
    rt.frame(0.0);
    assert_eq!(rt.state.physical, 1);
    for event in [modifiers, release] {
        assert!(rt.physical_with_fallback(event, None));
        assert!(rt.commit.dirty, "consumption must commit the handler's state changes");
        rt.frame(0.0);
    }
    assert_eq!(rt.state.physical, 3);
    rt.state.capture = false;
    assert!(!rt.physical_with_fallback(release, None), "a declining handler needs no frame");
    rt.state.capture = true;
    click(&mut rt, "edit");
    assert!(!rt.physical_with_fallback(modifiers, None));
    rt.frame(0.0);
    let delivered = rt.state.physical; // FocusLost may notify the old recorder.
    assert!(!rt.physical_with_fallback(release, None));
    assert_eq!(rt.state.physical, delivered);
    assert!(rt.frame.queue.is_empty() && !rt.commit.dirty);
}

/// A modifier change while state changed since the last frame, as a covered window's does
/// until it uncovers, goes to the focused handler of the view that state builds, not the
/// last frame's.
#[test]
fn a_modifier_change_after_an_update_waits_for_the_new_view() {
    let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, "recorder");
    rt.frame(0.0);
    assert!(rt.state.capture);
    let heard = rt.state.physical;
    rt.update(|s, _| s.gone = true);
    let modifiers = PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() });
    assert!(rt.physical_with_fallback(modifiers, None), "it asks for the frame that builds the new view");
    assert_eq!(rt.state.physical, heard, "the last frame's recorder doesn't hear it");
    rt.frame(0.0);
    assert!(rt.find("recorder").is_none());
    assert_eq!(rt.state.physical, heard, "and the new view has none");
}

#[derive(Default)]
struct Fields { first: TextEdit, second: TextEdit, secure: bool, keys: usize, app_keys: usize }
fn fields(s: &Fields, _: &mut Cx<Fields>) -> El {
    let first = text_input(&s.first, TextStyle::new(14.0, 400.0)).single_line()
        .key("first").size(100.0, 32.0).on_edit(|s: &mut Fields| &mut s.first)
        .on_input_key(|s: &mut Fields, _, _| { s.keys += 1; false });
    col().size(300.0, 200.0)
        .child(if s.secure { first.secure() } else { first })
        .child(text_input(&s.second, TextStyle::new(14.0, 400.0)).key("second")
            .size(100.0, 32.0).on_edit(|s: &mut Fields| &mut s.second))
}
fn click<S: 'static>(rt: &mut Runtime<S>, key: &str) {
    let (x, y) = rt.find(key).unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y);
}
fn key<S: 'static>(rt: &mut Runtime<S>, physical: PhysicalKeyCode, key: Key) {
    let mods = Mods { cmd: matches!(key, Key::Command(_)), ..Mods::default() };
    rt.physical_with_fallback(PhysicalInput::Key { key: physical, mods, pressed: true, repeat: false },
        Some(KeyEvent { key, mods }));
}
fn fields_runtime(secure: bool) -> Runtime<Fields> {
    let mut rt = Runtime::new(Fields { secure, ..Fields::default() }, fields, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, _, _| { s.app_keys += 1; true });
    rt.frame(0.0); rt
}

#[test]
fn paste_is_swallowed_without_text_and_target_survives_a_later_queued_click() {
    let mut rt = fields_runtime(false);
    key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
    rt.frame(0.0);
    assert!(rt.edit.paste_request.is_none(), "no focused editor or paste handler: no clipboard read");
    assert_eq!(rt.state.app_keys, 1);
    click(&mut rt, "first");
    key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
    rt.frame(0.0);
    let first = rt.edit.paste_request.take().unwrap();
    assert!(first.is_some());
    assert_eq!((rt.state.keys, rt.state.app_keys), (1, 1));
    // Image-only clipboard sends no reply. An empty text reply also never falls
    // through to either key handler.
    rt.paste_result(first.clone(), crate::ClipboardContent::new(""));
    rt.frame(0.0);
    assert_eq!((rt.state.keys, rt.state.app_keys), (1, 1));
    key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
    click(&mut rt, "second");
    rt.frame(0.0);
    assert_eq!((rt.state.keys, rt.state.app_keys), (2, 1));
    let target = rt.edit.paste_request.take().unwrap();
    assert_eq!(target, first, "target captured before the next click, not at worker flush");
    assert_ne!(target, rt.paste_target());
    rt.paste_result(target, crate::ClipboardContent::new("late"));
    rt.frame(0.0);
    assert!(rt.state.first.text.is_empty() && rt.state.second.text.is_empty());
}

#[test]
fn physical_fallback_edits_single_line_and_preserves_secure_copy_protection() {
    for secure in [false, true] {
        let mut rt = fields_runtime(secure);
        click(&mut rt, "first");
        key(&mut rt, PhysicalKeyCode::KeyX, Key::Text("x".into()));
        rt.frame(0.0);
        assert_eq!(rt.state.first.text, "x");
        assert_eq!(rt.has_secure_focus(), secure);
        key(&mut rt, PhysicalKeyCode::KeyA, Key::Command('a'));
        key(&mut rt, PhysicalKeyCode::KeyC, Key::Command('c'));
        rt.frame(0.0);
        assert_eq!(rt.copy_request.is_none(), secure);
        key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
        rt.frame(0.0);
        let target = rt.edit.paste_request.take().unwrap();
        rt.paste_result(target, crate::ClipboardContent::new("replacement"));
        rt.frame(0.0);
        assert_eq!(rt.state.first.text, "replacement");
        if secure {
            let elements = rt.elements();
            let field = elements.iter().find(|e| e.key.as_deref() == Some("first")).unwrap();
            assert!(field.text.is_none());
        }
    }
}

#[derive(Default)]
struct Held { edit: TextEdit, other: TextEdit, input: Vec<bool>, app: Vec<(Key, bool)> }
fn held(s: &Held, _: &mut Cx<Held>) -> El {
    col().size(300.0, 200.0)
        .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).single_line().key("field").size(200.0, 32.0)
            .on_edit(|s: &mut Held| &mut s.edit)
            .on_input_key(|s: &mut Held, _, tx| { s.input.push(tx.key_repeat()); false }))
        .child(text_input(&s.other, TextStyle::new(14.0, 400.0)).single_line().key("other").size(200.0, 32.0)
            .on_edit(|s: &mut Held| &mut s.other))
}
fn focused(rt: &Runtime<Held>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}
fn press<S: 'static>(rt: &mut Runtime<S>, physical: PhysicalKeyCode, key: Key, repeat: bool) {
    let mods = Mods::default();
    rt.physical_with_fallback(PhysicalInput::Key { key: physical, mods, pressed: true, repeat }, Some(KeyEvent { key, mods }));
    rt.frame(0.0);
}

#[test]
fn key_handlers_see_a_held_keys_repeats_and_editing_keeps_repeating() {
    let mut rt = Runtime::new(Held::default(), held, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, tx| { s.app.push((ev.key.clone(), tx.key_repeat())); true });
    rt.frame(0.0);
    // No focus: the app handler gets first refusal, so it sees both presses.
    press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, false);
    press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, true);
    assert_eq!(rt.state.app, [(Key::Enter, false), (Key::Enter, true)]);
    click(&mut rt, "field");
    rt.frame(0.0);
    press(&mut rt, PhysicalKeyCode::KeyX, Key::Text("x".into()), false);
    press(&mut rt, PhysicalKeyCode::KeyX, Key::Text("x".into()), true);
    press(&mut rt, PhysicalKeyCode::KeyX, Key::Text("x".into()), true);
    assert_eq!(rt.state.input, [false, true, true]);
    assert_eq!(rt.state.edit.text, "xxx", "a held letter keeps typing");
    // Held Tab keeps moving focus, and Rune consumes it before the app handler.
    press(&mut rt, PhysicalKeyCode::Tab, Key::Tab, true);
    assert_eq!(focused(&rt).as_deref(), Some("other"));
    press(&mut rt, PhysicalKeyCode::Tab, Key::Tab, true);
    assert_eq!(focused(&rt).as_deref(), Some("field"));
    assert_eq!(rt.state.app.len(), 2);
}

#[test]
fn a_queued_repeat_reaches_the_app_handler_with_the_flag_set() {
    let mut rt = Runtime::new(Held::default(), held, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, tx| { s.app.push((ev.key.clone(), tx.key_repeat())); true });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text("s".into()), mods: Mods::default() });
    rt.repeat_key(KeyEvent { key: Key::Text("s".into()), mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Text("s".into()), mods: Mods::default() });
    rt.frame(0.0);
    let s = Key::Text("s".into());
    assert_eq!(rt.state.app, [(s.clone(), false), (s.clone(), true), (s, false)]);
}

#[test]
fn key_handlers_see_the_physical_key_whatever_the_layout_types() {
    #[derive(Default)]
    struct Codes {
        edit: TextEdit,
        input: Vec<Option<PhysicalKeyCode>>,
        app: Vec<(Key, Option<PhysicalKeyCode>, bool)>,
        ups: Vec<Option<PhysicalKeyCode>>,
        change: Vec<Option<PhysicalKeyCode>>,
    }
    let view = |s: &Codes, _: &mut Cx<Codes>| col().size(300.0, 200.0)
        .on_key_up(|s: &mut Codes, _, tx| { s.ups.push(tx.physical_key()); true })
        .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 32.0)
            .on_edit(|s: &mut Codes| &mut s.edit)
            .on_input_change(|s: &mut Codes, tx| s.change.push(tx.physical_key()))
            .on_input_key(|s: &mut Codes, _, tx| { s.input.push(tx.physical_key()); false }));
    let mut rt = Runtime::new(Codes::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, tx| { s.app.push((ev.key.clone(), tx.physical_key(), tx.key_repeat())); true });
    rt.frame(0.0);
    click(&mut rt, "field");
    rt.frame(0.0);
    // ⌘⇧7 on a German layout types '/', on a US one '&'; both are the 7 key (press, repeat, key-up).
    let mods = Mods { cmd: true, shift: true, ..Mods::default() };
    for (c, repeat) in [('/', false), ('&', true)] {
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Digit7, mods, pressed: true, repeat },
            Some(KeyEvent { key: Key::Command(c), mods }));
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Digit7, mods, pressed: false, repeat: false },
            Some(KeyEvent { key: Key::Command(c), mods }));
        rt.frame(0.0);
    }
    let seven = Some(PhysicalKeyCode::Digit7);
    assert_eq!(rt.state.input, [seven, seven]);
    assert_eq!(rt.state.app, [(Key::Command('/'), seven, false), (Key::Command('&'), seven, true)]);
    assert_eq!(rt.state.ups, [seven, seven]);
    // A logical key with no physical origin has none, and non-key callbacks (`on_input_change`)
    // never inherit the key handler's physical key.
    rt.key(KeyEvent { key: Key::Command('&'), mods });
    rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::KeyA, mods: Mods::default(), pressed: true, repeat: false },
        Some(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() }));
    rt.frame(0.0);
    assert_eq!(rt.state.app.last(), Some(&(Key::Command('&'), None, false)));
    assert_eq!(rt.state.change, [None]);
}

#[test]
fn a_format_chord_the_field_does_not_allow_goes_to_the_app() {
    #[derive(Default)]
    struct Rich { edit: TextEdit, app: Vec<Key> }
    let view = |s: &Rich, _: &mut Cx<Rich>| col().size(300.0, 200.0)
        .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 32.0).on_edit(|s: &mut Rich| &mut s.edit));
    let edit = TextEdit::new("ab").rich().allow_marks([crate::Mark::Bold]);
    let mut rt = Runtime::new(Rich { edit, app: Vec::new() }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| { s.app.push(ev.key.clone()); true });
    rt.frame(0.0);
    click(&mut rt, "field");
    rt.frame(0.0);
    let cmd = |c| KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } };
    for c in ['a', 'u', 'b'] { rt.key(cmd(c)); }
    rt.frame(0.0);
    assert_eq!(rt.state.app, [Key::Command('u')]);
    assert!(rt.state.edit.marks().iter().all(|m| m.mark == crate::Mark::Bold) && !rt.state.edit.marks().is_empty());
}

fn release<S: 'static>(rt: &mut Runtime<S>, physical: PhysicalKeyCode) {
    rt.physical_with_fallback(PhysicalInput::Key { key: physical, mods: Mods::default(), pressed: false, repeat: false }, None);
    rt.frame(0.0);
}

#[test]
fn a_held_arrow_keeps_repeating_after_an_escape_tap_moves_focus() {
    let mut rt = fields_runtime(false);
    click(&mut rt, "first");
    press(&mut rt, PhysicalKeyCode::ArrowDown, Key::Down, false);
    press(&mut rt, PhysicalKeyCode::Escape, Key::Escape, false);
    release(&mut rt, PhysicalKeyCode::Escape);
    assert!(!rt.has_focus(), "Escape blurred the field");
    let before = rt.state.app_keys;
    press(&mut rt, PhysicalKeyCode::ArrowDown, Key::Down, true);
    press(&mut rt, PhysicalKeyCode::ArrowDown, Key::Down, true);
    assert_eq!(rt.state.app_keys, before + 2, "the held arrow's repeats still arrive");
    // The same without a release, from a script: only Escape's own repeats are dropped.
    click(&mut rt, "first");
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }); rt.frame(0.0);
    rt.repeat_key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.repeat_key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.app_keys, before + 3);
}

#[test]
fn a_released_key_and_a_pointer_focus_change_leave_later_repeats_alone() {
    let mut rt = fields_runtime(false);
    click(&mut rt, "second");
    press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, false);
    release(&mut rt, PhysicalKeyCode::Enter);
    click(&mut rt, "first");
    rt.frame(0.0);
    rt.repeat_key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.repeat_key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.keys, 2, "scripted repeats reach the newly focused field");
}

#[derive(Default)]
struct Opener { modal: bool, go_repeats: usize, sink: usize }
fn opener(s: &Opener, _: &mut Cx<Opener>) -> El {
    let mut root = col().size(300.0, 200.0)
        .child(el().key("go").size(40.0, 30.0).focusable(true).role("button")
            .on_press(|s: &mut Opener| s.modal = true)
            .on_physical_input(|s: &mut Opener, e, _| {
                if matches!(e, PhysicalInput::Key { repeat: true, .. }) { s.go_repeats += 1; }
                false
            }))
        .child(el().key("grab").size(40.0, 30.0).focusable(true)
            .on_physical_input(|s: &mut Opener, e, _| match e {
                PhysicalInput::Key { key: PhysicalKeyCode::Enter, pressed: true, repeat: false, .. } => { s.modal = true; true }
                _ => false,
            }));
    if s.modal {
        root = root.child(col().key("modal").absolute(100.0, 0.0).size(100.0, 100.0)
            .modal(true, |s: &mut Opener| s.modal = false)
            .child(el().key("sink").size(40.0, 30.0).focusable(true)
                .on_physical_input(|s: &mut Opener, e, _| {
                    if matches!(e, PhysicalInput::Key { pressed: true, .. }) { s.sink += 1; }
                    true
                })));
    }
    root
}
fn opener_focused_on(tabs: usize) -> Runtime<Opener> {
    let mut rt = Runtime::new(Opener::default(), opener, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..tabs { rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() }); rt.frame(0.0); }
    rt
}
fn focused_key<S: 'static>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

#[test]
fn repeats_never_reach_physical_handlers_after_their_key_moved_focus() {
    // A built-in press opens the modal: the new focus's physical handler sees no repeat.
    let mut rt = opener_focused_on(1);
    assert_eq!(focused_key(&rt).as_deref(), Some("go"));
    press(&mut rt, PhysicalKeyCode::Space, Key::Text(" ".into()), false);
    assert_eq!(focused_key(&rt).as_deref(), Some("sink"));
    press(&mut rt, PhysicalKeyCode::Space, Key::Text(" ".into()), true);
    press(&mut rt, PhysicalKeyCode::Space, Key::Text(" ".into()), true);
    assert_eq!(rt.state.sink, 0);
    // A physical handler consumes the first press and opens it: still recorded and dropped.
    let mut rt = opener_focused_on(2);
    assert_eq!(focused_key(&rt).as_deref(), Some("grab"));
    press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, false);
    assert_eq!(focused_key(&rt).as_deref(), Some("sink"));
    press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, true);
    assert_eq!(rt.state.sink, 0);
}

#[test]
fn a_repeat_queued_in_the_same_frame_sees_the_focus_its_press_moved() {
    let mut rt = opener_focused_on(1);
    let mods = Mods::default();
    rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Space, mods, pressed: true, repeat: false },
        Some(KeyEvent { key: Key::Text(" ".into()), mods }));
    rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Space, mods, pressed: true, repeat: true },
        Some(KeyEvent { key: Key::Text(" ".into()), mods }));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("sink"));
    assert_eq!((rt.state.go_repeats, rt.state.sink), (0, 0), "the repeat went nowhere");
}

/// A `keys_first` menu item's key: the focused field, then `on_key`, see it before the item,
/// which runs only when neither takes it (and on each repeat, as a menu equivalent does).
#[test]
fn a_menu_item_handing_over_its_key_runs_only_when_no_handler_takes_it() {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    let mut rt = fields_runtime(false);
    rt.on_key = Some(|s, ev, _| { s.app_keys += 1; ev.is_command(']') });
    let ran = Arc::new(AtomicUsize::new(0));
    let item = { let ran = ran.clone(); Unhandled::Menu(Arc::new(move || { ran.fetch_add(1, Ordering::Relaxed); })) };
    let mods = Mods { cmd: true, ..Mods::default() };
    let chord = |rt: &mut Runtime<Fields>, code, c, repeat| {
        rt.physical_then(PhysicalInput::Key { key: code, mods, pressed: true, repeat },
            Some(KeyEvent { key: Key::Command(c), mods }), None, Some(item.clone()));
        rt.frame(0.0);
        ran.load(Ordering::Relaxed)
    };
    assert_eq!(chord(&mut rt, PhysicalKeyCode::BracketRight, ']', false), 0, "the app's on_key took it");
    assert_eq!(chord(&mut rt, PhysicalKeyCode::BracketLeft, '[', false), 1, "nobody took it: the item runs");
    assert_eq!(chord(&mut rt, PhysicalKeyCode::BracketLeft, '[', true), 2, "and repeats");
    assert_eq!(rt.state.app_keys, 3);
    // A field that takes the key (⌘A selects all) keeps it from the item and from on_key.
    click(&mut rt, "first");
    rt.frame(0.0);
    assert_eq!(chord(&mut rt, PhysicalKeyCode::KeyA, 'a', false), 2);
    assert_eq!(rt.state.app_keys, 3);
}

#[test]
fn physical_paste_uses_app_handler_without_a_focused_editor() {
    fn view(_: &String, _: &mut Cx<String>) -> El {
        el().size(100.0, 100.0).on_paste(|s: &mut String, text, _| *s = text.into())
    }
    let mut rt = Runtime::new(String::new(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
    rt.frame(0.0);
    assert_eq!(rt.edit.paste_request, Some(None));
    let target = rt.edit.paste_request.take().unwrap();
    rt.paste_result(target, crate::ClipboardContent::new("app"));
    rt.frame(0.0);
    assert_eq!(rt.state, "app");
}

#[test]
fn focused_input_on_key_intercepts_paste_and_edit_chords_before_editor() {
    #[derive(Default)]
    struct S {
        edit: TextEdit,
        consume: bool,
        seen: Vec<(Key, Option<PhysicalKeyCode>)>,
        app_keys: usize,
        root_pasted: Vec<String>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(300.0, 200.0)
            .on_paste(|s: &mut S, text, _| s.root_pasted.push(text.to_owned()))
            .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("field").label("Field").size(200.0, 40.0)
                .on_edit(|s: &mut S| &mut s.edit)
                .on_input_key(|s: &mut S, ev, tx| {
                    s.seen.push((ev.key.clone(), tx.physical_key()));
                    if s.consume {
                        if ev.key == Key::Command('v') { s.edit.insert("[handled]"); }
                        true
                    } else {
                        false
                    }
                }))
    }
    let primary = Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), logo: cfg!(target_os = "macos"), ..Mods::default() };
    let primary_shift = Mods { shift: true, ..primary };
    let send = |rt: &mut Runtime<S>, physical: PhysicalKeyCode, c: char, mods: Mods| {
        rt.physical_with_fallback(
            PhysicalInput::Key { key: physical, mods, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Command(c), mods }),
        );
        rt.frame(0.0);
    };

    let mut rt = Runtime::new(S { consume: true, ..S::default() }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, _, _| { s.app_keys += 1; s.consume });
    rt.frame(0.0);

    // Without focus, app.on_key sees Command('v') before root on_paste.
    send(&mut rt, PhysicalKeyCode::KeyV, 'v', primary);
    assert!(rt.edit.paste_request.is_none(), "app.on_key consuming Command('v') must not request paste");
    assert_eq!(rt.state.app_keys, 1);
    rt.state.consume = false;
    send(&mut rt, PhysicalKeyCode::KeyV, 'v', primary);
    assert_eq!(rt.edit.paste_request.take(), Some(None), "declined Command('v') falls through to root on_paste");
    assert_eq!(rt.state.app_keys, 2);

    // Focus the field, type text, then verify on_input_key intercepts paste and every edit chord.
    click(&mut rt, "field");
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text("ab".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "ab");
    rt.state.consume = true;
    rt.state.seen.clear();
    let app_before = rt.state.app_keys;

    send(&mut rt, PhysicalKeyCode::KeyV, 'v', primary);
    assert!(rt.edit.paste_request.is_none(), "consumed Command('v') must not request clipboard paste");
    assert_eq!(rt.state.edit.text, "ab[handled]");
    assert_eq!(rt.state.seen.last(), Some(&(Key::Command('v'), Some(PhysicalKeyCode::KeyV))));

    // Select all, copy, cut, undo, redo are all intercepted when on_input_key returns true.
    let before_text = rt.state.edit.text.clone();
    let before_sel = (rt.state.edit.anchor, rt.state.edit.focus);
    send(&mut rt, PhysicalKeyCode::KeyA, 'a', primary);
    assert_eq!((rt.state.edit.anchor, rt.state.edit.focus), before_sel);
    send(&mut rt, PhysicalKeyCode::KeyC, 'c', primary);
    assert!(rt.copy_request.is_none());
    send(&mut rt, PhysicalKeyCode::KeyX, 'x', primary);
    assert!(rt.copy_request.is_none());
    assert_eq!(rt.state.edit.text, before_text);
    send(&mut rt, PhysicalKeyCode::KeyZ, 'z', primary);
    assert_eq!(rt.state.edit.text, before_text);
    send(&mut rt, PhysicalKeyCode::KeyZ, 'z', primary_shift);
    assert_eq!(rt.state.edit.text, before_text);
    if !cfg!(target_os = "macos") {
        send(&mut rt, PhysicalKeyCode::KeyY, 'y', primary);
        assert_eq!(rt.state.edit.text, before_text);
    }
    assert_eq!(rt.state.app_keys, app_before, "consumed field chords must not reach app.on_key");

    // When on_input_key returns false, every edit chord proceeds to the editor and never falls through to app.on_key.
    rt.state.consume = false;
    rt.state.seen.clear();
    send(&mut rt, PhysicalKeyCode::KeyV, 'v', primary);
    assert_eq!(rt.state.seen.last(), Some(&(Key::Command('v'), Some(PhysicalKeyCode::KeyV))));
    let target = rt.edit.paste_request.take().expect("declined Command('v') requests paste");
    assert!(target.is_some());
    rt.paste_result(target, crate::ClipboardContent::new("clipboard"));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "ab[handled]clipboard");

    send(&mut rt, PhysicalKeyCode::KeyA, 'a', primary);
    assert_eq!(rt.state.edit.selected(), "ab[handled]clipboard");
    send(&mut rt, PhysicalKeyCode::KeyC, 'c', primary);
    assert_eq!(rt.copy_request.take().unwrap().content().text(), "ab[handled]clipboard");
    send(&mut rt, PhysicalKeyCode::KeyX, 'x', primary);
    assert_eq!(rt.copy_request.take().unwrap().content().text(), "ab[handled]clipboard");
    assert_eq!(rt.state.edit.text, "");
    send(&mut rt, PhysicalKeyCode::KeyZ, 'z', primary);
    assert_eq!(rt.state.edit.text, "ab[handled]clipboard");
    send(&mut rt, PhysicalKeyCode::KeyZ, 'z', primary_shift);
    assert_eq!(rt.state.edit.text, "");
    if !cfg!(target_os = "macos") {
        send(&mut rt, PhysicalKeyCode::KeyZ, 'z', primary);
        assert_eq!(rt.state.edit.text, "ab[handled]clipboard");
        send(&mut rt, PhysicalKeyCode::KeyY, 'y', primary);
        assert_eq!(rt.state.edit.text, "");
    }
    assert_eq!(rt.state.app_keys, app_before, "editor-handled chords must not reach app.on_key");
}

#[test]
fn runtime_key_with_delivers_physical_key_base_and_matches_keybind() {
    #[derive(Default)]
    struct Seen {
        keys: Vec<(Key, Option<PhysicalKeyCode>, Option<char>, bool, bool)>,
    }
    fn view(_: &Seen, _: &mut Cx<Seen>) -> El { col().size(300.0, 200.0) }
    let mut rt = Runtime::new(Seen::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, tx| {
        let matched = crate::Keybind::new("j").matches_tx(ev, tx);
        s.keys.push((ev.key.clone(), tx.physical_key(), tx.key_base(), tx.key_repeat(), matched));
        true
    });
    rt.frame(0.0);

    // Non-ASCII layout ("о" on the J key) with explicit base character.
    rt.key_with(
        KeyEvent { key: Key::Text("о".into()), mods: Mods::default() },
        PhysicalKeyCode::KeyJ,
        Some('j'),
    );
    // Same press without a base character still matches via physical_key fallback.
    rt.key_with(
        KeyEvent { key: Key::Text("о".into()), mods: Mods::default() },
        PhysicalKeyCode::KeyJ,
        None,
    );
    // Plain Runtime::key stays unchanged: neither physical_key nor key_base is set.
    rt.key(KeyEvent { key: Key::Text("о".into()), mods: Mods::default() });
    rt.frame(0.0);

    assert_eq!(rt.state.keys, [
        (Key::Text("о".into()), Some(PhysicalKeyCode::KeyJ), Some('j'), false, true),
        (Key::Text("о".into()), Some(PhysicalKeyCode::KeyJ), None, false, true),
        (Key::Text("о".into()), None, None, false, false),
    ]);
}

#[test]
fn runtime_physical_key_input_delivers_repeats_releases_and_physical_handlers() {
    #[derive(Default)]
    struct Seen {
        consume: bool,
        physical: Vec<PhysicalInput>,
        downs: Vec<(Key, Option<PhysicalKeyCode>, Option<char>, bool)>,
        ups: Vec<(Key, Option<PhysicalKeyCode>, Option<char>)>,
    }
    fn view(_: &Seen, _: &mut Cx<Seen>) -> El {
        col().size(300.0, 200.0)
            .on_key_up(|s: &mut Seen, ev, tx| {
                s.ups.push((ev.key.clone(), tx.physical_key(), tx.key_base()));
                true
            })
            .child(el().key("cap").size(100.0, 40.0).focusable(true)
                .on_physical_input(|s: &mut Seen, ev, _| {
                    s.physical.push(*ev);
                    s.consume
                }))
    }
    let mut rt = Runtime::new(Seen::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, tx| {
        s.downs.push((ev.key.clone(), tx.physical_key(), tx.key_base(), tx.key_repeat()));
        true
    });
    rt.frame(0.0);

    let mods = Mods { alt: true, ..Mods::default() };
    rt.physical_key_input(
        PhysicalInput::Key { key: PhysicalKeyCode::KeyA, mods, pressed: true, repeat: false },
        KeyEvent { key: Key::Text("å".into()), mods },
        Some('a'),
    );
    rt.physical_key_input(
        PhysicalInput::Key { key: PhysicalKeyCode::KeyA, mods, pressed: true, repeat: true },
        KeyEvent { key: Key::Text("å".into()), mods },
        Some('a'),
    );
    rt.physical_key_input(
        PhysicalInput::Key { key: PhysicalKeyCode::KeyA, mods, pressed: false, repeat: false },
        KeyEvent { key: Key::Text("å".into()), mods },
        Some('a'),
    );
    rt.frame(0.0);

    assert_eq!(rt.state.downs, [
        (Key::Text("å".into()), Some(PhysicalKeyCode::KeyA), Some('a'), false),
        (Key::Text("å".into()), Some(PhysicalKeyCode::KeyA), Some('a'), true),
    ]);
    assert_eq!(rt.state.ups, [
        (Key::Text("å".into()), Some(PhysicalKeyCode::KeyA), Some('a')),
    ]);

    // Focused physical handler sees the event first and blocks logical fallback when consumed.
    click(&mut rt, "cap");
    rt.frame(0.0);
    rt.state.consume = true;
    let press = PhysicalInput::Key { key: PhysicalKeyCode::KeyJ, mods: Mods::default(), pressed: true, repeat: false };
    rt.physical_key_input(press, KeyEvent { key: Key::Text("j".into()), mods: Mods::default() }, Some('j'));
    rt.frame(0.0);
    assert!(matches!(rt.state.physical.last(), Some(PhysicalInput::Key { key: PhysicalKeyCode::KeyJ, pressed: true, repeat: false, .. })));
    assert_eq!(rt.state.downs.len(), 2, "consumed physical event must not fall through to on_key");
}
