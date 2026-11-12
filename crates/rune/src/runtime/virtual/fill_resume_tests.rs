//! A virtual list whose row heights keep changing for longer than the fill's bound still fills
//! every visible row once they settle (r262). Dev stopped at the bound and left the rows it still
//! wanted unmounted until something else asked for a frame.

use std::cell::Cell;
use std::rc::Rc;

use super::virtual_fill::FILL_FRAMES;
use super::*;
use crate::{GrowthSpec, ScrollSpec, col, el};

const ROWS: usize = 400;
const VIEWPORT: f32 = 200.0;
const ROW_H: f32 = 20.0;

/// Rows that measure shorter the more of them the list builds: the fill's budget buys rows at the
/// estimate, so each frame covers less of the overscan than it looks like it will. The first
/// rows are full height and the rest thin, which is what a list with heights still settling does.
struct Thinning(Rc<Cell<u32>>);

const THIN: f32 = 4.0;

fn thinning_view(s: &Thinning, _: &mut Cx<Thinning>) -> El {
    let builds = s.0.clone();
    col().w_full().h_full().child(
        col().key("scroll").w_full().h(VIEWPORT).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
                ROWS,
                ROW_H,
                |_: &Thinning, i| format!("r{i}").into(),
                move |_: &Thinning, i| {
                    let n = builds.get();
                    builds.set(n + 1);
                    let height = if n < 12 { ROW_H } else { THIN };
                    el().key(format!("r{i}")).w_full().h(height).fill(Color::WHITE)
                },
            ),
        ),
    )
}

/// Draws while the runtime asks for a frame and wakes at any timer it armed, like a host does.
/// Returns the frames drawn before nothing was left to draw.
fn draw_while_asked<S: 'static>(rt: &mut Runtime<S>, mut drew: bool) -> usize {
    let mut frames = 0;
    loop {
        frames += 1;
        assert!(frames < 4_000, "the fill kept the host drawing");
        if drew {
            drew = rt.frame(1.0 / 120.0);
            continue;
        }
        let Some(due) = rt.wake_deadline() else { return frames };
        drew = rt.frame((due - rt.clock).max(1.0 / 120.0));
    }
}

fn list_path<S>(rt: &Runtime<S>) -> String {
    rt.virtuals.keys().next().expect("a virtual list").clone()
}

/// The list's offset within its viewport, and whether the viewport is pinned to its end.
fn offset<S>(rt: &Runtime<S>, path: &str) -> (f32, bool) {
    let v = &rt.virtuals[path];
    let s = rt.path_ids.get(&v.scroll_path).and_then(|id| rt.scrolls.get(id));
    s.map_or((0.0, false), |s| ((s.offset as f32 - v.top).max(0.0), s.pinned()))
}

#[test]
fn heights_that_keep_changing_still_fill_every_visible_row() {
    let mut rt = Runtime::new(Thinning(Rc::new(Cell::new(0))), thinning_view, (400.0, VIEWPORT), Env::new(true, 2.0));
    rt.frame(0.0);
    let frames = draw_while_asked(&mut rt, true);
    let path = list_path(&rt);
    let (offset, pinned) = offset(&rt, &path);
    let v = &rt.virtuals[&path];
    let visible = v.visible_range(offset, pinned);
    assert!(
        v.range.start <= visible.start && v.range.end >= visible.end,
        "a visible row is blank after {frames} frames: mounted {:?}, visible {visible:?}",
        v.range
    );
    assert!(!v.needs_window(offset, pinned), "the fill stopped with rows unmounted after {frames} frames: mounted {:?}, visible {visible:?}", v.range);
    assert!(frames > FILL_FRAMES as usize, "the fill needed more than its bound: {frames} frames");
}

#[test]
fn an_idle_list_with_nothing_to_fill_draws_nothing() {
    struct Short;
    fn view(_: &Short, _: &mut Cx<Short>) -> El {
        col().w_full().h_full().child(
            col().key("scroll").w_full().h(VIEWPORT).scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
                    5,
                    ROW_H,
                    |_: &Short, i| format!("r{i}").into(),
                    |_: &Short, i| el().key(format!("r{i}")).w_full().h(ROW_H).fill(Color::WHITE),
                ),
            ),
        )
    }
    let mut rt = Runtime::new(Short, view, (400.0, VIEWPORT), Env::new(true, 2.0));
    rt.frame(0.0);
    let frames = draw_while_asked(&mut rt, true);
    assert_eq!(rt.wake_deadline(), None, "an idle list arms nothing, after {frames} frames");
    assert!(!rt.frame(1.0 / 120.0), "and nothing asks for another frame");
}
