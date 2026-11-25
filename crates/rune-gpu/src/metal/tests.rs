use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::bench::{clear, gpu};
use super::resource::MetalBuffer;
use super::*;

const W: u32 = 64;
const H: u32 = 32;

fn shared(device: &wgpu::Device) -> Arc<Shared> { device.as_custom::<MetalDevice>().unwrap().shared.clone() }

fn last(device: &wgpu::Device) -> u64 { shared(device).state.lock().last }

fn wgpu_metal() -> (wgpu::Device, wgpu::Queue) {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::METAL;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    // The iOS simulator passes fewer inter-stage variables than wgpu's defaults ask for.
    let desc = wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() };
    pollster::block_on(adapter.request_device(&desc)).unwrap()
}

fn texture(device: &wgpu::Device, format: wgpu::TextureFormat, (width, height): (u32, u32), usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
    })
}

fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage, mapped_at_creation: false })
}

fn copy_out(encoder: &mut wgpu::CommandEncoder, from: &wgpu::Texture, to: &wgpu::Buffer) {
    encoder.copy_texture_to_buffer(from.as_image_copy(), wgpu::TexelCopyBufferInfo {
        buffer: to, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(H) },
    }, from.size());
}

fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let out = slice.get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    out
}

const SCENE: &str = r#"
struct U { color: vec4<f32>, offset: vec2<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> weights: array<f32>;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) tint: vec4<f32> };

@vertex fn vs(@builtin(vertex_index) vi: u32, @location(0) rect: vec4<f32>, @location(1) tint: vec4<f32>) -> VOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let xy = rect.xy + corner * rect.zw + u.offset;
    var o: VOut;
    o.pos = vec4<f32>(xy.x * 2.0 - 1.0, 1.0 - xy.y * 2.0, 0.0, 1.0);
    o.uv = corner;
    o.tint = tint;
    return o;
}

struct FOut { @location(0) color: vec4<f32>, @location(1) mask: vec4<f32> };

@fragment fn fs(i: VOut) -> FOut {
    let n = arrayLength(&weights);
    var o: FOut;
    o.color = textureSample(tex, samp, i.uv) * i.tint * u.color * weights[n - 1u];
    o.mask = vec4<f32>(f32(n) / 8.0, 0.0, 0.0, 1.0);
    return o;
}
"#;

/// A frame using most of what Rune's renderers do: two targets, instanced vertex buffers, a
/// dynamic uniform offset, a runtime-sized storage array, a sampled texture written through the
/// queue, blending, a scissor, and buffer and texture copies on the way out. Returns the color
/// target (through a texture-to-texture copy) and the mask target, as read back.
fn scene(device: &wgpu::Device, queue: &wgpu::Queue) -> (Vec<u8>, Vec<u8>) {
    use wgpu::{BufferUsages as B, ShaderStages as S, TextureUsages as T};
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("scene"), source: wgpu::ShaderSource::Wgsl(SCENE.into()) });
    let uniform = |dynamic| wgpu::BindGroupLayoutEntry { binding: 0, visibility: S::VERTEX_FRAGMENT, count: None,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: dynamic, min_binding_size: None } };
    let globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[
        uniform(true),
        wgpu::BindGroupLayoutEntry { binding: 1, visibility: S::FRAGMENT, count: None,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None } },
    ] });
    let images = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[
        wgpu::BindGroupLayoutEntry { binding: 0, visibility: S::FRAGMENT, count: None, ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false } },
        wgpu::BindGroupLayoutEntry { binding: 1, visibility: S::FRAGMENT, count: None, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering) },
    ] });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&globals), Some(&images)], immediate_size: 0 });
    let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scene"),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Instance, attributes: &attributes })] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ] }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let uniforms = buffer(device, 512, B::UNIFORM | B::COPY_DST);
    let records: [[f32; 8]; 2] = [[0.0; 8], [1.0, 0.8, 0.6, 0.9, 0.05, 0.1, 0.0, 0.0]];
    queue.write_buffer(&uniforms, 0, bytemuck::cast_slice(&records[0]));
    queue.write_buffer(&uniforms, 256, bytemuck::cast_slice(&records[1]));
    // Staged through a copy, so the shader sees the second write's weights.
    let weights = buffer(device, 32, B::STORAGE | B::COPY_DST);
    let staging = buffer(device, 32, B::COPY_SRC | B::COPY_DST);
    queue.write_buffer(&staging, 0, bytemuck::cast_slice(&[0.1f32, 0.2, 0.3, 0.75, 0.0, 0.0, 0.0, 0.0]));
    let instances = buffer(device, 64, B::VERTEX | B::COPY_DST);
    let rects: [[f32; 8]; 2] = [[0.1, 0.1, 0.5, 0.6, 1.0, 1.0, 1.0, 1.0], [0.3, 0.4, 0.6, 0.5, 0.2, 0.9, 0.4, 0.7]];
    queue.write_buffer(&instances, 0, bytemuck::cast_slice(&rects));
    let image = texture(device, wgpu::TextureFormat::Rgba8Unorm, (2, 2), T::TEXTURE_BINDING | T::COPY_DST);
    let texels: [u8; 16] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128];
    queue.write_texture(image.as_image_copy(), &texels, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8), rows_per_image: None }, image.size());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
    let image_view = image.create_view(&Default::default());
    let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &globals, entries: &[
        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &uniforms, offset: 0, size: wgpu::BufferSize::new(32) }) },
        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &weights, offset: 0, size: wgpu::BufferSize::new(16) }) },
    ] });
    let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &images, entries: &[
        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&image_view) },
        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
    ] });
    let color = texture(device, wgpu::TextureFormat::Rgba8Unorm, (W, H), T::RENDER_ATTACHMENT | T::COPY_SRC);
    let mask = texture(device, wgpu::TextureFormat::R8Unorm, (W, H), T::RENDER_ATTACHMENT | T::COPY_SRC);
    let copy = texture(device, wgpu::TextureFormat::Rgba8Unorm, (W, H), T::COPY_DST | T::COPY_SRC);
    let (color_view, mask_view) = (color.create_view(&Default::default()), mask.create_view(&Default::default()));
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&staging, 0, &weights, 0, 32);
    {
        let attachment = |view, load| Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[attachment(&color_view, wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 })),
                attachment(&mask_view, wgpu::LoadOp::Clear(wgpu::Color::BLACK))],
            ..Default::default()
        });
        // Groups before the pipeline: they land at the slots it reads.
        pass.set_bind_group(1, &group1, &[]);
        pass.set_bind_group(0, &group0, &[256]);
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, instances.slice(..));
        pass.set_scissor_rect(4, 2, W - 8, H - 4);
        pass.draw(0..4, 0..2);
    }
    encoder.copy_texture_to_texture(color.as_image_copy(), copy.as_image_copy(), color.size());
    let (color_out, mask_out) = (buffer(device, 256 * u64::from(H), B::COPY_DST | B::MAP_READ), buffer(device, 256 * u64::from(H), B::COPY_DST | B::MAP_READ));
    copy_out(&mut encoder, &copy, &color_out);
    copy_out(&mut encoder, &mask, &mask_out);
    queue.submit([encoder.finish()]);
    (read(device, &color_out), read(device, &mask_out))
}

#[test]
fn a_scene_matches_wgpus_pixel_for_pixel() {
    let (device, queue) = gpu();
    let (color, mask) = scene(&device, &queue);
    let (wgpu_device, wgpu_queue) = wgpu_metal();
    let (want_color, want_mask) = scene(&wgpu_device, &wgpu_queue);
    let at = |x: u32, y: u32| (y * 256 + x * 4) as usize;
    assert_ne!(color[at(20, 12)..at(20, 12) + 4], [26, 51, 77, 255], "the quads drew");
    assert_eq!(&color[at(0, 0)..at(0, 0) + 4], &[26, 51, 77, 255], "the scissor kept the clear");
    assert_eq!(mask[(12 * 256 + 20) as usize], 128, "the shader saw four weights");
    assert!(color == want_color, "color differs from wgpu's at {:?}", color.iter().zip(&want_color).position(|(a, b)| a != b));
    assert!(mask == want_mask, "mask differs from wgpu's at {:?}", mask.iter().zip(&want_mask).position(|(a, b)| a != b));
}

const IMMEDIATE_SCENE: &str = r#"
struct I { offset: vec2<f32>, scale: f32, alpha: f32, tint: vec4<f32> };
var<immediate> i: I;
@group(0) @binding(0) var<uniform> base: vec4<f32>;

@vertex fn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let xy = i.offset + corner * i.scale;
    return vec4<f32>(xy.x * 2.0 - 1.0, 1.0 - xy.y * 2.0, 0.0, 1.0);
}

@fragment fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(base.rgb * i.tint.rgb, i.alpha);
}
"#;

/// Two quads whose immediates both stages read beside a bind group (so the group's slots sit
/// after the immediates'), the second changing only part of them. Returns the target, read back.
fn immediate_scene(device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
    use wgpu::{BufferUsages as B, ShaderStages as S, TextureUsages as T};
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("immediates"), source: wgpu::ShaderSource::Wgsl(IMMEDIATE_SCENE.into()) });
    let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[wgpu::BindGroupLayoutEntry {
        binding: 0, visibility: S::FRAGMENT, count: None,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
    }] });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&group_layout)], immediate_size: 32 });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("immediates"),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL }),
        ] }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let uniform = buffer(device, 16, B::UNIFORM | B::COPY_DST);
    queue.write_buffer(&uniform, 0, bytemuck::cast_slice(&[1.0f32, 0.8, 0.5, 1.0]));
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &group_layout, entries: &[
        wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
    ] });
    let target = texture(device, wgpu::TextureFormat::Rgba8Unorm, (W, H), T::RENDER_ATTACHMENT | T::COPY_SRC);
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 }), store: wgpu::StoreOp::Store } })],
            ..Default::default()
        });
        // The group before the pipeline: it lands after the immediates' slot.
        pass.set_bind_group(0, &group, &[]);
        pass.set_pipeline(&pipeline);
        pass.set_immediates(0, bytemuck::cast_slice(&[0.1f32, 0.1, 0.4, 0.9, 0.2, 1.0, 0.6, 1.0]));
        pass.draw(0..4, 0..1);
        // Only the offset and the tint's first channel change.
        pass.set_immediates(0, bytemuck::cast_slice(&[0.5f32, 0.4]));
        pass.set_immediates(16, bytemuck::cast_slice(&[0.9f32]));
        pass.draw(0..4, 0..1);
    }
    let out = buffer(device, 256 * u64::from(H), B::COPY_DST | B::MAP_READ);
    copy_out(&mut encoder, &target, &out);
    queue.submit([encoder.finish()]);
    read(device, &out)
}

#[test]
fn immediates_match_wgpus_pixel_for_pixel() {
    let adapter = pollster::block_on(instance().request_adapter(&Default::default())).unwrap();
    assert!(adapter.features().contains(wgpu::Features::IMMEDIATES));
    assert_eq!(adapter.limits().max_immediate_size, 4096, "what setVertexBytes takes");
    let (device, queue) = gpu();
    let pixels = immediate_scene(&device, &queue);
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::METAL;
    let theirs = pollster::block_on(wgpu::Instance::new(descriptor).request_adapter(&Default::default())).unwrap();
    let desc = wgpu::DeviceDescriptor { required_features: wgpu::Features::IMMEDIATES, required_limits: theirs.limits(), ..Default::default() };
    let (wgpu_device, wgpu_queue) = pollster::block_on(theirs.request_device(&desc)).unwrap();
    let want = immediate_scene(&wgpu_device, &wgpu_queue);
    let at = |x: u32, y: u32| (y * 256 + x * 4) as usize;
    assert_ne!(pixels[at(10, 6)..at(10, 6) + 4], [26, 51, 77, 255], "the first quad drew");
    assert_ne!(pixels[at(40, 20)..at(40, 20) + 4], pixels[at(10, 6)..at(10, 6) + 4], "the second, in another tint");
    assert!(pixels == want, "differs from wgpu's at {:?}", pixels.iter().zip(&want).position(|(a, b)| a != b));
}

#[test]
#[allow(unsafe_code)]
fn a_frame_is_one_command_buffer_and_writes_land() {
    let (device, queue) = gpu();
    let target = texture(&device, wgpu::TextureFormat::Bgra8UnormSrgb, (64, 64), wgpu::TextureUsages::RENDER_ATTACHMENT);
    queue.submit([]);
    assert_eq!(last(&device), 0, "a lazy-cleared render target queues no eager zeroing upload");
    let empty = device.create_command_encoder(&Default::default());
    queue.submit([empty.finish()]);
    assert_eq!(last(&device), 0, "an empty encoder commits nothing");
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    drop(clear(&mut encoder, &view));
    queue.submit([encoder.finish()]);
    assert_eq!(last(&device), 1);
    let staged = buffer(&device, 16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
    queue.write_buffer(&staged, 4, &[1, 2, 3, 4]);
    queue.submit([]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = staged.as_custom::<MetalBuffer>().unwrap().get_mapped_range(0..16).unwrap();
    // SAFETY: the GPU finished with the buffer.
    assert_eq!(unsafe { &mapped.read_slice()[4..8] }, &[1, 2, 3, 4]);
}

#[test]
fn a_map_waits_for_the_work_before_it() {
    let (device, queue) = gpu();
    let source = texture(&device, wgpu::TextureFormat::Rgba8Unorm, (W, H), wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let out = buffer(&device, 256 * u64::from(H), wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
    let view = source.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    drop(clear(&mut encoder, &view));
    copy_out(&mut encoder, &source, &out);
    let index = queue.submit([encoder.finish()]);
    let mapped = Arc::new(AtomicBool::new(false));
    let flag = mapped.clone();
    out.slice(..).map_async(wgpu::MapMode::Read, move |r| { if r.is_ok() { flag.store(true, Ordering::SeqCst) } });
    assert!(!mapped.load(Ordering::SeqCst), "callbacks wait for a poll after their work");
    device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).unwrap();
    assert!(mapped.load(Ordering::SeqCst), "the callback runs once the work completed");
    assert_eq!(&out.slice(..).get_mapped_range().unwrap()[..4], &[0, 0, 51, 102], "the copy landed before the map");
}

#[test]
fn what_a_buffer_used_lives_until_it_completes() {
    let (device, queue) = gpu();
    let target = texture(&device, wgpu::TextureFormat::Rgba8Unorm, (W, H), wgpu::TextureUsages::RENDER_ATTACHMENT);
    let view = target.create_view(&Default::default());
    let raw = view.as_custom::<super::resource::MetalView>().unwrap().raw.clone();
    let mut encoder = device.create_command_encoder(&Default::default());
    drop(clear(&mut encoder, &view));
    let recorded = encoder.finish();
    drop((view, target));
    assert!(Arc::strong_count(&raw) > 1, "the recorded buffer holds the view");
    let index = queue.submit([recorded]);
    device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).unwrap();
    assert_eq!(Arc::strong_count(&raw), 1, "retired once the GPU finished");
}

/// Unretained command buffers require every referenced Metal object to stay alive until Metal
/// marks the command buffer completed — not merely until its GPU-side shared event signals.
#[test]
fn resources_stay_alive_until_command_buffer_completes() {
    use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLDevice, MTLEvent, MTLSharedEvent};
    let completed = |cb: &Obj<dyn MTLCommandBuffer>| {
        matches!(cb.status(), MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error)
    };
    let (device, queue) = gpu();
    let sh = shared(&device);
    // Gate the queue so both the zeroing upload CB and the render CB stay in flight while we
    // grab handles to them and drop the caller's own texture and view handles.
    let gate = sh.device.newSharedEvent().unwrap();
    let wait = sh.command_buffer();
    wait.encodeWaitForEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&*gate), 1);
    wait.commit();

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lifetime target"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 2 },
        mip_level_count: 2,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor {
        label: Some("lifetime view"),
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(1),
        ..Default::default()
    });
    let tex_weak = Arc::downgrade(&target.as_custom::<super::resource::MetalTexture>().unwrap().raw);
    let view_weak = Arc::downgrade(&view.as_custom::<super::resource::MetalView>().unwrap().raw);

    let mut encoder = device.create_command_encoder(&Default::default());
    drop(clear(&mut encoder, &view));
    let recorded = encoder.finish();
    drop((view, target));
    let index = queue.submit([recorded]);

    let pending = sh.pending_cbs();
    assert_eq!(pending.len(), 1, "lazy-cleared render target tracks only the render command buffer");
    let (render_val, render_cb) = (pending[0].0, pending[0].1.clone());
    assert!(tex_weak.strong_count() > 0, "render CB keeps the texture alive while gated");
    assert!(view_weak.strong_count() > 0, "render CB keeps the view alive while gated");

    gate.setSignaledValue(1);
    // Spin through the window where the GPU event has signaled `render_val` before Metal marks the
    // command buffer completed: neither resource may drop before the command buffer completes.
    while sh.event.signaledValue() < render_val {
        std::hint::spin_loop();
    }
    for _ in 0..256 {
        sh.retire();
        if tex_weak.strong_count() == 0 || view_weak.strong_count() == 0 {
            assert!(completed(&render_cb), "resource dropped before render CB reached Completed ({:?})", render_cb.status());
            break;
        }
        std::hint::spin_loop();
    }
    device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).unwrap();
    assert!(completed(&render_cb), "poll(Wait) left render CB uncompleted: {:?}", render_cb.status());
    assert_eq!(tex_weak.strong_count(), 0, "texture retired once render CB completed");
    assert_eq!(view_weak.strong_count(), 0, "view retired once render CB completed");
    wait.waitUntilCompleted();
}

/// Metal ends a dropped buffer's open passes when it frees the buffer, which reads their
/// attachments; a thread's exit used to free it after its attachments were gone.
#[test]
fn an_encoder_dropped_mid_frame_frees_its_buffer_before_its_targets() {
    let (device, queue) = gpu();
    let raws = std::thread::spawn({
        let device = device.clone();
        move || {
            let target = texture(&device, wgpu::TextureFormat::Rgba8Unorm, (W, H), wgpu::TextureUsages::RENDER_ATTACHMENT);
            let view = target.create_view(&Default::default());
            let raw = view.as_custom::<super::resource::MetalView>().unwrap().raw.clone();
            let mut encoder = device.create_command_encoder(&Default::default());
            drop(clear(&mut encoder, &view));
            drop((encoder, view, target));
            raw
        }
    })
    .join()
    .unwrap();
    assert_eq!(Arc::strong_count(&raws), 1, "a dropped encoder lets go of its targets");
    assert_eq!(last(&device), 0, "and never runs");
    queue.submit([]);
}

#[test]
fn a_steady_frame_reuses_its_keep_lists() {
    let (device, queue) = gpu();
    let target = texture(&device, wgpu::TextureFormat::Rgba8Unorm, (W, H), wgpu::TextureUsages::RENDER_ATTACHMENT);
    queue.submit([]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let view = target.create_view(&Default::default());
    let frame = || {
        let mut encoder = device.create_command_encoder(&Default::default());
        drop(clear(&mut encoder, &view));
        let index = queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).unwrap();
    };
    let spare = || {
        let mut lists: Vec<usize> = shared(&device).state.lock().spare.iter().map(|k| k.as_ptr() as usize).collect();
        lists.sort_unstable();
        lists
    };
    frame();
    let lists = spare();
    assert_eq!(lists.len(), 2, "the encoder's and the pass's");
    frame();
    frame();
    assert_eq!(spare(), lists, "later frames take the same lists back");
}

/// A poll that finds the queue empty has run every callback: the GPU finishing between the two
/// must not leave one waiting for a poll nobody makes.
#[test]
fn an_empty_poll_has_run_every_callback() {
    let (device, queue) = gpu();
    let target = texture(&device, wgpu::TextureFormat::Rgba8Unorm, (W, H), wgpu::TextureUsages::RENDER_ATTACHMENT);
    let view = target.create_view(&Default::default());
    for frame in 0..300 {
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        drop(clear(&mut encoder, &view));
        encoder.on_submitted_work_done(move || flag.store(true, Ordering::SeqCst));
        queue.submit([encoder.finish()]);
        while !matches!(device.poll(wgpu::PollType::Poll).unwrap(), wgpu::PollStatus::QueueEmpty) {}
        assert!(ran.load(Ordering::SeqCst), "frame {frame}: the queue read empty before its callback ran");
    }
}

#[test]
fn adapters_are_the_ones_wgpu_picks() {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::METAL;
    let ours = instance();
    let theirs = wgpu::Instance::new(descriptor);
    for power_preference in [wgpu::PowerPreference::LowPower, wgpu::PowerPreference::HighPerformance] {
        let options = wgpu::RequestAdapterOptions { power_preference, ..Default::default() };
        let name = |instance: &wgpu::Instance| pollster::block_on(instance.request_adapter(&options)).unwrap().get_info().name;
        assert_eq!(name(&ours), name(&theirs), "{power_preference:?}");
    }
    let count = |instance: &wgpu::Instance| pollster::block_on(instance.enumerate_adapters(wgpu::Backends::METAL)).len();
    assert_eq!(count(&ours), count(&theirs));
}

/// Any command buffer of a submission that fails loses a stock wgpu Metal device (the vendored
/// wgpu-hal), not only the last: here the internal one that waits for an event nothing signals,
/// which Metal times out after about 5 s, while the submission's own work completes.
#[test]
#[allow(unsafe_code)]
fn a_failed_command_buffer_loses_a_stock_wgpu_metal_device() {
    use objc2_metal::MTLDevice;
    let (device, queue) = wgpu_metal();
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = heard.clone();
    device.set_device_lost_callback(move |_, message| into.lock().unwrap().push(message));
    // SAFETY: the hal device and queue are only read, to make an event and stage a wait on it.
    let event = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }.and_then(|d| d.raw_device().newSharedEvent()).expect("a Metal shared event");
    // SAFETY: the hal queue is only read to stage a wait on `event`, which stays alive below.
    unsafe { queue.as_hal::<wgpu::hal::api::Metal>() }.expect("a Metal queue").add_wait_event(event.clone(), 1);
    queue.submit([device.create_command_encoder(&Default::default()).finish()]);
    let deadline = rune_core::time::now() + std::time::Duration::from_secs(20);
    while heard.lock().unwrap().is_empty() && rune_core::time::now() < deadline {
        // wgpu 30 panics in `Device::poll` once the hal reports the device lost (gfx-rs/wgpu#10031).
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.poll(wgpu::PollType::Poll))).ok();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(heard.lock().unwrap().len(), 1, "the failed wait loses the device");
    drop(event);
}

#[test]
fn a_lost_device_tells_its_callback_once_even_one_set_after() {
    let (device, _queue) = gpu();
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    shared(&device).lose("the GPU timed out".into());
    let into = heard.clone();
    device.set_device_lost_callback(move |reason, message| into.lock().unwrap().push((reason, message)));
    shared(&device).lose("again".into());
    assert_eq!(*heard.lock().unwrap(), [(wgpu::DeviceLostReason::Unknown, "the GPU timed out".to_owned())]);
}

/// Another queue's signal releases what a Rune encoder records after `wait_for_event`, and its
/// `signal_event_on_finish` comes once that work finished: a copy the other queue then reads.
#[test]
#[allow(unsafe_code)]
fn an_encoder_waits_for_and_signals_events_across_queues() {
    use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue, MTLDevice, MTLEvent, MTLSharedEvent};
    let (device, queue) = gpu();
    let sh = shared(&device);
    let other = sh.device.newCommandQueue().expect("a second Metal queue");
    let (ready, read, seen) = (sh.device.newSharedEvent().unwrap(), sh.device.newSharedEvent().unwrap(), sh.device.newSharedEvent().unwrap());
    fn event(e: &Obj<dyn MTLSharedEvent>) -> &ProtocolObject<dyn MTLEvent> { ProtocolObject::from_ref(&**e) }
    let usage = wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ;
    let src = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage, mapped_at_creation: false });
    let dst = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage, mapped_at_creation: false });
    queue.write_buffer(&src, 0, &[7; 16]);
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(wait_for_event(&mut encoder, event(&ready), 1));
    encoder.copy_buffer_to_buffer(&src, 0, &dst, 0, 16);
    assert!(signal_event_on_finish(&mut encoder, event(&read), 1));
    queue.submit([encoder.finish()]);
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(read.signaledValue(), 0, "held until the other queue signals");
    // The other queue releases it, then waits for its signal before its own work.
    let cb = other.commandBuffer().unwrap();
    cb.encodeSignalEvent_value(event(&ready), 1);
    cb.encodeWaitForEvent_value(event(&read), 1);
    cb.encodeSignalEvent_value(event(&seen), 1);
    cb.commit();
    assert!(seen.waitUntilSignaledValue_timeoutMS(1, 2000), "the encoder's signal came");
    let raw = dst.as_custom::<MetalBuffer>().unwrap().store.current();
    // SAFETY: shared storage of 16 bytes, which the GPU finished writing (`read` reached 1).
    let bytes = unsafe { std::slice::from_raw_parts(raw.raw.contents().as_ptr() as *const u8, 16) };
    assert_eq!(bytes, [7; 16], "the signal came after the copy");
}

/// A stock wgpu encoder isn't this backend's: nothing is encoded.
#[test]
fn events_need_this_backends_encoder() {
    use objc2_metal::{MTLDevice, MTLEvent};
    let (device, _queue) = wgpu_metal();
    let (ours, _q) = gpu();
    let e = shared(&ours).device.newSharedEvent().unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(!wait_for_event(&mut encoder, ProtocolObject::<dyn MTLEvent>::from_ref(&*e), 1));
    assert!(!signal_event_on_finish(&mut encoder, ProtocolObject::<dyn MTLEvent>::from_ref(&*e), 1));
}

/// A command buffer that fails loses the device before anything sees its work retired: a thread
/// that retires it and another that runs the callbacks hear the loss before the work-done callback.
#[test]
fn a_failed_command_buffer_is_heard_lost_before_its_work_is_done() {
    use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLEvent};
    let (device, queue) = gpu();
    let sh = shared(&device);
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = heard.clone();
    device.set_device_lost_callback(move |_, _| into.lock().unwrap().push("lost"));
    // A wait for an event nothing signals: Metal times it out after about 5 s, an error.
    let never = sh.device.newSharedEvent().unwrap();
    let cb = sh.command_buffer();
    cb.encodeWaitForEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&*never), 1);
    sh.commit(cb);
    let into = heard.clone();
    queue.on_submitted_work_done(move || into.lock().unwrap().push("done"));
    let (_, failing) = sh.pending_cbs().pop().expect("tracked");
    failing.waitUntilCompleted();
    assert_eq!(failing.status(), objc2_metal::MTLCommandBufferStatus::Error);
    // One thread takes it off (its loss set in that lock), then another runs the callbacks first.
    assert!(sh.retire_one());
    assert!(sh.lost.lock().message.is_some(), "lost as it retires");
    sh.retire();
    assert_eq!(*heard.lock().unwrap(), ["lost", "done"]);
}

/// Mappings that complete after a loss fail, as WebGPU says (their data can't be right): one asked
/// before the failed work retired and one asked after; work-done callbacks still run, after the loss.
#[test]
fn mappings_fail_once_the_device_is_lost() {
    use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLEvent};
    let (device, queue) = gpu();
    let sh = shared(&device);
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = heard.clone();
    device.set_device_lost_callback(move |_, _| into.lock().unwrap().push("lost".to_owned()));
    let buffer = |label| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: 16, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let (before, after) = (buffer("before"), buffer("after"));
    // A wait for an event nothing signals: Metal times it out after about 5 s, an error.
    let never = sh.device.newSharedEvent().unwrap();
    let cb = sh.command_buffer();
    cb.encodeWaitForEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&*never), 1);
    sh.commit(cb);
    let map = |b: &wgpu::Buffer, name: &'static str| {
        let into = heard.clone();
        b.slice(..).map_async(wgpu::MapMode::Read, move |r| into.lock().unwrap().push(format!("{name} {}", if r.is_ok() { "mapped" } else { "failed" })));
    };
    map(&before, "before");
    let into = heard.clone();
    queue.on_submitted_work_done(move || into.lock().unwrap().push("done".to_owned()));
    let (_, failing) = sh.pending_cbs().pop().expect("tracked");
    failing.waitUntilCompleted();
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    map(&after, "after");
    assert_eq!(*heard.lock().unwrap(), ["lost", "before failed", "done", "after failed"]);
}

/// `Device::destroy` marks the native Metal device lost once with `DeviceLostReason::Destroyed`,
/// fires the device-lost callback (including one registered after `destroy`), and fails buffer
/// mappings as WebGPU specifies.
#[test]
fn destroying_a_device_loses_it_with_destroyed_reason_once_and_fails_mappings() {
    let (device, _queue) = gpu();
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = heard.clone();
    device.set_device_lost_callback(move |reason, message| into.lock().unwrap().push((reason, message)));
    device.destroy();
    device.destroy();
    assert_eq!(
        *heard.lock().unwrap(),
        [(wgpu::DeviceLostReason::Destroyed, "Device was destroyed.".to_owned())]
    );

    let late = Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = late.clone();
    device.set_device_lost_callback(move |reason, message| into.lock().unwrap().push((reason, message)));
    assert_eq!(
        *late.lock().unwrap(),
        [(wgpu::DeviceLostReason::Destroyed, "Device was destroyed.".to_owned())]
    );

    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("after-destroy"),
        size: 16,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mapped = Arc::new(std::sync::Mutex::new(None));
    let out = mapped.clone();
    buf.slice(..).map_async(wgpu::MapMode::Read, move |r| *out.lock().unwrap() = Some(r.is_ok()));
    assert_eq!(*mapped.lock().unwrap(), Some(false), "mappings on a destroyed device must fail");
}

/// Reconfiguring a `MetalSurface` (resize or pixel-format change) must preserve the layer's
/// existing `colorspace` across `setPixelFormat`, keeping both a tagged sRGB layer and an
/// untagged (`None` / unmanaged) layer intact.
#[allow(unsafe_code)]
#[test]
fn metal_surface_configure_preserves_layer_colorspace() {
    use objc2_metal::MTLPixelFormat;
    use objc2_quartz_core::CAMetalLayer;
    let layer = CAMetalLayer::new();
    // Seed the layer with the 8-bit default colorspace (`setPixelFormat` sets it on macOS).
    layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    let tagged = layer.colorspace();
    let instance = super::instance();
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
    // SAFETY: `layer` outlives `surface`.
    let surface = unsafe { instance.create_surface_unsafe(target) }.expect("MetalSurface");
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("MetalAdapter");
    let (device, _queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("MetalDevice");
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8Unorm,
        width: 64,
        height: 32,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: wgpu::SurfaceColorSpace::Auto,
    };

    // Tagged layer keeps its colorspace across size and format changes.
    layer.setColorspace(tagged.as_deref());
    surface.configure(&device, &config);
    assert_eq!(layer.colorspace().is_some(), tagged.is_some());
    config.width = 128;
    config.height = 64;
    config.format = wgpu::TextureFormat::Bgra8UnormSrgb;
    surface.configure(&device, &config);
    assert_eq!(layer.colorspace().is_some(), tagged.is_some());

    // Untagged (Unmanaged) layer stays `None` across size and pixel-format changes.
    layer.setColorspace(None);
    assert!(layer.colorspace().is_none());
    config.width = 96;
    config.height = 48;
    config.format = wgpu::TextureFormat::Bgra8Unorm;
    surface.configure(&device, &config);
    assert!(layer.colorspace().is_none(), "configure must not re-tag an unmanaged layer");
    config.format = wgpu::TextureFormat::Bgra8UnormSrgb;
    surface.configure(&device, &config);
    assert!(layer.colorspace().is_none(), "sRGB format configure must not re-tag an unmanaged layer");
    drop(surface);
}
