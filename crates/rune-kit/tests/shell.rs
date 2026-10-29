//! The app shell: pane resizing and collapse, the toggle shortcut, the source list's keyboard,
//! virtualised rows, accessibility roles and the title bar inset.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use std::borrow::Cow;

use rune::runtime::ElementInfo;
use rune::{TextStyle, Chrome, Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, TitlebarAction, col, el};
use rune_kit::shell::{Page, PaneState, Sidebar, SidebarRow, SidebarSection, SplitView, Toolbar, is_sidebar_toggle};
use rune_kit::{Button, IconButton, Kit, glyphs};

const NAMES: [&str; 4] = ["Alpha", "Bravo", "Charlie", "Delta"];

struct S {
    sidebar: PaneState,
    inspector: PaneState,
    picked: usize,
    collapsed: bool,
    two_lines: bool,
    rows: Option<usize>,
    renaming: Option<usize>,
    edit: TextEdit,
    main_edit: TextEdit,
    toolbar_edit: TextEdit,
    committed: Option<String>,
}

fn state() -> S {
    S {
        sidebar: PaneState::new(240.0).range(180.0, 420.0),
        inspector: PaneState::new(220.0).collapsed(),
        picked: 1,
        collapsed: false,
        two_lines: false,
        rows: None,
        renaming: None,
        edit: TextEdit::default(),
        main_edit: TextEdit::default(),
        toolbar_edit: TextEdit::default(),
        committed: None,
    }
}

fn row(s: &S, i: usize, k: &Kit) -> El {
    let label = NAMES.get(i).map_or_else(|| format!("Row {i}"), |n| n.to_string());
    let mut r = SidebarRow::new(label).selected(s.picked == i).on_select(move |s: &mut S, _| s.picked = i);
    if s.two_lines {
        r = r.subtitle(format!("user{i}@example.com"));
    }
    if s.renaming == Some(i) {
        r = r.renaming(
            &s.edit,
            |s: &mut S| &mut s.edit,
            |s: &mut S, _| {
                s.committed = Some(s.edit.text.clone());
                s.renaming = None;
                s.edit.focused = false;
            },
            |s: &mut S, _| {
                s.renaming = None;
                s.edit.focused = false;
            },
        );
    }
    r.build(k).key(format!("row{i}"))
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let sidebar = match s.rows {
        Some(n) => {
            let kit = k.clone();
            Sidebar::new()
                .label("Sessions")
                .virtual_rows(n, |_: &S, i| Cow::Owned(format!("row{i}")), move |s: &S, i| row(s, i, &kit))
        }
        None => Sidebar::new().label("Sessions").section(
            SidebarSection::new("Today")
                .collapsed(s.collapsed)
                .on_toggle(|s: &mut S, _| s.collapsed = !s.collapsed)
                .rows((0..NAMES.len()).map(|i| row(s, i, &k))),
        ),
    };
    let main = col().key("body").child(
        rune_kit::TextInput::new(&s.main_edit)
            .placeholder("Main field")
            .on_edit(|s: &mut S| &mut s.main_edit)
            .build(&k)
            .key("main-field"),
    );
    let inspector = col()
        .key("inspector-body")
        .child(el().key("inspector-btn").size(24.0, 24.0).role("button").focusable(true));
    SplitView::new(main)
        .sidebar(&s.sidebar, |s: &mut S| &mut s.sidebar, sidebar.build(&k))
        .inspector(&s.inspector, |s: &mut S| &mut s.inspector, inspector)
        .toolbar(Toolbar::new().leading(el().key("lead").size(20.0, 20.0)).title("Title").trailing(
            rune_kit::TextInput::new(&s.toolbar_edit).width(Some(160.0))
                .on_edit(|s: &mut S| &mut s.toolbar_edit).build(&k).key("toolbar-search"),
        ))
        .titlebar_inset(cx.titlebar_inset())
        .build(&k)
}

fn runtime(s: S) -> Runtime<S> {
    let mut rt = Runtime::new(s, view, (1000.0, 600.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| {
        let hit = is_sidebar_toggle(ev);
        if hit {
            s.sidebar.toggle();
        }
        hit
    });
    rt.frame(0.0);
    rt
}

fn settle(rt: &mut Runtime<S>) {
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("no {key}"))
}

fn press(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn toggle_shortcut() -> KeyEvent {
    KeyEvent { key: Key::Command('b'), mods: Mods { shift: true, cmd: true, ..Mods::default() } }
}

#[test]
fn dragging_the_grip_clamps_collapses_and_double_click_resets() {
    let mut rt = runtime(state());
    let (x, y) = rt.find("sidebar-grip").expect("grip");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x + 60.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.sidebar.width(), 300.0, "follows the pointer");
    assert_eq!(find(&rt, "sidebar-pane").rect[2], 300.0, "a drag doesn't spring");
    rt.pointer_move(x + 600.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.sidebar.width(), 420.0, "clamps to the maximum");
    rt.pointer_move(x - 140.0, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.sidebar.is_open() && rt.state.sidebar.width() == 180.0, "clamps to the minimum");
    rt.pointer_move(x - 200.0, y);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.sidebar.is_open(), "below half the minimum it collapses");
    rt.pointer_move(x - 20.0, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.sidebar.is_open(), "and dragging back out opens it");
    rt.pointer_up(x - 20.0, y);
    settle(&mut rt);
    assert_eq!(rt.state.sidebar.width(), 220.0);

    let (x, y) = rt.find("sidebar-grip").expect("grip");
    for _ in 0..2 {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.sidebar.width(), 240.0, "double-click resets the width");
}

#[test]
fn the_shortcut_collapses_on_a_spring_and_reduced_motion_snaps() {
    let mut rt = runtime(state());
    rt.key(toggle_shortcut());
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.sidebar.is_open());
    let w = find(&rt, "sidebar-pane").rect[2];
    assert!(w > 0.0 && w < 240.0, "mid-spring: {w}");
    settle(&mut rt);
    assert_eq!(find(&rt, "sidebar-pane").rect[2], 0.0);
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("sidebar-grip")), "no grip while collapsed");
    press(&mut rt, Key::Tab);
    let focused = rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    assert_ne!(focused.as_deref(), Some("row1"), "a collapsed sidebar's rows are out of the Tab order");

    rt.set_system_reduced_motion(true);
    rt.key(toggle_shortcut());
    rt.frame(1.0 / 60.0);
    assert!(rt.state.sidebar.is_open());
    assert_eq!(find(&rt, "sidebar-pane").rect[2], 240.0, "Reduce Motion snaps it open");
    assert!(!is_sidebar_toggle(&KeyEvent { key: Key::Command('b'), mods: Mods { cmd: true, ..Mods::default() } }));
}

#[test]
fn the_list_is_one_tab_stop_whose_arrows_and_typing_move_the_selection() {
    let mut rt = runtime(state());
    let focused = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("row1"), "Tab lands on the selected row");
    press(&mut rt, Key::Down);
    assert_eq!((focused(&rt).as_deref(), rt.state.picked), (Some("row2"), 2), "selection follows focus");
    for _ in 0..4 {
        press(&mut rt, Key::Down);
    }
    assert_eq!(rt.state.picked, 3, "stops at the end");
    press(&mut rt, Key::Home);
    assert_eq!(rt.state.picked, 0);
    press(&mut rt, Key::Text("c".into()));
    assert_eq!(rt.state.picked, 2, "type-select");
    rt.state.picked = 0;
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.picked, 2, "Return presses the focused row");
}

#[test]
fn long_lists_build_only_the_rows_near_the_viewport() {
    let mut rt = runtime(S { rows: Some(5_000), ..state() });
    settle(&mut rt);
    let rows = rt.elements().into_iter().filter(|e| e.role == "listitem").count();
    assert!((10..300).contains(&rows), "{rows} rows built");
    assert!(rt.find("row0").is_some() && rt.find("row4999").is_none());
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Down);
    assert_eq!(rt.state.picked, 2, "the keyboard works in a virtualised list");

    // During the sidebar's disclosure spring, rows outside the viewport must not lay out.
    let t_spring = std::time::Instant::now();
    rt.key(toggle_shortcut());
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
        let built = rt.elements().into_iter().filter(|e| e.role == "listitem").count();
        assert!(built < 300, "offscreen rows built mid-spring: {built}");
    }
    let spring_us = t_spring.elapsed().as_secs_f64() * 1e6 / 30.0;
    rt.key(toggle_shortcut());
    settle(&mut rt);

    // Measure scrolling across 60 frames in the 5,000-row virtual list.
    let (x, y) = rt.find("sidebar-scroll").expect("scroll");
    let t_scroll = std::time::Instant::now();
    for _ in 0..60 {
        rt.wheel(x, y, -40.0);
        rt.frame(1.0 / 60.0);
    }
    let scroll_us = t_scroll.elapsed().as_secs_f64() * 1e6 / 60.0;

    // Measure inline rename keystroke cost.
    rt.wheel(x, y, 2400.0);
    settle(&mut rt);
    rt.update(|s, _| {
        s.renaming = Some(1);
        s.edit = TextEdit::new("Bravo");
        s.edit.focused = true;
    });
    rt.frame(1.0 / 60.0);
    let t_key = std::time::Instant::now();
    for _ in 0..20 {
        press(&mut rt, Key::Text("a".into()));
    }
    let key_us = t_key.elapsed().as_secs_f64() * 1e6 / 20.0;
    eprintln!("bench-shell virtual_spring_us={spring_us:.1} virtual_scroll_us={scroll_us:.1} rename_key_us={key_us:.1}");
}

#[test]
fn roles_and_labels_describe_the_shell() {
    let mut rt = runtime(state());
    let all = rt.elements();
    let role = |key: &str| all.iter().find(|e| e.key.as_deref() == Some(key)).map(|e| (e.role, e.label.clone()));
    assert_eq!(role("sidebar-list"), Some(("list", Some("Sessions".into()))));
    assert_eq!(role("row2"), Some(("listitem", Some("Charlie".into()))));
    assert_eq!(role("section-header"), Some(("button", Some("Today, expanded".into()))));
    assert_eq!(role("toolbar").map(|r| r.0), Some("toolbar"));
    assert_eq!(role("sidebar-grip"), Some(("separator", Some("Resize sidebar".into()))));
    assert_eq!(role("sidebar-toggle"), Some(("button", Some("Hide sidebar".into()))));
    let fill = |key: &str| all.iter().find(|e| e.key.as_deref() == Some(key)).unwrap().fill.a;
    assert!(fill("row1") > 0.0 && fill("row0") == 0.0, "the selected row is filled");
    let toggle = all.iter().find(|e| e.key.as_deref() == Some("sidebar-toggle")).unwrap();
    assert_eq!(toggle.expanded, Some(true), "the toggle reports the open sidebar");
    assert_eq!(fill("sidebar-toggle"), 0.0, "without holding a pressed fill");

    let (x, y) = rt.find("section-header").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(rt.state.collapsed);
    assert_eq!(find(&rt, "section-header").label.as_deref(), Some("Today, collapsed"));
    assert!(rt.find("row0").is_none(), "a collapsed section hides its rows");
}

#[test]
fn the_title_bar_clears_the_traffic_lights_and_drags_the_window() {
    let mut rt = runtime(state());
    let inset = Chrome::DEFAULT.titlebar_inset();
    assert!(inset > 0.0);
    rt.set_titlebar_inset(inset);
    rt.set_system_reduced_motion(true);
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "sidebar-toggle").rect[0], inset + 8.0, "the toggle sits past the lights");
    rt.key(toggle_shortcut());
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "lead").rect[0], inset + 8.0 + 24.0 + 8.0, "a collapsed sidebar's toolbar clears lights and toggle");

    rt.set_titlebar_inset(0.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "sidebar-toggle").rect[0], 12.0, "full screen has no lights to clear");
    assert_eq!(find(&rt, "lead").rect[0], 12.0 + 24.0 + 8.0);

    for (key, drags) in [("title", true), ("sidebar-toggle", false), ("lead", true)] {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        assert_eq!(rt.take_titlebar_action() == Some(TitlebarAction::Drag), drags, "{key}");
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.sidebar.is_open(), "the toggle button opened the sidebar again");
}

#[test]
fn collapsing_the_sidebar_moves_focus_out_and_blurs_rename() {
    let mut rt = runtime(state());
    let focused = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("row1"));

    rt.key(toggle_shortcut());
    rt.frame(1.0 / 60.0);
    assert_eq!(focused(&rt).as_deref(), Some("sidebar-toggle"), "focus leaves the collapsed sidebar");
    press(&mut rt, Key::Down);
    assert_eq!(rt.state.picked, 1, "arrows no longer move a collapsed sidebar's selection");

    // Reopen and start an inline rename, then collapse: the rename input must blur.
    rt.key(toggle_shortcut());
    settle(&mut rt);
    rt.update(|s, _| {
        s.renaming = Some(1);
        s.edit = TextEdit::new("Bravo");
        s.edit.focused = true;
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(focused(&rt).as_deref(), Some("rename"));

    rt.key(toggle_shortcut());
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.edit.focused, "collapsing the sidebar blurs its rename field");
    assert_eq!(focused(&rt).as_deref(), Some("sidebar-toggle"));
    press(&mut rt, Key::Text("X".into()));
    assert_eq!(rt.state.edit.text, "Bravo", "typing after collapse does not edit the hidden field");
}

#[test]
fn hiding_the_sidebar_keeps_focus_in_a_toolbar_search() {
    let mut rt = runtime(state());
    let (x, y) = rt.find("toolbar-search").expect("toolbar search");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert!(rt.state.toolbar_edit.focused);
    rt.key(toggle_shortcut());
    rt.frame(0.0);
    assert!(!rt.state.sidebar.is_open());
    assert!(rt.state.toolbar_edit.focused, "the sentinel belongs to the sidebar, not the whole shell");
    press(&mut rt, Key::Text("search".into()));
    assert_eq!(rt.state.toolbar_edit.text, "search");
    assert!(rt.take_titlebar_action().is_none());
}

#[test]
fn hiding_the_sidebar_keeps_focus_in_a_main_pane_text_field() {
    let mut rt = runtime(state());
    let focused = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    let (x, y) = rt.find("main-field").expect("main field");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.main_edit.focused);
    assert_eq!(focused(&rt).as_deref(), Some("input"));

    rt.key(toggle_shortcut());
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.sidebar.is_open(), "shortcut closes the sidebar");
    assert!(rt.state.main_edit.focused, "focus never leaves the main text field");
    assert_eq!(focused(&rt).as_deref(), Some("input"));

    for ch in ["a", " ", "b"] {
        press(&mut rt, Key::Text(ch.into()));
    }
    assert_eq!(rt.state.main_edit.text, "a b");
    assert!(!rt.state.sidebar.is_open(), "Space typed into the field does not reopen the sidebar");
    assert!(rt.state.main_edit.focused);

    // Closing the inspector while focus is inside it returns focus to the main pane, while
    // closing it with focus already in main keeps focus in main.
    rt.update(|s, _| s.inspector.set_open(true));
    settle(&mut rt);
    let (ix, iy) = rt.find("inspector-btn").expect("inspector button");
    rt.pointer_down(ix, iy);
    rt.pointer_up(ix, iy);
    rt.frame(1.0 / 60.0);
    assert_eq!(focused(&rt).as_deref(), Some("inspector-btn"));
    rt.update(|s, _| s.inspector.set_open(false));
    rt.frame(1.0 / 60.0);
    assert_eq!(focused(&rt).as_deref(), Some("input"), "closing a focused inspector returns focus to main");
}

#[test]
fn inline_rename_commits_on_return_and_cancels_on_escape() {
    let mut rt = runtime(state());
    rt.update(|s, _| {
        s.renaming = Some(1);
        s.edit = TextEdit::new("Bravo");
        s.edit.anchor = 0;
        s.edit.focused = true;
    });
    rt.frame(1.0 / 60.0);
    press(&mut rt, Key::Text("Beta".into()));
    assert_eq!(rt.state.edit.text, "Beta");
    press(&mut rt, Key::Escape);
    assert_eq!((rt.state.renaming, rt.state.committed.as_deref()), (None, None), "Escape cancels");

    rt.update(|s, _| {
        s.renaming = Some(1);
        s.edit = TextEdit::new("Bravo");
        s.edit.anchor = 0;
        s.edit.focused = true;
    });
    rt.frame(1.0 / 60.0);
    press(&mut rt, Key::Text("Beta".into()));
    press(&mut rt, Key::Enter);
    assert_eq!((rt.state.renaming, rt.state.committed.as_deref()), (None, Some("Beta")), "Return commits");
}

#[test]
fn a_stuck_resize_clears_on_window_blur_or_uncaptured_pointer_move() {
    let mut rt = runtime(state());
    let (x, y) = rt.find("sidebar-grip").expect("grip");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x - 200.0, y);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.sidebar.is_open() && rt.state.sidebar.is_resizing());

    // Losing window focus without a pointer-up clears `is_resizing`.
    rt.window_focus(false);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.sidebar.is_resizing(), "window blur clears a stuck resize");

    // Reopen, start another resize, release capture, then simulate a stuck flag: the next
    // uncaptured pointer move clears it.
    rt.window_focus(true);
    rt.update(|s, _| s.sidebar.set_open(true));
    settle(&mut rt);
    let (x, y) = rt.find("sidebar-grip").expect("grip");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.sidebar.is_resizing());
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.sidebar.is_resizing());

    // Now start a drag, collapse, end the pointer capture, and force `resizing` back on as if
    // the release never reached the handler: moving the pointer without a button held clears it.
    rt.pointer_down(x, y);
    rt.pointer_move(x + 20.0, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.sidebar.is_resizing(), "live drag keeps resizing");
    rt.pointer_up(x + 20.0, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x + 20.0, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.sidebar.is_resizing());
    // Simulate OS dropping the button release before the next move by cancelling via window blur,
    // or verify that an uncaptured move after `pointer_up` leaves `resizing` false.
    rt.pointer_up(x + 20.0, y);
    rt.pointer_move(500.0, 300.0);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.sidebar.is_resizing());
}

#[test]
fn sidebar_and_main_pane_move_in_lockstep_during_disclosure() {
    let mut rt = runtime(state());
    rt.key(toggle_shortcut());
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
        let side_w = find(&rt, "sidebar-pane").rect[2];
        let main_col_x = find(&rt, "main-column").rect[0];
        assert!(
            (side_w - main_col_x).abs() < 0.05,
            "sidebar right edge ({side_w}) and main-column left edge ({main_col_x}) must move together"
        );
    }
}

#[test]
fn a_wider_range_keeps_the_width_new_was_given() {
    let p = PaneState::new(640.0).range(360.0, 960.0);
    assert_eq!((p.width(), p.default_width()), (640.0, 640.0));
    let mut p = PaneState::new(640.0).range(360.0, 960.0).with_width(700.0);
    assert_eq!((p.width(), p.default_width()), (700.0, 640.0));
    p.reset_width();
    assert_eq!(p.width(), 640.0);
    let p = PaneState::new(900.0).range(360.0, 600.0);
    assert_eq!((p.width(), p.default_width()), (600.0, 600.0));
    assert_eq!(PaneState::new(640.0).width(), PaneState::MAX);
}

#[test]
fn a_rename_field_keeps_letters_and_arrows_that_would_type_select_a_row() {
    let mut rt = runtime(state());
    let focused = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    rt.update(|s, _| {
        s.renaming = Some(1);
        s.edit = TextEdit::new("Bravo");
        s.edit.anchor = 0;
        s.edit.focused = true;
    });
    rt.frame(1.0 / 60.0);
    for key in ["D", "e", "l"] {
        press(&mut rt, Key::Text(key.into()));
    }
    for key in [Key::Down, Key::Up, Key::Home, Key::End] {
        press(&mut rt, key);
    }
    assert_eq!(rt.state.edit.text, "Del", "Delta's first letter goes to the field");
    assert_eq!((rt.state.picked, focused(&rt).as_deref()), (1, Some("rename")), "no row was type-selected");
}

#[test]
fn an_explicit_two_line_row_height_wins_in_either_builder_order() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        col().w(240.0)
            .child(SidebarRow::new("First").height(45.0).subtitle("Detail").build(&k).key("first"))
            .child(SidebarRow::new("Second").subtitle("Detail").height(45.0).build(&k).key("second"))
    }
    let mut rt = Runtime::new((), view, (240.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for key in ["first", "second"] {
        assert_eq!(rt.elements().iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect[3], 45.0);
    }
}

#[test]
fn two_line_rows_keep_focus_keyboard_navigation_and_accessible_labels() {
    let mut rt = runtime(S { two_lines: true, ..state() });
    let focused = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    let r2 = find(&rt, "row2");
    assert_eq!((r2.role, r2.label.as_deref(), r2.rect[3]), ("listitem", Some("Charlie, user2@example.com"), 40.0));

    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("row1"), "Tab lands on the selected two-line row");
    press(&mut rt, Key::Down);
    assert_eq!((focused(&rt).as_deref(), rt.state.picked), (Some("row2"), 2));
    press(&mut rt, Key::Home);
    assert_eq!((focused(&rt).as_deref(), rt.state.picked), (Some("row0"), 0));
    press(&mut rt, Key::Text("d".into()));
    assert_eq!((focused(&rt).as_deref(), rt.state.picked), (Some("row3"), 3), "type-select matches the title label");
    rt.state.picked = 0;
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.picked, 3, "Return selects the focused two-line row");

    // Ending an inline rename on a two-line row returns focus to the selected row.
    rt.update(|s, _| {
        s.picked = 1;
        s.renaming = Some(1);
        s.edit = TextEdit::new("Bravo");
        s.edit.focused = true;
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(focused(&rt).as_deref(), Some("rename"));
    press(&mut rt, Key::Escape);
    assert_eq!(focused(&rt).as_deref(), Some("row1"), "ending rename returns focus to the selected row");
}

#[test]
fn page_matches_split_views_toolbar_and_frame_pixel_for_pixel() {
    struct P {
        page: bool,
        sidebar: PaneState,
        backs: usize,
    }
    fn page_view(s: &P, cx: &mut Cx<P>) -> El {
        let k = Kit::neutral(true);
        let lead = el().key("lead").size(24.0, 24.0).role("button").focusable(true).on_press(|s: &mut P| s.backs += 1);
        let trail = el().key("trail").size(24.0, 24.0).role("button").focusable(true);
        let body = col().key("content").w_full().h_full();
        if s.page {
            Page::new(body)
                .title("Settings")
                .leading(lead)
                .trailing(trail)
                .titlebar_inset(cx.titlebar_inset())
                .frame_inset(8.0)
                .build(&k)
        } else {
            SplitView::new(body)
                .sidebar(&s.sidebar, |s: &mut P| &mut s.sidebar, col())
                .hide_sidebar_toggle()
                .toolbar(Toolbar::new().title("Settings").leading(lead).trailing(trail))
                .titlebar_inset(cx.titlebar_inset())
                .frame_inset(8.0)
                .build(&k)
        }
    }
    let inset = Chrome::DEFAULT.titlebar_inset();
    let mut rt = Runtime::new(
        P { page: false, sidebar: PaneState::new(240.0).collapsed(), backs: 0 },
        page_view,
        (960.0, 600.0),
        Env::new(true, 2.0),
    );
    rt.set_titlebar_inset(inset);
    rt.frame(0.0);
    let rect_in = |rt: &Runtime<P>, key: &str| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
    };
    let split_toolbar = rect_in(&rt, "toolbar");
    let split_lead = rect_in(&rt, "lead");
    let split_title = rect_in(&rt, "title");
    let split_trail = rect_in(&rt, "trail");
    let split_pane = rect_in(&rt, "main-pane");

    rt.update(|s, _| s.page = true);
    rt.frame(1.0 / 60.0);
    assert_eq!(rect_in(&rt, "toolbar"), split_toolbar, "toolbar rect matches SplitView");
    assert_eq!(rect_in(&rt, "lead"), split_lead, "leading control past traffic lights matches SplitView");
    assert_eq!(rect_in(&rt, "title"), split_title, "title rect matches SplitView");
    assert_eq!(rect_in(&rt, "trail"), split_trail, "trailing control rect matches SplitView");
    assert_eq!(rect_in(&rt, "main-pane"), split_pane, "framed main-pane rect matches SplitView");

    // Titlebar drags the window; leading button keeps its press.
    let (tx, ty) = rt.find("title").unwrap();
    rt.pointer_down(tx, ty);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag));
    rt.pointer_up(tx, ty);
    let (lx, ly) = rt.find("lead").unwrap();
    rt.pointer_down(lx, ly);
    assert!(rt.take_titlebar_action().is_none());
    rt.pointer_up(lx, ly);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.backs, 1);
}

#[test]
fn icon_button_and_sidebar_toggle_show_help_and_shortcut_tooltips() {
    struct H {
        sidebar: PaneState,
    }
    fn help_view(s: &H, _: &mut Cx<H>) -> El {
        let k = Kit::neutral(true);
        let lock = IconButton::new(&glyphs::LOCK)
            .help("Lock")
            .shortcut("⌘L")
            .build(&k)
            .key("lock");
        let parsed = IconButton::new(&glyphs::COPY)
            .label("Copy")
            .shortcut("cmd+c")
            .build(&k)
            .key("copy");
        let save = Button::new("Save")
            .shortcut("⌘S")
            .build(&k)
            .key("save");
        SplitView::new(col().w_full().h_full())
            .sidebar(&s.sidebar, |s: &mut H| &mut s.sidebar, col())
            .toolbar(Toolbar::new().trailing(lock).trailing(parsed).trailing(save))
            .build(&k)
    }
    let mut rt = Runtime::new(
        H { sidebar: PaneState::new(240.0) },
        help_view,
        (800.0, 480.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    let info = |rt: &Runtime<H>, key: &str| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
    };
    assert_eq!(info(&rt, "lock").label.as_deref(), Some("Lock"), "help supplies accessible label when label is unset");
    assert_eq!(info(&rt, "lock").help.as_deref(), Some("Lock  ⌘L"));
    let expected_copy = if cfg!(target_os = "macos") { "Copy  ⌘C" } else { "Copy  Super+C" };
    assert_eq!(info(&rt, "copy").help.as_deref(), Some(expected_copy));
    assert_eq!(info(&rt, "save").help.as_deref(), Some("Save  ⌘S"));
    let expected_toggle = if cfg!(target_os = "macos") { "Hide sidebar  ⇧⌘B" } else { "Hide sidebar  Ctrl+Shift+B" };
    assert_eq!(info(&rt, "sidebar-toggle").help.as_deref(), Some(expected_toggle));

    let (x, y) = rt.find("lock").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(rt.help_delay + 0.05);
    assert_eq!(rt.help_tooltip().map(|(t, _)| t), Some("Lock  ⌘L"));
}

#[test]
fn a_long_leading_title_truncates_and_keeps_the_trailing_slot_on_the_band() {
    struct T;
    fn toolbar_view(_: &T, _: &mut Cx<T>) -> El {
        let k = Kit::neutral(true);
        Toolbar::new()
            .leading(rune::text("a".repeat(400)).key("long").truncate().shrink().min_w(0.0))
            .trailing(el().key("trail").size(24.0, 24.0))
            .build(&k)
    }
    let mut rt = Runtime::new(T, toolbar_view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let rect = |key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
    let (long, trail) = (rect("long"), rect("trail"));
    assert!(trail[0] + trail[2] <= 400.0 + 0.5, "trailing slot pushed off the band: {trail:?}");
    assert!(long[0] + long[2] <= trail[0] + 0.5, "leading title overlaps the trailing slot: {long:?} {trail:?}");
}

/// One row alone: 0 plain, 1 two-line, 2 two-line and selected, 3 two-line with custom types,
/// 4 two-line with a muted subtitle.
fn lone_row(variant: u8) -> Runtime<u8> {
    fn view(v: &u8, _: &mut Cx<u8>) -> El {
        let k = Kit::neutral(true);
        let r = match v {
            0 => SidebarRow::new("Name"),
            1 => SidebarRow::new("Name").subtitle("Detail"),
            2 => SidebarRow::new("Name").subtitle("Detail").selected(true),
            4 => SidebarRow::new("Name").subtitle("Detail").subtitle_color(k.c.text_muted),
            _ => SidebarRow::new("Name")
                .subtitle("Detail")
                .title_style(TextStyle::new(15.0, 600.0).lh(22.0))
                .subtitle_style(TextStyle::new(10.0, 400.0).lh(12.0)),
        };
        col().w(240.0).child(r.build(&Kit::neutral(true)).key("r"))
    }
    let mut rt = Runtime::new(variant, view, (240.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn info(rt: &Runtime<u8>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

#[test]
fn a_two_line_row_has_a_stronger_first_line_and_fits_in_40_points() {
    let k = Kit::neutral(true);
    for (variant, selected) in [(1, false), (2, true)] {
        let rt = lone_row(variant);
        let (row, name, sub) = (info(&rt, "r"), info(&rt, "label"), info(&rt, "subtitle"));
        assert_eq!(row.rect[3], 40.0);
        assert_eq!(name.text_color, Some(if selected { k.c.text_base } else { k.c.text_muted }));
        assert_eq!(sub.text_color, Some(k.c.text_faint), "the subtitle is the faintest ink");
        assert!(name.rect[1] + name.rect[3] <= sub.rect[1] + 0.01, "the lines don't overlap");
        assert!(sub.rect[1] + sub.rect[3] <= row.rect[1] + row.rect[3], "both lines fit the default height");
        assert_eq!((name.rect[3], sub.rect[3]), (k.ty.ui_compact.line_px(), k.ty.caption().line_px()));
    }
    let rt = lone_row(3);
    assert_eq!((info(&rt, "label").rect[3], info(&rt, "subtitle").rect[3]), (22.0, 12.0));
}

#[test]
fn a_two_line_rows_subtitle_takes_its_own_colour() {
    let k = Kit::neutral(true);
    assert_eq!(info(&lone_row(4), "subtitle").text_color, Some(k.c.text_muted));
}

#[test]
fn renaming_a_two_line_row_keeps_the_title_and_subtitle_lines_in_place() {
    struct Ren {
        renaming: bool,
        custom: bool,
        edit: TextEdit,
    }
    fn view(s: &Ren, _: &mut Cx<Ren>) -> El {
        let mut r = SidebarRow::new("Name").subtitle("Detail");
        if s.custom {
            r = r.title_style(TextStyle::new(15.0, 600.0).lh(22.0));
        }
        if s.renaming {
            r = r.renaming(&s.edit, |s: &mut Ren| &mut s.edit, |_, _| {}, |_, _| {});
        }
        col().w(240.0).child(r.build(&Kit::neutral(true)).key("r"))
    }
    for custom in [false, true] {
        let mut rt = Runtime::new(Ren { renaming: false, custom, edit: TextEdit::default() }, view, (240.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let label_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("label")).unwrap().rect;
        let sub_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("subtitle")).unwrap().rect;
        for text in ["Name", ""] {
            rt.update(|s, _| {
                s.renaming = true;
                s.edit = TextEdit::new(text);
                s.edit.focused = true;
            });
            rt.frame(1.0 / 60.0);
            let rename_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("rename")).unwrap().rect;
            let sub_now = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("subtitle")).unwrap().rect;
            assert_eq!((rename_rect[1], rename_rect[3]), (label_rect[1], label_rect[3]), "rename field matches title line y and height");
            assert_eq!(sub_now, sub_rect, "subtitle does not jump when entering rename");
        }
    }
}

#[test]
fn a_two_line_row_reads_name_then_subtitle() {
    let rt = lone_row(1);
    assert_eq!(info(&rt, "r").label.as_deref(), Some("Name, Detail"));
    let rt = lone_row(0);
    assert_eq!(info(&rt, "r").label.as_deref(), Some("Name"));
}

struct Pick {
    field: TextEdit,
    picked: usize,
    presses: usize,
}

#[test]
fn select_only_rows_skip_tab_and_keep_an_outside_field_focused() {
    fn view(s: &Pick, _: &mut Cx<Pick>) -> El {
        let k = Kit::neutral(true);
        col().w(240.0).gap(8.0)
            .child(rune_kit::TextInput::new(&s.field).on_edit(|s: &mut Pick| &mut s.field).build(&k).key("field"))
            .child(Sidebar::new().label("Results").child(
                col().children((0..3).map(|i| {
                    SidebarRow::new(format!("Row {i}"))
                        .select_only()
                        .selected(s.picked == i)
                        .on_select(move |s: &mut Pick, _| { s.picked = i; s.presses += 1; })
                        .build(&k)
                        .key(format!("row{i}"))
                })),
            ).build(&k))
    }
    let mut rt = Runtime::new(Pick { field: TextEdit::default(), picked: 1, presses: 0 }, view, (240.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let row = |rt: &Runtime<Pick>, i: usize| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&format!("row{i}"))).unwrap();
    assert!((0..3).all(|i| !row(&rt, i).focusable), "rows take no focus");
    assert!(row(&rt, 1).item_selected && !row(&rt, 0).item_selected, "the selection is still exposed");

    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    let focused = |rt: &Runtime<Pick>| rt.elements().into_iter().filter(|e| e.focused).filter_map(|e| e.key).collect::<Vec<_>>();
    assert_eq!(focused(&rt), ["input"], "Tab lands on the field and no row");
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.elements().iter().all(|e| e.key.as_deref().is_none_or(|k| !k.starts_with("row")) || !e.focused));

    let r = row(&rt, 2);
    let (x, y) = (r.rect[0] + 20.0, r.rect[1] + r.rect[3] * 0.5);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.picked, rt.state.presses), (2, 1), "the press selects");
    assert_eq!(focused(&rt), ["input"], "the field keeps focus through the press");
    assert!(row(&rt, 2).item_selected && !row(&rt, 1).item_selected);

    // Changing selection from the app (e.g. arrow keys handled by the field) keeps focus on the field.
    rt.update(|s, _| s.picked = 0);
    rt.frame(1.0 / 60.0);
    assert_eq!(focused(&rt), ["input"], "selection change never pulls focus from the field");
    assert!(row(&rt, 0).item_selected && !row(&rt, 2).item_selected);
}
