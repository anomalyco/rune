//! Web DOM accessibility mirror parity for public `rune-kit` components:
//! `Button::disabled`, `Menu` with disabled `MenuItem`, `Slider`, `Table`, and `Tree`.

use std::borrow::Cow;

use rune::{Align, Cx, El, Env, Runtime, col};
use rune_kit::*;
use serde_json::{Value, json};

use crate::support::{App, kit};

fn items_json<S: 'static>(rt: &Runtime<S>) -> Vec<Value> {
    serde_json::from_str(&rt.a11y_json()).expect("valid a11y_json")
}

#[test]
fn kit_menu_and_disabled_button_mirror_roles_and_disabled_state() {
    #[derive(Default)]
    struct MenuState {
        open: bool,
        picks: usize,
    }

    fn view(s: &MenuState, _: &mut Cx<MenuState>) -> El {
        let k = kit();
        let mut layers = Vec::new();
        let trigger = Button::new("Actions")
            .on_press(|s: &mut MenuState| s.open = true)
            .build(&k);
        let menu = Menu::new("actions")
            .trigger(trigger)
            .open(s.open)
            .item(MenuItem::new("Open").on_select(|s: &mut MenuState| {
                s.picks += 1;
                s.open = false;
            }))
            .item(MenuItem::new("Disabled item").disabled(true))
            .on_close(|s: &mut MenuState| s.open = false)
            .build(&k, &mut layers);
        rune_kit::root(
            col()
                .w_full()
                .h_full()
                .child(menu)
                .child(Button::new("Disabled button").disabled(true).build(&k)),
            layers,
        )
    }

    let mut rt = Runtime::new(MenuState { open: false, picks: 0 }, view, (480.0, 360.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let closed_items = items_json(&rt);
    let dis_btn = closed_items
        .iter()
        .find(|v| v["k"] == "button" && v["t"] == "Disabled button")
        .unwrap_or_else(|| panic!("missing disabled button: {closed_items:?}"));
    assert_eq!(dis_btn.get("dis"), Some(&json!(true)));

    rt.update(|s, _| s.open = true);
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let items = items_json(&rt);

    let menu_i = items
        .iter()
        .position(|v| v["k"] == "group" && v.get("o") == Some(&json!("menu")))
        .unwrap_or_else(|| panic!("missing menu container: {items:?}"));
    let inside_menu = |item: &Value| -> bool {
        let mut cur = item.get("a").and_then(Value::as_u64).map(|n| n as usize);
        while let Some(idx) = cur {
            if idx == menu_i {
                return true;
            }
            cur = items.get(idx).and_then(|p| p.get("a")).and_then(Value::as_u64).map(|n| n as usize);
        }
        false
    };

    let open_item = items
        .iter()
        .find(|v| v.get("o") == Some(&json!("menuitem")) && v["t"] == "Open")
        .unwrap_or_else(|| panic!("missing Open menuitem: {items:?}"));
    assert_eq!(open_item.get("dis"), None);
    assert!(inside_menu(open_item), "Open menuitem must sit inside menu container: {items:?}");

    let disabled_item = items
        .iter()
        .find(|v| v.get("o") == Some(&json!("menuitem")) && v["t"] == "Disabled item")
        .unwrap_or_else(|| panic!("missing Disabled item menuitem: {items:?}"));
    assert_eq!(disabled_item.get("dis"), Some(&json!(true)));
    assert!(inside_menu(disabled_item), "Disabled item menuitem must sit inside menu container: {items:?}");

    let trigger_item = items
        .iter()
        .find(|v| v["k"] == "button" && v["t"] == "Actions")
        .unwrap_or_else(|| panic!("menu trigger must stay readable in web mirror: {items:?}"));
    assert_eq!(trigger_item.get("x"), Some(&json!(true)));
    assert_eq!(trigger_item.get("hp"), Some(&json!("menu")));
    assert_eq!(trigger_item.get("ct"), Some(&json!("actions-menu")));
    assert_eq!(trigger_item.get("dis"), None);
    assert!(
        !items.iter().any(|v| v["t"] == "Disabled button"),
        "unrelated background button stays hidden in web mirror: {items:?}",
    );
}

#[test]
fn kit_slider_table_and_tree_mirror_range_and_container_roles() {
    struct ViewState {
        app: App,
        files: TreeModel<&'static str>,
    }

    fn view(s: &ViewState, _: &mut Cx<ViewState>) -> El {
        let k = kit();
        let slider = Slider::new("volume", 40.0)
            .range(0.0..=100.0)
            .step(10.0)
            .label("Volume")
            .value_text("40%")
            .build(&k);
        let disabled_slider = Slider::new("locked", 20.0)
            .range(0.0..=100.0)
            .label("Locked")
            .state(State::Disabled)
            .build(&k);
        let table = Table::new(
            |s: &ViewState| &s.app.table,
            |s: &mut ViewState| &mut s.app.table,
            |_| 1,
            |_, i| Cow::Owned(format!("f{i}")),
        )
        .name("Files")
        .column(TableColumn::new("name", "Name").flex(1.0))
        .column(TableColumn::new("size", "Size").width(90.0).align(Align::End).resizable(false))
        .cell(|_: &ViewState, _, col| match col {
            "name" => Cell::text("main.rs"),
            _ => Cell::text("4 KB"),
        })
        .build(s, &k)
        .h(120.0);
        let tree = Tree::new(
            |s: &ViewState| &s.files,
            |s: &mut ViewState| &mut s.files,
            |n| *n,
        )
        .name("Workspace")
        .build(s, &k)
        .h(120.0);
        col().w_full().h_full().gap(8.0).child(slider).child(disabled_slider).child(table).child(tree)
    }

    let mut files = TreeModel::new().key("files");
    files.insert(None, "src", "src");
    files.insert(Some("src"), "main", "main.rs");
    files.expand_to("main");
    let mut rt = Runtime::new(
        ViewState { app: App::default(), files },
        view,
        (480.0, 480.0),
        Env::new(false, 1.0),
    );
    rt.frame(0.0);
    rt.frame(0.0);
    let items = items_json(&rt);

    let vol = items
        .iter()
        .find(|v| v.get("o") == Some(&json!("slider")) && v["t"] == "Volume")
        .unwrap_or_else(|| panic!("missing Volume slider: {items:?}"));
    assert_eq!((&vol["rg"], &vol["vt"], vol.get("dis")), (&json!([0.0, 100.0, 40.0]), &json!("40%"), None));

    let locked = items
        .iter()
        .find(|v| v.get("o") == Some(&json!("slider")) && v["t"] == "Locked")
        .unwrap_or_else(|| panic!("missing Locked slider: {items:?}"));
    assert_eq!((&locked["rg"], locked.get("dis")), (&json!([0.0, 100.0, 20.0]), Some(&json!(true))));

    let grid_i = items
        .iter()
        .position(|v| v["k"] == "group" && v.get("o") == Some(&json!("grid")))
        .unwrap_or_else(|| panic!("missing grid container for Table: {items:?}"));
    assert_eq!(items[grid_i]["t"], json!("Files"));
    assert!(items.iter().any(|v| v.get("o") == Some(&json!("columnheader"))), "missing columnheader: {items:?}");
    assert!(items.iter().any(|v| v.get("o") == Some(&json!("gridcell"))), "missing gridcell: {items:?}");

    let tree_i = items
        .iter()
        .position(|v| v["k"] == "group" && v.get("o") == Some(&json!("tree")))
        .unwrap_or_else(|| panic!("missing tree container for Tree: {items:?}"));
    assert_eq!(items[tree_i]["t"], json!("Workspace"));
    let src_item = items
        .iter()
        .find(|v| v.get("o") == Some(&json!("treeitem")) && v["t"] == "src")
        .unwrap_or_else(|| panic!("missing src treeitem: {items:?}"));
    assert_eq!((&src_item["x"], &src_item["l"]), (&json!(true), &json!(1)));
    let main_item = items
        .iter()
        .find(|v| v.get("o") == Some(&json!("treeitem")) && v["t"] == "main.rs")
        .unwrap_or_else(|| panic!("missing main.rs treeitem: {items:?}"));
    assert_eq!(main_item["l"], json!(2));
}
