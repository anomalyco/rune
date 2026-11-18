use rune::color::{srgb_decode, srgb_encode};
use rune::render::{Gpu, Offscreen};
use rune_fx::*;
use rune_ref::blend::{Separable, composite};

fn render(
    gpu: &Gpu,
    fx: &mut FxRenderer,
    format: wgpu::TextureFormat,
    clear: [f32; 4],
    layers: &[FxLayer],
    mode: FxBlend,
    srgb: bool,
    backdrop: Option<&wgpu::TextureView>,
) -> Vec<u8> {
    let target = Offscreen::with_format(&gpu.device, 96, 96, format);
    fx.prepare_with_slots(
        &gpu.device,
        &gpu.queue,
        &FxFrame {
            viewport: [96, 96],
            scale: 1.0,
            layers,
            backdrop,
            sources: &[],
        },
        &[FxSlot::default().blend(mode).backdrop_srgb(srgb)],
    );
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0] as f64,
                        g: clear[1] as f64,
                        b: clear[2] as f64,
                        a: clear[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        fx.draw_all(&mut pass, layers, None);
    }
    target.copy(&mut enc);
    gpu.queue.submit([enc.finish()]);
    target.read(&gpu.device)
}

#[test]
fn soft_glow_coverage_matches_reference_blending_in_both_spaces() {
    let gpu = Gpu::headless();
    let modes = [
        (FxBlend::Multiply, Separable::Multiply),
        (FxBlend::Screen, Separable::Screen),
        (FxBlend::Overlay, Separable::Overlay),
        (FxBlend::SoftLight, Separable::SoftLight),
    ];
    for srgb in [false, true] {
        let format = if srgb {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };
        let mut fx = FxRenderer::new(&gpu.device, format, FxOptions::default());
        fx.set_srgb_blend(srgb);
        let decode = |p: &[u8]| -> [f32; 4] {
            std::array::from_fn(|c| {
                let v = p[c] as f32 / 255.0;
                if c < 3 && !srgb { srgb_decode(v) } else { v }
            })
        };
        for opacity in [0.35, 1.0] {
            let layers = [Fx::glow(GlowParams {
                color: rgb(0xc85a14),
                core: rgb(0xe88c28),
                radius: 10.0,
                reach: 2.5,
                intensity: 0.8,
                occlusion: 1.0,
                ..Default::default()
            })
            .on(Shape::rounded(32.0, 32.0, 32.0, 32.0, 10.0))
            .at(0.5)
            .opacity(opacity)];
            // Normal with occlusion=1 yields the same premultiplied source as separable modes.
            let source = render(
                &gpu,
                &mut fx,
                format,
                [0.0; 4],
                &layers,
                FxBlend::Normal,
                srgb,
                None,
            );
            for (rgb, alpha) in [
                ([0.98, 0.97, 0.95], 1.0),
                ([0.3, 0.6, 0.8], 1.0),
                ([0.3, 0.6, 0.8], 0.35),
                ([0.0; 3], 0.0),
            ] {
                let clear = std::array::from_fn(|c| {
                    if c == 3 {
                        alpha
                    } else {
                        (if srgb { rgb[c] } else { srgb_decode(rgb[c]) }) * alpha
                    }
                });
                let backdrop = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 96,
                        height: 96,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let backdrop_view = backdrop.create_view(&Default::default());
                let bytes = render(
                    &gpu,
                    &mut fx,
                    format,
                    clear,
                    &[],
                    FxBlend::Normal,
                    srgb,
                    None,
                );
                // Upload exactly the quantized pixels used by the destination.
                gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &backdrop,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(96 * 4),
                        rows_per_image: Some(96),
                    },
                    wgpu::Extent3d {
                        width: 96,
                        height: 96,
                        depth_or_array_layers: 1,
                    },
                );
                for (mode, oracle) in modes {
                    let actual = render(
                        &gpu,
                        &mut fx,
                        format,
                        clear,
                        &layers,
                        mode,
                        srgb,
                        Some(&backdrop_view),
                    );
                    let mut partial = 0;
                    for ((src, dst), got) in source
                        .chunks_exact(4)
                        .zip(bytes.chunks_exact(4))
                        .zip(actual.chunks_exact(4))
                    {
                        let src = decode(src);
                        partial += usize::from(src[3] > 0.02 && src[3] < 0.3);
                        let want = composite(oracle, src, decode(dst));
                        for c in 0..4 {
                            let v = if c < 3 && !srgb {
                                srgb_encode(want[c])
                            } else {
                                want[c]
                            };
                            let want = (v * 255.0).round().clamp(0.0, 255.0) as u8;
                            assert!(
                                got[c].abs_diff(want) <= 3,
                                "{mode:?} srgb={srgb} opacity={opacity} backdrop alpha={alpha} channel={c}: {} vs {want}",
                                got[c]
                            );
                        }
                    }
                    assert!(
                        partial > 100,
                        "exercise soft edges, not just opaque centres"
                    );
                }
            }
        }
    }
}
