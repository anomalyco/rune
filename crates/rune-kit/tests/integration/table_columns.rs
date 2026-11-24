//! Table: `scroll_wide`, `TableColumn` getters and `size_el`, hidden columns in `TableState`,
//! `on_header_secondary_press`, and `on_column_fit`, in a headless runtime.

use std::borrow::Cow;
use std::cell::Cell as StdCell;
use std::rc::Rc;

use rune::accesskit::Action;
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, PointerPress, Runtime, col, row};
use rune_kit::{Cell, Kit, SortDir, Table, TableColumn, TableState, TableStyle};

const W: f32 = 500.0;
const H: f32 = 300.0;
const MIN: f32 = 120.0;

fn find<S>(rt: &Runtime<S>, f: impl Fn(&ElementInfo) -> bool) -> Vec<ElementInfo> {
    rt.elements().into_iter().filter(|e| !e.exiting && f(e)).collect()
}

fn headers<S>(rt: &Runtime<S>) -> Vec<ElementInfo> {
    find(rt, |e| e.role == "columnheader")
}

fn header<S>(rt: &Runtime<S>, id: &str) -> ElementInfo {
    find(rt, |e| e.role == "columnheader" && e.key.as_deref() == Some(id)).remove(0)
}

fn find_a11y<'a>(v: &'a serde_json::Value, role: &str, key: &str) -> Option<&'a serde_json::Value> {
    if v["role"] == role && v["key"] == key {
        return Some(v);
    }
    v["children"].as_array()?.iter().find_map(|c| find_a11y(c, role, key))
}

#[test]
fn scroll_wide_scrolls_wide_columns_horizontally_and_keeps_header_sticky_and_aligned() {
    struct WideApp {
        cols: usize,
        rows: Vec<String>,
        table: TableState,
        scroll_wide: bool,
        w: f32,
        views: Rc<StdCell<usize>>,
    }

    fn view(s: &WideApp, _: &mut Cx<WideApp>) -> El {
        s.views.set(s.views.get() + 1);
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &WideApp| &s.table,
            |s: &mut WideApp| &mut s.table,
            |s: &WideApp| s.rows.len(),
            |s: &WideApp, i| Cow::Borrowed(s.rows[i].as_str()),
        )
        .name("Query results")
        .scroll_wide(s.scroll_wide);
        for c in 0..s.cols {
            t = t.column(TableColumn::new(format!("c{c}"), format!("COLUMN_{c}")).flex(1.0).min(MIN));
        }
        let el = t.cell(|s: &WideApp, i, c| Cell::text(format!("{}:{c}", s.rows[i]))).build(s, &k);
        col().size(s.w, H).child(el.h(H))
    }

    let make = |cols, scroll_wide, w, n| {
        let views = Rc::new(StdCell::new(0));
        let app = WideApp {
            cols,
            rows: (0..n).map(|i| format!("r{i}")).collect(),
            table: TableState::new().key("grid"),
            scroll_wide,
            w,
            views,
        };
        let mut rt = Runtime::new(app, view, (w, H), Env::new(true, 1.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        rt
    };

    // 1. Wide table with scroll_wide(true): 8 * 120 = 960 pt in a 500 pt viewport.
    let mut rt = make(8, true, W, 200);
    let shown = |rt: &Runtime<WideApp>| headers(rt).iter().filter(|e| e.visible[2] >= 1.0).count();
    assert_eq!((shown(&rt), headers(&rt).len()), (5, 8));
    assert_eq!(headers(&rt).last().unwrap().visible[2], 0.0);

    // Sideways wheel brings the last column cleanly into view with zero extra view rebuilds.
    let views_before = rt.state.views.get();
    rt.wheel_xy(250.0, 150.0, -600.0, 0.0);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.views.get(), views_before, "horizontal scroll must not rebuild the view");
    let c7 = header(&rt, "c7");
    assert_eq!(c7.visible[2], MIN, "last header scrolled fully into view");
    let c7_cells = find(&rt, |e| e.role == "gridcell" && e.key.as_deref() == Some("c7"));
    assert!(!c7_cells.is_empty());
    for cell in &c7_cells {
        assert!((cell.rect[0] - c7.rect[0]).abs() < 0.5 && (cell.rect[2] - c7.rect[2]).abs() < 0.5, "body cell aligns with scrolled header");
    }

    // Vertical wheel keeps header sticky at the top with an opaque fill and keeps x alignment.
    let c7_x_before = c7.rect[0];
    rt.wheel_xy(250.0, 150.0, 0.0, -600.0);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let hdr_row = find(&rt, |e| e.role == "row" && e.key.as_deref() == Some("header")).remove(0);
    assert!(hdr_row.rect[1].abs() < 0.5, "header stays stuck at top during vertical scroll: {:?}", hdr_row.rect);
    assert!(hdr_row.fill.a >= 0.99, "sticky wide header has an opaque background fill");
    let c7_after = header(&rt, "c7");
    assert!((c7_after.rect[0] - c7_x_before).abs() < 0.5, "vertical scroll preserves horizontal position");
    let c7_cells_after = find(&rt, |e| e.role == "gridcell" && e.key.as_deref() == Some("c7"));
    assert!(!c7_cells_after.is_empty());
    assert!((c7_cells_after[0].rect[0] - c7_after.rect[0]).abs() < 0.5);

    // Keyboard PageDown uses body height (viewport minus header_height) and preserves horizontal scroll.
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Home, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.table.cursor(), Some("r0"));
    rt.key(KeyEvent { key: Key::PageDown, mods: Mods::default() });
    rt.frame(0.0);
    let page = rt.state.table.cursor().unwrap()[1..].parse::<usize>().unwrap();
    assert!((7..=10).contains(&page), "PageDown accounts for header_height: landed on r{page}");
    assert!((header(&rt, "c7").rect[0] - c7_x_before).abs() < 0.5, "row keyboard nav preserves horizontal scroll");

    // Once settled, an idle frame requests no further work.
    for _ in 0..30 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    assert!(!rt.frame(1.0 / 60.0), "settled wide table goes idle");

    // 2. Default scroll_wide(false) is unchanged: columns past 500 pt are clipped and sideways wheel does nothing.
    let mut rt_default = make(8, false, W, 50);
    assert_eq!(headers(&rt_default).last().unwrap().visible[2], 0.0);
    rt_default.wheel_xy(250.0, 150.0, -600.0, 0.0);
    for _ in 0..90 {
        rt_default.frame(1.0 / 60.0);
    }
    assert_eq!(headers(&rt_default).last().unwrap().visible[2], 0.0, "default scroll_wide(false) does not scroll sideways");
}

#[test]
fn scroll_wide_fills_viewport_when_columns_fit_and_handles_empty_table_and_resize() {
    struct FitApp {
        cols: usize,
        rows: usize,
        table: TableState,
        scroll_wide: bool,
        w: f32,
    }

    fn view(s: &FitApp, _: &mut Cx<FitApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &FitApp| &s.table,
            |s: &mut FitApp| &mut s.table,
            |s: &FitApp| s.rows,
            |_: &FitApp, i| Cow::Owned(format!("r{i}")),
        )
        .scroll_wide(s.scroll_wide);
        for c in 0..s.cols {
            t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).flex(1.0).min(MIN));
        }
        let cell_text = |i: usize, c: &str| {
            if i == 0 && c == "c0" {
                "a very long cell string that should truncate rather than widen scroll_wide content beyond min_total".to_owned()
            } else {
                format!("{i}:{c}")
            }
        };
        col().size(800.0, H).child(col().size(s.w, H).child(t.cell(move |_: &FitApp, i, c| Cell::text(cell_text(i, c))).build(s, &k).h(H)))
    }

    // 3 flex columns (min 360) in an 800 pt viewport fill all 800 pt (long cell text does not blow out width).
    let mut rt = Runtime::new(
        FitApp { cols: 3, rows: 10, table: TableState::new().key("fit"), scroll_wide: true, w: 800.0 },
        view,
        (800.0, H),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.frame(0.0);
    let total_w: f32 = headers(&rt).iter().map(|h| h.rect[2]).sum();
    assert!((total_w - 800.0).abs() < 1.0, "3 flex columns fill 800 pt viewport: {total_w}");

    // Shrinking the table to 600 pt shrinks the columns cleanly to fill 600 pt.
    rt.update(|s, tx| {
        tx.instant();
        s.w = 600.0;
    });
    rt.frame(0.0);
    rt.frame(0.0);
    let total_600: f32 = headers(&rt).iter().map(|h| h.rect[2]).sum();
    assert!((total_600 - 600.0).abs() < 1.0, "columns shrink to fill 600 pt: {total_600}");

    // Shrinking to 300 pt (< 360 min) keeps 120 pt minimums and scrolls sideways.
    rt.update(|s, tx| {
        tx.instant();
        s.w = 300.0;
    });
    rt.frame(0.0);
    rt.frame(0.0);
    let total_300: f32 = headers(&rt).iter().map(|h| h.rect[2]).sum();
    assert!((total_300 - 360.0).abs() < 1.0, "below minimums, columns keep 360 pt total: {total_300}");
    rt.wheel_xy(150.0, 150.0, -200.0, 0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(header(&rt, "c2").visible[2], MIN, "last column scrolls into view");

    // Empty wide table (0 rows) still lays out headers and scrolls horizontally.
    let mut rt_empty = Runtime::new(
        FitApp { cols: 8, rows: 0, table: TableState::new().key("empty-wide"), scroll_wide: true, w: W },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    for _ in 0..4 {
        rt_empty.frame(1.0 / 60.0);
    }
    rt_empty.wheel_xy(250.0, 15.0, -600.0, 0.0);
    for _ in 0..90 {
        rt_empty.frame(1.0 / 60.0);
    }
    assert_eq!(header(&rt_empty, "c7").visible[2], MIN, "empty wide table scrolls horizontally");
}

#[test]
fn scroll_wide_expands_on_resized_column_and_skips_hidden_columns_in_min_total() {
    struct QueryApp {
        table: TableState,
    }

    fn view(s: &QueryApp, _: &mut Cx<QueryApp>) -> El {
        let k = Kit::neutral(true);
        let t = Table::new(
            |s: &QueryApp| &s.table,
            |s: &mut QueryApp| &mut s.table,
            |_: &QueryApp| 20,
            |_: &QueryApp, i| Cow::Owned(format!("q{i}")),
        )
        .column(TableColumn::new("num", "Num").width(80.0))
        .column(TableColumn::new("flag", "Flag").width(72.0))
        .column(TableColumn::new("date", "Date").width(104.0))
        .column(TableColumn::new("dt", "Datetime").width(140.0))
        .column(TableColumn::new("text", "Text").flex(1.0).min(120.0))
        .cell(|_: &QueryApp, i, col| Cell::text(format!("{col}:{i}")))
        .scroll_wide(true)
        .build(s, &k);
        col().size(600.0, H).child(t.h(H))
    }

    // Initial total min is 80+72+104+140+120 = 516 <= 600: fits with no overflow.
    let mut rt = Runtime::new(QueryApp { table: TableState::new().key("q") }, view, (600.0, H), Env::new(true, 1.0));
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
    assert!((header(&rt, "text").rect[2] - 204.0).abs() < 1.0);

    // Widening "text" to 400 makes total 796 > 600: becomes horizontally scrollable.
    rt.update(|s, tx| {
        tx.instant();
        s.table.set_column_width("text", 400.0);
    });
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(header(&rt, "text").rect[2], 400.0);
    assert!(header(&rt, "text").visible[2] < 400.0, "widened column overflows 600 pt viewport");
    rt.wheel_xy(300.0, 150.0, -400.0, 0.0);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let text_hdr = header(&rt, "text");
    assert!((text_hdr.rect[0] + text_hdr.rect[2] - 600.0).abs() < 1.0, "scrolled right edge of widened column into view: {:?}", text_hdr.rect);

    // Hiding "date" (104) and "dt" (140) drops visible total to 80+72+400 = 552 <= 600:
    // scroll_wide skips hidden columns, so all remaining columns fit with x starting at 0.
    rt.update(|s, tx| {
        tx.instant();
        s.table.hide_column("date");
        s.table.hide_column("dt");
    });
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(headers(&rt).len(), 3);
    assert!(header(&rt, "num").rect[0].abs() < 0.5, "hidden columns excluded from scrollable width: {:?}", header(&rt, "num").rect);
    let text_after = header(&rt, "text");
    assert!((text_after.rect[0] - 152.0).abs() < 0.5 && (text_after.rect[2] - 400.0).abs() < 0.5);
}

#[test]
fn column_size_el_matches_table_column_layout_with_and_without_override() {
    struct SizeApp {
        table: TableState,
    }

    fn columns() -> Vec<TableColumn> {
        vec![
            TableColumn::new("name", "Name").flex(2.0).min(100.0),
            TableColumn::new("kind", "Kind").flex(1.0).min(80.0).max(150.0),
            TableColumn::new("size", "Size").width(90.0).min(60.0).max(160.0),
        ]
    }

    fn view(s: &SizeApp, _: &mut Cx<SizeApp>) -> El {
        let k = Kit::neutral(true);
        let cols = columns();
        let pad = TableStyle::from_kit(&k).cell_pad;
        let mut custom = row().key("custom").w_full().h(28.0);
        for c in &cols {
            custom = custom.child(c.size_el(row().key(format!("custom-{}", c.id())).h_full().px(pad), s.table.column_width(c.id())));
        }
        let mut t = Table::new(
            |s: &SizeApp| &s.table,
            |s: &mut SizeApp| &mut s.table,
            |_: &SizeApp| 2,
            |_: &SizeApp, i| Cow::Owned(format!("r{i}")),
        );
        for c in cols {
            t = t.column(c);
        }
        col().size(W, H).child(custom).child(t.cell(|_: &SizeApp, _, c| Cell::text(c)).build(s, &k).h(H - 28.0))
    }

    let mut rt = Runtime::new(SizeApp { table: TableState::new() }, view, (W, H), Env::new(true, 1.0));
    rt.frame(0.0);
    for id in ["name", "kind", "size"] {
        let th = header(&rt, id).rect;
        let ch = find(&rt, |e| e.key.as_deref() == Some(&format!("custom-{id}"))).remove(0).rect;
        assert!((th[0] - ch[0]).abs() < 0.5 && (th[2] - ch[2]).abs() < 0.5, "default {id}: table {th:?} vs size_el {ch:?}");
    }

    // Override width beyond max clamps identically in Table and TableColumn::size_el.
    rt.update(|s, tx| {
        tx.instant();
        s.table.set_column_width("size", 999.0);
    });
    rt.frame(0.0);
    for id in ["name", "kind", "size"] {
        let th = header(&rt, id).rect;
        let ch = find(&rt, |e| e.key.as_deref() == Some(&format!("custom-{id}"))).remove(0).rect;
        assert!((th[0] - ch[0]).abs() < 0.5 && (th[2] - ch[2]).abs() < 0.5, "override {id}: table {th:?} vs size_el {ch:?}");
    }
    assert_eq!(header(&rt, "size").rect[2], 160.0);
}

#[test]
fn hidden_columns_omit_headers_cells_and_a11y_and_restore_widths_when_shown() {
    struct VisApp {
        table: TableState,
        asked_cols: Rc<StdCell<usize>>,
    }

    fn view(s: &VisApp, _: &mut Cx<VisApp>) -> El {
        let k = Kit::neutral(true);
        let asked = s.asked_cols.clone();
        col().size(W, H).child(
            Table::new(
                |s: &VisApp| &s.table,
                |s: &mut VisApp| &mut s.table,
                |_: &VisApp| 4,
                |_: &VisApp, i| Cow::Owned(format!("r{i}")),
            )
            .name("Files")
            .column(TableColumn::new("name", "Name").flex(1.0).sortable())
            .column(TableColumn::new("size", "Size").width(100.0).min(60.0).max(200.0).sortable())
            .column(TableColumn::new("kind", "Kind").width(80.0).resizable(false))
            .cell(move |_: &VisApp, i, col| {
                if col == "size" {
                    asked.set(asked.get() + 1);
                }
                Cell::text(format!("{col}-{i}"))
            })
            .build(s, &k)
            .h(H),
        )
    }

    let asked = Rc::new(StdCell::new(0));
    let mut table = TableState::new();
    table.set_column_width("size", 145.0);
    table.set_sort("size", SortDir::Ascending);
    let mut rt = Runtime::new(VisApp { table, asked_cols: asked.clone() }, view, (W, H), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(header(&rt, "size").rect[2], 145.0);
    assert!(asked.get() > 0);

    // Hide "size": header, body cells, cell callback and a11y node disappear.
    asked.set(0);
    rt.update(|s, _| s.table.hide_column("size"));
    rt.frame(0.0);
    assert_eq!(asked.get(), 0, "cell callback is not called for hidden column");
    assert!(find(&rt, |e| e.role == "columnheader" && e.key.as_deref() == Some("size")).is_empty());
    assert!(find(&rt, |e| e.role == "gridcell" && e.key.as_deref() == Some("size")).is_empty());
    assert!(find_a11y(&rt.accessibility_json(), "column-header", "size").is_none());
    assert_eq!(rt.state.table.column_width("size"), Some(145.0), "resized width preserved while hidden");

    // Show "size" again: returns at resized width 145.0 with sort indicator intact.
    rt.update(|s, _| s.table.show_column("size"));
    rt.frame(0.0);
    assert_eq!(header(&rt, "size").rect[2], 145.0);
    assert_eq!(find_a11y(&rt.accessibility_json(), "column-header", "size").unwrap()["sort"], "ascending");

    // Edge case: hiding every column renders without panicking.
    rt.update(|s, _| {
        s.table.set_column_hidden("name", true);
        s.table.set_column_hidden("size", true);
        s.table.set_column_hidden("kind", true);
    });
    rt.frame(0.0);
    assert!(headers(&rt).is_empty());
    assert!(find(&rt, |e| e.role == "gridcell").is_empty());
    rt.update(|s, _| s.table.set_column_hidden("name", false));
    rt.frame(0.0);
    assert_eq!(headers(&rt).len(), 1);
}

#[test]
fn header_secondary_press_delivers_column_id_and_point_and_exposes_a11y_action() {
    struct MenuApp {
        table: TableState,
        hook: bool,
        header_menus: Vec<(String, PointerPress)>,
        outer_menus: usize,
        sorts: Vec<(String, SortDir)>,
    }

    fn view(s: &MenuApp, _: &mut Cx<MenuApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &MenuApp| &s.table,
            |s: &mut MenuApp| &mut s.table,
            |_: &MenuApp| 3,
            |_: &MenuApp, i| Cow::Owned(format!("r{i}")),
        )
        .name("Files")
        .column(TableColumn::new("name", "Name").flex(1.0).sortable())
        .column(TableColumn::new("size", "Size").width(100.0).sortable())
        .column(TableColumn::new("kind", "Kind").width(80.0).resizable(false))
        .cell(|_: &MenuApp, i, col| Cell::text(format!("{col}-{i}")))
        .on_sort(|s: &mut MenuApp, col, dir, _| s.sorts.push((col.to_owned(), dir)));
        if s.hook {
            t = t.on_header_secondary_press(|s: &mut MenuApp, col, press, _| {
                s.header_menus.push((col.to_owned(), press));
            });
        }
        col()
            .size(W, H)
            .on_secondary_press(|s: &mut MenuApp, _, _| s.outer_menus += 1)
            .child(t.build(s, &k).h(H))
    }

    let mut rt = Runtime::new(
        MenuApp { table: TableState::new(), hook: true, header_menus: Vec::new(), outer_menus: 0, sorts: Vec::new() },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Secondary-click sortable header "size".
    let size = header(&rt, "size").rect;
    let (sx, sy) = (size[0] + 25.0, size[1] + size[3] * 0.5);
    rt.secondary_down(sx, sy);
    rt.secondary_up(sx, sy);
    rt.frame(0.0);
    assert_eq!(rt.state.header_menus, [("size".to_owned(), PointerPress { x: sx, y: sy })]);
    assert_eq!(rt.state.outer_menus, 0, "header hook consumes the secondary press");
    assert!(rt.state.sorts.is_empty(), "secondary press does not trigger sort");

    // Secondary-click non-sortable, non-resizable header "kind".
    let kind = header(&rt, "kind").rect;
    let (kx, ky) = (kind[0] + 20.0, kind[1] + kind[3] * 0.5);
    rt.secondary_down(kx, ky);
    rt.secondary_up(kx, ky);
    rt.frame(0.0);
    assert_eq!(rt.state.header_menus.last(), Some(&("kind".to_owned(), PointerPress { x: kx, y: ky })));

    // Primary click still sorts.
    rt.pointer_down(sx, sy);
    rt.pointer_up(sx, sy);
    rt.frame(0.0);
    assert_eq!(rt.state.sorts, [("size".to_owned(), SortDir::Ascending)]);

    // Accessibility: columnheader exposes context-menu and runs on_header_secondary_press.
    let tree = rt.accessibility_json();
    let size_a11y = find_a11y(&tree, "column-header", "size").expect("size column-header in a11y");
    let actions = size_a11y["actions"].as_array().expect("header actions");
    assert!(actions.iter().any(|a| a == "context-menu"), "columnheader exposes context-menu: {actions:?}");
    let before = rt.state.header_menus.len();
    assert!(rt.accessibility_action("size", Action::ShowContextMenu, None));
    rt.frame(0.0);
    assert_eq!(rt.state.header_menus.len(), before + 1);
    assert_eq!(rt.state.header_menus.last().unwrap().0, "size");

    // Without on_header_secondary_press: no context-menu action on headers, and secondary press bubbles to outer element.
    rt.update(|s, _| s.hook = false);
    rt.frame(0.0);
    let plain_tree = rt.accessibility_json();
    let plain_hdr = find_a11y(&plain_tree, "column-header", "size").unwrap();
    assert!(plain_hdr["actions"].as_array().is_none_or(|a| !a.iter().any(|x| x == "context-menu")));
    rt.secondary_down(sx, sy);
    rt.secondary_up(sx, sy);
    rt.frame(0.0);
    assert_eq!(rt.state.outer_menus, 1, "without header hook, secondary press bubbles to outer container");
}

#[test]
fn on_column_fit_overrides_edge_double_click_and_unset_resets_width() {
    struct FitHookApp {
        table: TableState,
        use_fit: bool,
        fitted: Vec<String>,
    }

    fn view(s: &FitHookApp, _: &mut Cx<FitHookApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &FitHookApp| &s.table,
            |s: &mut FitHookApp| &mut s.table,
            |_: &FitHookApp| 4,
            |_: &FitHookApp, i| Cow::Owned(format!("r{i}")),
        )
        .column(TableColumn::new("name", "Name").flex(1.0))
        .column(TableColumn::new("size", "Size").width(100.0).min(60.0).max(200.0))
        .cell(|_: &FitHookApp, i, col| Cell::text(format!("{col}-{i}")));
        if s.use_fit {
            t = t.on_column_fit(|s: &mut FitHookApp, col, _| {
                s.fitted.push(col.to_owned());
                s.table.set_column_width(col, 165.0);
            });
        }
        col().size(W, H).child(t.build(s, &k).h(H))
    }

    let double_click_resize = |rt: &mut Runtime<FitHookApp>| {
        let handle = find(rt, |e| e.label.as_deref() == Some("Resize Size")).remove(0).rect;
        let (x, y) = (handle[0] + handle[2] * 0.5, handle[1] + handle[3] * 0.5);
        for _ in 0..2 {
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        }
        rt.frame(0.0);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
    };

    // 1. Default (on_column_fit unset): double-clicking edge resets column width.
    let mut table = TableState::new();
    table.set_column_width("size", 140.0);
    let mut rt = Runtime::new(FitHookApp { table, use_fit: false, fitted: Vec::new() }, view, (W, H), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(header(&rt, "size").rect[2], 140.0);
    double_click_resize(&mut rt);
    assert_eq!(rt.state.table.column_width("size"), None, "unset on_column_fit resets width");
    assert_eq!(header(&rt, "size").rect[2], 100.0);

    // 2. With on_column_fit set: double-clicking edge invokes the fit hook instead of resetting.
    rt.update(|s, _| {
        s.use_fit = true;
        s.table.set_column_width("size", 140.0);
    });
    rt.frame(0.0);
    double_click_resize(&mut rt);
    assert_eq!(rt.state.fitted, ["size"]);
    assert_eq!(rt.state.table.column_width("size"), Some(165.0));
    assert_eq!(header(&rt, "size").rect[2], 165.0);
}

#[test]
fn scroll_wide_preserves_virtualization_and_stick_bottom_on_large_table() {
    struct BigWideApp {
        count: usize,
        table: TableState,
    }

    fn view(s: &BigWideApp, _: &mut Cx<BigWideApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &BigWideApp| &s.table,
            |s: &mut BigWideApp| &mut s.table,
            |s: &BigWideApp| s.count,
            |_: &BigWideApp, i| Cow::Owned(format!("r{i}")),
        )
        .scroll_wide(true)
        .stick_bottom(true);
        for c in 0..8 {
            t = t.column(TableColumn::new(format!("c{c}"), format!("C{c}")).flex(1.0).min(MIN));
        }
        col().size(W, H).child(t.cell(|_: &BigWideApp, i, c| Cell::text(format!("{i}:{c}"))).build(s, &k).h(H))
    }

    let mut rt = Runtime::new(BigWideApp { count: 10_000, table: TableState::new().key("big-wide") }, view, (W, H), Env::new(true, 1.0));
    for f in 0..10 {
        rt.frame(f as f64 / 60.0);
    }
    let mounted = find(&rt, |e| e.role == "row").len();
    assert!((5..80).contains(&mounted), "10,000-row scroll_wide table only mounts visible window: {mounted}");
    assert_eq!(rt.scroll_at_bottom("viewport"), Some(true));
    assert!(!find(&rt, |e| e.role == "row" && e.key.as_deref() == Some("r9999") && e.visible[3] > 20.0).is_empty());

    // Appending rows keeps stick_bottom pinned and horizontal scrolling still works.
    rt.update(|s, _| s.count = 10_020);
    for f in 0..10 {
        rt.frame(1.0 + f as f64 / 60.0);
    }
    assert_eq!(rt.scroll_at_bottom("viewport"), Some(true));
    assert!(!find(&rt, |e| e.role == "row" && e.key.as_deref() == Some("r10019") && e.visible[3] > 20.0).is_empty());

    rt.wheel_xy(250.0, 150.0, -600.0, 0.0);
    for f in 0..90 {
        rt.frame(2.0 + f as f64 / 60.0);
    }
    assert_eq!(header(&rt, "c7").visible[2], MIN, "stick_bottom + scroll_wide scrolls sideways too");
}
