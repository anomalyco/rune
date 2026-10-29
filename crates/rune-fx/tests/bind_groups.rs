//! A steady frame reuses last frame's bind groups instead of making new ones.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use rune::render::Gpu;
use rune_fx::*;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn backdrop(gpu: &Gpu) -> wgpu::TextureView {
    gpu.device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 96, height: 96, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn frame(gpu: &Gpu, fx: &mut FxRenderer, layers: &[FxLayer], backdrop: &wgpu::TextureView, slot_backdrop: Option<&wgpu::TextureView>) -> Vec<Option<wgpu::BindGroup>> {
    fx.prepare(&gpu.device, &gpu.queue, &FxFrame { viewport: [96, 96], scale: 1.0, layers, backdrop: Some(backdrop), sources: &[] });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    if let Some(b) = slot_backdrop {
        fx.blur_backdrop(&gpu.device, &gpu.queue, &mut enc, b, 1);
    }
    gpu.queue.submit([enc.finish()]);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    (0..layers.len()).map(|i| fx.layer_bind_group(i).cloned()).collect()
}

#[test]
fn a_steady_frame_reuses_its_bind_groups() {
    let gpu = Gpu::headless();
    let mut fx = FxRenderer::new(&gpu.device, FORMAT, FxOptions::default());
    let bd = backdrop(&gpu);
    let slot = backdrop(&gpu);
    let layers = [
        Fx::glow(GlowParams { radius: 10.0, intensity: 0.8, ..Default::default() }).on(Shape::rounded(16.0, 16.0, 32.0, 32.0, 8.0)).at(0.5),
        Fx::lens(LensParams { frost: 0.6, ..Default::default() }).on(Shape::rounded(40.0, 40.0, 40.0, 40.0, 12.0)).at(0.5),
    ];
    let first = frame(&gpu, &mut fx, &layers, &bd, None);
    let second = frame(&gpu, &mut fx, &layers, &bd, None);
    assert!(first.iter().all(Option::is_some));
    assert_eq!(first, second, "the frame backdrop's groups were made again");

    // A per-slot backdrop bound after `prepare` is reused too.
    let first = frame(&gpu, &mut fx, &layers, &bd, Some(&slot));
    let second = frame(&gpu, &mut fx, &layers, &bd, Some(&slot));
    assert_eq!(first, second, "the slot backdrop's group was made again");

    // A different backdrop gets its own group.
    let other = backdrop(&gpu);
    let third = frame(&gpu, &mut fx, &layers, &other, None);
    assert_eq!(third[0], second[0]);
    assert_ne!(third[1], second[1]);
}
