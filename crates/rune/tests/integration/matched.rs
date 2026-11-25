use rune::{Color, Cx, El, Env, Live, Runtime, ScrollSpec, Spring, Transition, col, el, row};

/// A tile that lives in the left box (40 pt) or, zoomed, in the right box (160 pt).
fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
    let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE).radius(4.0);
    let mut left = el().key("left").size(100.0, 200.0);
    let mut right = el().key("right").size(200.0, 200.0);
    if *zoomed { right = right.child(tile(160.0)) } else { left = left.child(tile(40.0)) }
    row().size(300.0, 200.0).child(left).child(right)
}

const GRID: [f32; 4] = [0.0, 0.0, 40.0, 40.0];
const ONE: [f32; 4] = [100.0, 0.0, 160.0, 160.0];

fn rt() -> Runtime<bool> {
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn tile(rt: &Runtime<bool>) -> [f32; 4] {
    rt.presented("tile").expect("tile").0
}

fn moving(rt: &Runtime<bool>) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some("tile") && e.moving)
}

fn jump(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

#[test]
fn a_matched_element_moving_between_parents_flies_from_where_it_was() {
    let mut rt = rt();
    assert_eq!(tile(&rt), GRID);
    rt.update(|zoomed, _| *zoomed = true);
    rt.frame(1.0 / 120.0);
    let first = tile(&rt);
    assert!(jump(first, GRID) < 8.0, "starts at the old rect: {first:?}");
    assert!(moving(&rt), "a flight reports `moving`");
    let mut last = first;
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
        let r = tile(&rt);
        assert!(jump(r, last) < 12.0, "continuous: {last:?} -> {r:?}");
        last = r;
    }
    assert_eq!(tile(&rt), ONE);
    assert!(!moving(&rt));
}

#[test]
fn toggling_mid_flight_turns_around_without_a_jump() {
    let mut rt = rt();
    let mut last = tile(&rt);
    let mut worst = 0.0f32;
    for round in 0..4 {
        rt.update(|zoomed, _| *zoomed = !*zoomed);
        for _ in 0..12 + round * 5 {
            rt.frame(1.0 / 120.0);
            let r = tile(&rt);
            worst = worst.max(jump(r, last));
            last = r;
        }
        assert!(moving(&rt), "still in flight when toggled again");
    }
    assert!(worst < 12.0, "no rect jumps between frames: {worst}");
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(tile(&rt), GRID, "an even number of toggles lands back in the grid");
}

#[test]
fn reduced_motion_swaps_in_place() {
    let mut rt = rt();
    rt.set_system_reduced_motion(true);
    rt.update(|zoomed, _| *zoomed = true);
    rt.frame(1.0 / 120.0);
    assert_eq!(tile(&rt), ONE);
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        assert_eq!(tile(&rt), ONE);
    }
}

#[test]
fn children_scale_with_the_flight() {
    fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE)
            .child(el().key("dot").size(size / 4.0, size / 4.0).fill(Color::BLACK));
        let mut left = el().key("left").size(100.0, 200.0);
        let mut right = el().key("right").size(200.0, 200.0);
        if *zoomed { right = right.child(tile(160.0)) } else { left = left.child(tile(40.0)) }
        row().size(300.0, 200.0).child(left).child(right)
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    for _ in 0..10 {
        rt.frame(1.0 / 120.0);
    }
    let (t, _) = rt.presented("tile").unwrap();
    let (d, _) = rt.presented("dot").unwrap();
    assert!(t[2] > 50.0 && t[2] < 150.0, "mid-flight: {t:?}");
    assert!((d[0] - t[0]).abs() < 1.0 && (d[1] - t[1]).abs() < 1.0, "rides the tile's origin: {t:?} {d:?}");
    assert!((d[2] - t[2] / 4.0).abs() < 1.0 && (d[3] - t[3] / 4.0).abs() < 1.0, "a quarter of the tile: {t:?} {d:?}");
}

#[test]
fn unmatched_elements_are_untouched() {
    fn plain(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let tile = |size: f32| el().key("tile").size(size, size).fill(Color::WHITE);
        let mut left = el().key("left").size(100.0, 200.0);
        let mut right = el().key("right").size(200.0, 200.0);
        if *zoomed { right = right.child(tile(160.0)) } else { left = left.child(tile(40.0)) }
        row().size(300.0, 200.0).child(left).child(right)
    }
    let mut rt = Runtime::new(false, plain, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    rt.frame(1.0 / 120.0);
    let (r, _) = rt.presented("tile").unwrap();
    assert_eq!([r[0], r[1]], [ONE[0], ONE[1]], "a plain remount enters at its own place");
}

fn moving_key(rt: &Runtime<bool>, key: &str) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some(key) && e.moving)
}

#[test]
fn a_matched_element_inside_a_matched_one_flies_its_own_straight_path() {
    // The badge sits at the card's top-left in the grid and below a gap when zoomed, in a card
    // whose aspect changes, so riding the card's cover would bend or offset its path.
    fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let badge = |size: f32| el().key("badge").matched("badge").size(size, size).fill(Color::BLACK);
        let mut left = el().key("left").size(100.0, 200.0);
        let mut right = el().key("right").size(200.0, 200.0);
        if *zoomed {
            right = right.child(col().key("card").matched("card").size(160.0, 80.0).fill(Color::WHITE)
                .child(el().key("gap").size(10.0, 40.0)).child(badge(40.0)));
        } else {
            left = left.child(col().key("card").matched("card").size(40.0, 40.0).fill(Color::WHITE).child(badge(10.0)));
        }
        row().size(300.0, 200.0).child(left).child(right)
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let start = rt.presented("badge").unwrap().0;
    assert_eq!(start, [0.0, 0.0, 10.0, 10.0]);
    rt.update(|zoomed, _| *zoomed = true);
    rt.frame(1.0 / 120.0);
    let first = rt.presented("badge").unwrap().0;
    assert!(jump(first, start) < 3.0, "starts where the old badge was: {first:?}");
    assert!(moving_key(&rt, "badge") && moving_key(&rt, "card"), "both fly");
    let mut mid = false;
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
        let b = rt.presented("badge").unwrap().0;
        let t = b[0] / 100.0;
        mid |= t > 0.3 && t < 0.7;
        assert!((b[1] - 40.0 * t).abs() < 1.0, "on the straight path from its start to its end: {b:?}");
        assert!((b[2] - (10.0 + 30.0 * t)).abs() < 1.5, "grows with its own progress: {b:?}");
    }
    assert!(mid, "sampled mid-flight");
    assert_eq!(rt.presented("badge").unwrap().0, [100.0, 40.0, 40.0, 40.0]);
    assert_eq!(rt.presented("card").unwrap().0, [100.0, 0.0, 160.0, 80.0]);
}

/// 0: the tile in the left box, 1: in the right box, 2: gone, 3: the right box gone with it.
fn removable(state: &u8, _: &mut Cx<u8>) -> El {
    let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE)
        .child(el().key("dot").size(size / 4.0, size / 4.0).fill(Color::BLACK));
    let mut left = el().key("left").size(100.0, 200.0);
    let mut right = el().key("right").size(200.0, 200.0);
    match state {
        0 => left = left.child(tile(40.0)),
        1 => right = right.child(tile(160.0)),
        _ => {}
    }
    let r = row().size(300.0, 200.0).child(left);
    if *state == 3 { r } else { r.child(right) }
}

#[test]
fn a_flight_removed_mid_way_exits_without_a_pop() {
    for gone in [2u8, 3] {
        let mut rt = Runtime::new(0u8, removable, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, _| *s = 1);
        for _ in 0..10 {
            rt.frame(1.0 / 120.0);
        }
        let mut last = rt.presented("tile").unwrap();
        assert!(last.0[2] > 50.0 && last.0[2] < 150.0, "mid-flight: {last:?}");
        rt.update(move |s, _| *s = gone);
        let mut faded = false;
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
            let Some(now) = rt.presented("tile") else { break };
            let dot = rt.presented("dot").unwrap().0;
            assert!((dot[2] - now.0[2] / 4.0).abs() < 1.5, "children stay scaled with the flight ({gone}): {now:?} {dot:?}");
            assert!(jump(now.0, last.0) < 12.0, "continuous ({gone}): {last:?} -> {now:?}");
            assert!(now.1 <= last.1 + 1e-3, "fades out ({gone}): {} -> {}", last.1, now.1);
            faded |= now.1 > 0.2 && now.1 < 0.8;
            last = now;
        }
        assert!(faded, "an exit fade was sampled ({gone})");
        assert!(rt.presented("tile").is_none(), "gone once faded ({gone})");
    }
}

#[test]
fn a_fast_flight_waits_for_its_entering_parent() {
    // The flight lands well before the right box finishes entering with a scale and fade.
    fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE)
            .motion(Spring::new(0.08, 0.0));
        let r = row().size(300.0, 200.0);
        if *zoomed {
            r.child(el().key("left").size(100.0, 200.0))
                .child(el().key("right").size(200.0, 200.0).transition(Transition::scale(0.9)).child(tile(160.0)))
        } else {
            r.child(el().key("left").size(100.0, 200.0).child(tile(40.0)))
        }
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    let mut landed = None;
    for frame in 0..120 {
        rt.frame(1.0 / 120.0);
        let (r, opacity) = rt.presented("tile").unwrap();
        assert!(opacity > 0.99, "keeps its own opacity at frame {frame}: {opacity}");
        if landed.is_some() {
            assert!(jump(r, ONE) < 0.5, "stays where it landed at frame {frame}: {r:?}");
        } else if jump(r, ONE) < 0.5 {
            landed = Some(rt.presented("right").unwrap().1);
        }
    }
    let parent = landed.expect("landed");
    assert!(parent < 0.9, "landed while the parent was still entering: its opacity {parent}");
}

#[test]
fn reduced_motion_fades_the_old_one_with_the_parent_it_leaves_with() {
    fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let r = row().size(300.0, 200.0);
        if *zoomed {
            r.child(el().key("spacer").size(100.0, 200.0)).child(el().key("right").size(200.0, 200.0)
                .child(el().key("new").matched("tile").size(160.0, 160.0).fill(Color::WHITE)))
        } else {
            r.child(col().key("left").size(100.0, 200.0)
                .child(el().key("old").matched("tile").size(40.0, 40.0).fill(Color::WHITE))
                .child(el().key("peer").size(40.0, 40.0).fill(Color::WHITE)))
        }
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    let mut sampled = false;
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        let (Some(old), Some(peer)) = (rt.presented("old"), rt.presented("peer")) else { break };
        assert!((old.1 - peer.1).abs() < 0.02, "fades with its sibling: {} vs {}", old.1, peer.1);
        sampled |= peer.1 > 0.2 && peer.1 < 0.8;
        assert_eq!(rt.presented("new").unwrap().0, ONE, "the new one is in place");
    }
    assert!(sampled, "the fade was sampled");
}

/// A tile two parents deep in content, in the left box or, zoomed, in the right box.
fn nested(zoomed: &bool, _: &mut Cx<bool>) -> El {
    let tile = |size: f32| {
        el().key("tile").matched("tile").size(size, size).fill(Color::WHITE)
            .child(row().key("pill").w_full().h(20.0).child(el().key("mark").size(8.0, 8.0).child(el().key("dot").size(4.0, 4.0).fill(Color::BLACK))))
    };
    let mut left = el().key("left").size(100.0, 200.0);
    let mut right = el().key("right").size(200.0, 200.0);
    if *zoomed { right = right.child(tile(160.0)) } else { left = left.child(tile(40.0)) }
    row().size(300.0, 200.0).child(left).child(right)
}

#[test]
fn a_flights_content_flies_with_it_without_fading_in() {
    let mut rt = Runtime::new(false, nested, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    for frame in 0..12 {
        rt.frame(1.0 / 120.0);
        assert!(moving(&rt), "still in flight");
        for key in ["tile", "pill", "mark", "dot"] {
            let o = rt.elements().iter().find(|e| e.key.as_deref() == Some(key)).map_or(0.0, |e| e.opacity);
            assert!(o > 0.999, "frame {frame}: {key} drawn at {o:.3}");
        }
    }
}

#[derive(Default)]
struct OffsetState {
    step: u8,
    src_off: Option<[f32; 2]>,
    dst_off: Option<[f32; 2]>,
    src_live: Option<Live<[f32; 2]>>,
    dst_live: Option<Live<[f32; 2]>>,
    src_scale: Option<f32>,
    dst_scale: Option<f32>,
    src_live_scale: Option<Live<f32>>,
    dst_live_scale: Option<Live<f32>>,
    src_rot: Option<f32>,
    dst_rot: Option<f32>,
    anc_src_off: Option<[f32; 2]>,
    anc_dst_off: Option<[f32; 2]>,
}

fn offset_view(s: &OffsetState, _: &mut Cx<OffsetState>) -> El {
    let mut root = el().key("root").size(400.0, 300.0);
    if s.step == 0 {
        let mut a = el().key("a").matched("t").size(40.0, 40.0).fill(Color::WHITE).radius(4.0);
        if let Some([x, y]) = s.src_off { a = a.offset(x, y); }
        if let Some(l) = &s.src_live { a = a.live_offset(l); }
        if let Some(k) = s.src_scale { a = a.scale(k); }
        if let Some(l) = &s.src_live_scale { a = a.live_scale(l); }
        if let Some(r) = s.src_rot { a = a.rotate(r); }
        if let Some([ax, ay]) = s.anc_src_off {
            root = root.child(el().key("anc-a").size(400.0, 300.0).offset(ax, ay).child(a));
        } else {
            root = root.child(a);
        }
    } else if s.step == 1 {
        let mut b = el().key("b").matched("t").size(40.0, 40.0).absolute(200.0, 100.0).fill(Color::WHITE).radius(4.0);
        if let Some([x, y]) = s.dst_off { b = b.offset(x, y); }
        if let Some(l) = &s.dst_live { b = b.live_offset(l); }
        if let Some(k) = s.dst_scale { b = b.scale(k); }
        if let Some(l) = &s.dst_live_scale { b = b.live_scale(l); }
        if let Some(r) = s.dst_rot { b = b.rotate(r); }
        if let Some([ax, ay]) = s.anc_dst_off {
            root = root.child(el().key("anc-b").size(400.0, 300.0).offset(ax, ay).child(b));
        } else {
            root = root.child(b);
        }
    } else {
        let c = el().key("c").matched("t").size(40.0, 40.0).absolute(100.0, 200.0).offset(15.0, -25.0).fill(Color::WHITE).radius(4.0);
        root = root.child(c);
    }
    root
}

fn settle_offset(rt: &mut Runtime<OffsetState>) {
    rt.frame(0.0);
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
}

fn fly_offset(rt: &mut Runtime<OffsetState>, key: &str) -> ([f32; 4], [f32; 4], f32) {
    rt.frame(0.001);
    let first = rt.presented(key).expect(key).0;
    let mut prev = first;
    let mut worst_step = 0.0f32;
    for _ in 0..360 {
        rt.frame(1.0 / 120.0);
        let cur = rt.presented(key).expect(key).0;
        worst_step = worst_step.max(jump(cur, prev));
        prev = cur;
    }
    (first, prev, worst_step)
}

#[test]
fn matched_own_offset_source_starts_at_source_presented_rect() {
    let mut rt = Runtime::new(
        OffsetState { src_off: Some([30.0, 20.0]), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [30.0, 20.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, worst_step) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.0, "first flight frame dropped source offset: src={src:?}, first={first:?}");
    assert!(worst_step < 12.0, "continuous flight without jumps: worst_step={worst_step}");
    assert_eq!(rest, [200.0, 100.0, 40.0, 40.0]);
}

#[test]
fn matched_own_offset_target_starts_at_source_and_lands_with_target_offset() {
    let mut rt = Runtime::new(
        OffsetState { dst_off: Some([-50.0, 40.0]), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [0.0, 0.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, worst_step) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.0, "first flight frame applied target offset early: src={src:?}, first={first:?}");
    assert!(worst_step < 12.0, "continuous flight without jumps: worst_step={worst_step}");
    assert_eq!(rest, [150.0, 140.0, 40.0, 40.0]);
}

#[test]
fn matched_own_offset_both_flies_between_presented_rects() {
    let mut rt = Runtime::new(
        OffsetState { src_off: Some([30.0, 20.0]), dst_off: Some([-50.0, 40.0]), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [30.0, 20.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, worst_step) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.0, "both offsets: src={src:?}, first={first:?}");
    assert!(worst_step < 12.0, "continuous flight without jumps: worst_step={worst_step}");
    assert_eq!(rest, [150.0, 140.0, 40.0, 40.0]);
}

#[test]
fn matched_own_live_offset_source_target_and_moving() {
    // 1. Source live_offset (settled)
    let src_live = Live::new([30.0, 20.0]);
    let mut rt = Runtime::new(
        OffsetState { src_live: Some(src_live.clone()), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [30.0, 20.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, worst_step) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.0, "source live_offset: src={src:?}, first={first:?}");
    assert!(worst_step < 12.0, "source live_offset continuous: {worst_step}");
    assert_eq!(rest, [200.0, 100.0, 40.0, 40.0]);

    // 2. Source live_offset moving without commits: uses the last drawn frame's offset.
    let src_live = Live::new([10.0, 5.0]);
    let mut rt = Runtime::new(
        OffsetState { src_live: Some(src_live.clone()), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    src_live.set([45.0, 35.0]);
    rt.frame(1.0 / 120.0);
    let drawn = rt.presented("a").unwrap().0;
    assert_eq!(drawn, [45.0, 35.0, 40.0, 40.0]);
    // Mutate again without drawing before the switch: the last drawn value ([45, 35]) counts.
    src_live.set([90.0, 80.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, _) = fly_offset(&mut rt, "b");
    assert!(jump(first, drawn) < 1.0, "moving live_offset must start from last drawn rect: drawn={drawn:?}, first={first:?}");
    assert_eq!(rest, [200.0, 100.0, 40.0, 40.0]);

    // 3. Target live_offset (settled and mutated mid-flight)
    let dst_live = Live::new([-50.0, 40.0]);
    let mut rt = Runtime::new(
        OffsetState { dst_live: Some(dst_live.clone()), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [0.0, 0.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    rt.frame(0.001);
    let first = rt.presented("b").unwrap().0;
    assert!(jump(first, src) < 1.0, "target live_offset applied early: src={src:?}, first={first:?}");
    // Step part way, then shift target live_offset mid-flight.
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    dst_live.set([-20.0, 60.0]);
    for _ in 0..360 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.presented("b").unwrap().0, [180.0, 160.0, 40.0, 40.0]);

    // 4. Both live_offset
    let src_live = Live::new([30.0, 20.0]);
    let dst_live = Live::new([-50.0, 40.0]);
    let mut rt = Runtime::new(
        OffsetState { src_live: Some(src_live), dst_live: Some(dst_live), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [30.0, 20.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, worst_step) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.0, "both live_offset: src={src:?}, first={first:?}");
    assert!(worst_step < 12.0, "both live_offset continuous: {worst_step}");
    assert_eq!(rest, [150.0, 140.0, 40.0, 40.0]);
}

#[test]
fn matched_ancestor_offset_guard() {
    let mut rt = Runtime::new(
        OffsetState { anc_src_off: Some([30.0, 20.0]), anc_dst_off: Some([-50.0, 40.0]), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [30.0, 20.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, worst_step) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.0, "ancestor offsets: src={src:?}, first={first:?}");
    assert!(worst_step < 12.0, "ancestor offsets continuous: {worst_step}");
    assert_eq!(rest, [150.0, 140.0, 40.0, 40.0]);
}

#[test]
fn matched_own_offset_mid_flight_rematch_continues_from_presented_rect() {
    let mut rt = Runtime::new(
        OffsetState { src_off: Some([30.0, 20.0]), dst_off: Some([-50.0, 40.0]), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    rt.update(|s, _| s.step = 1);
    for _ in 0..15 { rt.frame(1.0 / 120.0); }
    let mid = rt.presented("b").unwrap().0;
    assert!(mid[0] > 45.0 && mid[0] < 135.0, "mid-flight before rematch: {mid:?}");
    rt.update(|s, _| s.step = 2);
    rt.frame(0.001);
    let after = rt.presented("c").unwrap().0;
    assert!(jump(after, mid) < 2.0, "mid-flight rematch jumped: mid={mid:?}, after={after:?}");
    let mut prev = after;
    let mut worst = 0.0f32;
    for _ in 0..360 {
        rt.frame(1.0 / 120.0);
        let cur = rt.presented("c").unwrap().0;
        worst = worst.max(jump(cur, prev));
        prev = cur;
    }
    assert!(worst < 12.0, "continuous after rematch: {worst}");
    assert_eq!(prev, [115.0, 175.0, 40.0, 40.0]);
}

#[test]
fn matched_own_offset_reduced_motion_swaps_at_target_presented_rect() {
    let mut rt = Runtime::new(
        OffsetState { src_off: Some([30.0, 20.0]), dst_off: Some([-50.0, 40.0]), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    rt.set_system_reduced_motion(true);
    settle_offset(&mut rt);
    assert_eq!(rt.presented("a").unwrap().0, [30.0, 20.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.presented("b").unwrap().0, [150.0, 140.0, 40.0, 40.0]);
}

#[test]
fn matched_own_offset_inside_scroll_view() {
    fn view(switched: &bool, _: &mut Cx<bool>) -> El {
        let mut inner = col().key("inner").w_full()
            .child(el().w_full().h(200.0));
        if !*switched {
            inner = inner.child(el().key("a").matched("t").size(40.0, 40.0).offset(25.0, 15.0).fill(Color::WHITE));
        } else {
            inner = inner.child(el().size(40.0, 40.0));
        }
        inner = inner.child(el().w_full().h(400.0));
        let scroller = col().key("list").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(inner);
        let mut root = el().key("root").size(400.0, 300.0).child(scroller);
        if *switched {
            root = root.child(el().key("b").matched("t").size(40.0, 40.0).absolute(220.0, 80.0).offset(-30.0, 20.0).fill(Color::WHITE));
        }
        root
    }
    let mut rt = Runtime::new(false, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.wheel(150.0, 100.0, -150.0);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [25.0, 65.0, 40.0, 40.0]);
    rt.update(|s, _| *s = true);
    rt.frame(0.001);
    let first = rt.presented("b").unwrap().0;
    assert!(jump(first, src) < 1.0, "scrolled source own offset: src={src:?}, first={first:?}");
    for _ in 0..360 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.presented("b").unwrap().0, [190.0, 100.0, 40.0, 40.0]);
}

#[test]
fn matched_own_scale_with_and_without_own_offset() {
    // Source scale(2.0) + offset(30, 20)
    let mut rt = Runtime::new(
        OffsetState { src_off: Some([30.0, 20.0]), src_scale: Some(2.0), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    // Center of [30, 20, 40, 40] is (50, 40); scaled 2x -> [10, 0, 80, 80].
    assert_eq!(src, [10.0, 0.0, 80.0, 80.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, worst) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.5, "source scale+offset: src={src:?}, first={first:?}");
    assert!(worst < 15.0, "continuous: {worst}");
    assert_eq!(rest, [200.0, 100.0, 40.0, 40.0]);

    // Target scale(2.0) + offset(-50, 40)
    let mut rt = Runtime::new(
        OffsetState { dst_off: Some([-50.0, 40.0]), dst_scale: Some(2.0), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [0.0, 0.0, 40.0, 40.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, worst) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.5, "target scale+offset: src={src:?}, first={first:?}");
    assert!(worst < 15.0, "continuous: {worst}");
    // Unscaled target is [150, 140, 40, 40], center (170, 160), scaled 2x -> [130, 120, 80, 80].
    assert_eq!(rest, [130.0, 120.0, 80.0, 80.0]);

    // Mid-flight rematch when target has own scale(2.0)
    let mut rt = Runtime::new(
        OffsetState { dst_scale: Some(2.0), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    rt.update(|s, _| s.step = 1);
    for _ in 0..15 { rt.frame(1.0 / 120.0); }
    let mid = rt.presented("b").unwrap().0;
    rt.update(|s, _| s.step = 2);
    rt.frame(0.001);
    let after = rt.presented("c").unwrap().0;
    assert!(jump(after, mid) < 2.0, "mid-flight rematch of scaled target jumped: mid={mid:?}, after={after:?}");
}

#[test]
fn matched_own_live_scale_and_rotated_source_offset() {
    // Source live_scale(2.0) + live_offset([30, 20])
    let mut rt = Runtime::new(
        OffsetState {
            src_live: Some(Live::new([30.0, 20.0])),
            src_live_scale: Some(Live::new(2.0)),
            ..Default::default()
        },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    assert_eq!(src, [10.0, 0.0, 80.0, 80.0]);
    rt.update(|s, _| s.step = 1);
    let (first, rest, _) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.5, "source live_scale+live_offset: src={src:?}, first={first:?}");
    assert_eq!(rest, [200.0, 100.0, 40.0, 40.0]);

    // Target live_scale(2.0) + live_offset([-50, 40])
    let mut rt = Runtime::new(
        OffsetState {
            dst_live: Some(Live::new([-50.0, 40.0])),
            dst_live_scale: Some(Live::new(2.0)),
            ..Default::default()
        },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    rt.update(|s, _| s.step = 1);
    let (first, rest, _) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.5, "target live_scale+live_offset: src={src:?}, first={first:?}");
    assert_eq!(rest, [130.0, 120.0, 80.0, 80.0]);

    // Rotated source with own offset: flight starts at the rotated, offset presented rect.
    let mut rt = Runtime::new(
        OffsetState { src_off: Some([30.0, 20.0]), src_rot: Some(0.5), ..Default::default() },
        offset_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle_offset(&mut rt);
    let src = rt.presented("a").unwrap().0;
    rt.update(|s, _| s.step = 1);
    let (first, rest, _) = fly_offset(&mut rt, "b");
    assert!(jump(first, src) < 1.0, "rotated source keeps own offset and rotation: src={src:?}, first={first:?}");
    assert_eq!(rest, [200.0, 100.0, 40.0, 40.0]);
}
