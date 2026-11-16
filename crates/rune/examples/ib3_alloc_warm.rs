// Measuring branch only. How a D3D12 device's command allocators warm, outside the app:
//   ib3_alloc_warm <adapter substring> <mode: none|seq|par> <encoders> <lists per warm encoder> <rounds> <frame passes> <draws per pass>
// Prints the warm-up's wall time and then 24 "frames" (an encoder with <frame passes> passes of
// <draws per pass> draws into a 1280x800 target, submitted while the previous two are in flight),
// each frame's record+finish+submit wall time in µs.
use std::time::Instant;

const SHADER: &str = "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> { let x = f32(i & 1u); let y = f32(i >> 1u); return vec4<f32>(x * 0.01 - 1.0, y * 0.01 - 1.0, 0.0, 1.0); }
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }";

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (name, mode) = (a[1].to_lowercase(), a[2].as_str());
    let n = |i: usize| a[i].parse::<usize>().unwrap();
    let (encoders, lists, rounds, passes, draws) = (n(3), n(4), n(5), n(6), n(7));
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12))
        .into_iter()
        .find(|a| a.get_info().name.to_lowercase().contains(&name))
        .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let texture = |w, h| device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] });
    let small = texture(4, 4).create_view(&Default::default());
    let big = texture(1280, 800).create_view(&Default::default());
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(format.into())] }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let pass = |enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, draws: usize| {
        let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            ..Default::default()
        });
        if draws > 0 { p.set_pipeline(&pipeline); }
        for _ in 0..draws { p.draw(0..4, 0..1); }
    };
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256, usage: wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let t = Instant::now();
    for _ in 0..rounds {
        match mode {
            "seq" => {
                let encs: Vec<_> = (0..encoders).map(|_| device.create_command_encoder(&Default::default())).collect();
                for mut e in encs { for _ in 0..lists { pass(&mut e, &small, 0); } queue.write_buffer(&buffer, 0, &[0; 4]); queue.submit([e.finish()]); }
            }
            "par" => {
                let bufs: Vec<_> = std::thread::scope(|s| {
                    let hs: Vec<_> = (0..encoders).map(|_| s.spawn(|| { let mut e = device.create_command_encoder(&Default::default()); for _ in 0..lists { pass(&mut e, &small, 0); } e.finish() })).collect();
                    hs.into_iter().map(|h| h.join().unwrap()).collect()
                });
                for b in bufs { queue.write_buffer(&buffer, 0, &[0; 4]); queue.submit([b]); }
            }
            _ => {}
        }
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }
    let warm_us = t.elapsed().as_micros();
    let mut out = Vec::new();
    for _ in 0..24 {
        let t = Instant::now();
        let mut e = device.create_command_encoder(&Default::default());
        for _ in 0..passes { pass(&mut e, &big, draws); }
        queue.write_buffer(&buffer, 0, &[0; 4]);
        queue.submit([e.finish()]);
        out.push(t.elapsed().as_micros());
        // Two frames in flight, as a window with one queued frame keeps.
        std::thread::sleep(std::time::Duration::from_millis(4));
        let _ = device.poll(wgpu::PollType::Poll);
    }
    println!("{mode} enc={encoders} lists={lists} rounds={rounds} warm_us={warm_us} frames={out:?}");
}
