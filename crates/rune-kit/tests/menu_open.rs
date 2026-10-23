use rune::{Cx, El, Env, Runtime, text};
use rune_kit::{Kit, Menu, MenuEntry, MenuItem, root};

#[derive(Default)]
struct State { open: bool, sub: bool }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let body = Menu::new("file")
        .trigger(text("File"))
        .item(MenuItem::new("Copy"))
        .item(MenuItem::new("More").submenu(vec![MenuEntry::item(MenuItem::new("Deeper"))], s.sub))
        .open(s.open)
        .build(&k, &mut layer);
    root(rune::col().w_full().h_full().child(body), layer)
}

fn surface<S>(rt: &Runtime<S>, key: &str) -> Option<rune::runtime::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

#[test]
fn menus_and_submenus_open_on_the_next_frame_and_fade_only_on_close() {
    let mut rt = Runtime::new(State::default(), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 120.0);
    let menu = surface(&rt, "file-menu").unwrap();
    assert_eq!(menu.opacity, 1.0, "menu fades in");
    assert_eq!(menu.rect, menu.target, "menu scales in");
    let (x, y) = rt.find("file-item0").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(surface(&rt, "file-item0").unwrap().hover, 1.0, "item highlight lands on arrival");

    rt.update(|s, _| s.sub = true);
    rt.frame(1.0 / 120.0);
    let sub = surface(&rt, "file-item1-sub-menu").unwrap();
    assert_eq!(sub.opacity, 1.0, "submenu fades in");
    assert_eq!(sub.rect, sub.target, "submenu scales in");

    rt.update(|s, _| { s.open = false; s.sub = false; });
    rt.frame(1.0 / 120.0);
    rt.frame(0.1);
    let closing = surface(&rt, "file-menu").expect("menu fades out instead of vanishing");
    assert!(closing.opacity > 0.0 && closing.opacity < 1.0, "closing opacity {}", closing.opacity);
    assert_eq!(closing.rect[2], closing.target[2], "close fades without scaling");
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert!(surface(&rt, "file-menu").is_none());
}
