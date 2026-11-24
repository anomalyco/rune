use super::*;
use crate::el::{col, el, row};
use crate::WheelEvent;

const DT: f64 = 1.0 / 60.0;

#[derive(Default)]
struct VState {
    rows: usize,
    stick_bottom: bool,
}

fn vertical_view(s: &VState, _: &mut Cx<VState>) -> El {
    let rows = col().key("rows").w_full()
        .children((0..s.rows).map(|k| el().key(format!("r{k}")).w_full().h(50.0)));
    let spec = if s.stick_bottom { ScrollSpec::stick_bottom() } else { ScrollSpec::default() };
    col().size(200.0, 200.0)
        .child(col().key("scroll").size(200.0, 200.0).scroll(spec).child(rows))
}

fn scroll_state<'a, S: 'static>(rt: &'a Runtime<S>, key: &str) -> &'a ScrollState {
    let i = rt.tree.nodes.iter().position(|n| n.named(key)).unwrap_or_else(|| panic!("missing {key}"));
    let id = rt.tree.nodes[i].id;
    rt.scroll.scrolls.get(&id).unwrap_or_else(|| panic!("no scroll state for {key}"))
}

#[test]
fn stop_scroll_halts_vertical_glide_and_settles_idle() {
    let mut rt = Runtime::new(VState { rows: 40, stick_bottom: false }, vertical_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    rt.update(|_, tx| tx.scroll_to_with("scroll", 0.0, 1000.0, Spring::new(0.25, 0.0)));
    for _ in 0..5 {
        assert!(rt.frame(DT));
    }
    let mid = scroll_state(&rt, "scroll").offset;
    assert!(mid > 10.0 && mid < 900.0, "mid-glide: {mid}");
    assert!(scroll_state(&rt, "scroll").glide().is_some());
    assert!(scroll_state(&rt, "scroll").spring.is_some());

    rt.update(|_, tx| tx.stop_scroll("scroll"));
    assert!(!rt.frame(DT), "halted glide settles idle on the stop frame");
    assert_eq!(scroll_state(&rt, "scroll").offset, mid, "offset frozen at drawn position");
    assert!(scroll_state(&rt, "scroll").glide().is_none());
    assert!(scroll_state(&rt, "scroll").spring.is_none());

    assert!(!rt.frame(DT), "subsequent frame stays idle");
    assert_eq!(scroll_state(&rt, "scroll").offset, mid);
}

#[test]
fn stop_scroll_halts_vertical_and_horizontal_flings() {
    // Vertical fling on El::scroll(ScrollSpec::default()).
    let mut rt = Runtime::new(VState { rows: 40, stick_bottom: false }, vertical_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.fling(100.0, 100.0, -1800.0);
    for _ in 0..4 {
        assert!(rt.frame(DT));
    }
    let mid_y = scroll_state(&rt, "scroll").offset;
    assert!(mid_y > 10.0, "vertical fling moved: {mid_y}");
    assert!(scroll_state(&rt, "scroll").coast().abs() > 1.0);

    rt.update(|_, tx| tx.stop_scroll("scroll"));
    assert!(!rt.frame(DT), "vertical fling settles idle");
    assert_eq!(scroll_state(&rt, "scroll").offset, mid_y);
    assert_eq!(scroll_state(&rt, "scroll").coast(), 0.0);

    // Horizontal fling on horizontal-only El::scroll_x().
    fn strip_view(_: &(), _: &mut Cx<()>) -> El {
        let tiles = row().children((0..20).map(|k| el().key(format!("t{k}")).size(100.0, 100.0)));
        col().size(200.0, 100.0)
            .child(col().key("strip").size(200.0, 100.0).scroll_x().child(tiles))
    }
    let mut rt = Runtime::new((), strip_view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.fling_xy(100.0, 50.0, -1800.0, 0.0);
    for _ in 0..4 {
        assert!(rt.frame(DT));
    }
    let mid_x = scroll_state(&rt, "strip").x.offset;
    assert!(mid_x > 10.0, "horizontal fling moved: {mid_x}");
    assert!(scroll_state(&rt, "strip").x.coast().abs() > 1.0);

    rt.update(|_, tx| tx.stop_scroll("strip"));
    assert!(!rt.frame(DT), "horizontal fling settles idle");
    assert_eq!(scroll_state(&rt, "strip").x.offset, mid_x);
    assert_eq!(scroll_state(&rt, "strip").x.coast(), 0.0);
}

#[test]
fn stop_scroll_halts_both_axes_of_scroll_xy_and_resolves_descendant_keys() {
    fn board_view(_: &(), _: &mut Cx<()>) -> El {
        let board = (0..10).fold(col(), |c, r| {
            c.child((0..10).fold(row(), |rw, i| rw.child(el().key(format!("c{r}-{i}")).size(100.0, 100.0))))
        });
        col().size(200.0, 200.0)
            .child(col().key("board").size(200.0, 200.0).scroll_xy().child(board))
    }
    let mut rt = Runtime::new((), board_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    rt.update(|_, tx| tx.scroll_to("board", 500.0, 600.0));
    for _ in 0..4 {
        assert!(rt.frame(DT));
    }
    let st = scroll_state(&rt, "board");
    let (mid_x, mid_y) = (st.x.offset, st.offset);
    assert!(mid_x > 5.0 && mid_x < 500.0 && mid_y > 5.0 && mid_y < 600.0, "both axes mid-glide: ({mid_x}, {mid_y})");

    // Passing a descendant key resolves to its enclosing scroll viewport.
    rt.update(|_, tx| tx.stop_scroll("c2-2"));
    assert!(!rt.frame(DT), "both axes halted and settled idle");
    let st = scroll_state(&rt, "board");
    assert_eq!((st.x.offset, st.offset), (mid_x, mid_y));
    assert_eq!(st.x.motion, XMotion::Rest);
    assert!(st.glide().is_none());
}

#[test]
fn stop_scroll_halts_notched_wheel_ease_on_both_axes() {
    fn board_view(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 200.0)
            .child(col().key("board").size(200.0, 200.0).scroll_xy().child(el().size(1500.0, 1500.0)))
    }
    let mut rt = Runtime::new((), board_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    rt.wheel_event(WheelEvent::new(0.0, -120.0).at(100.0, 100.0).notch(true));
    assert!(rt.frame(DT), "vertical wheel ease running");
    let mid_y = scroll_state(&rt, "board").offset;
    assert!(scroll_state(&rt, "board").mode.wheel().is_some());

    rt.update(|_, tx| tx.stop_scroll("board"));
    assert!(!rt.frame(DT), "vertical wheel ease stopped and settled idle");
    assert_eq!(scroll_state(&rt, "board").offset, mid_y);
    assert!(scroll_state(&rt, "board").mode.wheel().is_none());

    rt.wheel_event(WheelEvent::new(-120.0, 0.0).at(100.0, 100.0).notch(true));
    assert!(rt.frame(DT), "horizontal wheel ease running");
    let mid_x = scroll_state(&rt, "board").x.offset;
    assert!(matches!(scroll_state(&rt, "board").x.motion, XMotion::Wheel(_)));

    rt.update(|_, tx| tx.stop_scroll("board"));
    assert!(!rt.frame(DT), "horizontal wheel ease stopped and settled idle");
    assert_eq!(scroll_state(&rt, "board").x.offset, mid_x);
    assert_eq!(scroll_state(&rt, "board").x.motion, XMotion::Rest);
}

#[test]
fn stop_scroll_drops_pending_reveal_for_target_only() {
    fn two_lists(_: &(), _: &mut Cx<()>) -> El {
        let pane = |name: &'static str| {
            let rows = col().w_full().children((0..40).map(|k| el().key(format!("r{k}")).w_full().h(50.0)));
            col().key(name).size(150.0, 200.0).scroll(ScrollSpec::default()).child(rows)
        };
        row().size(300.0, 200.0).child(pane("left")).child(pane("right"))
    }
    let mut rt = Runtime::new((), two_lists, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    rt.update(|_, tx| {
        tx.scroll_center("left/r30");
        tx.scroll_center("right/r30");
    });
    for _ in 0..4 {
        assert!(rt.frame(DT));
    }
    let left_mid = scroll_state(&rt, "left").offset;
    assert!(left_mid > 10.0 && left_mid < 1400.0, "left mid-glide: {left_mid}");
    assert_eq!(rt.scroll.requests.reveals().count(), 2, "both viewports hold a pending reveal");

    rt.update(|_, tx| tx.stop_scroll("left"));
    assert!(rt.frame(DT), "right is still gliding");
    assert_eq!(scroll_state(&rt, "left").offset, left_mid, "left stopped where it was");
    assert!(scroll_state(&rt, "left").glide().is_none());
    assert_eq!(rt.scroll.requests.reveals().count(), 1, "only left's pending reveal was dropped");
    assert!(rt.scroll.requests.reveals().all(|r| r.viewport.ends_with("right")));

    for _ in 0..240 {
        if !rt.frame(DT) { break; }
    }
    assert_eq!(scroll_state(&rt, "left").offset, left_mid, "left stayed frozen while right finished");
    assert_eq!(scroll_state(&rt, "right").offset, 30.0 * 50.0 - 75.0, "right landed centred on r30");
}

#[test]
fn stop_scroll_keeps_pinned_stick_bottom_and_halts_glide_to_bottom() {
    let mut rt = Runtime::new(VState { rows: 20, stick_bottom: true }, vertical_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(scroll_state(&rt, "scroll").pinned(), "opens pinned at bottom");

    // Calling stop_scroll while already pinned keeps the pin.
    rt.update(|_, tx| tx.stop_scroll("scroll"));
    assert!(!rt.frame(DT));
    assert!(scroll_state(&rt, "scroll").pinned(), "pinned viewport stays pinned");
    rt.update(|s, tx| { s.rows = 22; tx.instant(); });
    rt.frame(DT);
    assert_eq!(scroll_state(&rt, "scroll").offset, 22.0 * 50.0 - 200.0, "new rows still follow bottom");

    // Glide to bottom from the top, then stop mid-glide: unpinned and frozen.
    rt.update(|_, tx| tx.scroll_to_y("scroll", 0.0));
    rt.frame(DT);
    assert_eq!(scroll_state(&rt, "scroll").offset, 0.0);
    assert!(!scroll_state(&rt, "scroll").pinned());

    rt.update(|_, tx| tx.scroll_to_bottom("scroll"));
    for _ in 0..3 {
        assert!(rt.frame(DT));
    }
    let mid = scroll_state(&rt, "scroll").offset;
    let end = scroll_state(&rt, "scroll").max;
    assert!(mid > 10.0 && mid < end - 10.0, "mid-glide to bottom: {mid} / {end}");

    rt.update(|_, tx| tx.stop_scroll("scroll"));
    assert!(!rt.frame(DT), "glide to bottom stopped idle");
    assert_eq!(scroll_state(&rt, "scroll").offset, mid);
    assert!(!scroll_state(&rt, "scroll").pinned(), "mid-glide stop does not pin");
    rt.update(|s, _| s.rows = 24);
    rt.frame(DT);
    assert_eq!(scroll_state(&rt, "scroll").offset, mid, "new rows do not pull stopped viewport to bottom");
}

#[test]
fn stop_scroll_handles_missing_key_live_only_and_same_tx_scroll() {
    let mut rt = Runtime::new(VState { rows: 40, stick_bottom: false }, vertical_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Missing key is a no-op and does not disturb another viewport's glide.
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 1000.0));
    for _ in 0..3 { assert!(rt.frame(DT)); }
    let before = scroll_state(&rt, "scroll").offset;
    rt.update(|_, tx| tx.stop_scroll("does-not-exist"));
    assert!(rt.frame(DT), "existing glide continues");
    assert!(scroll_state(&rt, "scroll").offset > before);

    // update_if returning false still processes stop_scroll without rebuilding the view.
    let mid = scroll_state(&rt, "scroll").offset;
    let commits_before = rt.commit.commits;
    assert!(rt.update_if(|_, tx| { tx.stop_scroll("scroll"); false }), "stop_scroll marks frame_work stale");
    assert!(!rt.frame(DT), "stopped without animating");
    assert_eq!(rt.commit.commits, commits_before, "no view commit needed to stop scroll");
    assert_eq!(scroll_state(&rt, "scroll").offset, mid);

    // stop_scroll followed by scroll_to in the same transaction starts the new glide from rest.
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 1500.0));
    for _ in 0..5 { assert!(rt.frame(DT)); }
    assert!(scroll_state(&rt, "scroll").glide().unwrap() > 100.0);
    rt.update(|_, tx| {
        tx.stop_scroll("scroll");
        tx.scroll_to("scroll", 0.0, 1500.0);
    });
    assert!(rt.frame(DT), "new glide started after stop_scroll");
    assert!(scroll_state(&rt, "scroll").glide().is_some());
}
