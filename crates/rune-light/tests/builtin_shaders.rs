//! Every pipeline Rune, rune-fx and rune-light ship finds its function in Rune's precompiled
//! Metal library under `RUNE_GPU_BACKEND=metal`. A miss means a shader or its layout changed:
//! run `scripts/precompile-metal-shaders.sh`, which runs this test with `RUNE_METAL_CAPTURE`
//! set to capture them.

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
