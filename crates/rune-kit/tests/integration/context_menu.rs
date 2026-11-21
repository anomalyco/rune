use rune::{Cx, El, Env, PointerPress, Runtime, el};
use rune_kit::{Kit, MenuItem, context_menu, root};

#[derive(Default)]
struct State { point: Option<PointerPress> }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let trigger = el().size(300.0, 200.0).on_secondary_press(|s: &mut State, p, _| s.point = Some(p));
    let mut layer = Vec::new();
    let body = context_menu("context", trigger, s.point)
        .item(MenuItem::new("Copy").on_select(|s: &mut State| s.point = None))
        .on_close(|s: &mut State| s.point = None)
        .build(&k, &mut layer);
    root(body, layer)
}

#[test]
fn context_menu_opens_at_pointer_and_stays_inside_viewport() {
    let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.secondary_down(290.0, 190.0);
    rt.secondary_up(290.0, 190.0);
    rt.frame(0.0);
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    let menu = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("context-menu")).unwrap();
    assert!(menu.rect[0] >= 0.0 && menu.rect[1] >= 0.0);
    assert!(menu.rect[0] + menu.rect[2] <= 300.01);
    assert!(menu.rect[1] + menu.rect[3] <= 200.01);
    let (x, y) = rt.find("context-item0").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert!(rt.state.point.is_none());
}

mod keyboard {
    use rune::{Cx, El, Env, Key, KeyEvent, Mods, PointerPress, Runtime, col, el};
    use rune_kit::{Kit, MenuItem, context_menu, root};

    #[derive(Default)]
    struct State { point: Option<PointerPress>, row_keys: bool }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let row = el().key("row").size(200.0, 40.0).focusable(true)
            .on_key(|s: &mut State, ev, _| s.row_keys && ev.key == Key::Function(10))
            .on_secondary_press(|s: &mut State, p, _| s.point = Some(p));
        let mut layer = Vec::new();
        let body = context_menu("context", row, s.point)
            .item(MenuItem::new("Copy").on_select(|s: &mut State| s.point = None))
            .on_close(|s: &mut State| s.point = None)
            .build(&k, &mut layer);
        root(col().padding(20.0).child(body).child(el().key("other").size(200.0, 40.0).focusable(true)), layer)
    }

    fn settle(rt: &mut Runtime<State>) {
        rt.frame(0.0);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
    }

    fn press(rt: &mut Runtime<State>, key: Key, shift: bool) {
        rt.key(KeyEvent { key, mods: Mods { shift, ..Mods::default() } });
        settle(rt);
    }

    fn focused(rt: &Runtime<State>) -> Option<String> {
        rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
    }

    fn menu_open(rt: &Runtime<State>) -> bool {
        rt.elements().iter().any(|e| e.key.as_deref() == Some("context-menu"))
    }

    fn focused_row() -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (400.0, 300.0), Env::new(true, 1.0));
        settle(&mut rt);
        press(&mut rt, Key::Tab, false);
        assert_eq!(focused(&rt).as_deref(), Some("row"));
        rt
    }

    #[test]
    fn shift_f10_opens_the_focused_rows_menu_and_escape_returns_focus() {
        let mut rt = focused_row();
        press(&mut rt, Key::Function(10), true);
        let p = rt.state.point.expect("the row's menu opened");
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap();
        assert_eq!((p.x, p.y), (row.rect[0], row.rect[1] + row.rect[3]), "at the row's bottom-left");
        assert!(menu_open(&rt));
        press(&mut rt, Key::Escape, false);
        assert!(!menu_open(&rt) && rt.state.point.is_none());
        assert_eq!(focused(&rt).as_deref(), Some("row"), "focus returns to the row");

        press(&mut rt, Key::ContextMenu, false);
        assert!(rt.state.point.is_some() && menu_open(&rt), "the Menu key opens it too");
    }

    #[test]
    fn a_key_handler_that_takes_shift_f10_keeps_it_and_nothing_focused_is_a_no_op() {
        let mut rt = focused_row();
        rt.state.row_keys = true;
        press(&mut rt, Key::Function(10), true);
        assert!(rt.state.point.is_none());
        rt.state.row_keys = false;
        press(&mut rt, Key::Function(10), false);
        assert!(rt.state.point.is_none(), "F10 without Shift");

        let mut rt = Runtime::new(State::default(), view, (400.0, 300.0), Env::new(true, 1.0));
        settle(&mut rt);
        press(&mut rt, Key::ContextMenu, false);
        assert!(rt.state.point.is_none() && focused(&rt).is_none());
    }
}
