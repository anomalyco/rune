use super::warm_frame;
use crate::render::{Gpu, Instance, KIND_RECT, NO_CLIP, Renderer};

fn rect_instance() -> Instance {
    Instance {
        rect: [0.0, 0.0, 16.0, 16.0],
        radii: [0.0; 4],
        fill: [0.5, 0.25, 0.125, 1.0],
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

/// A pre-warmed `Bgra8Unorm` renderer (`render/warm.rs` `FORMAT`) handed to a window whose swap
/// chain target is `Bgra8UnormSrgb` (`srgb_blend == false`) must switch its pipeline format
/// before `warm_frame` draws, or wgpu rejects the render pass with a validation error; likewise
/// for a parked `Bgra8UnormSrgb` renderer handed to a `Bgra8Unorm` (`srgb_blend == true`) window,
/// and `srgb_blend` must match the window's mode even when the format already agrees.
#[test]
fn warm_frame_rebuilds_renderer_for_target_format() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let instances = [rect_instance()];
    for (initial, target, srgb) in [
        (wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Bgra8UnormSrgb, false),
        (wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Bgra8Unorm, true),
        (wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Bgra8Unorm, true),
        (wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Bgra8UnormSrgb, false),
    ] {
        let mut renderer = Renderer::new(&gpu.device, initial);
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        warm_frame(&gpu, &mut renderer, target, srgb, (32, 32), &instances);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "warm_frame({initial:?} -> {target:?}, srgb={srgb}) hit validation error: {error:?}");
        assert_eq!(renderer.format(), target);
        assert_eq!(renderer.srgb_blend(), srgb);
    }
}
