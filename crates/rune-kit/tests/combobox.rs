#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use rune::{Cx, El, Env, Ime, Key, KeyEvent, Mods, Runtime};
use rune_kit::{Combobox, ComboboxOption, ComboboxState, Kit};

#[derive(Default)]
struct State { combo: ComboboxState, chosen: Option<String>, chooses: usize, other: rune::TextEdit, app_keys: usize, animate: bool }
fn view(s: &State, _: &mut Cx<State>) -> El {
    let options: Vec<_> = (0..200).map(|i| {
        let mut o = ComboboxOption::new(format!("item-{i}"), format!("Model {i:03}"));
        o.disabled = i == 1;
        o
    }).collect();
    let mut layer = Vec::new();
    let body = Combobox::new("models", &s.combo, &options, |s: &mut State| &mut s.combo)
        .current(s.chosen.as_deref())
        .animate_filtering(s.animate)
        .on_choose(|s, id, _| { s.chosen = Some(id.into()); s.chooses += 1; }).build(&Kit::neutral(true), &mut layer);
    let body = rune::col().child(rune_kit::TextInput::new(&s.other)
        .on_edit(|s: &mut State| &mut s.other).build(&Kit::neutral(true)).key("other")).child(body);
    rune_kit::root(body, layer)
}
fn runtime() -> Runtime<State> {
    open(State::default())
}
fn animated_runtime() -> Runtime<State> {
    open(State { animate: true, ..State::default() })
}
fn open(mut s: State) -> Runtime<State> {
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
    key(&mut rt, Key::Text(" ".into()), Mods::default());
    assert!(rt.state.combo.open && rt.state.combo.search.focused);
    key(&mut rt, Key::Escape, Mods::default());
    assert!(!rt.state.combo.open);
    assert_eq!(rt.state.app_keys, 1, "Enter/Space/arrows/Escape belong to the control");
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).unwrap().key.as_deref(), Some("models-value-trigger"));
    key(&mut rt, Key::Text(" ".into()), Mods::default());
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    rt.ime(Ime::Preedit("pending".into(), None)); rt.frame(0.0);
    rt.pointer_down(390.0, 390.0); rt.pointer_up(390.0, 390.0); rt.frame(0.0);
    assert!(!rt.state.combo.open && !rt.state.combo.search.focused);
    assert!(rt.state.combo.search.preedit.is_none());
    let trigger = rt.elements().into_iter().find(|e| e.focused).unwrap();
    assert_eq!(trigger.key.as_deref(), Some("models-value-trigger"));
    assert!(!trigger.focus_visible);
}

#[test]
fn held_enter_chooses_once_and_does_not_reopen_from_the_trigger() {
    let mut rt = runtime();
    rt.on_key = Some(|s, _, _| { s.app_keys += 1; false });
    key(&mut rt, Key::Down, Mods::default());
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() }); rt.frame(0.0);
    for _ in 0..2 { rt.repeat_key(KeyEvent { key: Key::Enter, mods: Mods::default() }); rt.frame(0.0); }
    assert_eq!((rt.state.chosen.as_deref(), rt.state.chooses), (Some("item-2"), 1));
    assert!(!rt.state.combo.open, "the repeat must not reopen the popup from its trigger");
    assert_eq!(rt.state.app_keys, 0);
}

/// A held key whose first press went to the IME: the repeats arrive after the composition ends,
/// while the popup is still open, and must not choose or close it.
#[test]
fn repeats_after_an_ime_composition_neither_choose_nor_close() {
    for held in [Key::Enter, Key::Escape] {
        let mut rt = runtime();
        key(&mut rt, Key::Down, Mods::default());
        rt.ime(Ime::Preedit("ka".into(), None)); rt.frame(0.0);
        key(&mut rt, held.clone(), Mods::default());
        assert!(rt.state.combo.open, "{held:?} during composition belongs to the IME");
        rt.ime(Ime::Commit("か".into())); rt.frame(0.0);
        for _ in 0..2 { rt.repeat_key(KeyEvent { key: held.clone(), mods: Mods::default() }); rt.frame(0.0); }
        assert!(rt.state.combo.open, "a repeated {held:?} keeps the popup open");
        assert_eq!(rt.state.chooses, 0);
    }
}

#[test]
fn a_keystroke_lands_its_rows_and_height_in_one_frame() {
    let mut rt = runtime();
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    key(&mut rt, Key::Up, Mods::default());
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let find = |rt: &Runtime<State>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    let open = find(&rt, "models-content").unwrap().rect;
    let top = |rt: &Runtime<State>, id: &str| {
        let vp = find(rt, "viewport").unwrap().rect;
        find(rt, &format!("models-option-{id}")).map(|e| (e.rect[1] - vp[1], e.opacity))
    };
    key(&mut rt, Key::Text("0".into()), Mods::default());
    rt.frame(1.0 / 120.0);
    assert!(rt.elements().iter().all(|e| !e.exiting), "nothing is left fading out");
    assert!(top(&rt, "item-199").is_none(), "the rows the filter drops are gone");
    assert_eq!(top(&rt, "item-0"), Some((0.0, 1.0)), "the first match is whole at the top");
    assert_eq!(find(&rt, "models-content").unwrap().rect, open, "a full list keeps its size");

    key(&mut rt, Key::Text("zz".into()), Mods::default());
    rt.frame(1.0 / 120.0);
    let empty = find(&rt, "models-content").unwrap().rect;
    assert!(empty[3] < open[3] - 100.0, "it takes the empty state's height: {open:?} → {empty:?}");
    assert!(top(&rt, "item-0").is_none());
    assert_eq!(find(&rt, "empty").unwrap().opacity, 1.0);
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    assert_eq!(find(&rt, "models-content").unwrap().rect, empty, "and holds it");

    key(&mut rt, Key::Backspace, Mods::default());
    key(&mut rt, Key::Backspace, Mods::default());
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "models-content").unwrap().rect, open, "deleting lands back at once");
    assert_eq!(top(&rt, "item-0"), Some((0.0, 1.0)));
}

#[test]
fn filtering_springs_the_popup_height_and_fades_rows() {
    let mut rt = animated_runtime();
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let find = |rt: &Runtime<State>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    let open = find(&rt, "models-content").unwrap().rect[3];
    key(&mut rt, Key::Text("199".into()), Mods::default());
    rt.frame(1.0 / 60.0);
    let gone = find(&rt, "models-option-item-0").expect("a filtered-out row fades out");
    assert!(gone.opacity > 0.0 && gone.opacity < 1.0, "row opacity {}", gone.opacity);
    key(&mut rt, Key::Text("zz".into()), Mods::default());
    let mut last = find(&rt, "models-content").unwrap().rect[3];
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        let h = find(&rt, "models-content").unwrap().rect[3];
        assert!((h - last).abs() < 20.0, "the popup's height jumped {last} → {h}");
        last = h;
    }
    assert!(last < open - 100.0, "it settles to the empty state: {open} → {last}");
    assert_eq!(find(&rt, "empty").unwrap().opacity, 1.0);
}

fn tall_view(s: &State, _: &mut Cx<State>) -> El {
    let options: Vec<_> = (0..200).map(|i| ComboboxOption::new(format!("item-{i}"), format!("Model {i:03}"))).collect();
    let mut layer = Vec::new();
    let trigger = rune::row().key("custom").size(120.0, 28.0).child(rune::text("Pick"));
    let body = Combobox::new("models", &s.combo, &options, |s: &mut State| &mut s.combo)
        .trigger(trigger).max_height(2000.0).build(&Kit::neutral(true), &mut layer);
    rune_kit::root(rune::col().pt(40.0).child(rune::row().child(body)), layer)
}

#[test]
fn a_custom_trigger_opens_a_list_that_fits_the_window_under_a_search_header() {
    let mut rt = Runtime::new(State::default(), tall_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let find = |rt: &Runtime<State>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
    let trigger = find(&rt, "custom").rect;
    rt.pointer_down(trigger[0] + 10.0, trigger[1] + 10.0);
    rt.pointer_up(trigger[0] + 10.0, trigger[1] + 10.0);
    rt.settle_all();
    rt.frame(0.0);
    assert!(rt.state.combo.open && rt.state.combo.search.focused);
    let content = find(&rt, "models-content").rect;
    assert!(content[1] + content[3] <= 300.0, "the popup runs off the window: {content:?}");
    let [_, header_top, _, header_height] = find(&rt, "header").rect;
    let viewport = find(&rt, "viewport").rect;
    assert!(viewport[1] >= header_top + header_height, "rows start under the search header");
    assert!(viewport[3] > 100.0, "the list uses the room below: {viewport:?}");
    key(&mut rt, Key::Up, Mods::default());
    assert_eq!(rt.state.combo.active.as_deref(), Some("item-199"));
    let last = find(&rt, "models-option-item-199");
    assert!(last.visible[3] > 0.0);
}

/// Typing while the list sits scrolled to its end: what the reader saw fades out where it stood,
/// the matches fade in from the top, and matches that land out of view skip their fade.
/// (`animate_filtering`)
#[test]
fn typing_crossfades_the_rows_in_view() {
    let mut rt = animated_runtime();
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    key(&mut rt, Key::Up, Mods::default());
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    // (top in the viewport, opacity, leaving)
    let row = |rt: &Runtime<State>, id: &str| {
        let vp = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("viewport")).unwrap().rect;
        let key = format!("models-option-{id}");
        let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str()))?;
        Some((e.rect[1] - vp[1], e.opacity, e.exiting))
    };
    key(&mut rt, Key::Text("0".into()), Mods::default());
    let (top, opacity, _) = row(&rt, "item-0").expect("the first match");
    assert!(top.abs() < 0.5 && opacity < 0.05, "it fades in at the top: {top} at {opacity}");
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let (top, opacity, _) = row(&rt, "item-7").expect("a match under the viewport");
    assert!(top > 190.0 && opacity == 1.0, "nobody sees it, so it skips its fade: {top} at {opacity}");
    let (stood, _, _) = row(&rt, "item-2").unwrap();
    key(&mut rt, Key::Text("9".into()), Mods::default());
    let (top, _, leaving) = row(&rt, "item-2").expect("a row the filter drops");
    assert!(leaving && (top - stood).abs() < 0.5, "it fades where it stood: {stood} → {top}");
    let (top, opacity, _) = row(&rt, "item-90").expect("the new first match");
    assert!(top.abs() < 0.5 && opacity < 0.05, "it fades in at the top over the leaving rows: {top} at {opacity}");
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    key(&mut rt, Key::Text("9".into()), Mods::default());
    let (top, opacity, _) = row(&rt, "item-99").expect("a match mounted out of view");
    assert!(top.abs() < 0.5 && opacity < 0.05, "it fades in rather than appearing whole: {top} at {opacity}");
}

