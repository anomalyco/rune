//! Verifies that `List::on_search_edit` and every widened `rune-kit` builder accept
//! function items, function-pointer variables, non-capturing closures, and capturing closures.

use rune::{Env, Key, KeyEvent, Runtime, TextEdit, el};
use rune_kit::shell::{TabDragState, TabItem, TabOrientation};
use rune_kit::{
    Canvas, FileTree, ImageZoom, Kit, List, ListItem, PaneState, Preview, Reorder, Sidebar,
    SidebarRow, SplitView, Stage, StageZoom, TabStrip, Tree, TreeModel, ZoomPan,
};

struct Panel {
    search: TextEdit,
    rename: TextEdit,
    reorder: Reorder,
    sidebar: PaneState,
    inspector: PaneState,
    tabs: TabDragState,
    tree: TreeModel<rune_kit::FileNode<()>>,
    preview: ImageZoom,
    stage: StageZoom,
    canvas: ZoomPan,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            search: TextEdit::new(""),
            rename: TextEdit::new("init"),
            reorder: Reorder::new(),
            sidebar: PaneState::new(220.0),
            inspector: PaneState::new(240.0),
            tabs: TabDragState::new(),
            tree: FileTree::new().build([("alpha.rs", ())]),
            preview: ImageZoom::new(),
            stage: StageZoom::default(),
            canvas: ZoomPan::new(),
        }
    }
}

struct App {
    active_panel: usize,
    panels: Vec<Panel>,
}

fn panel0_search(s: &mut App) -> &mut TextEdit {
    &mut s.panels[0].search
}

fn node_label(n: &rune_kit::FileNode<()>) -> &str {
    n.label()
}

#[test]
fn list_on_search_edit_accepts_fn_item_fn_ptr_non_capturing_and_capturing_closures() {
    let k = Kit::neutral(true);
    let app = App { active_panel: 1, panels: vec![Panel::default(), Panel::default()] };

    // 1. Plain function item.
    let _ = List::new().search(&app.panels[0].search, "Find").on_search_edit(panel0_search).build(&k);

    // 2. Function pointer in a variable.
    let ptr: fn(&mut App) -> &mut TextEdit = panel0_search;
    let _ = List::new().search(&app.panels[0].search, "Find").on_search_edit(ptr).build(&k);

    // 3. Non-capturing closure.
    let _ = List::new()
        .search(&app.panels[0].search, "Find")
        .on_search_edit(|s: &mut App| &mut s.panels[0].search)
        .build(&k);

    // 4. Capturing closure (reporter's multi-panel example), verified end-to-end with typing.
    let mut rt = Runtime::new(
        App {
            active_panel: 1,
            panels: vec![
                Panel::default(),
                Panel { search: { let mut e = TextEdit::new(""); e.focused = true; e }, ..Panel::default() },
            ],
        },
        |s: &App, _| {
            let k = Kit::neutral(true);
            let id = s.active_panel;
            List::new()
                .search(&s.panels[id].search, "Find")
                .on_search_edit(move |s: &mut App| &mut s.panels[id].search)
                .item(ListItem::new("One"))
                .build(&k)
        },
        (320.0, 240.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::Text("a".into())));
    rt.key(KeyEvent::from(Key::Text("b".into())));
    rt.frame(0.0);
    assert_eq!(rt.state.panels[0].search.text, "");
    assert_eq!(rt.state.panels[1].search.text, "ab");
}

#[test]
fn swept_kit_builders_accept_capturing_closures_and_fn_pointers() {
    let k = Kit::neutral(true);
    let id = 1usize;
    let app = App { active_panel: 1, panels: vec![Panel::default(), Panel::default()] };
    let prefix = "a".to_owned();

    // SidebarRow::renaming + reorder + reorder_tx with capturing panel id.
    let _ = Sidebar::new()
        .child(
                    SidebarRow::new("Alpha")
                        .reorder(
                            &app.panels[id].reorder,
                            0,
                            1,
                            move |s: &mut App| &mut s.panels[id].reorder,
                            move |_s: &mut App, _from, _to| { let _ = id; },
                        )
                        .renaming(
                            &app.panels[id].rename,
                            move |s: &mut App| &mut s.panels[id].rename,
                            move |_s: &mut App, _| { let _ = id; },
                            move |_s: &mut App, _| { let _ = id; },
                        )
                        .build(&k),
        )
        .child(
            SidebarRow::new("Beta")
                        .reorder_tx(
                            &app.panels[id].reorder,
                            0,
                            1,
                            move |s: &mut App| &mut s.panels[id].reorder,
                            move |_s: &mut App, _from, _to, _| { let _ = id; },
                        )
                .build(&k),
        )
        .build(&k);

    // SplitView::sidebar and inspector with capturing panel id.
    let _ = SplitView::new(el())
        .sidebar(&app.panels[id].sidebar, move |s: &mut App| &mut s.panels[id].sidebar, el())
        .inspector(&app.panels[id].inspector, move |s: &mut App| &mut s.panels[id].inspector, el())
        .build(&k);

    // TabStrip::drag_state with capturing panel id.
    let _ = TabStrip::new("tabs", TabOrientation::Horizontal)
        .tabs([TabItem::new("t0", "Tab 0")])
        .drag_state(&app.panels[id].tabs, move |s: &mut App| &mut s.panels[id].tabs)
        .build(&k);

    // Tree::new with capturing get, get_mut, and label, plus fn item compatibility.
    let _ = Tree::new(
        move |s: &App| &s.panels[id].tree,
        move |s: &mut App| &mut s.panels[id].tree,
        move |n: &rune_kit::FileNode<()>| n.label().strip_prefix(&prefix).unwrap_or_else(|| n.label()),
    )
    .build(&app, &k);
    let _ = Tree::new(|s: &App| &s.panels[0].tree, |s: &mut App| &mut s.panels[0].tree, node_label).build(&app, &k);

    // Preview::new, Stage::new, Canvas::new, Canvas::scaled with capturing panel id.
    let _ = Preview::new(move |s: &mut App| &mut s.panels[id].preview, [100.0, 80.0], el())
        .build(&app.panels[id].preview, &k, [400.0, 300.0], true);
    let _ = Stage::new(move |s: &mut App| &mut s.panels[id].stage, [100.0, 80.0], el())
        .build(&app.panels[id].stage, &k, [400.0, 300.0]);
    let _ = Canvas::new(move |s: &mut App| &mut s.panels[id].canvas, [200.0, 150.0], el())
        .build(&app.panels[id].canvas, [200.0, 150.0]);
    let _ = Canvas::scaled(move |s: &mut App| &mut s.panels[id].canvas, [200.0, 150.0], |_| el())
        .build(&app.panels[id].canvas, [200.0, 150.0]);

    // Verify SidebarRow::renaming edits the captured panel at runtime.
    let mut rt = Runtime::new(
        App {
            active_panel: 1,
            panels: vec![
                Panel::default(),
                Panel { rename: { let mut e = TextEdit::new(""); e.focused = true; e }, ..Panel::default() },
            ],
        },
        |s: &App, _| {
            let k = Kit::neutral(true);
            let id = s.active_panel;
            Sidebar::new()
                .child(
                    SidebarRow::new("Row")
                            .renaming(
                                &s.panels[id].rename,
                                move |s: &mut App| &mut s.panels[id].rename,
                                |_, _| {},
                                |_, _| {},
                            )
                            .build(&k),
                )
                .build(&k)
        },
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::Text("z".into())));
    rt.key(KeyEvent::from(Key::Enter));
    rt.frame(0.0);
    assert_eq!(rt.state.panels[0].rename.text, "init");
    assert_eq!(rt.state.panels[1].rename.text, "z");
}
