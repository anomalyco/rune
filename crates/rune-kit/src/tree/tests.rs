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
