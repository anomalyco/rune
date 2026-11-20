use super::*;

fn run(fps: f32, dist: f32) -> Vec<(f32, f32)> {
    let mut t = GrowTrack::settled(0.0);
    t.retarget(dist);
    let mut acc = 0.0;
    let mut out = Vec::new();
    let dt = 1.0 / fps;
    for k in 1..=(2.0 * fps) as usize {
        acc += dt;
        while acc >= SUBSTEP - 1e-6 {
            t.step(GROWTH, &edge::FEATHER, false);
            acc -= SUBSTEP;
        }
        out.push((k as f32 * dt, t.h));
    }
    out
}

#[test]
fn same_curve_at_30_60_120_fps() {
    let a = run(30.0, 457.0);
    let c = run(120.0, 457.0);
    for (t, h) in a {
        let (_, h2) = c.iter().copied().find(|(t2, _)| (t2 - t).abs() < 1e-3).unwrap();
        assert!((h - h2).abs() < 0.5, "t={t} {h} vs {h2}");
    }
}

#[test]
fn monotone_and_capped() {
    let r = run(120.0, 900.0);
    let mut prev = 0.0;
    for &(_, h) in &r {
        assert!(h >= prev - 1e-4);
        assert!(h - prev <= caps::MAX_LONG / 120.0 + 0.01);
        prev = h;
    }
    assert!((r.last().unwrap().1 - 900.0).abs() < 0.5);
}

#[test]
fn a_whole_row_glides_on_the_spring_alone() {
    let mut t = GrowTrack { whole: true, ..GrowTrack::settled(0.0) };
    t.retarget(900.0);
    let (mut prev, mut speeds) = (0.0, Vec::new());
    for _ in 0..180 {
        for _ in 0..2 {
            t.step(GROWTH, &edge::FEATHER, true);
        }
        assert!(t.h >= prev - 1e-4 && t.h <= 900.0, "monotone, no overshoot: {}", t.h);
        speeds.push(t.h - prev);
        prev = t.h;
    }
    let at = |s: f32| speeds.iter().take((s * 120.0) as usize).sum::<f32>();
    assert!(at(0.6) > 0.98 * 900.0, "past the capped speed: {} pt in 0.6 s", at(0.6));
    assert!(!t.moving(), "settled within 1.5 s");
    // One glide: speed rises, then falls, once.
    let peak = speeds.iter().enumerate().fold((0, 0.0), |m, (i, &v)| if v > m.1 { (i, v) } else { m }).0;
    assert!(speeds[..=peak].windows(2).all(|w| w[1] >= w[0] - 1e-3), "{speeds:?}");
    // (The settle snap closes the last SETTLE pt.)
    assert!(speeds[peak..].windows(2).all(|w| w[1] <= w[0] + SETTLE), "{speeds:?}");
}

#[test]
fn a_far_whole_glide_opens_no_harder_than_its_reach() {
    // The largest change in per-frame step at 120 Hz.
    let jerk = |dist: f32| {
        let mut t = GrowTrack { whole: true, ..GrowTrack::settled(0.0) };
        t.retarget(dist);
        let (mut prev, mut step, mut most) = (0.0, 0.0, 0.0f32);
        for _ in 0..360 {
            t.step(GROWTH, &edge::FEATHER, false);
            t.step(GROWTH, &edge::FEATHER, false);
            most = most.max((t.h - prev - step).abs());
            (step, prev) = (t.h - prev, t.h);
        }
        assert!(!t.moving(), "{dist} pt settles");
        most
    };
    // The spring's opening acceleration over a reach-long glide, as a step change per frame.
    let omega = std::f32::consts::TAU / GROWTH.duration;
    let most = WHOLE_REACH * omega * omega / (120.0 * 120.0);
    assert!(jerk(WHOLE_REACH / 2.0) < 0.6 * most, "a short glide keeps the spring's duration");
    for dist in [WHOLE_REACH, 900.0, 3600.0] {
        assert!(jerk(dist) <= most, "{dist} pt: {} vs {most}", jerk(dist));
    }
}

#[test]
fn eases_into_target() {
    for streaming in [false, true] {
        let mut t = GrowTrack::settled(0.0);
        t.streaming = streaming;
        t.retarget(300.0);
        let mut speeds = Vec::new();
        let mut prev = 0.0;
        for _ in 0..120 {
            for _ in 0..4 {
                t.step(GROWTH, &edge::FEATHER, false);
            }
            speeds.push(t.h - prev);
            prev = t.h;
        }
        // motion-lint's stop-dead: a frame at ≥ 0.5 px followed by one under 15% of it.
        for w in speeds.windows(2) {
            assert!(!(w[0] >= 0.5 && w[1] <= 0.15 * w[0]), "{streaming}: {speeds:?}");
        }
    }
}

const FOLD: Spring = Spring::new(0.18, 0.0);

/// A `fold_glide` row whose bottom leaves the window at 900 pt, stepped at 120 Hz on a slow stack spring and a
/// slow commit floor, neither of which may slow it: (seconds, height) per frame.
fn fold(from: f32, to: f32, secs: f32) -> (GrowTrack, Vec<(f32, f32)>) {
    let mut t = GrowTrack { whole: true, duration: 1.0, ..GrowTrack::settled(from) };
    t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: None }, 900.0)));
    t.retarget(to);
    let mut out = vec![(0.0, t.h)];
    for k in 1..=(secs * 120.0) as usize {
        t.step(GROWTH, &edge::FEATHER, false);
        t.step(GROWTH, &edge::FEATHER, false);
        out.push((k as f32 / 120.0, t.h));
    }
    (t, out)
}

/// The height `secs` into a fold.
fn at(hs: &[(f32, f32)], secs: f32) -> f32 {
    hs[(secs * 120.0).round() as usize].1
}

#[test]
fn a_fold_glides_on_its_own_spring_however_far() {
    for (from, to) in [(40.0, 640.0), (640.0, 40.0), (40.0, 900.0)] {
        let (t, hs) = fold(from, to, 0.5);
        // A critically damped spring's duration is when it looks done: under 2% to go.
        let left = (at(&hs, FOLD.duration) - to).abs();
        assert!(left < 0.02 * (to - from).abs(), "{from} -> {to}: {left} pt left at 0.18 s");
        assert!(!t.moving(), "{from} -> {to} settles within 0.5 s: {hs:?}");
        assert!(hs.windows(2).all(|w| (w[1].1 - w[0].1) * (to - from) >= -1e-3), "monotone: {hs:?}");
        assert!(hs.iter().all(|&(_, h)| h >= from.min(to) - 1e-3 && h <= from.max(to) + 1e-3), "no overshoot");
    }
}

#[test]
fn a_fold_lands_rather_than_creeping_its_last_sixteenth() {
    // The spring's tail under a sixteenth of a point moves in steps the pixel snap holds and
    // lets go, which reads as the row stopping and setting off again.
    for (from, to) in [(640.0, 40.0), (40.0, 640.0), (3040.0, 40.0), (41.0, 40.0), (40.0, 60.0)] {
        let mut t = GrowTrack { whole: true, ..GrowTrack::settled(from) };
        t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: None }, 900.0)));
        t.retarget(to);
        let hs: Vec<f32> = (0..60).map(|_| { t.step(GROWTH, &edge::FEATHER, false); t.step(GROWTH, &edge::FEATHER, false); t.h }).collect();
        let creeping = hs.iter().filter(|&&h| h != to && (h - to).abs() < SETTLE).count();
        assert!(creeping <= 2, "{from} -> {to}: {creeping} frames within a sixteenth of a point: {hs:?}");
        assert!(!t.moving() && t.h == to);
    }
}

#[test]
fn a_fold_past_the_window_moves_only_what_shows() {
    // Shutting: the part below the window goes first, then what shows glides.
    let (t, hs) = fold(3040.0, 40.0, 0.5);
    assert_eq!(hs[0].1, 900.0);
    assert!(at(&hs, FOLD.duration) < 40.0 + 18.0, "{}", at(&hs, FOLD.duration));
    assert!(!t.moving() && t.h == 40.0);
    // Opening: the spring carries the row to the window's edge, then the rest lands at once.
    let (t, hs) = fold(40.0, 3040.0, 0.5);
    assert!(!t.moving() && t.h == 3040.0);
    let jump = hs.iter().position(|&(_, h)| h > 900.0).unwrap();
    assert!(hs[jump - 1].1 > 898.0 && hs[jump].1 == 3040.0, "{:?}", &hs[jump - 2..=jump]);
    assert!(at(&hs, FOLD.duration) > 900.0 - 18.0, "{}", at(&hs, FOLD.duration));
    assert!(hs[jump].0 < 0.3, "lands in the spring's own time: {}", hs[jump].0);
}

#[test]
fn a_glide_handed_to_a_whole_row_mid_way_carries_on() {
    let mut t = GrowTrack { whole: true, ..GrowTrack::settled(40.0) };
    t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: None }, 200.0)));
    t.retarget(640.0);
    for _ in 0..10 {
        t.step(GROWTH, &edge::FEATHER, false);
    }
    t.set_kind(false, true, None);
    let mut prev = t.h;
    while t.moving() {
        t.step(GROWTH, &edge::FEATHER, false);
        assert!(t.h - prev < 20.0, "a step of {} pt at {}", t.h - prev, t.h);
        prev = t.h;
    }
    assert!((t.h - 640.0).abs() < 0.01, "{}", t.h);
}

#[test]
fn a_fold_feathers_its_moving_edge_both_ways() {
    for (from, to) in [(40.0, 640.0), (640.0, 40.0)] {
        let mut t = GrowTrack { whole: true, ..GrowTrack::settled(from) };
        t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: Some(20.0) }, f32::INFINITY)));
        t.retarget(to);
        let mut masks = Vec::new();
        for _ in 0..120 {
            t.step(GROWTH, &edge::FEATHER, false);
            t.step(GROWTH, &edge::FEATHER, false);
            masks.push(t.mask);
        }
        let peak = masks.iter().copied().fold(0.0, f32::max);
        assert!((peak - 20.0).abs() < 1e-3, "{from} -> {to}: feathers to its own depth: {masks:?}");
        assert!(masks[0] < 6.0, "{from} -> {to}: softens as the spring picks up: {masks:?}");
        assert!(masks.windows(2).all(|w| (w[1] - w[0]).abs() < 6.0), "{from} -> {to}: no jump in depth: {masks:?}");
        assert_eq!(*masks.last().unwrap(), 0.0, "{from} -> {to}: crisp at rest");
    }
}

#[test]
fn a_fold_reversed_mid_glide_keeps_its_velocity() {
    let mut t = GrowTrack { whole: true, ..GrowTrack::settled(640.0) };
    t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: None }, 900.0)));
    t.retarget(40.0);
    for _ in 0..12 {
        t.step(GROWTH, &edge::FEATHER, false);
    }
    let (h, v) = (t.h, t.v);
    assert!(v < 0.0 && h < 640.0 && h > 40.0);
    t.retarget(640.0);
    assert_eq!(t.h, h, "no jump");
    t.step(GROWTH, &edge::FEATHER, false);
    assert!(t.h < h, "carries on shutting for a moment: {h} -> {}", t.h);
    let mut masks = vec![t.mask];
    for _ in 0..120 {
        t.step(GROWTH, &edge::FEATHER, false);
        masks.push(t.mask);
    }
    // The feather holds through the turn and thins as the glide lands.
    assert!(masks.windows(2).all(|w| (w[1] - w[0]).abs() < 3.0), "{masks:?}");
    assert_eq!(*masks.last().unwrap(), 0.0);
    assert!(!t.moving() && t.h == 640.0, "settles within 0.5 s: {} at {} pt/s", t.h, t.v);
}

#[test]
fn a_fold_reopened_by_what_it_was_cut_by_still_feathers() {
    // Cut by 12 pt under its stuck header and reopened by exactly 12, a fold's contents ride
    // its bottom edge by arithmetic: the slope lands on -1.0 without the row ever being in
    // riding's state, so the glide is an easing and its moving edge feathers like any other.
    let cut = || {
        let mut t = GrowTrack::settled(640.0);
        t.set_kind(false, false, Some((FoldGlide { spring: FOLD, feather: None }, 900.0)));
        t.retarget(120.0);
        t.hide_above(12.0);
        t
    };
    // Riding: the contents aren't cut by the edge, so there's nothing to soften.
    let mut riding = cut();
    assert!(matches!(riding.lift, Lift::Riding { .. }));
    riding.step(GROWTH, &edge::FEATHER, false);
    assert_eq!(riding.mask, 0.0, "riding feathers nothing");
    // Reopened by exactly what it was cut by: the same 12 pt, on its edge by the numbers.
    let mut t = cut();
    t.retarget(640.0);
    assert_eq!(t.lift, Lift::Easing { a: 640.0, b: -1.0 }, "pinned by the numbers, not by state");
    assert_eq!(t.lift(), 12.0);
    t.step(GROWTH, &edge::FEATHER, false);
    assert!(t.mask > 2.4, "the moving edge softens at once: {} pt", t.mask);
}

#[test]
fn retarget_keeps_velocity() {
    let mut t = GrowTrack::settled(0.0);
    t.retarget(100.0);
    for _ in 0..24 {
        t.step(GROWTH, &edge::FEATHER, false);
    }
    let (h, v) = (t.h, t.v);
    t.retarget(200.0);
    t.step(GROWTH, &edge::FEATHER, false);
    assert!((t.v - v).abs() < v.abs() * 0.5 + 60.0, "{v} -> {}", t.v);
    assert!(t.h > h);
}
