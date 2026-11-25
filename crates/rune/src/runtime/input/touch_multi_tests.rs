use super::*;
use super::touch_machine::{PAN_SLOP, ROTATE_SLOP_DEG};
use crate::{GesturePhase, Live, MagnifyEvent, Phase, RotateEvent, ScrollSpec, WheelEvent, col, el};

#[derive(Clone, Debug, PartialEq)]
enum EvKind {
    Wheel(WheelEvent),
    Magnify(MagnifyEvent),
    Rotate(RotateEvent),
}

struct CanvasState {
    log: Vec<EvKind>,
    sibling_log: Vec<EvKind>,
    pan: [f32; 2],
    zoom: f32,
    angle: f32,
    live_pan: Live<[f32; 2]>,
    live_zoom: Live<f32>,
    live_only: bool,
    accept_wheel: bool,
    accept_magnify: bool,
    accept_rotate: bool,
    enable_wheel: bool,
    enable_magnify: bool,
    enable_rotate: bool,
    taps: usize,
}

impl CanvasState {
    fn all() -> Self {
        Self {
            log: Vec::new(),
            sibling_log: Vec::new(),
            pan: [0.0, 0.0],
            zoom: 1.0,
            angle: 0.0,
            live_pan: Live::new([0.0, 0.0]),
            live_zoom: Live::new(1.0),
            live_only: false,
            accept_wheel: true,
            accept_magnify: true,
            accept_rotate: true,
            enable_wheel: true,
            enable_magnify: true,
            enable_rotate: true,
            taps: 0,
        }
    }

    /// World point under canvas-local point `q`, for `screen = world * zoom + pan`.
    fn world(&self, q: [f32; 2]) -> [f32; 2] {
        [(q[0] - self.pan[0]) / self.zoom, (q[1] - self.pan[1]) / self.zoom]
    }
}

fn canvas_view(s: &CanvasState, _: &mut Cx<CanvasState>) -> El {
    let mut chart = el()
        .key("chart")
        .absolute(50.0, 50.0)
        .size(200.0, 200.0)
        .on_press(|s: &mut CanvasState| s.taps += 1);
    if s.enable_wheel {
        chart = chart.on_wheel(|s: &mut CanvasState, ev: WheelEvent, tx| {
            s.log.push(EvKind::Wheel(ev));
            if s.accept_wheel {
                s.pan[0] += ev.dx;
                s.pan[1] += ev.dy;
                if s.live_only {
                    s.live_pan.set(s.pan);
                    tx.live_only();
                }
            }
            s.accept_wheel
        });
    }
    if s.enable_magnify {
        chart = chart.on_magnify(|s: &mut CanvasState, ev: MagnifyEvent, tx| {
            s.log.push(EvKind::Magnify(ev));
            if s.accept_magnify {
                let w = s.world([ev.x, ev.y]);
                s.zoom *= 1.0 + ev.delta;
                s.pan = [ev.x - w[0] * s.zoom, ev.y - w[1] * s.zoom];
                if s.live_only {
                    s.live_pan.set(s.pan);
                    s.live_zoom.set(s.zoom);
                    tx.live_only();
                }
            }
            s.accept_magnify
        });
    }
    if s.enable_rotate {
        chart = chart.on_rotate(|s: &mut CanvasState, ev: RotateEvent, tx| {
            s.log.push(EvKind::Rotate(ev));
            if s.accept_rotate {
                s.angle += ev.degrees;
                if s.live_only {
                    tx.live_only();
                }
            }
            s.accept_rotate
        });
    }
    let sibling = el()
        .key("sibling")
        .absolute(270.0, 50.0)
        .size(100.0, 200.0)
        .on_wheel(|s: &mut CanvasState, ev, _| {
            s.sibling_log.push(EvKind::Wheel(ev));
            true
        })
        .on_magnify(|s: &mut CanvasState, ev, _| {
            s.sibling_log.push(EvKind::Magnify(ev));
            true
        })
        .on_rotate(|s: &mut CanvasState, ev, _| {
            s.sibling_log.push(EvKind::Rotate(ev));
            true
        });
    col()
        .key("list")
        .size(400.0, 400.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(el().w_full().h(1200.0).child(chart).child(sibling))
}

fn rig(s: CanvasState) -> Runtime<CanvasState> {
    let mut rt = Runtime::new(s, canvas_view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn list_offset(rt: &Runtime<CanvasState>) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    rt.scroll.scrolls[&id].offset
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn two_finger_parallel_drag_delivers_touch_wheel_events_without_magnify_or_outer_scroll() {
    let mut rt = rig(CanvasState::all());
    // Two fingers 80 pt apart, initial midpoint (150, 150) -> chart local (100, 100).
    rt.touch(1, TouchPhase::Started, 110.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 190.0, 150.0, 0.01);
    // Move both fingers right by 8 pt (within PAN_SLOP = 8 pt): emits nothing.
    rt.touch(1, TouchPhase::Moved, 118.0, 150.0, 0.02);
    rt.touch(2, TouchPhase::Moved, 198.0, 150.0, 0.03);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.log.is_empty(), "midpoint travel <= PAN_SLOP ({PAN_SLOP} pt) emits nothing");

    // Move both fingers right to +20 pt total (span unchanged at 80 pt): crosses 8 pt slop cleanly.
    rt.touch(1, TouchPhase::Moved, 130.0, 150.0, 0.04);
    rt.touch(2, TouchPhase::Moved, 210.0, 150.0, 0.05);
    rt.touch(2, TouchPhase::Ended, 210.0, 150.0, 0.06);
    rt.touch(1, TouchPhase::Ended, 130.0, 150.0, 0.07);
    rt.frame(1.0 / 60.0);

    let wheels: Vec<WheelEvent> = rt.state.log.iter().filter_map(|e| match *e { EvKind::Wheel(w) => Some(w), _ => None }).collect();
    assert!(!wheels.is_empty());
    assert_eq!(wheels.first().map(|w| (w.phase, w.dx, w.dy)), Some((GesturePhase::Began, 0.0, 0.0)));
    assert_eq!(wheels.last().map(|w| (w.phase, w.dx, w.dy)), Some((GesturePhase::Ended, 0.0, 0.0)));
    for w in &wheels {
        assert!(w.touch && w.precise && !w.momentum && !w.notch, "touch wheel flags: {w:?}");
    }
    // Total pan equals 20 - 8 (slop subtracted) = 12 pt right, 0 pt vertically.
    assert!(approx(rt.state.pan[0], 12.0) && approx(rt.state.pan[1], 0.0), "slop subtracted pan: {:?}", rt.state.pan);
    // Neither magnify nor rotate fired during a pure parallel drag.
    assert!(rt.state.log.iter().all(|e| matches!(e, EvKind::Wheel(_))), "no magnify/rotate: {:?}", rt.state.log);
    assert_eq!(list_offset(&rt), 0.0, "outer scroll list never moved");
    assert_eq!(rt.state.taps, 0);
}

#[test]
fn simultaneous_pinch_and_pan_emits_wheel_before_magnify_and_pins_both_finger_world_points() {
    let mut rt = rig(CanvasState::all());
    // Hold thumb at (100, 150) (local 50, 100) and spread index from (140, 150) -> (200, 150) -> (260, 150).
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 140.0, 150.0, 0.01);
    // Cross both slop thresholds so both channels are active.
    rt.touch(2, TouchPhase::Moved, 180.0, 150.0, 0.02);
    rt.frame(1.0 / 60.0);

    // Record world points under both fingers right after activation.
    let f1_local = [50.0, 100.0];
    let f2_local = [130.0, 100.0];
    let w1 = rt.state.world(f1_local);
    let w2 = rt.state.world(f2_local);

    // Now move BOTH fingers simultaneously: thumb slides right+down to (120, 170), index to (260, 170).
    rt.state.log.clear();
    rt.touch(1, TouchPhase::Moved, 120.0, 170.0, 0.03);
    rt.touch(2, TouchPhase::Moved, 260.0, 170.0, 0.04);
    rt.frame(1.0 / 60.0);

    // Every move step emits Wheel strictly before Magnify.
    assert!(matches!(rt.state.log.first(), Some(EvKind::Wheel(_))), "wheel precedes magnify: {:?}", rt.state.log);
    assert!(matches!(rt.state.log.get(1), Some(EvKind::Magnify(_))), "magnify follows wheel: {:?}", rt.state.log);

    // Both fingers' world points stay pinned 1:1 to their new local coordinates within 0.5 pt.
    let s1 = [w1[0] * rt.state.zoom + rt.state.pan[0], w1[1] * rt.state.zoom + rt.state.pan[1]];
    let s2 = [w2[0] * rt.state.zoom + rt.state.pan[0], w2[1] * rt.state.zoom + rt.state.pan[1]];
    assert!((s1[0] - 70.0).abs() < 0.5 && (s1[1] - 120.0).abs() < 0.5, "thumb world point drifted: {s1:?}");
    assert!((s2[0] - 210.0).abs() < 0.5 && (s2[1] - 120.0).abs() < 0.5, "index world point drifted: {s2:?}");
    assert_eq!(list_offset(&rt), 0.0);

    // Also verify symmetric pinch where pan did NOT cross 8 pt slop still begins pan at (0, 0) ahead of Magnify(Began).
    let mut rt2 = rig(CanvasState::all());
    rt2.touch(1, TouchPhase::Started, 110.0, 150.0, 0.0);
    rt2.touch(2, TouchPhase::Started, 190.0, 150.0, 0.01); // initial mid (150, 150), span 80
    // Spread symmetrically: mid stays (150, 150) (0 pt displacement <= 8 pt), span 80 -> 100 (> 16 pt).
    rt2.touch(1, TouchPhase::Moved, 100.0, 150.0, 0.02); // mid (145, 150) dist 5 <= 8, span 90 diff 10 <= 16
    rt2.touch(2, TouchPhase::Moved, 200.0, 150.0, 0.03); // mid (150, 150) dist 0 <= 8, span 100 diff 20 > 16!
    rt2.frame(1.0 / 60.0);
    assert_eq!(rt2.state.log.len(), 3, "Wheel(Began 0,0), Magnify(Began), Magnify(Moved): {:?}", rt2.state.log);
    assert!(matches!(rt2.state.log[0], EvKind::Wheel(w) if w.phase == GesturePhase::Began && w.dx == 0.0 && w.dy == 0.0 && w.touch));
    assert!(matches!(rt2.state.log[1], EvKind::Magnify(m) if m.phase == GesturePhase::Began && m.delta == 0.0));
    assert!(matches!(rt2.state.log[2], EvKind::Magnify(m) if m.phase == GesturePhase::Moved));
}

#[test]
fn pure_pinch_with_five_degree_twist_skips_rotate_while_twist_past_eight_degrees_fires_ccw_without_jump() {
    let mut rt = rig(CanvasState::all());
    // Start horizontal (angle 0 deg), span 100.
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 200.0, 150.0, 0.01);
    // Spread to span 140 with +5 deg incidental CCW twist (< 8 deg ROTATE_SLOP_DEG): no on_rotate.
    let rad5 = 5.0_f32.to_radians();
    rt.touch(2, TouchPhase::Moved, 100.0 + 140.0 * rad5.cos(), 150.0 - 140.0 * rad5.sin(), 0.02);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.log.iter().any(|e| matches!(e, EvKind::Magnify(_))));
    assert!(rt.state.log.iter().all(|e| !matches!(e, EvKind::Rotate(_))), "5 deg twist <= {ROTATE_SLOP_DEG} deg stays silent");

    // Twist further CCW to +20 deg total (excess past 8 deg slop = +12 deg).
    let rad20 = 20.0_f32.to_radians();
    rt.touch(2, TouchPhase::Moved, 100.0 + 140.0 * rad20.cos(), 150.0 - 140.0 * rad20.sin(), 0.03);
    // Twist further CCW to +35 deg (+15 deg step).
    let rad35 = 35.0_f32.to_radians();
    rt.touch(2, TouchPhase::Moved, 100.0 + 140.0 * rad35.cos(), 150.0 - 140.0 * rad35.sin(), 0.04);
    rt.touch(2, TouchPhase::Ended, 100.0 + 140.0 * rad35.cos(), 150.0 - 140.0 * rad35.sin(), 0.05);
    rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.06);
    rt.frame(1.0 / 60.0);

    let rots: Vec<RotateEvent> = rt.state.log.iter().filter_map(|e| match *e { EvKind::Rotate(r) => Some(r), _ => None }).collect();
    assert_eq!(rots.len(), 4, "Began + 2 Moved + Ended: {rots:?}");
    assert_eq!((rots[0].phase, rots[0].degrees), (GesturePhase::Began, 0.0));
    assert_eq!(rots[1].phase, GesturePhase::Moved);
    assert!((rots[1].degrees - 12.0).abs() < 0.05, "slop subtracted: got {}", rots[1].degrees);
    assert_eq!(rots[2].phase, GesturePhase::Moved);
    assert!((rots[2].degrees - 15.0).abs() < 0.05, "step delta: got {}", rots[2].degrees);
    assert_eq!((rots[3].phase, rots[3].degrees), (GesturePhase::Ended, 0.0));
    assert!((rt.state.angle - 27.0).abs() < 0.05, "total angle: {}", rt.state.angle);
}

#[test]
fn two_finger_rotation_wraps_across_180_branch_cut_and_ignores_coincident_fingers_below_min_span() {
    let mut rt = rig(CanvasState { enable_wheel: false, enable_magnify: false, ..CanvasState::all() });
    // 1. Coincident start (span 0 < 16 pt MIN_SPAN): spinning within 10 pt must not fire on_rotate.
    rt.touch(1, TouchPhase::Started, 150.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 150.0, 150.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 155.0, 150.0, 0.02);
    rt.touch(2, TouchPhase::Moved, 150.0, 145.0, 0.03); // 90 deg at span 5 < MIN_SPAN -> ignored
    rt.touch(2, TouchPhase::Ended, 150.0, 145.0, 0.04);
    rt.touch(1, TouchPhase::Ended, 150.0, 150.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.log.is_empty(), "fingers below MIN_SPAN never trigger rotation");

    // 2. Branch cut across +175 deg -> -170 deg (+15 deg CCW, excess 7 deg) -> -155 deg (+15 deg CCW).
    //    Also drift midpoint onto sibling to verify RotateOwner stays locked to chart until Ended.
    let pt = |cx: f32, cy: f32, deg: f32| {
        let r = deg.to_radians();
        (cx + 60.0 * r.cos(), cy - 60.0 * r.sin())
    };
    let p0 = pt(150.0, 150.0, 175.0);
    let p1 = pt(150.0, 150.0, -170.0);
    let p2 = pt(320.0, 150.0, -155.0);
    rt.touch(3, TouchPhase::Started, 150.0, 150.0, 0.10);
    rt.touch(4, TouchPhase::Started, p0.0, p0.1, 0.11);
    rt.touch(4, TouchPhase::Moved, p1.0, p1.1, 0.12);
    rt.frame(1.0 / 60.0);
    rt.touch(3, TouchPhase::Moved, 320.0, 150.0, 0.13);
    rt.touch(4, TouchPhase::Moved, p2.0, p2.1, 0.14);
    rt.touch(4, TouchPhase::Ended, p2.0, p2.1, 0.15);
    rt.touch(3, TouchPhase::Ended, 320.0, 150.0, 0.16);
    rt.frame(1.0 / 60.0);

    assert!(rt.state.sibling_log.is_empty(), "RotateOwner stays locked to chart across bounds");
    assert!((rt.state.angle - 22.0).abs() < 0.1, "branch-cut CCW total (15-8)+15=22 deg, got {}", rt.state.angle);
    assert!(matches!(rt.state.log.last(), Some(EvKind::Rotate(r)) if r.phase == GesturePhase::Ended));
}

#[test]
fn declining_touch_wheel_and_magnify_only_canvas_never_scroll_outer_scroll_view() {
    // 1. on_wheel returns false during two-finger touch -> never falls through to outer scroll list.
    let mut rt = rig(CanvasState { accept_wheel: false, enable_magnify: false, enable_rotate: false, ..CanvasState::all() });
    rt.touch(1, TouchPhase::Started, 110.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 190.0, 150.0, 0.01);
    rt.touch(1, TouchPhase::Moved, 110.0, 90.0, 0.02);
    rt.touch(2, TouchPhase::Moved, 190.0, 90.0, 0.03);
    rt.touch(2, TouchPhase::Ended, 190.0, 90.0, 0.04);
    rt.touch(1, TouchPhase::Ended, 110.0, 90.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(list_offset(&rt), 0.0, "declining touch on_wheel must never scroll outer list");
    assert_eq!(rt.state.log.len(), 1, "only Began offered; declined owner ignores Moved/Ended");

    // 2. Direct Runtime::wheel_event with touch(true) is also handler-only and never scrolls list.
    rt.state.log.clear();
    rt.wheel_event(WheelEvent::new(0.0, -40.0).at(150.0, 150.0).touch(true));
    rt.frame(1.0 / 60.0);
    assert_eq!(list_offset(&rt), 0.0, "direct touch WheelEvent never falls through to scroll view");

    // 3. on_magnify-only element inside col().scroll never scrolls outer list when midpoint moves vertically.
    let mut rt2 = rig(CanvasState { enable_wheel: false, enable_magnify: true, enable_rotate: false, ..CanvasState::all() });
    rt2.touch(1, TouchPhase::Started, 110.0, 150.0, 0.0);
    rt2.touch(2, TouchPhase::Started, 190.0, 150.0, 0.01);
    rt2.touch(1, TouchPhase::Moved, 90.0, 90.0, 0.02);
    rt2.touch(2, TouchPhase::Moved, 210.0, 90.0, 0.03);
    rt2.touch(2, TouchPhase::Ended, 210.0, 90.0, 0.04);
    rt2.touch(1, TouchPhase::Ended, 90.0, 90.0, 0.05);
    rt2.frame(1.0 / 60.0);
    assert_eq!(list_offset(&rt2), 0.0);
    assert!(rt2.state.log.iter().all(|e| matches!(e, EvKind::Magnify(_))));
}

#[test]
fn touch_pan_owner_locks_across_bounds_and_live_only_skips_commits_for_all_three_channels() {
    let mut rt = rig(CanvasState { live_only: true, ..CanvasState::all() });
    rt.touch(1, TouchPhase::Started, 110.0, 150.0, 0.0);
    rt.touch(2, TouchPhase::Started, 190.0, 150.0, 0.01);
    rt.frame(1.0 / 60.0);
    let commits_before = rt.stats.commits;

    // Cross pan, pinch, and rotate slop, then drift midpoint completely onto sibling.
    rt.touch(1, TouchPhase::Moved, 120.0, 160.0, 0.02);
    rt.touch(2, TouchPhase::Moved, 230.0, 110.0, 0.03);
    rt.frame(1.0 / 60.0);
    rt.touch(1, TouchPhase::Moved, 280.0, 150.0, 0.04);
    rt.touch(2, TouchPhase::Moved, 370.0, 120.0, 0.05);
    rt.touch(2, TouchPhase::Ended, 370.0, 120.0, 0.06);
    rt.touch(1, TouchPhase::Ended, 280.0, 150.0, 0.07);
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.stats.commits, commits_before, "live_only skips view commits across wheel, magnify, and rotate");
    assert!(rt.state.sibling_log.is_empty(), "all three channel owners stay locked to chart across bounds");
    assert!(rt.state.log.iter().any(|e| matches!(e, EvKind::Wheel(_))));
    assert!(rt.state.log.iter().any(|e| matches!(e, EvKind::Magnify(_))));
    assert!(rt.state.log.iter().any(|e| matches!(e, EvKind::Rotate(_))));
}

#[test]
fn plain_scroll_area_without_canvas_handlers_ignores_second_finger_and_keeps_panning_and_flinging() {
    #[derive(Default)]
    struct ListOnly { taps: usize }
    fn list_view(_: &ListOnly, _: &mut Cx<ListOnly>) -> El {
        col()
            .key("list")
            .size(300.0, 300.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .children((0..30).map(|i| {
                el().key(format!("r{i}")).w_full().h(40.0).on_press(|s: &mut ListOnly| s.taps += 1)
            }))
    }
    let mut rt = Runtime::new(ListOnly::default(), list_view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    rt.touch(1, TouchPhase::Started, 100.0, 250.0, 0.0);
    for i in 1..=4 {
        rt.touch(1, TouchPhase::Moved, 100.0, 250.0 - 20.0 * i as f32, i as f64 * 0.01);
    }
    // Second finger lands and drags while finger 1 pans: ignored completely.
    rt.touch(2, TouchPhase::Started, 200.0, 200.0, 0.045);
    rt.touch(2, TouchPhase::Moved, 260.0, 120.0, 0.05);
    rt.touch(2, TouchPhase::Ended, 260.0, 120.0, 0.055);
    assert!(rt.pointer.touch.single().is_some(), "plain scroll list stays in Single");
    rt.touch(1, TouchPhase::Moved, 100.0, 140.0, 0.06);
    rt.touch(1, TouchPhase::Ended, 100.0, 140.0, 0.065);
    let lifted = rt.scroll.scrolls[&id].offset;
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(rt.scroll.scrolls[&id].offset > lifted + 30.0, "single-finger pan and fling stay uninterrupted");
    assert_eq!(rt.state.taps, 0);
}

#[test]
fn active_edge_drag_ignores_second_finger_over_on_wheel_and_on_rotate_while_pending_yields() {
    #[derive(Default)]
    struct EdgeMulti {
        edge: Vec<Phase>,
        wheels: Vec<WheelEvent>,
        rotates: Vec<RotateEvent>,
    }
    fn edge_multi_view(_: &EdgeMulti, _: &mut Cx<EdgeMulti>) -> El {
        col()
            .key("root")
            .size(300.0, 300.0)
            .on_edge_drag(crate::SwipeEdge::Left, 20.0, |s: &mut EdgeMulti, d: &mut Drag, _| s.edge.push(d.phase))
            .child(
                el().key("pane")
                    .size(300.0, 300.0)
                    .on_wheel(|s: &mut EdgeMulti, ev, _| { s.wheels.push(ev); true })
                    .on_rotate(|s: &mut EdgeMulti, ev, _| { s.rotates.push(ev); true }),
            )
    }
    let mut rt = Runtime::new(EdgeMulti::default(), edge_multi_view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // 1. Active Mode::Edge ignores second finger over on_wheel + on_rotate.
    rt.touch(1, TouchPhase::Started, 8.0, 150.0, 0.0);
    rt.touch(1, TouchPhase::Moved, 30.0, 150.0, 0.01); // claims Mode::Edge
    rt.touch(2, TouchPhase::Started, 130.0, 150.0, 0.02);
    rt.touch(2, TouchPhase::Moved, 130.0, 90.0, 0.03);
    rt.touch(2, TouchPhase::Ended, 130.0, 90.0, 0.04);
    rt.touch(1, TouchPhase::Ended, 80.0, 150.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.wheels.is_empty() && rt.state.rotates.is_empty(), "active edge drag ignores second finger");
    assert_eq!(rt.state.edge.first(), Some(&Phase::Down));
    assert_eq!(rt.state.edge.last(), Some(&Phase::Up));

    // 2. While still Pending in edge band, second finger over on_wheel + on_rotate starts TwoFinger and drops edge candidate.
    rt.state.edge.clear();
    rt.touch(3, TouchPhase::Started, 8.0, 150.0, 0.10);
    rt.touch(4, TouchPhase::Started, 108.0, 150.0, 0.11);
    rt.touch(3, TouchPhase::Moved, 28.0, 150.0, 0.12);
    rt.touch(4, TouchPhase::Moved, 128.0, 110.0, 0.13);
    rt.touch(4, TouchPhase::Ended, 128.0, 110.0, 0.14);
    rt.touch(3, TouchPhase::Ended, 28.0, 150.0, 0.15);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edge.is_empty(), "pending edge candidate discarded when two-finger pan/rotate takes over");
    assert!(!rt.state.wheels.is_empty() && !rt.state.rotates.is_empty());
}

#[test]
fn split_ancestor_owners_and_phased_trackpad_rotate_lock_independently() {
    #[derive(Default)]
    struct Split { outer_wheels: Vec<WheelEvent>, inner_magnifies: Vec<MagnifyEvent>, inner_rotates: Vec<RotateEvent> }
    fn split_view(_: &Split, _: &mut Cx<Split>) -> El {
        el().key("outer").size(300.0, 300.0)
            .on_wheel(|s: &mut Split, ev, _| { s.outer_wheels.push(ev); true })
            .child(
                el().key("inner").absolute(40.0, 40.0).size(160.0, 160.0)
                    .on_magnify(|s: &mut Split, ev, _| { s.inner_magnifies.push(ev); true })
                    .on_rotate(|s: &mut Split, ev, _| { s.inner_rotates.push(ev); true }),
            )
    }
    let mut rt = Runtime::new(Split::default(), split_view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Two-finger pan+pinch+rotate over inner locks outer for wheel and inner for magnify & rotate independently.
    rt.touch(1, TouchPhase::Started, 80.0, 120.0, 0.0);
    rt.touch(2, TouchPhase::Started, 160.0, 120.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 210.0, 80.0, 0.02);
    rt.touch(2, TouchPhase::Ended, 210.0, 80.0, 0.03);
    rt.touch(1, TouchPhase::Ended, 80.0, 120.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.outer_wheels.is_empty());
    assert!(!rt.state.inner_magnifies.is_empty());
    assert!(!rt.state.inner_rotates.is_empty());

    // Phased trackpad RotateEvent locks inner across bounds, while unphased RotateEvent still hit-tests fresh.
    rt.state.inner_rotates.clear();
    rt.rotate_event(RotateEvent::new(0.0).at(100.0, 100.0).phase(GesturePhase::Began));
    rt.rotate_event(RotateEvent::new(12.0).at(280.0, 280.0).phase(GesturePhase::Moved));
    rt.rotate_event(RotateEvent::new(0.0).at(280.0, 280.0).phase(GesturePhase::Ended));
    rt.rotate_event(RotateEvent::new(5.0).at(280.0, 280.0)); // unphased outside inner -> ignored
    rt.rotate_event(RotateEvent::new(7.0).at(100.0, 100.0)); // unphased inside inner -> delivered
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.inner_rotates.len(), 4);
    assert_eq!(rt.state.inner_rotates[1].degrees, 12.0);
    assert_eq!(rt.state.inner_rotates[3].degrees, 7.0);
}
