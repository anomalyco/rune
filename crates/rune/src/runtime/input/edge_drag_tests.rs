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

#[test]
fn drag_edge_reports_swipe_edge_inside_edge_drag_and_none_inside_on_drag() {
    #[derive(Default)]
    struct E { edge_seen: Vec<Option<SwipeEdge>>, drag_seen: Vec<Option<SwipeEdge>> }
    fn v(_: &E, _: &mut Cx<E>) -> El {
        col().size(300.0, 300.0)
            .on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut E, d: &mut Drag, _| s.edge_seen.push(d.edge()))
            .child(el().size(300.0, 300.0).on_drag(|s: &mut E, d: &mut Drag, _| s.drag_seen.push(d.edge())))
    }
    let mut rt = Runtime::new(E::default(), v, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.touch(1, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 60.0, 100.0, 0.02);
    rt.touch(1, TouchPhase::Ended, 60.0, 100.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edge_seen.len() >= 3 && rt.state.edge_seen.iter().all(|e| *e == Some(SwipeEdge::Left)));
    assert!(!rt.state.drag_seen.is_empty() && rt.state.drag_seen.iter().all(Option::is_none));
}

#[test]
fn unmount_escape_cancel_pointer_and_modal_deliver_cancelled_up_to_active_edge_drag() {
    #[derive(Default)]
    struct C { show_page: bool, inert_page: bool, modal: bool, events: Vec<(Phase, bool, Option<SwipeEdge>)> }
    fn v(s: &C, _: &mut Cx<C>) -> El {
        let page = s.show_page.then(|| {
            let e = col().key("page").size(300.0, 300.0)
                .on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut C, d: &mut Drag, _| {
                    s.events.push((d.phase, d.cancelled(), d.edge()));
                });
            if s.inert_page { e.inert() } else { e }
        });
        let sheet = s.modal.then(|| {
            el().key("sheet").absolute(100.0, 100.0).size(100.0, 100.0).modal(true, |s: &mut C| s.modal = false)
        });
        el().w_full().h_full().children(page).children(sheet)
    }

    // 1. Unmounting the element mid-edge-swipe delivers a cancelled Phase::Up via fallback handler.
    let mut rt = Runtime::new(C { show_page: true, ..C::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.touch(1, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 40.0, 100.0, 0.02);
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.show_page = false);
    rt.frame(1.0 / 60.0);
    assert!(rt.has_pending_input(), "sync_held_edge_drag queues cancelled Up for next frame");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events.last(), Some(&(Phase::Up, true, Some(SwipeEdge::Left))), "unmount delivers cancelled Up");
    let count = rt.state.events.len();
    assert!(rt.touching(), "finger is still down until lifted");
    rt.touch(1, TouchPhase::Moved, 90.0, 100.0, 0.04);
    rt.touch(1, TouchPhase::Ended, 90.0, 100.0, 0.06);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events.len(), count, "post-cancel moves and lift emit nothing");
    assert!(!rt.touching());

    // 2. Escape cancels an active edge drag and consumes Escape.
    let mut rt2 = Runtime::new(C { show_page: true, ..C::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt2.frame(0.0);
    rt2.touch(2, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt2.touch(2, TouchPhase::Moved, 40.0, 100.0, 0.02);
    rt2.frame(1.0 / 60.0);
    rt2.key(crate::KeyEvent { key: crate::Key::Escape, mods: crate::Mods::default() });
    rt2.frame(1.0 / 60.0);
    assert_eq!(rt2.state.events.last(), Some(&(Phase::Up, true, Some(SwipeEdge::Left))), "Escape delivers cancelled Up");
    rt2.touch(2, TouchPhase::Ended, 40.0, 100.0, 0.05);
    rt2.frame(1.0 / 60.0);
    assert_eq!(rt2.state.events.iter().filter(|e| e.0 == Phase::Up).count(), 1);

    // 3. cancel_pointer() (window focus loss) delivers cancelled Phase::Up once.
    let mut rt3 = Runtime::new(C { show_page: true, ..C::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt3.frame(0.0);
    rt3.touch(3, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt3.touch(3, TouchPhase::Moved, 40.0, 100.0, 0.02);
    rt3.frame(1.0 / 60.0);
    rt3.cancel_pointer();
    rt3.frame(1.0 / 60.0);
    assert_eq!(rt3.state.events.last(), Some(&(Phase::Up, true, Some(SwipeEdge::Left))), "cancel_pointer delivers cancelled Up");
    rt3.touch(3, TouchPhase::Ended, 40.0, 100.0, 0.05);
    rt3.frame(1.0 / 60.0);
    assert_eq!(rt3.state.events.iter().filter(|e| e.0 == Phase::Up).count(), 1);

    // 4. Opening a modal mid-edge-swipe delivers cancelled Phase::Up.
    let mut rt4 = Runtime::new(C { show_page: true, ..C::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt4.frame(0.0);
    rt4.touch(4, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt4.touch(4, TouchPhase::Moved, 40.0, 100.0, 0.02);
    rt4.frame(1.0 / 60.0);
    rt4.update(|s, _| s.modal = true);
    rt4.frame(1.0 / 60.0);
    rt4.frame(1.0 / 60.0);
    assert_eq!(rt4.state.events.last(), Some(&(Phase::Up, true, Some(SwipeEdge::Left))), "modal open delivers cancelled Up");
    rt4.touch(4, TouchPhase::Ended, 40.0, 100.0, 0.05);
    rt4.frame(1.0 / 60.0);
    assert_eq!(rt4.state.events.iter().filter(|e| e.0 == Phase::Up).count(), 1);

    // 5. Turning inert mid-edge-swipe delivers cancelled Phase::Up.
    let mut rt5 = Runtime::new(C { show_page: true, ..C::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt5.frame(0.0);
    rt5.touch(5, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt5.touch(5, TouchPhase::Moved, 40.0, 100.0, 0.02);
    rt5.frame(1.0 / 60.0);
    rt5.update(|s, _| s.inert_page = true);
    rt5.frame(1.0 / 60.0);
    rt5.frame(1.0 / 60.0);
    assert_eq!(rt5.state.events.last(), Some(&(Phase::Up, true, Some(SwipeEdge::Left))), "inert delivers cancelled Up");
}

#[test]
fn raised_overlay_clipped_out_opacity_zero_and_modal_block_occluded_edge_drag() {
    #[derive(Default)]
    struct O { bg: Vec<Phase>, modal_edge: Vec<Phase>, modal_open: bool, cover_raised: bool, bg_hidden: bool }
    fn v(s: &O, _: &mut Cx<O>) -> El {
        let mut page = col().key("page").size(300.0, 300.0)
            .on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut O, d: &mut Drag, _| s.bg.push(d.phase));
        if s.bg_hidden { page = page.opacity(0.0); }
        let cover = s.cover_raised.then(|| {
            el().key("cover").absolute(0.0, 0.0).size(40.0, 300.0).raise()
        });
        let modal = s.modal_open.then(|| {
            col().key("sheet").absolute(0.0, 0.0).size(300.0, 300.0)
                .modal(true, |s: &mut O| s.modal_open = false)
                .on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut O, d: &mut Drag, _| s.modal_edge.push(d.phase))
        });
        el().w_full().h_full().child(page).children(cover).children(modal)
    }

    // 1. Raised sibling panel covering the left edge blocks the page beneath.
    let mut rt = Runtime::new(O { cover_raised: true, ..O::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.touch(1, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 60.0, 100.0, 0.02);
    rt.touch(1, TouchPhase::Ended, 60.0, 100.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.bg.is_empty(), "raised panel occludes edge drag beneath it");

    // 2. opacity(0.0) page does not claim edge drags.
    let mut rt2 = Runtime::new(O { bg_hidden: true, ..O::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt2.frame(0.0);
    rt2.touch(2, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt2.touch(2, TouchPhase::Moved, 60.0, 100.0, 0.02);
    rt2.touch(2, TouchPhase::Ended, 60.0, 100.0, 0.04);
    rt2.frame(1.0 / 60.0);
    assert!(rt2.state.bg.is_empty(), "invisible opacity(0.0) element does not claim edge drag");

    // 3. Active modal blocks background edge drag while allowing edge drag inside the modal.
    let mut rt3 = Runtime::new(O { modal_open: true, ..O::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt3.frame(0.0);
    rt3.touch(3, TouchPhase::Started, 5.0, 100.0, 0.0);
    rt3.touch(3, TouchPhase::Moved, 60.0, 100.0, 0.02);
    rt3.touch(3, TouchPhase::Ended, 60.0, 100.0, 0.04);
    rt3.frame(1.0 / 60.0);
    assert!(rt3.state.bg.is_empty(), "modal blocks background edge drag");
    assert_eq!(rt3.state.modal_edge.last(), Some(&Phase::Up), "modal's own edge drag works");
}

#[test]
fn set_safe_area_expands_edge_band_and_updates_cx_safe_area_and_memos() {
    use std::cell::Cell;
    use std::rc::Rc;
    struct SafeS {
        seen_safe: Rc<Cell<[f32; 4]>>,
        memo_builds: Rc<Cell<usize>>,
        edges: Vec<SwipeEdge>,
    }
    fn v(s: &SafeS, cx: &mut Cx<SafeS>) -> El {
        let safe = cx.safe_area();
        s.seen_safe.set(safe);
        let builds = s.memo_builds.clone();
        let m = crate::memo("safe-box", 0u32, move || {
            builds.set(builds.get() + 1);
            el().size(300.0, 300.0)
        });
        col().size(300.0, 300.0)
            .on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut SafeS, d: &mut Drag, _| {
                if d.phase == Phase::Up && let Some(e) = d.edge() { s.edges.push(e); }
            })
            .child(m)
    }
    let seen = Rc::new(Cell::new([0.0; 4]));
    let builds = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(
        SafeS { seen_safe: seen.clone(), memo_builds: builds.clone(), edges: Vec::new() },
        v,
        (300.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(seen.get(), [0.0; 4]);
    assert_eq!(builds.get(), 1);

    // At x=55 with safe left=0, width=20: outside band [0..20].
    rt.touch(1, TouchPhase::Started, 55.0, 100.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 120.0, 100.0, 0.02);
    rt.touch(1, TouchPhase::Ended, 120.0, 100.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edges.is_empty(), "x=55 outside 20 pt band when safe_area left=0");

    // Landscape notch on left (safe left=44, negative/NaN clamped to 0.0): band is [0..64].
    rt.set_safe_area([-5.0, f32::NAN, 21.0, 44.0]);
    rt.frame(1.0 / 60.0);
    assert_eq!(seen.get(), [0.0, 0.0, 21.0, 44.0]);
    assert_eq!(builds.get(), 2, "changing safe_area invalidates memos");

    // Identical insets do not rebuild memos.
    rt.set_safe_area([0.0, 0.0, 21.0, 44.0]);
    rt.frame(1.0 / 60.0);
    assert_eq!(builds.get(), 2);

    // Now x=55 is inside [0, 44 + 20 = 64] and triggers the left-edge swipe.
    rt.touch(2, TouchPhase::Started, 55.0, 100.0, 0.10);
    rt.touch(2, TouchPhase::Moved, 120.0, 100.0, 0.12);
    rt.touch(2, TouchPhase::Ended, 120.0, 100.0, 0.14);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.edges, [SwipeEdge::Left]);
}

#[test]
fn multi_edge_corner_disambiguation_rtl_live_only_and_vertical_list_fallthrough() {
    #[derive(Default)]
    struct M {
        fired: Vec<(SwipeEdge, crate::Direction)>,
        taps: usize,
        live_only: bool,
    }
    fn v(_: &M, _: &mut Cx<M>) -> El {
        let handler = |s: &mut M, d: &mut Drag, tx: &mut crate::Tx| {
            if s.live_only { tx.live_only(); }
            if d.phase == Phase::Up && !d.cancelled() && let Some(e) = d.edge() {
                s.fired.push((e, d.direction()));
            }
        };
        let list = col().key("list").size(300.0, 300.0).scroll(ScrollSpec { stick_bottom: false })
            .children((0..20).map(|i| el().key(format!("r{i}")).w_full().h(40.0).on_press(|s: &mut M| s.taps += 1)));
        col().size(300.0, 300.0)
            .dir(crate::Direction::Rtl)
            .on_edge_drag(SwipeEdge::Left, 20.0, handler)
            .on_edge_drag(SwipeEdge::Right, 20.0, handler)
            .on_edge_drag(SwipeEdge::Top, 20.0, handler)
            .on_edge_drag(SwipeEdge::Bottom, 20.0, handler)
            .child(list)
    }
    let mut rt = Runtime::new(M::default(), v, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let list_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;

    // 1. Top-left corner (5, 5): swiping right claims Left, swiping down claims Top.
    let do_swipe = |rt: &mut Runtime<M>, from: (f32, f32), to: (f32, f32)| {
        rt.touch(1, TouchPhase::Started, from.0, from.1, 0.0);
        for i in 1..=4 {
            let k = i as f32 / 4.0;
            rt.touch(1, TouchPhase::Moved, from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k, i as f64 * 0.01);
        }
        rt.touch(1, TouchPhase::Ended, to.0, to.1, 0.05);
        rt.frame(1.0 / 60.0);
    };
    do_swipe(&mut rt, (5.0, 5.0), (80.0, 5.0));
    do_swipe(&mut rt, (5.0, 5.0), (5.0, 80.0));
    // 2. Right edge in RTL (inward leftward) and Bottom edge (inward upward).
    do_swipe(&mut rt, (295.0, 150.0), (220.0, 150.0));
    do_swipe(&mut rt, (150.0, 295.0), (150.0, 220.0));
    assert_eq!(
        rt.state.fired,
        [
            (SwipeEdge::Left, crate::Direction::Rtl),
            (SwipeEdge::Top, crate::Direction::Rtl),
            (SwipeEdge::Right, crate::Direction::Rtl),
            (SwipeEdge::Bottom, crate::Direction::Rtl),
        ]
    );
    assert_eq!(rt.scroll.scrolls[&list_id].offset, 0.0);
    assert_eq!(rt.wake_deadline(), None, "idle edge drag leaves no wake deadline");

    // 3. Tap inside left-edge band clicks the child row and never fires on_edge_drag.
    rt.state.fired.clear();
    rt.touch(10, TouchPhase::Started, 5.0, 100.0, 0.20);
    rt.touch(10, TouchPhase::Ended, 7.0, 100.0, 0.22);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.taps, 1, "tap within slop clicks child button in edge band");
    assert!(rt.state.fired.is_empty());

    // 4. Vertical swipe in left-edge band falls through to vertical list pan + fling.
    rt.touch(11, TouchPhase::Started, 5.0, 200.0, 0.30);
    for i in 1..=5 {
        rt.touch(11, TouchPhase::Moved, 5.0, 200.0 - 20.0 * i as f32, 0.30 + i as f64 * 0.01);
    }
    rt.touch(11, TouchPhase::Ended, 5.0, 100.0, 0.355);
    let lifted = rt.scroll.scrolls[&list_id].offset;
    for _ in 0..20 { rt.frame(1.0 / 60.0); }
    assert!(rt.scroll.scrolls[&list_id].offset > lifted + 20.0, "vertical swipe in left band pans and flings list");
    assert!(rt.state.fired.is_empty());

    // 5. tx.live_only() inside on_edge_drag skips view commits on every move/up.
    let mut rt_live = Runtime::new(M { live_only: true, ..M::default() }, v, (300.0, 300.0), Env::new(true, 1.0));
    rt_live.frame(0.0);
    let commits = rt_live.stats.commits;
    rt_live.touch(1, TouchPhase::Started, 5.0, 150.0, 0.0);
    rt_live.touch(1, TouchPhase::Moved, 40.0, 150.0, 0.01);
    rt_live.frame(1.0 / 60.0);
    rt_live.touch(1, TouchPhase::Moved, 80.0, 150.0, 0.02);
    rt_live.frame(1.0 / 60.0);
    rt_live.touch(1, TouchPhase::Ended, 80.0, 150.0, 0.03);
    rt_live.frame(1.0 / 60.0);
    assert_eq!(rt_live.stats.commits, commits, "live_only skips view commits during edge drag");
    assert_eq!(rt_live.state.fired, [(SwipeEdge::Left, crate::Direction::Rtl)]);
}

