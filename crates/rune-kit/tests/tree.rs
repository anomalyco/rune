//! Tree: the file-tree helpers, the model's keyboard, filtering and selection, and the
//! virtualised view in a headless runtime (focus, keys, clicks, accessibility, reveal).
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, PhysicalInput, Runtime, col};
use rune_kit::{FileNode, FileTree, Kit, Tree, TreeItem, TreeModel};

type Files = TreeModel<FileNode<(u32, u32)>>;

fn files(paths: &[&str]) -> Files {
    FileTree::new().build(paths.iter().map(|p| (*p, (1, 0))))
}

/// Each visible row as `depth name`, with `+`/`-` for an open or closed folder.
fn outline(m: &Files) -> Vec<String> {
    (0..m.visible_len())
        .map(|i| {
            let (n, r) = m.row(i).unwrap();
            let mark = match r.expanded {
                Some(true) => "-",
                Some(false) => "+",
                None => " ",
            };
            format!("{}{mark}{}", "  ".repeat(r.depth as usize), n.name)
        })
        .collect()
}

const REPO: &[&str] = &[
    "src/main.rs",
    "src/ui/tree/row.rs",
    "src/ui/tree/model.rs",
    "src/lib.rs",
    "README.md",
    "docs/guide/intro/start.md",
    "Cargo.toml",
    "assets/icon10.png",
    "assets/icon2.png",
];

#[test]
fn paths_become_a_tree_with_folders_first_in_natural_order() {
    let mut m = files(REPO);
    m.expand_all();
    assert_eq!(
        outline(&m),
        [
            "-assets",
            "   icon2.png",
            "   icon10.png",
            "-docs/guide/intro",
            "   start.md",
            "-src",
            "  -ui/tree",
            "     model.rs",
            "     row.rs",
            "   lib.rs",
            "   main.rs",
            " Cargo.toml",
            " README.md",
        ]
    );
    assert_eq!(m.parent("src/ui/tree/row.rs"), Some("src/ui/tree"), "a compacted folder's id is its full path");
    assert_eq!(m.get("docs/guide/intro").unwrap().name, "docs/guide/intro");
    assert!(m.get("src/ui").is_none(), "the folders inside a compacted row aren't nodes");
}

#[test]
fn compact_folders_and_dirs_first_can_be_turned_off() {
    let mut m = FileTree::new().compact_folders(false).dirs_first(false).build(["b/c/d.rs", "a.rs", "c.rs"].map(|p| (p, ())));
    m.expand_all();
    let names: Vec<String> = (0..m.visible_len()).map(|i| m.row(i).unwrap().0.name.clone()).collect();
    assert_eq!(names, ["a.rs", "b", "c", "d.rs", "c.rs"]);
}

#[test]
fn a_filter_keeps_the_folders_leading_to_a_match_open() {
    let mut m = files(REPO);
    m.filter(|n| n.name.contains("row"));
    assert_eq!(outline(&m), ["-src", "  -ui/tree", "     row.rs"]);
    assert!(m.is_match("src/ui/tree/row.rs") && !m.is_match("src"));
    m.toggle("src/ui/tree");
    assert_eq!(outline(&m), ["-src", "  +ui/tree"], "a folder closes while filtering");
    m.clear_filter();
    assert_eq!(outline(&m), ["+assets", "+docs/guide/intro", "+src", " Cargo.toml", " README.md"], "the folders go back to how they were");
}

#[test]
fn a_matching_folder_shows_without_its_other_children() {
    let mut m = files(REPO);
    m.filter(|n| n.name == "assets");
    assert_eq!(outline(&m), [" assets"], "nothing under it matched, so it shows as a leaf");
}

// The view ---------------------------------------------------------------------------------

struct App {
    files: Files,
    picked: Vec<String>,
    opened: Vec<String>,
}

fn label(n: &FileNode<(u32, u32)>) -> &str {
    &n.name
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    col().size(320.0, 240.0).child(
        Tree::new(|s: &App| &s.files, |s: &mut App| &mut s.files, label)
            .name("Files")
            .row(|n, r| TreeItem::file(n, r).changes(n.data.map_or(0, |d| d.0), 0))
            .on_select(|s: &mut App, id, _| s.picked.push(id.to_owned()))
            .on_open(|s: &mut App, id, _| s.opened.push(id.to_owned()))
            .build(s, &k)
            .h(240.0),
    )
}

fn runtime(files: Files) -> Runtime<App> {
    let mut rt = Runtime::new(App { files, picked: Vec::new(), opened: Vec::new() }, view, (320.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn press_with(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(0.0);
}

fn press(rt: &mut Runtime<App>, key: Key) {
    press_with(rt, key, Mods::default());
}

fn focus(rt: &mut Runtime<App>) {
    press(rt, Key::Tab);
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key).as_deref(), Some("files"));
}

fn cursor(rt: &Runtime<App>) -> Option<&str> {
    rt.state.files.cursor()
}

fn rows(rt: &Runtime<App>) -> Vec<ElementInfo> {
    rt.elements().into_iter().filter(|e| e.role == "treeitem" && !e.exiting).collect()
}

#[test]
fn arrows_move_open_close_and_climb() {
    let mut rt = runtime(files(REPO));
    focus(&mut rt);
    press(&mut rt, Key::Down);
    assert_eq!(cursor(&rt), Some("assets"), "Down from nothing starts at the top");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Down);
    assert_eq!(cursor(&rt), Some("src"));
    press(&mut rt, Key::Right);
    assert!(rt.state.files.is_expanded("src"), "Right opens");
    assert_eq!(cursor(&rt), Some("src"));
    press(&mut rt, Key::Right);
    assert_eq!(cursor(&rt), Some("src/ui/tree"), "Right on an open folder goes to its first child");
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Right);
    assert_eq!(cursor(&rt), Some("src/ui/tree/model.rs"));
    press(&mut rt, Key::Right);
    assert_eq!(cursor(&rt), Some("src/ui/tree/model.rs"), "Right on a file stays");
    press(&mut rt, Key::Left);
    assert_eq!(cursor(&rt), Some("src/ui/tree"), "Left on a file goes to its folder");
    press(&mut rt, Key::Left);
    assert!(!rt.state.files.is_expanded("src/ui/tree"), "Left on an open folder closes it");
    press(&mut rt, Key::End);
    assert_eq!(cursor(&rt), Some("README.md"));
    press(&mut rt, Key::Home);
    assert_eq!(cursor(&rt), Some("assets"));
    press(&mut rt, Key::Up);
    assert_eq!(cursor(&rt), Some("assets"), "the top stops");
    assert_eq!(rt.state.picked.last().map(String::as_str), Some("assets"), "moves report the new row");
    assert!(rt.state.files.is_selected("assets") && !rt.state.files.is_selected("README.md"));
}

#[test]
fn return_toggles_a_folder_and_opens_a_file() {
    let mut rt = runtime(files(REPO));
    focus(&mut rt);
    press(&mut rt, Key::End);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened, ["README.md"]);
    press(&mut rt, Key::Home);
    press(&mut rt, Key::Enter);
    assert!(rt.state.files.is_expanded("assets"));
    press(&mut rt, Key::Text(" ".into()));
    assert!(!rt.state.files.is_expanded("assets"), "Space toggles too");
}

#[test]
fn closing_a_folder_brings_the_cursor_up_to_it() {
    let mut m = files(REPO);
    m.expand_all();
    m.select("src/ui/tree/row.rs");
    m.set_expanded("src", false);
    assert_eq!(m.cursor(), Some("src"));
    assert_eq!(m.selected().collect::<Vec<_>>(), ["src"], "single-select moves the selection up to the closed folder");

    let mut m = files(REPO).multi_select(true);
    m.expand_all();
    m.select("src/ui/tree/row.rs");
    m.set_expanded("src", false);
    let mut tx = rune::Tx::default();
    m.handle_key(&KeyEvent { key: Key::Down, mods: Mods { shift: true, ..Mods::default() } }, label, &mut tx);
    assert_eq!(m.selected().collect::<Vec<_>>(), ["src", "Cargo.toml"], "Shift+Down extends from the closed folder");
}

#[test]
fn filtering_clears_a_hidden_cursor_and_reveal_shows_a_filtered_row() {
    let mut m = files(REPO);
    m.expand_all();
    m.select("README.md");
    assert_eq!(m.visible_len(), 13);
    m.filter(|n| n.name.contains("row"));
    assert_eq!(m.cursor(), None, "a cursor hidden by a filter is cleared even after rows were cached");
    let mut tx = rune::Tx::default();
    assert!(m.reveal("README.md", &mut tx));
    assert!(m.visible_row("README.md").is_some(), "reveal makes a filtered-out node visible");
}

#[test]
fn typing_jumps_to_a_name() {
    let mut rt = runtime(files(REPO));
    focus(&mut rt);
    press(&mut rt, Key::Text("r".into()));
    assert_eq!(cursor(&rt), Some("README.md"));
    press(&mut rt, Key::Text("c".into()));
    assert_eq!(cursor(&rt), Some("README.md"), "\"rc\" matches nothing, so the cursor stays");
}

#[test]
fn rows_carry_tree_roles_level_open_state_and_selection() {
    let mut m = files(REPO);
    m.set_expanded("src", true);
    m.select("src/lib.rs");
    let rt = runtime(m);
    let all = rt.elements();
    let tree = all.iter().find(|e| e.role == "tree").expect("a tree");
    assert_eq!(tree.label.as_deref(), Some("Files"));
    assert!(tree.focusable);
    let rows = rows(&rt);
    let get = |label: &str| rows.iter().find(|e| e.label.as_deref() == Some(label)).unwrap_or_else(|| panic!("{label}"));
    assert_eq!((get("src").level, get("src").expanded), (Some(1), Some(true)));
    assert_eq!((get("assets").level, get("assets").expanded), (Some(1), Some(false)));
    assert_eq!((get("lib.rs").level, get("lib.rs").expanded, get("lib.rs").item_selected), (Some(2), None, true));
    assert!(!get("main.rs").item_selected);
}

#[test]
fn a_click_selects_and_a_folder_click_toggles() {
    let mut rt = runtime(files(REPO));
    let at = |rt: &Runtime<App>, label: &str| {
        let r = rows(rt).into_iter().find(|e| e.label.as_deref() == Some(label)).unwrap().rect;
        (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5)
    };
    let (x, y) = at(&rt, "src");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert!(rt.state.files.is_expanded("src") && rt.state.files.is_selected("src"));
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key).as_deref(), Some("files"), "the click focuses the tree");
    let (x, y) = at(&rt, "main.rs");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.files.selected().collect::<Vec<_>>(), ["src/main.rs"]);
    assert_eq!(rt.state.picked, ["src", "src/main.rs"]);
}

#[test]
fn multi_select_extends_with_shift_and_toggles_with_cmd() {
    let mut m = files(REPO).multi_select(true);
    m.expand_all();
    let mut rt = runtime(m);
    focus(&mut rt);
    press(&mut rt, Key::Down);
    press_with(&mut rt, Key::Down, Mods { shift: true, ..Mods::default() });
    press_with(&mut rt, Key::Down, Mods { shift: true, ..Mods::default() });
    assert_eq!(rt.state.files.selected().collect::<Vec<_>>(), ["assets", "assets/icon2.png", "assets/icon10.png"]);
    let r = rows(&rt).into_iter().find(|e| e.label.as_deref() == Some("lib.rs")).unwrap().rect;
    rt.physical_input(PhysicalInput::Modifiers(Mods { cmd: true, ..Mods::default() }));
    rt.pointer_down(r[0] + 40.0, r[1] + 5.0);
    rt.pointer_up(r[0] + 40.0, r[1] + 5.0);
    rt.frame(0.0);
    assert_eq!(rt.state.files.selected().count(), 4, "Cmd-click adds a row");
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    press_with(&mut rt, Key::Command('a'), Mods { cmd: true, ..Mods::default() });
    assert_eq!(rt.state.files.selected().count(), rt.state.files.visible_len());
}

/// 50,000 files in 500 folders of 100.
fn big() -> Files {
    let paths: Vec<String> = (0..500).flat_map(|d| (0..100).map(move |f| format!("pkg{d}/file{f}.rs"))).collect();
    let mut m = FileTree::new().build(paths.iter().map(|p| (p.as_str(), (1, 0))));
    m.expand_all();
    m
}

#[test]
fn fifty_thousand_rows_build_only_what_shows() {
    let m = big();
    assert_eq!(m.visible_len(), 50_500);
    let rt = runtime(m);
    let built = rows(&rt).len();
    assert!((10..80).contains(&built), "{built} rows built for a 240 pt viewport");
}

#[test]
fn reveal_opens_selects_and_scrolls_to_a_far_row() {
    let mut m = big();
    m.collapse_all();
    let mut rt = runtime(m);
    rt.update(|s, tx| assert!(s.files.reveal("pkg321/file77.rs", tx)));
    for f in 1..40 {
        rt.frame(f as f64 / 60.0);
    }
    assert!(rt.state.files.is_expanded("pkg321") && rt.state.files.is_selected("pkg321/file77.rs"));
    let row = rows(&rt).into_iter().find(|e| e.label.as_deref() == Some("file77.rs")).expect("the revealed row is built");
    let r = row.visible;
    assert!(r[3] > 20.0 && r[1] >= 0.0 && r[1] + r[3] <= 240.5, "and on screen: {r:?}");
    assert!(row.item_selected);
}

// Benchmark ---------------------------------------------------------------------------------

fn bench_view(s: &App, _: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    col().w_full().h_full().child(
        Tree::new(|s: &App| &s.files, |s: &mut App| &mut s.files, label)
            .row(|n, r| TreeItem::file(n, r).changes(n.data.map_or(0, |d| d.0), n.data.map_or(0, |d| d.1)))
            .build(s, &k)
            .grow(1.0)
            .min_h(0.0),
    )
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// 50,000 files: building the model from paths, the first frame, scrolling at 120 Hz, a jump
/// to the end, and a filter, in a 900 pt window.
/// `cargo test --profile perf -p rune-kit --test tree -- --ignored --nocapture tree_time`
#[test]
#[ignore = "benchmark"]
fn tree_time() {
    use std::time::Instant;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    for run in 0..3 {
        let t = Instant::now();
        let files = big();
        let model = ms(t);
        let t = Instant::now();
        let mut rt = Runtime::new(App { files, picked: Vec::new(), opened: Vec::new() }, bench_view, (360.0, 900.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let first = ms(t);
        let mut frames = Vec::new();
        for f in 1..=600 {
            rt.wheel(180.0, 450.0, -48.0);
            let t = Instant::now();
            rt.frame(f as f64 / 120.0);
            frames.push(ms(t));
        }
        let built = rows(&rt).len();
        let t = Instant::now();
        rt.update(|s, tx| { s.files.reveal("pkg499/file99.rs", tx); });
        rt.frame(5.1);
        let end = ms(t);
        let t = Instant::now();
        rt.update(|s, _| s.files.filter(|n| n.name.ends_with("7.rs")));
        rt.frame(5.2);
        let filter = ms(t);
        let max = frames.iter().copied().fold(0.0, f64::max);
        println!(
            "tree 50k run {run}: model {model:.1} ms, first frame {first:.1} ms; scroll p50 {:.3} p95 {:.3} max {max:.3} ms ({built} rows built); reveal last {end:.2} ms; filter {filter:.1} ms",
            percentile(&mut frames, 0.5),
            percentile(&mut frames, 0.95),
        );
    }
}
