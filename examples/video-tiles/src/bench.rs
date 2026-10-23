//! No window/pixel readback. Import/encode/submit CPU and GPU render-pass timestamp duration.
use rune::native_surface::{NativeSurface, SurfaceDraw, SurfaceFit, SurfaceRenderer, synthetic};
use rune::render::{Gpu, OFFSCREEN_FORMAT};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub fn run() {
    let gpu = Gpu::new(Gpu::instance(), None);
    eprintln!("adapter={:?}", gpu.adapter.get_info());
    for (tiles, w, h) in [(1, 1920, 1080), (4, 1920, 1080), (9, 1920, 1080), (1, 3840, 2160), (1, 5120, 2880)] {
        let columns = (tiles as f32).sqrt() as u32;
        let size = (w * columns, h * columns);
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video benchmark"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let timings = Arc::new(Mutex::new(Vec::new()));
        let (completed, completion) = std::sync::mpsc::channel();
        let surfaces: Vec<_> = (0..tiles).map(|_| NativeSurface::new()).collect();
        let times = timings.clone();
        surfaces[0].set_observer(Arc::new(move |t| {
            if let (Some(a), Some(b)) = (t.gpu_start_seconds, t.gpu_end_seconds) {
                times.lock().unwrap().push((b - a) * 1000.0);
            }
            if t.completed_at.is_some() {
                let _ = completed.send(());
            }
        }));
        let draws: Vec<_> = surfaces
            .iter()
            .enumerate()
            .map(|(i, s)| SurfaceDraw {
                surface: s.clone(),
                fit: SurfaceFit::Fit,
                before_instance: 0,
                rect: [(i as u32 % columns * w) as f32, (i as u32 / columns * h) as f32, w as f32, h as f32],
                radii: [0.0; 4],
                clip: rune::render::NO_CLIP,
                fade: [0; 4],
                opacity: 1.0,
            })
            .collect();
        // Three independent IOSurface buffers per tile model a decoder's recyclable output pool.
        // Fill only before timing: production import never accesses pixels on CPU.
        #[cfg(target_os = "macos")]
        let buffers: Vec<Vec<_>> = (0..tiles)
            .map(|_| {
                (0..3)
                    .map(|i| rune::native_surface::macos::PixelBuffer::from_bytes(&synthetic(w, h, i)).unwrap())
                    .collect()
            })
            .collect();
        let mut runs = Vec::new();
        for run in 0..8 {
            timings.lock().unwrap().clear();
            let mut cpu = Vec::new();
            for frame in 0..30 {
                let begin = Instant::now();
                for (i, s) in surfaces.iter().enumerate() {
                    #[cfg(target_os = "macos")]
                    s.push(buffers[i][frame % 3].clone(), (run * 30 + frame) as u64).unwrap();
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = i;
                        s.push_bytes(synthetic(w, h, frame as u64), (run * 30 + frame) as u64).unwrap();
                    }
                }
                renderer.prepare(&gpu.device, &gpu.queue, &draws, size);
                let mut encoder = gpu.device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("video benchmark"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
                renderer.submit(&gpu.queue, encoder);
                cpu.push(begin.elapsed().as_secs_f64() * 1000.0);
                completion.recv_timeout(Duration::from_secs(10)).expect("final frame completion without another draw");
                // Pace actual submissions at 60 Hz; waiting is excluded from CPU cost.
                if let Some(wait) = Duration::from_secs_f64(1.0 / 60.0).checked_sub(begin.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            let gpu_ms = timings.lock().unwrap().clone();
            if gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
                assert_eq!(gpu_ms.len(), 30, "every submitted frame must have a valid GPU timestamp pair");
            }
            for s in &surfaces {
                assert_eq!(s.last_error(), None);
            }
            if run > 0 {
                runs.push(serde_json::json!({"cpu_p50_ms":median(&cpu),"cpu_p95_ms":percentile(&cpu,0.95),"gpu_samples":gpu_ms.len(),"gpu_p50_ms":median(&gpu_ms),"gpu_p95_ms":percentile(&gpu_ms,0.95)}));
            }
        }
        let cpu: Vec<_> = runs.iter().map(|r| r["cpu_p50_ms"].as_f64().unwrap()).collect();
        let gpu_ms: Vec<_> = runs.iter().filter_map(|r| r["gpu_p50_ms"].as_f64()).collect();
        println!(
            "{}",
            serde_json::json!({"tiles":tiles,"source":[w,h],"target":[size.0,size.1],"hz":60,"warmup_runs":1,"runs":runs,
            "cpu_median_ms":median(&cpu),"gpu_median_ms":median(&gpu_ms),"cpu_per_tile_ms":median(&cpu).map(|v|v/tiles as f64),"gpu_per_tile_ms":median(&gpu_ms).map(|v|v/tiles as f64)})
        );
    }
}
fn median(xs: &[f64]) -> Option<f64> {
    percentile(xs, 0.5)
}
fn percentile(xs: &[f64], p: f64) -> Option<f64> {
    let mut xs = xs.to_vec();
    xs.sort_by(f64::total_cmp);
    xs.get((xs.len() as f64 * p) as usize).copied()
}
