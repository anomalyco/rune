//! Builds every pipeline Rune, rune-fx and rune-light ship, once, timing each step in
//! milliseconds.

use std::time::Instant;

fn ms<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed().as_secs_f64() * 1e3)
}

pub fn build_every_pipeline(gpu: &rune::render::Gpu) -> Vec<(&'static str, f64)> {
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let mut out = Vec::new();
    let (renderer, t) = ms(|| rune::render::Renderer::new(&gpu.device, format));
    out.push(("renderer", t));
    out.extend(renderer.first_use_ms(&gpu.device));
    let (mut fx, t) = ms(|| rune_fx::FxRenderer::new(&gpu.device, format, Default::default()));
    out.push(("fx", t));
    out.push(("fx_layer_format", ms(|| fx.add_format(&gpu.device, wgpu::TextureFormat::Rgba16Float)).1));
    out.push(("fx_every_pipeline", ms(|| fx.build_every_pipeline()).1));
    out.push(("fx_fire_sim", ms(|| fx.set_fire_sim(&gpu.device, true)).1));
    out.push(("light", ms(|| drop(rune_light::LightRenderer::new(&gpu.device, &gpu.queue, 256, 256, format))).1));
    out.push(("glass", ms(|| drop(rune_light::GlassRenderer::new(&gpu.device, 256, 256, format))).1));
    out
}
