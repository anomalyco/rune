use rune::{Cx, El, Env, Key, KeyEvent, Runtime};
use rune_kit::{Button, Kit, Popover, root};

#[derive(Default)]
struct State { open: bool }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let trigger = Button::new("Open").on_press(|s: &mut State| s.open = true).build(&k).key("open");
    let body = Popover::new("details").trigger(trigger).open(s.open)
        .on_close(|s: &mut State| s.open = false)
        .child(Button::new("Close").on_press(|s: &mut State| s.open = false).build(&k).key("close"))
        .build(&k, &mut layer);
    root(body, layer)
}

#[test]
fn popover_close_restores_its_trigger_before_exit_finishes() {
    let mut rt = Runtime::new(State::default(), view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for key in [Key::Tab, Key::Text(" ".into()), Key::Tab] {
        rt.key(KeyEvent { key, mods: Default::default() }); rt.frame(0.0);
    }
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).unwrap().key.as_deref(), Some("close"));
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Default::default() }); rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).unwrap().key.as_deref(), Some("open"));
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).unwrap().key.as_deref(), Some("open"));
}
