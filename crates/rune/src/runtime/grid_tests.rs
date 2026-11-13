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
