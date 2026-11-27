use crate::{Beat, Easing, Sequence};

const RISE: &[Beat] = &[
    Beat::new(0.0).opacity(0.0).translate(0.0, 4.0).rotate(-0.4),
    Beat::new(0.2).easing(Easing::LINEAR),
];

fn row(reduced: &bool, cx: &mut crate::Cx<bool>) -> crate::El {
    if *reduced { cx.reduced_motion(true); }
    crate::el().size(300.0, 300.0).child(
        crate::el().key("row").absolute(100.0, 100.0).size(100.0, 20.0).fill(crate::Color::WHITE)
            .offset(2.0, 0.0).sequence(Sequence::new(RISE)),
    )
}

#[test]
fn a_sequence_translates_and_turns_its_element_then_rests_in_place() {
    let mut rt = crate::Runtime::new(false, row, (300.0, 300.0), crate::Env::new(true, 1.0));
    let pose = |rt: &crate::Runtime<bool>| {
        let p = rt.presented_pose("row").unwrap();
        (p.rotation(), rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap().rect)
    };
    // The first frame already draws the first beat, so the row doesn't flash at rest.
    rt.frame(0.0);
    let (turn, rect) = pose(&rt);
    assert!((turn + 0.4).abs() < 1e-3, "first turn {turn}");
    assert!((rect[1] + rect[3] * 0.5 - 114.0).abs() < 0.5, "first {rect:?}");
    rt.frame(0.1);
    // Halfway: 2pt up from its offset place and half the turn left, on top of `El::offset`.
    let (turn, rect) = pose(&rt);
    assert!((turn + 0.2).abs() < 1e-3, "turn {turn}");
    let centre = [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5];
    assert!((centre[0] - 152.0).abs() < 0.5 && (centre[1] - 112.0).abs() < 0.5, "centre {centre:?} of {rect:?}");
    for _ in 0..4 { rt.frame(0.1); }
    let (turn, rect) = pose(&rt);
    assert_eq!(turn, 0.0);
    assert_eq!(rect, [102.0, 100.0, 100.0, 20.0]);
}

#[test]
fn reduce_motion_lands_a_moving_sequence_at_its_last_beat() {
    let mut rt = crate::Runtime::new(true, row, (300.0, 300.0), crate::Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.presented_pose("row").unwrap().rotation(), 0.0);
    assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap().rect, [102.0, 100.0, 100.0, 20.0]);
}

struct Turn {
    rotate: f32,
    pan: crate::Live<[f32; 2]>,
    angle: crate::Live<f32>,
}

fn turned(s: &Turn, _: &mut crate::Cx<Turn>) -> crate::El {
    crate::el().size(300.0, 300.0)
        .child(crate::el().key("turned").absolute(100.0, 100.0).size(100.0, 20.0).fill(crate::Color::WHITE)
            .rotate(s.rotate).rotate_motion(crate::Curve::tween(0.2, Easing::LINEAR)).sequence(Sequence::new(RISE)))
        .child(crate::el().key("bound").absolute(100.0, 200.0).size(100.0, 20.0).fill(crate::Color::WHITE)
            .live_offset(&s.pan).live_rotation(&s.angle).sequence(Sequence::new(RISE)))
}

#[test]
fn after_a_moving_sequence_el_rotate_springs_on_its_motion_and_live_bindings_keep_their_channels() {
    let state = Turn { rotate: 0.0, pan: crate::Live::new([0.0; 2]), angle: crate::Live::new(0.3) };
    let mut rt = crate::Runtime::new(state, turned, (300.0, 300.0), crate::Env::new(true, 1.0));
    let rect = |rt: &crate::Runtime<Turn>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).unwrap().rect;
    rt.frame(0.0);
    // Live bindings own their channels while the sequence plays: no sequence turn or lift.
    rt.frame(0.1);
    assert!((rt.presented_pose("bound").unwrap().rotation() - 0.3).abs() < 1e-5);
    assert!(rt.state.pan.set([5.0, 0.0]));
    rt.frame(1.0 / 120.0);
    let bound = rect(&rt, "bound");
    let centre = [bound[0] + bound[2] * 0.5, bound[1] + bound[3] * 0.5];
    assert!((centre[0] - 155.0).abs() < 0.5 && (centre[1] - 210.0).abs() < 0.5, "live offset kept: {centre:?}");
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    assert!(!rt.animating, "a finished sequence writes nothing more");
    assert_eq!(rt.presented_pose("turned").unwrap().rotation(), 0.0);
    // A later `El::rotate` change springs on `rotate_motion` instead of snapping.
    rt.update(|s, _| s.rotate = 0.5);
    rt.frame(0.0);
    rt.frame(0.1);
    let mid = rt.presented_pose("turned").unwrap().rotation();
    assert!(mid > 0.1 && mid < 0.4, "rotate tweens: {mid}");
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!((rt.presented_pose("turned").unwrap().rotation() - 0.5).abs() < 1e-5);
    assert!((rt.presented_pose("bound").unwrap().rotation() - 0.3).abs() < 1e-5);
    assert!(!rt.animating);
}

const SETTLE_UP: &[Beat] = &[Beat::new(0.0), Beat::new(0.2).translate(0.0, -3.0).rotate(0.2).easing(Easing::LINEAR)];

fn lifted(_: &u32, _: &mut crate::Cx<u32>) -> crate::El {
    crate::el().size(300.0, 300.0).child(
        crate::el().key("lifted").absolute(100.0, 100.0).size(20.0, 20.0).fill(crate::Color::WHITE)
            .sequence(Sequence::new(SETTLE_UP)),
    )
}

#[test]
fn a_finished_sequence_keeps_its_last_moved_pose_across_commits() {
    let mut rt = crate::Runtime::new(0u32, lifted, (300.0, 300.0), crate::Env::new(true, 1.0));
    let centre = |rt: &crate::Runtime<u32>| {
        let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("lifted")).unwrap().rect;
        [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5]
    };
    rt.frame(0.0);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    rt.update(|s, _| *s += 1);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    let c = centre(&rt);
    assert!((c[0] - 110.0).abs() < 0.05 && (c[1] - 107.0).abs() < 0.05, "held lifted: {c:?}");
    assert!((rt.presented_pose("lifted").unwrap().rotation() - 0.2).abs() < 1e-5);
    assert!(!rt.animating);
}

struct Shift {
    offset: f32,
    rotate: f32,
    on: bool,
}

const SWAY: &[Beat] = &[
    Beat::new(0.0),
    Beat::new(0.25).translate(6.0, 0.0).rotate(0.3).easing(Easing::LINEAR),
    Beat::new(0.5).easing(Easing::LINEAR),
];

fn shifted(s: &Shift, _: &mut crate::Cx<Shift>) -> crate::El {
    let delayed = crate::el().key("delayed").absolute(100.0, 100.0).size(20.0, 20.0).fill(crate::Color::WHITE)
        .offset(s.offset, 0.0).rotate(s.rotate).sequence(Sequence::new(RISE).delay(1.0));
    let mut swaying = crate::el().key("swaying").absolute(100.0, 200.0).size(20.0, 20.0).fill(crate::Color::WHITE)
        .offset(s.offset, 0.0).rotate(s.rotate);
    if s.on {
        swaying = swaying.sequence(Sequence::new(SWAY).repeating());
    }
    crate::el().size(300.0, 300.0).child(delayed).child(swaying)
}

fn centre_of(rt: &crate::Runtime<Shift>, k: &str) -> [f32; 2] {
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).unwrap().rect;
    [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5]
}

#[test]
fn a_delayed_one_shot_holds_its_first_beat_and_offset_changes_land_there_at_once() {
    let mut rt = crate::Runtime::new(Shift { offset: 0.0, rotate: 0.0, on: true }, shifted, (300.0, 300.0), crate::Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.2);
    // Waiting: the first beat (4 pt down, turned -0.4) on top of `El::offset`.
    let c = centre_of(&rt, "delayed");
    assert!((c[0] - 110.0).abs() < 0.05 && (c[1] - 114.0).abs() < 0.05, "waiting {c:?}");
    rt.update(|s, _| { s.offset = 10.0; s.rotate = 0.1; });
    rt.frame(1.0 / 120.0);
    let c = centre_of(&rt, "delayed");
    assert!((c[0] - 120.0).abs() < 0.05 && (c[1] - 114.0).abs() < 0.05, "lands at once {c:?}");
    assert!((rt.presented_pose("delayed").unwrap().rotation() + 0.3).abs() < 1e-5);
    // Then it plays and rests on its last beat over the new offset and rotation.
    for _ in 0..90 { rt.frame(1.0 / 60.0); }
    let c = centre_of(&rt, "delayed");
    assert!((c[0] - 120.0).abs() < 0.05 && (c[1] - 110.0).abs() < 0.05, "rests {c:?}");
    assert!((rt.presented_pose("delayed").unwrap().rotation() - 0.1).abs() < 1e-5);
}

#[test]
fn dropping_a_repeating_moving_sequence_springs_exactly_home_and_sleeps() {
    let mut rt = crate::Runtime::new(Shift { offset: 2.0, rotate: 0.05, on: true }, shifted, (300.0, 300.0), crate::Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..12 { rt.frame(1.0 / 60.0); }
    let swung = centre_of(&rt, "swaying");
    assert!(swung[0] > 113.0, "swaying {swung:?}");
    rt.update(|s, _| s.on = false);
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    let c = centre_of(&rt, "swaying");
    assert!((c[0] - 112.0).abs() < 1e-3 && (c[1] - 210.0).abs() < 1e-3, "home {c:?}");
    assert_eq!(rt.presented_pose("swaying").unwrap().rotation(), 0.05);
    assert!(!rt.animating);
}

#[test]
fn a_releasing_moving_sequence_returns_offset_and_rotate_to_element_rest_and_sleeps() {
    const FLOURISH: &[Beat] = &[
        Beat::new(0.0).translate(-6.0, 2.0).rotate(-0.4),
        Beat::new(0.2).translate(10.0, -4.0).rotate(std::f32::consts::PI).easing(Easing::LINEAR),
    ];
    struct Icon {
        play: u32,
        offset: [f32; 2],
        rotate: f32,
        delay: f64,
        reduced: bool,
        tick: u32,
    }
    fn view(s: &Icon, cx: &mut crate::Cx<Icon>) -> crate::El {
        if s.reduced { cx.reduced_motion(true); }
        let _ = s.tick;
        let mut seq = Sequence::new(FLOURISH).release();
        if s.delay > 0.0 { seq = seq.delay(s.delay); }
        crate::el().size(300.0, 300.0).child(
            crate::el().key(format!("icon-{}", s.play)).absolute(100.0, 100.0).size(20.0, 20.0)
                .fill(crate::Color::WHITE).transition(crate::Transition::none())
                .offset(s.offset[0], s.offset[1])
                .rotate(s.rotate).rotate_motion(crate::Curve::tween(0.2, Easing::LINEAR))
                .sequence(seq),
        )
    }
    let pose = |rt: &crate::Runtime<Icon>, play: u32| {
        let k = format!("icon-{play}");
        let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k.as_str())).unwrap().rect;
        let centre = [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5];
        (centre, rt.presented_pose(&k).unwrap().rotation())
    };
    let mut rt = crate::Runtime::new(
        Icon { play: 0, offset: [4.0, -3.0], rotate: 0.25, delay: 0.0, reduced: false, tick: 0 },
        view, (300.0, 300.0), crate::Env::new(true, 1.0),
    );
    // Starts on beat 0 on top of the element's own offset (4, -3) and rotation 0.25.
    assert!(rt.frame(0.0));
    let (c, rot) = pose(&rt, 0);
    assert!((c[0] - 108.0).abs() < 0.05 && (c[1] - 109.0).abs() < 0.05, "start {c:?}");
    assert!((rot - (0.25 - 0.4)).abs() < 1e-4, "start rot {rot}");
    // Mid-play (t = 0.1): translate (2, -1), rotate (PI - 0.4) / 2.
    assert!(rt.frame(0.1));
    let (c, rot) = pose(&rt, 0);
    assert!((c[0] - 116.0).abs() < 0.05 && (c[1] - 106.0).abs() < 0.05, "mid {c:?}");
    assert!((rot - (0.25 + (std::f32::consts::PI - 0.4) * 0.5)).abs() < 1e-3, "mid rot {rot}");
    // Unrelated commit on the exact ending frame must still snap cleanly to rest, never unwind.
    rt.update(|s, _| s.tick += 1);
    assert!(!rt.frame(0.1), "ending frame sleeps immediately");
    assert!(!rt.animating);
    assert_eq!(rt.wake_deadline(), None);
    let (c, rot) = pose(&rt, 0);
    assert!((c[0] - 114.0).abs() < 0.05 && (c[1] - 107.0).abs() < 0.05, "rest {c:?}");
    assert!((rot - 0.25).abs() < 1e-5, "rest rot {rot}");
    // Later unrelated commits keep the element at its own pose and asleep.
    rt.update(|s, _| s.tick += 1);
    assert!(!rt.frame(1.0 / 60.0));
    let (c, rot) = pose(&rt, 0);
    assert!((c[0] - 114.0).abs() < 0.05 && (c[1] - 107.0).abs() < 0.05);
    assert!((rot - 0.25).abs() < 1e-5);
    // A later `El::rotate` change still tweens on `rotate_motion` from the resting angle.
    rt.update(|s, _| s.rotate = 0.75);
    rt.frame(0.0);
    rt.frame(0.1);
    let (_, mid_rot) = pose(&rt, 0);
    assert!(mid_rot > 0.35 && mid_rot < 0.65, "rotate tweens after release: {mid_rot}");
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!((pose(&rt, 0).1 - 0.75).abs() < 1e-5);
    assert!(!rt.animating);
    // Changing the key replays the flourish over the new resting rotation, then releases back.
    rt.update(|s, _| s.play = 1);
    assert!(rt.frame(0.0));
    assert!((pose(&rt, 1).1 - (0.75 - 0.4)).abs() < 1e-4);
    assert!(!rt.frame(0.25));
    assert!(!rt.animating);
    let (c, rot) = pose(&rt, 1);
    assert!((c[0] - 114.0).abs() < 0.05 && (c[1] - 107.0).abs() < 0.05);
    assert!((rot - 0.75).abs() < 1e-5);
    // Delayed release keeps the element's own pose during the delay, then plays and releases.
    let mut delayed = crate::Runtime::new(
        Icon { play: 0, offset: [4.0, -3.0], rotate: 0.25, delay: 0.5, reduced: false, tick: 0 },
        view, (300.0, 300.0), crate::Env::new(true, 1.0),
    );
    assert!(!delayed.frame(0.0));
    assert_eq!(delayed.wake_deadline(), Some(0.5));
    let (c, rot) = pose(&delayed, 0);
    assert!((c[0] - 114.0).abs() < 0.05 && (c[1] - 107.0).abs() < 0.05, "delay rest {c:?}");
    assert!((rot - 0.25).abs() < 1e-5, "delay rest rot {rot}");
    assert!(delayed.frame(0.5));
    assert!((pose(&delayed, 0).1 - (0.25 - 0.4)).abs() < 1e-4);
    assert!(!delayed.frame(0.2));
    assert!(!delayed.animating);
    assert!((pose(&delayed, 0).1 - 0.25).abs() < 1e-5);
    // Reduce Motion rests immediately at the element's own pose and sleeps.
    let mut reduced = crate::Runtime::new(
        Icon { play: 0, offset: [4.0, -3.0], rotate: 0.25, delay: 0.0, reduced: true, tick: 0 },
        view, (300.0, 300.0), crate::Env::new(true, 1.0),
    );
    assert!(!reduced.frame(0.0));
    let (c, rot) = pose(&reduced, 0);
    assert!((c[0] - 114.0).abs() < 0.05 && (c[1] - 107.0).abs() < 0.05);
    assert!((rot - 0.25).abs() < 1e-5);
    assert_eq!(reduced.wake_deadline(), None);
}
