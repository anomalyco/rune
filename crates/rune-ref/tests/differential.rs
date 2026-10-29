#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
#![cfg(feature = "gpu")]
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, TestError, TestRunner},
};
use rune_ref::{
    Frame, Primitive, Scene, compare, generate,
    gpu::{GpuRenderer, text_run},
};
use std::{cell::RefCell, path::PathBuf};

// A provisional 2-LSB cap per primitive; measured maxima and backend in research/24.
// Composite has its own 2-LSB cap, never multiplied by primitive count.
const BUDGET: [u8; 5] = [2, 2, 2, 2, 2];
fn check(gpu: &mut GpuRenderer, scene: &Scene, tolerance: u8) -> Result<u8, String> {
    let mut cpu = Frame::new(scene.width, scene.height, true);
    cpu.render(scene);
    let actual = gpu.render(scene);
    let diff = compare(scene, &cpu, &actual, tolerance);
    if diff.outlier_pixels > 0 {
        Err(serde_json::to_string(&diff).unwrap())
    } else {
        Ok(diff.max_channel)
    }
}
fn save(name: &str, scene: &Scene) {
    let dir = std::env::var_os("RUNE_REF_FAILURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("failures"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(scene).unwrap()).unwrap();
    eprintln!("saved replay {}", path.display());
}

#[test]
fn cpu_gpu_corpus() {
    let mut gpu = GpuRenderer::default();
    let mut maxima = [0u8; 6];
    for seed in 0..128 {
        let scene = generate::seeded(seed, 8);
        for primitive in &scene.primitives {
            let mut single = scene.clone();
            single.primitives = vec![primitive.clone()];
            let kind = primitive.params[1] as usize;
            match check(&mut gpu, &single, BUDGET[kind]) {
                Ok(max) => maxima[kind] = maxima[kind].max(max),
                Err(e) => {
                    save(&format!("seed-{seed}-kind-{kind}"), &single);
                    panic!("{e}");
                }
            }
        }
        match check(&mut gpu, &scene, 2) {
            Ok(max) => maxima[5] = maxima[5].max(max),
            Err(e) => {
                save(&format!("seed-{seed}-composite"), &scene);
                panic!("{e}");
            }
        }
    }
    for seed in 0..16 {
        let mut scene = generate::seeded(seed, 4);
        text_run(&mut scene, seed);
        if let Err(e) = check(&mut gpu, &scene, 2) {
            save(&format!("text-{seed}"), &scene);
            panic!("{e}");
        }
    }
    eprintln!("METRIC channel_max_rect_shadow_glyph_inset_gradient_composite={maxima:?}");
}

#[test]
fn cpu_gpu_proptest() {
    let gpu = RefCell::new(GpuRenderer::default());
    let mut runner = TestRunner::new(Config {
        cases: 128,
        max_shrink_iters: 2048,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/differential.proptest-regressions",
        ))),
        ..Config::default()
    });
    let strategy = prop::collection::vec(any::<[u8; 32]>(), 1..9);
    let result = runner.run(&strategy, |records| {
        let scene = generate::scene(&records);
        check(&mut gpu.borrow_mut(), &scene, 2).map_err(TestCaseError::fail)?;
        Ok(())
    });
    if let Err(TestError::Fail(reason, records)) = &result {
        save("minimized", &generate::scene(records));
        eprintln!("minimized differential failure: {reason}");
    }
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn saved_regressions() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("regressions");
    let mut gpu = GpuRenderer::default();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let scene: Scene = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        scene.validate().unwrap();
        assert!(check(&mut gpu, &scene, 2).is_ok(), "{}", path.display());
    }
}

// Quarter turns about a centre on the pixel grid keep every turned edge half a pixel from the
// nearest pixel centre and every sample on a texel centre, so rasterizer and oracle agree.
// The clipped pass cuts the turned footprint unevenly, so a clip in the glyph's own frame differs.
#[test]
fn rotated_glyphs() {
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut gpu = GpuRenderer::default();
    let clips = [
        (rune_ref::NO_CLIP, [0.0; 4]),
        ([24.5, 15.25, 40.0, 26.75], [3.0, 0.0, 0.0, 2.5]),
    ];
    for (n, angle) in [FRAC_PI_2, PI, PI + FRAC_PI_2, -FRAC_PI_2].into_iter().enumerate() {
        for uv in [[1.0, 1.0, 0.0, 0.0], [1.0, 1.0, 8.0, 5.0]] {
            for (c, (clip, fade)) in clips.into_iter().enumerate() {
                let mut scene = generate::scene(&[]);
                scene.primitives.push(Primitive {
                    node: 1,
                    source: format!("rotated/{n}"),
                    rect: [20.0, 17.0, 16.0, 10.0],
                    radii: [0.0; 4],
                    fill: [0.7, 0.6, 0.5, 0.8],
                    border: [0.0; 4],
                    params: [angle, 2.0, 0.0, 1.0],
                    uv,
                    clip,
                    fade,
                    fade_origin: None,
                    turn: None,
                });
                if let Err(e) = check(&mut gpu, &scene, 2) {
                    save(&format!("rotated-{n}-{}-clip-{c}", uv[2]), &scene);
                    panic!("{e}");
                }
            }
        }
    }
}

#[test]
fn gpu_metamorphic() {
    let mut gpu = GpuRenderer::default();
    let close = |a: &[u8], b: &[u8]| {
        assert_eq!(a.len(), b.len());
        assert!(
            a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 2),
            "metamorphic pixel mismatch"
        );
    };
    for seed in 0..32 {
        let scene = generate::seeded(seed, 8);
        let original = gpu.render(&scene);
        let mut shifted = scene.clone();
        shifted.width += 4;
        shifted.height += 6;
        generate::translate(&mut shifted, 4.0, 6.0);
        let actual = gpu.render(&shifted);
        for y in 0..64 {
            close(
                &original[y * 256..(y + 1) * 256],
                &actual[((y + 6) * 68 + 4) * 4..((y + 6) * 68 + 68) * 4],
            );
        }
        let mut transparent = scene.clone();
        for mut p in scene.primitives.clone() {
            p.fill = [0.0; 4];
            p.border = [0.0; 4];
            if p.params[1] == 4.0 {
                p.uv = [0.0; 4];
            }
            transparent.primitives.push(p);
        }
        assert_eq!(original, gpu.render(&transparent));
        let mut full = scene.clone();
        for p in &mut full.primitives {
            p.clip = rune_ref::NO_CLIP;
            p.fade = [0.0; 4];
        }
        let unclipped = gpu.render(&full);
        for p in &mut full.primitives {
            p.clip = [0.0, 0.0, 64.0, 64.0];
        }
        close(&unclipped, &gpu.render(&full));
        let mut disjoint = scene.clone();
        for p in &mut disjoint.primitives {
            p.clip = [0.0, 0.0, 31.0, 64.0];
            p.fade = [0.0; 4];
        }
        let n = disjoint.primitives.len();
        for mut p in scene.primitives.clone() {
            p.rect[0] += 32.0;
            p.clip = [32.0, 0.0, 64.0, 64.0];
            p.fade = [0.0; 4];
            disjoint.primitives.push(p);
        }
        let first = gpu.render(&disjoint);
        disjoint.primitives.rotate_left(n);
        assert_eq!(first, gpu.render(&disjoint));
    }
}

#[test]
fn cpu_gpu_dashed() {
    let mut gpu = GpuRenderer::default();
    let mut max = 0;
    for seed in 0..64 {
        let scene = generate::dashed(seed, 8);
        for (n, primitive) in scene.primitives.iter().enumerate() {
            let mut single = scene.clone();
            single.primitives = vec![primitive.clone()];
            match check(&mut gpu, &single, 2) {
                Ok(m) => max = max.max(m),
                Err(e) => {
                    save(&format!("dashed-{seed}-{n}"), &single);
                    panic!("{e}");
                }
            }
        }
        if let Err(e) = check(&mut gpu, &scene, 2) {
            save(&format!("dashed-{seed}-composite"), &scene);
            panic!("{e}");
        }
    }
    eprintln!("METRIC channel_max_dashed={max}");
}

#[test]
fn cpu_gpu_effect_slots() {
    let mut gpu = GpuRenderer::default();
    for seed in 0..16 {
        let mut scene = generate::seeded(seed, 8);
        let mut slots = scene.primitives.clone();
        for slot in &mut slots {
            slot.fill = [0.0, 0.0, 0.0, 1.0];
            slot.border = [0.0; 4];
            slot.params = [0.0, 12.0, 6.0, 1.0];
            slot.uv = [0.0; 4];
        }
        scene.primitives.extend(slots);
        scene.primitives.rotate_right(4);
        if let Err(e) = check(&mut gpu, &scene, 2) {
            save(&format!("effect-{seed}"), &scene);
            panic!("{e}");
        }
    }
}

#[test]
fn cpu_gpu_radial() {
    let mut gpu = GpuRenderer::default();
    let mut max = 0;
    for seed in 0..64 {
        let scene = generate::radial(seed, 8);
        for (n, primitive) in scene.primitives.iter().enumerate() {
            let mut single = scene.clone();
            single.primitives = vec![primitive.clone()];
            match check(&mut gpu, &single, 2) {
                Ok(m) => max = max.max(m),
                Err(e) => {
                    save(&format!("radial-{seed}-{n}"), &single);
                    panic!("{e}");
                }
            }
        }
        if let Err(e) = check(&mut gpu, &scene, 2) {
            save(&format!("radial-{seed}-composite"), &scene);
            panic!("{e}");
        }
    }
    // Multi-stop radial gradient split into two non-overlapping band segments at t = 0.5.
    let mut multi = generate::scene(&[]);
    let base = Primitive {
        node: 1,
        source: "radial/multi-0".into(),
        rect: [8.0, 8.0, 48.0, 48.0],
        radii: [12.0; 4],
        fill: [0.8, 0.2, 0.1, 0.9],
        border: [0.2, 0.7, 0.4, 0.9],
        params: [0.0, 11.0, 0.0, 2.5],
        uv: [24.0, 24.0, 24.0, 20.0],
        clip: rune_ref::NO_CLIP,
        fade: [0.0; 4],
        fade_origin: None,
        turn: None,
    };
    let outer = Primitive {
        node: 2,
        source: "radial/multi-1".into(),
        fill: base.border,
        border: [0.05, 0.1, 0.3, 0.4],
        params: [2.5, 11.0, 0.0, 1.0],
        ..base.clone()
    };
    multi.primitives = vec![base, outer];
    match check(&mut gpu, &multi, 2) {
        Ok(m) => max = max.max(m),
        Err(e) => {
            save("radial-multi", &multi);
            panic!("{e}");
        }
    }
    eprintln!("METRIC channel_max_radial={max}");
}

#[test]
fn cpu_gpu_cached_subtrees() {
    use rune::{Color, Cx, El, Env, Runtime, col, el, text};
    let mut gpu = GpuRenderer::default();
    let build_scene = |cached: bool| {
        fn view(cached: &bool, _: &mut Cx<bool>) -> El {
            let mut card = col()
                .size(48.0, 44.0)
                .padding(4.0)
                .gap(3.0)
                .fill(Color { r: 0.18, g: 0.24, b: 0.36, a: 1.0 })
                .radius(8.0)
                .border(1.0, Color { r: 0.45, g: 0.62, b: 0.88, a: 1.0 })
                .child(text("Rune").color(Color::WHITE))
                .child(
                    el().size(36.0, 12.0)
                        .fill(Color { r: 0.85, g: 0.32, b: 0.28, a: 0.85 })
                        .radius(4.0),
                );
            if *cached {
                card = card.cache();
            }
            col()
                .size(64.0, 64.0)
                .padding(8.0)
                .fill(Color { r: 0.06, g: 0.08, b: 0.12, a: 1.0 })
                .child(card)
        }
        let mut rt = Runtime::new(cached, view, (64.0, 64.0), Env { scale: 1.0, system_dark: true });
        rt.frame(0.0);
        let primitives = rt
            .instances
            .iter()
            .enumerate()
            .map(|(idx, inst)| Primitive::from_instance(inst, idx as u32 + 1, format!("cache/{idx}")))
            .collect();
        Scene {
            width: 64,
            height: 64,
            clear: [0.025, 0.04, 0.06, 1.0],
            primitives,
            atlas: rune_ref::Atlas {
                width: rune::text::ATLAS_SIZE,
                height: rune::text::ATLAS_SIZE,
                data: rt.text.atlas.data.clone(),
            },
        }
    };
    let uncached = build_scene(false);
    let cached = build_scene(true);
    assert!(
        cached.primitives.iter().any(|p| p.params[1] == 8.0 && p.params[3] == 1.0),
        "cached scene must emit KIND_LAYER with cache marker"
    );
    check(&mut gpu, &uncached, 2).expect("uncached CPU vs GPU");
    check(&mut gpu, &cached, 2).expect("cached CPU vs GPU");
    let gpu_uncached = gpu.render(&uncached);
    let gpu_cached = gpu.render(&cached);
    let mut cpu_uncached = Frame::new(64, 64, true);
    cpu_uncached.render(&uncached);
    let diff = compare(&cached, &cpu_uncached, &gpu_cached, 2);
    assert_eq!(diff.outlier_pixels, 0, "cached GPU vs uncached CPU: {diff:?}");
    for (a, b) in gpu_uncached.iter().zip(&gpu_cached) {
        assert!(a.abs_diff(*b) <= 1, "cached vs uncached GPU mismatch: {a} vs {b}");
    }

    // A scrolled multi-line text culls offscreen lines by band and clips overhanging ink at the
    // viewport edges identically on CPU and GPU.
    fn scrolled(_: &(), _: &mut Cx<()>) -> El {
        let lines = (0..40).map(|i| format!("Line {i:02} rune")).collect::<Vec<_>>().join("\n");
        col()
            .size(64.0, 64.0)
            .padding(6.0)
            .fill(Color { r: 0.06, g: 0.08, b: 0.12, a: 1.0 })
            .child(
                col()
                    .w_full()
                    .h(52.0)
                    .scroll(rune::ScrollSpec { stick_bottom: false })
                    .child(text(lines).font(rune::TextStyle::new(11.0, 400.0).lh(14.0)).color(Color::WHITE)),
            )
    }
    let mut rt = Runtime::new((), scrolled, (64.0, 64.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    rt.wheel(32.0, 32.0, -145.0);
    rt.frame(1.0 / 60.0);
    let scrolled_scene = Scene {
        width: 64,
        height: 64,
        clear: [0.025, 0.04, 0.06, 1.0],
        primitives: rt
            .instances
            .iter()
            .enumerate()
            .map(|(idx, inst)| Primitive::from_instance(inst, idx as u32 + 1, format!("scroll/{idx}")))
            .collect(),
        atlas: rune_ref::Atlas {
            width: rune::text::ATLAS_SIZE,
            height: rune::text::ATLAS_SIZE,
            data: rt.text.atlas.data.clone(),
        },
    };
    check(&mut gpu, &scrolled_scene, 2).expect("scrolled multi-line text CPU vs GPU");
}

#[test]
fn cpu_gpu_cached_rounded_clip() {
    use rune::render::{Instance, LayerSpec};
    let mut scene = generate::scene(&[]);
    scene.width = 32;
    scene.height = 32;
    let begin = Instance::begin_layer([0.0, 0.0, 24.0, 24.0], [0.0, 0.0, 24.0, 24.0],
        LayerSpec { clip_radius: 8.0, ..Default::default() });
    let mut paint = generate::scene(&[[255; 32]]).primitives.into_iter().next().unwrap();
    paint.rect = [0.0, 0.0, 24.0, 24.0];
    paint.radii = [0.0; 4];
    paint.fill = [1.0; 4];
    paint.border = [0.0; 4];
    paint.params = [0.0; 4];
    paint.clip = rune_ref::NO_CLIP;
    paint.fade = [0.0; 4];
    paint.fade_origin = None;
    scene.primitives = vec![Primitive::from_instance(&begin, 1, "rounded".into()), paint.clone(),
        Primitive::from_instance(&Instance::end_layer(), 3, "end".into())];
    assert_eq!(f32::from_bits(Instance::from(&scene.primitives[0]).fade[3]), 8.0,
        "replay capture must preserve the rounded clip, not compare two square layers");
    let mut gpu = GpuRenderer::default();
    check(&mut gpu, &scene, 2).expect("rounded layer survives replay capture");

    let mut mixed = Instance::begin_layer([0.0, 0.0, 24.0, 24.0], [0.0, 0.0, 24.0, 24.0], LayerSpec::default());
    mixed.fade[3] = rune::render::pack_clip_radii(mixed.clip, [-10.0, -10.0, 0.0, -4.0]);
    scene.primitives = vec![Primitive::from_instance(&mixed, 1, "mixed-continuous".into()), paint,
        Primitive::from_instance(&Instance::end_layer(), 3, "end".into())];
    assert_eq!(scene.primitives[0].border, [-10.0, -10.0, 0.0, -4.0]);
    check(&mut gpu, &scene, 2).expect("mixed continuous-corner layer matches CPU oracle");
}

#[test]
fn cpu_gpu_turned_layers() {
    use rune::{Color, Cx, El, Env, Runtime, col, el, text};
    let mut gpu = GpuRenderer::default();
    // A card with text, a border, a shadow and a child, turned; one nested in another, and
    // one under a clip, so the layer's clip applies after the turn.
    fn view(angle: &f32, _: &mut Cx<f32>) -> El {
        let card = |a: f32| col()
            .size(30.0, 22.0)
            .padding(3.0)
            .fill(Color { r: 0.18, g: 0.24, b: 0.36, a: 1.0 })
            .radius(5.0)
            .border(1.0, Color { r: 0.45, g: 0.62, b: 0.88, a: 1.0 })
            .shadow(2.0, 4.0, Color::BLACK.alpha(0.4))
            .child(text("Ru").color(Color::WHITE))
            .rotate(a);
        col()
            .size(64.0, 64.0)
            .padding(4.0)
            .gap(2.0)
            .fill(Color { r: 0.06, g: 0.08, b: 0.12, a: 1.0 })
            .child(card(*angle).origin(0.5, 1.0))
            .child(el().size(56.0, 30.0).clip_rounded(4.0).child(card(-*angle).child(
                el().size(8.0, 8.0).fill(Color { r: 0.85, g: 0.32, b: 0.28, a: 0.85 }).rotate(*angle * 2.0),
            )))
    }
    for (n, angle) in [0.3f32, -1.1, std::f32::consts::FRAC_PI_2, 2.6].into_iter().enumerate() {
        let mut rt = Runtime::new(angle, view, (64.0, 64.0), Env { scale: 1.0, system_dark: true });
        rt.frame(0.0);
        let primitives: Vec<Primitive> = rt
            .instances
            .iter()
            .enumerate()
            .map(|(idx, inst)| Primitive::from_instance(inst, idx as u32 + 1, format!("turned/{idx}")))
            .collect();
        assert!(primitives.iter().any(|p| p.params[1] == 8.0 && p.turn.is_some_and(|t| t[0] == angle)), "a turned layer");
        let scene = Scene {
            width: 64,
            height: 64,
            clear: [0.025, 0.04, 0.06, 1.0],
            primitives,
            atlas: rune_ref::Atlas {
                width: rune::text::ATLAS_SIZE,
                height: rune::text::ATLAS_SIZE,
                data: rt.text.atlas.data.clone(),
            },
        };
        if let Err(e) = check(&mut gpu, &scene, 2) {
            save(&format!("turned-{n}"), &scene);
            panic!("{e}");
        }
    }
}
