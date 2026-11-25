use rune::{Cx, El, Env, Runtime, text};
use rune_kit::{Kit, Menu, MenuItem, root};

#[derive(Default)]
struct S {
    more: bool,
    custom: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let mut menu = Menu::new("m").trigger(text("Menu")).standard();
    if s.more {
        menu = menu.item(MenuItem::new("New").key("new"));
    }
    menu = menu.item(MenuItem::new("Add").key("add")).separator_keyed("edit-sep").item(MenuItem::new("Arrange").key("arrange"));
    if s.custom {
        menu = menu.padding(6.0).separator_spacing(3.0, 9.0).inner_border(0.0);
    }
    let body = menu.open(true).build(&k, &mut layer);
    root(rune::col().w_full().h_full().child(body), layer)
}

fn rect(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}")).rect
}

#[test]
fn a_keyed_separator_keeps_its_key_and_the_spacing_defaults_to_the_padding() {
    let mut rt = Runtime::new(S::default(), view, (480.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (add, sep, arrange) = (rect(&rt, "add"), rect(&rt, "edit-sep"), rect(&rt, "arrange"));
    assert_eq!(sep[1] - (add[1] + add[3]), 4.0);
    assert_eq!(arrange[1] - (sep[1] + sep[3]), 4.0);
    rt.update(|s, _| s.more = true);
    rt.frame(16.0);
    rect(&rt, "edit-sep");
}

#[test]
fn padding_separator_spacing_and_border_can_be_set() {
    let mut rt = Runtime::new(S { custom: true, ..S::default() }, view, (480.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (rows, add, sep, arrange) = (rect(&rt, "rows"), rect(&rt, "add"), rect(&rt, "edit-sep"), rect(&rt, "arrange"));
    assert_eq!(add[1] - rows[1], 6.0);
    assert_eq!(add[0] - rows[0], 6.0);
    assert_eq!(sep[1] - (add[1] + add[3]), 3.0);
    assert_eq!(arrange[1] - (sep[1] + sep[3]), 9.0);
    assert_eq!(sep[0], rows[0]);
    assert_eq!(sep[2], rows[2]);
}
