//! Table: dragging headers to reorder columns (`Table::on_reorder`, `TableState::column_order`),
//! a header over app-drawn rows (`Table::header`, `Table::visible_columns`), and closures that
//! capture for `Table::new`'s accessors, in a headless runtime.

use std::borrow::Cow;

use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Runtime, col, row, text};
use rune_kit::{Cell, Kit, SortDir, Table, TableColumn, TableState};

const W: f32 = 600.0;
const H: f32 = 300.0;

fn find<S>(rt: &Runtime<S>, f: impl Fn(&ElementInfo) -> bool) -> Vec<ElementInfo> {
    rt.elements().into_iter().filter(|e| !e.exiting && f(e)).collect()
}

fn header_ids<S>(rt: &Runtime<S>) -> Vec<String> {
    let mut h = find(rt, |e| e.role == "columnheader");
    h.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    h.into_iter().filter_map(|e| e.key).collect()
}

fn header<S>(rt: &Runtime<S>, id: &str) -> ElementInfo {
    find(rt, |e| e.role == "columnheader" && e.key.as_deref() == Some(id)).remove(0)
}

fn settle<S>(rt: &mut Runtime<S>) {
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

struct App {
    rows: Vec<String>,
    tables: [TableState; 2],
    moves: Vec<(usize, usize)>,
    sorts: usize,
}

fn columns() -> [TableColumn; 3] {
    [
        TableColumn::new("name", "Name").width(200.0).sortable(),
        TableColumn::new("kind", "Kind").width(150.0),
        TableColumn::new("size", "Size").width(150.0),
    ]
}

/// The table over `tables[which]`: the accessors capture `which`.
fn table(which: usize) -> Table<App> {
    let mut t = Table::new(
        move |s: &App| &s.tables[which],
        move |s: &mut App| &mut s.tables[which],
        |s: &App| s.rows.len(),
        |s: &App, i| Cow::Borrowed(s.rows[i].as_str()),
    );
    for c in columns() {
        t = t.column(c);
    }
    t.cell(|s: &App, i, c| Cell::text(format!("{}:{c}", s.rows[i])))
        .on_sort(|s: &mut App, _, _: SortDir, _| s.sorts += 1)
        .on_reorder(|s: &mut App, from, to, _| s.moves.push((from, to)))
}

fn app() -> App {
    App {
        rows: (0..5).map(|i| format!("r{i}")).collect(),
        tables: [TableState::new().key("first"), TableState::new().key("second")],
        moves: Vec::new(),
        sorts: 0,
    }
}

fn drag<S>(rt: &mut Runtime<S>, from: (f32, f32), dx: f32) {
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(from.0 + dx / 2.0, from.1);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(from.0 + dx, from.1);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(from.0 + dx, from.1);
    settle(rt);
}

#[test]
fn dragging_a_header_past_its_neighbours_moves_the_column_in_header_and_rows() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = Kit::neutral(true);
        col().size(W, H).child(table(0).build(s, &k).h(H))
    }
    let mut rt = Runtime::new(app(), view, (W, H), Env::new(true, 1.0));
    settle(&mut rt);
    assert_eq!(header_ids(&rt), ["name", "kind", "size"]);

    // "name" (0..200) dragged right until its middle passes "kind"'s (275) but not "size"'s (425).
    let name = header(&rt, "name");
    let y = name.rect[1] + name.rect[3] / 2.0;

    // The edge still resizes rather than moving the column: 180 pt would carry "name"'s middle
    // (100) past "kind"'s (275) if the header's own drag took it.
    drag(&mut rt, (name.rect[2] - 1.0, y), 180.0);
    assert_eq!(rt.state.tables[0].column_width("name"), Some(380.0));
    assert!(rt.state.moves.is_empty() && rt.state.tables[0].column_order().is_empty());
    rt.update(|s, _| s.tables[0].reset_column_width("name"));
    settle(&mut rt);

    drag(&mut rt, (100.0, y), 200.0);
    assert_eq!(rt.state.moves, [(0, 1)]);
    assert_eq!(rt.state.sorts, 0, "a drag doesn't sort");
    assert_eq!(rt.state.tables[0].column_order(), ["kind", "name", "size"]);
    assert_eq!(header_ids(&rt), ["kind", "name", "size"]);
    let name = header(&rt, "name");
    assert!((name.rect[0] - 150.0).abs() < 0.5, "dropped header sits in its new place: {:?}", name.rect);
    let cells = find(&rt, |e| e.role == "gridcell" && e.key.as_deref() == Some("name"));
    assert!(!cells.is_empty());
    for c in &cells {
        assert!((c.rect[0] - name.rect[0]).abs() < 0.5, "body cells follow the moved column");
    }

    // A drag inside the slop is still a click: it sorts and moves nothing.
    drag(&mut rt, (name.rect[0] + 50.0, y), 2.0);
    assert_eq!(rt.state.sorts, 1);
    assert_eq!(rt.state.moves.len(), 1);

    // Hidden columns keep their place; places are among the visible columns.
    rt.update(|s, _| s.tables[0].hide_column("kind"));
    settle(&mut rt);
    assert_eq!(header_ids(&rt), ["name", "size"]);
    drag(&mut rt, (300.0, y), -250.0);
    assert_eq!(rt.state.moves, [(0, 1), (1, 0)]);
    assert_eq!(header_ids(&rt), ["size", "name"]);
    assert_eq!(rt.state.tables[0].column_order(), ["kind", "size", "name"]);
}

#[test]
fn a_header_over_app_drawn_rows_lines_up_and_two_tables_share_one_state() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = Kit::neutral(true);
        let mut list = col().key("custom").w_full().child(table(1).header(s, &k));
        let cols = table(1).visible_columns(s);
        for r in &s.rows {
            let mut line = row().key(r.clone()).w_full().h(24.0);
            for c in &cols {
                let cell = c.size_el(
                    row().key(c.id().to_owned()).role("cell").child(text(r.clone())),
                    s.tables[1].column_width(c.id()),
                );
                line = line.child(cell);
            }
            list = list.child(line);
        }
        col().size(W, H * 2.0).child(table(0).build(s, &k).h(H)).child(list)
    }
    let mut rt = Runtime::new(app(), view, (W, H * 2.0), Env::new(true, 1.0));
    settle(&mut rt);
    let custom_cells = |rt: &Runtime<App>, id: &str| find(rt, |e| e.role == "cell" && e.key.as_deref() == Some(id));

    // The second table's header is over the custom rows; the first table's is unaffected.
    let second_header = |rt: &Runtime<App>, id: &str| {
        find(rt, |e| e.role == "columnheader" && e.key.as_deref() == Some(id) && e.rect[1] >= H).remove(0)
    };
    let size = second_header(&rt, "size");
    for c in custom_cells(&rt, "size") {
        assert!(
            (c.rect[0] - size.rect[0]).abs() < 0.5 && (c.rect[2] - size.rect[2]).abs() < 0.5,
            "custom cells line up with the header"
        );
    }
    assert_eq!(rt.state.tables[1].laid_out_width("size"), Some(150.0));

    // Dragging the standalone header reorders only the second table's columns, and the app's rows follow.
    let y = size.rect[1] + size.rect[3] / 2.0;
    drag(&mut rt, (size.rect[0] + 75.0, y), -400.0);
    assert_eq!(rt.state.tables[1].column_order(), ["size", "name", "kind"]);
    assert!(rt.state.tables[0].column_order().is_empty());
    let size = second_header(&rt, "size");
    assert!(size.rect[0].abs() < 0.5);
    for c in custom_cells(&rt, "size") {
        assert!(c.rect[0].abs() < 0.5, "custom rows follow the new order");
    }
}

#[test]
fn a_header_whose_release_is_lost_lets_go_once_the_pointer_moves_on() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = Kit::neutral(true);
        col().size(W, H).child(table(0).build(s, &k).h(H))
    }
    let raised = |rt: &Runtime<App>| find(rt, |e| e.role == "columnheader" && e.raised).len();
    let mut rt = Runtime::new(app(), view, (W, H), Env::new(true, 1.0));
    settle(&mut rt);
    let kind = header(&rt, "kind");
    let (x, y) = (kind.rect[0] + 75.0, kind.rect[1] + kind.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(x + 60.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.tables[0].dragged_column(), Some((1, 60.0)));
    assert_eq!(raised(&rt), 1, "the dragged header rides above its neighbours");

    // The dragged header goes mid-drag, so its release never reaches it.
    rt.update(|s, _| s.tables[0].hide_column("kind"));
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x + 60.0, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(100.0, y);
    rt.pointer_move(110.0, y);
    settle(&mut rt);
    assert_eq!(rt.state.tables[0].dragged_column(), None, "the table let go of the header");
    assert_eq!(raised(&rt), 0, "no header shows as dragged");
    assert!(rt.state.moves.is_empty() && rt.state.tables[0].column_order().is_empty(), "a lost drag moves nothing");

    // The next press on a header is a click that sorts.
    rt.pointer_down(100.0, y);
    rt.pointer_up(100.0, y);
    settle(&mut rt);
    assert_eq!(rt.state.sorts, 1);
}
