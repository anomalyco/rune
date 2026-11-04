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

fn contents_are_present<S>(rt: &Runtime<S>, menu: &rune::runtime::ElementInfo) {
    let prefix = format!("{}/", menu.path);
    for e in rt.elements().into_iter().filter(|e| e.path.starts_with(&prefix)) {
        assert_eq!(e.opacity, 1.0, "{} fades in inside an instant menu", e.path);
    }
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
    contents_are_present(&rt, &menu);
    let (x, y) = rt.find("file-item0").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(surface(&rt, "file-item0").unwrap().hover, 1.0, "item highlight lands on arrival");

    rt.update(|s, _| s.sub = true);
    rt.frame(1.0 / 120.0);
    let sub = surface(&rt, "file-item1-sub-menu").unwrap();
    assert_eq!(sub.opacity, 1.0, "submenu fades in");
    assert_eq!(sub.rect, sub.target, "submenu scales in");
    contents_are_present(&rt, &sub);
    let (x, y) = rt.find("file-item1-sub-item0").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(surface(&rt, "file-item1-sub-item0").unwrap().hover, 1.0, "submenu highlight lands on arrival");

    rt.update(|s, _| { s.open = false; s.sub = false; });
    rt.frame(1.0 / 120.0);
    rt.frame(0.1);
    let closing = surface(&rt, "file-menu").expect("menu fades out instead of vanishing");
    assert!(closing.opacity > 0.0 && closing.opacity < 1.0, "closing opacity {}", closing.opacity);
    assert_eq!(closing.rect[2], closing.target[2], "close fades without scaling");
    let item = surface(&rt, "file-item0").expect("menu contents remain during the closing fade");
    assert_eq!(item.opacity, closing.opacity, "contents ride the surface's closing fade");
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert!(surface(&rt, "file-menu").is_none());
}

#[test]
fn menu_open_motion_gate() {
    use rune::motion_gate::{self, Run};

    motion_gate::gate_rows(vec![("menu/open-submenu-close".into(), Box::new(|| {
        let mut r = Run::new(Runtime::new(State::default(), view, (400.0, 300.0), Env::new(true, 1.0)), 120.0);
        r.idle(0.5);
        r.step();
        r.act("open", |rt| rt.update(|s, _| s.open = true));
        r.play(0.1);
        r.act("submenu", |rt| rt.update(|s, _| s.sub = true));
        r.play(0.1);
        r.act("close", |rt| rt.update(|s, _| { s.open = false; s.sub = false; }));
        r.play(1.0);
        r.rec
    }))], &[]);
}
