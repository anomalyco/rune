//! The simulated fire's compute steps, on Rune's GPU (`RUNE_GPU_BACKEND` picks its backend) and
//! on wgpu's own, frame for frame.

use rune::render::Gpu;
use rune_fx::*;

const W: u32 = 64;
const H: u32 = 96;

/// Three frames of a simulated fire, each one's f16 pixels.
fn frames(device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<Vec<u16>> {
    let fmt = wgpu::TextureFormat::Rgba16Float;
    let mut fx = FxRenderer::new(device, fmt, FxOptions { dither: false });
    fx.set_fire_sim(device, true);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(W * 8 * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    (1..=3)
        .map(|n| {
            let layers = [Fx::fire(FireParams::default()).on(Shape::rounded(8.0, 60.0, 48.0, 24.0, 6.0)).at(n as f32 * 0.05)];
            fx.prepare(device, queue, &FxFrame { viewport: [W, H], scale: 1.0, layers: &layers, backdrop: None, sources: &[] });
            let mut encoder = device.create_command_encoder(&Default::default());
            assert!(fx.simulate(queue, &mut encoder, None), "frame {n} stepped the simulation");
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                });
                fx.draw_layer(&mut pass, 0);
            }
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 8), rows_per_image: Some(H) },
                },
                target.size(),
            );
            queue.submit([encoder.finish()]);
            let slice = readback.slice(..);
            slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let words = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
            readback.unmap();
            words
        })
        .collect()
}

#[test]
fn simulated_fire_matches_wgpus_frame_for_frame() {
    let gpu = Gpu::headless();
    let got = frames(&gpu.device, &gpu.queue);
    let wgpu = Gpu::new(wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()), None);
    let want = frames(&wgpu.device, &wgpu.queue);
    for (n, (got, want)) in got.iter().zip(&want).enumerate() {
        assert!(got.iter().all(|w| w & 0x7c00 != 0x7c00), "frame {n} is finite");
        let lit = got.chunks(4).filter(|p| p[3] != 0).count();
        assert!(lit > (W * H / 20) as usize, "frame {n} drew a fire ({lit} pixels lit)");
        assert!(got == want, "frame {n} differs from wgpu's at {:?}", got.iter().zip(want).position(|(a, b)| a != b));
    }
    assert!(got[0] != got[2], "the simulation moved the flame");
}
