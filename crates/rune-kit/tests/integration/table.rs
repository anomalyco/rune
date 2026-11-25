//! Table: column widths and resizing, sorting from the header, row selection and keys,
//! virtualisation, accessibility roles and truncation tooltips, in a headless runtime.

use std::borrow::Cow;

use rune::runtime::ElementInfo;
use rune::{Align, Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Cell, Kit, SortDir, Table, TableColumn, TableState};

struct File {
    id: String,
    name: String,
    size: u64,
}

struct App {
    files: Vec<File>,
    table: TableState,
    sorts: Vec<(String, SortDir)>,
    opened: Vec<String>,
}

impl App {
    fn new(n: usize, multi: bool) -> App {
        let files = (0..n)
            .map(|i| File { id: format!("f{i}"), name: format!("file {i} with a name long enough to cut short.rs"), size: (i as u64 * 7919) % 100_000 })
            .collect();
        App { files, table: TableState::new().multi_select(multi), sorts: Vec::new(), opened: Vec::new() }
    }
}

const W: f32 = 500.0;
const H: f32 = 300.0;

fn view(s: &App, _: &mut Cx<App>) -> El {
    sized(s, W, H)
}

fn sized(s: &App, w: f32, h: f32) -> El {
    let k = Kit::neutral(true);
    col().size(w, h).child(
        Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.files.len(), |s: &App, i| Cow::Borrowed(s.files[i].id.as_str()))
            .name("Files")
            .column(TableColumn::new("name", "Name").flex(1.0).min(120.0).sortable())
            .column(TableColumn::new("size", "Size").width(100.0).min(60.0).max(180.0).align(Align::End).sortable())
            .column(TableColumn::new("kind", "Kind").width(80.0).resizable(false))
            .cell(|s: &App, i, col| match col {
                "name" => Cell::text(&s.files[i].name),
                "size" => Cell::text(s.files[i].size.to_string()).mono(),
                _ => Cell::text("Rust").muted(),
            })
            .on_sort(|s: &mut App, col, dir, _| {
                s.sorts.push((col.to_owned(), dir));
                if col == "size" {
                    s.files.sort_by(|a, b| dir.apply(a.size.cmp(&b.size)));
                }
            })
            .on_open(|s: &mut App, id, _| s.opened.push(id.to_owned()))
            .zebra(true)
            .build(s, &k)
            .h(h),
    )
}

fn runtime(app: App) -> Runtime<App> {
    let mut rt = Runtime::new(app, view, (W, H), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt
}

fn find(rt: &Runtime<App>, f: impl Fn(&ElementInfo) -> bool) -> Vec<ElementInfo> {
    rt.elements().into_iter().filter(|e| !e.exiting && f(e)).collect()
}

fn header(rt: &Runtime<App>, id: &str) -> ElementInfo {
    find(rt, |e| e.role == "columnheader" && e.key.as_deref() == Some(id)).remove(0)
}

/// How assistive technology hears column `id` sorted: "ascending", "descending" or "".
fn sort_read(rt: &Runtime<App>, id: &str) -> String {
    fn walk(v: &serde_json::Value, id: &str) -> Option<String> {
        if v["role"] == "column-header" && v["key"] == id { return Some(v["sort"].as_str().unwrap_or("").to_owned()); }
        v["children"].as_array()?.iter().find_map(|c| walk(c, id))
    }
    walk(&rt.accessibility_json(), id).unwrap_or_default()
}

fn press_with(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(0.0);
}

fn press(rt: &mut Runtime<App>, key: Key) {
    press_with(rt, key, Mods::default());
}

fn click(rt: &mut Runtime<App>, x: f32, y: f32) {
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

#[test]
fn fixed_columns_keep_their_width_and_flex_takes_the_rest_in_header_and_rows_alike() {
    let rt = runtime(App::new(20, false));
    let (name, size, kind) = (header(&rt, "name"), header(&rt, "size"), header(&rt, "kind"));
    assert_eq!((size.rect[2], kind.rect[2]), (100.0, 80.0));
    assert_eq!(name.rect[2], W - 180.0);
    let cells = find(&rt, |e| e.role == "gridcell" && e.key.as_deref() == Some("size"));
    assert!(!cells.is_empty());
    for c in cells {
        assert_eq!((c.rect[0], c.rect[2]), (size.rect[0], size.rect[2]), "a body cell lines up under its header");
    }
}

#[test]
fn dragging_a_header_edge_resizes_within_the_column_bounds() {
    let mut rt = runtime(App::new(20, false));
    let size = header(&rt, "size");
    let handle = find(&rt, |e| e.label.as_deref() == Some("Resize Size")).remove(0).rect;
    let (x, y) = (handle[0] + handle[2] * 0.5, handle[1] + handle[3] * 0.5);
    let drag = |rt: &mut Runtime<App>, dx: f32| {
        rt.pointer_down(x, y);
        rt.frame(0.0);
        for k in 1..=4 {
            rt.pointer_move(x + dx * k as f32 / 4.0, y);
            rt.frame(0.0);
        }
        rt.pointer_up(x + dx, y);
        rt.frame(0.0);
        rt.frame(0.0);
    };
    drag(&mut rt, 30.0);
    assert_eq!(rt.state.table.column_width("size"), Some(130.0));
    assert_eq!(header(&rt, "size").rect[2], 130.0, "the column draws at its new width");
    assert_eq!(header(&rt, "name").rect[2], W - 210.0, "and the flex column gives way");
    let handle = find(&rt, |e| e.label.as_deref() == Some("Resize Size")).remove(0).rect;
    let (x2, y2) = (handle[0] + handle[2] * 0.5, handle[1] + handle[3] * 0.5);
    rt.pointer_down(x2, y2);
    rt.frame(0.0);
    rt.pointer_move(x2 + 400.0, y2);
    rt.frame(0.0);
    rt.pointer_up(x2 + 400.0, y2);
    rt.frame(0.0);
    assert_eq!(rt.state.table.column_width("size"), Some(180.0), "clamped to the max");
    rt.pointer_down(x2, y2);
    rt.frame(0.0);
    rt.pointer_move(x2 - 400.0, y2);
    rt.frame(0.0);
    rt.pointer_up(x2 - 400.0, y2);
    rt.frame(0.0);
    assert_eq!(rt.state.table.column_width("size"), Some(60.0), "clamped to the min");
    assert!(size.rect[2] == 100.0);
    assert!(find(&rt, |e| e.label.as_deref() == Some("Resize Kind")).is_empty(), "a fixed column has no handle");
}

#[test]
fn a_header_click_sorts_and_shows_the_direction() {
    let mut rt = runtime(App::new(50, false));
    let r = header(&rt, "size").rect;
    click(&mut rt, r[0] + 20.0, r[1] + r[3] * 0.5);
    assert_eq!(rt.state.sorts, [("size".to_owned(), SortDir::Ascending)]);
    assert_eq!(rt.state.table.sort(), Some(("size", SortDir::Ascending)));
    assert_eq!(header(&rt, "size").label.as_deref(), Some("Size"));
    assert_eq!(sort_read(&rt, "size"), "ascending");
    assert!(rt.state.files.windows(2).all(|w| w[0].size <= w[1].size));
    click(&mut rt, r[0] + 20.0, r[1] + r[3] * 0.5);
    assert_eq!(rt.state.table.sort(), Some(("size", SortDir::Descending)));
    assert_eq!(sort_read(&rt, "size"), "descending");
    assert_eq!(header(&rt, "name").label.as_deref(), Some("Name"));
    assert_eq!(sort_read(&rt, "name"), "");
    let r = header(&rt, "kind").rect;
    click(&mut rt, r[0] + 20.0, r[1] + r[3] * 0.5);
    assert_eq!(rt.state.sorts.len(), 2, "a column that doesn't sort ignores clicks");
}

#[test]
fn cancelling_a_column_resize_restores_the_original_width_policy() {
    for initial in [None, Some(110.0)] {
        let mut rt = runtime(App::new(20, false));
        if let Some(w) = initial { rt.update(|s, _| s.table.set_column_width("size", w)); rt.frame(0.0); }
        let handle = find(&rt, |e| e.label.as_deref() == Some("Resize Size")).remove(0).rect;
        let (x, y) = (handle[0] + handle[2] / 2.0, handle[1] + handle[3] / 2.0);
        rt.pointer_down(x, y);
        rt.pointer_move(x + 40.0, y);
        rt.frame(0.0);
        assert_ne!(rt.state.table.column_width("size"), initial);
        press(&mut rt, Key::Escape);
        assert_eq!(rt.state.table.column_width("size"), initial);
        rt.pointer_up(x + 40.0, y);
        rt.frame(0.0);
        assert_eq!(rt.state.table.column_width("size"), initial);
    }
}

#[test]
fn keys_move_the_selection_and_return_opens() {
    let mut rt = runtime(App::new(200, false));
    press(&mut rt, Key::Tab);
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key).as_deref(), Some("table"));
    press(&mut rt, Key::Down);
    assert_eq!(rt.state.table.cursor(), Some("f0"));
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Down);
    assert_eq!(rt.state.table.cursor(), Some("f2"));
    press(&mut rt, Key::PageDown);
    let page = rt.state.table.cursor().unwrap()[1..].parse::<usize>().unwrap() - 2;
    assert!((5..12).contains(&page), "Page Down moves about a screen of rows: {page}");
    press(&mut rt, Key::End);
    assert_eq!(rt.state.table.cursor(), Some("f199"));
    for f in 1..30 {
        rt.frame(f as f64 / 60.0);
    }
    let row = find(&rt, |e| e.role == "row" && e.item_selected).remove(0);
    assert!(row.visible[3] > 20.0, "End scrolls the last row into view");
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened, ["f199"]);
    press(&mut rt, Key::Home);
    assert_eq!(rt.state.table.selected().collect::<Vec<_>>(), ["f0"]);
}

#[test]
fn shift_extends_and_cmd_a_selects_all_in_a_multi_select_table() {
    let mut rt = runtime(App::new(30, true));
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Down);
    press_with(&mut rt, Key::Down, Mods { shift: true, ..Mods::default() });
    press_with(&mut rt, Key::Down, Mods { shift: true, ..Mods::default() });
    let mut sel: Vec<&str> = rt.state.table.selected().collect();
    sel.sort();
    assert_eq!(sel, ["f0", "f1", "f2"]);
    press_with(&mut rt, Key::Command('a'), Mods { cmd: true, ..Mods::default() });
    assert_eq!(rt.state.table.selected_len(), 30);
}

#[test]
fn a_selection_follows_its_row_through_a_sort() {
    let mut rt = runtime(App::new(50, false));
    rt.update(|s, tx| s.table.reveal("f7", tx));
    rt.frame(0.0);
    let r = header(&rt, "size").rect;
    click(&mut rt, r[0] + 20.0, r[1] + r[3] * 0.5);
    assert_eq!(rt.state.table.selected().collect::<Vec<_>>(), ["f7"]);
}

#[test]
fn a_hundred_thousand_rows_build_only_what_shows() {
    let rt = runtime(App::new(100_000, false));
    let built = find(&rt, |e| e.role == "row").len();
    assert!((5..60).contains(&built), "{built} rows built for a {H} pt table");
}

#[test]
fn roles_and_cut_short_text_offer_their_whole_text() {
    let mut app = App::new(5, false);
    app.files[1].name = "short.rs".into();
    let mut rt = runtime(app);
    rt.update(|s, _| {
        s.table.set_column_width("name", 120.0);
        s.table.select("f0");
    });
    rt.frame(0.0);
    let all = rt.elements();
    let grid = all.iter().find(|e| e.role == "grid").unwrap();
    assert_eq!(grid.label.as_deref(), Some("Files"));
    assert_eq!(all.iter().filter(|e| e.role == "columnheader").count(), 3);
    assert_eq!(all.iter().filter(|e| e.role == "row" && e.item_selected).count(), 1);
    assert!(all.iter().filter(|e| e.role == "gridcell").count() >= 15);
    let help: Vec<_> = all.iter().filter_map(|e| e.help.as_deref()).collect();
    assert!(help.contains(&"file 0 with a name long enough to cut short.rs"));
    assert!(!help.contains(&"short.rs"), "text that fits offers no truncation tooltip");
}

#[test]
fn shrinking_rows_under_a_selection_stays_consistent() {
    let mut rt = runtime(App::new(20, true));
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::End);
    assert_eq!(rt.state.table.cursor(), Some("f19"));
    rt.update(|s, _| s.files.truncate(5));
    rt.frame(0.0);
    press(&mut rt, Key::Down);
    assert_eq!(rt.state.table.cursor(), Some("f0"), "a removed cursor starts at the top on Down");
    press_with(&mut rt, Key::End, Mods { shift: true, ..Mods::default() });
    assert_eq!(rt.state.table.selected_len(), 5);
}

#[test]
fn row_hook_fires_secondary_press_across_full_row_and_preserves_cells_and_handlers() {
    use rune::accesskit::Action;
    use rune::PointerPress;

    struct RowApp {
        files: Vec<File>,
        table: TableState,
        selected: Vec<String>,
        opened: Vec<String>,
        menus: Vec<(String, PointerPress)>,
    }

    fn row_view(s: &RowApp, _: &mut Cx<RowApp>) -> El {
        let k = Kit::neutral(true);
        col().size(W, H).child(
            Table::new(
                |s: &RowApp| &s.table,
                |s: &mut RowApp| &mut s.table,
                |s: &RowApp| s.files.len(),
                |s: &RowApp, i| Cow::Borrowed(s.files[i].id.as_str()),
            )
            .name("Files")
            // Fixed widths total 220 pt in a 500 pt table, leaving 280 pt of trailing row space.
            .column(TableColumn::new("name", "Name").width(140.0).resizable(false))
            .column(TableColumn::new("size", "Size").width(80.0).align(Align::End).resizable(false))
            .cell(|s: &RowApp, i, col| match col {
                "name" => Cell::text(&s.files[i].name),
                _ => Cell::text(s.files[i].size.to_string()).mono(),
            })
            .on_select(|s: &mut RowApp, id, _| s.selected.push(id.to_owned()))
            .on_open(|s: &mut RowApp, id, _| s.opened.push(id.to_owned()))
            .row(|s: &RowApp, i, r| {
                let id = s.files[i].id.clone();
                r.on_secondary_press(move |s: &mut RowApp, p, _| {
                    if !s.table.is_selected(&id) {
                        s.table.select(id.clone());
                    }
                    s.menus.push((id.clone(), p));
                })
            })
            .build(s, &k)
            .h(H),
        )
    }

    let mut files: Vec<File> = (0..6)
        .map(|i| File { id: format!("f{i}"), name: format!("file {i} with a name long enough to cut short.rs"), size: i as u64 * 100 })
        .collect();
    files[1].name = "short.rs".into();
    let app = RowApp {
        files,
        table: TableState::new().multi_select(true),
        selected: Vec::new(),
        opened: Vec::new(),
        menus: Vec::new(),
    };
    let mut rt = Runtime::new(app, row_view, (W, H), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    let row_rect = |rt: &Runtime<RowApp>, key: &str| {
        rt.elements().into_iter().find(|e| !e.exiting && e.role == "row" && e.key.as_deref() == Some(key)).unwrap().rect
    };
    let right_click = |rt: &mut Runtime<RowApp>, x: f32, y: f32| {
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(0.0);
    };

    // 1. Secondary press on text cell fires hook and selects unselected row.
    let r0 = row_rect(&rt, "f0");
    right_click(&mut rt, r0[0] + 40.0, r0[1] + r0[3] * 0.5);
    assert_eq!(rt.state.menus.len(), 1);
    assert_eq!(rt.state.menus[0].0, "f0");
    assert_eq!(rt.state.table.selected().collect::<Vec<_>>(), ["f0"]);

    // 2. Secondary press on cell horizontal padding (x = 4 pt inside cell_pad = 10 pt) fires hook.
    let r1 = row_rect(&rt, "f1");
    right_click(&mut rt, r1[0] + 4.0, r1[1] + r1[3] * 0.5);
    assert_eq!(rt.state.menus.len(), 2);
    assert_eq!(rt.state.menus[1].0, "f1");
    assert_eq!(rt.state.table.selected().collect::<Vec<_>>(), ["f1"]);

    // 3. Secondary press in empty row space past the last fixed column (x = 350 > 220) fires hook.
    let r2 = row_rect(&rt, "f2");
    assert!(r2[2] >= 490.0 && r2[0] + 350.0 < r2[0] + r2[2]);
    right_click(&mut rt, r2[0] + 350.0, r2[1] + r2[3] * 0.5);
    assert_eq!(rt.state.menus.len(), 3);
    assert_eq!(rt.state.menus[2].0, "f2");
    assert_eq!(rt.state.table.selected().collect::<Vec<_>>(), ["f2"]);

    // 4. Primary click still selects and fires on_select; Shift-click builds multi-selection,
    //    and right-clicking an already-selected row preserves that multi-selection.
    let r0 = row_rect(&rt, "f0");
    rt.pointer_down(r0[0] + 40.0, r0[1] + r0[3] * 0.5);
    rt.pointer_up(r0[0] + 40.0, r0[1] + r0[3] * 0.5);
    rt.frame(0.0);
    assert_eq!(rt.state.selected.last().map(String::as_str), Some("f0"));
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Down, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.table.selected_len(), 2, "f0 and f1 selected");
    let r1 = row_rect(&rt, "f1");
    right_click(&mut rt, r1[0] + 40.0, r1[1] + r1[3] * 0.5);
    assert_eq!(rt.state.menus.last().map(|(id, _)| id.as_str()), Some("f1"));
    assert_eq!(rt.state.table.selected_len(), 2, "multi-selection preserved on right-click");

    // 5. Double-click still fires Table's on_open handler.
    let r3 = row_rect(&rt, "f3");
    let (dx, dy) = (r3[0] + 40.0, r3[1] + r3[3] * 0.5);
    for _ in 0..2 {
        rt.pointer_down(dx, dy);
        rt.pointer_up(dx, dy);
    }
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["f3"]);

    // 6. Cell::text's help_when_truncated tooltip is preserved.
    let help: Vec<_> = rt.elements().into_iter().filter_map(|e| e.help).collect();
    assert!(help.iter().any(|h| h == "file 0 with a name long enough to cut short.rs"));
    assert!(!help.iter().any(|h| h == "short.rs"), "untruncated text still has no tooltip");

    // 7. Accessibility: rows with on_secondary_press expose ShowContextMenu and execute it.
    fn find_a11y<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        if v["key"] == key { return Some(v); }
        v["children"].as_array()?.iter().find_map(|c| find_a11y(c, key))
    }
    let tree = rt.accessibility_json();
    let row_a11y = find_a11y(&tree, "f4").expect("row f4 in a11y tree");
    let actions = row_a11y["actions"].as_array().expect("row actions");
    assert!(actions.iter().any(|a| a == "context-menu"), "row exposes context-menu: {actions:?}");
    let before = rt.state.menus.len();
    assert!(rt.accessibility_action("f4", Action::ShowContextMenu, None));
    rt.frame(0.0);
    assert_eq!(rt.state.menus.len(), before + 1);
    assert_eq!(rt.state.menus.last().map(|(id, _)| id.as_str()), Some("f4"));

    // 8. A default table without .row(..) does not add context-menu to its rows.
    let plain = runtime(App::new(4, false));
    let plain_tree = plain.accessibility_json();
    let plain_row = find_a11y(&plain_tree, "f0").expect("plain row f0");
    let plain_actions = plain_row["actions"].as_array();
    assert!(plain_actions.is_none_or(|a| !a.iter().any(|x| x == "context-menu")));
}

#[test]
fn row_hook_runs_only_for_mounted_virtual_rows_and_handles_empty_table() {
    use std::cell::Cell as StdCell;
    use std::rc::Rc;

    struct CountApp {
        n: usize,
        table: TableState,
        calls: Rc<StdCell<usize>>,
    }

    fn count_view(s: &CountApp, _: &mut Cx<CountApp>) -> El {
        let k = Kit::neutral(true);
        let calls = s.calls.clone();
        col().size(W, H).child(
            Table::new(
                |s: &CountApp| &s.table,
                |s: &mut CountApp| &mut s.table,
                |s: &CountApp| s.n,
                |_: &CountApp, i| Cow::Owned(format!("r{i}")),
            )
            .column(TableColumn::new("name", "Name").flex(1.0))
            .cell(|_: &CountApp, i, _| Cell::text(format!("row {i}")))
            .row(move |_: &CountApp, _, r| {
                calls.set(calls.get() + 1);
                r
            })
            .build(s, &k)
            .h(H),
        )
    }

    // Empty table: hook is never called.
    let empty_calls = Rc::new(StdCell::new(0));
    let mut rt_empty = Runtime::new(
        CountApp { n: 0, table: TableState::new(), calls: empty_calls.clone() },
        count_view,
        (W, H),
        Env::new(true, 1.0),
    );
    rt_empty.frame(0.0);
    assert_eq!(empty_calls.get(), 0, "empty table calls row hook zero times");

    // 10,000 rows: hook runs only for mounted rows.
    let big_calls = Rc::new(StdCell::new(0));
    let mut rt_big = Runtime::new(
        CountApp { n: 10_000, table: TableState::new(), calls: big_calls.clone() },
        count_view,
        (W, H),
        Env::new(true, 1.0),
    );
    rt_big.frame(0.0);
    assert!((5..80).contains(&big_calls.get()), "row hook ran {} times for 10,000 rows", big_calls.get());
}

#[test]
fn stick_bottom_follows_appended_rows_and_preserves_position_when_scrolled_up() {
    struct LogApp {
        count: usize,
        stick: bool,
        table: TableState,
    }

    fn log_view(s: &LogApp, _: &mut Cx<LogApp>) -> El {
        let k = Kit::neutral(true);
        col().size(W, H).child(
            Table::new(
                |s: &LogApp| &s.table,
                |s: &mut LogApp| &mut s.table,
                |s: &LogApp| s.count,
                |_: &LogApp, i| Cow::Owned(format!("l{i}")),
            )
            .column(TableColumn::new("msg", "Message").flex(1.0))
            .cell(|_: &LogApp, i, _| Cell::text(format!("log entry {i}")))
            .stick_bottom(s.stick)
            .build(s, &k)
            .h(H),
        )
    }

    let settle = |rt: &mut Runtime<LogApp>| {
        for f in 0..10 {
            rt.frame(f as f64 / 60.0);
        }
    };
    let row_visible = |rt: &Runtime<LogApp>, key: &str| {
        rt.elements().into_iter().find(|e| !e.exiting && e.role == "row" && e.key.as_deref() == Some(key) && e.visible[3] > 20.0)
    };

    // 1. stick_bottom(true) starts at the end and follows appended rows.
    let mut rt = Runtime::new(LogApp { count: 40, stick: true, table: TableState::new() }, log_view, (W, H), Env::new(true, 1.0));
    settle(&mut rt);
    assert_eq!(rt.scroll_at_bottom("viewport"), Some(true));
    assert!(row_visible(&rt, "l39").is_some(), "last row is in view at launch");

    rt.update(|s, _| s.count = 60);
    settle(&mut rt);
    assert_eq!(rt.scroll_at_bottom("viewport"), Some(true), "stays at bottom after append");
    assert!(row_visible(&rt, "l59").is_some(), "newly appended last row is in view");

    // 2. Scrolling up detaches and preserves scroll position across further appends.
    rt.wheel(W * 0.5, H * 0.5, 220.0);
    for f in 1..=60 {
        rt.frame(1.0 + f as f64 / 60.0);
    }
    assert_eq!(rt.scroll_at_bottom("viewport"), Some(false), "scrolling up detaches from bottom");
    let anchor = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.role == "row" && e.visible[3] > 20.0 && e.key.as_deref().is_some_and(|k| k.starts_with('l')))
        .expect("a visible row after scrolling up");
    let anchor_key = anchor.key.clone().unwrap();
    let anchor_y = anchor.rect[1];

    rt.update(|s, _| s.count = 80);
    for f in 1..=20 {
        rt.frame(3.0 + f as f64 / 60.0);
    }
    assert_eq!(rt.scroll_at_bottom("viewport"), Some(false), "stays detached after appending");
    assert!(row_visible(&rt, "l79").is_none(), "appended tail does not pull scrolled-up viewport");
    let after = row_visible(&rt, &anchor_key).expect("anchor row stayed visible");
    assert!((after.rect[1] - anchor_y).abs() < 1.0, "anchor row kept position: {anchor_y} -> {}", after.rect[1]);

    // 3. Starting empty with stick_bottom(true) and growing past viewport height stays pinned.
    let mut rt_empty = Runtime::new(LogApp { count: 0, stick: true, table: TableState::new() }, log_view, (W, H), Env::new(true, 1.0));
    settle(&mut rt_empty);
    rt_empty.update(|s, _| s.count = 30);
    settle(&mut rt_empty);
    assert_eq!(rt_empty.scroll_at_bottom("viewport"), Some(true));
    assert!(row_visible(&rt_empty, "l29").is_some(), "empty->filled table pins to new end");

    // 4. Default stick_bottom(false) starts at the top and stays there on append.
    let mut rt_default = Runtime::new(LogApp { count: 40, stick: false, table: TableState::new() }, log_view, (W, H), Env::new(true, 1.0));
    settle(&mut rt_default);
    assert_eq!(rt_default.scroll_at_bottom("viewport"), Some(false));
    assert!(row_visible(&rt_default, "l0").is_some(), "default starts at row 0");
    rt_default.update(|s, _| s.count = 60);
    settle(&mut rt_default);
    assert_eq!(rt_default.scroll_at_bottom("viewport"), Some(false));
    assert!(row_visible(&rt_default, "l0").is_some(), "default keeps row 0 in view after append");
}

#[test]
fn cursor_outline_can_be_disabled_in_multi_select_while_default_keeps_it() {
    struct OutlineApp {
        table: TableState,
        outline: Option<bool>,
    }

    fn outline_view(s: &OutlineApp, _: &mut Cx<OutlineApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &OutlineApp| &s.table,
            |s: &mut OutlineApp| &mut s.table,
            |_: &OutlineApp| 6,
            |_: &OutlineApp, i| Cow::Owned(format!("f{i}")),
        )
        .name("Files")
        .column(TableColumn::new("name", "Name").flex(1.0))
        .cell(|_: &OutlineApp, i, _| Cell::text(format!("row {i}")));
        if let Some(on) = s.outline {
            t = t.cursor_outline(on);
        }
        col().size(W, H).child(t.build(s, &k).h(H))
    }

    let has_row_outline = |rt: &Runtime<OutlineApp>, key: &str| -> bool {
        let r = rt.elements().into_iter().find(|e| !e.exiting && e.role == "row" && e.key.as_deref() == Some(key)).unwrap().rect;
        rt.instances.iter().any(|i| {
            i.params[1] == rune::render::KIND_RECT
                && (i.params[0] - 1.0).abs() < 0.01
                && i.fill == [0.0; 4]
                && i.border[3] > 0.0
                && (i.rect[0] - r[0]).abs() < 0.5
                && (i.rect[1] - r[1]).abs() < 0.5
                && (i.rect[2] - r[2]).abs() < 0.5
                && (i.rect[3] - r[3]).abs() < 0.5
        })
    };

    // 1. Default (cursor_outline not called): multi-select draws 1 pt inset outline on cursor row only.
    let mut table = TableState::new().multi_select(true);
    table.select("f1");
    let mut rt_default = Runtime::new(OutlineApp { table, outline: None }, outline_view, (W, H), Env::new(true, 1.0));
    rt_default.frame(0.0);
    assert!(has_row_outline(&rt_default, "f1"), "default multi-select draws cursor outline on cursor row");
    assert!(!has_row_outline(&rt_default, "f0"), "non-cursor row has no cursor outline");

    // 2. cursor_outline(false): removes the outline while keeping selection fill, keys and a11y activeDescendant.
    let mut table = TableState::new().multi_select(true);
    table.select("f1");
    let mut rt_off = Runtime::new(OutlineApp { table, outline: Some(false) }, outline_view, (W, H), Env::new(true, 1.0));
    rt_off.frame(0.0);
    assert!(!has_row_outline(&rt_off, "f1"), "cursor_outline(false) suppresses the 1 pt focus outline");
    let row_f1 = rt_off.elements().into_iter().find(|e| !e.exiting && e.role == "row" && e.key.as_deref() == Some("f1")).unwrap();
    assert!(row_f1.item_selected && row_f1.fill.a > 0.0, "selection fill and item_selected stay intact");

    rt_off.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt_off.frame(0.0);
    rt_off.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt_off.frame(0.0);
    assert_eq!(rt_off.state.table.cursor(), Some("f2"));
    assert!(!has_row_outline(&rt_off, "f2"), "moved cursor row still has no outline");
    fn find_grid<'a>(v: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
        if v["role"] == "grid" { return Some(v); }
        v["children"].as_array()?.iter().find_map(find_grid)
    }
    let tree = rt_off.accessibility_json();
    assert_eq!(find_grid(&tree).unwrap()["activeDescendant"], "f2", "a11y activeDescendant still tracks cursor");

    // 3. Single-select table never draws the multi-select cursor outline even with cursor_outline(true).
    let mut single = TableState::new().multi_select(false);
    single.select("f1");
    let mut rt_single = Runtime::new(OutlineApp { table: single, outline: Some(true) }, outline_view, (W, H), Env::new(true, 1.0));
    rt_single.frame(0.0);
    assert!(!has_row_outline(&rt_single, "f1"), "single-select draws no cursor outline");
}

#[test]
fn table_virtual_overscan_forwards_and_reduces_mounted_nodes() {
    struct OverscanApp {
        rows: usize,
        table: TableState,
        overscan: Option<(f32, f32)>,
    }
    fn overscan_view(s: &OverscanApp, _: &mut Cx<OverscanApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &OverscanApp| &s.table,
            |s: &mut OverscanApp| &mut s.table,
            |s: &OverscanApp| s.rows,
            |_: &OverscanApp, i| Cow::Owned(format!("r{i}")),
        )
        .name("Signals")
        .column(TableColumn::new("id", "ID").width(80.0))
        .column(TableColumn::new("val", "Value").width(100.0).align(Align::End))
        .column(TableColumn::new("state", "State").flex(1.0).min(100.0))
        .cell(|_: &OverscanApp, i, col| match col {
            "id" => Cell::text(format!("sig-{i:04}")).mono(),
            "val" => Cell::text(format!("{:.2}", i as f32 * 0.25)).mono(),
            _ => Cell::text("ok"),
        });
        if let Some((o, e)) = s.overscan {
            t = t.virtual_overscan(o, e);
        }
        col().size(500.0, 300.0).child(t.build(s, &k).h(300.0))
    }

    let run = |overscan: Option<(f32, f32)>| {
        let mut rt = Runtime::new(
            OverscanApp { rows: 5_000, table: TableState::new(), overscan },
            overscan_view,
            (500.0, 300.0),
            Env::new(true, 1.0),
        );
        for _ in 0..30 {
            if !rt.frame(1.0 / 60.0) { break; }
        }
        rt.update(|_, tx| {
            tx.scroll_to_instant("table/viewport", 0.0, 2_500.0 * 28.0);
        });
        for _ in 0..80 {
            if !rt.frame(1.0 / 60.0) { break; }
        }
        assert!(!rt.frame(1.0 / 60.0), "table overscan={overscan:?} must settle idle");
        // Body rows have role "row" excluding the single "header" row.
        let body_rows = rt.elements().into_iter().filter(|e| e.role == "row" && e.key.as_deref() != Some("header")).count();
        let nodes = rt.stats.layout_nodes;
        (rt, body_rows, nodes)
    };

    let (_, def_rows, def_nodes) = run(None);
    let (_, explicit_rows, explicit_nodes) = run(Some((2.0, 1.75)));
    assert_eq!(def_rows, explicit_rows, "explicit (2.0, 1.75) matches default table");
    assert_eq!(def_nodes, explicit_nodes, "explicit (2.0, 1.75) matches default table nodes");

    let (mut rt_tight, tight_rows, tight_nodes) = run(Some((0.5, 0.25)));
    let (_, zero_rows, zero_nodes) = run(Some((0.0, 0.0)));
    assert!(tight_rows < def_rows / 2, "tight ({tight_rows}) must mount under half of default ({def_rows})");
    assert!(tight_nodes < def_nodes / 2, "tight nodes ({tight_nodes}) must be under half of default ({def_nodes})");
    assert!(zero_rows < tight_rows, "zero ({zero_rows}) mounts fewer rows than tight ({tight_rows})");
    assert!(zero_nodes < tight_nodes, "zero nodes ({zero_nodes}) fewer than tight ({tight_nodes})");

    // Scrolling and accessibility roles remain intact with custom virtual_overscan.
    for _ in 0..30 {
        rt_tight.wheel(250.0, 150.0, -18.0);
        rt_tight.frame(1.0 / 60.0);
    }
    for _ in 0..30 {
        if !rt_tight.frame(1.0 / 60.0) { break; }
    }
    assert!(!rt_tight.frame(1.0 / 60.0), "tight table settles idle after wheel scroll");
    let tree = rt_tight.accessibility_json();
    fn find_grid(v: &serde_json::Value) -> Option<&serde_json::Value> {
        if v["role"] == "grid" { return Some(v); }
        v["children"].as_array()?.iter().find_map(find_grid)
    }
    let grid = find_grid(&tree).expect("grid role present");
    assert_eq!(grid["name"], "Signals");
}

// Benchmark ---------------------------------------------------------------------------------

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// 100,000 rows: the first frame, scrolling at 120 Hz, a sort and a column resize, in a
/// 900 pt window. `cargo test --profile perf -p rune-kit --test integration -- --ignored --nocapture table::table_time`
#[test]
#[ignore = "benchmark"]
fn table_time() {
    use std::time::Instant;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    fn big_view(s: &App, _: &mut Cx<App>) -> El {
        sized(s, 900.0, 900.0)
    }
    for run in 0..3 {
        let app = App::new(100_000, true);
        let t = rune::time::now();
        let mut rt = Runtime::new(app, big_view, (900.0, 900.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let first = ms(t);
        let mut frames = Vec::new();
        let (mut with_commit, c0) = (Vec::new(), rt.stats.commits);
        for f in 1..=600 {
            rt.wheel(450.0, 450.0, -48.0);
            let c = rt.stats.commits;
            let t = rune::time::now();
            rt.frame(f as f64 / 120.0);
            frames.push(ms(t));
            if rt.stats.commits > c {
                with_commit.push(ms(t));
            }
        }
        println!("  {} commits in 600 scroll frames, p50 {:.3} ms", rt.stats.commits - c0, percentile(&mut with_commit, 0.5));
        let built = find(&rt, |e| e.role == "row").len();
        let commits = rt.stats.commits;
        let mut rebuild = Vec::new();
        for _ in 0..30 {
            rt.invalidate();
            let t = rune::time::now();
            rt.frame(5.0);
            rebuild.push(ms(t));
        }
        println!("  rebuild at rest p50 {:.3} ms, {} commits", percentile(&mut rebuild, 0.5), rt.stats.commits - commits);
        let r = header(&rt, "size").rect;
        let t = rune::time::now();
        click(&mut rt, r[0] + 20.0, r[1] + r[3] * 0.5);
        let sort = ms(t);
        let handle = find(&rt, |e| e.label.as_deref() == Some("Resize Size")).remove(0).rect;
        let (x, y) = (handle[0] + handle[2] * 0.5, handle[1] + handle[3] * 0.5);
        rt.pointer_down(x, y);
        rt.frame(0.0);
        let mut drag = Vec::new();
        for k in 1..=60 {
            rt.pointer_move(x + k as f32, y);
            let t = rune::time::now();
            rt.frame(0.0);
            drag.push(ms(t));
        }
        rt.pointer_up(x + 60.0, y);
        let max = frames.iter().copied().fold(0.0, f64::max);
        println!(
            "table 100k run {run}: first frame {first:.1} ms; scroll p50 {:.3} p95 {:.3} max {max:.3} ms ({built} rows built); sort click {sort:.1} ms; resize drag p50 {:.3} p95 {:.3} ms",
            percentile(&mut frames, 0.5),
            percentile(&mut frames, 0.95),
            percentile(&mut drag, 0.5),
            percentile(&mut drag, 0.95),
        );
    }
}
