//! `Tx::scroll_to`, `Tx::scroll_to_instant` and `Tx::scroll_to_y`: moving a viewport to an
//! offset the app picked, gliding or at once, clamped, on virtual rows too; and
//! `Tx::scroll_key_to`, landing a row at a height in its viewport.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, row};

#[derive(Default)]
struct S { list: bool, virtual_rows: bool, rows: usize }

fn view(s: &S, _: &mut Cx<S>) -> El {
    if s.virtual_rows {
        let rows = col().key("rows").growth(GrowthSpec::default())
            .virtual_rows(100, 40.0, |_: &S, i| format!("r{i}").into(), |_: &S, _| el().w_full().h(40.0));
        return col().w_full().h_full().child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows));
    }
    if s.list {
        let rows = (0..s.rows).fold(col().w_full(), |c, i| c.child(el().key(format!("row{i}")).w_full().h(40.0)));
        return col().w_full().h_full().child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: true }).child(rows));
    }
    // A 600 x 600 pt board in a 300 x 200 pt viewport: 300 across and 400 down to scroll.
    let board = (0..6).fold(col(), |c, r| c.child(
        (0..6).fold(row(), |row, i| row.child(el().key(format!("cell{r}-{i}")).w(100.0).h(100.0)))));
    col().w_full().h_full().child(col().key("board").w_full().h(200.0).scroll_xy().child(board))
}

fn app(s: S) -> Runtime<S> {
    let mut rt = Runtime::new(s, view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn at(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"));
    (e.rect[0], e.rect[1])
}

fn settle(rt: &mut Runtime<S>) -> usize {
    for n in 0..600 { if !rt.frame(1.0 / 60.0) { return n; } }
    panic!("never settled");
}

#[test]
fn scroll_to_glides_both_axes_and_lands_clamped() {
    let mut rt = app(S::default());
    rt.update(|_, tx| tx.scroll_to("board", 150.0, 250.0));
    rt.frame(1.0 / 60.0);
    let (x, y) = at(&rt, "cell0-0");
    assert!(x < 0.0 && x > -150.0 && y < 0.0 && y > -250.0, "on its way, not there: {x}, {y}");
    assert!(settle(&mut rt) > 5, "a glide, not a jump");
    assert_eq!(at(&rt, "cell0-0"), (-150.0, -250.0));

    rt.update(|_, tx| tx.scroll_to("cell2-2", 9_999.0, -50.0));
    settle(&mut rt);
    assert_eq!(at(&rt, "cell0-0"), (-300.0, 0.0), "a key inside finds its viewport; each axis clamps");
}

#[test]
fn scroll_to_instant_jumps_and_a_wheel_takes_over_a_glide() {
    let mut rt = app(S::default());
    rt.update(|_, tx| tx.scroll_to_instant("board", 100.0, 120.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "cell0-0"), (-100.0, -120.0));

    rt.update(|_, tx| tx.scroll_to("board", 0.0, 400.0));
    for _ in 0..3 { rt.frame(1.0 / 60.0); }
    let (_, y) = at(&rt, "cell0-0");
    rt.wheel_xy(150.0, 100.0, 0.0, 30.0);
    rt.frame(1.0 / 60.0);
    settle(&mut rt);
    let (_, landed) = at(&rt, "cell0-0");
    assert!(landed > -400.0 && (landed - (y + 30.0)).abs() < 1.0, "the wheel stopped the glide where it was: {y} then {landed}");
}

#[test]
fn a_vertical_list_takes_y_alone_and_follows_its_end_again_once_sent_there() {
    let mut rt = app(S { list: true, rows: 20, ..S::default() });
    assert_eq!(at(&rt, "row19").1, 160.0, "stick_bottom opens at the end");
    rt.update(|_, tx| tx.scroll_to_y("list", 80.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row0").1, -80.0);
    rt.update(|s, _| s.rows = 21);
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row0").1, -80.0, "off the end it stays put as rows arrive");

    rt.update(|_, tx| tx.scroll_to("row3", 500.0, 9_999.0));
    settle(&mut rt);
    assert_eq!(at(&rt, "row20").1, 160.0, "x ignored, y clamped to the end");
    rt.update(|s, _| s.rows = 22);
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row21").1, 160.0, "and the end is followed again");
}

#[test]
fn scroll_to_lands_on_virtual_rows_it_mounts() {
    let mut rt = app(S { virtual_rows: true, ..S::default() });
    rt.update(|_, tx| tx.scroll_to_instant("list", 0.0, 2_000.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "r50").1, 0.0, "row 50 mounted at the top");

    rt.update(|_, tx| tx.scroll_to("list", 0.0, 3_000.0));
    settle(&mut rt);
    assert_eq!(at(&rt, "r75").1, 0.0);
    rt.update(|_, tx| tx.scroll_to("list", 0.0, 99_999.0));
    settle(&mut rt);
    assert_eq!(at(&rt, "r99").1, 160.0, "clamped to the end the rows measure");
}

#[test]
fn scroll_key_to_lands_a_row_where_asked_in_the_frame_that_builds_it() {
    let mut rt = app(S { list: true, rows: 20, ..S::default() });
    rt.update(|s, tx| {
        s.rows = 30;
        tx.scroll_key_to("row12", 30.0);
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row12").1, 30.0, "placed from the new rows, on the first frame");
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row12").1, 30.0, "and it stays");

    rt.update(|_, tx| tx.scroll_key_to("row29", 0.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row29").1, 160.0, "clamped to the end");
    rt.update(|_, tx| tx.scroll_key_to("row0", 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row0").1, 0.0, "and to the top");
}
