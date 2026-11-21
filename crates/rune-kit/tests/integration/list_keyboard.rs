use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Kit, List, ListItem};

#[derive(Default)]
struct State {
    picked: Option<usize>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let mut list = List::new().active(None);
    for (i, name) in ["One", "Two", "Three"].into_iter().enumerate() {
        list = list.item(
            ListItem::new(name).selected(s.picked == Some(i)).on_select(move |s: &mut State, _| s.picked = Some(i)),
        );
    }
    col()
        .w_full()
        .child(rune::row().key("before").role("button").label("Before").size(40.0, 20.0).focusable(true).on_press(|_: &mut State| {}))
        .child(list.build(&k))
}

fn plain(_: &State, _: &mut Cx<State>) -> El {
    List::new().item(ListItem::new("Only")).build(&Kit::neutral(true))
}

fn press(rt: &mut Runtime<State>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn focused(rt: &Runtime<State>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

#[test]
fn rows_with_an_action_are_one_tab_stop_with_arrow_navigation() {
    let mut rt = Runtime::new(State::default(), view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("before"));
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("item0"), "Tab enters the list once");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt).as_deref(), Some("item2"));
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.picked, Some(2));
    press(&mut rt, Key::Tab);
    let in_list = |rt: &Runtime<State>| focused(rt).is_some_and(|k| k.starts_with("item"));
    assert!(!in_list(&rt), "Tab leaves the list");
    for _ in 0..3 {
        if in_list(&rt) {
            break;
        }
        press(&mut rt, Key::Tab);
    }
    assert_eq!(focused(&rt).as_deref(), Some("item2"), "re-entering lands on the selected row");
}

#[test]
fn rows_without_an_action_stay_out_of_the_focus_order() {
    let mut rt = Runtime::new(State::default(), plain, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt), None);
    assert!(!rt.elements().iter().any(|e| e.role == "listbox"));
}
