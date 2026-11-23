use std::sync::atomic::Ordering;

use objc2_metal::{MTLDevice, MTLGPUFamily, MTLResource, MTLStorageMode};

use super::bench::gpu;
use super::resource::MetalTexture;
use super::*;

const SAMPLE_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    let x = f32((vi & 1u) * 2u) - 1.0;
    let y = 1.0 - f32(((vi >> 1u) & 1u) * 2u);
    var o: VOut;
    o.pos = vec4<f32>(x, y, 0.0, 1.0);
    o.uv = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    return o;
}

@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv);
}
"#;

fn shared_of(device: &wgpu::Device) -> std::sync::Arc<Shared> {
    device.as_custom::<MetalDevice>().unwrap().shared.clone()
}

fn last(device: &wgpu::Device) -> u64 {
    shared_of(device).state.lock().last
}

fn apple_silicon(device: &wgpu::Device) -> bool {
    shared_of(device).device.supportsFamily(MTLGPUFamily::Apple1)
}

fn make_texture(
    device: &wgpu::Device,
    (width, height): (u32, u32),
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shared texture test"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    })
}

struct SamplerHarness {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl SamplerHarness {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sample"),
            source: wgpu::ShaderSource::Wgsl(SAMPLE_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sample"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self { pipeline, layout, sampler }
    }

    fn bind(&self, device: &wgpu::Device, tex: &wgpu::Texture) -> wgpu::BindGroup {
        let view = tex.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    fn draw_into(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        group: &wgpu::BindGroup,
        clear: bool,
        scissor: Option<(u32, u32, u32, u32)>,
        instances: u32,
    ) {
        let load = if clear {
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
        } else {
            wgpu::LoadOp::Load
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        if let Some((x, y, w, h)) = scissor {
            pass.set_scissor_rect(x, y, w, h);
        }
        pass.draw(0..4, 0..instances);
    }
}

fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
) -> Vec<u8> {
    let w = tex.width();
    let h = tex.height();
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
        },
        tex.size(),
    );
    queue.submit([encoder.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let start = y * row as usize;
        out.extend_from_slice(&mapped[start..start + (w * 4) as usize]);
    }
    drop(mapped);
    buf.unmap();
    out
}

fn fill_rgba(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 4) as usize];
    for c in out.chunks_exact_mut(4) { c.copy_from_slice(&px); }
    out
}

#[test]
fn idle_shared_texture_writes_in_place_and_reads_exact_pixels() {
    use wgpu::TextureUsages as U;
    let (device, queue) = gpu();
    if !apple_silicon(&device) { return; }

    let shared_tex = make_texture(&device, (64, 64), U::TEXTURE_BINDING | U::COPY_DST);
    let mtl = shared_tex.as_custom::<MetalTexture>().unwrap();
    assert_eq!(mtl.raw.raw.storageMode(), MTLStorageMode::Shared, "CPU-written sampled texture uses Shared");
    assert!(mtl.raw.is_shared());

    // Creating a Shared texture zeroes it on the CPU without queueing a zero_texture blit.
    queue.submit([]);
    assert_eq!(last(&device), 0, "new Shared texture must not queue a zero_texture blit");

    // Render targets, storage textures, and COPY_SRC textures stay Private.
    for usage in [
        U::TEXTURE_BINDING | U::COPY_DST | U::RENDER_ATTACHMENT,
        U::TEXTURE_BINDING | U::COPY_DST | U::STORAGE_BINDING,
        U::TEXTURE_BINDING | U::COPY_DST | U::COPY_SRC,
    ] {
        let priv_tex = make_texture(&device, (16, 16), usage);
        let raw = &priv_tex.as_custom::<MetalTexture>().unwrap().raw;
        assert_eq!(raw.raw.storageMode(), MTLStorageMode::Private, "usage {usage:?} stays Private");
        assert!(!raw.is_shared());
    }

    let harness = SamplerHarness::new(&device);
    let target = make_texture(&device, (64, 64), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let target_view = target.create_view(&Default::default());
    let group = harness.bind(&device, &shared_tex);

    // Fresh Shared texture reads as exact zeros.
    let mut encoder = device.create_command_encoder(&Default::default());
    harness.draw_into(&mut encoder, &target_view, &group, true, None, 1);
    queue.submit([encoder.finish()]);
    let zeros = read_texture(&device, &queue, &target);
    assert!(zeros.iter().all(|&b| b == 0), "fresh Shared texture must read as zeros");

    // Idle Staging::write_texture_rows uses replaceRegion with zero chunks and zero staging busy.
    let staging = rune_staging::Staging::new(&device, &queue);
    let top = fill_rgba(64, 32, [220, 40, 20, 255]);
    let bot = fill_rgba(64, 32, [20, 180, 240, 255]);
    staging.write_texture_rows(&shared_tex, 0, &top);
    staging.write_texture_rows(&shared_tex, 32, &bot);
    let stats = staging.stats();
    assert_eq!(stats.direct, 2, "both idle row writes went direct via replaceRegion");
    assert_eq!(stats.chunks, 0, "no staging chunks allocated for direct writes");
    assert!(!staging.busy(), "staging stays idle after direct texture writes");

    let mut encoder = device.create_command_encoder(&Default::default());
    staging.flush(&mut encoder);
    harness.draw_into(&mut encoder, &target_view, &group, true, None, 1);
    queue.submit([encoder.finish()]);
    let pixels = read_texture(&device, &queue, &target);
    let px = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * 64 + x) * 4) as usize;
        pixels[i..i + 4].try_into().unwrap()
    };
    assert_eq!(px(10, 10), [220, 40, 20, 255], "top half written in place");
    assert_eq!(px(10, 48), [20, 180, 240, 255], "bottom half written in place");

    // Idle Queue::write_texture also uses replaceRegion without allocating a staging buffer.
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bufs_before = shared_of(&device).buffers.load(Ordering::Relaxed);
    let green = fill_rgba(64, 64, [10, 230, 50, 255]);
    queue.write_texture(
        shared_tex.as_image_copy(),
        &green,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(64 * 4), rows_per_image: None },
        shared_tex.size(),
    );
    assert_eq!(shared_of(&device).buffers.load(Ordering::Relaxed), bufs_before, "idle write_texture allocates no staging buffer");
    let mut encoder = device.create_command_encoder(&Default::default());
    harness.draw_into(&mut encoder, &target_view, &group, true, None, 1);
    queue.submit([encoder.finish()]);
    let pixels = read_texture(&device, &queue, &target);
    assert_eq!(&pixels[0..4], &[10, 230, 50, 255], "idle Queue::write_texture landed via replaceRegion");
}

#[test]
fn busy_shared_texture_falls_back_to_staged_blit_preserving_order() {
    use wgpu::TextureUsages as U;
    let (device, queue) = gpu();
    if !apple_silicon(&device) { return; }

    let harness = SamplerHarness::new(&device);
    let tex = make_texture(&device, (32, 32), U::TEXTURE_BINDING | U::COPY_DST);
    let group = harness.bind(&device, &tex);
    let target1 = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let target2 = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let view1 = target1.create_view(&Default::default());
    let view2 = target2.create_view(&Default::default());
    let staging = rune_staging::Staging::new(&device, &queue);

    let red = fill_rgba(32, 32, [255, 0, 0, 255]);
    let green = fill_rgba(32, 32, [0, 255, 0, 255]);
    let blue = fill_rgba(32, 32, [0, 0, 255, 255]);

    // 1. Unsubmitted command buffer binds the texture: a subsequent Staging write must fall back
    // to a staged blit so the earlier draw still sees RED and the later draw sees GREEN.
    staging.write_texture_rows(&tex, 0, &red);
    assert_eq!(staging.stats().direct, 1);

    let mut enc1 = device.create_command_encoder(&Default::default());
    harness.draw_into(&mut enc1, &view1, &group, true, None, 1);
    let cb1 = enc1.finish();

    let uses = tex.as_custom::<MetalTexture>().unwrap().raw.uses.as_ref().unwrap().load(Ordering::Acquire);
    assert!(uses > 0, "unsubmitted command buffer must hold a texture usage guard");

    staging.write_texture_rows(&tex, 0, &green);
    assert_eq!(staging.stats().direct, 1, "busy texture must fall back to staged blit");
    assert!(staging.is_pending(), "fallback write must be pending in staging");

    let mut enc2 = device.create_command_encoder(&Default::default());
    staging.flush(&mut enc2);
    harness.draw_into(&mut enc2, &view2, &group, true, None, 1);
    let cb2 = enc2.finish();

    queue.submit([cb1, cb2]);
    let px1 = read_texture(&device, &queue, &target1);
    let px2 = read_texture(&device, &queue, &target2);
    assert_eq!(&px1[0..4], &[255, 0, 0, 255], "earlier draw must still see RED");
    assert_eq!(&px2[0..4], &[0, 255, 0, 255], "later draw must see GREEN");

    // 2. Same-frame pending order: if a write was staged while busy, and the GPU finishes before
    // flush, a second write to the same texture must NOT jump ahead via replaceRegion.
    let mut enc3 = device.create_command_encoder(&Default::default());
    harness.draw_into(&mut enc3, &view1, &group, true, None, 1);
    queue.submit([enc3.finish()]);
    // Stage RED while cb3 is in flight (or hold an unsubmitted encoder to guarantee busy).
    let mut hold = device.create_command_encoder(&Default::default());
    harness.draw_into(&mut hold, &view1, &group, true, None, 1);
    let direct_before = staging.stats().direct;
    staging.write_texture_rows(&tex, 0, &red);
    assert_eq!(staging.stats().direct, direct_before, "staged while hold encoder keeps texture busy");
    // Drop hold and wait for GPU so texture becomes idle while RED is still pending in staging.
    drop(hold);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert_eq!(tex.as_custom::<MetalTexture>().unwrap().raw.uses.as_ref().unwrap().load(Ordering::Acquire), 0);

    staging.write_texture_rows(&tex, 0, &blue);
    assert_eq!(staging.stats().direct, direct_before, "second write must not bypass earlier pending write");
    let mut enc4 = device.create_command_encoder(&Default::default());
    staging.flush(&mut enc4);
    harness.draw_into(&mut enc4, &view2, &group, true, None, 1);
    queue.submit([enc4.finish()]);
    let px_final = read_texture(&device, &queue, &target2);
    assert_eq!(&px_final[0..4], &[0, 0, 255, 255], "BLUE must overwrite RED in FIFO order");

    // 3. Queue::write_texture while an encoder is recorded (unsubmitted): WebGPU orders
    // queue.write_texture before the next submit, so it must stage into Uploads ahead of cb.
    let mut enc5 = device.create_command_encoder(&Default::default());
    harness.draw_into(&mut enc5, &view1, &group, true, None, 1);
    let cb5 = enc5.finish();
    queue.write_texture(
        tex.as_image_copy(),
        &green,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(32 * 4), rows_per_image: None },
        tex.size(),
    );
    queue.submit([cb5]);
    let px5 = read_texture(&device, &queue, &target1);
    assert_eq!(&px5[0..4], &[0, 255, 0, 255], "Queue::write_texture must land before next submit");
}

/// Stress loop: 200 back-to-back GPU-heavy frames sampling one shared texture without waiting
/// between frames. Every tile must render the exact color written for its frame, proving that an
/// in-flight texture is never overwritten in place by CPU replaceRegion.
#[test]
fn inflight_shared_texture_stress_loop_never_writes_in_place_while_gpu_reads() {
    use wgpu::TextureUsages as U;
    let (device, queue) = gpu();
    if !apple_silicon(&device) { return; }

    const FRAMES: u32 = 200;
    const COLS: u32 = 20;
    const TILE: u32 = 16;
    const TARGET_W: u32 = COLS * TILE;
    const TARGET_H: u32 = (FRAMES / COLS) * TILE;

    let harness = SamplerHarness::new(&device);
    let tex = make_texture(&device, (128, 128), U::TEXTURE_BINDING | U::COPY_DST);
    let group = harness.bind(&device, &tex);
    let target = make_texture(&device, (TARGET_W, TARGET_H), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let target_view = target.create_view(&Default::default());
    let staging = rune_staging::Staging::new(&device, &queue);

    let color_for = |i: u32| -> [u8; 4] {
        [((i * 37 + 13) & 255) as u8, ((i * 73 + 59) & 255) as u8, (255 - ((i * 19) & 255)) as u8, 255]
    };

    for i in 0..FRAMES {
        let rows = fill_rgba(128, 128, color_for(i));
        staging.write_texture_rows(&tex, 0, &rows);
        let mut enc = device.create_command_encoder(&Default::default());
        staging.flush(&mut enc);
        let sx = (i % COLS) * TILE;
        let sy = (i / COLS) * TILE;
        // 512 overlapping instances keep the GPU busy long enough that the next iteration's CPU
        // write_texture_rows arrives while this command buffer is still in flight.
        harness.draw_into(&mut enc, &target_view, &group, i == 0, Some((sx, sy, TILE, TILE)), 512);
        queue.submit([enc.finish()]);
    }

    let pixels = read_texture(&device, &queue, &target);
    for i in 0..FRAMES {
        let x = (i % COLS) * TILE + TILE / 2;
        let y = (i / COLS) * TILE + TILE / 2;
        let at = ((y * TARGET_W + x) * 4) as usize;
        let got: [u8; 4] = pixels[at..at + 4].try_into().unwrap();
        let want = color_for(i);
        assert_eq!(got, want, "frame {i}: expected {want:?}, got {got:?} (CPU/GPU race!)");
    }

    let stats = staging.stats();
    assert_eq!(stats.writes, u64::from(FRAMES));
    assert!(
        stats.direct < u64::from(FRAMES),
        "stress loop should exercise the busy-texture blit fallback (direct={} of {FRAMES})",
        stats.direct,
    );
}
