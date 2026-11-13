use std::rc::Rc;

use crate::{Color, Cx, El, Env, GridRow, Runtime, Span, TextStyle, col, grid};

struct Screen {
    rows: Vec<Rc<GridRow>>,
}

fn view(s: &Screen, _: &mut Cx<Screen>) -> El {
    col().w_full().h_full().child(grid(24, 8.0, 18.0, s.rows.clone()).w(192.0).h(18.0 * s.rows.len() as f32))
}

fn row(text: &str, k: usize) -> Rc<GridRow> {
    let style = TextStyle::new(14.0, 400.0).lh(18.0);
    let red = Color { r: 0.8, g: 0.1, b: 0.1, a: 1.0 };
    Rc::new(
        GridRow::new(text, vec![Span::new(0, text.len().min(4)).color(red)], style)
            .fill(0, 3 + k as u16, Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 })
            .cell(20, 2, "日", style, Color::WHITE),
    )
}

fn instances(rt: &Runtime<Screen>) -> Vec<String> {
    rt.instances.iter().map(|i| format!("{:?}", (i.rect, i.fill, i.params, i.uv, i.clip))).collect()
}

/// Where and how each instance draws, not where its mask sits in the atlas: two runtimes fill
/// their atlases in different orders.
fn placed(rt: &Runtime<Screen>) -> Vec<String> {
    rt.instances.iter().map(|i| format!("{:?}", (i.rect, i.fill, i.params[1], i.clip))).collect()
}

#[test]
fn an_unchanged_screen_draws_what_it_drew() {
    let rows: Vec<_> = (0..6).map(|k| row(&format!("line {k} of output"), k)).collect();
    let mut rt = Runtime::new(Screen { rows }, view, (240.0, 140.0), Env::new(true, 1.5));
    rt.frame(0.0);
    let first = instances(&rt);
    assert!(first.len() > 6 * 4, "fills, glyphs and the wide cell of each row");
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(instances(&rt), first, "rows drawn again from what they drew");
}

#[test]
fn one_changed_row_draws_as_a_fresh_screen_would() {
    let rows: Vec<_> = (0..6).map(|k| row(&format!("line {k} of output"), k)).collect();
    let mut rt = Runtime::new(Screen { rows }, view, (240.0, 140.0), Env::new(true, 1.5));
    rt.frame(0.0);
    rt.update(|s, _| s.rows[2] = row("a new third line", 9));
    rt.frame(1.0 / 60.0);
    let fresh_rows: Vec<_> = (0..6).map(|k| if k == 2 { row("a new third line", 9) } else { row(&format!("line {k} of output"), k) }).collect();
    let mut fresh = Runtime::new(Screen { rows: fresh_rows }, view, (240.0, 140.0), Env::new(true, 1.5));
    fresh.frame(0.0);
    assert_eq!(placed(&rt), placed(&fresh));
}

#[test]
fn a_reader_gets_a_line_per_row_and_the_cursor_as_its_caret() {
    struct Caret;
    fn view(_: &Caret, _: &mut Cx<Caret>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(18.0);
        let rows = vec![
            Rc::new(GridRow::new("$ ls", Vec::new(), style)),
            Rc::new(GridRow::new("ab", Vec::new(), style).cell(3, 2, "日", style, Color::WHITE)),
            Rc::new(GridRow::new("", Vec::new(), style)),
        ];
        col().child(grid(24, 8.0, 18.0, rows).key("screen").w(192.0).h(54.0).grid_caret(4, 1))
    }
    let mut rt = Runtime::new(Caret, view, (240.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_tree();
    let (_, screen) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("screen")).expect("the grid");
    assert_eq!(screen.role(), accesskit::Role::Terminal);
    let runs: Vec<_> = tree.nodes.iter().filter(|(_, n)| n.role() == accesskit::Role::TextRun).collect();
    assert_eq!(runs.iter().map(|(_, n)| n.value().unwrap_or("")).collect::<Vec<_>>(), ["$ ls", "ab 日", ""], "a line per row, its wide cell in place");
    let caret = screen.text_selection().expect("the cursor as the caret");
    assert_eq!(caret.focus.node, runs[1].0, "on the cursor's row");
    assert_eq!(caret.focus.character_index, 3, "the wide character whose second column the cursor is on");
}
