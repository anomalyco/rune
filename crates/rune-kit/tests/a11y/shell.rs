//! The app shell: sidebar sections and rows, split panes and their grips, toolbar and page.

use rune::accesskit::{Action, ActionData};
use rune::{Cx, El, Env, Key, LiveRegion, Runtime, Tx, col};
use rune_kit::shell::{Page, PaneState, Reorder, ReorderLanding, RowStatus, Sidebar, SidebarRow, SidebarSection, SidebarToggle, SplitView, Toolbar};
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
fn sidebar_grip_set_value_lands_on_exact_width_from_off_grid() {
    let mut rt = mount(App { sidebar: PaneState::new(243.0).range(180.0, 420.0), ..App::default() }, shell);
    assert!(rt.accessibility_action("sidebar-grip", Action::SetValue, Some(ActionData::NumericValue(300.0))));
    assert!(rt.accessibility_action("sidebar-grip", Action::SetValue, Some(ActionData::NumericValue(300.0))));
    settle(&mut rt);
    assert_eq!(rt.state.sidebar.width(), 300.0, "SetValue lands on the requested width, even off the step grid");
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

fn flat_localized_shell(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let sidebar = Sidebar::new().label("Navigation").build(&k);
    SplitView::new(col().key("body").child(Button::new("Run").build(&k)))
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar)
        .sidebar_toggle(SidebarToggle::new().labels("显示侧边栏", "隐藏侧边栏"))
        .toolbar(Toolbar::new().title("Project"))
        .flat_main(true)
        .build(&k)
}

#[test]
fn localized_sidebar_toggle_and_flat_main_expose_accessible_names_and_roles() {
    let mut rt = mount(App { sidebar: PaneState::new(240.0).range(180.0, 420.0), ..App::default() }, flat_localized_shell);
    let tree = rt.accessibility_json();
    assert_eq!(said(node(&tree, "main-pane")).0, "main");
    let toggle = node(&tree, "sidebar-toggle");
    assert_eq!(said(toggle), ("button", "隐藏侧边栏"));
    assert_eq!(toggle["description"].as_str(), Some("隐藏侧边栏"));
    assert!(has(toggle, "expanded") && can(toggle, "collapse"));
    no_echo(&tree);

    assert!(act(&mut rt, "sidebar-toggle", Action::Collapse));
    assert!(!rt.state.sidebar.is_open());
    let tree = rt.accessibility_json();
    let toggle = node(&tree, "sidebar-toggle");
    assert_eq!(said(toggle), ("button", "显示侧边栏"));
    assert_eq!(toggle["description"].as_str(), Some("显示侧边栏"));
    assert!(has(toggle, "collapsed") && can(toggle, "expand"));
    no_echo(&tree);
}

#[test]
fn reorder_tx_preserves_listitem_role_set_position_and_keyboard_navigation() {
    struct ReorderApp {
        items: Vec<&'static str>,
        picked: usize,
        reorder: Reorder,
    }

    fn view(s: &ReorderApp, _: &mut Cx<ReorderApp>) -> El {
        let k = kit();
        let count = s.items.len();
        let sb = Sidebar::new().label("Places").row_gap(2.0);
        let sb = s.items.iter().enumerate().fold(sb, |sb, (i, name)| {
            sb.child(
                SidebarRow::new(*name)
                    .height(24.0)
                    .selected(s.picked == i)
                    .on_select(move |s: &mut ReorderApp, _| s.picked = i)
                    .reorder_tx(
                        &s.reorder,
                        i,
                        count,
                        |s: &mut ReorderApp| &mut s.reorder,
                        |s: &mut ReorderApp, from, to, tx: &mut Tx| {
                            let moved = s.items.remove(from);
                            s.items.insert(to, moved);
                            s.picked = to;
                            tx.announce(format!("Moved {moved} to position {}", to + 1), LiveRegion::Polite);
                        },
                    )
                    .build(&k)
                    .key(*name),
            )
        });
        sb.build(&k)
    }

    let state = ReorderApp {
        items: vec!["Alpha", "Bravo", "Charlie"],
        picked: 0,
        reorder: Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow).cancel_outside_x(80.0),
    };
    let mut rt = Runtime::new(state, view, (260.0, 240.0), Env::new(false, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);

    let tree = rt.accessibility_json();
    assert_eq!(said(node(&tree, "sidebar-list")), ("list", "Places"));
    let alpha = node(&tree, "Alpha");
    let bravo = node(&tree, "Bravo");
    assert_eq!(said(alpha), ("list-item", "Alpha"));
    assert_eq!(alpha["position"], 1);
    assert_eq!(bravo["position"], 2);
    assert!(has(alpha, "selected"));
    no_echo(&tree);

    // Keyboard navigation still steps selection across reorder_tx rows.
    assert!(rt.accessibility_action("Alpha", Action::Focus, None));
    rt.frame(1.0 / 60.0);
    rt.key(rune::KeyEvent { key: Key::Down, mods: rune::Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.picked, 1);

    // Drag Alpha down 39 pt (HalfRow -> position 2) and check updated set positions.
    let rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("Alpha")).expect("Alpha").rect;
    let (x, y) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 39.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x, y + 39.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.items, ["Bravo", "Alpha", "Charlie"]);
    let tree = rt.accessibility_json();
    assert_eq!(node(&tree, "Bravo")["position"], 1);
    assert_eq!(node(&tree, "Alpha")["position"], 2);
    assert!(has(node(&tree, "Alpha"), "selected"));
}
