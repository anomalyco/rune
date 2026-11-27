//! Calls this backend doesn't implement fail as wgpu errors do, naming the call.

use std::sync::{Arc, Mutex};

use super::bench::{clear, gpu};

fn target(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    })
}

fn indirect_args(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::INDIRECT, mapped_at_creation: false })
}

/// Clears `texture`, calls `unsupported` in the pass, and submits.
fn draw(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, unsupported: impl FnOnce(&mut wgpu::RenderPass<'_>)) {
    let view = texture.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    unsupported(&mut clear(&mut encoder, &view));
    queue.submit([encoder.finish()]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

#[test]
fn an_unsupported_command_goes_to_the_error_scope_and_is_skipped() {
    let (device, queue) = gpu();
    let (texture, args) = (target(&device), indirect_args(&device));
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    draw(&device, &queue, &texture, |pass| pass.draw_indirect(&args, 0));
    let error = pollster::block_on(scope.pop()).expect("the indirect draw is an error");
    let message = error.to_string();
    assert!(message.contains("indirect draws") && message.contains("GpuBackend::Wgpu"), "names the call and the way back: {message}");
    // The queue keeps working after the skipped call.
    draw(&device, &queue, &texture, |_| {});
}

#[test]
fn the_uncaptured_error_handler_hears_unsupported_calls_and_shader_errors() {
    let (device, queue) = gpu();
    let heard = Arc::new(Mutex::new(Vec::new()));
    device.on_uncaptured_error(Arc::new({ let heard = Arc::clone(&heard); move |e: wgpu::Error| heard.lock().unwrap().push(e.to_string()) }));
    let texture = target(&device);
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.clear_texture(&texture, &Default::default());
    queue.submit([encoder.finish()]);
    let _broken = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("broken"), source: wgpu::ShaderSource::Wgsl("fn (".into()) });
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), 2, "{heard:?}");
    assert!(heard[0].contains("CommandEncoder::clear_texture"), "{}", heard[0]);
}

#[test]
fn a_depth_pipeline_is_an_invalid_pipeline_that_draws_nothing() {
    let (device, queue) = gpu();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(r#"
        @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
            return vec4<f32>(vec2<f32>(f32(i & 1u), f32(i >> 1u)) * 4.0 - 1.0, 0.0, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
    "#.into()) });
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("depth"), layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())] }),
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less), stencil: Default::default(), bias: Default::default() }),
        multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let error = pollster::block_on(scope.pop()).expect("depth is an error");
    assert!(error.to_string().contains("depth and stencil"), "{error}");
    let texture = target(&device);
    draw(&device, &queue, &texture, |pass| { pass.set_pipeline(&pipeline); pass.draw(0..3, 0..1); });
    let out = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 1024, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(texture.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &out,
        layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: None } }, texture.size());
    queue.submit([encoder.finish()]);
    out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert_eq!(out.slice(..).get_mapped_range().unwrap()[..4], [0, 0, 51, 102], "only the clear: the invalid pipeline drew nothing");
}

#[test]
fn with_no_scope_or_handler_an_unsupported_call_panics_naming_it() {
    let (device, queue) = gpu();
    let (texture, args) = (target(&device), indirect_args(&device));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        draw(&device, &queue, &texture, |pass| pass.multi_draw_indirect(&args, 0, 1));
    }));
    let payload = panicked.expect_err("wgpu's default handler panics");
    let message = payload.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("indirect draws"), "{message}");
}
