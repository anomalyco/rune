//! Measuring prototype for keeping the scrolled content as tiles: what a scroll frame costs when
//! the timeline's rows are cached in window-wide tiles and the frame only draws them at the new
//! offset (one textured quad per visible tile, opaque, no blending), against clearing the same
//! target (the floor). The rows that enter are not drawn here: their cost is a share of today's
//! main pass (see the README). Timed with GPU timestamps around the pass, 240 frames each, at
//! 1440x960 and 2560x1440, tiles 256 and 512 device px tall. Measurement only; not shipped.
use rune::render::Gpu;

const WGSL: &str = r#"
struct Tile { rect: vec4<f32>, viewport: vec4<f32> };
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> tile: Tile;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let c = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let p = tile.rect.xy + c * tile.rect.zw;
    var o: Out;
    o.pos = vec4<f32>(p.x / tile.viewport.x * 2.0 - 1.0, 1.0 - p.y / tile.viewport.y * 2.0, 0.0, 1.0);
    o.uv = c;
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> { return textureSampleLevel(tex, samp, i.uv, 0.0); }
"#;

fn main() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    println!("adapter {:?}", gpu.adapter.get_info().name);
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(WGSL.into()) });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[
        wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
        wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: None }, count: None },
    ] });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 })),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(format.into())] }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let sampler = device.create_sampler(&Default::default());
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: 2 * 240 });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 2 * 240 * 8, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 2 * 240 * 8, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let period = f64::from(queue.get_timestamp_period());
    for (w, h) in [(1440u32, 960u32), (2560, 1440)] {
        let target = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] });
        let view = target.create_view(&Default::default());
        for tile_h in [256u32, 512] {
            // Enough tiles to cover the window at any offset, plus one.
            let count = h.div_ceil(tile_h) + 1;
            let tiles: Vec<_> = (0..count).map(|_| device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: w, height: tile_h, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST, view_formats: &[] })).collect();
            // Real pixels in each tile (noise): a texture never written samples as its compressed clear state.
            let noise: Vec<u8> = (0..w * tile_h * 4).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
            for t in &tiles {
                queue.write_texture(wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, &noise,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(tile_h) }, wgpu::Extent3d { width: w, height: tile_h, depth_or_array_layers: 1 });
            }
            let stride = 256u64;
            let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: stride * u64::from(count) * 240, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let binds: Vec<_> = tiles.iter().map(|t| device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&t.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &uniforms, offset: 0, size: std::num::NonZeroU64::new(32) }) },
            ] })).collect();
            let mut data = vec![0u8; (stride * u64::from(count) * 240) as usize];
            for f in 0..240u64 {
                // A glide of 40 px a frame (PRESENT's step at 1.5x is 60 device px; 40 keeps fractional rows in play).
                let offset = (f * 40) as f32 % tile_h as f32;
                for k in 0..u64::from(count) {
                    let rect = [0.0f32, k as f32 * tile_h as f32 - offset, w as f32, tile_h as f32, w as f32, h as f32, 0.0, 0.0];
                    let at = ((f * u64::from(count) + k) * stride) as usize;
                    data[at..at + 32].copy_from_slice(bytemuck::cast_slice(&rect));
                }
            }
            queue.write_buffer(&uniforms, 0, &data);
            for (mode, draw) in [("clear only", false), ("tiles", true)] {
                let mut enc = device.create_command_encoder(&Default::default());
                for f in 0..240u32 {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None, color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                        depth_stencil_attachment: None, timestamp_writes: Some(wgpu::RenderPassTimestampWrites { query_set: &queries, beginning_of_pass_write_index: Some(2 * f), end_of_pass_write_index: Some(2 * f + 1) }),
                        occlusion_query_set: None, multiview_mask: None });
                    if draw {
                        pass.set_pipeline(&pipeline);
                        for k in 0..count {
                            pass.set_bind_group(0, &binds[k as usize], &[((u64::from(f) * u64::from(count) + u64::from(k)) * stride) as u32]);
                            pass.draw(0..4, 0..1);
                        }
                    }
                }
                enc.resolve_query_set(&queries, 0..480, &resolve, 0);
                enc.copy_buffer_to_buffer(&resolve, 0, &read, 0, 480 * 8);
                queue.submit([enc.finish()]);
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let mut us: Vec<f64> = {
                    let v = read.slice(..).get_mapped_range().unwrap();
                    let t: &[u64] = bytemuck::cast_slice(&v);
                    (0..240).map(|f| (t[2 * f + 1].saturating_sub(t[2 * f])) as f64 * period / 1e3).collect()
                };
                read.unmap();
                us.sort_by(f64::total_cmp);
                let bytes = u64::from(w) * u64::from(h) * 4;
                let tile_bytes = u64::from(count) * u64::from(w) * u64::from(tile_h) * 4;
                println!("{w}x{h} tile {tile_h} ({count} tiles, {:.1} MiB) {mode}: p50 {:.0} us p90 {:.0} us (target {:.1} MiB)", tile_bytes as f64 / 1048576.0, us[120], us[216], bytes as f64 / 1048576.0);
            }
        }
        // The shift design's copies (review of tile-cache.md, finding 6): the whole kept frame copied to another
        // window-sized texture (`draw_armed`'s copy to the back buffer), and the kept frame moved by 40 rows into a
        // second one (the shift). Each copy sits between two empty passes: the first pass's end and the second's
        // start bracket it, so the time includes a pass boundary (the "clear only" row above is the floor).
        let make = |usage| device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] });
        let kept = make(wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT);
        // Real pixels in the kept frame (noise, as text and edges are): a texture never written copies as its compressed
        // clear state, far faster than a frame's bytes.
        let noise: Vec<u8> = (0..w * h * 4).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &kept, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, &noise,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) }, wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
        let back = make(wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT);
        let (kept_view, back_view) = (kept.create_view(&Default::default()), back.create_view(&Default::default()));
        for (mode, dy) in [("whole copy", 0u32), ("shift copy by 40 rows", 40)] {
            let mut enc = device.create_command_encoder(&Default::default());
            for f in 0..240u32 {
                let empty = |enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, write: wgpu::RenderPassTimestampWrites<'_>| {
                    drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None, color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                        depth_stencil_attachment: None, timestamp_writes: Some(write), occlusion_query_set: None, multiview_mask: None }));
                };
                empty(&mut enc, &kept_view, wgpu::RenderPassTimestampWrites { query_set: &queries, beginning_of_pass_write_index: None, end_of_pass_write_index: Some(2 * f) });
                enc.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: &kept, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo { texture: &back, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: dy, z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::Extent3d { width: w, height: h - dy, depth_or_array_layers: 1 });
                empty(&mut enc, &back_view, wgpu::RenderPassTimestampWrites { query_set: &queries, beginning_of_pass_write_index: Some(2 * f + 1), end_of_pass_write_index: None });
            }
            enc.resolve_query_set(&queries, 0..480, &resolve, 0);
            enc.copy_buffer_to_buffer(&resolve, 0, &read, 0, 480 * 8);
            queue.submit([enc.finish()]);
            read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let mut us: Vec<f64> = {
                let v = read.slice(..).get_mapped_range().unwrap();
                let t: &[u64] = bytemuck::cast_slice(&v);
                (0..240).map(|f| (t[2 * f + 1].saturating_sub(t[2 * f])) as f64 * period / 1e3).collect()
            };
            read.unmap();
            us.sort_by(f64::total_cmp);
            println!("{w}x{h} {mode}: p50 {:.0} us p90 {:.0} us ({:.1} MiB)", us[120], us[216], f64::from(w) * f64::from(h - dy) * 4.0 / 1048576.0);
        }
    }
}
