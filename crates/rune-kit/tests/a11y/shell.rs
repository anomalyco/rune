//! The app shell: sidebar sections and rows, split panes and their grips, toolbar and page.

use rune::accesskit::{Action, ActionData};
use rune::{Cx, El, Key, col};
use rune_kit::shell::{Page, PaneState, RowStatus, Sidebar, SidebarRow, SidebarSection, SplitView, Toolbar};
use rune_kit::*;

use crate::support::*;

const NAMES: [&str; 3] = ["Alpha", "Bravo", "Charlie"];

fn shell(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let rows = (0..NAMES.len()).map(|i| {
        let mut r = SidebarRow::new(NAMES[i]).selected(s.picked == Some(i)).on_select(move |s: &mut App, _| s.picked = Some(i));
        r = match i {
            0 => r.badge("3").detail("2m"),
            1 => r.subtitle("bravo@example.com").status(RowStatus::Busy),
            _ => r.status(RowStatus::Unread),
        };
        r.build(&k).key(format!("row{i}"))
    });
    let sidebar = Sidebar::new()
        .label("Sessions")
        .section(SidebarSection::new("Today").count(3).collapsed(s.on).on_toggle(|s: &mut App, _| s.on = !s.on).rows(rows))
        .build(&k);
    SplitView::new(col().key("body").child(Button::new("Run").build(&k)))
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar)
        .toolbar(Toolbar::new().title("Project").trailing(IconButton::new(k.glyph.copy).label("Share").build(&k)))
        .build(&k)
}

#[test]
fn the_shell_reads_its_landmarks_sections_rows_and_grip() {
    let mut rt = mount(App { picked: Some(1), sidebar: PaneState::new(240.0).range(180.0, 420.0), ..App::default() }, shell);
    let tree = rt.accessibility_json();
    assert_eq!(said(node(&tree, "sidebar-pane")), ("region", "Sidebar"));
    assert_eq!(said(node(&tree, "main-pane")).0, "main");
    assert_eq!(said(node(&tree, "toolbar")), ("toolbar", "Project"));
    assert_eq!(said(node(&tree, "sidebar-list")), ("list", "Sessions"));
    assert_eq!(said(node(&tree, "Today")), ("group", "Today"));
    let header = node(&tree, "section-header");
    assert_eq!((said(header), header["description"].as_str()), (("button", "Today"), Some("3 items")));
    assert!(has(header, "expanded") && can(header, "collapse"));
    let alpha = node(&tree, "row0");
    assert_eq!((said(alpha), alpha["description"].as_str()), (("list-item", "Alpha"), Some("2m, 3")));
    assert!(has(alpha, "unselected"));
    let bravo = node(&tree, "row1");
    assert_eq!((said(bravo), bravo["description"].as_str()), (("list-item", "Bravo, bravo@example.com"), Some("Working")));
    assert!(has(bravo, "selected"));
    assert_eq!(node(&tree, "row2")["description"], "Unread");
    let grip = node(&tree, "sidebar-grip");
    assert_eq!(said(grip), ("splitter", "Resize sidebar"));
    assert_eq!(grip["range"], serde_json::json!([240.0, 180.0, 420.0, 8.0]));
    assert!(can(grip, "increment") && can(grip, "set-value") && !can(grip, "focus"), "a reader resizes the pane; Tab skips the grip");
    let toggle = node(&tree, "sidebar-toggle");
    assert_eq!(said(toggle), ("button", "Hide sidebar"));
    assert!(has(toggle, "expanded"));
    no_echo(&tree);
    golden("shell", &tree);

    assert!(act(&mut rt, "sidebar-grip", Action::Increment));
    assert_eq!(rt.state.sidebar.width(), 248.0);
    assert!(rt.accessibility_action("sidebar-grip", Action::SetValue, Some(ActionData::NumericValue(304.0))));
    settle(&mut rt);
    assert_eq!(rt.state.sidebar.width(), 304.0, "a reader sets the width in grip steps");
    assert!(act(&mut rt, "row0", Action::Click));
    assert_eq!(rt.state.picked, Some(0));
    act(&mut rt, "row0", Action::Focus);
    press_key(&mut rt, Key::Down);
    assert_eq!(rt.state.picked, Some(1), "selection follows the arrows");
    assert!(act(&mut rt, "section-header", Action::Collapse));
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "section-header"), "collapsed") && find(&tree, "row0").is_none());
    assert!(act(&mut rt, "sidebar-toggle", Action::Collapse));
    assert!(!rt.state.sidebar.is_open());
    let tree = rt.accessibility_json();
    assert!(find(&tree, "sidebar-list").is_none(), "a closed sidebar leaves the tree");
    assert!(has(node(&tree, "sidebar-toggle"), "collapsed"));
}

fn page(_: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    Page::new(col().key("settings")).title("Settings").leading(IconButton::new(k.glyph.close).label("Back").build(&k)).build(&k)
}

#[test]
fn a_page_names_its_toolbar() {
    let rt = mount(App::default(), page);
    let tree = rt.accessibility_json();
    assert_eq!(said(node(&tree, "toolbar")), ("toolbar", "Settings"));
    assert_eq!(said(node(&tree, "main-pane")).0, "main");
    assert!(!named(&tree, "Back").is_empty());
    golden("page", &tree);
}
