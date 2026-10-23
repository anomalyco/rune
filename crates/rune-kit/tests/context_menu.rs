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
