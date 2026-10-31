use super::*;
use crate::params::invert;

#[test]
fn sparkle_cap_and_tables_reach_64_without_regrowing() {
    let p = SparkleParams { count: 100, burst: Some(0.0), life: 10.0, ..Default::default() };
    let layer = Fx::sparkles(p).on(card()).at(1.0);
    let (_, packed, _) = layer.pack().unwrap();
    assert_eq!(packed.p0[0], 64.0);
    assert_eq!(layer.fx.cost().samples, 128);
    let mut table = Vec::with_capacity(128);
    let capacity = table.capacity();
    let (_, count) = cpu::sparkles(&p, card().rect, 1.0, 1, false, &mut table);
    assert!(count > 32.0 && count <= 64.0);
    assert_eq!(table.len(), count as usize * 2);
    assert_eq!(table.capacity(), capacity);
}

fn card() -> Shape {
    Shape::rounded(100.0, 100.0, 200.0, 120.0, 16.0)
}

#[test]
fn idle_layers_cost_nothing() {
    let l = Fx::glow(GlowParams { intensity: 0.0, ..Default::default() }).on(card());
    assert!(l.pack().is_none());
    let l = Fx::glow(GlowParams { heat: 0.0, ..Default::default() }).on(card());
    assert!(l.pack().is_none(), "a fully cooled light is culled");
    let ripple = PulseParams { style: PulseStyle::Ripple { at: [150.0, 150.0] }, t0: 1.0, duration: 0.5, ..Default::default() };
    assert!(Fx::pulse(ripple).on(card()).at(0.5).pack().is_none(), "before the tap");
    assert!(Fx::pulse(ripple).on(card()).at(1.2).pack().is_some());
    assert!(Fx::pulse(ripple).on(card()).at(1.6).pack().is_none(), "after the ripple");
    assert!(!Fx::pulse(ripple).on(card()).at(1.6).is_animating());
    let shock = DistortParams::shockwave([0.0, 0.0], 0.0);
    assert!(Fx::distort(shock).on(card()).at(2.0).pack().is_none());
}

#[test]
fn layers_that_draw_nothing_never_keep_the_loop_awake() {
    let glow = GlowParams { breathe: 0.3, ..Default::default() };
    let pulse = PulseParams::default();
    let electric = ElectricParams::default();
    let fire = FireParams::default();
    let haze = |amount| DistortParams::HeatHaze { height: 40.0, scale: 14.0, speed: 2.2, amount, whole_shape: false };
    let chroma = |glitch| DistortParams::ChromaSplit { offset: [0.0, 0.0], glitch, feather: 0.0 };
    let shimmer = ShimmerParams::default();
    let sparkles = SparkleParams::default();
    let aurora = AuroraParams::default();
    let grain = GrainParams { fps: 24.0, ..Default::default() };
    let pairs = [
        (Fx::glow(glow), Fx::glow(GlowParams { intensity: 0.0, ..glow })),
        (Fx::glow(glow), Fx::glow(GlowParams { heat: 0.0, ..glow })),
        (Fx::pulse(pulse), Fx::pulse(PulseParams { intensity: 0.0, ..pulse })),
        (Fx::electric(electric), Fx::electric(ElectricParams { intensity: 0.0, ..electric })),
        (Fx::fire(fire), Fx::fire(FireParams { intensity: 0.0, ..fire })),
        (Fx::distort(haze(1.6)), Fx::distort(haze(0.0))),
        (Fx::distort(chroma(0.5)), Fx::distort(chroma(0.0))),
        (Fx::shimmer(shimmer), Fx::shimmer(ShimmerParams { intensity: 0.0, base_alpha: 0.0, ..shimmer })),
        (Fx::sparkles(sparkles), Fx::sparkles(SparkleParams { intensity: 0.0, ..sparkles })),
        (Fx::sparkles(sparkles), Fx::sparkles(SparkleParams { count: 0, ..sparkles })),
        (Fx::aurora(aurora), Fx::aurora(AuroraParams { intensity: 0.0, ..aurora })),
        (Fx::grain(grain), Fx::grain(GrainParams { amount: 0.0, ..grain })),
    ];
    for (live, silent) in pairs {
        let live = live.on(card()).at(1.0);
        assert!(live.is_animating() && live.pack().is_some(), "{live:?}");
        assert!(!live.opacity(0.0).is_animating(), "zero opacity: {live:?}");
        let silent = silent.on(card()).at(1.0);
        assert!(!silent.is_animating() && silent.pack().is_none(), "{silent:?}");
    }
}

#[test]
fn a_ping_ticks_until_its_shot_has_cooled() {
    let ping = PulseParams { style: PulseStyle::Ping { from: [0.0, 0.0], dir: [1.0, 0.0], travel: 200.0 }, t0: 1.0, duration: 0.5, ..Default::default() };
    let at = |t: f32| Fx::pulse(ping).on(card()).at(t);
    let cooling = 1.0 + 0.5 * 2.1;
    assert!(at(cooling).pack().is_some(), "still drawn while it cools");
    assert!(at(cooling).is_animating(), "so the loop keeps ticking");
    let done = 1.0 + 0.5 * 2.3;
    assert!(at(done).pack().is_none() && !at(done).is_animating());
}

#[test]
fn one_shots_overlap_and_retire_by_fading() {
    let mut p = OneShots::new(3);
    for k in 0..3 {
        p.trigger(k as f32 * 0.1, k);
    }
    let live: Vec<_> = p.live(0.25, 1.0).map(|(s, o)| (s.data, o)).collect();
    assert_eq!(live, vec![(0, 1.0), (1, 1.0), (2, 1.0)], "a retrigger never restarts or cuts the running ones");
    // Full: the oldest fades out over FADE instead of vanishing.
    p.trigger(0.3, 3);
    let o0 = |t: f32| p.live(t, 1.0).find(|(s, _)| s.data == 0).map_or(0.0, |(_, o)| o);
    assert_eq!(o0(0.3), 1.0);
    let mut prev = 1.0;
    for i in 1..=20 {
        let v = o0(0.3 + i as f32 * OneShots::<i32>::FADE / 20.0);
        assert!(v <= prev && prev - v < 0.2, "fades smoothly: {prev} -> {v}");
        prev = v;
    }
    assert_eq!(prev, 0.0);
    assert_eq!(p.live(0.31, 1.0).filter(|(_, o)| *o == 1.0).count(), 3);
    // A storm of triggers stays bounded.
    for k in 0..100 {
        p.trigger(0.4 + k as f32 * 0.001, k);
    }
    assert!(p.live(0.5, 1.0).count() <= 6);
    let seeds: std::collections::HashSet<u32> = p.live(0.5, 1.0).map(|(s, _)| s.seed).collect();
    assert!(seeds.len() > 1, "instances get distinct seeds");
}

#[test]
fn opacity_zero_is_culled() {
    assert!(Fx::glow(GlowParams::default()).on(card()).opacity(0.0).pack().is_none());
    let (_, i, _) = Fx::glow(GlowParams::default()).on(card()).opacity(0.4).pack().unwrap();
    assert_eq!(i.shape[2], 0.4);
}

#[test]
fn zaps_and_bursts_live_out_their_window_then_cull() {
    let zap = Fx::electric(ElectricParams { style: ElectricStyle::Arc { a: [0.0, 0.0], b: [200.0, 40.0] }, zap: Some(1.0), ..Default::default() }).on(card());
    assert!(zap.at(0.9).pack().is_none());
    assert!(zap.at(1.2).pack().is_some() && zap.at(1.2).is_animating());
    assert!(zap.at(1.0 + ElectricParams::ZAP_LIFE + 0.01).pack().is_none());
    let p = SparkleParams { burst: Some(2.0), ..Default::default() };
    let burst = Fx::sparkles(p).on(card());
    assert!(burst.at(2.3).pack().is_some());
    assert!(!burst.at(2.0 + p.burst_life() + 0.01).is_animating());
    let mut data = Vec::new();
    let (_, n) = cpu::sparkles(&p, card().rect, 2.0 + p.burst_life() + 0.01, 1, false, &mut data);
    assert_eq!(n, 0.0, "every burst star is gone by burst_life()");

    let dense = SparkleParams { count: 64, burst: Some(0.0), life: 2.0, ..Default::default() };
    let (_, inst, _) = Fx::sparkles(dense).on(card()).at(0.3).pack().unwrap();
    assert_eq!(inst.p0[0], 64.0);
    assert_eq!(Fx::sparkles(SparkleParams { count: 100, ..dense }).cost().samples, 128);
    data.clear();
    let (_, n64) = cpu::sparkles(&dense, card().rect, 0.3, 1, false, &mut data);
    assert_eq!(n64, 64.0, "all 64 burst stars are emitted within the cap");
    assert_eq!(data.len(), 128, "two vec4f entries per star");
}

#[test]
fn motion_blur_is_one_sample_at_rest_and_scales_with_speed() {
    let rest = Rect::new(0.0, 0.0, 100.0, 60.0);
    let p = MotionBlurParams::default();
    let still = MotionSamples::from_track(&p, rest, 1.0, 2.0, |_| Pose::default());
    assert_eq!(still.count, 1);
    assert!(!still.is_moving());
    let slow = MotionSamples::from_track(&p, rest, 1.0, 2.0, |t| Pose { x: t * 300.0, ..Default::default() });
    let fast = MotionSamples::from_track(&p, rest, 1.0, 2.0, |t| Pose { x: t * 1500.0, ..Default::default() });
    assert!(slow.count > 1 && fast.count > slow.count, "{} {}", slow.count, fast.count);
    let w: f32 = fast.xforms[..fast.count as usize].iter().map(|x| x[6]).sum();
    assert!((w - 1.0).abs() < 1e-4, "shutter weights sum to opacity: {w}");
    // Bounds cover the swept rect.
    assert!(fast.bounds.w > rest.w + 10.0);
}

#[test]
fn pose_inverse_round_trips() {
    let rest = Rect::new(10.0, 20.0, 100.0, 60.0);
    let pose = Pose { x: 33.0, y: -7.0, rotation: 0.4, scale: 1.3, opacity: 1.0 };
    let inv = invert(pose.affine(rest));
    let p = pose.apply(rest, [25.0, 40.0]);
    let back = [inv[0] * p[0] + inv[2] * p[1] + inv[4], inv[1] * p[0] + inv[3] * p[1] + inv[5]];
    assert!((back[0] - 25.0).abs() < 1e-3 && (back[1] - 40.0).abs() < 1e-3, "{back:?}");
}

#[test]
fn phase_never_jumps_when_speed_changes() {
    let mut ph = Phase::new(1.0);
    let before = ph.at(2.0);
    ph.set_speed(2.0, 3.0);
    assert!((ph.at(2.0) - before).abs() < 1e-6);
    assert!((ph.at(2.1) - before - 0.3).abs() < 1e-5);
}

#[test]
fn heat_cools_continuously() {
    let mut h = Heat::new(0.1, 0.3);
    h.set(0.0, true);
    let lit = h.at(0.5);
    assert!((lit - 1.0).abs() < 1e-6);
    h.set(0.5, false);
    assert!((h.at(0.5) - lit).abs() < 1e-6, "no cut at switch-off");
    let mut prev = h.at(0.5);
    for i in 1..100 {
        let v = h.at(0.5 + i as f32 * 0.01);
        assert!(v < prev && prev - v < 0.05, "monotonic, no step");
        prev = v;
    }
    // Switching back on mid-cool resumes from the current heat.
    let mid = h.at(0.7);
    h.set(0.7, true);
    assert!((h.at(0.7) - mid).abs() < 1e-6);
}

#[test]
fn cpu_noise_is_bounded_periodic_and_deterministic() {
    for i in 0..2000 {
        let x = i as f32 * 0.173;
        let v = cpu::gnoise(x, x * 0.37, 0, 9);
        assert!(v.abs() <= 1.01, "{v}");
        assert_eq!(v, cpu::gnoise(x, x * 0.37, 0, 9));
        let a = cpu::gnoise(x, 0.4, 7, 3);
        let b = cpu::gnoise(x + 7.0, 0.4, 7, 3);
        assert!((a - b).abs() < 1e-4, "periodic in x");
    }
}

#[test]
fn electric_tables_have_the_declared_layout() {
    let mut data = Vec::new();
    let p = ElectricParams { style: ElectricStyle::Arc { a: [0.0, 0.0], b: [300.0, 0.0] }, strands: 2, forks: 1.0, ..Default::default() };
    let xf = cpu::electric(&p, card(), 1.7, 3, false, &mut data);
    let (n, forks, strands) = (xf[1] as usize, xf[2] as usize, xf[3] as usize);
    assert_eq!(data.len(), 2 + 2 * strands * n / 4 + forks * 6);
    // Endpoints are pinned.
    assert!(data[2][0].abs() < 1e-3);
    let reduced = cpu::electric(&p, card(), 1.7, 3, true, &mut Vec::new());
    assert_eq!(reduced[2], 0.0, "reduced motion: no forks");
}

#[test]
fn zero_amplitude_electricity_is_a_straight_finite_line() {
    for style in [ElectricStyle::Arc { a: [0.0, 0.0], b: [300.0, 0.0] }, ElectricStyle::Border] {
        let p = ElectricParams { style, amplitude: 0.0, forks: 1.0, ..Default::default() };
        for t in [0.0, 0.4, 1.7] {
            let mut data = Vec::new();
            cpu::electric(&p, card(), t, 3, false, &mut data);
            assert!(data.iter().flatten().all(|v| v.is_finite()), "{style:?} at {t}");
        }
    }
}
