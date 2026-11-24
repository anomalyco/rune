#![cfg(feature = "gpu")]
use rune::render::{
    Blend, Instance, KIND_EFFECT, KIND_INSET, KIND_RECT, KIND_SEGMENT, KIND_SHADOW, KIND_TRIANGLE,
    LayerSpec, NO_CLIP, pack_clip_radii,
};
use rune_ref::{Frame, Primitive, Scene, compare, generate, gpu::GpuRenderer};

fn check(gpu: &mut GpuRenderer, scene: &Scene, tolerance: u8) {
    scene.validate().expect("scene must validate");
    let mut cpu = Frame::new(scene.width, scene.height, true);
    cpu.render(scene);
    let actual = gpu.render(scene);
    let diff = compare(scene, &cpu, &actual, tolerance);
    assert_eq!(diff.outlier_pixels, 0, "CPU vs GPU mismatch: {diff:?}");
}

fn from_instances(width: u32, height: u32, instances: &[Instance]) -> Scene {
    let mut scene = generate::scene(&[]);
    scene.width = width;
    scene.height = height;
    scene.primitives = instances
        .iter()
        .enumerate()
        .map(|(i, inst)| Primitive::from_instance(inst, i as u32 + 1, format!("parity/{i}")))
        .collect();
    scene
}

fn zero_instance() -> Instance {
    Instance {
        rect: [0.0; 4],
        radii: [0.0; 4],
        fill: [0.0; 4],
        border: [0.0; 4],
        params: [0.0; 4],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

#[test]
fn cpu_gpu_banded_rects_and_shadows() {
    let mut gpu = GpuRenderer::default();
    let ring = Instance {
        rect: [8.0, 8.0, 48.0, 48.0],
        radii: [-8.0; 4],
        border: [0.85, 0.35, 0.2, 0.9],
        params: [3.0, KIND_RECT, 0.0, 0.9],
        ..zero_instance()
    };
    let shadow = Instance {
        rect: [14.0, 14.0, 36.0, 36.0],
        radii: [6.0; 4],
        fill: [0.0, 0.0, 0.0, 0.6],
        params: [0.0, KIND_SHADOW, 3.0, 1.0],
        ..zero_instance()
    };
    let bands = |inst: Instance, q: [f32; 4], hole: [f32; 4]| -> Vec<Instance> {
        let [qx0, qy0, qx1, qy1] = q;
        let [hx0, hy0, hx1, hy1] = hole;
        [
            [qx0, qy0, qx1, hy0],
            [qx0, hy0, hx0, hy1],
            [hx1, hy0, qx1, hy1],
            [qx0, hy1, qx1, qy1],
        ]
        .into_iter()
        .map(|[x0, y0, x1, y1]| Instance { uv: [x0, y0, -x1, y1], ..inst })
        .collect()
    };
    let ring_bands = bands(ring, [-1.0, -1.0, 49.0, 49.0], [16.0, 16.0, 32.0, 32.0]);
    let shadow_bands = bands(shadow, [-10.0, -10.0, 46.0, 46.0], [8.0, 8.0, 28.0, 28.0]);

    check(&mut gpu, &from_instances(64, 64, &ring_bands), 2);
    check(&mut gpu, &from_instances(64, 64, &shadow_bands), 2);
    check(&mut gpu, &from_instances(64, 64, &[shadow_bands[0], ring_bands[1]]), 2);
}

#[test]
fn cpu_gpu_segments_and_triangles() {
    let mut gpu = GpuRenderer::default();
    let instances = [
        // Round-cap diagonal stroke
        Instance {
            rect: [4.0, 4.0, 30.0, 24.0],
            radii: [8.0, 8.0, 30.0, 24.0],
            fill: [0.9, 0.35, 0.15, 0.95],
            params: [2.25, KIND_SEGMENT, 1.0, 1.0],
            ..zero_instance()
        },
        // Butt-cap hairline (< 1 px wide)
        Instance {
            rect: [32.0, 4.0, 26.0, 20.0],
            radii: [36.0, 10.0, 54.0, 18.0],
            fill: [0.16, 0.68, 0.4, 0.8],
            params: [0.3, KIND_SEGMENT, 0.0, 1.0],
            ..zero_instance()
        },
        // Zero-length round-cap dot
        Instance {
            rect: [42.0, 20.0, 12.0, 12.0],
            radii: [48.0, 26.0, 48.0, 26.0],
            fill: [0.9, 0.85, 0.2, 1.0],
            params: [3.0, KIND_SEGMENT, 1.0, 1.0],
            ..zero_instance()
        },
        // Two triangles sharing an interior hard edge (seam-free ownership)
        Instance {
            rect: [8.0, 32.0, 24.0, 24.0],
            radii: [8.0, 32.0, 32.0, 32.0],
            uv: [32.0, 56.0, 0.0, 0.0],
            fill: [0.175, 0.315, 0.63, 0.7],
            params: [3.0, KIND_TRIANGLE, 0.0, 1.0],
            ..zero_instance()
        },
        Instance {
            rect: [8.0, 32.0, 24.0, 24.0],
            radii: [8.0, 32.0, 32.0, 56.0],
            uv: [8.0, 56.0, 0.0, 0.0],
            fill: [0.175, 0.315, 0.63, 0.7],
            params: [6.0, KIND_TRIANGLE, 0.0, 1.0],
            ..zero_instance()
        },
        // Clipped triangle with all edges antialiased
        Instance {
            rect: [34.0, 32.0, 24.0, 24.0],
            radii: [46.0, 33.0, 57.0, 55.0],
            uv: [35.0, 52.0, 0.0, 0.0],
            fill: [0.72, 0.17, 0.55, 0.85],
            params: [7.0, KIND_TRIANGLE, 0.0, 1.0],
            clip: [38.0, 36.0, 54.0, 53.0],
            ..zero_instance()
        },
    ];
    check(&mut gpu, &from_instances(64, 64, &instances), 2);
}

#[test]
fn cpu_gpu_negative_spread_inset_shadows() {
    let mut gpu = GpuRenderer::default();
    let instances = [
        // Sharp negative-spread inset on square corners (must stay square, not round to +|spread|)
        Instance {
            rect: [4.0, 4.0, 26.0, 26.0],
            radii: [0.0; 4],
            fill: [0.68, 0.2, 0.12, 0.8],
            params: [-4.0, KIND_INSET, 0.0, 1.0],
            uv: [0.0, 6.0, 0.0, 0.0],
            ..zero_instance()
        },
        // Blurred negative-spread inset on circular corners
        Instance {
            rect: [34.0, 4.0, 26.0, 26.0],
            radii: [6.0; 4],
            fill: [0.12, 0.56, 0.34, 0.75],
            params: [-2.0, KIND_INSET, 3.0, 1.0],
            uv: [3.0, 4.0, 0.0, 0.0],
            ..zero_instance()
        },
        // Sharp negative-spread inset on continuous corners
        Instance {
            rect: [10.0, 34.0, 44.0, 24.0],
            radii: [-6.0; 4],
            fill: [0.14, 0.28, 0.63, 0.7],
            params: [-3.0, KIND_INSET, 0.0, 1.0],
            uv: [-4.0, 5.0, 0.0, 0.0],
            ..zero_instance()
        },
    ];
    check(&mut gpu, &from_instances(64, 64, &instances), 2);
}

#[test]
fn cpu_gpu_mixed_clip_radii_quantization() {
    let mut gpu = GpuRenderer::default();
    // Fractional mixed radii (0.5 px step) on a direct Primitive (not round-tripped from Instance)
    let mut fine = generate::scene(&[]);
    fine.width = 64;
    fine.height = 64;
    fine.primitives = vec![Primitive {
        node: 1,
        source: "clip-fine".into(),
        rect: [0.0, 0.0, 64.0, 64.0],
        radii: [0.0; 4],
        fill: [0.9, 0.4, 0.2, 1.0],
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip: [4.0, 4.0, 60.0, 60.0],
        fade: [0.0; 4],
        fade_origin: None,
        turn: None,
        clip_radii: Some([10.3, 4.7, 0.0, 8.2]),
    }];
    check(&mut gpu, &fine, 2);

    // Coarse mixed radii (> 63.5 px -> 2.0 px step) and negative continuous sign
    let mut coarse = generate::scene(&[]);
    coarse.width = 200;
    coarse.height = 200;
    coarse.primitives = vec![Primitive {
        node: 1,
        source: "clip-coarse".into(),
        rect: [0.0, 0.0, 200.0, 200.0],
        radii: [0.0; 4],
        fill: [0.2, 0.7, 0.9, 1.0],
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip: [10.0, 10.0, 190.0, 190.0],
        fade: [0.0; 4],
        fade_origin: None,
        turn: None,
        clip_radii: Some([-71.3, -13.1, 0.0, -40.7]),
    }];
    check(&mut gpu, &coarse, 2);
}

#[test]
fn cpu_gpu_layer_expansion_offscreen_turns_and_scissor_inline() {
    let mut gpu = GpuRenderer::default();

    // 1. Uncached opacity layer with small initial rect containing a child + outer drop shadow.
    let expanded = [
        Instance::begin_layer(
            [24.0, 24.0, 8.0, 8.0],
            NO_CLIP,
            LayerSpec { opacity: 0.65, ..Default::default() },
        ),
        Instance {
            rect: [16.0, 16.0, 28.0, 24.0],
            radii: [6.0; 4],
            fill: [0.0, 0.0, 0.0, 0.7],
            params: [0.0, KIND_SHADOW, 5.0, 1.0],
            ..zero_instance()
        },
        Instance {
            rect: [16.0, 16.0, 28.0, 24.0],
            radii: [6.0; 4],
            fill: [0.8, 0.3, 0.2, 1.0],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            ..zero_instance()
        },
        Instance::end_layer(),
    ];
    check(&mut gpu, &from_instances(64, 64, &expanded), 2);

    // 2. Uncached layer with empty initial rect [24, 24, 0, 0].
    let empty_seed = [
        Instance::begin_layer(
            [24.0, 24.0, 0.0, 0.0],
            NO_CLIP,
            LayerSpec { opacity: 0.5, blend: Blend::PlusLighter, ..Default::default() },
        ),
        Instance {
            rect: [10.0, 10.0, 26.0, 26.0],
            radii: [4.0; 4],
            fill: [0.7, 0.2, 0.1, 0.7],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            ..zero_instance()
        },
        Instance {
            rect: [20.0, 20.0, 26.0, 26.0],
            radii: [4.0; 4],
            fill: [0.14, 0.42, 0.56, 0.7],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            ..zero_instance()
        },
        Instance::end_layer(),
    ];
    check(&mut gpu, &from_instances(64, 64, &empty_seed), 2);

    // 3. Turned layer whose upright content starts at x < 0 and rotates into the viewport.
    let offscreen_turn = [
        Instance::begin_layer([-12.0, 22.0, 24.0, 16.0], NO_CLIP, LayerSpec::default())
            .turned(std::f32::consts::FRAC_PI_2, [16.0, 30.0]),
        Instance {
            rect: [-12.0, 22.0, 24.0, 16.0],
            radii: [4.0; 4],
            fill: [0.2, 0.85, 0.45, 1.0],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            ..zero_instance()
        },
        Instance::end_layer(),
    ];
    check(&mut gpu, &from_instances(64, 64, &offscreen_turn), 2);

    // 4. Integer-clipped opaque layer (`Disposition::Inline` with hardware scissor) with overlapping children.
    let mut scissored_inner = Instance::begin_layer(
        [10.0, 10.0, 32.0, 32.0],
        [12.0, 12.0, 40.0, 40.0],
        LayerSpec { opacity: 0.7, ..Default::default() },
    );
    scissored_inner.fade[3] = pack_clip_radii(scissored_inner.clip, [8.0; 4]);
    let scissored = [
        Instance::begin_layer([8.0, 8.0, 48.0, 48.0], [8.0, 8.0, 48.0, 48.0], LayerSpec::default()),
        Instance {
            rect: [0.0, 0.0, 64.0, 64.0],
            fill: [0.2, 0.1, 0.3, 0.5],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            ..zero_instance()
        },
        scissored_inner,
        Instance {
            rect: [4.0, 4.0, 56.0, 56.0],
            fill: [0.72, 0.48, 0.08, 0.8],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            ..zero_instance()
        },
        Instance::end_layer(),
        Instance::end_layer(),
    ];
    check(&mut gpu, &from_instances(64, 64, &scissored), 2);
}

#[test]
fn cpu_gpu_effect_slot_with_mark_and_legacy_glyph_radii() {
    let mut gpu = GpuRenderer::default();
    let mark: u32 = 0x1234_5678;
    let slot = Instance {
        rect: [8.0, 8.0, 24.0, 24.0],
        fill: [0.0, 0.0, 0.0, 1.0],
        border: [(mark & 0xFFFF) as f32, (mark >> 16) as f32, 0.0, 0.0],
        params: [1.0, KIND_EFFECT, 4.0, 1.0],
        uv: [-3.0, 7.5, 1.0, 0.25],
        ..zero_instance()
    };
    let bg = Instance {
        rect: [4.0, 4.0, 40.0, 40.0],
        fill: [0.3, 0.5, 0.8, 1.0],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        ..zero_instance()
    };
    check(&mut gpu, &from_instances(64, 64, &[bg, slot]), 2);

    // A legacy glyph primitive with non-zero radii and params[2] must still render via page-0 atlas.
    let mut glyph_scene = generate::scene(&[]);
    glyph_scene.primitives = vec![Primitive {
        node: 1,
        source: "legacy-glyph".into(),
        rect: [12.0, 12.0, 16.0, 16.0],
        radii: [12.0, 8.0, 4.0, 2.0],
        fill: [0.8, 0.7, 0.5, 0.9],
        border: [0.0; 4],
        params: [0.0, 2.0, 3.5, 1.2],
        uv: [1.0, 1.0, 0.0, 0.0],
        clip: NO_CLIP,
        fade: [0.0; 4],
        fade_origin: None,
        turn: None,
        clip_radii: None,
    }];
    let inst = Instance::from(&glyph_scene.primitives[0]);
    assert_eq!(inst.radii, [0.0; 4]);
    assert_eq!(inst.params[2], 0.0);
    check(&mut gpu, &glyph_scene, 2);
}

#[test]
fn cpu_gpu_nested_cutout_circles_match_reference() {
    let mut gpu = GpuRenderer::default();
    let mut rt = rune::Runtime::new(
        (),
        |_: &(), _: &mut rune::Cx<()>| {
            rune::el()
                .size(96.0, 48.0)
                .fill(rune::Color { r: 0.15, g: 0.3, b: 0.6, a: 1.0 })
                .child(
                    rune::el()
                        .size(96.0, 48.0)
                        .radius(10.0)
                        .clip_rounded(10.0)
                        .fill(rune::Color::WHITE)
                        .cutout_circles([
                            ([0.0, 24.0], 7.0, 8.0),
                            ([48.0, 24.0], 6.0, 7.0),
                            ([96.0, 24.0], 7.0, 8.0),
                        ]),
                )
        },
        (96.0, 48.0),
        rune::Env::new(true, 1.0),
    );
    rt.frame(0.0);
    check(&mut gpu, &from_instances(96, 48, &rt.instances), 2);
}
