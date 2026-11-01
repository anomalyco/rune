//! What one glow costs the first time a process draws it, in milliseconds, as one JSON line:
//! `FxRenderer::new`, `add_format` for an isolated layer's `Rgba16Float`, and the first frame
//! (prepare, draw, submit, read back), each timed on its own, with `total` the three together.
//! Run it in fresh processes, `--profile perf`, after clearing the Metal shader cache
//! (`$(getconf DARWIN_USER_CACHE_DIR)/com.apple.metal`) for a cold run.

use rune::render::{Gpu, Offscreen};
use rune_fx::*;
use std::time::Instant;

fn main() {
    let gpu = Gpu::headless();
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let target = Offscreen::with_format(&gpu.device, 96, 96, format);
    let layers = [Fx::glow(GlowParams::default()).on(Shape::rounded(24.0, 24.0, 48.0, 48.0, 8.0))];
    let start = Instant::now();
    let mut fx = FxRenderer::new(&gpu.device, format, FxOptions::default());
    let new = start.elapsed();
    fx.add_format(&gpu.device, wgpu::TextureFormat::Rgba16Float);
    let add_format = start.elapsed() - new;
    fx.prepare(&gpu.device, &gpu.queue, &FxFrame { viewport: [96, 96], scale: 1.0, layers: &layers, backdrop: None, sources: &[] });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        fx.draw_layer(&mut pass, 0);
    }
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    let pixels = target.read(&gpu.device);
    let total = start.elapsed();
    assert!(pixels.chunks(4).any(|p| p[3] != 0), "the glow drew");
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
    println!(
        "{{\"new\":{:.3},\"add_format\":{:.3},\"first_frame\":{:.3},\"total\":{:.3}}}",
        ms(new),
        ms(add_format),
        ms(total - new - add_format),
        ms(total)
    );
}
