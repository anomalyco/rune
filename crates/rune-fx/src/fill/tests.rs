use super::pack::oklab;
use super::*;

fn card() -> Shape {
    Shape::rounded(100.0, 100.0, 240.0, 140.0, 18.0)
}

fn every_preset() -> impl Iterator<Item = (&'static StyleInfo, &'static Preset)> {
    fill_styles().iter().flat_map(|s| s.presets.iter().map(move |p| (s, p)))
}

fn packed(l: &FxLayer) -> Option<(InstRaw, Vec<[f32; 4]>)> {
    let (_, mut i, _) = l.pack()?;
    let mut data = Vec::new();
    l.fill(&mut i, &mut data);
    Some((i, data))
}

fn bits(i: &InstRaw) -> Vec<u32> {
    bytemuck::bytes_of(i).chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

#[test]
fn idle_fills_are_culled() {
    for (s, p) in every_preset() {
        assert!(Fx::fill(p.fill().intensity(0.0)).on(card()).pack().is_none(), "{} {}", s.id, p.id);
        assert!(Fx::fill(p.fill().intensity(f32::NAN)).on(card()).pack().is_none(), "{} {}", s.id, p.id);
        assert!(Fx::fill(p.fill().border(-1.0)).on(card()).pack().is_none());
        assert!(Fx::fill(p.fill().border(f32::NAN)).on(card()).pack().is_none());
        assert!(Fx::fill(p.fill().border(f32::INFINITY)).on(card()).pack().is_none());
        assert!(Fx::fill(p.fill()).on(Shape::rounded(10.0, 10.0, 0.0, 40.0, 0.0)).pack().is_none(), "{} {}: zero width", s.id, p.id);
        assert!(Fx::fill(p.fill()).on(Shape::rounded(10.0, 10.0, 40.0, 0.0, 0.0)).pack().is_none(), "{} {}: zero height", s.id, p.id);
        assert!(Fx::fill(p.fill()).on(card()).pack().is_some());
    }
}

#[test]
fn reduced_motion_holds_one_pose() {
    for (s, p) in every_preset() {
        let l = Fx::fill(p.fill()).on(card()).seed(3).reduced(true);
        assert!(!l.is_animating(), "{} {}", s.id, p.id);
        let (a, da) = packed(&l.at(0.5)).unwrap();
        let (b, db) = packed(&l.at(7.25)).unwrap();
        assert_eq!(bits(&a), bits(&b), "{} {}: same instance at any time", s.id, p.id);
        assert_eq!(da, db, "{} {}: same tables at any time", s.id, p.id);
    }
}

#[test]
fn invisible_fills_never_animate() {
    for (s, p) in every_preset() {
        let off = Fx::fill(FillParams { intensity: 0.0, ..p.fill() }).on(card()).seed(11);
        assert!(!off.is_animating() && off.pack().is_none(), "{} {}", s.id, p.id);
        assert!(!Fx::fill(p.fill()).on(card()).opacity(0.0).is_animating(), "{} {}", s.id, p.id);
    }
    assert!(every_preset().any(|(_, p)| p.fill().animates()));
}

#[test]
fn moving_fills_animate_and_are_deterministic() {
    for (s, p) in every_preset() {
        let l = Fx::fill(p.fill()).on(card()).seed(11);
        assert_eq!(l.is_animating(), p.fill().animates(), "{} {}", s.id, p.id);
        let (a, da) = packed(&l.at(1.5)).unwrap();
        let (b, db) = packed(&l.at(1.5)).unwrap();
        assert_eq!(bits(&a), bits(&b));
        assert_eq!(da, db);
        assert!(da.iter().flatten().all(|v| v.is_finite()), "{} {}", s.id, p.id);
    }
    let moving = |s: FillStyle| FillParams::new(s).animates();
    assert!(moving(LiquidMetalParams::gold().into()));
    assert!(!moving(LiquidMetalParams { flow: 0.0, ..LiquidMetalParams::gold() }.into()));
    assert!(!moving(FlutedGlassParams::reeded().into()));
    assert!(!moving(DitherParams::bayer().into()), "a ramp holds still");
    assert!(!moving(WaveParams { amplitude: 0.0, ..WaveParams::sea() }.into()), "flat waves hold still");
}

#[test]
fn large_times_fold_without_precision_loss_or_jump() {
    assert_eq!(cpu::fold_t(0.0), 0.0);
    assert_eq!(cpu::fold_t(1.2), 1.2);
    assert_eq!(cpu::fold_t(512.0), 512.0);
    for t in [1e4f32, 1e5, -1e5] {
        let a = cpu::fold_t(t);
        let b = cpu::fold_t(t + 1.0 / 60.0);
        assert!((0.0..=512.0).contains(&a) && (0.0..=512.0).contains(&b), "{t} -> {a}");
        assert!((b - a).abs() > 1e-3 && (b - a).abs() < 0.05, "smooth step at {t}: {a} -> {b}");
    }
    for (s, p) in every_preset() {
        let l = Fx::fill(p.fill()).on(card()).seed(7);
        let (i0, d0) = packed(&l.at(1e5)).unwrap();
        let (i1, d1) = packed(&l.at(1e5 + 0.25)).unwrap();
        assert!(i0.t[0] >= 0.0 && i0.t[0] <= 512.0, "{} {}", s.id, p.id);
        assert!(d0.iter().flatten().all(|v| v.is_finite()), "{} {}", s.id, p.id);
        if p.fill().animates() {
            assert!(i0.t[0] != i1.t[0] || d0 != d1, "{} {} still moves at t = 1e5", s.id, p.id);
        }
    }
}

#[test]
fn backdrop_and_placement_follow_the_input() {
    for (s, p) in every_preset() {
        let f = p.fill();
        if f.needs_backdrop() {
            assert!(s.reads_backdrop, "{} {}", s.id, p.id);
        }
        let l = Fx::fill(f).on(card());
        assert_eq!(l.fx.needs_backdrop(), f.needs_backdrop());
        assert_eq!(l.place, if f.needs_backdrop() { Place::Over } else { Place::Under });
        assert_eq!(l.fx.cost().backdrop, f.needs_backdrop());
    }
    assert!(Fx::fill(DitherParams::backdrop()).needs_backdrop());
    assert!(!Fx::fill(DitherParams::bayer()).needs_backdrop());
}

#[test]
fn element_anchor_only_moves_the_anchor_slot() {
    assert_eq!(LiquidMetalParams::default().anchor, FillAnchor::World);
    let world = packed(&Fx::fill(LiquidMetalParams::gold()).on(card()).at(3.0)).unwrap().0;
    let element = packed(&Fx::fill(LiquidMetalParams::gold().anchor(FillAnchor::Element)).on(card()).at(3.0)).unwrap().0;
    assert_eq!(world.p2, [0.0; 4]);
    assert_eq!(element.p2, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(bits(&InstRaw { p2: [0.0; 4], ..element }), bits(&world));
}

#[test]
fn plastic_holds_still() {
    for p in [PlasticParams::glossy(), PlasticParams::matte(), PlasticParams::dished()] {
        assert!(!FillParams::from(p).animates());
        assert!(!Fx::fill(p).on(card()).at(2.0).is_animating());
        assert_eq!(Fx::fill(p).cost(), FxCost { passes: 1, samples: 0, alu: 0, backdrop: false, source: false });
    }
    let non_finite_plastic = PlasticParams { roughness: f32::NAN, dome: f32::INFINITY, bevel: f32::NEG_INFINITY, angle: f32::NAN, ..PlasticParams::glossy() };
    let non_finite_metal = LiquidMetalParams {
        flow: f32::NAN,
        scale: f32::NAN,
        speed: f32::INFINITY,
        bevel: f32::NAN,
        dome: f32::NEG_INFINITY,
        angle: f32::NAN,
        iridescence: f32::INFINITY,
        contrast: f32::NAN,
        ..LiquidMetalParams::chrome()
    };
    for fx in [Fx::fill(non_finite_plastic), Fx::fill(non_finite_metal)] {
        let (i, _) = packed(&fx.on(card()).at(1.0)).unwrap();
        assert!(i.p0.iter().chain(&i.p1).chain(&i.p2).all(|v| v.is_finite()), "{i:?}");
    }
}

#[test]
fn mesh_points_wander_within_drift() {
    let p = MeshGradientParams::dusk();
    for k in 0..40 {
        let pts = cpu::mesh_points(&p, k as f32 * 0.37, 5);
        for (q, r) in pts.iter().zip(p.points) {
            assert!((q[0] - r[0]).abs() <= p.drift * 1.5 && (q[1] - r[1]).abs() <= p.drift * 1.5, "{q:?} {r:?}");
        }
    }
    let still = MeshGradientParams { drift: 0.0, ..p };
    assert_eq!(cpu::mesh_points(&still, 3.0, 5), p.points);
}

#[test]
fn blue_noise_tile_ranks_every_pixel_once() {
    let t = cpu::blue_noise();
    assert_eq!(t.len() * 4, cpu::BLUE_N * cpu::BLUE_N);
    let mut v: Vec<f32> = t.iter().flatten().copied().collect();
    v.sort_by(f32::total_cmp);
    for (k, x) in v.iter().enumerate() {
        assert!((x - (k as f32 + 0.5) / v.len() as f32).abs() < 1e-6);
    }
    // Blue: 2x2 box means stay near 0.5 (white noise would spread them far wider).
    let n = cpu::BLUE_N;
    let at = |x: usize, y: usize| t[(y % n * n + x % n) / 4][(y % n * n + x % n) % 4];
    let worst = (0..n * n).map(|i| (i % n, i / n)).map(|(x, y)| ((at(x, y) + at(x + 1, y) + at(x, y + 1) + at(x + 1, y + 1)) / 4.0 - 0.5).abs()).fold(0.0f32, f32::max);
    assert!(worst < 0.36, "{worst}");
}

#[test]
fn oklab_of_white_and_grey_is_neutral() {
    let w = oklab(Rgb([1.0; 3]));
    assert!((w[0] - 1.0).abs() < 1e-3 && w[1].abs() < 1e-3 && w[2].abs() < 1e-3, "{w:?}");
    let g = oklab(rgb(0x777777));
    assert!(g[1].abs() < 1e-3 && g[2].abs() < 1e-3);
}

#[test]
fn oklab_matches_canonical_for_non_negative_rgb_and_clamps_negative_lms() {
    assert_eq!(0.412_221_46f32.to_bits(), 0.412_221_47f32.to_bits());
    for v in 0..=255u8 {
        let c = rune_motion::srgb_decode(v as f32 / 255.0);
        for rgb in [[c, c, c], [c, 0.0, 0.0], [0.0, c, 0.0], [0.0, 0.0, c], [c, 1.0 - c, 0.5 * c]] {
            let got = oklab(Rgb(rgb));
            let canon = rune_motion::linear_to_oklab(rgb[0], rgb[1], rgb[2]);
            assert_eq!(got.map(f32::to_bits), canon.map(f32::to_bits), "{rgb:?}");
        }
    }
    for rgb in [[0.0, 0.0, 0.0], [2.5, 1.2, 0.4], [0.001, 0.999, 0.123]] {
        let got = oklab(Rgb(rgb));
        let canon = rune_motion::linear_to_oklab(rgb[0], rgb[1], rgb[2]);
        assert_eq!(got.map(f32::to_bits), canon.map(f32::to_bits), "{rgb:?}");
    }
    // Negative inputs can make LMS negative before `cbrt`, where `pack::oklab` clamps LMS to 0.
    let neg = [-0.5f32, 0.2, 0.1];
    assert_ne!(oklab(Rgb(neg)), rune_motion::linear_to_oklab(neg[0], neg[1], neg[2]));
    assert_ne!(oklab(Rgb(neg)), rune_motion::linear_to_oklab(neg[0].max(0.0), neg[1].max(0.0), neg[2].max(0.0)));
}

#[test]
fn stops_keep_order_and_cap() {
    let s = Stops::hex(&[0xff0000, 0x00ff00, 0x0000ff]);
    assert_eq!(s.len(), 3);
    assert_eq!(s.as_slice()[1].hex(), 0x00ff00);
    assert_eq!(Stops::hex(&[0; 9]).len(), MAX_STOPS);
    assert_eq!(Stops::new(&[]).len(), 1);
}
