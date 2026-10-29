//! `rune_kit::diff`: patches parse into files of hunks, texts diff line by line and word by
//! word, folded ranges open, and `DiffView` lays annotations under their lines, builds only
//! the rows in view, selects lines from the gutter and copies one side's text.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use std::fmt::Write;

use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, TextStyle, Tx, col, el, text_input};
use rune_kit::Kit;
use rune_kit::diff::{Annotation, DiffMode, DiffState, DiffView, Expand, FileDiff, LineKind, LineRange, LineRef, Side, parse};

/// Files, hunks and lines as text: `-`/`+`/space, the old and new numbers, and word changes
/// in brackets.
fn dump(files: &[FileDiff]) -> String {
    let mut out = String::new();
    for f in files {
        write!(out, "{:?} {:?} -> {:?}", f.status, f.old_path, f.new_path).unwrap();
        if let Some(s) = f.similarity {
            write!(out, " {s}%").unwrap();
        }
        if f.old_mode != f.new_mode {
            write!(out, " mode {:?} -> {:?}", f.old_mode, f.new_mode).unwrap();
        }
        if f.binary {
            out.push_str(" binary");
        }
        writeln!(out, " +{} -{}", f.additions(), f.deletions()).unwrap();
        for h in &f.hunks {
            writeln!(out, "  {}", h.header()).unwrap();
            for l in &h.lines {
                let mark = match l.kind {
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                    LineKind::Context => ' ',
                };
                let n = |n: Option<u32>| n.map_or(String::from("."), |n| n.to_string());
                let mut text = l.text.clone();
                for r in l.changes.iter().rev() {
                    text.insert(r.end, ']');
                    text.insert(r.start, '[');
                }
                let eof = if l.no_newline { " (no newline)" } else { "" };
                writeln!(out, "  {mark} {:>2} {:>2} {text}{eof}", n(l.old), n(l.new)).unwrap();
            }
        }
    }
    out
}

const PATCH: &str = r#"diff --git a/src/old name.rs b/src/new name.rs
similarity index 90%
rename from src/old name.rs
rename to src/new name.rs
index 1111111..2222222 100644
--- a/src/old name.rs
+++ b/src/new name.rs
@@ -1,3 +1,3 @@ fn main() {
 fn main() {
-    println!("hi");
+    println!("hello");
 }
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
diff --git a/logo.png b/logo.png
index 3333333..4444444 100644
Binary files a/logo.png and b/logo.png differ
diff --git a/README b/README
index 5555555..6666666 100644
--- a/README
+++ b/README
@@ -1,2 +1,2 @@
 first
-last
\ No newline at end of file
+last line
\ No newline at end of file
diff --git "a/caf\303\251.txt" "b/caf\303\251.txt"
new file mode 100644
index 0000000..7777777
--- /dev/null
+++ "b/caf\303\251.txt"
@@ -0,0 +1 @@
+bonjour
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 8888888..0000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
"#;

#[test]
fn a_patch_parses_renames_modes_binaries_and_missing_newlines() {
    let golden = r#"Renamed Some("src/old name.rs") -> Some("src/new name.rs") 90% +1 -1
  @@ -1,3 +1,3 @@ fn main() {
     1  1 fn main() {
  -  2  .     println!("[hi]");
  +  .  2     println!("[hello]");
     3  3 }
Modified Some("run.sh") -> Some("run.sh") mode Some("100644") -> Some("100755") +0 -0
Modified Some("logo.png") -> Some("logo.png") binary +0 -0
Modified Some("README") -> Some("README") +1 -1
  @@ -1,2 +1,2 @@
     1  1 first
  -  2  . last (no newline)
  +  .  2 last[ line] (no newline)
Added None -> Some("café.txt") mode None -> Some("100644") +1 -0
  @@ -0,0 +1 @@
  +  .  1 bonjour
Deleted Some("gone.txt") -> None mode Some("100644") -> None +0 -1
  @@ -1 +0,0 @@
  -  1  . bye
"#;
    assert_eq!(dump(&parse(PATCH)), golden);
}

#[test]
fn two_texts_diff_into_hunks_with_word_changes() {
    let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let new = old.replace("line 10\n", "line ten\n").replace("line 20\n", "line 20\nline 21\n");
    let f = FileDiff::from_texts("a.txt", &old, &new);
    let golden = r#"Modified Some("a.txt") -> Some("a.txt") +2 -1
  @@ -7,7 +7,7 @@ line 6
     7  7 line 7
     8  8 line 8
     9  9 line 9
  - 10  . line [10]
  +  . 10 line [ten]
    11 11 line 11
    12 12 line 12
    13 13 line 13
  @@ -18,3 +18,4 @@ line 17
    18 18 line 18
    19 19 line 19
    20 20 line 20
  +  . 21 line 21
"#;
    assert_eq!(dump(std::slice::from_ref(&f)), golden);
}

#[test]
fn folded_ranges_open_twenty_lines_at_a_time() {
    let old: String = (1..=100).map(|i| format!("{i}\n")).collect();
    let new = old.replace("\n50\n", "\nfifty\n");
    let mut s = DiffState::new(vec![FileDiff::from_texts("n.txt", &old, &new)]);
    // One hunk, 47..53: 46 lines fold above it and 47 below.
    assert_eq!((s.hidden(0, 0), s.hidden(0, 1)), (Some(46), Some(47)));
    s.expand(0, 0, Expand::Up);
    assert_eq!(s.hidden(0, 0), Some(26));
    s.expand(0, 1, Expand::Down);
    s.expand(0, 1, Expand::Down);
    assert_eq!(s.hidden(0, 1), Some(7));
    s.expand(0, 1, Expand::Down);
    assert_eq!(s.hidden(0, 1), Some(0));
    s.expand(0, 0, Expand::All);
    assert_eq!(s.hidden(0, 0), Some(0));
    // A parsed patch has no whole texts: its ranges stay folded.
    let mut p = DiffState::new(parse(PATCH));
    p.expand(0, 1, Expand::All);
    assert_eq!(p.hidden(0, 1), None);
}

// ---- the view ----

struct App {
    diff: DiffState,
    selections: Vec<Option<LineRange>>,
    comments: Vec<LineRange>,
    visible: Vec<usize>,
    actions: Vec<usize>,
}

const NOTE_H: f32 = 90.0;

fn view(s: &App, _: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    let diff = DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
        .annotation(|_: &App, a: &Annotation| el().key(format!("note-{}", a.id)).w_full().h(NOTE_H))
        .header_actions(|_: &App, f| el().key("act").size(28.0, 20.0).on_press(move |s: &mut App| s.actions.push(f)))
        .on_select(|s: &mut App, sel, _| s.selections.push(sel))
        .on_comment(|s: &mut App, lines, _| s.comments.push(lines))
        .on_visible_file(|s: &mut App, f| s.visible.push(f))
        .build(s, &k);
    col().w_full().h_full().child(diff)
}

fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.diff.key(ev, tx)
}

fn runtime(diff: DiffState, size: (f32, f32)) -> Runtime<App> {
    let app = App { diff, selections: Vec::new(), comments: Vec::new(), visible: Vec::new(), actions: Vec::new() };
    let mut rt = Runtime::new(app, view, size, Env::new(true, 2.0));
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    settle(&mut rt);
    rt
}

fn tick(rt: &mut Runtime<App>) {
    rt.frame(1.0 / 60.0);
}

/// Frames until scrolling glides and layout reports have settled.
fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        tick(rt);
    }
}

/// The element whose path ends with `/{end}`.
fn find(rt: &Runtime<App>, end: &str) -> ElementInfo {
    let suffix = format!("/{end}");
    rt.elements().into_iter().find(|e| e.path.ends_with(&suffix) && !e.exiting).unwrap_or_else(|| panic!("no {end}"))
}

fn rows(rt: &Runtime<App>) -> Vec<ElementInfo> {
    rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("df")) && !e.exiting).collect()
}

fn bottom(e: &ElementInfo) -> f32 {
    e.rect[1] + e.rect[3]
}

fn numbered(n: usize, every: usize) -> (String, String) {
    let old: String = (1..=n).map(|i| format!("let value_{i} = compute({i});\n")).collect();
    let new: String = (1..=n).map(|i| if i % every == 0 { format!("let value_{i} = compute({i}) + 1;\n") } else { format!("let value_{i} = compute({i});\n") }).collect();
    (old, new)
}

#[test]
fn annotations_sit_under_their_line_and_push_the_rest_down() {
    let (old, new) = numbered(30, 10);
    let mut d = DiffState::new(vec![FileDiff::from_texts("a.rs", &old, &new)]);
    d.set_annotations(vec![Annotation::new(0, LineRef::new(Side::New, 10), "c1"), Annotation::new(0, LineRef::new(Side::New, 10), "c2")]);
    let mut rt = runtime(d, (900.0, 1200.0));
    let line = find(&rt, "df0.n10/line");
    let (c1, c2) = (find(&rt, "note-c1"), find(&rt, "note-c2"));
    assert_eq!(line.rect[3], 20.0);
    assert_eq!(find(&rt, "df0.c9").rect[2], 900.0, "short lines still fill the view's width");
    assert_eq!(c1.rect[1], bottom(&line), "the first note right under its line");
    assert_eq!(c2.rect[1], bottom(&c1), "the next under it");
    assert_eq!(find(&rt, "df0.c11").rect[1], bottom(&c2), "the next line after both");

    // Split: under the new side's half.
    rt.state.diff.set_mode(DiffMode::Split);
    rt.invalidate();
    rt.frame(1.0);
    let line = find(&rt, "df0.o10/line");
    let c1 = find(&rt, "note-c1");
    assert_eq!(c1.rect[1], bottom(&line));
    assert!(c1.rect[0] >= 450.0, "in the new half: {:?}", c1.rect);
    assert!(rt.state.diff.annotations().len() == 2);
}

#[test]
fn a_long_diff_builds_only_the_rows_in_view_and_measures_tall_ones() {
    let (old, new) = numbered(4000, 3);
    let mut d = DiffState::new(vec![FileDiff::from_texts("long.rs", &old, &new)]);
    d.set_annotations((1..=40).map(|i| Annotation::new(0, LineRef::new(Side::New, i * 99), format!("n{i}"))).collect());
    let mut rt = runtime(d, (800.0, 600.0));
    let built = rows(&rt).len();
    assert!(built < 100, "{built} rows built for a 600pt view of 5300: the view and two below");
    // To the end: the last line sits at the bottom of the view, under every tall row above.
    for _ in 0..40 {
        rt.wheel(400.0, 300.0, -100_000.0);
        rt.frame(1.0 / 120.0);
    }
    settle(&mut rt);
    let last = find(&rt, "df0.c4000");
    assert!((bottom(&last) - 600.0).abs() < 1.0, "the last line ends the list: {:?}", last.rect);
    assert!(rows(&rt).len() < 160, "at most five views of rows: the view and two either side");
    let note = find(&rt, "note-n40");
    let line = find(&rt, "df0.n3960/line");
    assert_eq!(note.rect[1], bottom(&line));
    // And back to the top.
    for _ in 0..40 {
        rt.wheel(400.0, 300.0, 100_000.0);
        rt.frame(1.0 / 120.0);
    }
    settle(&mut rt);
    assert_eq!(find(&rt, "df0").rect[1], 0.0);
}

/// Unified diff of a tabbed file: a removed line, an added one and context around them.
fn tabbed() -> DiffState {
    let old = "fn f() {\n\tlet a = 1;\n\tlet b = 2;\n\ta + b\n}\n";
    let new = "fn f() {\n\tlet a = 1;\n\tlet b = 3;\n\ta + b\n}\n";
    DiffState::new(vec![FileDiff::from_texts("f.rs", old, new)])
}

#[test]
fn copying_lines_takes_one_side_without_numbers_or_markers() {
    let mut d = tabbed();
    d.set_selection(Some(LineRange { file: 0, start: LineRef::new(Side::Old, 2), end: LineRef::new(Side::New, 4) }));
    assert_eq!(d.selected_text().as_deref(), Some("\tlet a = 1;\n\tlet b = 2;\n\ta + b"), "the old side's lines");
    d.set_selection(Some(LineRange { file: 0, start: LineRef::new(Side::New, 2), end: LineRef::new(Side::New, 4) }));
    assert_eq!(d.selected_text().as_deref(), Some("\tlet a = 1;\n\tlet b = 3;\n\ta + b"), "the new side's");

    // A text selection dragged over the code: the side under the pointer, tabs as tabs.
    let mut rt = runtime(tabbed(), (800.0, 400.0));
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let (from, to) = (find(&rt, "df0.c2/code/text"), find(&rt, "df0.c4/code/text"));
    let (x0, y0) = (from.rect[0] + 1.0, from.rect[1] + 10.0);
    let (x1, y1) = (to.rect[0] + 200.0, to.rect[1] + 10.0);
    rt.pointer_move(x0, y0);
    tick(&mut rt);
    rt.pointer_down(x0, y0);
    tick(&mut rt);
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        rt.pointer_move(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
        tick(&mut rt);
    }
    rt.pointer_up(x1, y1);
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("\tlet a = 1;\n\tlet b = 3;\n\ta + b"));
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(&mut rt);
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("\tlet a = 1;\n\tlet b = 3;\n\ta + b"));

    // Split mode: dragging over the old (left) column vs new (right) column copies only that side, with tabs.
    let mut split_diff = tabbed();
    split_diff.set_mode(DiffMode::Split);
    let mut rt = runtime(split_diff, (800.0, 400.0));
    rt.use_clipboard(&clipboard);
    for (cell_start, cell_end, expected) in [
        ("df0.c2/old/code/text", "df0.c4/old/code/text", "\tlet a = 1;\n\tlet b = 2;\n\ta + b"),
        ("df0.c2/new/code/text", "df0.c4/new/code/text", "\tlet a = 1;\n\tlet b = 3;\n\ta + b"),
    ] {
        let (from, to) = (find(&rt, cell_start), find(&rt, cell_end));
        let (x0, y0) = (from.rect[0] + 1.0, from.rect[1] + 10.0);
        let (x1, y1) = (to.rect[0] + 200.0, to.rect[1] + 10.0);
        rt.pointer_move(x0, y0);
        tick(&mut rt);
        rt.pointer_down(x0, y0);
        tick(&mut rt);
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            rt.pointer_move(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            tick(&mut rt);
        }
        rt.pointer_up(x1, y1);
        tick(&mut rt);
        assert_eq!(rt.selected_text().as_deref(), Some(expected));
    }

    // Selecting across a hunk boundary and folded gap omits the gap label and line numbers.
    let old: String = (1..=30).map(|i| format!("\tline {i}\n")).collect();
    let new = old.replace("\tline 2\n", "\tline two\n").replace("\tline 29\n", "\tline twenty-nine\n");
    let mut across = DiffState::new(vec![FileDiff::from_texts("gap.rs", &old, &new)]);
    assert_eq!(across.files()[0].hunks.len(), 2);
    across.set_selection(Some(LineRange { file: 0, start: LineRef::new(Side::New, 1), end: LineRef::new(Side::New, 30) }));
    let copied = across.selected_text().unwrap();
    assert!(!copied.contains("unchanged"), "no gap label in copied text: {copied}");
    assert!(!copied.contains("@@"), "no hunk header in copied text: {copied}");
    assert!(copied.starts_with("\tline 1\n\tline two\n\tline 3\n"));
    assert!(copied.ends_with("\n\tline twenty-nine\n\tline 30"));
}

fn press(rt: &mut Runtime<App>, end: &str) {
    let e = find(rt, end);
    let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.pointer_move(x, y);
    tick(rt);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    tick(rt);
}

#[test]
fn line_numbers_select_lines_and_the_plus_comments_on_them() {
    let (old, new) = numbered(20, 5);
    let mut rt = runtime(DiffState::new(vec![FileDiff::from_texts("a.rs", &old, &new)]), (800.0, 900.0));
    press(&mut rt, "df0.c3/gutter/nn");
    let three = LineRange::line(0, LineRef::new(Side::New, 3));
    assert_eq!(rt.state.diff.selection(), Some(three));
    assert_eq!(rt.state.selections, [Some(three)]);
    // Dragging down the numbers.
    let (a, b) = (find(&rt, "df0.c3/gutter/nn"), find(&rt, "df0.n5/gutter/nn"));
    let x = a.rect[0] + a.rect[2] / 2.0;
    let (y0, y1) = (a.rect[1] + 10.0, b.rect[1] + 10.0);
    rt.pointer_down(x, y0);
    tick(&mut rt);
    for i in 1..=6 {
        rt.pointer_move(x, y0 + (y1 - y0) * i as f32 / 6.0);
        tick(&mut rt);
    }
    rt.pointer_up(x, y1);
    tick(&mut rt);
    let range = LineRange { file: 0, start: LineRef::new(Side::New, 3), end: LineRef::new(Side::New, 5) };
    assert_eq!(rt.state.diff.selection(), Some(range));
    assert_eq!(rt.state.selections.last(), Some(&Some(range)));
    // The "+" on a selected line comments on the selection; on another line, on that line.
    press(&mut rt, "df0.c4/gutter/plus");
    assert_eq!(rt.state.comments, [range]);
    press(&mut rt, "df0.c9/gutter/plus");
    assert_eq!(rt.state.comments[1], LineRange::line(0, LineRef::new(Side::New, 9)));
    // Escape clears the selection.
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    tick(&mut rt);
    assert_eq!(rt.state.diff.selection(), None);
}

#[test]
fn unified_file_headers_span_the_viewport_while_code_scrolls_sideways() {
    let long = "x".repeat(400);
    let text = |n: &str| (0..60).map(|i| if i == 1 { format!("{long}\n") } else { format!("let v{i} = {}{n};\n", i % 2) }).collect::<String>();
    let (old, new) = (text(""), text("+1"));
    let files = (0..2).map(|i| FileDiff::from_texts(format!("dir/f{i}.rs"), &old, &new)).collect();
    let mut d = DiffState::new(files);
    d.set_mode(DiffMode::Unified);
    let app = App { diff: d, selections: Vec::new(), comments: Vec::new(), visible: Vec::new(), actions: Vec::new() };
    let mut rt = Runtime::new(app, view, (600.0, 400.0), Env::new(true, 2.0));
    rt.on_key = Some(on_key);
    // Frame 0 already settles the viewport width in the same commit, with no one-frame lag.
    rt.frame(0.0);
    let inside = |rt: &Runtime<App>, w: f32, end: &str| {
        let r = find(rt, end).rect;
        assert!(r[0] >= -0.5 && r[0] + r[2] <= w + 0.5 && r[2] > 0.0, "{end} within {w}: {r:?}");
    };
    let check = |rt: &Runtime<App>, w: f32| {
        inside(rt, w, "df0/inner/adds");
        inside(rt, w, "df0/inner/dels");
        inside(rt, w, "df0/inner/actions");
        let h = find(rt, "df0");
        assert!(h.rect[0].abs() < 0.5 && (h.rect[2] - w).abs() < 0.5, "header spans {w}: {:?}", h.rect);
    };
    check(&rt, 600.0);
    settle(&mut rt);
    for dx in [-250.0, -250.0] {
        rt.wheel_xy(300.0, 200.0, dx, 0.0);
        settle(&mut rt);
        check(&rt, 600.0);
    }
    let code = find(&rt, "df0.c2/code").rect;
    assert!(code[0] < -100.0, "the code did scroll: {code:?}");

    // Header actions on the shifted in-list header remain clickable after scrolling sideways.
    press(&mut rt, "df0/inner/actions");
    assert_eq!(rt.state.actions, vec![0]);

    // Window resizes update the header width on the resize frame itself (not a frame later).
    for w in [800.0, 450.0, 600.0] {
        rt.set_viewport((w, 400.0), 2.0);
        rt.frame(0.0);
        check(&rt, w);
    }
    press(&mut rt, "df0/inner/actions");
    assert_eq!(rt.state.actions, vec![0, 0]);

    rt.wheel(300.0, 200.0, -150.0);
    settle(&mut rt);
    let sticky = find(&rt, "sticky");
    assert!(sticky.rect[0].abs() < 0.5 && (sticky.rect[2] - 600.0).abs() < 0.5, "sticky spans the viewport: {:?}", sticky.rect);
    inside(&rt, 600.0, "sticky/inner/adds");
    press(&mut rt, "sticky/inner/actions");
    assert_eq!(rt.state.actions, vec![0, 0, 0]);
}

#[test]
fn cancelling_a_gutter_drag_restores_selection_without_notifying_a_drop() {
    let (old, new) = numbered(20, 5);
    let mut rt = runtime(DiffState::new(vec![FileDiff::from_texts("a.rs", &old, &new)]), (800.0, 900.0));
    press(&mut rt, "df0.c3/gutter/nn");
    let original = rt.state.diff.selection();
    let calls = rt.state.selections.len();
    let a = find(&rt, "df0.c4/gutter/nn");
    let (x, y) = (a.rect[0] + a.rect[2] / 2.0, a.rect[1] + 10.0);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 40.0);
    tick(&mut rt);
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    tick(&mut rt);
    assert_eq!(rt.state.diff.selection(), original);
    assert_eq!(rt.state.selections.len(), calls);
    rt.pointer_up(x, y + 40.0);
    tick(&mut rt);
    assert_eq!(rt.state.selections.len(), calls);
}

#[test]
fn unified_gutters_stay_at_the_left_edge_while_code_scrolls_sideways() {
    let long = "x".repeat(400);
    let text = |n: &str| (0..30).map(|i| if i == 1 { format!("{long}\n") } else { format!("let v{i} = {}{n};\n", i % 2) }).collect::<String>();
    let mut d = DiffState::new(vec![FileDiff::from_texts("a.rs", &text(""), &text("+1"))]);
    d.set_mode(DiffMode::Unified);
    let mut rt = runtime(d, (600.0, 400.0));
    let before = find(&rt, "df0.o6/code").rect;
    for _ in 0..2 {
        rt.wheel_xy(300.0, 200.0, -250.0, 0.0);
        settle(&mut rt);
    }
    let code = find(&rt, "df0.o6/code").rect;
    assert!(code[0] < before[0] - 100.0, "the code scrolled: {before:?} -> {code:?}");
    for end in ["df0.o6/gutter", "df0.o6/gutter/no", "df0.o6/gutter/nn"] {
        let r = find(&rt, end).rect;
        assert!(r[0] >= -0.5 && r[0] <= 100.0, "{end} stays in view: {r:?}");
    }
    let g = find(&rt, "df0.o6/gutter").rect;
    assert!(g[0].abs() < 0.5, "gutter at the left edge: {g:?}");
    let e = |k: &str| rt.elements().into_iter().find(|e| e.path.ends_with(k) && !e.exiting).unwrap();
    assert!(e("df0.o6/gutter").stuck > 100.0);

    // Pressing either number column (including the blank new-side column on a removed row)
    // selects the line after the sideways scroll without selecting scrolled code underneath.
    press(&mut rt, "df0.o6/gutter/nn");
    assert_eq!(rt.state.diff.selection(), Some(LineRange::line(0, LineRef::new(Side::Old, 6))));
    assert!(rt.selected_text().is_none(), "gutter press does not select code under the gutter");

    // Dragging down the stuck gutter while the view scrolls vertically selects across the scroll.
    let start = find(&rt, "df0.o4/gutter/no");
    let x = start.rect[0] + start.rect[2] / 2.0;
    let y0 = start.rect[1] + 10.0;
    rt.pointer_down(x, y0);
    tick(&mut rt);
    rt.wheel(300.0, 200.0, -80.0);
    settle(&mut rt);
    let end = find(&rt, "df0.o10/gutter/no");
    let y1 = end.rect[1] + 10.0;
    rt.pointer_move(x, y1);
    tick(&mut rt);
    rt.pointer_up(x, y1);
    tick(&mut rt);
    assert_eq!(
        rt.state.diff.selection(),
        Some(LineRange { file: 0, start: LineRef::new(Side::Old, 4), end: LineRef::new(Side::Old, 10) }),
    );
}

#[test]
fn rows_read_as_their_change_and_keys_move_between_files() {
    let (old, new) = numbered(200, 50);
    let files = (0..3).map(|i| FileDiff::from_texts(format!("f{i}.rs"), &old, &new)).collect();
    let mut rt = runtime(DiffState::new(files), (800.0, 400.0));
    let els = rt.elements();
    let label = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.label.clone());
    assert_eq!(label("df0.n50").as_deref(), Some("added line 50: let value_50 = compute(50) + 1;"));
    assert_eq!(label("df0.o50").as_deref(), Some("removed line 50: let value_50 = compute(50);"));
    assert_eq!(label("df0.c49").as_deref(), Some("unchanged line 49: let value_49 = compute(49);"));
    assert!(els.iter().any(|e| e.role == "heading" && e.label.as_deref() == Some("f0.rs, 4 added, 4 removed")));

    let text = |c: &str| KeyEvent { key: Key::Text(c.into()), mods: Mods::default() };
    rt.key(text("]"));
    settle(&mut rt);
    assert_eq!(rt.state.diff.visible_file(), 1);
    assert!(find(&rt, "df1").rect[1].abs() < 1.0, "the next file's header at the top");
    rt.key(text("j"));
    settle(&mut rt);
    assert_eq!(rt.state.diff.visible_file(), 1);
    rt.key(text("["));
    settle(&mut rt);
    assert_eq!(rt.state.diff.visible_file(), 0);
    assert_eq!(rt.state.visible.last(), Some(&0));
}

#[test]
fn focused_annotation_input_keeps_navigation_and_escape_keys() {
    struct ComposeApp {
        diff: DiffState,
        comment: TextEdit,
    }
    fn compose_view(s: &ComposeApp, _: &mut Cx<ComposeApp>) -> El {
        let k = Kit::neutral(true);
        let diff = DiffView::new(|s: &ComposeApp| &s.diff, |s: &mut ComposeApp| &mut s.diff)
            .annotation(|s: &ComposeApp, a: &Annotation| {
                col()
                    .w_full()
                    .child(
                        text_input(&s.comment, TextStyle::new(13.0, 400.0))
                            .key(format!("input-{}", a.id))
                            .size(300.0, 28.0)
                            .on_edit(|s: &mut ComposeApp| &mut s.comment),
                    )
                    .child(el().key(format!("pop-{}", a.id)).size(80.0, 20.0).anchored(format!("input-{}", a.id), rune::Side::Bottom, rune::Align::Start, 4.0))
            })
            .build(s, &k);
        col().w_full().h_full().child(diff)
    }
    fn compose_key(s: &mut ComposeApp, ev: &KeyEvent, tx: &mut Tx) -> bool {
        s.diff.key(ev, tx)
    }

    let long = "x".repeat(400);
    let (old, new) = numbered(60, 10);
    let old = old.replace("let value_9 = compute(9);", &long);
    let new = new.replace("let value_9 = compute(9);", &long);
    let files = (0..2).map(|i| FileDiff::from_texts(format!("f{i}.rs"), &old, &new)).collect();
    let mut diff = DiffState::new(files);
    diff.set_annotations(vec![Annotation::new(0, LineRef::new(Side::New, 10), "c1")]);
    let sel = LineRange::line(0, LineRef::new(Side::New, 10));
    diff.set_selection(Some(sel));
    let mut rt = Runtime::new(ComposeApp { diff, comment: TextEdit::default() }, compose_view, (600.0, 600.0), Env::new(true, 2.0));
    rt.on_key = Some(compose_key);
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    // Scroll sideways and slightly down: the pinned annotation card, its text input, caret and
    // anchored popover stay in the viewport, and typing does not yank the horizontal scroll back.
    rt.wheel_xy(300.0, 200.0, -250.0, -40.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let find_el = |rt: &Runtime<ComposeApp>, end: &str| {
        let suffix = format!("/{end}");
        rt.elements().into_iter().find(|e| e.path.ends_with(&suffix) && !e.exiting).unwrap()
    };
    let code_x = find_el(&rt, "df0.n10/line/code").rect[0];
    assert!(code_x < -100.0, "code scrolled sideways: {code_x}");
    let inp = find_el(&rt, "input-c1");
    assert!((inp.rect[0]).abs() < 0.5, "input stays pinned at left edge: {:?}", inp.rect);
    let pop = find_el(&rt, "pop-c1");
    assert!(
        (pop.rect[0] - inp.rect[0].max(8.0)).abs() < 0.5 && (pop.rect[1] - (inp.rect[1] + inp.rect[3] + 4.0)).abs() < 0.5,
        "popover follows pinned input: {:?} vs {:?}",
        pop.rect,
        inp.rect
    );

    // Click the comment box after scrolling sideways to focus it.
    rt.pointer_down(inp.rect[0] + 10.0, inp.rect[1] + 10.0);
    rt.pointer_up(inp.rect[0] + 10.0, inp.rect[1] + 10.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.comment.focused);
    for ch in ["]", "[", "j", "k"] {
        rt.key(KeyEvent { key: Key::Text(ch.into()), mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.comment.text, "][jk", "keys type into the focused comment box");
    let inp = find_el(&rt, "input-c1");
    let caret = inp.caret.expect("focused input reports caret");
    assert!(
        caret[0] >= inp.rect[0] && caret[0] < inp.rect[0] + inp.rect[2],
        "caret sits inside pinned input: {caret:?} in {:?}",
        inp.rect
    );
    assert!(
        (find_el(&rt, "df0.n10/line/code").rect[0] - code_x).abs() < 1.0,
        "typing in pinned input did not reset horizontal scroll"
    );
    let pop = find_el(&rt, "pop-c1");
    assert!(
        (pop.rect[0] - inp.rect[0].max(8.0)).abs() < 0.5 && (pop.rect[1] - (inp.rect[1] + inp.rect[3] + 4.0)).abs() < 0.5,
        "popover stays anchored after commit while scrolled: {:?} vs {:?}",
        pop.rect,
        inp.rect
    );
    assert_eq!(rt.state.diff.visible_file(), 0, "did not jump files");
    assert_eq!(rt.state.diff.selection(), Some(sel), "selection intact");

    // Escape blurs the comment box first without clearing the diff's line selection.
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.comment.focused, "Escape blurred the comment box");
    assert_eq!(rt.state.diff.selection(), Some(sel), "Escape did not steal the diff's selection while blurring the field");

    // Now that the comment box is blurred, Escape clears the diff's selection.
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.diff.selection(), None);
}

/// `cargo test -p rune-kit --profile perf --test diff diff_50k_time -- --ignored --nocapture`
#[test]
#[ignore]
fn diff_50k_time() {
    use std::time::Instant;
    let (old, new) = numbered(50_000, 7);
    let start = Instant::now();
    let file = FileDiff::from_texts("big.rs", &old, &new);
    let diff_ms = start.elapsed().as_secs_f64() * 1e3;
    let start = Instant::now();
    let state = DiffState::new(vec![file]);
    let rows_ms = start.elapsed().as_secs_f64() * 1e3;
    let app = App { diff: state, selections: Vec::new(), comments: Vec::new(), visible: Vec::new(), actions: Vec::new() };
    let start = Instant::now();
    let mut rt = Runtime::new(app, view, (1200.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let open_ms = start.elapsed().as_secs_f64() * 1e3;
    println!("50k lines (every 7th changed): diff {diff_ms:.1} ms, rows {rows_ms:.1} ms, first frame {open_ms:.1} ms, {} rows", rows(&rt).len());
    // A commit costs the same as a short diff's: the rows in view, not the length.
    {
        let (old, new) = numbered(300, 7);
        let app = App { diff: DiffState::new(vec![FileDiff::from_texts("small.rs", &old, &new)]), selections: Vec::new(), comments: Vec::new(), visible: Vec::new(), actions: Vec::new() };
        let mut small = Runtime::new(app, view, (1200.0, 900.0), Env::new(true, 2.0));
        for _ in 0..30 {
            small.frame(1.0 / 120.0);
        }
        let mut times = Vec::new();
        for _ in 0..200 {
            small.invalidate();
            let start = Instant::now();
            small.frame(1.0 / 120.0);
            times.push(start.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        println!("commit, 300 lines: p50 {:.2} ms, p90 {:.2} ms, {} rows", times[100], times[180], rows(&small).len());
    }
    for _ in 0..30 {
        rt.frame(1.0 / 120.0);
    }
    let mut times = Vec::new();
    for _ in 0..200 {
        rt.invalidate();
        let start = Instant::now();
        rt.frame(1.0 / 120.0);
        times.push(start.elapsed().as_secs_f64() * 1e3);
    }
    times.sort_by(f64::total_cmp);
    println!("commit, 50k lines: p50 {:.2} ms, p90 {:.2} ms", times[100], times[180]);
    let mut times = Vec::new();
    for _ in 0..200 {
        let start = Instant::now();
        rt.frame(1.0 / 120.0);
        times.push(start.elapsed().as_secs_f64() * 1e3);
    }
    times.sort_by(f64::total_cmp);
    println!("idle frame: p50 {:.2} ms, p90 {:.2} ms", times[100], times[180]);
    for (name, dy) in [("scroll 30pt/frame", -30.0f32), ("fling 400pt/frame", -400.0)] {
        let mut times = Vec::new();
        for _ in 0..600 {
            rt.wheel(600.0, 450.0, dy);
            let start = Instant::now();
            rt.frame(1.0 / 120.0);
            times.push(start.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        println!("{name}: p50 {:.2} ms, p90 {:.2} ms, p99 {:.2} ms, max {:.2} ms", times[300], times[540], times[594], times[599]);
    }
    let mut jumps = Vec::new();
    for i in 0..20 {
        rt.wheel(600.0, 450.0, if i % 2 == 0 { -300_000.0 } else { 150_000.0 });
        let start = Instant::now();
        rt.frame(1.0 / 120.0);
        jumps.push(start.elapsed().as_secs_f64() * 1e3);
    }
    jumps.sort_by(f64::total_cmp);
    println!("jump 150k-300k pt: p50 {:.2} ms, max {:.2} ms", jumps[10], jumps[19]);
}
