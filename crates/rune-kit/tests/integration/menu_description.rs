use rune::{Cx, El, Env, Runtime, text};
use rune_kit::{Kit, Menu, MenuItem, root};

fn view(_: &(), _: &mut Cx<()>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let body = Menu::new("pick")
        .trigger(text("Pick"))
        .item(MenuItem::new("Off").checked(false))
        .item(MenuItem::new("Here").description("Nothing leaves this computer").checked(true))
        .item(MenuItem::new("Away").description("Turn it on in settings").checked(false).disabled(true))
        .open(true)
        .build(&k, &mut layer);
    root(rune::col().w_full().h_full().child(body), layer)
}

fn element(rt: &Runtime<()>, key: &str) -> rune::runtime::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"))
}

#[test]
fn a_described_item_grows_to_show_its_second_line() {
    let mut rt = Runtime::new((), view, (480.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let plain = element(&rt, "pick-item0").rect;
    let described = element(&rt, "pick-item1");
    assert_eq!(plain[3], 28.0);
    assert!(described.rect[3] > plain[3], "described item is {} tall", described.rect[3]);
    let prefix = format!("{}/", described.path);
    let line = rt
        .elements()
        .into_iter()
        .find(|e| e.path.starts_with(&prefix) && e.key.as_deref() == Some("description"))
        .expect("the description shows");
    let label = rt
        .elements()
        .into_iter()
        .find(|e| e.path.starts_with(&prefix) && e.key.as_deref() == Some("label"))
        .unwrap();
    assert!(line.rect[1] >= label.rect[1] + label.rect[3], "the description sits under the label");
    assert!(line.rect[1] + line.rect[3] <= described.rect[1] + described.rect[3]);
}
