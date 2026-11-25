use crate::runtime::touch_input::TouchPhase;
use crate::{Cx, Drag, El, Env, Phase, Runtime, ScrollSpec, SwipeEdge, col, el, row};

#[derive(Default)]
struct S { page: bool, drawer: Vec<(Phase, f32, bool)>, back: Vec<Phase>, chart: Vec<(Phase, bool)>, taps: usize }

/// A full-window chart that drags sideways, a horizontal filmstrip under it, and (when `page`
/// is open) a page over both with its own back swipe.
fn view(s: &S, _: &mut Cx<S>) -> El {
    let chart = el().key("chart").size(300.0, 200.0)
        .on_press(|s: &mut S| s.taps += 1)
        .on_drag(|s: &mut S, d: &mut Drag, _| s.chart.push((d.phase, d.cancelled())));
    let strip = row().key("strip").w(300.0).h(100.0).scroll_x()
        .children((0..10).map(|i| el().key(format!("frame{i}")).size(100.0, 100.0)));
    let page = s.page.then(|| col().key("page").size(300.0, 300.0).absolute(0.0, 0.0)
        .on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut S, d: &mut Drag, _| s.back.push(d.phase))
        .child(el().key("page-body").size(300.0, 300.0).on_drag(|_: &mut S, _: &mut Drag, _| {})));
    col().key("root").size(300.0, 300.0)
        .on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut S, d: &mut Drag, _| s.drawer.push((d.phase, d.dx, d.cancelled())))
        .child(chart)
        .child(strip)
        .children(page)
}

fn rig(page: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S { page, ..S::default() }, view, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt
}

fn swipe(rt: &mut Runtime<S>, from: (f32, f32), to: (f32, f32)) {
    rt.touch(1, TouchPhase::Started, from.0, from.1, 0.0);
    for i in 1..=4 {
        let k = i as f32 / 4.0;
        rt.touch(1, TouchPhase::Moved, from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k, i as f64 * 0.01);
    }
    rt.touch(1, TouchPhase::Ended, to.0, to.1, 0.05);
    rt.frame(1.0 / 60.0);
}

fn strip_offset(rt: &Runtime<S>) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("strip")).unwrap().id;
    rt.scroll.scrolls.get(&id).map_or(0.0, |s| s.offset)
}

#[test]
fn a_swipe_in_from_the_edge_goes_to_the_ancestor_and_cancels_the_chart() {
    let mut rt = rig(false);
    swipe(&mut rt, (5.0, 100.0), (85.0, 100.0));
    let phases: Vec<_> = rt.state.drawer.iter().map(|d| d.0).collect();
    assert_eq!(phases.first(), Some(&Phase::Down));
    assert_eq!(phases.last(), Some(&Phase::Up));
    assert!(phases.contains(&Phase::Move));
    assert_eq!(rt.state.drawer.last().map(|d| (d.1, d.2)), Some((80.0, false)));
    assert_eq!(rt.state.chart.first(), Some(&(Phase::Down, false)), "the chart heard the finger land");
    assert_eq!(rt.state.chart.last(), Some(&(Phase::Up, true)), "and then let go of it, cancelled");
    assert!(!rt.state.chart.iter().any(|c| c.0 == Phase::Move), "it never dragged");
    assert_eq!(rt.state.taps, 0);
    assert!(!rt.touching());
}

#[test]
fn an_edge_swipe_beats_a_horizontal_scroll_viewport() {
    let mut rt = rig(false);
    swipe(&mut rt, (5.0, 250.0), (85.0, 250.0));
    assert_eq!(rt.state.drawer.last().map(|d| d.0), Some(Phase::Up));
    assert_eq!(strip_offset(&rt), 0.0, "the filmstrip didn't pan");
}

#[test]
fn a_move_along_the_edge_or_a_landing_off_it_leaves_the_touch_to_the_content() {
    let mut rt = rig(false);
    swipe(&mut rt, (5.0, 20.0), (5.0, 120.0));
    assert!(rt.state.drawer.is_empty(), "along the edge it isn't an edge swipe");
    swipe(&mut rt, (100.0, 100.0), (180.0, 100.0));
    assert!(rt.state.drawer.is_empty(), "off the edge it isn't either");
    assert_eq!(rt.state.chart.last(), Some(&(Phase::Up, false)), "the chart dragged");
}

#[test]
fn the_innermost_edge_handler_wins() {
    let mut rt = rig(true);
    swipe(&mut rt, (5.0, 100.0), (85.0, 100.0));
    assert_eq!(rt.state.back.first(), Some(&Phase::Down));
    assert_eq!(rt.state.back.last(), Some(&Phase::Up));
    assert!(rt.state.drawer.is_empty());
}

#[test]
fn a_mouse_drag_from_the_edge_is_the_content_s() {
    let mut rt = rig(false);
    rt.pointer_move(5.0, 100.0);
    rt.pointer_down(5.0, 100.0);
    rt.pointer_move(85.0, 100.0);
    rt.pointer_up(85.0, 100.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.drawer.is_empty());
    assert_eq!(rt.state.chart.last(), Some(&(Phase::Up, false)));
}

#[derive(Default)]
struct Z { edge: Vec<(Phase, f32, bool)>, slider: Vec<(Phase, bool)>, taps: usize, menus: usize }

/// A root with a left-edge swipe over a `touch_drags_at_once` slider and a vertical list,
/// both reaching the edge. The edge handler counts its moves in `Drag::data`.
fn zone_view(_: &Z, _: &mut Cx<Z>) -> El {
    let slider = el().key("slider").size(300.0, 60.0).touch_drags_at_once()
        .on_secondary_press(|s: &mut Z, _, _| s.menus += 1)
        .on_drag(|s: &mut Z, d: &mut Drag, _| s.slider.push((d.phase, d.cancelled())));
    let list = col().key("list").w(300.0).h(240.0).scroll(ScrollSpec { stick_bottom: false })
        .children((0..30).map(|i| el().key(format!("row{i}")).w_full().h(40.0).on_press(|s: &mut Z| s.taps += 1)));
    col().key("root").size(300.0, 300.0)
        .on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut Z, d: &mut Drag, _| {
            match d.phase { Phase::Down => d.data = 7.0, Phase::Move => d.data += 1.0, Phase::Up => {} }
            s.edge.push((d.phase, d.data, d.cancelled()));
        })
        .child(slider)
        .child(list)
}

fn zone_rig() -> Runtime<Z> {
    let mut rt = Runtime::new(Z::default(), zone_view, (300.0, 300.0), Env::default());
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.frame(0.0);
    rt
}

fn list_offset(rt: &Runtime<Z>) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    rt.scroll.scrolls.get(&id).map_or(0.0, |s| s.offset)
}

#[test]
fn an_at_once_drag_in_the_zone_has_no_long_press_and_still_drags_along_the_edge() {
    let mut rt = zone_rig();
    rt.touch(1, TouchPhase::Started, 5.0, 20.0, 0.0);
    assert_eq!(rt.pointer.touch.hold_deadline(), None, "no long press is armed");
    rt.frame.test_clock_mut().unwrap().seek(0.8);
    rt.frame(0.8);
    rt.touch(1, TouchPhase::Moved, 5.0, 50.0, 0.81);
    rt.touch(1, TouchPhase::Ended, 5.0, 50.0, 0.82);
    rt.frame(0.83);
    assert_eq!(rt.state.menus, 0, "holding still opened no menu");
    assert!(rt.state.slider.contains(&(Phase::Move, false)), "it dragged: {:?}", rt.state.slider);
    assert_eq!(rt.state.slider.last(), Some(&(Phase::Up, false)));
    assert!(rt.state.edge.is_empty());
}

#[test]
fn an_edge_swipe_over_a_vertical_list_claims_it_and_the_list_stays() {
    let mut rt = zone_rig();
    rt.touch(1, TouchPhase::Started, 5.0, 150.0, 0.0);
    for i in 1..=4 { rt.touch(1, TouchPhase::Moved, 5.0 + 20.0 * i as f32, 150.0 - 2.0 * i as f32, i as f64 * 0.01); }
    rt.touch(1, TouchPhase::Ended, 85.0, 142.0, 0.05);
    for _ in 0..10 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.state.edge.last().map(|e| (e.0, e.2)), Some((Phase::Up, false)));
    assert_eq!(list_offset(&rt), 0.0, "the list didn't pan or fling");
    assert_eq!(rt.state.taps, 0);
}

#[test]
fn a_cancelled_edge_swipe_hears_one_cancelled_up_and_presses_nothing() {
    let mut rt = zone_rig();
    rt.touch(1, TouchPhase::Started, 5.0, 150.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 40.0, 150.0, 0.01);
    rt.touch(1, TouchPhase::Cancelled, 40.0, 150.0, 0.02);
    rt.frame(1.0 / 60.0);
    let ups: Vec<_> = rt.state.edge.iter().filter(|e| e.0 == Phase::Up).collect();
    assert_eq!(ups.len(), 1, "{:?}", rt.state.edge);
    assert!(ups[0].2, "the Up is cancelled");
    assert_eq!(rt.state.taps, 0);
    assert!(!rt.touching());
}

#[test]
fn drag_data_carries_across_an_edge_swipe_s_phases() {
    let mut rt = zone_rig();
    rt.touch(1, TouchPhase::Started, 5.0, 150.0, 0.0);
    for i in 1..=4 { rt.touch(1, TouchPhase::Moved, 5.0 + 20.0 * i as f32, 150.0, i as f64 * 0.01); }
    rt.touch(1, TouchPhase::Ended, 85.0, 150.0, 0.05);
    rt.frame(1.0 / 60.0);
    let data: Vec<_> = rt.state.edge.iter().map(|e| (e.0, e.1)).collect();
    assert_eq!(data, [(Phase::Down, 7.0), (Phase::Move, 8.0), (Phase::Move, 9.0), (Phase::Move, 10.0), (Phase::Move, 11.0), (Phase::Up, 11.0)]);
}
