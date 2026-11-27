use super::bench::{clear, gpu};
use super::instance;

fn wgpu_metal() -> (wgpu::Device, wgpu::Queue) {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::METAL;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    // The iOS simulator passes fewer inter-stage variables than wgpu's defaults ask for.
    let desc = wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() };
    pollster::block_on(adapter.request_device(&desc)).unwrap()
}

fn timed() -> (wgpu::Device, wgpu::Queue) {
    let adapter = pollster::block_on(instance().request_adapter(&Default::default())).unwrap();
    assert!(adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY), "Apple GPUs sample at stage boundaries");
    let desc = wgpu::DeviceDescriptor { required_features: wgpu::Features::TIMESTAMP_QUERY, ..Default::default() };
    pollster::block_on(adapter.request_device(&desc)).unwrap()
}

fn texture(device: &wgpu::Device, format: wgpu::TextureFormat, size: wgpu::Extent3d, mips: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None, size, mip_level_count: mips, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
    })
}

fn size(width: u32, height: u32) -> wgpu::Extent3d { wgpu::Extent3d { width, height, depth_or_array_layers: 1 } }

fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage, mapped_at_creation: false })
}

fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let out = slice.get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    out
}

const STEP: &str = r#"
struct U { scale: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> weights: array<f32>;
@group(0) @binding(2) var src: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var<storage, read_write> counts: array<atomic<u32>>;

@compute @workgroup_size(8, 4)
fn step(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = arrayLength(&weights);
    let c = textureLoad(src, vec2<i32>(id.xy), 0) * u.scale * weights[(id.x + id.y) % n];
    textureStore(dst, vec2<i32>(id.xy), c + vec4<f32>(f32(id.x) / 64.0, f32(id.y) / 32.0, 0.0, 0.0));
    atomicAdd(&counts[id.x % 4u], 1u);
}
"#;

const W: u32 = 64;
const H: u32 = 32;

/// Two dispatches in one pass, the second reading what the first stored, through a dynamic
/// uniform offset, a runtime-sized storage array, a storage texture and storage atomics.
fn steps(device: &wgpu::Device, queue: &wgpu::Queue) -> (Vec<u8>, Vec<u8>) {
    use wgpu::{BufferUsages as B, ShaderStages as S, TextureUsages as T};
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("step"), source: wgpu::ShaderSource::Wgsl(STEP.into()) });
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: S::COMPUTE, ty, count: None };
    let storage = |read_only| wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None };
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[
        entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: None }),
        entry(1, storage(true)),
        entry(2, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
        entry(3, wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: wgpu::TextureFormat::Rgba16Float,
            view_dimension: wgpu::TextureViewDimension::D2 }),
        entry(4, storage(false)),
    ] });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("step"), layout: Some(&layout), module: &module, entry_point: Some("step"), compilation_options: Default::default(), cache: None,
    });
    let uniforms = buffer(device, 512, B::UNIFORM | B::COPY_DST);
    queue.write_buffer(&uniforms, 0, bytemuck::cast_slice(&[0.5f32, 1.0, 2.0, 1.0]));
    queue.write_buffer(&uniforms, 256, bytemuck::cast_slice(&[1.5f32, 0.25, 1.0, 0.5]));
    let weights = buffer(device, 64, B::STORAGE | B::COPY_DST);
    queue.write_buffer(&weights, 0, bytemuck::cast_slice(&[1.0f32, 0.5, 0.25]));
    let counts = buffer(device, 16, B::STORAGE | B::COPY_SRC);
    let image = texture(device, wgpu::TextureFormat::Rgba16Float, size(W, H), 1, T::TEXTURE_BINDING | T::COPY_DST);
    let texels: Vec<u16> = (0..W * H * 4).map(|i| half_bits((i % 97) as f32 / 97.0)).collect();
    queue.write_texture(image.as_image_copy(), bytemuck::cast_slice(&texels),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 8), rows_per_image: None }, image.size());
    let mid = texture(device, wgpu::TextureFormat::Rgba16Float, size(W, H), 1, T::TEXTURE_BINDING | T::STORAGE_BINDING);
    let out = texture(device, wgpu::TextureFormat::Rgba16Float, size(W, H), 1, T::STORAGE_BINDING | T::COPY_SRC);
    let view = |t: &wgpu::Texture| t.create_view(&Default::default());
    let (image_view, mid_view, out_view) = (view(&image), view(&mid), view(&out));
    let group = |src, dst| device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &bgl, entries: &[
        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &uniforms, offset: 0, size: wgpu::BufferSize::new(16) }) },
        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &weights, offset: 0, size: wgpu::BufferSize::new(12) }) },
        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(src) },
        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(dst) },
        wgpu::BindGroupEntry { binding: 4, resource: counts.as_entire_binding() },
    ] });
    let (first, second) = (group(&image_view, &mid_view), group(&mid_view, &out_view));
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        // The group before the pipeline: it lands at the slots the pipeline reads.
        pass.set_bind_group(0, &first, &[0]);
        pass.set_pipeline(&pipeline);
        pass.dispatch_workgroups(W / 8, H / 4, 1);
        pass.set_bind_group(0, &second, &[256]);
        pass.dispatch_workgroups(W / 8, H / 4, 1);
    }
    let (pixels, totals) = (buffer(device, u64::from(W * 8 * H), B::COPY_DST | B::MAP_READ), buffer(device, 16, B::COPY_DST | B::MAP_READ));
    encoder.copy_texture_to_buffer(out.as_image_copy(), wgpu::TexelCopyBufferInfo {
        buffer: &pixels, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 8), rows_per_image: Some(H) },
    }, out.size());
    encoder.copy_buffer_to_buffer(&counts, 0, &totals, 0, 16);
    queue.submit([encoder.finish()]);
    (read(device, &pixels), read(device, &totals))
}

/// `f32` to IEEE half bits, for values in [0, 1].
fn half_bits(v: f32) -> u16 {
    if v == 0.0 { return 0 }
    let bits = v.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = (bits >> 13) & 0x3ff;
    if exp <= 0 { 0 } else { ((exp as u32) << 10 | mantissa) as u16 }
}

#[test]
fn compute_matches_wgpus_bit_for_bit() {
    let (device, queue) = gpu();
    let (pixels, totals) = steps(&device, &queue);
    let (wgpu_device, wgpu_queue) = wgpu_metal();
    let (want_pixels, want_totals) = steps(&wgpu_device, &wgpu_queue);
    let totals: &[u32] = bytemuck::cast_slice(&totals);
    assert_eq!(totals, [2 * W * H / 4; 4], "every invocation of both dispatches ran");
    assert!(pixels.iter().any(|&b| b != 0), "the second dispatch stored");
    assert!(pixels == want_pixels, "differs from wgpu's at {:?}", pixels.iter().zip(&want_pixels).position(|(a, b)| a != b));
    assert_eq!(totals, bytemuck::cast_slice::<u8, u32>(&want_totals));
}

/// wgpu promises a texture nothing wrote reads as zero, as a fill's backdrop does.
#[test]
fn new_textures_read_as_zero() {
    use wgpu::TextureUsages as T;
    let (device, queue) = gpu();
    let layered = wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 2 };
    for _ in 0..2 {
        let t = texture(&device, wgpu::TextureFormat::Rgba16Float, layered, 2, T::RENDER_ATTACHMENT | T::TEXTURE_BINDING | T::COPY_SRC);
        let mut encoder = device.create_command_encoder(&Default::default());
        for layer in 0..2 {
            for mip in 0..2 {
                let view = t.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_mip_level: mip,
                    mip_level_count: Some(1), base_array_layer: layer, array_layer_count: Some(1), ..Default::default() });
                drop(clear(&mut encoder, &view));
            }
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    let t = texture(&device, wgpu::TextureFormat::Rgba16Float, layered, 2, T::RENDER_ATTACHMENT | T::TEXTURE_BINDING | T::COPY_SRC);
    let out = buffer(&device, 256 * u64::from(H / 2), wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 1, origin: wgpu::Origin3d { x: 0, y: 0, z: 1 }, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(H / 2) } },
        size(W / 2, H / 2));
    queue.submit([encoder.finish()]);
    assert!(read(&device, &out).iter().all(|&b| b == 0), "a new texture's last layer and mip read as zero");
}

#[test]
#[cfg_attr(target_abi = "sim", ignore = "the iOS simulator samples no GPU counters")]
fn passes_sample_timestamps_at_their_boundaries() {
    let (device, queue) = timed();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(r#"
        @compute @workgroup_size(1) fn noop() {}
        @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
            return vec4<f32>(f32(i & 1u) * 4.0 - 1.0, f32(i >> 1u) * 4.0 - 1.0, 0.0, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
    "#.into()) });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[], immediate_size: 0 });
    let noop = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None, layout: Some(&layout), module: &module, entry_point: None, compilation_options: Default::default(), cache: None,
    });
    let fill = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&layout),
        vertex: wgpu::VertexState { module: &module, entry_point: None, compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: None, compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())] }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: 4 });
    let target = texture(&device, wgpu::TextureFormat::Rgba8Unorm, size(W, H), 1, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let view = target.create_view(&Default::default());
    let resolve = buffer(&device, 32, wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC);
    let out = buffer(&device, 32, wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
            query_set: &queries, beginning_of_pass_write_index: Some(0), end_of_pass_write_index: Some(1) }) });
        pass.set_pipeline(&noop);
        pass.dispatch_workgroups(1, 1, 1);
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            timestamp_writes: Some(wgpu::RenderPassTimestampWrites { query_set: &queries, beginning_of_pass_write_index: Some(2), end_of_pass_write_index: Some(3) }),
            ..Default::default()
        });
        pass.set_pipeline(&fill);
        pass.draw(0..3, 0..1);
    }
    let index = queue.submit([encoder.finish()]);
    device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.resolve_query_set(&queries, 0..4, &resolve, 0);
    encoder.copy_buffer_to_buffer(&resolve, 0, &out, 0, 32);
    queue.submit([encoder.finish()]);
    let ticks: Vec<u64> = bytemuck::cast_slice(&read(&device, &out)).to_vec();
    assert!(ticks[0] != 0 && ticks.windows(2).all(|w| w[0] <= w[1]), "pass boundaries in order: {ticks:?}");
    let ms = (ticks[3] - ticks[0]) as f64 * f64::from(queue.get_timestamp_period()) / 1e6;
    assert!(ms < 100.0, "a noop dispatch and a clear take well under 100 ms, not {ms} ms");
}

const WORKGROUP: &str = r#"
@group(0) @binding(0) var<storage, read> input: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;

// Unused by `gather`: Naga's MSL writer skips it, leaving `ids` at index 0 and `count` at index 1.
var<workgroup> unused_wg: array<u32, 16>;
var<workgroup> ids: array<u32, 64>;
var<workgroup> count: atomic<u32>;

@compute @workgroup_size(64)
fn gather(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wid: vec3<u32>) {
    if (li == 0u) {
        atomicStore(&count, 0u);
    }
    workgroupBarrier();
    let idx = wid.x * 64u + li;
    let v = input[idx];
    if ((v & 1u) != 0u) {
        let slot = atomicAdd(&count, 1u);
        ids[slot] = v;
    }
    workgroupBarrier();
    let n = atomicLoad(&count);
    var sum = 0u;
    for (var k = 0u; k < n; k = k + 1u) {
        sum = sum + ids[k];
    }
    output[idx] = sum + n * 100000u;
}
"#;

/// Runs `WORKGROUP` once after switching from a pipeline with no workgroup memory in the same pass,
/// and once in a fresh compute pass on the same encoder.
fn run_workgroup(device: &wgpu::Device, queue: &wgpu::Queue) -> (Vec<u32>, Vec<u32>) {
    use wgpu::{BufferUsages as B, ShaderStages as S};
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("workgroup"),
        source: wgpu::ShaderSource::Wgsl(WORKGROUP.into()),
    });
    let noop_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("noop"),
        source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn noop() {}".into()),
    });
    let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: S::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[storage(0, true), storage(1, false)],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let gather = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("gather"),
        layout: Some(&layout),
        module: &module,
        entry_point: Some("gather"),
        compilation_options: Default::default(),
        cache: None,
    });
    let noop_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let noop = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("noop"),
        layout: Some(&noop_layout),
        module: &noop_module,
        entry_point: Some("noop"),
        compilation_options: Default::default(),
        cache: None,
    });
    let input = buffer(device, 128 * 4, B::STORAGE | B::COPY_DST);
    let data: Vec<u32> = (1..=128).collect();
    queue.write_buffer(&input, 0, bytemuck::cast_slice(&data));
    let out1 = buffer(device, 128 * 4, B::STORAGE | B::COPY_SRC);
    let out2 = buffer(device, 128 * 4, B::STORAGE | B::COPY_SRC);
    let read1 = buffer(device, 128 * 4, B::COPY_DST | B::MAP_READ);
    let read2 = buffer(device, 128 * 4, B::COPY_DST | B::MAP_READ);
    let group = |out: &wgpu::Buffer| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: out.as_entire_binding() },
            ],
        })
    };
    let (bg1, bg2) = (group(&out1), group(&out2));
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&noop);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&gather);
        pass.set_bind_group(0, &bg1, &[]);
        pass.dispatch_workgroups(2, 1, 1);
    }
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&gather);
        pass.set_bind_group(0, &bg2, &[]);
        pass.dispatch_workgroups(2, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&out1, 0, &read1, 0, 128 * 4);
    encoder.copy_buffer_to_buffer(&out2, 0, &read2, 0, 128 * 4);
    queue.submit([encoder.finish()]);
    (
        bytemuck::cast_slice(&read(device, &read1)).to_vec(),
        bytemuck::cast_slice(&read(device, &read2)).to_vec(),
    )
}

#[test]
fn workgroup_memory_matches_wgpus_bit_for_bit() {
    let (device, queue) = gpu();
    let (first, second) = run_workgroup(&device, &queue);
    let (wgpu_device, wgpu_queue) = wgpu_metal();
    let (want_first, want_second) = run_workgroup(&wgpu_device, &wgpu_queue);
    // 32 odd values in 1..=64 sum to 1024; 32 odd values in 65..=128 sum to 3072.
    assert!(first[..64].iter().all(|&v| v == 32 * 100_000 + 1024), "workgroup 0: {:?}", &first[..4]);
    assert!(first[64..].iter().all(|&v| v == 32 * 100_000 + 3072), "workgroup 1: {:?}", &first[64..68]);
    assert_eq!(first, want_first);
    assert_eq!(second, first, "fresh compute pass bound workgroup memory too");
    assert_eq!(second, want_second);
}

/// Dispatches `rune-light`'s `field.wgsl` (the only built-in shader with `var<workgroup>`) with
/// its exact bind-group layout so this backend loads it from `builtin_native.metallib` and binds
/// its workgroup memory from `builtin_table.rs`.
fn run_field(device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
    use wgpu::{BufferUsages as B, ShaderStages as S, TextureUsages as T};
    const FW: u32 = 48;
    const FH: u32 = 40;
    const PADDED_ROW: u32 = 512;
    let src = format!(
        "{}\n{}",
        include_str!("../../../rune-light/src/shaders/common.wgsl"),
        include_str!("../../../rune-light/src/shaders/field.wgsl"),
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("light field"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("field"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: S::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: S::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: S::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba16Float,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("light field"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("light field"),
        layout: Some(&layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    if let Some(super::shader::ShaderModule::Naga(ir)) = module.as_custom::<super::shader::ShaderModule>()
        && super::builtin::known(&ir.hash)
    {
        assert!(!ir.was_parsed(), "light field pipeline missed the precompiled builtin table");
    }
    let frame_buf = buffer(device, 144, B::UNIFORM | B::COPY_DST);
    let mut frame = [0u32; 36];
    frame[0] = 96.0f32.to_bits();
    frame[1] = 80.0f32.to_bits();
    frame[2] = (FW as f32).to_bits();
    frame[3] = (FH as f32).to_bits();
    frame[4] = 16.0f32.to_bits();
    frame[5] = 16.0f32.to_bits();
    frame[6] = 0.5f32.to_bits();
    frame[7] = 0.125f32.to_bits();
    frame[12] = 1;
    queue.write_buffer(&frame_buf, 0, bytemuck::cast_slice(&frame));
    let prim_buf = buffer(device, 96, B::STORAGE | B::COPY_DST);
    let prim: [f32; 24] = [
        8.0, 8.0, 80.0, 64.0,
        12.0, 12.0, 12.0, 12.0,
        0.0, 16.0, 0.0, 0.0,
        0.0, 0.6, 1.0, 0.0,
        0.0, 0.0, 0.0, 0.0,
        0.5, 0.5, 0.5, 0.0,
    ];
    queue.write_buffer(&prim_buf, 0, bytemuck::cast_slice(&prim));
    let out = texture(device, wgpu::TextureFormat::Rgba16Float, size(FW, FH), 1, T::STORAGE_BINDING | T::COPY_SRC);
    let out_view = out.create_view(&Default::default());
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: prim_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&out_view) },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups(FW.div_ceil(16), FH.div_ceil(16), 1);
    }
    let readback = buffer(device, u64::from(PADDED_ROW * FH), B::COPY_DST | B::MAP_READ);
    encoder.copy_texture_to_buffer(
        out.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(PADDED_ROW), rows_per_image: Some(FH) },
        },
        out.size(),
    );
    queue.submit([encoder.finish()]);
    read(device, &readback)
}

#[test]
fn precompiled_field_pipeline_binds_workgroup_memory() {
    let (device, queue) = gpu();
    let got = run_field(&device, &queue);
    let (wgpu_device, wgpu_queue) = wgpu_metal();
    let want = run_field(&wgpu_device, &wgpu_queue);
    // Texel (23, 19) is device px (47, 39), inside [8..88, 8..72] r=12: exact distance -31.0 (f16 0xCFC0).
    let at = 19 * 512 + 23 * 8;
    let center_x = u16::from_le_bytes([got[at], got[at + 1]]);
    assert_eq!(center_x, 0xCFC0, "centre of occluder must hold -31.0 (0xCFC0), not CLAMP 256.0 (0x5C00)");
    for y in 0..40usize {
        let row = y * 512..y * 512 + 48 * 8;
        assert_eq!(&got[row.clone()], &want[row], "field row {y} differs from wgpu");
    }
}

const INDIRECT_PRODUCER: &str = r#"
@group(0) @binding(0) var<storage, read_write> args: array<u32>;

@compute @workgroup_size(1)
fn emit_args() {
    // Non-zero offset 16 bytes (u32 index 4): [4, 2, 1] workgroups.
    args[4] = 4u;
    args[5] = 2u;
    args[6] = 1u;
    // Offset 32 bytes (u32 index 8): [0, 2, 1] workgroups (valid no-op).
    args[8] = 0u;
    args[9] = 2u;
    args[10] = 1u;
}
"#;

const INDIRECT_CONSUMER: &str = r#"
@group(0) @binding(0) var<storage, read> weights: array<u32>;
@group(0) @binding(1) var<storage, read_write> out: array<atomic<u32>>;

@compute @workgroup_size(8, 4, 1)
fn consume(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = arrayLength(&weights);
    let idx = id.y * 32u + id.x;
    atomicAdd(&out[idx], weights[idx % n]);
}
"#;

/// Exercises GPU-written indirect dispatch args at a non-zero offset, a zero-workgroup indirect
/// dispatch, and back-to-back CPU-updated indirect submissions.
fn run_indirect(device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u32> {
    use wgpu::{BufferUsages as B, ShaderStages as S};
    let prod_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("indirect producer"),
        source: wgpu::ShaderSource::Wgsl(INDIRECT_PRODUCER.into()),
    });
    let cons_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("indirect consumer"),
        source: wgpu::ShaderSource::Wgsl(INDIRECT_CONSUMER.into()),
    });
    let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: S::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let prod_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[storage(0, false)],
    });
    let prod_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&prod_bgl)],
        immediate_size: 0,
    });
    let producer = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("emit_args"),
        layout: Some(&prod_layout),
        module: &prod_module,
        entry_point: Some("emit_args"),
        compilation_options: Default::default(),
        cache: None,
    });
    let cons_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[storage(0, true), storage(1, false)],
    });
    let cons_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&cons_bgl)],
        immediate_size: 0,
    });
    let consumer = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("consume"),
        layout: Some(&cons_layout),
        module: &cons_module,
        entry_point: Some("consume"),
        compilation_options: Default::default(),
        cache: None,
    });

    let gpu_indirect = buffer(device, 48, B::STORAGE | B::INDIRECT | B::COPY_DST);
    queue.write_buffer(&gpu_indirect, 0, bytemuck::cast_slice(&[99u32, 99, 99, 99]));
    let cpu_indirect = buffer(device, 16, B::INDIRECT | B::COPY_DST);
    let weights = buffer(device, 16, B::STORAGE | B::COPY_DST);
    queue.write_buffer(&weights, 0, bytemuck::cast_slice(&[3u32, 5, 7, 11]));
    let out = buffer(device, 256 * 4, B::STORAGE | B::COPY_SRC);
    let readback = buffer(device, 256 * 4, B::COPY_DST | B::MAP_READ);

    let prod_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &prod_bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: gpu_indirect.as_entire_binding() }],
    });
    let cons_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &cons_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: weights.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: out.as_entire_binding() },
        ],
    });

    // Submit 1: GPU writes indirect args at offsets 16 ([4, 2, 1]) and 32 ([0, 2, 1]), then
    // dispatches both indirectly inside the same compute pass.
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&producer);
        pass.set_bind_group(0, &prod_bg, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&consumer);
        pass.set_bind_group(0, &cons_bg, &[]);
        pass.dispatch_workgroups_indirect(&gpu_indirect, 16);
        pass.dispatch_workgroups_indirect(&gpu_indirect, 32);
    }
    queue.submit([encoder.finish()]);

    // Submit 2 and 3: back-to-back CPU updates on a non-pinned indirect buffer (exercises swap).
    queue.write_buffer(&cpu_indirect, 0, bytemuck::cast_slice(&[2u32, 1, 1]));
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&consumer);
        pass.set_bind_group(0, &cons_bg, &[]);
        pass.dispatch_workgroups_indirect(&cpu_indirect, 0);
    }
    queue.submit([encoder.finish()]);

    queue.write_buffer(&cpu_indirect, 0, bytemuck::cast_slice(&[1u32, 2, 1]));
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&consumer);
        pass.set_bind_group(0, &cons_bg, &[]);
        pass.dispatch_workgroups_indirect(&cpu_indirect, 0);
    }
    encoder.copy_buffer_to_buffer(&out, 0, &readback, 0, 256 * 4);
    queue.submit([encoder.finish()]);

    bytemuck::cast_slice(&read(device, &readback)).to_vec()
}

#[test]
fn indirect_dispatch_matches_wgpus_bit_for_bit() {
    let (device, queue) = gpu();
    let got = run_indirect(&device, &queue);
    let (wgpu_device, wgpu_queue) = wgpu_metal();
    let want = run_indirect(&wgpu_device, &wgpu_queue);
    let weights = [3u32, 5, 7, 11];
    for y in 0..8usize {
        for x in 0..32usize {
            let idx = y * 32 + x;
            let hits = 1u32 + u32::from(x < 16 && y < 4) + u32::from(x < 8);
            assert_eq!(got[idx], hits * weights[idx % 4], "cell ({x}, {y})");
        }
    }
    assert_eq!(got, want, "indirect dispatch output differs from wgpu");
}

#[test]
fn indirect_dispatch_validation_captures_errors() {
    use wgpu::BufferUsages as B;
    let (device, _queue) = gpu();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn noop() {}".into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let noop = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &module,
        entry_point: Some("noop"),
        compilation_options: Default::default(),
        cache: None,
    });
    let no_indirect = buffer(&device, 16, B::STORAGE | B::COPY_DST);
    let indirect = buffer(&device, 16, B::INDIRECT);

    for (label, buf, offset) in [
        ("missing INDIRECT", &no_indirect, 0u64),
        ("misaligned offset", &indirect, 2u64),
        ("out-of-bounds range", &indirect, 8u64),
    ] {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&noop);
            pass.dispatch_workgroups_indirect(buf, offset);
        }
        let err = pollster::block_on(scope.pop());
        assert!(matches!(err, Some(wgpu::Error::Validation { .. })), "{label}: got {err:?}");
    }
}
