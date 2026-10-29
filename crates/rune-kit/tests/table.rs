//! Table: column widths and resizing, sorting from the header, row selection and keys,
//! virtualisation, accessibility roles and truncation tooltips, in a headless runtime.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

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
    assert_eq!(header(&rt, "size").label.as_deref(), Some("Size, sorted ascending"));
    assert!(rt.state.files.windows(2).all(|w| w[0].size <= w[1].size));
    click(&mut rt, r[0] + 20.0, r[1] + r[3] * 0.5);
    assert_eq!(rt.state.table.sort(), Some(("size", SortDir::Descending)));
    assert_eq!(header(&rt, "size").label.as_deref(), Some("Size, sorted descending"));
    assert_eq!(header(&rt, "name").label.as_deref(), Some("Name"));
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

// Benchmark ---------------------------------------------------------------------------------

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// 100,000 rows: the first frame, scrolling at 120 Hz, a sort and a column resize, in a
/// 900 pt window. `cargo test --profile perf -p rune-kit --test table -- --ignored --nocapture table_time`
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
        let t = Instant::now();
        let mut rt = Runtime::new(app, big_view, (900.0, 900.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let first = ms(t);
        let mut frames = Vec::new();
        let (mut with_commit, c0) = (Vec::new(), rt.stats.commits);
        for f in 1..=600 {
            rt.wheel(450.0, 450.0, -48.0);
            let c = rt.stats.commits;
            let t = Instant::now();
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
            let t = Instant::now();
            rt.frame(5.0);
            rebuild.push(ms(t));
        }
        println!("  rebuild at rest p50 {:.3} ms, {} commits", percentile(&mut rebuild, 0.5), rt.stats.commits - commits);
        let r = header(&rt, "size").rect;
        let t = Instant::now();
        click(&mut rt, r[0] + 20.0, r[1] + r[3] * 0.5);
        let sort = ms(t);
        let handle = find(&rt, |e| e.label.as_deref() == Some("Resize Size")).remove(0).rect;
        let (x, y) = (handle[0] + handle[2] * 0.5, handle[1] + handle[3] * 0.5);
        rt.pointer_down(x, y);
        rt.frame(0.0);
        let mut drag = Vec::new();
        for k in 1..=60 {
            rt.pointer_move(x + k as f32, y);
            let t = Instant::now();
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
