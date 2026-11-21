use rune::{Cursor, Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Kit, Markdown};

#[derive(Default)]
struct S { selectable: bool, link_menus: Vec<(String, f32, f32)>, row_menus: usize, opened: Vec<String> }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let text = Markdown::new("Hello there and [link](https://x.dev/a) and [other](https://x.dev/b).")
        .selectable(s.selectable)
        .on_link(|s: &mut S, url| s.opened.push(url.to_owned()))
        .on_link_secondary(|s: &mut S, url, p, _| s.link_menus.push((url.to_owned(), p.x, p.y)))
        .build(&k);
    col().w_full().padding(10.0).child(text).on_secondary_press(|s: &mut S, _, _| s.row_menus += 1)
}

fn tick(rt: &mut Runtime<S>) {
    for _ in 0..4 { rt.frame(1.0 / 60.0); }
}

fn right_click(rt: &mut Runtime<S>, x: f32, y: f32) {
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    tick(rt);
}

#[test]
fn a_right_click_on_a_link_opens_its_menu_and_elsewhere_the_rows() {
    for selectable in [false, true] {
        let mut rt = Runtime::new(S { selectable, ..S::default() }, view, (480.0, 200.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let link = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("https://x.dev/a")).expect("the link");
        let at = (link.rect[0] + link.rect[2] / 2.0, link.rect[1] + link.rect[3] / 2.0);
        rt.pointer_move(at.0, at.1);
        tick(&mut rt);
        assert_eq!(rt.cursor(), Cursor::Pointer, "selectable {selectable}");

        right_click(&mut rt, at.0, at.1);
        assert_eq!(rt.state.link_menus, [("https://x.dev/a".to_owned(), at.0, at.1)], "selectable {selectable}");
        assert_eq!(rt.state.row_menus, 0, "the link took it (selectable {selectable})");
        assert!(rt.state.opened.is_empty());

        right_click(&mut rt, 16.0, at.1);
        assert_eq!(rt.state.row_menus, 1, "plain text goes to the row (selectable {selectable})");
        assert_eq!(rt.state.link_menus.len(), 1);
    }
}

#[test]
fn the_menu_key_on_a_focused_link_opens_its_menu_at_its_bottom_left() {
    let mut rt = Runtime::new(S::default(), view, (480.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    tick(&mut rt);
    let link = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("https://x.dev/a")).expect("the link");
    assert!(link.focused);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    tick(&mut rt);
    assert_eq!(rt.state.link_menus, [("https://x.dev/a".to_owned(), link.rect[0], link.rect[1] + link.rect[3])]);
    assert_eq!(rt.state.row_menus, 0);
}

fn link_center(rt: &Runtime<S>, url: &str) -> (f32, f32) {
    let link = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(url)).expect("the link");
    (link.rect[0] + link.rect[2] / 2.0, link.rect[1] + link.rect[3] / 2.0)
}

#[test]
fn a_right_click_that_moves_to_another_link_opens_no_menu() {
    let mut rt = Runtime::new(S::default(), view, (480.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let (a, b) = (link_center(&rt, "https://x.dev/a"), link_center(&rt, "https://x.dev/b"));
    rt.secondary_down(a.0, a.1);
    rt.pointer_move(b.0, b.1);
    rt.secondary_up(b.0, b.1);
    tick(&mut rt);
    assert!(rt.state.link_menus.is_empty(), "{:?}", rt.state.link_menus);
    assert_eq!(rt.state.row_menus, 0);
    right_click(&mut rt, b.0, b.1);
    assert_eq!(rt.state.link_menus, [("https://x.dev/b".to_owned(), b.0, b.1)]);
}
