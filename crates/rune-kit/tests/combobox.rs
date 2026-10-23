use rune::{Cx, El, Env, Ime, Key, KeyEvent, Mods, Runtime};
use rune_kit::{Combobox, ComboboxOption, ComboboxState, Kit};

#[derive(Default)]
struct State { combo: ComboboxState, chosen: Option<String>, other: rune::TextEdit, app_keys: usize }
fn view(s: &State, _: &mut Cx<State>) -> El {
    let options: Vec<_> = (0..200).map(|i| {
        let mut o = ComboboxOption::new(format!("item-{i}"), format!("Model {i:03}"));
        o.disabled = i == 1;
        o
    }).collect();
    let mut layer = Vec::new();
    let body = Combobox::new("models", &s.combo, &options, |s: &mut State| &mut s.combo)
        .current(s.chosen.as_deref())
        .on_choose(|s, id, _| s.chosen = Some(id.into())).build(&Kit::neutral(true), &mut layer);
    let body = rune::col().child(rune_kit::TextInput::new(&s.other)
        .on_edit(|s: &mut State| &mut s.other).build(&Kit::neutral(true)).key("other")).child(body);
    rune_kit::root(body, layer)
}
fn runtime() -> Runtime<State> {
    let mut s = State::default();
    s.combo.open = true;
    s.combo.search.focused = true;
    let mut rt = Runtime::new(s, view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}
fn key(rt: &mut Runtime<State>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods }); rt.frame(0.0);
}

#[test]
fn opening_takes_focus_and_reveals_the_offscreen_selection() {
    let mut s = State::default();
    s.other.focused = true;
    s.chosen = Some("item-199".into());
    let mut rt = Runtime::new(s, view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.ime(Ime::Preedit("pending".into(), None)); rt.frame(0.0);
    let trigger = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("models-value-trigger")).unwrap();
    let x = trigger.rect[0] + trigger.rect[2] / 2.0;
    let y = trigger.rect[1] + trigger.rect[3] / 2.0;
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert!(rt.state.combo.open && rt.state.combo.search.focused);
    assert!(!rt.state.other.focused);
    assert!(rt.state.other.preedit.is_none());
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("models-option-item-199")).unwrap();
    assert!(last.visible[3] > 0.0);
    key(&mut rt, Key::Text("099".into()), Mods::default());
    assert_eq!(rt.state.combo.search.text, "099");
    assert_eq!(rt.state.other.text, "");
}

#[test]
fn closing_a_scrolled_virtual_popup_keeps_rows_in_place_while_fading() {
    let mut rt = runtime();
    key(&mut rt, Key::Up, Mods::default());
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let before = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("models-option-item-199")).unwrap();
    key(&mut rt, Key::Escape, Mods::default());
    rt.frame(1.0 / 60.0);
    let after = rt.elements().into_iter().find(|e| e.key == before.key).unwrap();
    assert!((after.rect[1] - before.rect[1]).abs() < 5.0, "row jumped: {:?} -> {:?}", before.rect, after.rect);
    assert!(after.opacity > 0.0 && after.opacity < before.opacity);
    assert!(after.visible[3] > 0.0);
}

#[test]
fn combobox_loops_skips_disabled_and_scrolls_virtual_rows_into_view() {
    let mut rt = runtime();
    assert!(rt.elements().iter().filter(|e| e.role == "option").count() < 50);
    key(&mut rt, Key::Down, Mods::default());
    assert_eq!(rt.state.combo.active.as_deref(), Some("item-2"));
    key(&mut rt, Key::Up, Mods::default());
    key(&mut rt, Key::Up, Mods::default());
    assert_eq!(rt.state.combo.active.as_deref(), Some("item-199"));
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("models-option-item-199")).unwrap();
    assert!(last.visible[3] > 0.0);
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.chosen.as_deref(), Some("item-199"));
    assert!(!rt.state.combo.open);
}

#[test]
fn combobox_filters_from_scroll_end_preserves_editor_modifiers_and_ime() {
    let mut rt = runtime();
    key(&mut rt, Key::Up, Mods::default());
    key(&mut rt, Key::Text("099".into()), Mods::default());
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("models-option-item-99")));
    assert_eq!(rt.state.combo.search.text, "099");
    key(&mut rt, Key::Down, Mods { alt: true, ..Mods::default() });
    assert!(rt.state.combo.active.is_none());
    rt.ime(Ime::Preedit("x".into(), None)); rt.frame(0.0);
    key(&mut rt, Key::Enter, Mods::default());
    assert!(rt.state.combo.open && rt.state.chosen.is_none());
    rt.ime(Ime::Preedit(String::new(), None)); rt.frame(0.0);
    key(&mut rt, Key::Escape, Mods::default());
    assert!(!rt.state.combo.open);
}

#[test]
fn focused_combobox_handles_keys_before_app_and_returns_focus_on_close() {
    let mut rt = runtime();
    rt.on_key = Some(|s, _, _| { s.app_keys += 1; true });
    key(&mut rt, Key::Down, Mods::default());
    assert_eq!(rt.state.combo.active.as_deref(), Some("item-2"));
    key(&mut rt, Key::Function(5), Mods::default());
    assert_eq!(rt.state.app_keys, 1, "unhandled keys bubble to the app");
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.chosen.as_deref(), Some("item-2"));
    let trigger = rt.elements().into_iter().find(|e| e.focused).unwrap();
    assert_eq!(trigger.key.as_deref(), Some("models-value-trigger"));
    assert!(trigger.focus_visible);
    key(&mut rt, Key::Enter, Mods::default());
    assert!(rt.state.combo.open && rt.state.combo.search.focused);
    key(&mut rt, Key::Escape, Mods::default());
    assert!(!rt.state.combo.open);
    assert_eq!(rt.state.app_keys, 1, "Enter/arrows/Escape belong to the control");
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).unwrap().key.as_deref(), Some("models-value-trigger"));
    key(&mut rt, Key::Enter, Mods::default());
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    rt.ime(Ime::Preedit("pending".into(), None)); rt.frame(0.0);
    rt.pointer_down(390.0, 390.0); rt.pointer_up(390.0, 390.0); rt.frame(0.0);
    assert!(!rt.state.combo.open && !rt.state.combo.search.focused);
    assert!(rt.state.combo.search.preedit.is_none());
    let trigger = rt.elements().into_iter().find(|e| e.focused).unwrap();
    assert_eq!(trigger.key.as_deref(), Some("models-value-trigger"));
    assert!(!trigger.focus_visible);
}
