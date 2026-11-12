//! A virtual list whose row heights keep changing for longer than the fill's bound still fills
//! every visible row once they settle (r262). Dev stopped at the bound and left the rows it still
//! wanted unmounted until something else asked for a frame. A wake after the list fills or goes
//! away draws nothing.

use std::cell::Cell;
use std::rc::Rc;

use super::virtual_fill::{FILL_FRAMES, State};
use super::*;
use crate::{GrowthSpec, ScrollSpec, col, el};

const ROWS: usize = 400;
const VIEWPORT: f32 = 200.0;
const ROW_H: f32 = 20.0;
const THIN: f32 = 4.0;

/// Rows that measure shorter the more of them the list builds: the fill's budget buys rows at the
/// estimate, so each frame covers less of the overscan than it looks like it will. The first
/// rows are full height and the rest thin, which is what a list with heights still settling does.
struct Thinning {
    builds: Rc<Cell<u32>>,
    /// The list keeps only a few rows, so it fills at once.
    short: bool,
    /// The list is gone from the view.
    gone: bool,
}

fn thinning() -> Runtime<Thinning> {
    let state = Thinning { builds: Rc::new(Cell::new(0)), short: false, gone: false };
    Runtime::new(state, thinning_view, (400.0, VIEWPORT), Env::new(true, 2.0))
}

fn thinning_view(s: &Thinning, _: &mut Cx<Thinning>) -> El {
    if s.gone { return el().key("empty").w_full().h_full(); }
    let builds = s.builds.clone();
    let count = if s.short { 3 } else { ROWS };
    let list = col().key("scroll").w(300.0).h(VIEWPORT).scroll(ScrollSpec { stick_bottom: false }).child(
        col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
            count,
            ROW_H,
            |_: &Thinning, i| format!("r{i}").into(),
            move |_: &Thinning, i| {
                let n = builds.get();
                builds.set(n + 1);
                let height = if n < 12 { ROW_H } else { THIN };
                el().key(format!("r{i}")).w_full().h(height).fill(Color::WHITE)
            },
        ),
    );
    // A second viewport, so another scroller can move while the list waits.
    let other = col().key("other").w(80.0).h(VIEWPORT).scroll(ScrollSpec { stick_bottom: false }).child(
        col().key("other-rows").w_full().children(
            (0..60).map(|i| el().key(format!("o{i}")).w_full().h(ROW_H).fill(Color::WHITE)),
        ),
    );
    crate::row().w_full().h_full().child(list).child(other)
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

/// Draws like a host until the fill reaches its bound, returning the deadline it armed.
fn draw_to_wait<S: 'static>(rt: &mut Runtime<S>) -> f64 {
    let mut drew = true;
    for _ in 0..400 {
        if matches!(rt.virtual_fill.state(), State::Waiting { .. }) {
            return rt.wake_deadline().expect("the wait armed a deadline");
        }
        if drew {
            drew = rt.frame(1.0 / 120.0);
            continue;
        }
        let Some(due) = rt.wake_deadline() else { panic!("the fill stopped before its bound: {:?}", rt.virtual_fill.state()) };
        drew = rt.frame((due - rt.clock).max(1.0 / 120.0));
    }
    panic!("the fill never reached its bound");
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
    let mut rt = thinning();
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

/// r367: the list unmounts while a fill wake waits, and that wake draws nothing when it comes due.
#[test]
fn a_wake_after_the_list_unmounts_draws_nothing() {
    let mut rt = thinning();
    let due = draw_to_wait(&mut rt);
    rt.update(|s, _| s.gone = true);
    // The frame that unmounts the list is also the one the wake comes due in.
    let dt = (due - rt.clock).max(1.0 / 120.0);
    rt.frame(dt);
    assert!(rt.clock >= due - 1e-9, "the deadline came due");
    assert_eq!(rt.virtual_fill.state(), State::Idle, "the unmounted list retires the wait, not resuming it");
    assert_eq!(rt.anim_sources & AnimSource::VIRTUAL_FILL, 0, "and the fill adds no frame");
    assert_eq!(rt.wake_deadline(), None, "nothing is left armed");
}

/// r373: a list that unmounts mid-wait must not leave a deadline that turns away the first fill
/// frame of the list that replaces it.
#[test]
fn a_replacement_list_draws_its_first_fill_frame() {
    let mut rt = thinning();
    let due = draw_to_wait(&mut rt);
    rt.update(|s, _| s.gone = true);
    // The frame the wake comes due in is also the one the list unmounts in.
    rt.frame((due - rt.clock).max(1.0 / 120.0));
    assert_eq!(rt.virtual_fill.state(), State::Idle, "the unmounted list retires the wait");
    // The replacement starts exactly as a list that was never here: its first frame fills.
    rt.update(|s, _| s.gone = false);
    rt.frame(1.0 / 120.0);
    assert_ne!(rt.anim_sources & AnimSource::VIRTUAL_FILL, 0, "the replacement's first fill frame draws");
    assert!(matches!(rt.virtual_fill.state(), State::Filling { .. }), "and starts a run: {:?}", rt.virtual_fill.state());
}

/// r372: another viewport scrolling is not this list's invalidation, so the fill keeps the budget
/// it has and still stops within its bound.
#[test]
fn an_unrelated_scroller_does_not_reset_the_fill() {
    let mut rt = thinning();
    let due = draw_to_wait(&mut rt);
    rt.wheel(340.0, 100.0, -60.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.clock < due, "the frame is before the fill's deadline");
    assert!(
        matches!(rt.virtual_fill.state(), State::Waiting { .. }),
        "another scroller leaves the wait alone: {:?}",
        rt.virtual_fill.state()
    );
    let frames = draw_while_asked(&mut rt, false);
    assert!(frames < 4_000, "the fill still stops: {frames} frames");
    assert_eq!(rt.wake_deadline(), None, "nothing is left armed");
}

/// r367: the wake that was waiting comes due after the list finished filling, and draws nothing.
#[test]
fn a_wake_after_the_fill_completes_draws_nothing() {
    let mut rt = thinning();
    let due = draw_to_wait(&mut rt);
    rt.update(|s, _| s.short = true);
    // The frame that fills the last rows is also the one the wake comes due in.
    let dt = (due - rt.clock).max(1.0 / 120.0);
    rt.frame(dt);
    assert!(rt.clock >= due - 1e-9, "the deadline came due");
    assert_eq!(rt.virtual_fill.state(), State::Idle, "the filled list retires the wait, not resuming it");
    assert_eq!(rt.anim_sources & AnimSource::VIRTUAL_FILL, 0, "and the fill adds no frame");
    assert_eq!(rt.wake_deadline(), None, "nothing is left armed");
}
