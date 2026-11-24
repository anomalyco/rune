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

fn make_layered_texture(
    device: &wgpu::Device,
    (width, height): (u32, u32),
    mips: u32,
    layers: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lazy clear test"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: layers },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    })
}

fn read_subresource(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    mip: u32,
    layer: u32,
) -> Vec<u8> {
    let w = (tex.width() >> mip).max(1);
    let h = (tex.height() >> mip).max(1);
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("subresource readback"),
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: mip,
            origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
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

/// Allocates, clears to bright magenta, submits, and drops a texture of `(w, h)` so any
/// subsequent recycled Metal allocation without lazy clearing would likely show non-zero bytes.
fn pollute_gpu_memory(device: &wgpu::Device, queue: &wgpu::Queue, (w, h): (u32, u32)) {
    use wgpu::TextureUsages as U;
    let junk = make_texture(device, (w, h), U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC);
    let view = junk.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 1.0, g: 0.0, b: 1.0, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    queue.submit([enc.finish()]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

#[test]
fn private_render_target_skips_eager_zero_blit_and_texture_binding_only_uses_shared() {
    use wgpu::TextureUsages as U;
    let (device, queue) = gpu();

    let rt = make_texture(&device, (64, 64), U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC);
    let rt_raw = &rt.as_custom::<MetalTexture>().unwrap().raw;
    assert_eq!(rt_raw.raw.storageMode(), MTLStorageMode::Private, "render target stays Private");
    assert!(rt_raw.is_lazy(), "2D color Private RENDER_ATTACHMENT uses lazy render-pass clearing");
    assert!(rt_raw.lazy_init.as_ref().unwrap().has_uninit());

    queue.submit([]);
    assert_eq!(last(&device), 0, "creating Private RENDER_ATTACHMENT queues no eager zero_texture upload");

    if apple_silicon(&device) {
        // TEXTURE_BINDING-only dummy (no COPY_DST, like backdrop_blur's 1x1 stand-in) uses Shared
        // + CPU zeroing with uses: None.
        let dummy = make_texture(&device, (1, 1), U::TEXTURE_BINDING);
        let dummy_raw = &dummy.as_custom::<MetalTexture>().unwrap().raw;
        assert_eq!(dummy_raw.raw.storageMode(), MTLStorageMode::Shared, "TEXTURE_BINDING-only uses Shared on Apple Silicon");
        assert!(dummy_raw.uses.is_none(), "read-only Shared texture tracks no in-flight uses");
        assert!(!dummy_raw.is_lazy());
        queue.submit([]);
        assert_eq!(last(&device), 0, "TEXTURE_BINDING-only Shared texture queues no blit");

        let harness = SamplerHarness::new(&device);
        let group = harness.bind(&device, &dummy);
        let target = make_texture(&device, (8, 8), U::RENDER_ATTACHMENT | U::COPY_SRC);
        let view = target.create_view(&Default::default());
        let mut enc = device.create_command_encoder(&Default::default());
        harness.draw_into(&mut enc, &view, &group, true, None, 1);
        queue.submit([enc.finish()]);
        let px = read_texture(&device, &queue, &target);
        assert!(px.iter().all(|&b| b == 0), "CPU-zeroed TEXTURE_BINDING-only dummy samples as zeros");
    }
}

#[test]
#[allow(unsafe_code)]
fn lazy_clear_render_pass_load_and_store_ops() {
    use wgpu::TextureUsages as U;
    let (device, queue) = gpu();
    pollute_gpu_memory(&device, &queue, (32, 32));

    // 1. LoadOp::Load on uninitialized texture promotes to Clear(0,0,0,0), then subsequent
    // LoadOp::Load after a Store preserves the stored color.
    let t_load = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let v_load = t_load.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &v_load,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
    }
    queue.submit([enc.finish()]);
    assert!(read_texture(&device, &queue, &t_load).iter().all(|&b| b == 0), "uninitialized LoadOp::Load clears to zero");
    assert!(!t_load.as_custom::<MetalTexture>().unwrap().raw.lazy_init.as_ref().unwrap().has_uninit());

    // Store solid RED, then open another LoadOp::Load pass: RED must survive.
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &v_load,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &v_load,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
    }
    queue.submit([enc.finish()]);
    let px = read_texture(&device, &queue, &t_load);
    assert_eq!(&px[0..4], &[255, 0, 0, 255], "initialized LoadOp::Load must not re-clear");

    // 2. LoadOp::DontCare on uninitialized texture promotes to Clear(0,0,0,0).
    let t_dc = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let v_dc = t_dc.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        // SAFETY: this test explicitly verifies `LoadOp::DontCare` on an uninitialized texture.
        let load = wgpu::LoadOp::DontCare(unsafe { wgpu::LoadOpDontCare::enabled() });
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &v_dc,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
    }
    queue.submit([enc.finish()]);
    assert!(read_texture(&device, &queue, &t_dc).iter().all(|&b| b == 0), "uninitialized LoadOp::DontCare clears to zero");

    // 3. StoreOp::Discard leaves an uninitialized texture uninitialized, so a later LoadOp::Load
    // still promotes to Clear(0,0,0,0).
    let t_disc = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let v_disc = t_disc.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &v_disc,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            ..Default::default()
        });
    }
    assert!(t_disc.as_custom::<MetalTexture>().unwrap().raw.lazy_init.as_ref().unwrap().has_uninit(), "Discard keeps subresource uninitialized");
    queue.submit([enc.finish()]);
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &v_disc,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
    }
    queue.submit([enc.finish()]);
    assert!(read_texture(&device, &queue, &t_disc).iter().all(|&b| b == 0), "after Discard, LoadOp::Load clears to zero");
}

#[test]
fn lazy_clear_before_sample_compute_copy_and_partial_write() {
    use wgpu::TextureUsages as U;
    let (device, queue) = gpu();
    pollute_gpu_memory(&device, &queue, (32, 32));
    let harness = SamplerHarness::new(&device);

    // 1. Sampling an uninitialized Private RENDER_ATTACHMENT in a render pass (set_bind_group).
    let uninit_tex = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
    let target = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let target_view = target.create_view(&Default::default());
    let group = harness.bind(&device, &uninit_tex);
    let mut enc = device.create_command_encoder(&Default::default());
    harness.draw_into(&mut enc, &target_view, &group, true, None, 1);
    queue.submit([enc.finish()]);
    assert!(read_texture(&device, &queue, &target).iter().all(|&b| b == 0), "sampling uninitialized render target reads zeros");

    // 2. Reading an uninitialized Private RENDER_ATTACHMENT in a compute pass (set_bind_group).
    let comp_tex = make_texture(&device, (16, 16), U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
    let comp_view = comp_tex.create_view(&Default::default());
    let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("compute out"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("compute readback"),
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let cs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("read tex"),
        source: wgpu::ShaderSource::Wgsl(r#"
            @group(0) @binding(0) var t: texture_2d<f32>;
            @group(0) @binding(1) var<storage, read_write> out: vec4<f32>;
            @compute @workgroup_size(1) fn main() {
                out = textureLoad(t, vec2<i32>(7, 9), 0);
            }
        "#.into()),
    });
    let cp = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("read tex"),
        layout: None,
        module: &cs,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let cbg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &cp.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&comp_view) },
            wgpu::BindGroupEntry { binding: 1, resource: out_buf.as_entire_binding() },
        ],
    });
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(&cp);
        pass.set_bind_group(0, &cbg, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    enc.copy_buffer_to_buffer(&out_buf, 0, &read_buf, 0, 16);
    queue.submit([enc.finish()]);
    let slice = read_buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let floats: [f32; 4] = *bytemuck::from_bytes(&slice.get_mapped_range().unwrap());
    read_buf.unmap();
    assert_eq!(floats, [0.0; 4], "compute textureLoad on uninitialized render target reads zeros");

    // 3. copy_texture_to_texture: uninitialized src, and partial dst.
    let copy_src = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let copy_dst = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_DST | U::COPY_SRC);
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_texture(copy_src.as_image_copy(), copy_dst.as_image_copy(), copy_src.size());
    queue.submit([enc.finish()]);
    assert!(read_texture(&device, &queue, &copy_dst).iter().all(|&b| b == 0), "copy_texture_to_texture from uninitialized src reads zeros");

    let red_src = make_texture(&device, (4, 4), U::RENDER_ATTACHMENT | U::COPY_SRC);
    let red_view = red_src.create_view(&Default::default());
    let partial_dst = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_DST | U::COPY_SRC);
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &red_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
    }
    enc.copy_texture_to_texture(red_src.as_image_copy(), partial_dst.as_image_copy(), red_src.size());
    queue.submit([enc.finish()]);
    let px = read_texture(&device, &queue, &partial_dst);
    assert_eq!(&px[0..4], &[255, 0, 0, 255], "partial copy_texture_to_texture wrote patch");
    assert_eq!(&px[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 4], &[0, 0, 0, 0], "outside partial patch reads zero");

    // 4. copy_buffer_to_texture (partial dst) and queue.write_texture (partial dst).
    let partial_buf_dst = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_DST | U::COPY_SRC);
    let green_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("green patch"),
        size: 256 * 4,
        usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut padded = vec![0u8; 256 * 4];
    for y in 0..4 {
        for x in 0..4 {
            padded[y * 256 + x * 4..y * 256 + x * 4 + 4].copy_from_slice(&[0, 255, 0, 255]);
        }
    }
    queue.write_buffer(&green_buf, 0, &padded);
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_texture(
        wgpu::TexelCopyBufferInfo {
            buffer: &green_buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(4) },
        },
        partial_buf_dst.as_image_copy(),
        wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let px = read_texture(&device, &queue, &partial_buf_dst);
    assert_eq!(&px[0..4], &[0, 255, 0, 255], "partial copy_buffer_to_texture wrote patch");
    assert_eq!(&px[(20 * 32 + 20) * 4..(20 * 32 + 20) * 4 + 4], &[0, 0, 0, 0], "outside partial buffer copy reads zero");

    let partial_q_dst = make_texture(&device, (32, 32), U::RENDER_ATTACHMENT | U::COPY_DST | U::COPY_SRC);
    let yellow = fill_rgba(4, 4, [255, 255, 0, 255]);
    queue.write_texture(
        partial_q_dst.as_image_copy(),
        &yellow,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16), rows_per_image: None },
        wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
    );
    queue.submit([]);
    let px = read_texture(&device, &queue, &partial_q_dst);
    assert_eq!(&px[0..4], &[255, 255, 0, 255], "partial queue.write_texture wrote patch");
    assert_eq!(&px[(24 * 32 + 24) * 4..(24 * 32 + 24) * 4 + 4], &[0, 0, 0, 0], "outside partial write_texture reads zero");

    // 5. Multi-mip, multi-layer isolation: drawing into (mip=0, layer=0) leaves (mip=1, layer=0)
    // and (mip=0, layer=1) uninitialized so reading them clears them to zero without clobbering (0,0).
    let layered = make_layered_texture(&device, (32, 32), 2, 2, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC);
    let m0l0 = layered.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_mip_level: 0, mip_level_count: Some(1),
        base_array_layer: 0, array_layer_count: Some(1),
        ..Default::default()
    });
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &m0l0,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
    }
    queue.submit([enc.finish()]);
    assert!(layered.as_custom::<MetalTexture>().unwrap().raw.lazy_init.as_ref().unwrap().has_uninit());
    assert!(read_subresource(&device, &queue, &layered, 1, 0).iter().all(|&b| b == 0), "(mip=1, layer=0) reads zero");
    assert!(read_subresource(&device, &queue, &layered, 0, 1).iter().all(|&b| b == 0), "(mip=0, layer=1) reads zero");
    let blue = read_subresource(&device, &queue, &layered, 0, 0);
    assert_eq!(&blue[0..4], &[0, 0, 255, 255], "(mip=0, layer=0) kept its rendered blue");
}
