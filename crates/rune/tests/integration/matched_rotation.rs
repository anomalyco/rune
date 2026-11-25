use rune::{Color, Cx, El, Env, Live, Runtime, el};

#[derive(Default)]
struct RotState {
    step: u8,
    src_rot: Option<f32>,
    dst_rot: Option<f32>,
    c_rot: Option<f32>,
    src_live: Option<Live<f32>>,
    dst_live: Option<Live<f32>>,
    same_place: bool,
    src_off: Option<[f32; 2]>,
    dst_off: Option<[f32; 2]>,
    origin: Option<[f32; 2]>,
    in_place: bool,
}

fn rot_view(s: &RotState, _: &mut Cx<RotState>) -> El {
    let mut root = el().key("root").size(400.0, 300.0);
    let place = |mut e: El, pos: [f32; 2], off: Option<[f32; 2]>, rot: Option<f32>, live: &Option<Live<f32>>| {
        if pos != [0.0, 0.0] {
            e = e.absolute(pos[0], pos[1]);
        }
        if let Some([ox, oy]) = off {
            e = e.offset(ox, oy);
        }
        if let Some([px, py]) = s.origin {
            e = e.origin(px, py);
        }
        if let Some(r) = rot {
            e = e.rotate(r);
        }
        if let Some(l) = live {
            e = e.live_rotation(l);
        }
        if s.in_place {
            e = e.matched_in_place();
        }
        e
    };
    match s.step {
        0 => {
            let a = el().key("a").matched("t").size(40.0, 40.0).fill(Color::WHITE).radius(4.0);
            root = root.child(place(a, [0.0, 0.0], s.src_off, s.src_rot, &s.src_live));
        }
        1 => {
            let pos = if s.same_place { [0.0, 0.0] } else { [200.0, 100.0] };
            let b = el().key("b").matched("t").size(40.0, 40.0).fill(Color::WHITE).radius(4.0);
            root = root.child(place(b, pos, s.dst_off, s.dst_rot, &s.dst_live));
        }
        _ => {
            let c = el().key("c").matched("t").size(40.0, 40.0).fill(Color::WHITE).radius(4.0);
            root = root.child(place(c, [100.0, 200.0], None, s.c_rot, &None));
        }
    }
    root
}

fn jump(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = (b - a).rem_euclid(std::f32::consts::TAU);
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    d.abs()
}

fn settle(rt: &mut Runtime<RotState>) {
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0 / 120.0);
    }
}

#[test]
fn rotated_source_to_upright_target_starts_rotated_and_unwinds_smoothly() {
    let mut rt = Runtime::new(
        RotState { src_rot: Some(0.5), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    let src_rect = rt.presented("a").unwrap().0;
    let src_rot = rt.presented_pose("a").unwrap().rotation();
    assert!((src_rot - 0.5).abs() < 1e-4);

    rt.update(|s, _| s.step = 1);
    rt.frame(0.001);
    let first_rect = rt.presented("b").unwrap().0;
    let first_rot = rt.presented_pose("b").unwrap().rotation();
    assert!(jump(first_rect, src_rect) < 1.0, "first frame snapped rect: src={src_rect:?}, first={first_rect:?}");
    assert!(angle_diff(first_rot, src_rot) < 0.02, "first frame snapped rotation: src={src_rot}, first={first_rot}");

    let mut prev_rect = first_rect;
    let mut prev_rot = first_rot;
    let mut saw_mid = false;
    for frame in 0..360 {
        rt.frame(1.0 / 120.0);
        let cur_rect = rt.presented("b").unwrap().0;
        let cur_rot = rt.presented_pose("b").unwrap().rotation();
        assert!(jump(cur_rect, prev_rect) < 12.0, "frame {frame}: rect jumped {prev_rect:?} -> {cur_rect:?}");
        assert!(cur_rot <= prev_rot + 1e-3 && cur_rot >= -1e-3, "frame {frame}: rotation non-monotonic {prev_rot} -> {cur_rot}");
        if cur_rot > 0.1 && cur_rot < 0.4 {
            saw_mid = true;
            // Rotated 40x40 square is wider than 40 pt while tilted.
            assert!(cur_rect[2] > 42.0, "frame {frame}: painted upright mid-flight at rot={cur_rot}: rect={cur_rect:?}");
        }
        prev_rect = cur_rect;
        prev_rot = cur_rot;
    }
    assert!(saw_mid, "sampled mid-flight rotation");
    assert_eq!(prev_rect, [200.0, 100.0, 40.0, 40.0]);
    assert!(prev_rot.abs() < 1e-4);
}

#[test]
fn upright_source_to_rotated_target_starts_upright_and_turns_smoothly() {
    let mut rt = Runtime::new(
        RotState { dst_rot: Some(0.5), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    let src_rect = rt.presented("a").unwrap().0;
    assert_eq!(src_rect, [0.0, 0.0, 40.0, 40.0]);

    rt.update(|s, _| s.step = 1);
    rt.frame(0.001);
    let first_rect = rt.presented("b").unwrap().0;
    let first_rot = rt.presented_pose("b").unwrap().rotation();
    assert!(jump(first_rect, src_rect) < 1.0, "first frame rect: src={src_rect:?}, first={first_rect:?}");
    assert!(angle_diff(first_rot, 0.0) < 0.02, "first frame applied target rotation early: first_rot={first_rot}");

    let mut prev_rect = first_rect;
    let mut prev_rot = first_rot;
    let mut saw_mid = false;
    for frame in 0..360 {
        rt.frame(1.0 / 120.0);
        let cur_rect = rt.presented("b").unwrap().0;
        let cur_rot = rt.presented_pose("b").unwrap().rotation();
        assert!(jump(cur_rect, prev_rect) < 12.0, "frame {frame}: rect jumped {prev_rect:?} -> {cur_rect:?}");
        assert!(cur_rot >= prev_rot - 1e-3 && cur_rot <= 0.5 + 1e-3, "frame {frame}: rotation non-monotonic {prev_rot} -> {cur_rot}");
        if cur_rot > 0.15 && cur_rot < 0.35 {
            saw_mid = true;
            assert!(cur_rect[2] > 42.0, "frame {frame}: painted upright mid-flight at rot={cur_rot}: rect={cur_rect:?}");
        }
        prev_rect = cur_rect;
        prev_rot = cur_rot;
    }
    assert!(saw_mid, "sampled mid-flight rotation");
    assert!(angle_diff(prev_rot, 0.5) < 1e-4);
    assert!(prev_rect[2] > 53.0, "landed rotated: {prev_rect:?}");
}

#[test]
fn both_rotated_different_angles_never_snap_upright_and_take_shortest_arc() {
    // 1. Same sign: 0.3 -> 0.9 rad (never passes through 0).
    let mut rt = Runtime::new(
        RotState { src_rot: Some(0.3), dst_rot: Some(0.9), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    let src_rect = rt.presented("a").unwrap().0;
    rt.update(|s, _| s.step = 1);
    rt.frame(0.001);
    let first_rect = rt.presented("b").unwrap().0;
    let first_rot = rt.presented_pose("b").unwrap().rotation();
    assert!(jump(first_rect, src_rect) < 1.0, "first={first_rect:?}, src={src_rect:?}");
    assert!(angle_diff(first_rot, 0.3) < 0.02, "first_rot={first_rot}");

    let mut prev_rect = first_rect;
    for frame in 0..360 {
        rt.frame(1.0 / 120.0);
        let cur_rect = rt.presented("b").unwrap().0;
        let cur_rot = rt.presented_pose("b").unwrap().rotation();
        assert!(jump(cur_rect, prev_rect) < 12.0, "frame {frame}: jumped {prev_rect:?} -> {cur_rect:?}");
        assert!(cur_rot > 0.25 && cur_rot < 0.95, "frame {frame}: snapped outside [0.3, 0.9]: {cur_rot}");
        assert!(cur_rect[2] > 48.0, "frame {frame}: painted upright at rot={cur_rot}: {cur_rect:?}");
        prev_rect = cur_rect;
    }
    assert!(angle_diff(rt.presented_pose("b").unwrap().rotation(), 0.9) < 1e-4);

    // 2. Wrap-around shortest arc: 3.0 rad -> -3.0 rad (short ~0.28 rad arc through PI, never near 0).
    let mut rt = Runtime::new(
        RotState { src_rot: Some(3.0), dst_rot: Some(-3.0), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    rt.update(|s, _| s.step = 1);
    for frame in 0..360 {
        rt.frame(1.0 / 120.0);
        let r = rt.presented_pose("b").unwrap().rotation();
        // Shortest arc stays within 0.35 rad of PI, never passing near 0.
        assert!(angle_diff(r, std::f32::consts::PI) < 0.35, "frame {frame}: took long arc through upright: rot={r}");
    }
    assert!(angle_diff(rt.presented_pose("b").unwrap().rotation(), -3.0) < 1e-4);

    // 3. Same angle: 0.5 -> 0.5 stays at 0.5 on every frame.
    let mut rt = Runtime::new(
        RotState { src_rot: Some(0.5), dst_rot: Some(0.5), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    let src_rect = rt.presented("a").unwrap().0;
    rt.update(|s, _| s.step = 1);
    for frame in 0..120 {
        rt.frame(1.0 / 120.0);
        let cur_rect = rt.presented("b").unwrap().0;
        let cur_rot = rt.presented_pose("b").unwrap().rotation();
        assert!(angle_diff(cur_rot, 0.5) < 1e-3, "frame {frame}: drifted from 0.5: {cur_rot}");
        assert!((cur_rect[2] - src_rect[2]).abs() < 0.5, "frame {frame}: upright snap: {cur_rect:?} vs {src_rect:?}");
    }
}

#[test]
fn live_rotation_on_source_target_and_while_moving() {
    // 1. Source live_rotation moving without commits: uses the last drawn frame's angle.
    let src_live = Live::new(0.2);
    let mut rt = Runtime::new(
        RotState { src_live: Some(src_live.clone()), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    src_live.set(0.7);
    rt.frame(1.0 / 120.0);
    let drawn_rect = rt.presented("a").unwrap().0;
    let drawn_rot = rt.presented_pose("a").unwrap().rotation();
    assert!((drawn_rot - 0.7).abs() < 1e-4);
    // Mutate without drawing before the switch: the last drawn angle (0.7) counts.
    src_live.set(1.4);
    rt.update(|s, _| s.step = 1);
    rt.frame(0.001);
    let first_rect = rt.presented("b").unwrap().0;
    let first_rot = rt.presented_pose("b").unwrap().rotation();
    assert!(jump(first_rect, drawn_rect) < 1.0, "moving live_rotation rect: drawn={drawn_rect:?}, first={first_rect:?}");
    assert!(angle_diff(first_rot, 0.7) < 0.02, "moving live_rotation angle: first={first_rot}");
    for _ in 0..360 { rt.frame(1.0 / 120.0); }
    assert!(rt.presented_pose("b").unwrap().rotation().abs() < 1e-4);

    // 2. Target live_rotation (settled and updated mid-flight).
    let dst_live = Live::new(0.6);
    let mut rt = Runtime::new(
        RotState { dst_live: Some(dst_live.clone()), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    let src_rect = rt.presented("a").unwrap().0;
    rt.update(|s, _| s.step = 1);
    rt.frame(0.001);
    let first_rect = rt.presented("b").unwrap().0;
    let first_rot = rt.presented_pose("b").unwrap().rotation();
    assert!(jump(first_rect, src_rect) < 1.0, "target live_rotation rect jumped: src={src_rect:?}, first={first_rect:?}");
    assert!(angle_diff(first_rot, 0.0) < 0.02, "target live_rotation applied early: first_rot={first_rot}");
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    dst_live.set(1.0);
    for _ in 0..360 { rt.frame(1.0 / 120.0); }
    assert!(angle_diff(rt.presented_pose("b").unwrap().rotation(), 1.0) < 1e-4);
}

#[test]
fn mid_flight_rematch_preserves_current_rotation_without_upright_snap() {
    let mut rt = Runtime::new(
        RotState { src_rot: Some(0.3), dst_rot: Some(0.9), c_rot: Some(1.2), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    rt.update(|s, _| s.step = 1);
    for _ in 0..15 {
        rt.frame(1.0 / 120.0);
    }
    let mid_rect = rt.presented("b").unwrap().0;
    let mid_rot = rt.presented_pose("b").unwrap().rotation();
    assert!(mid_rot > 0.4 && mid_rot < 0.85, "mid-flight rotation: {mid_rot}");

    rt.update(|s, _| s.step = 2);
    rt.frame(0.001);
    let after_rect = rt.presented("c").unwrap().0;
    let after_rot = rt.presented_pose("c").unwrap().rotation();
    assert!(jump(after_rect, mid_rect) < 2.0, "mid-flight rematch rect jumped: mid={mid_rect:?}, after={after_rect:?}");
    assert!(angle_diff(after_rot, mid_rot) < 0.03, "mid-flight rematch rot jumped: mid={mid_rot}, after={after_rot}");

    let mut prev_rect = after_rect;
    for frame in 0..360 {
        rt.frame(1.0 / 120.0);
        let cur_rect = rt.presented("c").unwrap().0;
        let cur_rot = rt.presented_pose("c").unwrap().rotation();
        assert!(jump(cur_rect, prev_rect) < 12.0, "frame {frame}: {prev_rect:?} -> {cur_rect:?}");
        assert!(cur_rot > 0.35 && cur_rot < 1.25, "frame {frame}: snapped upright after rematch: {cur_rot}");
        prev_rect = cur_rect;
    }
    assert!(angle_diff(rt.presented_pose("c").unwrap().rotation(), 1.2) < 1e-4);
}

#[test]
fn rotation_only_match_at_same_rect_animates_without_landing_early() {
    let mut rt = Runtime::new(
        RotState { src_rot: Some(0.0), dst_rot: Some(0.8), same_place: true, ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    rt.update(|s, _| s.step = 1);
    rt.frame(0.001);
    let first_rot = rt.presented_pose("b").unwrap().rotation();
    assert!(first_rot < 0.05, "must not snap on frame 1 when only rotation changes: {first_rot}");
    let mut saw_mid = false;
    for _ in 0..360 {
        rt.frame(1.0 / 120.0);
        let r = rt.presented_pose("b").unwrap().rotation();
        if r > 0.2 && r < 0.6 {
            saw_mid = true;
        }
    }
    assert!(saw_mid, "rotation-only match must animate smoothly rather than landing on frame 1");
    assert!(angle_diff(rt.presented_pose("b").unwrap().rotation(), 0.8) < 1e-4);
}

#[test]
fn rotated_matched_reduced_motion_and_custom_origin_and_in_place() {
    // 1. Reduced motion: swaps straight to target presented rect and rotation.
    let mut rt = Runtime::new(
        RotState { src_rot: Some(0.3), dst_rot: Some(0.8), ..Default::default() },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    rt.set_system_reduced_motion(true);
    settle(&mut rt);
    rt.update(|s, _| s.step = 1);
    rt.frame(1.0 / 120.0);
    assert!(angle_diff(rt.presented_pose("b").unwrap().rotation(), 0.8) < 1e-4);
    assert!(rt.presented("b").unwrap().0[2] > 50.0);

    // 2. Custom origin + own offset + matched_in_place: continuous from take-off to landing.
    let mut rt = Runtime::new(
        RotState {
            src_rot: Some(0.4),
            dst_rot: Some(0.9),
            src_off: Some([30.0, 20.0]),
            dst_off: Some([-40.0, 30.0]),
            origin: Some([0.0, 0.0]),
            in_place: true,
            ..Default::default()
        },
        rot_view, (400.0, 300.0), Env::new(true, 1.0),
    );
    settle(&mut rt);
    let src_rect = rt.presented("a").unwrap().0;
    rt.update(|s, _| s.step = 1);
    rt.frame(0.001);
    let first_rect = rt.presented("b").unwrap().0;
    assert!(jump(first_rect, src_rect) < 1.5, "custom origin + offset: src={src_rect:?}, first={first_rect:?}");
    let mut prev = first_rect;
    for frame in 0..360 {
        rt.frame(1.0 / 120.0);
        let cur = rt.presented("b").unwrap().0;
        assert!(jump(cur, prev) < 12.0, "frame {frame}: {prev:?} -> {cur:?}");
        prev = cur;
    }
    assert!(angle_diff(rt.presented_pose("b").unwrap().rotation(), 0.9) < 1e-4);
}
