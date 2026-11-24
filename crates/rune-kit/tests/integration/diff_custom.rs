//! `DiffView::file_header`, `DiffView::syntax_theme`, and edge-whitespace-ignoring `DiffOptions`:
//! custom file headers in list rows and sticky overlays, live syntax theme switches invalidating
//! memoised rows, and split-mode rendering, selection and copying with per-side original text.

use rune::runtime::ElementInfo;
use rune::{Color, Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, row, text};
use rune_kit::Kit;
use rune_kit::diff::{DiffMode, DiffOptions, DiffState, DiffView, Expand, FileDiff, LineRange, LineRef, Side};
use rune_kit::syntax::{Syntax, SyntaxTheme};

struct App {
    diff: DiffState,
    custom_header: bool,
    syntax_theme: Option<SyntaxTheme>,
    header_clicks: usize,
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    let mut diff = DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff);
    if s.custom_header {
        diff = diff.file_header(|s: &App, f, collapsed| {
            let path = s.diff.files()[f].path().to_owned();
            let marker = if collapsed { "closed" } else { "open" };
            row()
                .key(format!("custom-header-{f}"))
                .w_full()
                .h_full()
                .px(10.0)
                .gap(8.0)
                .items_center()
                .role("button")
                .label(format!("Toggle {path}"))
                .expanded(!collapsed)
                .on_press(move |s: &mut App| {
                    s.header_clicks += 1;
                    s.diff.set_collapsed(f, !collapsed);
                })
                .child(text(format!("{path} [{marker}]")).key("title"))
        });
    }
    if let Some(theme) = s.syntax_theme {
        diff = diff.syntax_theme(theme);
    }
    col().w_full().h_full().child(diff.build(s, &k))
}

fn mount(app: App, size: (f32, f32)) -> Runtime<App> {
    let mut rt = Runtime::new(app, view, size, Env::new(true, 2.0));
    rt.frame(0.0);
    settle(&mut rt);
    rt
}

fn tick(rt: &mut Runtime<App>) {
    rt.frame(1.0 / 60.0);
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..40 {
        tick(rt);
    }
}

fn find(rt: &Runtime<App>, end: &str) -> ElementInfo {
    let suffix = format!("/{end}");
    rt.elements().into_iter().find(|e| e.path.ends_with(&suffix) && !e.exiting).unwrap_or_else(|| panic!("no {end}"))
}

fn find_opt(rt: &Runtime<App>, end: &str) -> Option<ElementInfo> {
    let suffix = format!("/{end}");
    rt.elements().into_iter().find(|e| e.path.ends_with(&suffix) && !e.exiting)
}

fn press(rt: &mut Runtime<App>, end: &str) {
    let e = find(rt, end);
    let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.pointer_move(x, y);
    tick(rt);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn has_instance_color(rt: &Runtime<App>, c: Color) -> bool {
    rt.instances.iter().any(|inst| {
        (inst.fill[0] - c.r).abs() < 1e-3
            && (inst.fill[1] - c.g).abs() < 1e-3
            && (inst.fill[2] - c.b).abs() < 1e-3
            && (inst.fill[3] - c.a).abs() < 1e-3
    })
}

#[test]
fn file_header_replaces_built_in_header_in_list_and_sticky_overlay_and_toggles_collapse() {
    let wide = "x".repeat(220);
    let old: String = (1..=35).map(|i| format!("let line_{i} = \"{wide}\";\n")).collect();
    let new = old.replace("let line_2 =", "let changed_2 =").replace("let line_30 =", "let changed_30 =");
    let files = vec![
        FileDiff::from_texts("src/first.rs", &old, &new),
        FileDiff::from_texts("src/second.rs", "a\n", "b\n"),
    ];
    let mut rt = mount(
        App { diff: DiffState::new(files), custom_header: true, syntax_theme: None, header_clicks: 0 },
        (600.0, 320.0),
    );

    // Built-in toggle is gone; custom header fills the 44pt header row and spans the viewport.
    assert!(find_opt(&rt, "df0/inner/toggle").is_none());
    let header_row = find(&rt, "df0");
    assert_eq!(header_row.role, "heading");
    assert!((header_row.rect[3] - 44.0).abs() < 0.5, "header height stays HEADER_H: {:?}", header_row.rect);
    assert!((header_row.rect[2] - 600.0).abs() < 0.5, "header spans viewport width: {:?}", header_row.rect);
    let custom = find(&rt, "df0/inner/custom-header-0");
    assert!((custom.rect[2] - 600.0).abs() < 0.5);
    assert_eq!(find(&rt, "df0/inner/custom-header-0/title").text.as_deref(), Some("src/first.rs [open]"));

    // Scroll horizontally: custom header stays pinned at x = 0 via sticky_left(0.0).
    rt.wheel_xy(300.0, 160.0, -240.0, 0.0);
    settle(&mut rt);
    let pinned = find(&rt, "df0");
    assert!(pinned.rect[0].abs() < 0.5 && (pinned.rect[2] - 600.0).abs() < 0.5, "custom header stays pinned horizontally: {:?}", pinned.rect);

    // Scroll down past file 0's header: sticky overlay uses the custom header at y = 0.
    for _ in 0..8 {
        rt.wheel(300.0, 160.0, -40.0);
        tick(&mut rt);
    }
    settle(&mut rt);
    let sticky = find(&rt, "sticky");
    assert!((sticky.rect[1]).abs() < 0.5 && (sticky.rect[2] - 600.0).abs() < 0.5 && (sticky.rect[3] - 44.0).abs() < 0.5);
    assert_eq!(find(&rt, "sticky/inner/custom-header-0/title").text.as_deref(), Some("src/first.rs [open]"));

    // Pressing the custom header toggles collapse via DiffState::set_collapsed.
    press(&mut rt, "sticky/inner/custom-header-0");
    assert_eq!(rt.state.header_clicks, 1);
    assert!(rt.state.diff.collapsed(0));
    assert_eq!(find(&rt, "df0/inner/custom-header-0/title").text.as_deref(), Some("src/first.rs [closed]"));
    assert!(find_opt(&rt, "df0.o2").is_none(), "collapsed file hides its lines");

    // Pressing again expands file 0 back.
    press(&mut rt, "df0/inner/custom-header-0");
    assert_eq!(rt.state.header_clicks, 2);
    assert!(!rt.state.diff.collapsed(0));
    assert_eq!(find(&rt, "df0/inner/custom-header-0/title").text.as_deref(), Some("src/first.rs [open]"));

    // Turning custom_header off restores the built-in header without mutating DiffState.
    rt.state.custom_header = false;
    rt.invalidate();
    settle(&mut rt);
    assert!(find_opt(&rt, "df0/inner/toggle").is_some());
    assert!(find_opt(&rt, "df0/inner/custom-header-0").is_none());

    // Empty diff with custom file_header builds cleanly.
    rt.state.diff = DiffState::new(Vec::new());
    rt.state.custom_header = true;
    rt.invalidate();
    settle(&mut rt);
    assert!(find_opt(&rt, "df0").is_none());
}

#[cfg(feature = "syntax")]
#[test]
fn syntax_theme_overrides_token_colours_and_invalidates_row_memos() {
    let old = "fn main() {\n    let value = 1;\n}\n";
    let new = "fn main() {\n    let value = 2;\n}\n";
    let magenta = Color { r: 0.91, g: 0.13, b: 0.57, a: 1.0 };
    let lime = Color { r: 0.17, g: 0.89, b: 0.23, a: 1.0 };
    let theme_a = SyntaxTheme::neutral(true).with(Syntax::Keyword, magenta);
    let theme_b = SyntaxTheme::neutral(true).with(Syntax::Keyword, lime);

    let mut rt = mount(
        App { diff: DiffState::new(vec![FileDiff::from_texts("main.rs", old, new)]), custom_header: false, syntax_theme: None, header_clicks: 0 },
        (800.0, 300.0),
    );
    assert!(!has_instance_color(&rt, magenta), "default theme does not paint custom magenta");
    assert!(!has_instance_color(&rt, lime), "default theme does not paint custom lime");

    // Apply custom theme A without mutating DiffState: memoised rows must rebuild with magenta keywords.
    rt.state.syntax_theme = Some(theme_a);
    rt.invalidate();
    settle(&mut rt);
    assert!(has_instance_color(&rt, magenta), "custom theme A paints keywords in magenta");
    assert!(!has_instance_color(&rt, lime));

    // Switch live to custom theme B without mutating DiffState: row memos invalidate via look.version.
    rt.state.syntax_theme = Some(theme_b);
    rt.invalidate();
    settle(&mut rt);
    assert!(!has_instance_color(&rt, magenta), "stale magenta keyword colour is cleared");
    assert!(has_instance_color(&rt, lime), "custom theme B paints keywords in lime");

    // Clear override back to default SyntaxTheme::from_kit.
    rt.state.syntax_theme = None;
    rt.invalidate();
    settle(&mut rt);
    assert!(!has_instance_color(&rt, lime), "default theme restores kit syntax colours");
}

#[test]
fn ignore_edge_whitespace_split_renders_selects_and_copies_per_side_text() {
    // Line 1 is a hunk context line with spaces on old vs tab on new.
    // Lines 3..=10 have a middle line (line 6) inside a folded gap (with context(1)) so we also
    // verify expanded Row::Ctx rows keep per-side indentation.
    let old = "   keep\n  old\n   after\n  gap1\n    gap_ws\n  gap3\n   tail\n  end_old\n";
    let new = "\tkeep\n  new\n\tafter\n  gap1\n\tgap_ws\n  gap3\n\ttail\n  end_new\n";
    let opts = DiffOptions::new().context(1).ignore_edge_whitespace(true);
    let file = FileDiff::from_texts_with("a.txt", old, new, opts);
    assert_eq!((file.additions(), file.deletions()), (2, 2));

    let mut state = DiffState::new(vec![file]);
    state.set_mode(DiffMode::Split);
    state.expand(0, 1, Expand::All);

    let mut rt = mount(
        App { diff: state, custom_header: false, syntax_theme: None, header_clicks: 0 },
        (900.0, 500.0),
    );

    // Hunk context line 1: old half has 3 spaces, new half has tab (padded for display, uncopied pad).
    assert_eq!(find(&rt, "df0.c1/old/code/text").text.as_deref(), Some("   keep"));
    assert_eq!(find(&rt, "df0.c1/new/code/text").text.as_deref(), Some("\t   keep"));
    assert_eq!(find(&rt, "df0.c1/old").label.as_deref(), Some("unchanged line 1:    keep"));
    assert_eq!(find(&rt, "df0.c1/new").label.as_deref(), Some("unchanged line 1: \tkeep"));

    // Expanded Row::Ctx line 5 also preserves per-side text.
    assert_eq!(find(&rt, "df0.c5/old/code/text").text.as_deref(), Some("    gap_ws"));
    assert_eq!(find(&rt, "df0.c5/new/code/text").text.as_deref(), Some("\t   gap_ws"));

    // Gutter line selection copies each side's original text.
    rt.state.diff.set_selection(Some(LineRange { file: 0, start: LineRef::new(Side::Old, 1), end: LineRef::new(Side::Old, 2) }));
    assert_eq!(rt.state.diff.selected_text().as_deref(), Some("   keep\n  old"));
    rt.state.diff.set_selection(Some(LineRange { file: 0, start: LineRef::new(Side::New, 1), end: LineRef::new(Side::New, 2) }));
    assert_eq!(rt.state.diff.selected_text().as_deref(), Some("\tkeep\n  new"));
    rt.state.diff.set_selection(None);
    rt.invalidate();
    settle(&mut rt);

    // Drag-selecting and Cmd+C copying on each split half copies that side's original indentation.
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let copy_key = KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } };

    for (cell, expected, label) in [
        ("df0.c1/old/code/text", "   keep", "old side drag copy preserves leading spaces"),
        ("df0.c1/new/code/text", "\tkeep", "new side drag copy preserves leading tab"),
    ] {
        let line = find(&rt, cell);
        let (x0, y) = (line.rect[0] + 1.0, line.rect[1] + 10.0);
        let x1 = line.rect[0] + 200.0;
        rt.pointer_move(x0, y);
        tick(&mut rt);
        rt.pointer_down(x0, y);
        tick(&mut rt);
        for step in 1..=8 {
            rt.pointer_move(x0 + (x1 - x0) * (step as f32 / 8.0), y);
            tick(&mut rt);
        }
        rt.pointer_up(x1, y);
        tick(&mut rt);
        assert_eq!(rt.selected_text().as_deref(), Some(expected), "{label}");
        rt.key(copy_key.clone());
        tick(&mut rt);
        assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(expected), "{label}");
    }

    // Default from_texts remains unchanged: edge-whitespace-only line is still a change.
    let default_file = FileDiff::from_texts("a.txt", "   keep\n  old\n", "\tkeep\n  new\n");
    assert_eq!((default_file.additions(), default_file.deletions()), (2, 2));
}
