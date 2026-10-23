use super::*;
use crate::render::{Gpu, NO_CLIP, OFFSCREEN_FORMAT, Offscreen};

fn draw(surface: &NativeSurface, rect: [f32; 4]) -> SurfaceDraw {
    SurfaceDraw {
        surface: surface.clone(),
        fit: SurfaceFit::Fit,
        before_instance: 0,
        rect,
        radii: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
        opacity: 1.0,
    }
}
fn render(gpu: &Gpu, renderer: &mut SurfaceRenderer, draws: &[SurfaceDraw], size: (u32, u32)) -> Vec<u8> {
    let (completed, completion) = std::sync::mpsc::channel();
    for d in draws {
        let completed = completed.clone();
        d.surface.set_observer(Arc::new(move |t| {
            if t.completed_at.is_some() {
                let _ = completed.send(());
            }
        }));
    }
    renderer.prepare(&gpu.device, &gpu.queue, draws, size);
    for d in draws {
        assert_eq!(d.surface.last_error(), None);
    }
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: renderer.timestamp_writes(),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for i in 0..renderer.len() {
            renderer.draw(&mut pass, i);
        }
    }
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    let pixels = target.read(&gpu.device);
    for _ in draws {
        completion.recv_timeout(std::time::Duration::from_secs(10)).expect("frame completion");
    }
    pixels
}
fn srgb(x: f32) -> u8 {
    let x = if x <= 0.0031308 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Buffer destruction and queue.present both acquire wgpu-core's device snatch write lock.
/// Exercise that lock without a window: it must remain available while the completion
/// worker services a submission whose GPU workload is deliberately still running.
#[test]
fn completion_worker_preserves_cpu_gpu_overlap() {
    use std::time::Duration;
    let gpu = Gpu::headless();
    let surface = NativeSurface::new();
    surface.push_bytes(synthetic(16, 16, 0), 1).unwrap();
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
    let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("overlap workload"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
            @group(0) @binding(0) var<storage, read_write> result: array<u32>;
            @compute @workgroup_size(64)
            fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                var x = id.x;
                for (var i = 0u; i < 1048576u; i++) {
                    x = (x ^ (x >> 13u)) * 1664525u + 1013904223u;
                }
                result[id.x] = x;
            }
        "#
            .into(),
        ),
    });
    let compute = gpu.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("overlap workload"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 65536 * 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &compute.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
    });
    let probe = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("present lock probe"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let target = Offscreen::new(&gpu.device, 16, 16);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&compute);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(1024, 1, 1);
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: renderer.timestamp_writes(),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.draw(&mut pass, 0);
    }
    let (done, completed) = std::sync::mpsc::channel();
    encoder.on_submitted_work_done(move || {
        let _ = done.send(Instant::now());
    });
    let start = Instant::now();
    renderer.submit(&gpu.queue, encoder);
    let submitted = Instant::now();
    // Allow the completion thread to enter its polling path before taking the write lock.
    std::thread::sleep(Duration::from_millis(10));
    let lock_start = Instant::now();
    probe.destroy();
    let unlocked = Instant::now();
    gpu.device.poll(wgpu::PollType::Poll).unwrap();
    let state = completed.try_recv();
    eprintln!(
        "overlap probe: submit {:.3} ms, write lock {:.3} ms, callback {state:?}",
        (submitted - start).as_secs_f64() * 1000.0,
        (unlocked - lock_start).as_secs_f64() * 1000.0
    );
    let still_running = matches!(state, Err(std::sync::mpsc::TryRecvError::Empty));
    assert!(still_running, "snatch write lock was serialized behind GPU completion (or workload too short)");
    let finish = completed.recv_timeout(Duration::from_secs(30)).expect("GPU workload completed");
    assert!(unlocked - lock_start < (finish - start) / 2, "write lock consumed the GPU workload's duration");
    eprintln!(
        "CPU/GPU overlap: write lock {:.3} ms, returned {:.3} ms before GPU completion ({:.3} ms workload)",
        (unlocked - lock_start).as_secs_f64() * 1000.0,
        (finish - unlocked).as_secs_f64() * 1000.0,
        (finish - start).as_secs_f64() * 1000.0
    );
}
fn publish(s: &NativeSurface, b: ByteFrame, id: u64, native: bool) {
    #[cfg(target_os = "macos")]
    if native {
        s.push(macos::PixelBuffer::from_bytes(&b).unwrap(), id).unwrap();
        return;
    }
    let _ = native;
    s.push_bytes(b, id).unwrap();
}

#[test]
fn queued_video_frames_then_readback() {
    let gpu = Gpu::headless();
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let surfaces: Vec<_> = (0..4).map(|_| NativeSurface::new()).collect();
    let (done, completed) = std::sync::mpsc::channel();
    let draws: Vec<_> = surfaces
        .iter()
        .enumerate()
        .map(|(i, s)| {
            publish(s, synthetic(640, 360, 0), 0, cfg!(target_os = "macos"));
            let done = done.clone();
            s.set_observer(Arc::new(move |t| {
                if t.completed_at.is_some() {
                    done.send(t).unwrap();
                }
            }));
            draw(s, [(i % 2) as f32 * 640.0, (i / 2) as f32 * 360.0, 640.0, 360.0])
        })
        .collect();
    let target = Offscreen::new(&gpu.device, 1280, 720);
    let mut expected = None;
    // Headless --at 1 queues 61 frames but maps pixels only after the last one.
    // Exercise multiple bursts so completion polling overlaps the next submission.
    for _ in 0..8 {
        for frame in 0..61 {
            renderer.prepare(&gpu.device, &gpu.queue, &draws, (1280, 720));
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    timestamp_writes: renderer.timestamp_writes(),
                    ..Default::default()
                });
                for i in 0..renderer.len() {
                    renderer.draw(&mut pass, i);
                }
            }
            if frame == 60 {
                target.copy(&mut encoder);
            }
            renderer.submit(&gpu.queue, encoder);
        }
        let pixels = target.read(&gpu.device);
        if let Some(expected) = &expected {
            assert_eq!(&pixels, expected, "unchanged queued frames must read back identically");
        } else {
            assert!(pixels.iter().any(|&v| v != 0));
            expected = Some(pixels);
        }
    }
    let mut timed = 0;
    for _ in 0..8 * 61 * 4 {
        let t = completed.recv_timeout(std::time::Duration::from_secs(10)).expect("queued frame completion");
        assert_eq!(t.gpu_start_seconds.is_some(), t.gpu_end_seconds.is_some());
        if let Some((start, end)) = t.gpu_start_seconds.zip(t.gpu_end_seconds) {
            assert!(end >= start && end > 0.0, "invalid timestamps: {t:?}");
            timed += 1;
        }
    }
    if gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        assert!(timed > 0, "queued frames must still produce GPU timing samples");
    }
}

#[test]
fn video_gpu_colour_sharpness_geometry_and_lifetime() {
    let gpu = Gpu::new(Gpu::instance(), None);
    // Other renderers (including the parallel burst test) may own native planes.
    // Their lifetime must not affect this renderer's ownership assertions.
    #[cfg(target_os = "macos")]
    let _unrelated = {
        let surface = NativeSurface::new();
        publish(&surface, synthetic(16, 16, 0), 0, true);
        let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
        assert_eq!(renderer.live_planes(), 2);
        renderer
    };
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let surface = NativeSurface::new();
    let mut id = 0;
    for native in [false, cfg!(target_os = "macos")] {
        for format in [Format::Nv12Video, Format::Nv12Full] {
            for matrix in [Matrix::Bt601, Matrix::Bt709] {
                for transfer in [Transfer::Srgb, Transfer::Bt709] {
                    let color = ColorInfo { matrix, transfer };
                    for codes in [
                        [16, 128, 128],
                        [235, 128, 128],
                        [0, 128, 128],
                        [255, 128, 128],
                        [81, 90, 240],
                        [145, 54, 34],
                        [41, 240, 110],
                        [128, 128, 128],
                    ] {
                        id += 1;
                        publish(
                            &surface,
                            ByteFrame {
                                width: 8,
                                height: 8,
                                format,
                                color,
                                planes: [vec![codes[0]; 64], [codes[1], codes[2]].repeat(16)],
                            },
                            id,
                            native,
                        );
                        let pixels = render(&gpu, &mut renderer, &[draw(&surface, [0.0, 0.0, 8.0, 8.0])], (8, 8));
                        let reference = nv12_reference(codes[0], codes[1], codes[2], format, color).map(srgb);
                        for pixel in pixels.chunks_exact(4) {
                            for c in 0..3 {
                                assert!(
                                    pixel[c].abs_diff(reference[c]) <= 1,
                                    "native={native} {format:?} {matrix:?} {transfer:?} {codes:?}: {pixel:?} != {reference:?}"
                                );
                            }
                            assert_eq!(pixel[3], 255);
                        }
                    }
                }
            }
        }
        // At matching device size every one-pixel stroke survives exactly. sRGB NV12 full
        // neutral chroma is an identity; BGRA checks all 256 code values in all channels.
        for format in [Format::Nv12Full, Format::Bgra] {
            let mut b = ByteFrame {
                width: 256,
                height: 16,
                format,
                color: ColorInfo::default(),
                planes: [Vec::new(), Vec::new()],
            };
            let mut expected = Vec::new();
            for y in 0..16 {
                for x in 0..256 {
                    let v = if y % 2 == 0 { x as u8 } else { if x % 2 == 0 { 0 } else { 255 } };
                    if format == Format::Bgra {
                        b.planes[0].extend_from_slice(&[v, 255 - v, v / 2, 255]);
                        expected.extend_from_slice(&[v / 2, 255 - v, v, 255]);
                    } else {
                        b.planes[0].push(v);
                        expected.extend_from_slice(&[v, v, v, 255]);
                    }
                }
            }
            if format != Format::Bgra {
                b.planes[1] = vec![128; 256 * 16 / 2];
            }
            id += 1;
            publish(&surface, b, id, native);
            let pixels = render(&gpu, &mut renderer, &[draw(&surface, [0.0, 0.0, 256.0, 16.0])], (256, 16));
            assert_eq!(pixels, expected, "1:1 pixels changed for native={native}, format={format:?}");
        }
    }
    let held = surface.latest().unwrap();
    let weak = Arc::downgrade(&held);
    drop(held);
    surface.clear();
    assert!(weak.upgrade().is_some(), "renderer must retain its current frame");
    renderer.prepare(&gpu.device, &gpu.queue, &[], (256, 16));
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert!(weak.upgrade().is_none(), "old frame leaked after completion and removal");
    #[cfg(target_os = "macos")]
    assert_eq!(renderer.live_planes(), 0, "CVMetalTexture ownership leaked");
    let (submitted, completed) = surface.counts();
    assert_eq!(submitted, completed);
    let timing = surface.timing().unwrap();
    assert_eq!(timing.frame_id, id);
    assert!(timing.completed_at.unwrap() >= timing.submitted_at);
    if gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        assert!(timing.gpu_end_seconds.is_some(), "timestamp query produced no times: {timing:?}");
        assert!(timing.gpu_end_seconds.unwrap() >= timing.gpu_start_seconds.unwrap());
    } else {
        eprintln!("adapter has no timestamp queries: {:?}", gpu.adapter.get_info());
    }

    // Contain letterboxes, cover crops, and rounded clip remains local to the node.
    publish(
        &surface,
        ByteFrame {
            width: 16,
            height: 8,
            format: Format::Bgra,
            color: ColorInfo::default(),
            planes: [vec![255; 16 * 8 * 4], vec![]],
        },
        999,
        false,
    );
    let mut d = draw(&surface, [0.0, 0.0, 16.0, 16.0]);
    let p = render(&gpu, &mut renderer, &[d.clone()], (16, 16));
    assert_eq!(&p[0..4], &[0; 4]);
    assert_eq!(&p[8 * 16 * 4..8 * 16 * 4 + 4], &[255; 4]);
    d.fit = SurfaceFit::Fill;
    let p = render(&gpu, &mut renderer, &[d.clone()], (16, 16));
    assert_eq!(&p[0..4], &[255; 4]);
    d.radii = [8.0; 4];
    d.clip[2] = 12.0;
    let p = render(&gpu, &mut renderer, &[d], (16, 16));
    assert_eq!(p[3], 0);
    assert_eq!(p[(8 * 16 + 14) * 4 + 3], 0);
    assert_eq!(p[(8 * 16 + 8) * 4 + 3], 255);

    #[cfg(target_os = "macos")]
    {
        // Drop mailbox + renderer references while commands are encoded but unsubmitted.
        // The wgpu HAL ownership guard, not a borrowed CoreVideo/Metal pointer, must survive.
        surface.push(macos::PixelBuffer::from_bytes(&synthetic(16, 16, 0)).unwrap(), 1000).unwrap();
        renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
        let target = Offscreen::new(&gpu.device, 16, 16);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.draw(&mut pass, 0);
        }
        surface.clear();
        renderer.prepare(&gpu.device, &gpu.queue, &[], (16, 16));
        assert_eq!(renderer.live_planes(), 2, "encoded frames released too early");
        target.copy(&mut encoder);
        gpu.queue.submit([encoder.finish()]);
        let p = target.read(&gpu.device);
        assert_eq!(p[3], 255);
        assert_eq!(renderer.live_planes(), 0, "submitted plane owners leaked");
        publish(
            &surface,
            ByteFrame {
                width: 16,
                height: 8,
                format: Format::Bgra,
                color: ColorInfo::default(),
                planes: [vec![255; 16 * 8 * 4], vec![]],
            },
            1001,
            false,
        );
    }

    fn view(s: &NativeSurface, _: &mut crate::Cx<NativeSurface>) -> crate::El {
        crate::el()
            .w(8.0)
            .h(4.0)
            .fill(crate::Color::BLACK)
            .native_surface(s, SurfaceFit::Fit)
            .border(1.0, crate::Color::BLACK)
            .child(crate::el().w(2.0).h(2.0).fill(crate::Color::BLACK))
    }
    let mut rt = crate::Runtime::new(surface.clone(), view, (8.0, 4.0), crate::Env { system_dark: true, scale: 2.0 });
    rt.frame(0.0);
    assert_eq!(rt.surfaces[0].rect, [0.0, 0.0, 16.0, 8.0]);
    let mut renderer = crate::render::Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (16, 8));
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (16, 8));
    let target = Offscreen::new(&gpu.device, 16, 8);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    let p = target.read(&gpu.device);
    assert_eq!(&p[0..4], &[0, 0, 0, 255], "border must paint over video");
    assert_eq!(&p[(3 * 16 + 3) * 4..(3 * 16 + 3) * 4 + 4], &[0, 0, 0, 255], "child must paint over video");
    assert_eq!(&p[(4 * 16 + 10) * 4..(4 * 16 + 10) * 4 + 4], &[255; 4], "video must paint over background");

    // A video pipeline switch must also restore the retained-springs bind group before
    // painting the border and child. Identity tracks should exactly match ordinary rendering.
    let zero = crate::gpu_springs::Track::constant(0.0);
    let node = crate::gpu_springs::Node { x: zero, y: zero, opacity: crate::gpu_springs::Track::constant(1.0) };
    let mut sprung =
        crate::render::Renderer::with_springs(&gpu.device, OFFSCREEN_FORMAT, &[node], &vec![0; rt.instances.len()]);
    sprung.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (16, 8));
    sprung.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (16, 8));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    sprung.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    sprung.submit(&gpu.queue, encoder);
    assert_eq!(target.read(&gpu.device), p, "retained-springs binding must survive the video draw");

    // A custom compositor can omit video. Preparing it alone must not report a submission.
    let submitted = surface.counts().0;
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (16, 8));
    renderer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));
    assert_eq!(surface.counts().0, submitted, "undrawn video must not report submission");

    // External pass timing takes precedence without resolving NativeSurface's unwritten
    // query set. Frame completion still arrives, with no native GPU timestamps.
    if gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        let external = gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("external UI timing"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let (done, completion) = std::sync::mpsc::channel();
        surface.set_observer(Arc::new(move |t| {
            if t.completed_at.is_some() {
                let _ = done.send(t);
            }
        }));
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (16, 8));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw_timed(
            &mut encoder,
            &target.view,
            [0.0; 4],
            Some(wgpu::RenderPassTimestampWrites {
                query_set: &external,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: Some(1),
            }),
        );
        renderer.submit(&gpu.queue, encoder);
        // The preceding render's callback may still arrive; consume until this submission.
        let submission = surface.timing().unwrap().submitted_at;
        loop {
            let t =
                completion.recv_timeout(std::time::Duration::from_secs(10)).expect("externally timed frame completion");
            if t.submitted_at != submission {
                continue;
            }
            assert_eq!(t.gpu_start_seconds, None);
            assert_eq!(t.gpu_end_seconds, None);
            break;
        }
    }
}
