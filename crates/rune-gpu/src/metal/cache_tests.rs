#![allow(unsafe_code)]

use std::sync::atomic::Ordering;

use super::cache::PipelineCache;
use super::instance;

fn device() -> (wgpu::Device, wgpu::Queue) {
    let adapter = pollster::block_on(instance().request_adapter(&Default::default())).unwrap();
    assert!(adapter.features().contains(wgpu::Features::PIPELINE_CACHE));
    let desc = wgpu::DeviceDescriptor { required_features: wgpu::Features::PIPELINE_CACHE, ..Default::default() };
    pollster::block_on(adapter.request_device(&desc)).unwrap()
}

fn cache(device: &wgpu::Device, data: Option<&[u8]>) -> wgpu::PipelineCache {
    // SAFETY: the backend checks the data's header and Metal checks the archive.
    unsafe { device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor { label: None, data, fallback: true }) }
}

fn misses(cache: &wgpu::PipelineCache) -> u64 {
    cache.as_custom::<PipelineCache>().unwrap().misses.load(Ordering::Relaxed)
}

const SHADERS: &str = r#"
@group(0) @binding(0) var<storage, read_write> v: array<u32>;
@compute @workgroup_size(4)
fn bump(@builtin(global_invocation_id) id: vec3<u32>) { v[id.x] = v[id.x] * 3u + 1u; }

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32(i & 1u), f32(i >> 1u)) * 4.0 - 1.0;
    return vec4<f32>(p, 0.0, 1.0);
}
@fragment
fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.25, 0.5, 0.75, 1.0); }
"#;

/// Makes a compute and a render pipeline with `cache` and runs both: what the dispatch wrote and
/// the pixel the draw filled.
fn run(device: &wgpu::Device, queue: &wgpu::Queue, cache: Option<&wgpu::PipelineCache>) -> (Vec<u32>, Vec<u8>) {
    use wgpu::BufferUsages as B;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("cached"), source: wgpu::ShaderSource::Wgsl(SHADERS.into()) });
    let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("bump"), layout: None, module: &module, entry_point: Some("bump"), compilation_options: Default::default(), cache,
    });
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fill"), layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
            targets: &[Some(format.into())] }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache,
    });
    let values = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: B::STORAGE | B::COPY_DST | B::COPY_SRC, mapped_at_creation: false });
    queue.write_buffer(&values, 0, bytemuck::cast_slice(&[1u32, 2, 3, 4]));
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &compute.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: values.as_entire_binding() }] });
    let target = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 64, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&compute);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            ..Default::default()
        });
        pass.set_pipeline(&render);
        pass.draw(0..3, 0..1);
    }
    let (out, pixels) = (device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: B::COPY_DST | B::MAP_READ, mapped_at_creation: false }),
        device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256, usage: B::COPY_DST | B::MAP_READ, mapped_at_creation: false }));
    encoder.copy_buffer_to_buffer(&values, 0, &out, 0, 16);
    encoder.copy_texture_to_buffer(target.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &pixels,
        layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: None } }, target.size());
    queue.submit([encoder.finish()]);
    let read = |b: &wgpu::Buffer| {
        b.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        b.slice(..).get_mapped_range().unwrap().to_vec()
    };
    (bytemuck::pod_collect_to_vec(&read(&out)), read(&pixels)[..4].to_vec())
}

const WANT: ([u32; 4], [u8; 4]) = ([4, 7, 10, 13], [64, 128, 191, 255]);

#[test]
fn a_saved_cache_makes_working_pipelines_without_compiling_them() {
    let (device, queue) = device();
    let first = cache(&device, None);
    assert_eq!(run(&device, &queue, Some(&first)), (WANT.0.to_vec(), WANT.1.to_vec()));
    assert_eq!(misses(&first), 2, "an empty cache compiles both pipelines into its archive");
    let data = first.get_data().expect("the archive serializes");
    // As on a later launch: another device, from the saved bytes.
    let (device, queue) = self::device();
    let again = cache(&device, Some(&data));
    assert_eq!(run(&device, &queue, Some(&again)), (WANT.0.to_vec(), WANT.1.to_vec()));
    assert_eq!(misses(&again), 0, "both pipelines came from the archive");
    assert_eq!(again.get_data().as_deref(), Some(&data[..]), "nothing new to save");
}

#[test]
fn stale_or_foreign_data_recompiles_instead_of_failing() {
    let (device, queue) = device();
    let good = cache(&device, None);
    run(&device, &queue, Some(&good));
    let good = good.get_data().unwrap();
    let mut other_gpu = good.clone();
    other_gpu[8] ^= 0xff;
    let mut broken_archive = good.clone();
    let end = broken_archive.len();
    broken_archive.truncate(40 + (end - 40) / 2);
    for data in [b"not a cache".to_vec(), Vec::new(), other_gpu, broken_archive] {
        let stale = cache(&device, Some(&data));
        assert_eq!(run(&device, &queue, Some(&stale)), (WANT.0.to_vec(), WANT.1.to_vec()));
        assert_eq!(misses(&stale), 2);
        let fresh = stale.get_data().expect("a fresh archive to save");
        assert_eq!(fresh[..40], good[..40], "for this GPU and OS build");
    }
}

#[test]
fn threads_share_one_cache() {
    let (device, queue) = device();
    let shared = cache(&device, None);
    std::thread::scope(|s| {
        for _ in 0..4 { s.spawn(|| assert_eq!(run(&device, &queue, Some(&shared)).0, WANT.0)); }
    });
    let data = shared.get_data().unwrap();
    let again = cache(&device, Some(&data));
    run(&device, &queue, Some(&again));
    assert_eq!(misses(&again), 0);
}

#[test]
fn wgpus_metal_backend_has_no_pipeline_cache() {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::METAL;
    let adapter = pollster::block_on(wgpu::Instance::new(descriptor).request_adapter(&Default::default())).unwrap();
    assert!(!adapter.features().contains(wgpu::Features::PIPELINE_CACHE), "apps check the feature, so this backend runs without one");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    assert_eq!(run(&device, &queue, None).0, WANT.0);
}
