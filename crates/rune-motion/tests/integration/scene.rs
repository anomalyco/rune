use rune_motion::prelude::*;
use rune_motion::{MotionTrace, Phase, SMALL_SCALE_FLOOR, TraceHeader, flags};

const HZ: f64 = 120.0;

#[test]
fn pixel_scale_resnaps_settled_nodes_without_retargeting() {
    let make = |scale| {
        let mut scene = Scene::new();
        scene.set_pixel_scale(scale);
        scene.insert(NodeId(1), spec(0.123, 0.137, 30.123, 20.137), Mount::Initial);
        scene.frame_at(0.0);
        scene
    };
    let mut scene = make(1.0);
    for scale in [2.0, 1.5, 1.0] {
        scene.set_pixel_scale(scale);
        assert_eq!(scene.frame_at(0.0).changed.len(), 1, "scale changes invalidate settled poses");
        let fresh = make(scale);
        assert_eq!(scene.presented(NodeId(1)).unwrap().rect, fresh.presented(NodeId(1)).unwrap().rect);
        scene.set_pixel_scale(scale);
        assert!(scene.frame_at(0.0).changed.is_empty(), "an unchanged scale does no work");
    }
}

#[test]
fn instant_frame_lands_an_unchanged_target_mid_flight() {
    let mut s = Scene::new();
    let id = NodeId(1);
    s.insert(id, spec(0.0, 0.0, 100.0, 40.0), Mount::Initial);
    s.frame_at(0.0);
    let target = Rect::new(30.0, 60.0, 200.0, 120.0);
    s.set_frame(id, target, Spring::new(0.32, 0.0));
    assert!(s.frame_at(0.05).animating);
    s.set_frame(id, target, LayoutAnim::INSTANT);
    assert!(!s.frame_at(0.05).animating);
    assert_eq!(s.presented(id).unwrap().rect, target);
}

fn find(out: &[Presented], id: NodeId) -> Option<Presented> {
    out.iter().find(|p| p.node == id).copied()
}

fn spec(x: f32, y: f32, w: f32, h: f32) -> NodeSpec {
    NodeSpec::new(Rect::new(x, y, w, h))
}

#[test]
fn first_mount_does_not_animate() {
    let mut s = Scene::new();
    s.insert(NodeId(1), spec(0.0, 0.0, 100.0, 40.0).transition(Transition::blur_replace()), Mount::Initial);
    let out = s.frame_at(0.0);
    let p = find(out.changed, NodeId(1)).unwrap();
    assert_eq!((p.opacity, p.blur, p.transform.scale.x), (1.0, 0.0, 1.0));
    assert!(!out.animating);
    assert_eq!(p.phase, Phase::Present);
}

/// Steps the scene at `HZ` until `until`, collecting every presented value of `id`.
fn run(s: &mut Scene, from: f64, until: f64, id: NodeId) -> (Vec<(f64, Presented)>, Option<f64>) {
    let mut seen = Vec::new();
    let mut exited_at = None;
    let mut t = from;
    while t <= until + 1e-9 {
        let out = s.frame_at(t);
        if let Some(p) = find(out.changed, id) {
            seen.push((t, p));
        }
        if out.exited.contains(&id) && exited_at.is_none() {
            exited_at = Some(t);
        }
        t += 1.0 / HZ;
    }
    (seen, exited_at)
}

#[test]
fn blur_replace_enters_and_exits_and_never_unmounts_visible() {
    let mut s = Scene::new();
    let id = NodeId(2);
    s.insert(id, spec(0.0, 0.0, 200.0, 60.0).transition(Transition::blur_replace()), Mount::Animated);
    let (enter, _) = run(&mut s, 0.0, 1.0, id);
    let first = enter[0].1;
    assert!(first.opacity < 1e-3 && (first.blur - 1.5).abs() < 1e-3 && (first.transform.scale.x - 0.96).abs() < 1e-4);
    let last = enter.last().unwrap().1;
    assert_eq!((last.opacity, last.blur, last.transform.scale.x), (1.0, 0.0, 1.0), "rests exactly, no filter");
    assert_eq!(s.phase(id), Some(Phase::Present));
    let enter_time = enter.last().unwrap().0;

    s.set_time(2.0);
    s.remove(id);
    let (exit, exited_at) = run(&mut s, 2.0, 3.0, id);
    let exited_at = exited_at.expect("exit finishes");
    // Drawn every frame until it's gone; the final presented frame is fully transparent.
    let final_frame = exit.iter().rev().find(|(t, _)| *t <= exited_at).unwrap().1;
    assert!(final_frame.opacity < 1.0 / 255.0, "unmounted at opacity {}", final_frame.opacity);
    for (_, p) in &exit {
        assert!(p.blur <= 1.5 + 1e-3 && p.transform.scale.x >= 0.9);
        assert!(!p.in_flow, "popLayout: exiting nodes leave flow");
    }
    assert!(!s.contains(id));
    // Exits run shorter than entrances.
    assert!(exited_at - 2.0 < enter_time, "exit {} vs enter {}", exited_at - 2.0, enter_time);
}

#[test]
fn an_exit_unmounts_once_it_fades_out_of_sight() {
    for transition in [Transition::opacity(), Transition::push(Edge::Bottom), Transition::scale(0.9)] {
        let mut s = Scene::new();
        let id = NodeId(4);
        s.insert(id, spec(0.0, 0.0, 120.0, 40.0).transition(transition), Mount::Initial);
        s.frame_at(0.0);
        s.remove(id);
        let (exit, exited_at) = run(&mut s, 0.0, 1.0, id);
        let exited_at = exited_at.expect("exit finishes");
        let unseen = exit.iter().find(|(_, p)| p.opacity < 1.0 / 255.0).expect("fades out").0;
        assert_eq!(exited_at, unseen, "unmounts the frame it can no longer be seen");
    }
}

fn same_look(a: &Presented, b: &Presented) {
    let close = |x: f32, y: f32| (x - y).abs() < 2e-3;
    assert!(close(a.opacity, b.opacity), "opacity {} → {}", a.opacity, b.opacity);
    assert!(close(a.blur, b.blur), "blur {} → {}", a.blur, b.blur);
    assert!(close(a.transform.scale.x, b.transform.scale.x));
    assert!(close(a.rect.x, b.rect.x) && close(a.rect.y, b.rect.y), "{:?} → {:?}", a.rect, b.rect);
}

#[test]
fn a_key_that_returns_mid_exit_revives_without_a_jump() {
    for transition in [Transition::blur_replace(), Transition::push(Edge::Bottom), Transition::scale(0.9)] {
        let mut s = Scene::new();
        let id = NodeId(3);
        let sp = spec(0.0, 0.0, 120.0, 40.0).transition(transition);
        s.insert(id, sp, Mount::Initial);
        s.frame_at(0.0);
        s.remove(id);
        let (_, gone) = run(&mut s, 0.0, 0.1, id);
        assert!(gone.is_none());
        let t = 0.1 + 1.0 / HZ;
        let before = find(s.frame_at(t).changed, id).unwrap();
        s.insert(id, sp, Mount::Animated); // same key comes back
        let after = find(s.frame_at(t).changed, id).unwrap();
        same_look(&before, &after);
        assert_eq!(after.phase, Phase::Entering);
        // ... and lands exactly at rest.
        let (seen, gone) = run(&mut s, t, t + 1.5, id);
        assert!(gone.is_none());
        let rest = seen.last().unwrap().1;
        assert_eq!((rest.opacity, rest.blur, rest.transform.scale.x), (1.0, 0.0, 1.0));
        assert_eq!(rest.rect, Rect::new(0.0, 0.0, 120.0, 40.0));
        // The same, the other way round: removed mid-entrance.
        let mut s = Scene::new();
        s.insert(NodeId(9), spec(0.0, 0.0, 10.0, 10.0), Mount::Initial);
        s.frame_at(0.0);
        s.insert(id, sp, Mount::Animated);
        run(&mut s, 0.0, 0.08, id);
        let t = 0.08 + 1.0 / HZ;
        let before = find(s.frame_at(t).changed, id).unwrap();
        s.remove(id);
        let after = find(s.frame_at(t).changed, id).unwrap();
        same_look(&before, &after);
        let (seen, gone) = run(&mut s, t, t + 1.0, id);
        assert!(gone.is_some());
        assert!(seen.last().unwrap().1.opacity < 1.0 / 255.0);
    }
}

#[test]
fn a_key_that_returns_mid_exit_on_an_initial_mount_lands_at_rest() {
    for transition in [Transition::opacity(), Transition::blur_replace(), Transition::push(Edge::Bottom)] {
        let mut s = Scene::new();
        let id = NodeId(5);
        let sp = spec(0.0, 0.0, 120.0, 40.0).transition(transition);
        s.insert(id, sp, Mount::Initial);
        s.frame_at(0.0);
        s.remove(id);
        // Deep in the exit tail.
        let (seen, gone) = run(&mut s, 0.0, 0.2, id);
        assert!(gone.is_none() && seen.last().unwrap().1.opacity < 0.2);
        let t = 0.2 + 1.0 / HZ;
        let moved = Rect::new(10.0, 30.0, 140.0, 40.0);
        s.insert(id, NodeSpec::new(moved).transition(transition), Mount::Initial);
        let out = s.frame_at(t);
        assert!(!out.animating, "nothing left to animate");
        let p = find(out.changed, id).unwrap();
        assert_eq!((p.opacity, p.blur, p.transform.scale.x, p.phase), (1.0, 0.0, 1.0, Phase::Present));
        assert_eq!(p.rect, moved, "at its final layout");
        // The stale exit never retires it.
        let (seen, gone) = run(&mut s, t, t + 1.0, id);
        assert!(gone.is_none() && s.contains(id));
        assert!(seen.iter().all(|(_, p)| p.opacity == 1.0 && p.rect == moved));
    }
}

#[test]
fn an_initial_mount_revive_lands_motion_toward_the_same_frame_and_screen_travel() {
    for screen in [false, true] {
        let mut s = Scene::new();
        let id = NodeId(6);
        let target = Rect::new(60.0, 80.0, 120.0, 40.0);
        s.insert(id, spec(0.0, 0.0, 120.0, 40.0).transition(Transition::opacity()), Mount::Initial);
        s.frame_at(0.0);
        let slow = LayoutAnim::new(Spring::new(1.0, 0.0));
        s.set_frame(id, target, if screen { slow.screen() } else { slow });
        s.frame_at(0.05);
        s.remove(id);
        let (seen, gone) = run(&mut s, 0.05, 0.15, id);
        assert!(gone.is_none() && seen.last().unwrap().1.rect != target, "still travelling");
        let t = 0.15 + 1.0 / HZ;
        s.insert(id, NodeSpec::new(target).transition(Transition::opacity()), Mount::Initial);
        let out = s.frame_at(t);
        assert!(!out.animating, "nothing left to animate (screen: {screen})");
        let p = find(out.changed, id).unwrap();
        assert_eq!((p.opacity, p.rect, p.phase), (1.0, target, Phase::Present));
        let (seen, _) = run(&mut s, t, t + 1.0, id);
        assert!(seen.iter().all(|(_, p)| p.opacity == 1.0 && p.rect == target));
    }
}

#[test]
fn stationary_edges_never_move_and_width_leads() {
    let mut s = Scene::new();
    let id = NodeId(4);
    s.insert(id, spec(0.0, 0.0, 100.0, 50.0), Mount::Initial);
    s.frame_at(0.0);
    s.set_frame(id, Rect::new(0.0, 20.0, 300.0, 30.0), LayoutAnim::new(motion::REFLOW).width_leads());
    let (seen, _) = run(&mut s, 0.0, 1.5, id);
    assert!(seen.len() > 30);
    let mut x_ahead = 0;
    for (_, p) in &seen {
        assert_eq!(p.rect.x, 0.0, "left edge is stationary");
        assert_eq!(p.rect.y + p.rect.h, 50.0, "bottom edge is stationary");
        let xp = (p.rect.right() - 100.0) / 200.0;
        let yp = p.rect.y / 20.0;
        if xp > yp + 1e-3 {
            x_ahead += 1;
        }
    }
    assert!(x_ahead > 10, "width leads ({x_ahead} frames ahead)");
    assert_eq!(seen.last().unwrap().1.rect, Rect::new(0.0, 20.0, 300.0, 30.0));
}

fn max_path_deviation(points: &[(f32, f32)], start: (f32, f32), end: (f32, f32)) -> f32 {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let len = (dx * dx + dy * dy).sqrt();
    points
        .iter()
        .map(|&(x, y)| ((x - start.0) * dy - (y - start.1) * dx).abs() / len)
        .fold(0.0, f32::max)
}

#[test]
fn screen_travel_takes_one_straight_path() {
    let child_path = |screen: bool| {
        let mut s = Scene::new();
        let (p, c) = (NodeId(10), NodeId(11));
        s.insert(p, spec(0.0, 0.0, 400.0, 400.0), Mount::Initial);
        s.insert(c, spec(10.0, 10.0, 50.0, 50.0).parent(p), Mount::Initial);
        s.frame_at(0.0);
        // One commit: the parent glides, the child moves inside it on a different spring.
        s.set_frame(p, Rect::new(200.0, 100.0, 400.0, 400.0), motion::GLIDE);
        let child = LayoutAnim::new(motion::DRIFT);
        s.set_frame(c, Rect::new(100.0, 200.0, 50.0, 50.0), if screen { child.screen() } else { child });
        let (seen, _) = run(&mut s, 0.0, 3.0, c);
        let pts: Vec<_> = seen.iter().map(|(_, q)| (q.rect.x, q.rect.y)).collect();
        let last = *pts.last().unwrap();
        (pts, last, s)
    };
    let (pts, last, mut s) = child_path(true);
    assert_eq!(last, (300.0, 300.0));
    let dev = max_path_deviation(&pts, (10.0, 10.0), (300.0, 300.0));
    assert!(dev < 0.05, "screen travel deviates {dev} pt from a straight line");
    // Monotone: never reverses.
    for w in pts.windows(2) {
        assert!(w[1].0 >= w[0].0 - 1e-3 && w[1].1 >= w[0].1 - 1e-3);
    }
    // Once there, the child rides its parent again.
    s.set_frame(NodeId(10), Rect::new(0.0, 0.0, 400.0, 400.0), Anim::INSTANT);
    let out = s.frame_at(3.1);
    assert_eq!(find(out.changed, NodeId(11)).unwrap().rect.origin(), Vec2::new(100.0, 200.0));

    let (pts, _, _) = child_path(false);
    let dev = max_path_deviation(&pts, (10.0, 10.0), (300.0, 300.0));
    assert!(dev > 2.0, "parent-space travel with mixed springs dog-legs ({dev} pt): the test bites");
}

#[test]
fn matched_geometry_morphs_one_surface() {
    let mut s = Scene::new();
    let (chip, panel) = (NodeId(20), NodeId(21));
    let red = Color::hex(0xff3b30);
    let white = Color::WHITE;
    s.insert(chip, spec(10.0, 10.0, 80.0, 30.0).radius(15.0).fill(red), Mount::Initial);
    let before = find(s.frame_at(0.0).changed, chip).unwrap();
    s.set_time(1.0 / HZ);
    s.insert(panel, spec(0.0, 100.0, 300.0, 200.0).radius(12.0).fill(white), Mount::Initial);
    s.match_geometry(chip, panel, LayoutAnim::new(motion::MORPH).width_leads());
    let out = s.frame_at(1.0 / HZ);
    assert!(out.exited.contains(&chip));
    let first = find(out.changed, panel).unwrap();
    assert_eq!(first.rect, before.rect, "starts exactly where the chip was seen");
    assert_eq!(first.radius, Corners::all(15.0));
    assert!((first.fill.r - red.r).abs() < 2e-3 && first.opacity == 1.0);
    let (seen, _) = run(&mut s, 2.0 / HZ, 2.0, panel);
    let last = seen.last().unwrap().1;
    assert_eq!(last.rect, Rect::new(0.0, 100.0, 300.0, 200.0));
    assert_eq!(last.radius, Corners::all(12.0));
}

#[test]
fn reduced_motion_swaps_travel_for_a_short_fade() {
    let mut s = Scene::new();
    s.set_reduced_motion(true);
    let id = NodeId(30);
    s.insert(NodeId(31), spec(0.0, 0.0, 10.0, 10.0), Mount::Initial);
    s.frame_at(0.0);
    s.insert(id, spec(0.0, 0.0, 100.0, 40.0).transition(Transition::blur_replace().combined(Transition::push(Edge::Top))), Mount::Animated);
    let first = find(s.frame_at(0.0).changed, id).unwrap();
    assert_eq!((first.blur, first.transform.scale.x, first.rect.y), (0.0, 1.0, 0.0), "no blur, scale or travel");
    assert!(first.opacity < 1e-3);
    let mid = find(s.frame_at(0.09).changed, id).unwrap();
    assert!(mid.opacity > 0.3 && mid.opacity < 0.7, "{}", mid.opacity);
    assert_eq!(find(s.frame_at(0.18).changed, id).unwrap().opacity, 1.0);
    s.set_frame(id, Rect::new(50.0, 50.0, 100.0, 40.0), motion::GLIDE);
    assert_eq!(find(s.frame_at(0.2).changed, id).unwrap().rect.origin(), Vec2::new(50.0, 50.0), "layout jumps");
}

#[test]
fn children_ride_their_parent_in_and_out() {
    let mut s = Scene::new();
    let (p, c) = (NodeId(40), NodeId(41));
    s.insert(NodeId(1), spec(0.0, 0.0, 1.0, 1.0), Mount::Initial);
    s.frame_at(0.0);
    s.insert(p, spec(0.0, 0.0, 100.0, 100.0).transition(Transition::move_edge(Edge::Leading)), Mount::Animated);
    s.insert(c, spec(10.0, 10.0, 20.0, 20.0).parent(p), Mount::Animated);
    let out = s.frame_at(0.0);
    let (pp, cp) = (find(out.changed, p).unwrap(), find(out.changed, c).unwrap());
    assert_eq!(cp.phase, Phase::Present, "rides the entering parent instead of animating itself");
    assert_eq!(cp.rect.x, pp.rect.x + 10.0, "moves with the parent's presence offset");
    s.set_time(1.0);
    s.remove(p);
    let (_, gone) = run(&mut s, 1.0, 2.0, p);
    assert!(gone.is_some());
    assert!(!s.contains(c));
}

#[test]
fn a_child_mounted_after_its_parent_visibly_arrives_enters_on_its_own() {
    let mut s = Scene::new();
    let (p, c) = (NodeId(42), NodeId(43));
    s.insert(NodeId(1), spec(0.0, 0.0, 1.0, 1.0), Mount::Initial);
    s.frame_at(0.0);
    s.insert(p, spec(0.0, 0.0, 100.0, 100.0), Mount::Animated);
    let mut t = 0.0;
    loop {
        t += 1.0 / HZ;
        let out = s.frame_at(t);
        if find(out.changed, p).is_some_and(|pp| pp.opacity >= 1.0 - 1.0 / 255.0) { break; }
        assert!(t < 2.0, "the parent arrives");
    }
    assert_eq!(s.phase(p), Some(Phase::Entering), "its presence is still settling");
    s.set_time(t + 1.0 / HZ);
    s.insert(c, spec(10.0, 10.0, 20.0, 20.0).parent(p), Mount::Animated);
    let cp = find(s.frame().changed, c).unwrap();
    assert_eq!(cp.phase, Phase::Entering, "fades in instead of popping in on a finished entrance");
    assert!(cp.opacity < 0.5, "{}", cp.opacity);
}

#[test]
fn an_entrance_that_changes_nothing_lands_at_once() {
    let mut s = Scene::new();
    s.insert(NodeId(1), spec(0.0, 0.0, 1.0, 1.0), Mount::Initial);
    s.frame_at(0.0);
    let id = NodeId(46);
    let spring = SpringPair::new(Spring::new(0.3, 0.0), Spring::new(0.3, 0.0));
    s.insert(id, spec(0.0, 0.0, 100.0, 40.0).transition(Transition::identity().springs(spring)), Mount::Animated);
    s.set_time(1.0 / HZ);
    let out = s.frame();
    assert!(!out.animating, "no presence spring runs for an identity insertion");
    assert_eq!(s.phase(id), Some(Phase::Present));
}

#[test]
fn a_child_mounted_late_in_its_parents_entrance_enters_on_its_own() {
    let mut s = Scene::new();
    let (p, c) = (NodeId(44), NodeId(45));
    s.insert(NodeId(1), spec(0.0, 0.0, 1.0, 1.0), Mount::Initial);
    s.frame_at(0.0);
    s.insert(p, spec(0.0, 0.0, 100.0, 100.0), Mount::Animated);
    let mut t = 0.0;
    loop {
        t += 1.0 / HZ;
        let out = s.frame_at(t);
        if find(out.changed, p).is_some_and(|pp| pp.opacity >= 0.9) { break; }
        assert!(t < 2.0, "the parent arrives");
    }
    s.set_time(t + 1.0 / HZ);
    s.insert(c, spec(10.0, 10.0, 20.0, 20.0).parent(p), Mount::Animated);
    let cp = find(s.frame().changed, c).unwrap();
    assert_eq!(cp.phase, Phase::Entering, "fades in instead of appearing at the parent's 90%");
    assert!(cp.opacity < 0.5, "{}", cp.opacity);
}

#[test]
fn flags_mark_intent_for_motion_lint() {
    let mut s = Scene::new();
    let id = NodeId(50);
    s.insert(id, spec(0.0, 0.0, 100.0, 40.0), Mount::Initial);
    s.frame_at(0.0);
    s.set_frame(id, Rect::new(0.0, 80.0, 100.0, 40.0), motion::SETTLE);
    let p = find(s.frame_at(0.05).changed, id).unwrap();
    assert!(p.flags & flags::ANIMATED != 0);
    assert!((p.flags >> 8) & rune_motion::Intent::POSITION != 0);
}

#[test]
fn trace_writes_motion_lint_jsonl() {
    let mut s = Scene::new();
    let header = TraceHeader { launch: "test".into(), width: 400.0, height: 300.0, hz: 120.0, scale: 2.0 };
    let (trace, buf) = MotionTrace::in_memory(&header);
    s.set_trace(Some(trace));
    s.insert(NodeId(1), spec(0.0, 0.0, 10.0, 10.0), Mount::Initial);
    s.frame_at(0.0);
    s.insert(NodeId(2), spec(0.0, 20.0, 100.0, 40.0).transition(Transition::blur_replace()), Mount::Animated);
    run(&mut s, 0.0, 0.6, NodeId(2));
    s.remove(NodeId(2));
    run(&mut s, 0.6, 1.4, NodeId(2));
    drop(s.take_trace());
    let text = buf.contents();
    let lines: Vec<_> = text.lines().collect();
    assert!(lines[0].starts_with(r#"{"type":"header","launch":"test""#));
    assert!(lines.iter().any(|l| l.contains(r#""e":[[2,"#)), "elements");
    assert!(lines.iter().any(|l| l.contains(r#""ai":[[2,"#)), "intents");
    assert!(lines.iter().any(|l| l.contains(r#""g":[2]"#)), "gone after exit");
    assert!(lines.iter().all(|l| l.starts_with('{') && l.ends_with('}')));
}

#[test]
fn scale_stays_above_point_nine_unless_a_small_glyph_opts_in() {
    let start = |t: Transition| {
        let mut s = Scene::new();
        s.insert(NodeId(9), spec(0.0, 0.0, 16.0, 16.0).transition(t), Mount::Animated);
        find(s.frame_at(0.0).changed, NodeId(9)).unwrap().transform.scale.x
    };
    assert!((start(Transition::scale(0.6)) - 0.9).abs() < 1e-4, "the default rule");
    assert!((start(Transition::scale(0.6).allow_small_scale()) - 0.6).abs() < 1e-4);
    assert!((start(Transition::scale(0.0).allow_small_scale()) - SMALL_SCALE_FLOOR).abs() < 1e-4, "never from 0");
    assert!((start(Transition::scale(0.6).allow_small_scale().combined(Transition::opacity())) - 0.6).abs() < 1e-4);

    // The removal leaves at the small scale too, still fading out.
    let mut s = Scene::new();
    let id = NodeId(10);
    s.insert(id, spec(0.0, 0.0, 16.0, 16.0).transition(Transition::scale(0.6).allow_small_scale()), Mount::Initial);
    s.frame_at(0.0);
    s.remove(id);
    let (exit, exited_at) = run(&mut s, 0.0, 1.0, id);
    let exited_at = exited_at.expect("exit finishes");
    let last = exit.iter().rev().find(|(t, _)| *t <= exited_at).unwrap().1;
    assert!(last.transform.scale.x < 0.65 && last.opacity < 1.0 / 255.0, "{:?}", (last.transform.scale.x, last.opacity));
}

#[test]
fn a_frozen_ancestor_holds_screen_travel_without_frames() {
    let mut s = Scene::new();
    let (p, c) = (NodeId(10), NodeId(11));
    s.insert(p, spec(0.0, 0.0, 400.0, 400.0), Mount::Initial);
    s.insert(c, spec(10.0, 10.0, 50.0, 50.0).parent(p), Mount::Initial);
    s.frame_at(0.0);
    s.set_frame(p, Rect::new(200.0, 100.0, 400.0, 400.0), motion::GLIDE);
    s.set_frame(c, Rect::new(100.0, 200.0, 50.0, 50.0), LayoutAnim::new(motion::DRIFT).screen());
    s.frame_at(0.05);
    s.set_time_scale(p, 0.0);
    let mut t = 0.05;
    while t < 3.0 {
        t += 1.0 / HZ;
        s.frame_at(t);
    }
    assert!(!s.frame_at(t).animating, "the child arrived and its frozen parent asks for no frames");
    s.set_time_scale(p, 1.0);
    let (seen, _) = run(&mut s, t, t + 3.0, c);
    assert!(!s.frame_at(t + 3.1).animating);
    assert_eq!(seen.last().unwrap().1.rect.origin(), Vec2::new(300.0, 300.0));
}

#[test]
fn a_trailing_child_rests_where_its_parent_does() {
    let (p, c) = (NodeId(1), NodeId(2));
    let mut s = Scene::new();
    s.insert(p, spec(0.0, 0.0, 40.0, 20.0), Mount::Initial);
    s.insert(c, spec(30.0, 4.0, 6.0, 6.0).parent(p), Mount::Initial);
    s.set_ride_trailing(c, true);
    s.frame_at(0.0);
    s.set_frame(p, Rect::new(0.0, 0.0, 70.0, 20.0), Spring::new(0.3, 0.0));
    s.frame_at(0.05);
    let (shown, rest) = (s.presented(c).unwrap(), s.rest(c));
    assert!(shown.rect.x > 30.0 && shown.rect.x < 60.0, "{:?}", shown.rect);
    assert_eq!(rest, Some(Rect::new(60.0, 4.0, 6.0, 6.0)), "the child comes to rest against the parent's new edge");
}

#[test]
fn a_static_node_keeps_no_tracks_and_an_unchanged_commit_leaves_it_clean() {
    let id = NodeId(1);
    let frame = Rect::new(0.0, 0.0, 100.0, 40.0);
    let mut s = Scene::new();
    s.insert(id, NodeSpec::new(frame).opacity(0.5).radius(Corners([4.0; 4])), Mount::Initial);
    s.frame_at(0.0);
    assert_eq!(s.store().track_len(), 0, "a node that never moves keeps no tracks, presence included");
    for k in 1..3 {
        s.set_frame(id, frame, Spring::new(0.3, 0.0));
        s.animate(id, prop::OPACITY, 0.5, Spring::new(0.3, 0.0));
        s.set(id, prop::RADIUS, Corners([4.0; 4]));
        assert!(s.frame_at(k as f64 / HZ).changed.is_empty(), "nothing changed, so nothing presents again");
    }
    assert_eq!(s.store().track_len(), 0);
}

#[test]
fn a_settled_track_set_to_its_value_stays_clean() {
    let id = NodeId(1);
    let mut s = Scene::new();
    s.insert(id, spec(0.0, 0.0, 100.0, 40.0), Mount::Initial);
    s.frame_at(0.0);
    s.animate(id, prop::OPACITY, 0.2, Spring::new(0.2, 0.0));
    let half = s.frame_at(0.05);
    let o = find(half.changed, id).unwrap().opacity;
    assert!(o < 1.0 && o > 0.2, "the first animation starts from the base value: {o}");
    let mut t = 0.05;
    while s.frame_at(t).animating { t += 1.0 / HZ; }
    s.set(id, prop::OPACITY, 0.2);
    s.animate(id, prop::OPACITY, 0.2, Spring::new(0.2, 0.0));
    assert!(s.frame_at(t + 1.0 / HZ).changed.is_empty());
}

#[test]
fn a_node_without_a_presence_track_still_fades_out() {
    let id = NodeId(1);
    let mut s = Scene::new();
    s.insert(id, spec(0.0, 0.0, 100.0, 40.0), Mount::Initial);
    s.frame_at(0.0);
    s.remove(id);
    let o = find(s.frame_at(0.05).changed, id).unwrap().opacity;
    assert!(o > 0.0 && o < 1.0, "the exit plays: {o}");
    let mut t = 0.05;
    let mut gone = false;
    while t < 3.0 && !gone {
        t += 1.0 / HZ;
        gone = s.frame_at(t).exited.contains(&id);
    }
    assert!(gone);
}

#[test]
fn exiting_tracks_nodes_leaving_across_exit_revive_unmount_and_slot_reuse() {
    let mut s = Scene::new();
    let (p, c, other, reused) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    s.insert(p, spec(0.0, 0.0, 100.0, 40.0), Mount::Initial);
    s.insert(c, spec(0.0, 0.0, 40.0, 20.0).parent(p), Mount::Initial);
    s.insert(other, spec(0.0, 50.0, 100.0, 40.0), Mount::Animated);
    s.frame_at(0.0);
    assert!(s.exiting().is_empty(), "present and entering nodes are not exiting");

    // Removing adds the node once; duplicate remove does not duplicate it.
    s.remove(c);
    s.remove(c);
    assert_eq!(s.exiting(), &[c]);

    // Reviving mid-exit removes it immediately.
    s.frame_at(0.05);
    s.insert(c, spec(0.0, 0.0, 40.0, 20.0).parent(p), Mount::Animated);
    assert!(s.exiting().is_empty(), "revived node leaves exiting");

    // Landing an exiting node via Mount::Initial removes it immediately.
    s.remove(c);
    assert_eq!(s.exiting(), &[c]);
    s.insert(c, spec(0.0, 0.0, 40.0, 20.0).parent(p), Mount::Initial);
    assert!(s.exiting().is_empty(), "landed node leaves exiting");

    // Unmounting a parent drops both the parent and any exiting child from exiting.
    s.remove(c);
    s.remove(p);
    let mut both = s.exiting().to_vec();
    both.sort_by_key(|id| id.0);
    assert_eq!(both, vec![p, c]);
    s.unmount(p);
    assert!(s.exiting().is_empty(), "unmount clears parent and subtree from exiting");

    // Reusing a freed slot does not resurrect stale exiting state.
    s.insert(reused, spec(0.0, 0.0, 60.0, 20.0), Mount::Initial);
    s.frame_at(0.1);
    assert!(s.exiting().is_empty(), "node mounted into freed slot is not exiting");

    // An exit that plays out to completion clears itself when retired.
    s.remove(reused);
    assert_eq!(s.exiting(), &[reused]);
    let mut t = 0.1;
    while t < 3.0 && s.contains(reused) {
        t += 1.0 / HZ;
        s.frame_at(t);
    }
    assert!(!s.contains(reused));
    assert!(s.exiting().is_empty(), "retired exit leaves exiting");
}

#[test]
fn transition_motion_drives_enter_exit_revive_and_respects_reduced_motion() {
    let mut s = Scene::new();
    let id = NodeId(101);
    let tr = Transition::opacity();

    // Timed linear enter (0.4 s) and exit (0.2 s).
    s.set_transition_motion(id, Curve::linear(0.4), Curve::linear(0.2));
    s.insert(id, spec(0.0, 0.0, 80.0, 40.0).transition(tr), Mount::Animated);
    s.frame_at(0.0);
    assert!(s.presented(id).unwrap().opacity < 0.01);

    s.frame_at(0.2);
    let mid_enter = s.presented(id).unwrap().opacity;
    assert!((mid_enter - 0.5).abs() < 0.05, "expected ~0.5 at 0.2s of 0.4s linear enter, got {mid_enter}");

    s.frame_at(0.41);
    assert!((s.presented(id).unwrap().opacity - 1.0).abs() < 1e-4, "settled at 1.0");
    assert_eq!(s.phase(id), Some(Phase::Present));

    // Exit runs on the 0.2 s linear curve.
    s.remove(id);
    s.frame_at(0.51);
    let mid_exit = s.presented(id).unwrap().opacity;
    assert!((mid_exit - 0.5).abs() < 0.08, "expected ~0.5 at 0.1s of 0.2s linear exit, got {mid_exit}");

    // Reviving mid-exit uses the custom enter curve (0.4 s linear; settles at 0.51 + 0.4 = 0.91).
    s.insert(id, spec(0.0, 0.0, 80.0, 40.0).transition(tr), Mount::Animated);
    s.frame_at(0.81);
    let mid_revive = s.presented(id).unwrap().opacity;
    assert!((mid_revive - 0.75).abs() < 0.08, "revive interpolates on custom 0.4s enter curve (~0.75 at +0.3s), got {mid_revive}");
    s.frame_at(0.95);
    assert_eq!(s.phase(id), Some(Phase::Present));

    // Clearing reverts to Transition::springs on the next exit, and removing cleans up.
    s.clear_transition_motion(id);
    s.set_transition(id, Transition::opacity().springs(SpringPair::new(Spring::new(0.3, 0.0), Spring::NONE)));
    s.remove(id);
    s.frame_at(1.01);
    assert!(!s.contains(id), "cleared override falls back to Transition::springs (Spring::NONE exits at once)");

    // Anim::INSTANT stays instant even under Reduce Motion (not stretched into a fade).
    let instant_id = NodeId(102);
    s.set_reduced_motion(true);
    s.set_transition_motion(instant_id, Anim::INSTANT, Anim::INSTANT);
    s.insert(instant_id, spec(0.0, 0.0, 80.0, 40.0).transition(tr), Mount::Animated);
    s.frame_at(1.05);
    assert!((s.presented(instant_id).unwrap().opacity - 1.0).abs() < 1e-4);
    assert_eq!(s.phase(instant_id), Some(Phase::Present));
    s.remove(instant_id);
    s.frame_at(1.06);
    assert!(!s.contains(instant_id), "Anim::INSTANT exit unmounts at once under Reduce Motion");

    // A long timed curve reduces to REDUCED_FADE (~0.18 s) under Reduce Motion.
    let reduced_id = NodeId(103);
    s.set_transition_motion(reduced_id, Curve::linear(2.0), Curve::linear(2.0));
    s.insert(reduced_id, spec(0.0, 0.0, 80.0, 40.0).transition(tr), Mount::Animated);
    s.frame_at(1.06);
    s.frame_at(1.30);
    assert_eq!(s.phase(reduced_id), Some(Phase::Present), "2.0s timed enter reduced to ~0.18s fade settles by 0.24s");
}

#[test]
fn reduced_motion_instant_match_geometry_and_zero_tween_transition_add_no_fade() {
    let mut s = Scene::new();
    s.set_reduced_motion(true);
    s.insert(NodeId(1), spec(0.0, 0.0, 10.0, 10.0), Mount::Initial);
    s.frame_at(0.0);

    // Zero-duration timed transition must stay instant under Reduce Motion.
    let id = NodeId(200);
    s.set_transition_motion(id, Curve::linear(0.0), Curve::linear(0.0));
    s.insert(id, spec(0.0, 0.0, 80.0, 40.0).transition(Transition::opacity()), Mount::Animated);
    let out = s.frame_at(0.1);
    assert!(!out.animating, "0 s enter tween must not animate under reduced motion");
    assert_eq!(s.phase(id), Some(Phase::Present));
    assert_eq!(s.presented(id).unwrap().opacity, 1.0);
    s.remove(id);
    let out = s.frame_at(0.2);
    assert!(!out.animating, "0 s exit tween must not animate under reduced motion");
    assert!(!s.contains(id));

    // match_geometry with an instant LayoutAnim must not gain a crossfade under Reduce Motion.
    let (from, to) = (NodeId(201), NodeId(202));
    s.insert(from, spec(0.0, 0.0, 60.0, 30.0), Mount::Initial);
    s.frame_at(0.3);
    s.insert(to, spec(120.0, 40.0, 100.0, 50.0), Mount::Initial);
    s.match_geometry(from, to, LayoutAnim::INSTANT);
    let out = s.frame_at(0.3);
    assert!(!out.animating, "instant match_geometry must not add a crossfade under reduced motion");
    assert!(!s.contains(from), "source must unmount at once");
    assert_eq!(s.phase(to), Some(Phase::Present));
    assert_eq!(s.presented(to).unwrap().opacity, 1.0);
    assert_eq!(s.presented(to).unwrap().rect, Rect::new(120.0, 40.0, 100.0, 50.0));
}
