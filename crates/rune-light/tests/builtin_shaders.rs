//! Every pipeline Rune, rune-fx and rune-light ship finds its function in Rune's precompiled
//! Metal library under `RUNE_GPU_BACKEND=metal`. A miss means a shader or its layout changed:
//! run `scripts/precompile-metal-shaders.sh` on a Mac, which runs this test with
//! `RUNE_METAL_CAPTURE` set to capture them.

#[path = "support/builtin.rs"]
mod builtin;

#[test]
fn every_builtin_pipeline_is_precompiled() {
    let Some(gpu) = rune::render::Gpu::test_headless() else { return };
    builtin::build_every_pipeline(&gpu);
    if std::env::var_os("RUNE_METAL_CAPTURE").is_some() { return }
    let Some(misses) = rune::render::precompiled_misses(&gpu.device) else { return };
    assert_eq!(misses, 0, "{misses} built-in pipelines compiled at runtime; run scripts/precompile-metal-shaders.sh");
}

fn gi_section(src: &str, name: &str) -> String {
    let mut parts = src.split("// ---- ");
    let header = parts.next().unwrap_or_default();
    let body = parts.find(|p| p.starts_with(name)).unwrap_or_default();
    format!("{header}\n// ---- {body}")
}

fn glass_section(src: &str, name: &str) -> String {
    let (main, pyr) = src.split_once("// ---- pyramid ----").unwrap_or((src, ""));
    if name == "pyramid" {
        format!(
            "@vertex\nfn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {{\n    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));\n    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);\n}}\n{pyr}"
        )
    } else {
        main.to_string()
    }
}

/// Cross-platform, GPU-free freshness guard: runs on Windows, Linux and macOS and fails as soon
/// as any built-in WGSL shader in `rune`, `rune-media`, `rune-gpu`, `rune-fx` or `rune-light`
/// drifts from `crates/rune-gpu/src/metal/builtin_table.rs`.
#[test]
fn every_builtin_shader_source_matches_precompiled_table() {
    if std::env::var_os("RUNE_METAL_CAPTURE").is_some() {
        return;
    }

    for (name, bytes) in [
        ("builtin_native.metallib", include_bytes!("../../rune-gpu/src/shaders/builtin_native.metallib").as_slice()),
        ("builtin_native_ios.metallib", include_bytes!("../../rune-gpu/src/shaders/builtin_native_ios.metallib").as_slice()),
        ("builtin_native_ios_sim.metallib", include_bytes!("../../rune-gpu/src/shaders/builtin_native_ios_sim.metallib").as_slice()),
    ] {
        assert!(
            bytes.starts_with(b"MTLB") && bytes.len() > 1024,
            "crates/rune-gpu/src/shaders/{name} must be a valid Metal library binary"
        );
    }

    let table = include_str!("../../rune-gpu/src/metal/builtin_table.rs");
    let sources_block = table
        .split("const SOURCES:")
        .nth(1)
        .and_then(|s| s.split("];").next())
        .expect("builtin_table.rs SOURCES");
    let recorded: Vec<&str> = sources_block.split('"').skip(1).step_by(2).collect();
    assert!(!recorded.is_empty(), "builtin_table.rs SOURCES must not be empty");

    let clip = include_str!("../../rune/src/shaders/clip.wgsl");
    let shader = include_str!("../../rune/src/shaders/shader.wgsl");
    let springs = include_str!("../../rune-gpu/src/springs.wgsl");
    let fx_common = include_str!("../../rune-fx/src/shaders/common.wgsl");
    let light_common = include_str!("../src/shaders/common.wgsl");
    let gi = include_str!("../src/shaders/gi.wgsl");
    let glass = include_str!("../src/shaders/glass.wgsl");

    let mut shaders: Vec<(&str, String)> = vec![
        // rune + rune-media + rune-gpu
        ("rune/uber (classic)", format!("{clip}\n{shader}")),
        ("rune/backdrop_blur.wgsl", include_str!("../../rune/src/shaders/backdrop_blur.wgsl").to_owned()),
        ("rune/backdrop_composite.wgsl", format!("{clip}\n{}", include_str!("../../rune/src/shaders/backdrop_composite.wgsl"))),
        ("rune/layer_blur.wgsl", include_str!("../../rune/src/shaders/layer_blur.wgsl").to_owned()),
        ("rune-media/video.wgsl", include_str!("../../rune-media/src/native_surface/video.wgsl").to_owned()),
        ("rune/surface_alpha.wgsl", include_str!("../../rune/src/shaders/surface_alpha.wgsl").to_owned()),
        ("rune/uber+springs", format!("{clip}\n{shader}\n{springs}")),
        // rune-fx
        ("rune-fx/glow.wgsl", format!("{clip}\n{fx_common}\n{}", include_str!("../../rune-fx/src/shaders/glow.wgsl"))),
        ("rune-fx/pulse.wgsl", format!("{clip}\n{fx_common}\n{}", include_str!("../../rune-fx/src/shaders/pulse.wgsl"))),
        ("rune-fx/electric.wgsl", format!("{clip}\n{fx_common}\n{}", include_str!("../../rune-fx/src/shaders/electric.wgsl"))),
        ("rune-fx/fire.wgsl", format!("{clip}\n{fx_common}\n{}", include_str!("../../rune-fx/src/shaders/fire.wgsl"))),
        ("rune-fx/distort.wgsl", format!("{clip}\n{fx_common}\n{}", include_str!("../../rune-fx/src/shaders/distort.wgsl"))),
        ("rune-fx/motion.wgsl", format!("{clip}\n{fx_common}\n{}", include_str!("../../rune-fx/src/shaders/motion.wgsl"))),
        ("rune-fx/extras.wgsl", format!("{clip}\n{fx_common}\n{}", include_str!("../../rune-fx/src/shaders/extras.wgsl"))),
        ("rune-fx/fill.wgsl", format!("{clip}\n{fx_common}\n{}", include_str!("../../rune-fx/src/shaders/fill.wgsl"))),
        ("rune-fx/mip.wgsl", include_str!("../../rune-fx/src/shaders/mip.wgsl").to_owned()),
        ("rune-fx/blur.wgsl", include_str!("../../rune-fx/src/shaders/blur.wgsl").to_owned()),
        ("rune-fx/firesim.wgsl", include_str!("../../rune-fx/src/shaders/firesim.wgsl").to_owned()),
        // rune-light
        ("rune-light/gbuffer.wgsl", format!("{light_common}\n{}", include_str!("../src/shaders/gbuffer.wgsl"))),
        ("rune-light/field.wgsl", format!("{light_common}\n{}", include_str!("../src/shaders/field.wgsl"))),
        ("rune-light/shadow.wgsl", format!("{light_common}\n{}", include_str!("../src/shaders/shadow.wgsl"))),
        ("rune-light/gi.wgsl (scene)", format!("{light_common}\n{}", gi_section(gi, "scene"))),
        ("rune-light/gi.wgsl (cascades)", format!("{light_common}\n{}", gi_section(gi, "cascades"))),
        ("rune-light/gi.wgsl (fluence)", format!("{light_common}\n{}", gi_section(gi, "fluence"))),
        ("rune-light/composite.wgsl", format!("{light_common}\n{}", include_str!("../src/shaders/composite.wgsl"))),
        ("rune-light/noop", format!("{light_common}\n@compute @workgroup_size(1) fn noop() {{}}")),
        ("rune-light/glass.wgsl (pyramid)", glass_section(glass, "pyramid")),
        ("rune-light/glass.wgsl (glass)", glass_section(glass, "glass")),
    ];

    let mut missing = Vec::new();
    let mut computed: Vec<String> = Vec::with_capacity(shaders.len());
    for (name, wgsl) in shaders.drain(..) {
        let hash = rune::render::builtin_wgsl_hash(&wgsl);
        if recorded.binary_search(&hash.as_str()).is_err() {
            missing.push(format!("{name} ({hash})"));
        }
        computed.push(hash);
    }
    assert!(
        missing.is_empty(),
        "precompiled Metal built-in shader table (crates/rune-gpu/src/metal/builtin_table.rs) is stale for: {}; run scripts/precompile-metal-shaders.sh on a Mac",
        missing.join(", ")
    );

    computed.sort();
    computed.dedup();
    let expected: Vec<&str> = computed.iter().map(String::as_str).collect();
    assert_eq!(
        recorded, expected,
        "crates/rune-gpu/src/metal/builtin_table.rs SOURCES differs from built-in shader set; run scripts/precompile-metal-shaders.sh on a Mac"
    );
}
