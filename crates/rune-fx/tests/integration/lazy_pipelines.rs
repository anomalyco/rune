//! A renderer builds an effect's pipeline only when a frame draws that effect, in that blend,
//! into that format, so one that only glows never pays for the other families.

use rune::render::{Gpu, Offscreen};
use rune_fx::*;

fn draw(gpu: &Gpu, fx: &mut FxRenderer, format: wgpu::TextureFormat, layers: &[FxLayer], slots: &[FxSlot]) {
    let target = Offscreen::with_format(&gpu.device, 64, 64, format);
    fx.prepare_with_slots(&gpu.device, &gpu.queue, &FxFrame { viewport: [64, 64], scale: 1.0, layers, backdrop: None, sources: &[] }, slots);
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
        for i in 0..layers.len() {
            fx.draw_layer_in(&mut pass, format, [0.0; 2], i);
        }
    }
    gpu.queue.submit([encoder.finish()]);
}

#[test]
fn pipelines_are_built_for_what_frames_draw() {
    let gpu = Gpu::headless();
    let window = wgpu::TextureFormat::Rgba8UnormSrgb;
    let layer = wgpu::TextureFormat::Rgba16Float;
    let mut fx = FxRenderer::new(&gpu.device, window, FxOptions::default());
    fx.add_format(&gpu.device, layer);
    assert_eq!(fx.built_pipelines(), 0, "nothing is built before a frame draws");

    let shape = Shape::rounded(16.0, 16.0, 32.0, 32.0, 6.0);
    let glow = Fx::glow(GlowParams::default()).on(shape);
    draw(&gpu, &mut fx, window, &[glow], &[]);
    assert_eq!(fx.built_pipelines(), 1, "a glow builds the glow");
    draw(&gpu, &mut fx, window, &[glow], &[]);
    assert_eq!(fx.built_pipelines(), 1, "and keeps it");

    let fire = Fx::fire(FireParams::default()).on(shape);
    draw(&gpu, &mut fx, window, &[glow, fire, glow], &[FxSlot::default(), FxSlot::default(), FxSlot::default().blend(FxBlend::Screen)]);
    assert_eq!(fx.built_pipelines(), 3, "a fire and a screen-blended glow build one each");

    draw(&gpu, &mut fx, layer, &[glow], &[]);
    assert_eq!(fx.built_pipelines(), 4, "another format builds its own when drawn into");

    fx.build_every_pipeline();
    assert_eq!(fx.built_pipelines(), 32, "eight families, two blends, two formats");
}
