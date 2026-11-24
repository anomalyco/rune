use super::*;

fn model() -> TreeModel<&'static str> {
    let mut m = TreeModel::new();
    m.insert(None, "a", "alpha");
    m.insert(Some("a"), "a/1", "one");
    m.insert(Some("a"), "a/2", "two");
    m.insert(None, "b", "beta");
    m.insert(None, "c", "bravo");
    m
}

fn label<'a>(t: &'a &'static str) -> &'a str {
    t
}

/// Types `text` at `now`, as a key handler would, and jumps to the row it selects.
fn typed(m: &mut TreeModel<&'static str>, text: &str, now: f64) -> Option<usize> {
    let prefix = m.meta.type_ahead.step(text, now);
    m.type_select(&prefix, label)
}

#[test]
fn typing_jumps_by_prefix_and_a_repeated_letter_cycles() {
    let mut m = model();
    assert_eq!(typed(&mut m, "b", 0.0), Some(1), "beta: rows are alpha, beta, bravo");
    m.select("b");
    assert_eq!(typed(&mut m, "b", 0.1), Some(2), "b again cycles to bravo");
    m.select("c");
    assert_eq!(typed(&mut m, "r", 0.2), None, "\"bbr\" is no name");
    assert_eq!(typed(&mut m, "b", 2.0), Some(1), "after a pause, a new prefix from the next row");
    m.select("a");
    assert_eq!(typed(&mut m, "b", 3.5), Some(1));
    assert_eq!(typed(&mut m, "r", 3.6), Some(2), "\"br\" keeps looking past beta to bravo");
}

#[test]
fn natural_order_counts_digits_and_ignores_case() {
    let mut v = vec!["file10", "File2", "file1", "a", "B"];
    v.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(v, ["a", "B", "file1", "File2", "file10"]);
}

// on_menu & scroll_bar ---------------------------------------------------------------------

use rune::{Cx, Direction, El, Env, Key, KeyEvent, Mods, PointerPress, Runtime, ScrollBars, col, row};

use crate::{Kit, ScrollBarStyle};

struct MenuApp {
    files: TreeModel<&'static str>,
    menus: Vec<(String, PointerPress)>,
    selected: Vec<String>,
    inert: bool,
    with_menu: bool,
}

fn menu_tree_model() -> TreeModel<&'static str> {
    let mut m = TreeModel::new().key("files");
    m.insert(None, "a", "alpha");
    m.insert(Some("a"), "a/1", "one");
    m.insert(None, "b", "beta");
    m.insert(None, "c", "gamma");
    m
}

fn menu_view(s: &MenuApp, _: &mut Cx<MenuApp>) -> El {
    let k = Kit::neutral(true);
    let mut t = Tree::new(|s: &MenuApp| &s.files, |s: &mut MenuApp| &mut s.files, label)
        .name("Files")
        .on_select(|s: &mut MenuApp, id, _| s.selected.push(id.to_owned()));
    if s.with_menu {
        t = t.on_menu(|s: &mut MenuApp, id, p, _| s.menus.push((id.to_owned(), p)));
    }
    let tree = t.build(s, &k).h(240.0);
    col().size(300.0, 240.0).child(if s.inert { tree.inert() } else { tree })
}

fn row_rect<S>(rt: &Runtime<S>, id: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.role == "treeitem" && e.key.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("row {id:?} not found"))
        .rect
}

#[test]
fn on_menu_handles_secondary_click_shift_f10_and_context_menu_key() {
    let app = MenuApp { files: menu_tree_model(), menus: Vec::new(), selected: Vec::new(), inert: false, with_menu: true };
    let mut rt = Runtime::new(app, menu_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Secondary click on a row opens its menu at the pointer without changing selection.
    let rb = row_rect(&rt, "b");
    let (bx, by) = (rb[0] + 20.0, rb[1] + rb[3] * 0.5);
    rt.secondary_down(bx, by);
    rt.secondary_up(bx, by);
    rt.frame(0.0);
    assert_eq!(rt.state.menus.len(), 1);
    assert_eq!(rt.state.menus[0].0, "b");
    assert_eq!(rt.state.menus[0].1, PointerPress { x: bx, y: by });
    assert!(rt.state.selected.is_empty(), "secondary click does not select");

    // Secondary click on a folder row works too.
    let ra = row_rect(&rt, "a");
    let (ax, ay) = (ra[0] + 20.0, ra[1] + ra[3] * 0.5);
    rt.secondary_down(ax, ay);
    rt.secondary_up(ax, ay);
    rt.frame(0.0);
    assert_eq!(rt.state.menus.last().unwrap().0, "a");

    // Secondary click on empty space below the last row does not open a row's menu.
    rt.state.menus.clear();
    let rc = row_rect(&rt, "c");
    rt.pointer_down(rc[0] + 20.0, rc[1] + rc[3] * 0.5);
    rt.pointer_up(rc[0] + 20.0, rc[1] + rc[3] * 0.5);
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("c"));
    rt.secondary_down(50.0, 200.0);
    rt.secondary_up(50.0, 200.0);
    rt.frame(0.0);
    assert!(rt.state.menus.is_empty(), "empty space below rows must not open a row menu");

    // Shift+F10 and Key::ContextMenu while focused open the cursor row's menu at its bottom-left.
    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.menus.len(), 2);
    for (id, p) in &rt.state.menus {
        assert_eq!(id, "c");
        assert!((p.x - rc[0]).abs() < 0.5 && (p.y - (rc[1] + rc[3])).abs() < 0.5, "expected bottom-left of row c ({rc:?}), got {p:?}");
    }
}

#[test]
fn on_menu_is_a_no_op_without_cursor_on_empty_tree_or_when_inert() {
    // No cursor selected yet: Menu key and Shift+F10 do nothing.
    let app = MenuApp { files: menu_tree_model(), menus: Vec::new(), selected: Vec::new(), inert: false, with_menu: true };
    let mut rt = Runtime::new(app, menu_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), None);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert!(rt.state.menus.is_empty());

    // Empty tree: no-op and no panic.
    let empty = MenuApp { files: TreeModel::new().key("files"), menus: Vec::new(), selected: Vec::new(), inert: false, with_menu: true };
    let mut rt = Runtime::new(empty, menu_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.menus.is_empty());

    // Inert tree: ignores secondary click and keyboard context menu.
    let mut m = menu_tree_model();
    m.select("b");
    let inert_app = MenuApp { files: m, menus: Vec::new(), selected: Vec::new(), inert: true, with_menu: true };
    let mut rt = Runtime::new(inert_app, menu_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let rb = row_rect(&rt, "b");
    rt.secondary_down(rb[0] + 10.0, rb[1] + 10.0);
    rt.secondary_up(rb[0] + 10.0, rb[1] + 10.0);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert!(rt.state.menus.is_empty());
}

enum BarMode {
    Default,
    Kit,
    DisabledAfterStyled,
    CustomWidth(f32),
}

struct ScrollApp {
    files: TreeModel<String>,
    mode: BarMode,
    rtl: bool,
}

fn tall_model(key: &str, count: usize) -> TreeModel<String> {
    let mut m = TreeModel::new().key(key);
    for i in 0..count {
        m.insert(None, format!("row{i}"), format!("Row {i}"));
    }
    m
}

fn owned_label(s: &String) -> &str {
    s
}

thread_local! {
    static SCROLL_VIEW_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn scroll_view(s: &ScrollApp, _: &mut Cx<ScrollApp>) -> El {
    SCROLL_VIEW_RUNS.with(|c| c.set(c.get() + 1));
    let k = Kit::neutral(true);
    let mut t = Tree::new(|s: &ScrollApp| &s.files, |s: &mut ScrollApp| &mut s.files, owned_label)
        .name("Files")
        .right_to_left(s.rtl);
    t = match s.mode {
        BarMode::Default => t,
        BarMode::Kit => t.scroll_bar(true),
        BarMode::DisabledAfterStyled => t.scroll_bar_styled(ScrollBarStyle::from_kit(&k).width(8.0)).scroll_bar(false),
        BarMode::CustomWidth(w) => t.scroll_bar_styled(ScrollBarStyle::from_kit(&k).width(w)),
    };
    let root = col().size(240.0, 120.0).child(t.build(s, &k).h(120.0));
    if s.rtl { root.dir(Direction::Rtl) } else { root }
}

fn find_key<S>(rt: &Runtime<S>, key: &str) -> Option<rune::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

#[test]
fn tree_has_no_scroll_bar_by_default_and_scroll_bar_false_disables_it() {
    for mode in [BarMode::Default, BarMode::DisabledAfterStyled] {
        let mut rt = Runtime::new(
            ScrollApp { files: tall_model("files", 40), mode, rtl: false },
            scroll_view,
            (240.0, 120.0),
            Env::new(true, 1.0),
        );
        rt.set_system_scroll_bars(ScrollBars::Always);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert!(find_key(&rt, "scroll-bar").is_none(), "unset or disabled scroll bar must mount no bar");
        assert!(find_key(&rt, "thumb").is_none());
    }
}

#[test]
fn tree_scroll_bar_renders_drags_wheels_pages_reveals_with_zero_commits() {
    let mut rt = Runtime::new(
        ScrollApp { files: tall_model("files", 50), mode: BarMode::Kit, rtl: false },
        scroll_view,
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    let thumb = find_key(&rt, "thumb").expect("scroll bar thumb");
    let bar = find_key(&rt, "scroll-bar").expect("scroll bar track");
    assert!(thumb.rect[3] >= 20.0 && bar.opacity > 0.5, "thumb={:?} opacity={}", thumb.rect, bar.opacity);
    assert!(bar.rect[0] > 200.0, "LTR bar sits on the right edge: {:?}", bar.rect);

    // Wheel scroll within the mounted virtual window commits zero views and zero virtual refills.
    rt.check_live_only(false);
    let runs_before = SCROLL_VIEW_RUNS.with(|c| c.get());
    // Step past y = 0 (an exact multiple of row_height = 24), then scroll within the same row span.
    rt.wheel(bar.rect[0] + bar.rect[2] * 0.5, 60.0, -2.0);
    rt.frame(2.0 / 60.0);
    let gen_small = rune::commit_generation();
    rt.wheel(bar.rect[0] + bar.rect[2] * 0.5, 60.0, -2.0);
    rt.frame(3.0 / 60.0);
    assert_eq!(SCROLL_VIEW_RUNS.with(|c| c.get()), runs_before, "small wheel over bar must not run view");
    assert_eq!(rune::commit_generation(), gen_small, "sub-row wheel within mounted window must not commit");

    // Dragging the thumb across many virtual rows scrolls without re-running the app's view
    // (press first so the focusable tree's pointer_down focus action settles before drag moves).
    let thumb = find_key(&rt, "thumb").expect("thumb after small wheel");
    let x = thumb.rect[0] + thumb.rect[2] * 0.5;
    let y0 = thumb.rect[1] + thumb.rect[3] * 0.5;
    rt.pointer_move(x, y0);
    rt.pointer_down(x, y0);
    rt.frame(4.0 / 60.0);
    let runs_before = SCROLL_VIEW_RUNS.with(|c| c.get());
    rt.pointer_move(x, y0 + 30.0);
    rt.frame(5.0 / 60.0);
    rt.pointer_move(x, y0 + 60.0);
    rt.frame(6.0 / 60.0);
    rt.pointer_up(x, y0 + 60.0);
    rt.frame(7.0 / 60.0);
    assert_eq!(SCROLL_VIEW_RUNS.with(|c| c.get()), runs_before, "dragging the tree scroll bar must not re-run the app view");
    assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());
    let moved_thumb = find_key(&rt, "thumb").unwrap().rect;
    assert!(moved_thumb[1] > thumb.rect[1] + 20.0, "thumb moved down: {:?} -> {:?}", thumb.rect, moved_thumb);

    // Wheel over the bar scrolls back up without re-running the app's view.
    rt.wheel(bar.rect[0] + bar.rect[2] * 0.5, 60.0, 200.0);
    rt.frame(8.0 / 60.0);
    assert_eq!(SCROLL_VIEW_RUNS.with(|c| c.get()), runs_before, "wheeling over the scroll bar must not re-run the app view");
    let after_wheel = find_key(&rt, "thumb").unwrap().rect;
    assert!(after_wheel[1] < moved_thumb[1], "wheel over bar scrolled back up: {:?} -> {:?}", moved_thumb, after_wheel);

    // Page Down uses the viewport height reported through on_layout_tx (120 / 24 - 1 = 4 rows).
    rt.key(KeyEvent { key: Key::Home, mods: Mods::default() });
    rt.frame(9.0 / 60.0);
    assert_eq!(rt.state.files.cursor(), Some("row0"));
    rt.key(KeyEvent { key: Key::PageDown, mods: Mods::default() });
    rt.frame(10.0 / 60.0);
    assert_eq!(rt.state.files.cursor(), Some("row4"), "PageDown steps by viewport height (leaving one context row)");

    // reveal() still scrolls a far row into view through the wrapped viewport.
    rt.update(|s, tx| assert!(s.files.reveal("row45", tx)));
    for f in 8..30 {
        rt.frame(f as f64 / 60.0);
    }
    let revealed = find_key(&rt, "row45").expect("row45 mounted after reveal");
    assert!(revealed.visible[3] > 20.0, "row45 visible: {:?}", revealed.visible);
}

#[test]
fn tree_scroll_bar_styled_short_empty_and_rtl_edges() {
    // Custom thumb width via scroll_bar_styled.
    let mut rt = Runtime::new(
        ScrollApp { files: tall_model("files", 40), mode: BarMode::CustomWidth(9.0), rtl: false },
        scroll_view,
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let thumb = find_key(&rt, "thumb").expect("styled thumb");
    assert!((thumb.rect[2] - 9.0).abs() < 0.5, "custom thumb width 9pt: {:?}", thumb.rect);

    // Short and empty trees hide the thumb cleanly.
    for count in [0, 3] {
        let mut rt = Runtime::new(
            ScrollApp { files: tall_model("files", count), mode: BarMode::Kit, rtl: false },
            scroll_view,
            (240.0, 120.0),
            Env::new(true, 1.0),
        );
        rt.set_system_scroll_bars(ScrollBars::Always);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let bar = find_key(&rt, "scroll-bar").expect("track mounted");
        let thumb = find_key(&rt, "thumb").expect("thumb mounted");
        assert!(bar.opacity == 0.0 || thumb.rect[3] == 0.0, "count={count} bar={:?} thumb={:?}", bar.opacity, thumb.rect);
    }

    // RTL places the scroll bar on the left edge.
    let mut rt = Runtime::new(
        ScrollApp { files: tall_model("files", 40), mode: BarMode::Kit, rtl: true },
        scroll_view,
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let bar = find_key(&rt, "scroll-bar").expect("rtl bar");
    assert!(bar.rect[0] < 10.0, "RTL scroll bar sits on the left edge: {:?}", bar.rect);
}

#[test]
fn two_trees_with_scroll_bars_scroll_independently() {
    struct Pair {
        left: TreeModel<String>,
        right: TreeModel<String>,
    }
    fn pair_view(s: &Pair, _: &mut Cx<Pair>) -> El {
        let k = Kit::neutral(true);
        row()
            .size(400.0, 120.0)
            .child(Tree::new(|s: &Pair| &s.left, |s: &mut Pair| &mut s.left, owned_label).scroll_bar(true).build(s, &k).w(200.0).h(120.0))
            .child(Tree::new(|s: &Pair| &s.right, |s: &mut Pair| &mut s.right, owned_label).scroll_bar(true).build(s, &k).w(200.0).h(120.0))
    }
    let mut rt = Runtime::new(
        Pair { left: tall_model("left", 40), right: tall_model("right", 40) },
        pair_view,
        (400.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let thumbs = |rt: &Runtime<Pair>| -> Vec<[f32; 4]> {
        rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("thumb")).map(|e| e.rect).collect()
    };
    let before = thumbs(&rt);
    assert_eq!(before.len(), 2);
    let x = before[0][0] + before[0][2] * 0.5;
    let y = before[0][1] + before[0][3] * 0.5;
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 60.0);
    rt.frame(2.0 / 60.0);
    rt.pointer_up(x, y + 60.0);
    rt.frame(3.0 / 60.0);
    let after = thumbs(&rt);
    assert!(after[0][1] > before[0][1] + 20.0, "left tree scrolled");
    assert!((after[1][1] - before[1][1]).abs() < 0.5, "right tree stayed put: {:?} -> {:?}", before[1], after[1]);
}

// branches & on_toggle ---------------------------------------------------------------------

#[test]
fn insert_branch_set_branch_is_branch_and_expand_all_preserve_branch_semantics() {
    let mut m: TreeModel<FileNode<()>> = TreeModel::new();
    let dir = |id: &str| FileNode { name: id.into(), path: id.into(), dir: true, data: None };
    let file = |name: &str, path: &str| FileNode { name: name.into(), path: path.into(), dir: false, data: Some(()) };

    assert!(m.insert_branch(None, "unloaded", dir("unloaded")));
    assert!(!m.insert_branch(None, "unloaded", dir("unloaded")), "duplicate id rejected");
    assert!(!m.insert_branch(Some("missing"), "missing/sub", dir("sub")), "missing parent rejected");
    assert!(m.insert_branch(None, "empty", dir("empty")));
    assert!(m.insert(None, "promoted", dir("promoted")));
    assert!(m.insert(None, "loaded", dir("loaded")));
    assert!(m.insert(Some("loaded"), "loaded/file.txt", file("file.txt", "loaded/file.txt")));
    assert!(m.insert(None, "leaf", file("leaf", "leaf")));

    assert!(m.is_branch("unloaded") && !m.has_children("unloaded"));
    assert!(m.is_branch("empty") && !m.has_children("empty"));
    assert!(!m.is_branch("promoted"), "plain insert defaults branch=false");
    assert!(m.is_branch("loaded") && m.has_children("loaded"), "node with children is a branch");
    assert!(!m.is_branch("leaf"));
    assert!(!m.is_branch("nope"));

    // Promote and demote via set_branch; children keep a node a branch even if branch=false.
    m.set_branch("promoted", true);
    assert!(m.is_branch("promoted"));
    assert_eq!(m.row(m.visible_row("promoted").unwrap()).unwrap().1.expanded, Some(false));
    m.set_branch("promoted", false);
    assert!(!m.is_branch("promoted"));
    assert_eq!(m.row(m.visible_row("promoted").unwrap()).unwrap().1.expanded, None);
    m.set_branch("loaded", false);
    assert!(m.is_branch("loaded"), "a node with children remains a branch");
    m.set_branch("nope", true);

    // Every empty/unloaded branch reports Some(false/true) when expanded or collapsed.
    for id in ["unloaded", "empty", "loaded"] {
        assert_eq!(m.row(m.visible_row(id).unwrap()).unwrap().1.expanded, Some(false));
        m.set_expanded(id, true);
        assert!(m.is_expanded(id));
        assert_eq!(m.row(m.visible_row(id).unwrap()).unwrap().1.expanded, Some(true));
    }
    assert_eq!(m.row(m.visible_row("leaf").unwrap()).unwrap().1.expanded, None);

    // collapse_all and expand_all include empty branches, leaving leaves closed.
    m.collapse_all();
    assert!(!m.is_expanded("unloaded") && !m.is_expanded("empty") && !m.is_expanded("loaded"));
    m.expand_all();
    assert!(m.is_expanded("unloaded") && m.is_expanded("empty") && m.is_expanded("loaded"));
    assert!(!m.is_expanded("leaf"));

    // Under a filter, matching branches with no matching children show expanded: None; clearing restores branch state.
    m.filter(|n| n.name == "empty");
    assert_eq!(m.visible_len(), 1);
    assert_eq!(m.row(0).unwrap().1.expanded, None);
    m.clear_filter();
    assert_eq!(m.row(m.visible_row("empty").unwrap()).unwrap().1.expanded, Some(true));

    // FileTree marks dir: true nodes as branches.
    let ft = FileTree::new().build([("src/lib.rs", ()), ("README.md", ())]);
    assert!(ft.is_branch("src"));
    assert!(!ft.is_branch("README.md"));
}

struct LazyApp {
    files: TreeModel<FileNode<()>>,
    toggles: Vec<(String, bool)>,
    selected: Vec<String>,
    opened: Vec<String>,
    rtl: bool,
    inert: bool,
    lazy_load: bool,
}

fn lazy_dir(name: &str, path: &str) -> FileNode<()> {
    FileNode { name: name.into(), path: path.into(), dir: true, data: None }
}

fn lazy_file(name: &str, path: &str) -> FileNode<()> {
    FileNode { name: name.into(), path: path.into(), dir: false, data: Some(()) }
}

fn lazy_tree_model(multi: bool) -> TreeModel<FileNode<()>> {
    let mut m = TreeModel::new().key("lazy").multi_select(multi);
    m.insert_branch(None, "unloaded", lazy_dir("unloaded", "unloaded"));
    m.insert_branch(None, "empty", lazy_dir("empty", "empty"));
    m.insert_branch(None, "parent", lazy_dir("parent", "parent"));
    m.insert_branch(Some("parent"), "parent/sub", lazy_dir("sub", "parent/sub"));
    m.insert(None, "readme", lazy_file("README.md", "readme"));
    m
}

fn lazy_view(s: &LazyApp, _: &mut Cx<LazyApp>) -> El {
    let k = Kit::neutral(true);
    let tree = Tree::new(|s: &LazyApp| &s.files, |s: &mut LazyApp| &mut s.files, FileNode::label)
        .name("Files")
        .right_to_left(s.rtl)
        .row(|n, r| TreeItem::file(n, r))
        .on_select(|s: &mut LazyApp, id, _| s.selected.push(id.to_owned()))
        .on_open(|s: &mut LazyApp, id, _| s.opened.push(id.to_owned()))
        .on_toggle(|s: &mut LazyApp, id, open, _| {
            s.toggles.push((id.to_owned(), open));
            if s.lazy_load && open && id == "unloaded" && !s.files.has_children("unloaded") {
                s.files.insert(Some("unloaded"), "unloaded/child.rs", lazy_file("child.rs", "unloaded/child.rs"));
            }
        })
        .build(s, &k)
        .h(240.0);
    col().size(300.0, 240.0).child(if s.inert { tree.inert() } else { tree })
}

fn lazy_rt(multi: bool, rtl: bool, inert: bool, lazy_load: bool) -> Runtime<LazyApp> {
    let app = LazyApp {
        files: lazy_tree_model(multi),
        toggles: Vec::new(),
        selected: Vec::new(),
        opened: Vec::new(),
        rtl,
        inert,
        lazy_load,
    };
    let mut rt = Runtime::new(app, lazy_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

#[test]
fn empty_and_unloaded_branches_keyboard_click_and_lazy_load() {
    let mut rt = lazy_rt(false, false, false, true);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);

    // Down selects "unloaded" (closed branch).
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("unloaded"));
    assert!(rt.state.toggles.is_empty());

    // Right expands "unloaded" -> fires on_toggle("unloaded", true) -> lazily inserts child.rs; cursor stays.
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.files.is_expanded("unloaded"));
    assert_eq!(rt.state.files.cursor(), Some("unloaded"));
    assert_eq!(rt.state.toggles, [("unloaded".into(), true)]);
    assert!(rt.state.files.contains("unloaded/child.rs"));

    // Right again steps into the lazily inserted child without firing on_toggle.
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("unloaded/child.rs"));
    assert_eq!(rt.state.toggles.len(), 1);

    // Move to "empty" (genuinely empty branch).
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("empty"));

    // Right opens "empty" and fires on_toggle("empty", true).
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.files.is_expanded("empty"));
    assert_eq!(rt.state.files.cursor(), Some("empty"));
    assert_eq!(rt.state.toggles.last(), Some(&("empty".into(), true)));

    // Right again on an already-open empty branch stays on "empty" (does NOT jump to sibling "parent")
    // and does NOT fire on_toggle again.
    let before_toggles = rt.state.toggles.len();
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("empty"), "Right on open empty branch must not jump to sibling");
    assert_eq!(rt.state.toggles.len(), before_toggles);

    // Left on open empty branch closes it and fires on_toggle("empty", false).
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.files.is_expanded("empty"));
    assert_eq!(rt.state.files.cursor(), Some("empty"));
    assert_eq!(rt.state.toggles.last(), Some(&("empty".into(), false)));

    // Enter and Space on empty branch toggle it and never call on_open.
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.files.is_expanded("empty"));
    assert!(rt.state.opened.is_empty(), "Enter on empty branch must not call on_open");
    assert_eq!(rt.state.toggles.last(), Some(&("empty".into(), true)));

    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.files.is_expanded("empty"));
    assert!(rt.state.opened.is_empty());
    assert_eq!(rt.state.toggles.last(), Some(&("empty".into(), false)));

    // Nested empty branch: Left closes when open, second Left climbs to parent.
    rt.state.files.set_expanded("parent", true);
    rt.state.files.select("parent/sub");
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.files.is_expanded("parent/sub"));
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("parent/sub"), "Right on nested open empty branch stays put");
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.files.is_expanded("parent/sub"));
    assert_eq!(rt.state.files.cursor(), Some("parent/sub"));
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("parent"), "Left on closed nested empty branch climbs to parent");

    // Plain click on empty branch toggles and fires on_toggle; double-click does not call on_open.
    rt.state.toggles.clear();
    let r = row_rect(&rt, "empty");
    let (x, y) = (r[0] + 20.0, r[1] + r[3] * 0.5);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.01);
    assert!(rt.state.files.is_expanded("empty"));
    assert_eq!(rt.state.toggles, [("empty".into(), true)]);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.05);
    assert!(!rt.state.files.is_expanded("empty"));
    assert!(rt.state.opened.is_empty(), "double-click on empty branch must not call on_open");
    assert_eq!(rt.state.toggles, [("empty".into(), true), ("empty".into(), false)]);

    // Leaf still calls on_open on Enter and never fires on_toggle.
    rt.state.toggles.clear();
    rt.state.files.select("readme");
    rt.frame(0.1);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.1);
    assert_eq!(rt.state.opened, ["readme"]);
    assert!(rt.state.toggles.is_empty());

    // Demoting an empty branch back to leaf restores leaf Enter -> on_open behaviour.
    rt.state.files.set_branch("empty", false);
    rt.state.files.select("empty");
    rt.frame(0.1);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.1);
    assert_eq!(rt.state.opened, ["readme", "empty"]);
    assert!(rt.state.toggles.is_empty());
}

#[test]
fn empty_branches_respect_rtl_multi_select_mods_and_inert() {
    // RTL: Left opens an empty branch, Left again stays on it, Right closes it.
    let mut rt = lazy_rt(false, true, false, false);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("unloaded"));
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.files.is_expanded("unloaded"));
    assert_eq!(rt.state.toggles, [("unloaded".into(), true)]);
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.files.cursor(), Some("unloaded"), "RTL Left on open empty branch stays put");
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.files.is_expanded("unloaded"));
    assert_eq!(rt.state.toggles, [("unloaded".into(), true), ("unloaded".into(), false)]);

    // Multi-select: Shift-click and Cmd-click select without toggling or firing on_toggle.
    let mut rt = lazy_rt(true, false, false, false);
    rt.state.files.select("unloaded");
    rt.frame(0.0);
    let re = row_rect(&rt, "empty");
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    rt.pointer_down(re[0] + 20.0, re[1] + re[3] * 0.5);
    rt.pointer_up(re[0] + 20.0, re[1] + re[3] * 0.5);
    rt.frame(0.0);
    assert!(!rt.state.files.is_expanded("empty"), "Shift-click in multi-select does not toggle");
    assert!(rt.state.toggles.is_empty());
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods { cmd: true, ..Mods::default() }));
    rt.pointer_down(re[0] + 20.0, re[1] + re[3] * 0.5);
    rt.pointer_up(re[0] + 20.0, re[1] + re[3] * 0.5);
    rt.frame(0.0);
    assert!(!rt.state.files.is_expanded("empty"), "Cmd-click in multi-select does not toggle");
    assert!(rt.state.toggles.is_empty());

    // Inert tree ignores clicks and keys on empty branches.
    let mut rt = lazy_rt(false, false, true, false);
    rt.state.files.select("empty");
    rt.frame(0.0);
    let re = row_rect(&rt, "empty");
    rt.pointer_down(re[0] + 20.0, re[1] + re[3] * 0.5);
    rt.pointer_up(re[0] + 20.0, re[1] + re[3] * 0.5);
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.files.is_expanded("empty"));
    assert!(rt.state.toggles.is_empty());
}
