//! A commit only a virtual window's shift caused reuses the last view build and builds only
//! the rows the window newly mounts (`memo::Kept`).

use std::cell::Cell;

use super::*;
use crate::el::row;

thread_local! {
    static VIEWS: Cell<usize> = const { Cell::new(0) };
    static ROWS: Cell<usize> = const { Cell::new(0) };
}

struct List { label: &'static str, read: bool }

fn view(s: &List, cx: &mut Cx<List>) -> El {
    VIEWS.set(VIEWS.get() + 1);
    if s.read { let _ = cx.layout_size("side"); }
    let label = s.label;
    let rows = col().key("rows").w_full().growth(crate::GrowthSpec::default()).virtual_rows(1000, 20.0, |_: &List, i| format!("r{i}").into(), move |_: &List, i| {
        ROWS.set(ROWS.get() + 1);
        row().w_full().h(20.0).child(crate::text(format!("{label} {i}")))
    });
    row().w_full().h_full()
        .child(col().key("side").w(100.0).h_full())
        .child(col().key("scroll").w(200.0).h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
}

fn runtime(read: bool) -> Runtime<List> {
    VIEWS.set(0);
    ROWS.set(0);
    let mut rt = Runtime::new(List { label: "a", read }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

/// The last row the list mounts.
fn last_row(rt: &Runtime<List>) -> usize {
    rt.elements().iter().filter_map(|e| e.key.as_deref()?.strip_prefix('r')?.parse().ok()).max().unwrap_or(0)
}

/// Wheels the list until it commits for a window shift, returning how many views and rows ran.
fn scroll_to_a_new_window(rt: &mut Runtime<List>) -> (usize, usize) {
    let (views, rows) = (VIEWS.get(), ROWS.get());
    for _ in 0..40 {
        rt.wheel(250.0, 100.0, -60.0);
        rt.frame(1.0 / 120.0);
    }
    (VIEWS.get() - views, ROWS.get() - rows)
}

#[test]
fn a_window_shift_builds_only_the_rows_it_mounts() {
    let mut rt = runtime(false);
    let (_, first) = scroll_to_a_new_window(&mut rt);
    let before = last_row(&rt);
    let (views, rows) = scroll_to_a_new_window(&mut rt);
    assert!(last_row(&rt) > before + 50, "the list scrolled to a new window: {before} to {}", last_row(&rt));
    assert_eq!(views, 0, "the view doesn't run again while only the window moves");
    assert!(rows > 0 && rows <= 160, "each newly mounted row builds once ({rows} rows, {first} before)");
}

#[test]
fn a_state_change_rebuilds_the_view_and_its_rows() {
    let mut rt = runtime(false);
    scroll_to_a_new_window(&mut rt);
    let (views, rows) = (VIEWS.get(), ROWS.get());
    rt.update(|s, _| s.label = "b");
    rt.frame(1.0 / 120.0);
    assert!(VIEWS.get() > views && ROWS.get() > rows, "the update re-ran the view and the mounted rows");
    let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    assert!(texts.iter().any(|t| t.starts_with("b ")) && !texts.iter().any(|t| t.starts_with("a ")), "{texts:?}");
    // Scrolling on after it mounts rows built from the new state.
    scroll_to_a_new_window(&mut rt);
    let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    assert!(!texts.iter().any(|t| t.starts_with("a ")), "no row from before the update comes back: {texts:?}");
}

#[test]
fn a_view_with_a_held_layout_read_is_still_reused() {
    let mut rt = runtime(true);
    scroll_to_a_new_window(&mut rt);
    let (views, _) = scroll_to_a_new_window(&mut rt);
    assert_eq!(views, 0, "a read whose rect didn't move doesn't re-run the view");
}

#[test]
fn memos_inside_a_reused_view_survive_to_its_next_build() {
    thread_local! { static SIDES: Cell<usize> = const { Cell::new(0) }; }
    fn view(n: &u32, _: &mut Cx<u32>) -> El {
        let rows = col().key("rows").w_full().growth(crate::GrowthSpec::default()).virtual_rows(1000, 20.0, |_: &u32, i| format!("r{i}").into(), |_: &u32, _| row().w_full().h(20.0));
        row().w_full().h_full()
            .child(crate::memo("side", 0, || { SIDES.set(SIDES.get() + 1); col().key("side").w(100.0).h_full() }))
            .child(col().key("scroll").w(200.0).h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
            .child(crate::text(format!("{n}")))
    }
    let mut rt = Runtime::new(0u32, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..80 {
        rt.wheel(150.0, 100.0, -60.0);
        rt.frame(1.0 / 120.0);
    }
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 120.0);
    assert_eq!(SIDES.get(), 1, "the memo built once and was reused after the window shifts");
}
