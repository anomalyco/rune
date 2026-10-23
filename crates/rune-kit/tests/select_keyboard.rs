use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Kit, Select, SelectAppearance, root};

struct State { open: bool, value: String }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let select = Select::new("fruit", &["Apple", "Banana", "Cherry"]).current(Some(&s.value))
        .appearance(SelectAppearance::Field).width(120.0)
        .open(s.open).on_toggle(|s: &mut State| s.open = !s.open)
        .on_close(|s: &mut State| s.open = false)
        .on_choose(|s: &mut State, v, _| { s.value = v.into(); s.open = false; })
        .build(&k, &mut layer);
    root(col().w_full().h_full().child(select), layer)
}

fn press(rt: &mut Runtime<State>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn focused(rt: &Runtime<State>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

#[test]
fn a_select_opens_on_its_value_and_chooses_or_dismisses_from_the_keyboard() {
    let mut rt = Runtime::new(State { open: false, value: "Banana".into() }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("fruit-trigger"));
    press(&mut rt, Key::Down);
    assert!(rt.state.open, "Down opens a popup button");
    assert_eq!(focused(&rt).as_deref(), Some("option1"), "the listbox opens on the current value");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("option2"), "Tab stays inside the open listbox");
    press(&mut rt, Key::Escape);
    assert!(!rt.state.open);
    assert_eq!(rt.state.value, "Banana");
    assert_eq!(focused(&rt).as_deref(), Some("fruit-trigger"), "focus returns to the trigger");

    press(&mut rt, Key::Text(" ".into()));
    assert!(rt.state.open);
    press(&mut rt, Key::Text("c".into()));
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.value, "Cherry");
    assert!(!rt.state.open);
    assert_eq!(focused(&rt).as_deref(), Some("fruit-trigger"));

    press(&mut rt, Key::Up);
    let (x, y) = (390.0, 290.0);
    rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(1.0 / 60.0);
    assert!(!rt.state.open, "a press outside closes it");
    assert_eq!(rt.state.value, "Cherry");
}
