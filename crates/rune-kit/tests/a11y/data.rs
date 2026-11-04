//! Collections and surfaces: list, table, card and diff view read their items, place in the
//! set, selection and headers.

use std::borrow::Cow;

use rune::accesskit::Action;
use rune::{Align, Cx, El, Key, col, text};
use rune_kit::diff::{DiffState, DiffView, FileDiff};
use rune_kit::*;

use crate::support::*;

fn lists(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let mut picker = List::new().active(None);
    for (i, name) in ["Inbox", "Drafts", "Sent"].into_iter().enumerate() {
        picker = picker.item(ListItem::new(name).group(if i < 2 { "Mail" } else { "Other" }).selected(s.picked == Some(i))
            .on_select(move |s: &mut App, _| s.picked = Some(i)));
    }
    col()
        .gap(8.0)
        .child(picker.build(&k).key("mail"))
        .child(List::new().item(ListItem::new("Read only")).item(ListItem::new("Rows")).build(&k).key("plain"))
}

#[test]
fn lists_read_their_rows_place_and_selection() {
    let mut rt = mount(App { picked: Some(1), ..App::default() }, lists);
    let tree = rt.accessibility_json();
    let mail = node(node(&tree, "mail"), "scroll");
    assert_eq!(said(mail).0, "list-box");
    assert_eq!(said(node(mail, "group0")), ("group", "Mail"));
    let inbox = node(mail, "item0");
    assert_eq!(said(inbox), ("list-box-option", "Inbox"));
    assert!(has(inbox, "unselected") && inbox["position"] == 1);
    assert!(has(node(mail, "item1"), "selected"));
    let plain = node(node(&tree, "plain"), "scroll");
    assert_eq!(said(plain).0, "list");
    assert_eq!(said(node(plain, "item1")), ("list-item", "Rows"));
    assert_eq!(node(plain, "item1")["position"], 2);
    no_echo(&tree);
    golden("list", &tree);

    assert!(act(&mut rt, "item0", Action::Click));
    assert_eq!(rt.state.picked, Some(0));
    act(&mut rt, "item0", Action::Focus);
    press_key(&mut rt, Key::Down);
    press_key(&mut rt, Key::Enter);
    assert_eq!(rt.state.picked, Some(1));
    assert!(has(node(&rt.accessibility_json(), "item1"), "selected"));
}

const NAMES: [&str; 4] = ["alpha.rs", "beta.rs", "gamma.rs", "delta.rs"];

fn table(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().size(500.0, 300.0).child(
        Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |_| NAMES.len(), |_, i| Cow::Owned(format!("f{i}")))
            .name("Files")
            .column(TableColumn::new("name", "Name").flex(1.0).sortable())
            .column(TableColumn::new("size", "Size").width(90.0).align(Align::End).resizable(false))
            .cell(|_: &App, i, col| match col {
                "name" => Cell::text(NAMES[i]),
                _ => Cell::text(format!("{} KB", i + 1)).mono(),
            })
            .on_open(|s: &mut App, id, _| s.chosen = Some(id.to_owned()))
            .build(s, &k)
            .h(300.0),
    )
}

#[test]
fn a_table_reads_its_headers_rows_and_selection() {
    let mut rt = mount(App::default(), table);
    rt.update(|s, _| s.table.select("f1"));
    settle(&mut rt);
    let tree = rt.accessibility_json();
    let grid = node(&tree, "table");
    assert_eq!(said(grid), ("grid", "Files"));
    assert!(can(grid, "focus"));
    let name = node(&node(&tree, "header").clone(), "name").clone();
    assert_eq!(said(&name), ("column-header", "Name"));
    assert!(can(&name, "click"), "a sortable header sorts on click");
    let row = node(&tree, "f1");
    assert_eq!(said(row), ("row", "beta.rs 2 KB"));
    assert!(has(row, "selected") && row["position"] == 2);
    assert!(!has(node(&tree, "f0"), "selected"));
    assert_eq!(said(node(row, "size")), ("grid-cell", "2 KB"));
    assert_eq!(said(node(&tree, "resize")), ("splitter", "Resize Name"));
    golden("table", &tree);

    assert!(act(&mut rt, "name", Action::Click));
    let name = node(&node(&rt.accessibility_json(), "header").clone(), "name").clone();
    assert_eq!((name["name"].as_str(), name["sort"].as_str()), (Some("Name"), Some("ascending")));
    assert!(act(&mut rt, "f3", Action::Click));
    assert!(has(node(&rt.accessibility_json(), "f3"), "selected"), "an assistive click selects the row");
    act(&mut rt, "table", Action::Focus);
    press_key(&mut rt, Key::Up);
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "f2"), "selected"));
    assert_eq!(node(&tree, "table")["activeDescendant"], "f2", "focus stays on the table; a reader follows the cursor row");
    press_key(&mut rt, Key::Enter);
    assert_eq!(rt.state.chosen.as_deref(), Some("f2"));
}

fn card(_: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    Card::new()
        .title("Build failed", Some(k.glyph.close))
        .description("3 tests failed on Linux")
        .action(Button::new("Retry").build(&k))
        .build(&k)
        .key("card")
}

#[test]
fn a_card_is_a_group_named_by_its_title() {
    let rt = mount(App::default(), card);
    let tree = rt.accessibility_json();
    let card = node(&tree, "card");
    assert_eq!(said(card), ("group", "Build failed"));
    assert!(find(card, "rule").is_none() && with_role(card, "image").is_empty(), "the accent rule and icon are decoration");
    assert_eq!(said(node(card, "description")), ("label", "3 tests failed on Linux"));
    assert_eq!(named(card, "Retry")[0]["role"], "button");
    golden("card", &tree);
}

fn diff(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().w_full().h(400.0).child(DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff).build(s, &k)).child(text("end"))
}

#[test]
fn a_diff_reads_its_file_header_and_lines() {
    let old = "fn main() {\n    println!(\"hi\");\n}\n";
    let new = "fn main() {\n    println!(\"hello\");\n}\n";
    let state = DiffState::new(vec![FileDiff::from_texts("src/main.rs", old, new)]);
    let mut rt = mount(App { diff: state, ..App::default() }, diff);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    let tree = rt.accessibility_json();
    let lines = with_role(&tree, "list");
    let list = lines.iter().find(|l| l["name"] == "Diff").expect("the diff's rows are a list");
    let heading = &with_role(&tree, "heading")[0];
    assert_eq!(heading["name"], "src/main.rs, 1 added, 1 removed");
    let toggle = named(&tree, "Collapse file")[0];
    assert!(has(toggle, "expanded") && can(toggle, "collapse"));
    let items = with_role(list, "list-item");
    assert!(!items.is_empty() && items.iter().all(|i| i["name"].as_str().is_some_and(|n| !n.is_empty())));
    assert!(with_role(list, "button").iter().all(|b| b["name"].as_str().is_some_and(|n| !n.is_empty())), "no nameless buttons");
    golden("diff", &tree);
    assert!(rt.accessibility_action("toggle", Action::Collapse, None));
    settle(&mut rt);
    assert!(has(named(&rt.accessibility_json(), "Expand file")[0], "collapsed"));
}
