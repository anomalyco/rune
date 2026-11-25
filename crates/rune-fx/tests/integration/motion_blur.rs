//! Motion blur rendering parity: boxing `MotionSamples` keeps packing and rendered pixels
//! bit-identical, and updating boxed samples in place matches building a fresh `Fx::motion_blur`.

use rune::render::{Gpu, Offscreen};
use rune_fx::*;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const W: u32 = 160;
const H: u32 = 96;

fn make_card_source(gpu: &Gpu, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("motion-blur source"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    // Bright orange rounded card inside (48..96, 24..72) on transparent background.
    for y in 24..72u32 {
        for x in 48..96u32 {
            let at = ((y * w + x) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&[255, 136, 40, 255]);
        }
    }
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let view = tex.create_view(&Default::default());
    (tex, view)
}

fn render_motion(gpu: &Gpu, fx: &mut FxRenderer, src: &wgpu::TextureView, layer: &FxLayer) -> Vec<u8> {
    let target = Offscreen::with_format(&gpu.device, W, H, FORMAT);
    let stats = fx.prepare(
        &gpu.device,
        &gpu.queue,
        &FxFrame {
            viewport: [W, H],
            scale: 1.0,
            layers: std::slice::from_ref(layer),
            backdrop: None,
            sources: &[src],
        },
    );
    assert_eq!(stats.drawn, 1);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        fx.draw_layer(&mut pass, 0);
    }
    target.copy(&mut enc);
    gpu.queue.submit([enc.finish()]);
    target.read(&gpu.device)
}

#[test]
fn motion_blur_shutter_and_directional_render_trails_consistently() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (_tex, src) = make_card_source(&gpu, W, H);
    let mut fx = FxRenderer::new(&gpu.device, FORMAT, FxOptions::default());
    let rest = Rect::new(48.0, 24.0, 48.0, 48.0);
    let src_rect = Rect::new(0.0, 0.0, W as f32, H as f32);
    let track = |t: f32| Pose { x: (t - 0.1) * 1800.0, y: 0.0, rotation: 0.0, scale: 1.0, opacity: 1.0 };

    for mode in [BlurMode::Shutter, BlurMode::Directional] {
        let p = MotionBlurParams { mode, shutter_angle: 180.0, ..Default::default() };
        let s = MotionSamples::from_track(&p, rest, 0.1, 1.0, track);
        assert!(s.is_moving() && s.count > 4);
        let layer = Fx::motion_blur(p, s)
            .on(Shape::rounded(rest.x, rest.y, rest.w, rest.h, 8.0))
            .source(0, src_rect)
            .at(0.1);
        let pixels = render_motion(&gpu, &mut fx, &src, &layer);
        let lit = pixels.as_chunks::<4>().0.iter().filter(|c| c[3] > 0).count();
        let rest_area = (48 * 48) as usize;
        assert!(lit > rest_area, "{mode:?} trail must extend beyond rest rect ({lit} <= {rest_area})");

        // Reusing a persistent layer's boxed MotionSamples in place renders bit-identically.
        let s0 = MotionSamples::from_track(&p, rest, 0.0, 1.0, |_| Pose::default());
        let mut reused = Fx::motion_blur(p, s0)
            .on(Shape::rounded(rest.x, rest.y, rest.w, rest.h, 8.0))
            .source(0, src_rect)
            .at(0.1);
        if let Fx::MotionBlur(_, samples) = &mut reused.fx {
            **samples = s;
        }
        let pixels_reused = render_motion(&gpu, &mut fx, &src, &reused);
        assert_eq!(pixels, pixels_reused, "{mode:?} in-place boxed sample update must render identically");
    }
}
