//! Table: `Table::virtual_columns`, `Table::frozen_columns`, ARIA grid coordinates,
//! focus/caret-browsing pinning across horizontal scroll, resize/reorder while windowed,
//! and zero-`view`-run vertical `scroll_xy` remounts, in a headless runtime.

use std::borrow::Cow;
use std::cell::Cell as StdCell;
use std::rc::Rc;

use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, TextStyle, col, text, text_input};
use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
use serde_json::Value;

const W: f32 = 800.0;
const H: f32 = 360.0;
const COL_W: f32 = 100.0;

fn find<S>(rt: &Runtime<S>, f: impl Fn(&ElementInfo) -> bool) -> Vec<ElementInfo> {
    rt.elements().into_iter().filter(|e| !e.exiting && f(e)).collect()
}

fn headers<S>(rt: &Runtime<S>) -> Vec<ElementInfo> {
    find(rt, |e| e.role == "columnheader")
}

fn header<S>(rt: &Runtime<S>, id: &str) -> ElementInfo {
    find(rt, |e| e.role == "columnheader" && e.key.as_deref() == Some(id))
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("missing columnheader {id}"))
}

fn cells_for<S>(rt: &Runtime<S>, col_id: &str) -> Vec<ElementInfo> {
    find(rt, |e| e.role == "gridcell" && e.key.as_deref() == Some(col_id))
}

fn find_a11y<'a>(v: &'a Value, role: &str, key: &str) -> Option<&'a Value> {
    if v["role"] == role && v["key"] == key {
        return Some(v);
    }
    v["children"].as_array()?.iter().find_map(|c| find_a11y(c, role, key))
}

fn any_a11y_key(v: &Value, prefix: &str) -> bool {
    if v["key"].as_str().is_some_and(|k| k.starts_with(prefix)) {
        return true;
    }
    v["children"].as_array().is_some_and(|kids| kids.iter().any(|c| any_a11y_key(c, prefix)))
}

fn settle<S: 'static>(rt: &mut Runtime<S>, max_frames: usize) {
    for i in 0..max_frames {
        let anim = rt.frame(1.0 / 60.0);
        if i >= 3 && !anim {
            break;
        }
    }
}

#[test]
fn defaults_off_leave_cells_visuals_and_accessibility_tree_unchanged() {
    struct App {
        table: TableState,
        explicit_defaults: bool,
        cells_built: Rc<StdCell<usize>>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = Kit::neutral(true);
        let built = Rc::clone(&s.cells_built);
        let mut t = Table::new(
            |s: &App| &s.table,
            |s: &mut App| &mut s.table,
            |_: &App| 6,
            |_: &App, i| Cow::Owned(format!("r{i}")),
        )
        .name("Plain")
        .scroll_wide(true);
        if s.explicit_defaults {
            t = t.virtual_columns(false).frozen_columns(0);
        }
        for c in 0..12 {
            t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).width(COL_W));
        }
        let el = t
            .cell(move |_: &App, i, col| {
                built.set(built.get() + 1);
                Cell::text(format!("{i}:{col}"))
            })
            .build(s, &k);
        col().size(W, H).child(el.h(H))
    }

    let make = |explicit_defaults| {
        let built = Rc::new(StdCell::new(0));
        let mut rt = Runtime::new(
            App { table: TableState::new().key("grid"), explicit_defaults, cells_built: Rc::clone(&built) },
            view,
            (W, H),
            Env::new(true, 1.0),
        );
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        (rt, built)
    };

    let (rt_plain, built_plain) = make(false);
    let (rt_explicit, built_explicit) = make(true);
    assert_eq!(built_plain.get(), built_explicit.get());
    assert_eq!(headers(&rt_plain).len(), 12);
    assert_eq!(headers(&rt_explicit).len(), 12);
    assert_eq!(find(&rt_plain, |e| e.role == "gridcell").len(), 6 * 12);
    assert_eq!(find(&rt_explicit, |e| e.role == "gridcell").len(), 6 * 12);
    let a11y_plain = rt_plain.accessibility_json();
    assert_eq!(a11y_plain, rt_explicit.accessibility_json());
    let grid = find_a11y(&a11y_plain, "grid", "grid").unwrap();
    assert!(grid["columnCount"].is_null(), "defaults off must not alter a11y tree");
}

#[test]
fn wide_10k_by_200_table_windows_columns_aligns_headers_keeps_frozen_pinned_and_zero_view_vertical_scroll() {
    struct BigApp {
        table: TableState,
        rows: usize,
        cols: usize,
        frozen: usize,
        virtual_cols: bool,
        views: Rc<StdCell<usize>>,
        cells_built: Rc<StdCell<usize>>,
    }

    fn view(s: &BigApp, _: &mut Cx<BigApp>) -> El {
        s.views.set(s.views.get() + 1);
        let k = Kit::neutral(true);
        let built = Rc::clone(&s.cells_built);
        let mut t = Table::new(
            |s: &BigApp| &s.table,
            |s: &mut BigApp| &mut s.table,
            |s: &BigApp| s.rows,
            |_: &BigApp, i| Cow::Owned(format!("r{i}")),
        )
        .name("Metrics")
        .zebra(true)
        .scroll_wide(true)
        .virtual_columns(s.virtual_cols)
        .frozen_columns(s.frozen)
        .virtual_overscan(0.5, 0.25);
        for c in 0..s.cols {
            t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).width(COL_W));
        }
        let el = t
            .cell(move |_: &BigApp, i, col| {
                built.set(built.get() + 1);
                Cell::text(format!("r{i}:{col}"))
            })
            .build(s, &k);
        col().size(W, H).child(el.h(H))
    }

    let views = Rc::new(StdCell::new(0));
    let built = Rc::new(StdCell::new(0));
    let mut rt = Runtime::new(
        BigApp {
            table: TableState::new().key("big"),
            rows: 10_000,
            cols: 200,
            frozen: 2,
            virtual_cols: true,
            views: Rc::clone(&views),
            cells_built: Rc::clone(&built),
        },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    settle(&mut rt, 10);

    // 1. Bounded mounted headers and cells (far fewer than 200 columns).
    let hdrs = headers(&rt);
    let mounted_rows = find(&rt, |e| e.role == "row" && e.key.as_deref() != Some("header")).len();
    let mounted_cells = find(&rt, |e| e.role == "gridcell").len();
    assert!(hdrs.len() <= 20, "mounted headers bounded to viewport + overscan + frozen: {}", hdrs.len());
    assert_eq!(mounted_cells, mounted_rows * hdrs.len());
    assert!(mounted_cells < mounted_rows * 25, "mounted cells an order of magnitude below 200 cols: {mounted_cells}");

    // 2. Small horizontal scroll within hysteresis band produces zero live_only warnings, and
    // with the debug live_only verifier off (as in release), triggers zero view runs.
    rt.wheel_xy(400.0, 180.0, -40.0, 0.0);
    settle(&mut rt, 60);
    assert!(rt.live_only_warnings().is_empty(), "live_only warnings: {:?}", rt.live_only_warnings());
    rt.check_live_only(false);
    let views_before_small = views.get();
    rt.wheel_xy(400.0, 180.0, -40.0, 0.0);
    settle(&mut rt, 60);
    assert_eq!(views.get(), views_before_small, "within-band horizontal scroll triggers zero view runs");

    // 3. Large horizontal scroll brings distant columns in, aligns headers with cells, and pins frozen cols.
    rt.update(|_, tx| tx.scroll_to_x("big/viewport", 8000.0));
    settle(&mut rt, 10);
    let hdrs_mid = headers(&rt);
    assert!(hdrs_mid.len() <= 24, "windowed headers stay bounded after scroll: {}", hdrs_mid.len());
    assert!(hdrs_mid.iter().any(|h| h.key.as_deref() == Some("c83")), "distant column c83 mounted");
    assert!(!hdrs_mid.iter().any(|h| h.key.as_deref() == Some("c10")), "offscreen non-frozen c10 unmounted");

    // Frozen columns 0 and 1 stay pinned at left edge (0 and 100 pt) with opaque fills.
    let h0 = header(&rt, "c0");
    let h1 = header(&rt, "c1");
    assert!(h0.rect[0].abs() < 0.5 && (h1.rect[0] - COL_W).abs() < 0.5, "frozen headers stay pinned: {:?}, {:?}", h0.rect, h1.rect);
    assert!(h0.fill.a >= 0.99 && h1.fill.a >= 0.99, "frozen headers have opaque fills");
    for fc in ["c0", "c1"] {
        let fh = header(&rt, fc);
        let fcells = cells_for(&rt, fc);
        assert!(!fcells.is_empty());
        for cell in &fcells {
            assert!((cell.rect[0] - fh.rect[0]).abs() < 0.5, "frozen cell {fc} aligned with header");
            assert!(cell.fill.a >= 0.99, "frozen body cell {fc} has opaque fill: {:?}", cell.fill);
        }
    }

    // Scrolled non-frozen column c83 aligns with its header within 0.5 pt.
    let h83 = header(&rt, "c83");
    assert!((h83.rect[0] - 300.0).abs() < 0.5, "c83 sits at 8300 - 8000 = 300 pt: {:?}", h83.rect);
    let c83_cells = cells_for(&rt, "c83");
    assert!(!c83_cells.is_empty());
    for cell in &c83_cells {
        assert!((cell.rect[0] - h83.rect[0]).abs() < 0.5 && (cell.rect[2] - h83.rect[2]).abs() < 0.5);
    }

    // 4. Vertical scroll while horizontally scrolled takes Remount with zero view runs and keeps header stuck at top.
    let views_before_v = views.get();
    rt.wheel_xy(400.0, 180.0, 0.0, -1200.0);
    settle(&mut rt, 90);
    assert_eq!(views.get(), views_before_v, "vertical scroll inside scroll_wide table must run zero views");
    let hdr_row = find(&rt, |e| e.role == "row" && e.key.as_deref() == Some("header")).remove(0);
    assert!(hdr_row.rect[1].abs() < 0.5, "header stays sticky at top: {:?}", hdr_row.rect);
    let h0_v = header(&rt, "c0");
    assert!(h0_v.rect[0].abs() < 0.5 && h0_v.rect[1].abs() < 0.5, "frozen header corner stays pinned in both axes: {:?}", h0_v.rect);
    let h83_v = header(&rt, "c83");
    for cell in cells_for(&rt, "c83") {
        assert!((cell.rect[0] - h83_v.rect[0]).abs() < 0.5, "body cells stay aligned after vertical scroll");
    }

    // 5. Accessibility tree (AccessKit JSON + web mirror JSON) reports true counts and 1-based indices.
    let a11y = rt.accessibility_json();
    assert!(!any_a11y_key(&a11y, "__rune_sp_"), "spacers stay hidden from accessibility tree");
    let grid = find_a11y(&a11y, "grid", "big").expect("grid a11y node");
    assert_eq!(grid["rowCount"], 10_000);
    assert_eq!(grid["columnCount"], 200);
    let a_h0 = find_a11y(&a11y, "column-header", "c0").expect("c0 header");
    let a_h1 = find_a11y(&a11y, "column-header", "c1").expect("c1 header");
    let a_h83 = find_a11y(&a11y, "column-header", "c83").expect("c83 header");
    assert_eq!(a_h0["columnIndex"], 1);
    assert_eq!(a_h1["columnIndex"], 2);
    assert_eq!(a_h83["columnIndex"], 84, "c83 is 1-based column 84 even when leading columns are unmounted");
    let any_row = find(&rt, |e| e.role == "row" && e.key.as_deref() != Some("header")).remove(0);
    let row_key = any_row.key.as_deref().unwrap();
    let row_num: u64 = row_key[1..].parse().unwrap();
    let a_row = find_a11y(&a11y, "row", row_key).expect("mounted row in a11y");
    assert_eq!(a_row["rowIndex"], row_num + 1);
    let a_cell83 = find_a11y(a_row, "grid-cell", "c83").expect("c83 cell in a11y");
    assert_eq!(a_cell83["columnIndex"], 84);

    // Web mirror JSON carries rc, cc, ri, ci for ARIA parity.
    let web_items: Vec<Value> = serde_json::from_str(&rt.a11y_json()).expect("valid web mirror json");
    let web_grid = web_items.iter().find(|v| v.get("o").and_then(Value::as_str) == Some("grid")).expect("web grid");
    assert_eq!(web_grid["rc"], 10_000);
    assert_eq!(web_grid["cc"], 200);
    let web_h83 = web_items.iter().find(|v| v.get("o").and_then(Value::as_str) == Some("columnheader") && v["t"] == "Col 83").expect("web c83 header");
    assert_eq!(web_h83["ci"], 84);

    // 6. Settled idle check: requests no further frames and produces zero live_only warnings.
    settle(&mut rt, 30);
    assert!(!rt.frame(1.0 / 60.0), "settled virtual-column table goes idle");
    assert!(rt.live_only_warnings().is_empty(), "live_only warnings: {:?}", rt.live_only_warnings());
}

#[test]
fn frozen_columns_work_standalone_without_virtual_columns() {
    struct App { table: TableState }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &App| &s.table,
            |s: &mut App| &mut s.table,
            |_: &App| 30,
            |_: &App, i| Cow::Owned(format!("r{i}")),
        )
        .scroll_wide(true)
        .frozen_columns(2)
        .zebra(true);
        for c in 0..16 {
            t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).width(COL_W));
        }
        col().size(W, H).child(t.cell(|_: &App, i, c| Cell::text(format!("{i}:{c}"))).build(s, &k).h(H))
    }
    let mut rt = Runtime::new(App { table: TableState::new().key("frz") }, view, (W, H), Env::new(true, 1.0));
    settle(&mut rt, 10);
    assert_eq!(headers(&rt).len(), 16, "without virtual_columns all 16 columns mount");
    rt.update(|_, tx| tx.scroll_to_x("frz/viewport", 500.0));
    settle(&mut rt, 10);
    assert!(header(&rt, "c0").rect[0].abs() < 0.5);
    assert!((header(&rt, "c1").rect[0] - COL_W).abs() < 0.5);
    assert!((header(&rt, "c7").rect[0] - 200.0).abs() < 0.5);
    assert!(cells_for(&rt, "c0")[0].fill.a >= 0.99);
}

#[test]
fn keyboard_focus_and_caret_browsing_survive_horizontal_scroll_out_of_view() {
    struct FocusApp {
        table: TableState,
        edit: TextEdit,
        mode_caret: bool,
    }

    fn view(s: &FocusApp, _: &mut Cx<FocusApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &FocusApp| &s.table,
            |s: &mut FocusApp| &mut s.table,
            |_: &FocusApp| 4,
            |_: &FocusApp, i| Cow::Owned(format!("r{i}")),
        )
        .scroll_wide(true)
        .virtual_columns(true)
        .frozen_columns(1);
        for c in 0..80 {
            t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).width(COL_W));
        }
        let mode_caret = s.mode_caret;
        let el = t
            .cell(move |s: &FocusApp, i, col| {
                if i == 0 && col == "c3" {
                    if mode_caret {
                        Cell::el(
                            text("alpha beta gamma")
                                .key("caret-cell")
                                .font(TextStyle::new(13.0, 400.0))
                                .selectable(true)
                                .caret_browsing(true),
                        )
                    } else {
                        Cell::el(
                            text_input(&s.edit, TextStyle::new(13.0, 400.0))
                                .key("cell-input")
                                .label("Cell input")
                                .w_full()
                                .on_edit(|s: &mut FocusApp| &mut s.edit),
                        )
                    }
                } else {
                    Cell::text(format!("{i}:{col}"))
                }
            })
            .build(s, &k);
        col().size(W, H).child(el.h(H))
    }

    // Part A: Focused TextInput inside cell c3 stays focused and accepts typing after scrolling far right.
    let mut rt = Runtime::new(
        FocusApp { table: TableState::new().key("foc"), edit: TextEdit::new("hi"), mode_caret: false },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    settle(&mut rt, 10);
    rt.update(|_, tx| tx.focus("cell-input", true));
    settle(&mut rt, 6);
    assert!(rt.state.edit.focused, "cell input focused");

    // Scroll far right so c3 is thousands of points outside the horizontal window.
    rt.update(|_, tx| tx.scroll_to_x("foc/viewport", 5000.0));
    settle(&mut rt, 10);
    assert!(rt.state.edit.focused, "focused cell input must not lose focus when scrolled horizontally out of view");
    assert!(headers(&rt).iter().any(|h| h.key.as_deref() == Some("c3")), "focused column c3 stays pinned");
    assert!(!headers(&rt).iter().any(|h| h.key.as_deref() == Some("c4")), "adjacent unfocused c4 is unmounted");
    rt.key(KeyEvent { key: Key::Text("!".into()), mods: Mods::default() });
    settle(&mut rt, 4);
    assert!(rt.state.edit.text.contains('!'), "typing still reaches offscreen focused cell: {:?}", rt.state.edit.text);

    // Scroll far right again while still focused, then blur: releasing the pin unmounts c3.
    rt.update(|_, tx| tx.scroll_to_x("foc/viewport", 5000.0));
    settle(&mut rt, 10);
    assert!(headers(&rt).iter().any(|h| h.key.as_deref() == Some("c3")), "c3 remains pinned while focused");
    rt.update(|_, tx| tx.blur());
    settle(&mut rt, 10);
    assert!(!headers(&rt).iter().any(|h| h.key.as_deref() == Some("c3")), "blurred offscreen column c3 unmounts");

    // Part B: Caret browsing inside cell c3 survives horizontal scroll out of view.
    let mut crt = Runtime::new(
        FocusApp { table: TableState::new().key("car"), edit: TextEdit::new(""), mode_caret: true },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    settle(&mut crt, 10);
    crt.update(|_, tx| tx.focus("caret-cell", true));
    settle(&mut crt, 6);
    // Select first 5 characters ("alpha") via Shift+Right.
    let shift = Mods { shift: true, ..Mods::default() };
    for _ in 0..5 {
        crt.key(KeyEvent { key: Key::Right, mods: shift });
        crt.frame(0.0);
    }
    assert_eq!(crt.selected_text().as_deref(), Some("alpha"));

    // Scroll far right: c3 stays pinned so caret browsing focus and selection survive.
    crt.update(|_, tx| tx.scroll_to_x("car/viewport", 5000.0));
    settle(&mut crt, 10);
    assert!(headers(&crt).iter().any(|h| h.key.as_deref() == Some("c3")), "caret-browsing column c3 stays pinned");
    assert_eq!(crt.selected_text().as_deref(), Some("alpha"), "caret selection survives horizontal scroll");
    // Extending selection by one more character still works while scrolled out of view.
    crt.key(KeyEvent { key: Key::Right, mods: shift });
    crt.frame(0.0);
    assert_eq!(crt.selected_text().as_deref(), Some("alpha "), "caret navigation still works while scrolled out of view");
}

#[test]
fn resize_and_reorder_work_while_virtualized_and_with_frozen_columns() {
    struct EditApp {
        table: TableState,
        reorders: Vec<(usize, usize)>,
    }

    fn view(s: &EditApp, _: &mut Cx<EditApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &EditApp| &s.table,
            |s: &mut EditApp| &mut s.table,
            |_: &EditApp| 10,
            |_: &EditApp, i| Cow::Owned(format!("r{i}")),
        )
        .scroll_wide(true)
        .virtual_columns(true)
        .frozen_columns(2)
        .on_reorder(|s: &mut EditApp, from, to, _| s.reorders.push((from, to)));
        for c in 0..60 {
            t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).width(COL_W).min(60.0).max(240.0));
        }
        col().size(W, H).child(t.cell(|_: &EditApp, i, c| Cell::text(format!("{i}:{c}"))).build(s, &k).h(H))
    }

    let mut rt = Runtime::new(
        EditApp { table: TableState::new().key("ed"), reorders: Vec::new() },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    settle(&mut rt, 10);

    // Scroll into the middle of the 60 columns.
    rt.update(|_, tx| tx.scroll_to_x("ed/viewport", 2500.0));
    settle(&mut rt, 10);

    // 1. Resize frozen column c0 by +40 pt: c1 shifts to x = 140 pt and cells follow.
    let h0 = header(&rt, "c0").rect;
    let edge_x = h0[0] + h0[2] - 1.0;
    let mid_y = h0[1] + h0[3] * 0.5;
    rt.pointer_down(edge_x, mid_y);
    rt.pointer_move(edge_x + 40.0, mid_y);
    rt.pointer_up(edge_x + 40.0, mid_y);
    settle(&mut rt, 6);
    assert!((header(&rt, "c0").rect[2] - 140.0).abs() < 1.0, "frozen c0 resized to 140");
    assert!((header(&rt, "c1").rect[0] - 140.0).abs() < 1.0, "frozen c1 shifted to 140");
    assert!((cells_for(&rt, "c1")[0].rect[0] - 140.0).abs() < 1.0);

    // 2. Resize a scrolled non-frozen column (c28) by +30 pt: header and body cells stay aligned.
    let h28 = header(&rt, "c28").rect;
    let e28 = h28[0] + h28[2] - 1.0;
    rt.pointer_down(e28, mid_y);
    rt.pointer_move(e28 + 30.0, mid_y);
    rt.pointer_up(e28 + 30.0, mid_y);
    settle(&mut rt, 6);
    let h28_after = header(&rt, "c28").rect;
    assert!((h28_after[2] - 130.0).abs() < 1.0, "scrolled c28 resized to 130: {h28_after:?}");
    for cell in cells_for(&rt, "c28") {
        assert!((cell.rect[0] - h28_after[0]).abs() < 0.5 && (cell.rect[2] - h28_after[2]).abs() < 0.5);
    }

    // 3. Reorder a scrolled column (c29) right by 1 slot via header drag.
    let h29 = header(&rt, "c29").rect;
    let cx29 = h29[0] + h29[2] * 0.5;
    rt.pointer_down(cx29, mid_y);
    rt.pointer_move(cx29 + 115.0, mid_y);
    rt.pointer_up(cx29 + 115.0, mid_y);
    settle(&mut rt, 6);
    assert_eq!(rt.state.reorders, vec![(29, 30)], "reordered scrolled column c29 -> 30");
    let h30_new = header(&rt, "c30").rect;
    let h29_new = header(&rt, "c29").rect;
    assert!(h30_new[0] < h29_new[0], "c30 now sits before c29: {h30_new:?} vs {h29_new:?}");
    for cell in cells_for(&rt, "c29") {
        assert!((cell.rect[0] - h29_new[0]).abs() < 0.5, "reordered cells stay aligned with header");
    }
}

#[test]
fn edge_cases_empty_all_hidden_all_frozen_fitting_and_no_scroll_wide() {
    struct EdgeApp {
        table: TableState,
        rows: usize,
        cols: usize,
        frozen: usize,
        scroll_wide: bool,
    }
    fn view(s: &EdgeApp, _: &mut Cx<EdgeApp>) -> El {
        let k = Kit::neutral(true);
        let mut t = Table::new(
            |s: &EdgeApp| &s.table,
            |s: &mut EdgeApp| &mut s.table,
            |s: &EdgeApp| s.rows,
            |_: &EdgeApp, i| Cow::Owned(format!("r{i}")),
        )
        .scroll_wide(s.scroll_wide)
        .virtual_columns(true)
        .frozen_columns(s.frozen);
        for c in 0..s.cols {
            t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).width(COL_W));
        }
        col().size(W, H).child(t.cell(|_: &EdgeApp, i, c| Cell::text(format!("{i}:{c}"))).build(s, &k).h(H))
    }

    // Empty rows with 50 columns still renders windowed headers and scrolls.
    let mut rt = Runtime::new(
        EdgeApp { table: TableState::new().key("e0"), rows: 0, cols: 50, frozen: 2, scroll_wide: true },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    settle(&mut rt, 8);
    assert!(headers(&rt).len() <= 20);
    assert!(find(&rt, |e| e.role == "gridcell").is_empty());

    // 0 columns / all hidden does not panic.
    let mut rt_empty = Runtime::new(
        EdgeApp { table: TableState::new().key("e1"), rows: 5, cols: 0, frozen: 3, scroll_wide: true },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    settle(&mut rt_empty, 6);
    assert!(headers(&rt_empty).is_empty());

    // frozen_columns > cols.len() clamps safely.
    let mut rt_all_frz = Runtime::new(
        EdgeApp { table: TableState::new().key("e2"), rows: 5, cols: 12, frozen: 99, scroll_wide: true },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    settle(&mut rt_all_frz, 6);
    assert_eq!(headers(&rt_all_frz).len(), 12);

    // virtual_columns(true) without scroll_wide(true) is a safe no-op.
    let mut rt_no_wide = Runtime::new(
        EdgeApp { table: TableState::new().key("e3"), rows: 5, cols: 12, frozen: 2, scroll_wide: false },
        view,
        (W, H),
        Env::new(true, 1.0),
    );
    settle(&mut rt_no_wide, 6);
    assert_eq!(headers(&rt_no_wide).len(), 12);
}

#[test]
#[ignore = "benchmark"]
fn wide_table_time() {
    assert!(!cfg!(debug_assertions), "run with --profile perf");
    // 40 visible rows (40 * 28 = 1120 + 30 header = 1150 pt tall) x 200 columns (100 pt each)
    // in a 1000 pt wide viewport, paired alternating runs (ABABABABAB).
    const BW: f32 = 1000.0;
    const BH: f32 = 1150.0;
    const ROWS: usize = 10_000;
    const COLS: usize = 200;

    struct BenchApp {
        table: TableState,
        virtual_cols: bool,
        frozen: usize,
        views: Rc<StdCell<usize>>,
        cells: Rc<StdCell<usize>>,
    }

    fn bench_view(s: &BenchApp, _: &mut Cx<BenchApp>) -> El {
        s.views.set(s.views.get() + 1);
        let k = Kit::neutral(true);
        let cells = Rc::clone(&s.cells);
        let mut t = Table::new(
            |s: &BenchApp| &s.table,
            |s: &mut BenchApp| &mut s.table,
            |_: &BenchApp| ROWS,
            |_: &BenchApp, i| Cow::Owned(format!("r{i}")),
        )
        .scroll_wide(true)
        .virtual_columns(s.virtual_cols)
        .frozen_columns(s.frozen)
        .virtual_overscan(0.0, 0.0);
        for c in 0..COLS {
            t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).width(COL_W));
        }
        let el = t
            .cell(move |_: &BenchApp, i, col| {
                cells.set(cells.get() + 1);
                Cell::text(format!("r{i}:{col}"))
            })
            .build(s, &k);
        col().size(BW, BH).child(el.h(BH))
    }

    let median = |xs: &mut Vec<f64>| -> f64 {
        xs.sort_by(f64::total_cmp);
        xs[xs.len() / 2]
    };
    let run_one = |virtual_cols: bool, frozen: usize| {
        let views = Rc::new(StdCell::new(0));
        let cells = Rc::new(StdCell::new(0));
        let mut rt = Runtime::new(
            BenchApp {
                table: TableState::new().key("bench"),
                virtual_cols,
                frozen,
                views: Rc::clone(&views),
                cells: Rc::clone(&cells),
            },
            bench_view,
            (BW, BH),
            Env::new(true, 1.0),
        );
        settle(&mut rt, 8);
        let mounted_rows = find(&rt, |e| e.role == "row" && e.key.as_deref() != Some("header")).len();
        let mounted_cols = headers(&rt).len();
        let mounted_cells = find(&rt, |e| e.role == "gridcell").len();

        // Measure full commit + layout rebuild (10 iterations per run).
        let mut rebuild_ms = Vec::new();
        let mut cells_per_rebuild = 0usize;
        for _ in 0..10 {
            cells.set(0);
            rt.invalidate();
            let t = rune::time::now();
            rt.frame(1.0 / 60.0);
            rebuild_ms.push(t.elapsed().as_secs_f64() * 1e3);
            cells_per_rebuild = cells.get();
        }

        // Measure vertical scroll (200 frames of vertical wheel, counting commits & view runs).
        let v_views0 = views.get();
        let mut v_commit_ms = Vec::new();
        for f in 1..=200 {
            rt.wheel_xy(500.0, 500.0, 0.0, -56.0);
            let c0 = rt.stats.commits;
            let t = rune::time::now();
            rt.frame(1.0 / 60.0 + f as f64 / 60.0);
            let dt_ms = t.elapsed().as_secs_f64() * 1e3;
            if rt.stats.commits > c0 {
                v_commit_ms.push(dt_ms);
            }
        }
        let v_views = views.get() - v_views0;

        // Measure horizontal wheel scroll across 100 frames (spanning multiple column windows).
        let h_views0 = views.get();
        let mut h_commit_ms = Vec::new();
        for f in 1..=100 {
            rt.wheel_xy(500.0, 500.0, -120.0, 0.0);
            let c0 = rt.stats.commits;
            let t = rune::time::now();
            rt.frame(10.0 + f as f64 / 60.0);
            let dt_ms = t.elapsed().as_secs_f64() * 1e3;
            if rt.stats.commits > c0 {
                h_commit_ms.push(dt_ms);
            }
        }
        let h_views = views.get() - h_views0;
        let h_med = if h_commit_ms.is_empty() { 0.0 } else { median(&mut h_commit_ms) };

        (
            mounted_rows,
            mounted_cols,
            mounted_cells,
            cells_per_rebuild,
            median(&mut rebuild_ms),
            if v_commit_ms.is_empty() { 0.0 } else { median(&mut v_commit_ms) },
            v_views,
            h_med,
            h_views,
        )
    };

    let (mut off_rebuild, mut on_rebuild) = (Vec::new(), Vec::new());
    let (mut off_v, mut on_v) = (Vec::new(), Vec::new());
    let (mut off_h, mut on_h) = (Vec::new(), Vec::new());
    let mut off_meta = (0, 0, 0, 0, 0, 0);
    let mut on_meta = (0, 0, 0, 0, 0, 0);

    for pair in 0..5 {
        let a = run_one(false, 0);
        let b = run_one(true, 2);
        off_rebuild.push(a.4);
        on_rebuild.push(b.4);
        off_v.push(a.5);
        on_v.push(b.5);
        off_h.push(a.7);
        on_h.push(b.7);
        off_meta = (a.0, a.1, a.2, a.3, a.6, a.8);
        on_meta = (b.0, b.1, b.2, b.3, b.6, b.8);
        println!(
            "pair {pair}: OFF rebuild={:.2}ms v_remount={:.2}ms (views={}) h_jump={:.2}ms (views={}) | ON rebuild={:.2}ms v_remount={:.2}ms (views={}) h_jump={:.2}ms (views={})",
            a.4, a.5, a.6, a.7, a.8, b.4, b.5, b.6, b.7, b.8
        );
    }

    println!(
        "SUMMARY OFF (virtual_columns=false, frozen=0): rows={} cols={} mounted_cells={} cells_built={} | rebuild_med={:.2}ms (min={:.2} max={:.2}) | v_remount_med={:.2}ms (v_views={}) | h_jump_med={:.2}ms (h_views={})",
        off_meta.0, off_meta.1, off_meta.2, off_meta.3,
        median(&mut off_rebuild.clone()), off_rebuild.iter().copied().fold(f64::INFINITY, f64::min), off_rebuild.iter().copied().fold(0.0, f64::max),
        median(&mut off_v.clone()), off_meta.4,
        median(&mut off_h.clone()), off_meta.5,
    );
    println!(
        "SUMMARY ON  (virtual_columns=true,  frozen=2): rows={} cols={} mounted_cells={} cells_built={} | rebuild_med={:.2}ms (min={:.2} max={:.2}) | v_remount_med={:.2}ms (v_views={}) | h_jump_med={:.2}ms (h_views={})",
        on_meta.0, on_meta.1, on_meta.2, on_meta.3,
        median(&mut on_rebuild.clone()), on_rebuild.iter().copied().fold(f64::INFINITY, f64::min), on_rebuild.iter().copied().fold(0.0, f64::max),
        median(&mut on_v.clone()), on_meta.4,
        median(&mut on_h.clone()), on_meta.5,
    );
}
