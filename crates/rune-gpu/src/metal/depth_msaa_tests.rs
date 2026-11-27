//! Native Metal depth/stencil and multisample resolve tests, compared against wgpu's Metal backend.

use super::bench::gpu;
use super::resource::MetalTexture;

const W: u32 = 32;
const H: u32 = 32;

fn wgpu_metal() -> (wgpu::Device, wgpu::Queue) {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::METAL;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let desc = wgpu::DeviceDescriptor {
        required_features: adapter.features() & wgpu::Features::DEPTH32FLOAT_STENCIL8,
        required_limits: adapter.limits(),
        ..Default::default()
    };
    pollster::block_on(adapter.request_device(&desc)).unwrap()
}

fn tex(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn read_rgba(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let row = (W * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row) * u64::from(H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(H) },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let mut out = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H as usize {
        let start = y * row as usize;
        out.extend_from_slice(&mapped[start..start + (W * 4) as usize]);
    }
    drop(mapped);
    buf.unmap();
    out
}

const DEPTH_MSAA_WGSL: &str = r#"
struct VIn { @location(0) pos: vec3<f32>, @location(1) color: vec4<f32> };
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };

@vertex fn vs(v: VIn) -> VOut {
    var o: VOut;
    o.pos = vec4<f32>(v.pos, 1.0);
    o.color = v.color;
    return o;
}

@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    return i.color;
}
"#;

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

/// Draws a near red triangle first and a far green quad second (with a diagonal edge to exercise
/// 4x MSAA coverage) into a 4x MSAA color + Depth32Float depth pass resolving into a 1x slot
/// texture, then samples that slot texture into a second 1x target.
fn run_msaa_depth_slot_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    depth_format: wgpu::TextureFormat,
    store_msaa: wgpu::StoreOp,
) -> (Vec<u8>, Vec<u8>) {
    use wgpu::{BufferUsages as B, TextureUsages as T};
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("depth_msaa"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_MSAA_WGSL.into()),
    });
    let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("depth_msaa"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 28,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attrs,
            })],
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
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState { count: 4, mask: !0, alpha_to_coverage_enabled: false },
        multiview_mask: None,
        cache: None,
    });

    // Near diagonal red triangle (z = 0.2), then far full-screen green quad (z = 0.8).
    // With depth test Less, red must stay in front of green despite being drawn first,
    // and the diagonal boundary must show MSAA partial coverage between red and green.
    #[rustfmt::skip]
    let verts: [[f32; 7]; 9] = [
        // Near red triangle (z = 0.2)
        [-0.8, -0.8, 0.2,  1.0, 0.0, 0.0, 1.0],
        [ 0.8, -0.8, 0.2,  1.0, 0.0, 0.0, 1.0],
        [-0.8,  0.8, 0.2,  1.0, 0.0, 0.0, 1.0],
        // Far green quad (z = 0.8) as two triangles
        [-1.0, -1.0, 0.8,  0.0, 1.0, 0.0, 1.0],
        [ 1.0, -1.0, 0.8,  0.0, 1.0, 0.0, 1.0],
        [ 1.0,  1.0, 0.8,  0.0, 1.0, 0.0, 1.0],
        [-1.0, -1.0, 0.8,  0.0, 1.0, 0.0, 1.0],
        [ 1.0,  1.0, 0.8,  0.0, 1.0, 0.0, 1.0],
        [-1.0,  1.0, 0.8,  0.0, 1.0, 0.0, 1.0],
    ];
    let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: std::mem::size_of_val(&verts) as u64,
        usage: B::VERTEX | B::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&verts));

    let msaa_color = tex(device, wgpu::TextureFormat::Rgba8Unorm, 4, T::RENDER_ATTACHMENT);
    let msaa_depth = tex(device, depth_format, 4, T::RENDER_ATTACHMENT);
    let resolved_slot = tex(device, wgpu::TextureFormat::Rgba8Unorm, 1, T::RENDER_ATTACHMENT | T::TEXTURE_BINDING | T::COPY_SRC);
    let final_target = tex(device, wgpu::TextureFormat::Rgba8Unorm, 1, T::RENDER_ATTACHMENT | T::COPY_SRC);

    let msaa_color_view = msaa_color.create_view(&Default::default());
    let msaa_depth_view = msaa_depth.create_view(&Default::default());
    let slot_view = resolved_slot.create_view(&Default::default());
    let final_view = final_target.create_view(&Default::default());

    let sample_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sample_slot"),
        source: wgpu::ShaderSource::Wgsl(SAMPLE_WGSL.into()),
    });
    let sample_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sample_slot"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &sample_mod,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &sample_mod,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
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
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &sample_pipe.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&slot_view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3d_msaa"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa_color_view,
                resolve_target: Some(&slot_view),
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: store_msaa },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &msaa_depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: depth_format.has_stencil_aspect().then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0),
                    store: wgpu::StoreOp::Discard,
                }),
            }),
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.draw(0..9, 0..1);
    }
    // Sample the resolved slot texture in a subsequent pass (proves lazy_init marked the
    // resolve_target initialized instead of clearing it to transparent black on bind).
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite_slot"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &final_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLUE), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&sample_pipe);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..4, 0..1);
    }
    queue.submit([encoder.finish()]);

    if let Some(mtl) = resolved_slot.as_custom::<MetalTexture>() {
        assert!(
            !mtl.raw.lazy_init.as_ref().is_some_and(|l| l.has_uninit()),
            "resolve_target must be marked initialized after multisample resolve",
        );
    }

    (read_rgba(device, queue, &resolved_slot), read_rgba(device, queue, &final_target))
}

#[test]
fn depth32_msaa4_resolve_matches_wgpu_and_samples_cleanly() {
    let (device, queue) = gpu();
    let (wgpu_dev, wgpu_queue) = wgpu_metal();

    for (depth_format, store_msaa) in [
        (wgpu::TextureFormat::Depth32Float, wgpu::StoreOp::Discard),
        (wgpu::TextureFormat::Depth32Float, wgpu::StoreOp::Store),
        (wgpu::TextureFormat::Depth24Plus, wgpu::StoreOp::Discard),
        (wgpu::TextureFormat::Depth16Unorm, wgpu::StoreOp::Discard),
        (wgpu::TextureFormat::Depth32FloatStencil8, wgpu::StoreOp::Discard),
        (wgpu::TextureFormat::Depth24PlusStencil8, wgpu::StoreOp::Discard),
    ] {
        let (slot, sampled) = run_msaa_depth_slot_scene(&device, &queue, depth_format, store_msaa);
        let (want_slot, want_sampled) = run_msaa_depth_slot_scene(&wgpu_dev, &wgpu_queue, depth_format, store_msaa);

        let px = |buf: &[u8], x: u32, y: u32| -> [u8; 4] {
            let at = ((y * W + x) * 4) as usize;
            buf[at..at + 4].try_into().unwrap()
        };
        // Bottom-left interior of near red triangle must occlude far green quad.
        assert_eq!(px(&slot, 6, H - 6), [255, 0, 0, 255], "{depth_format:?} near triangle occludes far quad");
        // Top-right corner is outside red triangle -> far green quad visible.
        assert_eq!(px(&slot, W - 4, 4), [0, 255, 0, 255], "{depth_format:?} far quad visible outside triangle");
        // Sampled slot must match the resolved slot (lazy_init must not wipe resolve_target).
        assert_eq!(sampled, slot, "{depth_format:?} sampled slot matches resolved slot");
        assert_eq!(slot, want_slot, "{depth_format:?}/{store_msaa:?} resolved pixels match wgpu");
        assert_eq!(sampled, want_sampled, "{depth_format:?}/{store_msaa:?} sampled pixels match wgpu");
    }
}

/// Stencil mask + read-only depth attachment across two pipelines in one pass.
fn run_stencil_mask_scene(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Vec<u8> {
    use wgpu::{BufferUsages as B, TextureUsages as T};
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("stencil"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_MSAA_WGSL.into()),
    });
    let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
    let make_pipe = |stencil: wgpu::StencilState, write_mask: wgpu::ColorWrites| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stencil"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 28,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format,
                depth_write_enabled: format.has_depth_aspect().then_some(false),
                depth_compare: format.has_depth_aspect().then_some(wgpu::CompareFunction::Always),
                stencil,
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        })
    };

    let write_face = wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Always,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op: wgpu::StencilOperation::Replace,
    };
    let test_face = wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Equal,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op: wgpu::StencilOperation::Keep,
    };
    let mask_pipe = make_pipe(
        wgpu::StencilState { front: write_face, back: write_face, read_mask: 0xFF, write_mask: 0xFF },
        wgpu::ColorWrites::empty(),
    );
    let fill_pipe = make_pipe(
        wgpu::StencilState { front: test_face, back: test_face, read_mask: 0xFF, write_mask: 0x00 },
        wgpu::ColorWrites::ALL,
    );

    #[rustfmt::skip]
    let verts: [[f32; 7]; 12] = [
        // Left-half mask quad
        [-1.0, -1.0, 0.5,  1.0, 1.0, 1.0, 1.0],
        [ 0.0, -1.0, 0.5,  1.0, 1.0, 1.0, 1.0],
        [ 0.0,  1.0, 0.5,  1.0, 1.0, 1.0, 1.0],
        [-1.0, -1.0, 0.5,  1.0, 1.0, 1.0, 1.0],
        [ 0.0,  1.0, 0.5,  1.0, 1.0, 1.0, 1.0],
        [-1.0,  1.0, 0.5,  1.0, 1.0, 1.0, 1.0],
        // Full-screen cyan quad masked by stencil == 1
        [-1.0, -1.0, 0.5,  0.0, 1.0, 1.0, 1.0],
        [ 1.0, -1.0, 0.5,  0.0, 1.0, 1.0, 1.0],
        [ 1.0,  1.0, 0.5,  0.0, 1.0, 1.0, 1.0],
        [-1.0, -1.0, 0.5,  0.0, 1.0, 1.0, 1.0],
        [ 1.0,  1.0, 0.5,  0.0, 1.0, 1.0, 1.0],
        [-1.0,  1.0, 0.5,  0.0, 1.0, 1.0, 1.0],
    ];
    let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: std::mem::size_of_val(&verts) as u64,
        usage: B::VERTEX | B::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&verts));

    let color = tex(device, wgpu::TextureFormat::Rgba8Unorm, 1, T::RENDER_ATTACHMENT | T::COPY_SRC);
    let ds = tex(device, format, 1, T::RENDER_ATTACHMENT);
    let color_view = color.create_view(&Default::default());
    let ds_view = ds.create_view(&Default::default());

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("stencil_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &ds_view,
                depth_ops: None,
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
            }),
            ..Default::default()
        });
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.set_stencil_reference(1);
        pass.set_pipeline(&mask_pipe);
        pass.draw(0..6, 0..1);
        pass.set_pipeline(&fill_pipe);
        pass.draw(6..12, 0..1);
    }
    queue.submit([encoder.finish()]);
    read_rgba(device, queue, &color)
}

#[test]
fn stencil_masking_and_readonly_depth_match_wgpu() {
    let (device, queue) = gpu();
    let (wgpu_dev, wgpu_queue) = wgpu_metal();
    for format in [wgpu::TextureFormat::Depth32FloatStencil8, wgpu::TextureFormat::Stencil8] {
        let got = run_stencil_mask_scene(&device, &queue, format);
        let want = run_stencil_mask_scene(&wgpu_dev, &wgpu_queue, format);
        let px = |x: u32, y: u32| -> [u8; 4] {
            let at = ((y * W + x) * 4) as usize;
            got[at..at + 4].try_into().unwrap()
        };
        assert_eq!(px(4, H / 2), [0, 255, 255, 255], "{format:?} left half passes stencil");
        assert_eq!(px(W - 4, H / 2), [0, 0, 0, 255], "{format:?} right half masked out");
        assert_eq!(got, want, "{format:?} stencil output matches wgpu");
    }
}

#[test]
fn msaa_store_and_resolve_preserves_samples_for_second_pass() {
    use wgpu::{BufferUsages as B, TextureUsages as T};
    let (device, queue) = gpu();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("msaa_two_pass"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_MSAA_WGSL.into()),
    });
    let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("msaa_two_pass"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 28,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attrs,
            })],
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
        primitive: Default::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState { count: 4, mask: !0, alpha_to_coverage_enabled: false },
        multiview_mask: None,
        cache: None,
    });

    #[rustfmt::skip]
    let verts: [[f32; 7]; 6] = [
        // Left half: red
        [-1.0, -1.0, 0.0,  1.0, 0.0, 0.0, 1.0],
        [ 0.0, -1.0, 0.0,  1.0, 0.0, 0.0, 1.0],
        [ 0.0,  1.0, 0.0,  1.0, 0.0, 0.0, 1.0],
        // Right half: blue
        [ 0.0, -1.0, 0.0,  0.0, 0.0, 1.0, 1.0],
        [ 1.0, -1.0, 0.0,  0.0, 0.0, 1.0, 1.0],
        [ 1.0,  1.0, 0.0,  0.0, 0.0, 1.0, 1.0],
    ];
    let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: std::mem::size_of_val(&verts) as u64,
        usage: B::VERTEX | B::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&verts));

    let msaa = tex(&device, wgpu::TextureFormat::Rgba8Unorm, 4, T::RENDER_ATTACHMENT);
    let resolved = tex(&device, wgpu::TextureFormat::Rgba8Unorm, 1, T::RENDER_ATTACHMENT | T::COPY_SRC);
    let msaa_view = msaa.create_view(&Default::default());
    let resolved_view = resolved.create_view(&Default::default());

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa_view,
                resolve_target: Some(&resolved_view),
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.draw(0..3, 0..1);
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa_view,
                resolve_target: Some(&resolved_view),
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Discard },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.draw(3..6, 0..1);
    }
    queue.submit([encoder.finish()]);
    let px = read_rgba(&device, &queue, &resolved);
    let at = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * W + x) * 4) as usize;
        px[i..i + 4].try_into().unwrap()
    };
    // Both pass 1's red triangle and pass 2's blue triangle must appear after Load + MultisampleResolve.
    assert_eq!(at(W / 2 - 2, H - 3), [255, 0, 0, 255], "pass 1 MSAA samples survived StoreAndMultisampleResolve");
    assert_eq!(at(W - 3, H - 3), [0, 0, 255, 255], "pass 2 MSAA samples resolved cleanly");
}
