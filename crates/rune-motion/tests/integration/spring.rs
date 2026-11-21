use rune_motion::prelude::*;
use rune_motion::{Animated, Decay};

/// RK4 on x'' = −k(x − T) − c x' in f64 with a tiny step: the reference the closed form must match.
fn integrate(s: Spring, x0: f64, v0: f64, target: f64, t_end: f64) -> (f64, f64) {
    let k = (s.stiffness()) as f64;
    let c = (s.damping()) as f64;
    let f = |x: f64, v: f64| (v, -k * (x - target) - c * v);
    let dt = 1e-5;
    let (mut x, mut v) = (x0, v0);
    let steps = (t_end / dt).round() as usize;
    for _ in 0..steps {
        let (a1, b1) = f(x, v);
        let (a2, b2) = f(x + 0.5 * dt * a1, v + 0.5 * dt * b1);
        let (a3, b3) = f(x + 0.5 * dt * a2, v + 0.5 * dt * b2);
        let (a4, b4) = f(x + dt * a3, v + dt * b3);
        x += dt / 6.0 * (a1 + 2.0 * a2 + 2.0 * a3 + a4);
        v += dt / 6.0 * (b1 + 2.0 * b2 + 2.0 * b3 + b4);
    }
    (x, v)
}

#[test]
fn closed_form_matches_ode_integration_in_every_regime() {
    let springs = [
        Spring::new(0.3, 0.0),   // critical (Kit's default)
        Spring::new(0.5, 0.3),   // under (SwiftUI .bouncy)
        Spring::new(0.4, 0.7),   // very bouncy
        Spring::new(0.35, -0.5), // over
        Spring::new(0.6, 0.0005), // near-critical, under
        Spring::new(0.25, -0.0005), // near-critical, over
    ];
    let starts = [(0.0f32, 0.0f32), (0.0, 25.0), (0.7, -8.0)];
    for s in springs {
        for (x0, v0) in starts {
            for t in [0.01f32, 0.05, 0.13, 0.3, 0.71] {
                let (x, v) = s.sample(x0, v0, 1.0, t);
                let (rx, rv) = integrate(s, x0 as f64, v0 as f64, 1.0, t as f64);
                // Position within 1e-4 per unit of travel; velocity scaled by ω.
                assert!((x as f64 - rx).abs() < 1e-4, "{s:?} x0={x0} v0={v0} t={t}: x {x} vs {rx}");
                let vtol = 1e-4 * s.omega() as f64 + 1e-4 * v0.abs() as f64;
                assert!((v as f64 - rv).abs() < vtol, "{s:?} x0={x0} v0={v0} t={t}: v {v} vs {rv}");
            }
        }
    }
}

#[test]
fn swiftui_parameter_mapping() {
    // Spring(duration: 0.5, bounce: 0): stiffness (2π/0.5)² ≈ 157.914, damping 4π ≈ 25.133.
    let s = Spring::new(0.5, 0.0);
    assert!((s.stiffness() - 157.913_67).abs() < 1e-2, "{}", s.stiffness());
    assert!((s.damping() - 25.132_74).abs() < 1e-3, "{}", s.damping());
    assert_eq!(Spring::BOUNCY.zeta(), 0.7);
    assert!((Spring::SNAPPY.zeta() - 0.85).abs() < 1e-6);
    assert_eq!(Spring::new(0.4, -0.5).zeta(), 2.0);
    // Round trips.
    let back = Spring::from_stiffness_damping(s.stiffness(), s.damping(), 1.0);
    assert!((back.duration - 0.5).abs() < 1e-5 && back.bounce.abs() < 1e-5);
    let r = Spring::from_response(0.55, 0.825);
    assert!((r.duration - 0.55).abs() < 1e-6 && (r.zeta() - 0.825).abs() < 1e-6);
    let over = Spring::from_omega_zeta(10.0, 2.0);
    assert!((over.bounce + 0.5).abs() < 1e-6 && (over.zeta() - 2.0).abs() < 1e-5);
    // Mass: k/m and c/(2√(km)).
    let m = Spring::from_stiffness_damping(400.0, 20.0, 2.0);
    assert!((m.omega() - (200.0f32).sqrt()).abs() < 1e-4);
    // Motion's visualDuration is ours / 1.2.
    assert!((Spring::from_motion(0.45, 0.0).duration - 0.54).abs() < 1e-6);
    assert!((motion::GLIDE.motion_visual_duration() - 0.32 / 1.2).abs() < 1e-6);
}

#[test]
fn kit_tokens_match_kit_motion_json() {
    let expect = [
        ("instant", 0.1, 0.0, 0.1, 0.0),
        ("snap", 0.25, 0.0, 0.2, 0.0),
        ("settle", 0.3, 0.0, 0.24, 0.0),
        ("reflow", 0.4, 0.0, 0.32, 0.0),
        ("glide", 0.32, 0.0, 0.26, 0.0),
        ("morph", 0.3, 0.16, 0.27, 0.08),
        ("pop", 0.25, 0.15, 0.2, 0.05),
        ("drift", 0.85, 0.0, 0.6, 0.0),
    ];
    for ((name, ed, eb, xd, xb), (n, pair)) in expect.iter().zip(motion::ALL) {
        assert_eq!(name, n);
        assert_eq!((pair.enter.duration, pair.enter.bounce), (*ed, *eb), "{name}");
        assert_eq!((pair.exit.duration, pair.exit.bounce), (*xd, *xb), "{name}");
        if *name != "instant" {
            // Exits run 10–25 % shorter (instant is already at the floor).
            let ratio = xd / ed;
            assert!((0.7..=0.9).contains(&ratio), "{name} exit ratio {ratio}");
        }
    }
}

#[test]
fn settle_time_bounds_the_whole_tail() {
    for s in [Spring::new(0.3, 0.0), Spring::new(0.5, 0.3), Spring::new(0.4, -0.4), Spring::new(0.3, 0.9)] {
        let ts = s.settling_duration(1e-3);
        assert!(ts > 0.0 && ts < 20.0, "{s:?} {ts}");
        for i in 0..2000 {
            let t = ts + i as f32 * 0.005;
            let (x, v) = s.sample(0.0, 0.0, 1.0, t);
            assert!((x - 1.0).abs() <= 1.01e-3, "{s:?} t={t} x={x}");
            assert!(v.abs() <= 1.01e-2, "{s:?} t={t} v={v}");
        }
    }
    // Critically damped at duration d settles in about d (SwiftUI's perceptual duration).
    let t = Spring::new(0.5, 0.0).settling_duration(1e-3);
    assert!(t > 0.4 && t < 1.0, "{t}");
}

#[test]
fn critically_damped_from_rest_never_overshoots() {
    let s = motion::SETTLE;
    let mut prev = 0.0;
    for i in 1..400 {
        let (x, _) = s.sample(0.0, 0.0, 1.0, i as f32 / 400.0);
        assert!(x >= prev && x <= 1.0);
        prev = x;
    }
}

#[test]
fn animated_value_retargets_keeping_velocity() {
    let mut a = Animated::new(0.0f32);
    a.animate_to(100.0, motion::GLIDE, 0.0);
    let (x1, v1) = (a.sample(0.1), a.velocity(0.1));
    a.animate_to(-50.0, motion::GLIDE, 0.1);
    assert!((a.sample(0.1) - x1).abs() < 1e-4);
    assert!((a.velocity(0.1) - v1).abs() < 1e-2);
    a.retarget(-50.0, 0.2); // unchanged: no-op
    assert!(a.is_settled(5.0));
    assert!((a.sample(5.0) + 50.0).abs() < 1e-3);
    // Decay coasts to x + v/k.
    let mut d = Animated::new(0.0f32);
    d.release(1000.0, 0.0, Curve::Decay(Decay::IOS_NORMAL), 0.0);
    assert!((d.target() - 1000.0 / Decay::IOS_NORMAL.rate).abs() < 1e-2);
    assert!((d.sample(30.0) - d.target()).abs() < 1e-2);
}

/// Golden samples for `animate_to`/`release` across every curve kind, so a refactor of the
/// lane restart can't drift the numbers.
///
/// Samples are pinned with a tight tolerance, not a bit hash. Springs and decay sample
/// `exp`/`sin`/`cos` from the platform's libm, and glibc and Apple's libm disagree in the
/// last bits: the same run at the same revision moves a value of ~400 by up to 3e-5. The
/// tolerance below is about 20× that platform noise and still orders of magnitude below a
/// real change to how a lane restarts.
#[test]
fn animated_retarget_golden() {
    const ANCHORS: [usize; 6] = [0, 1, 3, 9, 21, 39];
    /// Per curve, per anchor, `[x.x, x.y, v.x, v.y]`.
    const SAMPLES: [[[f32; 4]; 6]; 5] = [
        [
            [30.078003, 7.5296297, -800.0, 400.0],
            [15.205883, 12.992083, -426.75085, 80.23154],
            [3.2957265, 11.149232, -112.73632, -93.2812],
            [-0.20551914, 1.577376, 1.509766, -24.364733],
            [-0.0026638499, 0.009844182, 0.045319706, -0.1750176],
            [0.0, 0.0, 0.0, 0.0],
        ],
        [
            [56.422695, -0.41329098, -800.0, 400.0],
            [38.957146, 7.563013, -600.40234, 244.27997],
            [17.499737, 14.067252, -277.22702, 37.680252],
            [11.163436, 5.5244317, 65.7598, -71.8772],
            [20.527206, -0.43597618, -1.4065659, 3.7947469],
            [19.988346, 0.0052623716, 0.1345897, -0.10417824],
        ],
        [
            [48.35736, 7.3219295, -800.0, 400.0],
            [30.425076, 14.737576, -593.33307, 237.03568],
            [11.4471245, 21.428228, -188.11761, 41.604954],
            [29.531113, 6.3093452, 231.20714, -141.46454],
            [40.0, 0.0, 0.0, -0.0],
            [40.0, 0.0, 0.0, -0.0],
        ],
        [
            [111.8616, -21.43715, -800.0, 400.0],
            [92.35385, -11.683273, -760.94543, 380.47272],
            [56.148865, 6.41922, -688.46295, 344.23148],
            [-33.05693, 51.022118, -509.8728, 254.9364],
            [-148.05019, 108.518745, -279.6561, 139.82805],
            [-230.99655, 149.99193, -113.5973, 56.79865],
        ],
        [
            [80.0, 0.0, 0.0, 0.0],
            [80.0, 0.0, 0.0, 0.0],
            [80.0, 0.0, 0.0, 0.0],
            [80.0, 0.0, 0.0, 0.0],
            [80.0, 0.0, 0.0, 0.0],
            [80.0, 0.0, 0.0, 0.0],
        ],
    ];
    const TARGETS: [[f32; 2]; 5] = [[0.0, 0.0], [20.0, 0.0], [40.0, 0.0], [-287.7384, 178.36285], [80.0, 0.0]];
    let close = |got: f32, want: f32| (got - want).abs() <= 2e-6 + 1e-6 * want.abs();

    let tween = Curve::Tween { duration: 0.3, easing: Easing::EASE_OUT };
    let curves = [Curve::Spring(motion::GLIDE), Curve::Spring(Spring::new(0.5, 0.3)), tween, Curve::Decay(Decay::IOS_NORMAL), Curve::Instant];
    for (n, &c) in curves.iter().enumerate() {
        let mut a = Animated::new(Vec2::new(0.0, 10.0));
        a.animate_to(Vec2::new(100.0, -40.0), Anim::new(Curve::Spring(motion::SNAP)), 0.0);
        a.animate_to(Vec2::new(-30.0, 60.0), Anim::new(c), 0.07);
        a.animate_to(Vec2::new(5.0, 5.0), Anim::new(c).velocity(Velocity::Uniform(250.0)), 0.13);
        a.release(Vec2::new(-800.0, 400.0), Vec2::new(20.0 * n as f32, 0.0), c, 0.2);
        for (i, &k) in ANCHORS.iter().enumerate() {
            let t = 0.2 + k as f64 * 0.025;
            let (x, v) = (a.sample(t), a.velocity(t));
            let got = [x.x, x.y, v.x, v.y];
            for (j, (&g, &w)) in got.iter().zip(&SAMPLES[n][i]).enumerate() {
                assert!(close(g, w), "curve {n} sample {k} field {j}: {g} vs {w}");
            }
        }
        let tg = a.target();
        assert!(close(tg.x, TARGETS[n][0]) && close(tg.y, TARGETS[n][1]), "curve {n} target: {tg:?} vs {:?}", TARGETS[n]);
    }
}

#[test]
fn css_linear_export() {
    let (css, ms) = motion::SETTLE.css_linear();
    assert!(css.starts_with("linear(0,") && css.ends_with(",1)"));
    assert!(ms > 200 && ms < 1000, "{ms}");
}

#[test]
fn the_none_spring_is_an_instant_curve_everywhere_a_spring_converts() {
    use rune_motion::{Anim, Curve, Effect, PropClass, Spring, SpringPair, Transition};
    assert_eq!(Curve::from(Spring::NONE), Curve::Instant);
    assert_eq!(Anim::from(Spring::NONE).curve, Curve::Instant);
    assert_eq!(Anim::from(SpringPair::NONE).curve, Curve::Instant);
    // Reduce Motion keeps it instant; a short spring would become a fade.
    assert_eq!(PropClass::Presence.reduce(Spring::NONE.into()), Curve::Instant);
    let t = Transition::none();
    assert_eq!((t.insertion, t.removal, t.springs), (Effect::IDENTITY, Effect::IDENTITY, SpringPair::NONE));
    // Any other spring, however short, is still a spring.
    assert_eq!(Curve::from(Spring::new(0.001, 0.0)), Curve::Spring(Spring::new(0.001, 0.0)));
}
