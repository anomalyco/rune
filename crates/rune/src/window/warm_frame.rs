//! The frame a GPU thread draws once, offscreen, for a window that opened on the CPU, before
//! the window goes to the GPU (`win32::handover`, `linux::handover`): what the GPU's first frame
//! would make on the app's thread (the pipelines its layers' formats need, its layers' targets,
//! the staging space, the instance buffer) is made there instead.

use crate::render::{Gpu, Renderer};

/// On the GPU thread, before the hand-over: the renderer draws the window's latest CPU frame once,
/// into a target of the swap chain's format, and waits for the GPU. Its first frame's costs are
/// paid here, so the hand-over frame on the app's thread costs what a frame does: the pipelines
/// its layers' formats need, its layers' targets (the hand-over frame's layers are the latest
/// frame's, or nearly), the staging space and the instance buffer. Its own target goes as soon as
/// the GPU is done with it. The atlas is an empty one: the glyphs land with the hand-over frame's
/// own prepare.
pub(in crate::window) fn warm_frame(gpu: &Gpu, renderer: &mut Renderer, format: wgpu::TextureFormat, srgb: bool, size: (u32, u32), instances: &[crate::render::Instance]) {
    renderer.set_format(&gpu.device, format);
    renderer.set_srgb_blend(srgb);
    debug_assert_eq!(renderer.format(), format);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rune warm frame"),
        size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut atlas = crate::text::Atlas::new();
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.prepare_ui(&gpu.device, &gpu.queue, &mut atlas, instances, size);
    renderer.draw(&mut enc, &view, [0.0; 4]);
    renderer.submit(&gpu.queue, enc);
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
    target.destroy();
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
}

/// A CPU frame's instances as the warm frame draws them: what reads the app's own state (an
/// image's slot, a glyph's atlas page, a compositor's effect) draws as a plain box, so the frame
/// needs nothing but the renderer.
pub(in crate::window) fn warm_instances(instances: &[crate::render::Instance]) -> Vec<crate::render::Instance> {
    use crate::render::{KIND_RECT, Kind};
    instances
        .iter()
        .map(|i| {
            let mut i = *i;
            match i.kind() {
                Some(Kind::Image | Kind::Effect) => i.params[1] = KIND_RECT,
                Some(Kind::Glyph) => i.params[2] = 0.0,
                _ => {}
            }
            i
        })
        .collect()
}
