use super::*;

pub(super) const NO_CLIP: [f32; 4] = [-1e6, -1e6, 1e6, 1e6];
pub(super) const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

static GPU_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// One test's GPU at a time on Windows, so the overlap probe's pending workload is timed alone,
/// not beside another test's GPU work.
#[cfg(target_os = "windows")]
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A test's GPU. A lost device (a GPU reset, a driver update, a GPU removed) fails the test that
/// owns it, at the latest when the test ends: a loss also completes work and fires callbacks,
/// which would otherwise read as the GPU having done what the test asked.
pub(super) struct Gpu {
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    lost: Arc<std::sync::Mutex<Option<String>>>,
    #[cfg(target_os = "windows")]
    _serial: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.assert_alive();
        }
    }
}

impl Gpu {
    pub(super) fn try_new(instance: wgpu::Instance) -> Option<Self> {
        #[cfg(target_os = "windows")]
        let _serial = GPU_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _init = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        let pick = match rune_gpu::pick::pick(&instance, None, false) {
            Ok(pick) => pick,
            Err(error) => {
                rune_gpu::skip_test_without_gpu(&error);
                return None;
            }
        };
        let required_features = pick.adapter.features() & (wgpu::Features::TIMESTAMP_QUERY | rune_gpu::backend::free_features(&pick.adapter));
        let adapter_limits = pick.adapter.limits();
        let required_limits = if adapter_limits.max_inter_stage_shader_variables < wgpu::Limits::default().max_inter_stage_shader_variables
            || adapter_limits.max_color_attachments < wgpu::Limits::default().max_color_attachments
            || adapter_limits.max_compute_workgroup_storage_size < wgpu::Limits::default().max_compute_workgroup_storage_size
        {
            wgpu::Limits::downlevel_defaults().using_resolution(adapter_limits.clone()).using_alignment(adapter_limits)
        } else {
            wgpu::Limits::default().using_resolution(adapter_limits.clone()).using_alignment(adapter_limits)
        };
        let (device, queue) = pollster::block_on(pick.adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rune-media-test"),
            required_features,
            required_limits,
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .expect("request GPU device");
        let lost = Arc::new(std::sync::Mutex::new(None));
        let heard = lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                *heard.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{reason:?}: {message}"));
            }
        });
        Some(Self {
            adapter: pick.adapter,
            device,
            queue,
            lost,
            #[cfg(target_os = "windows")]
            _serial,
        })
    }

    /// Fails the test if its device was lost.
    pub(super) fn assert_alive(&self) {
        if rune_gpu::health::removed(&self.device) {
            panic!("the GPU device was removed during this test, on {}: nothing it read from the GPU since counts", self.adapter.get_info().name);
        }
        if let Some(why) = self.lost.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            panic!("the GPU device was lost during this test ({why}), on {}: nothing it read from the GPU since counts", self.adapter.get_info().name);
        }
    }

    /// What a GPU callback sends on `heard` within `within`, failing the test as soon as the
    /// device is lost: a lost device's callbacks may never come.
    pub(super) fn wait<T>(&self, heard: &std::sync::mpsc::Receiver<T>, within: std::time::Duration) -> Option<T> {
        let until = rune_core::time::now() + within;
        loop {
            if let Ok(value) = heard.recv_timeout(std::time::Duration::from_millis(5)) {
                self.assert_alive();
                return Some(value);
            }
            self.assert_alive();
            if rune_core::time::now() >= until {
                return None;
            }
        }
    }

    pub(super) fn headless_wgpu() -> Option<Self> {
        static INSTANCE: std::sync::OnceLock<wgpu::Instance> = std::sync::OnceLock::new();
        let instance = INSTANCE.get_or_init(|| {
            let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
            #[cfg(target_os = "windows")]
            {
                desc.backends = wgpu::Backends::DX12;
            }
            desc.flags &= !wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL;
            wgpu::Instance::new(desc)
        });
        Self::try_new(instance.clone())
    }
}

pub(super) struct Offscreen {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row: u32,
}

impl Offscreen {
    pub(super) fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let padded_row = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { texture, view, buffer, width, height, padded_row }
    }

    pub(super) fn copy(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.padded_row), rows_per_image: Some(self.height) },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    pub(super) fn read(&self, device: &wgpu::Device) -> Vec<u8> {
        let slice = self.buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let row = (self.width * 4) as usize;
        let stride = self.padded_row as usize;
        let out = {
            let data = slice.get_mapped_range().expect("mapped range");
            let mut out = Vec::with_capacity(row * self.height as usize);
            for y in 0..self.height as usize {
                out.extend_from_slice(&data[y * stride..y * stride + row]);
            }
            out
        };
        self.buffer.unmap();
        out
    }
}

/// Records, on drop, whether the surface's state lock was free: a callback that reads `counts`
/// would deadlock if it weren't.
struct CallbackProbe(NativeSurface, Arc<std::sync::Mutex<Vec<bool>>>);

impl Drop for CallbackProbe {
    fn drop(&mut self) {
        self.1.lock().unwrap().push(self.0.0.try_lock().is_ok());
    }
}

/// A replaced callback's captures drop after the surface's lock, so one reading `counts` or
/// `last_error` can't deadlock there. // differs from dev: dev dropped it under the lock.
#[test]
fn a_replaced_callback_is_dropped_outside_the_surfaces_lock() {
    let surface = NativeSurface::new();
    let freed = Arc::new(std::sync::Mutex::new(Vec::new()));

    let waker = CallbackProbe(surface.clone(), freed.clone());
    surface.set_waker(Arc::new(move || {
        let _ = &waker;
    }));
    surface.set_waker(Arc::new(|| {}));

    let observer = CallbackProbe(surface.clone(), freed.clone());
    surface.set_observer(Arc::new(move |_| {
        let _ = &observer;
    }));
    surface.set_observer(Arc::new(|_| {}));

    assert_eq!(*freed.lock().unwrap(), [true, true], "a callback dropped while the lock was held");
}

pub(super) fn draw(surface: &NativeSurface, rect: [f32; 4]) -> SurfaceDraw {
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
pub(super) fn render(gpu: &Gpu, renderer: &mut SurfaceRenderer, draws: &[SurfaceDraw], size: (u32, u32)) -> Vec<u8> {
    let (completed, completion) = std::sync::mpsc::channel();
    for d in draws {
        let completed = completed.clone();
        d.surface.set_observer(Arc::new(move |t| {
            if t.completed_at.is_some() {
                completed.send(()).ok();
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
        let mut pass = clear_pass(&mut encoder, &target.view, wgpu::Color::TRANSPARENT, renderer.timestamp_writes());
        for i in 0..renderer.len() {
            renderer.draw(&mut pass, i);
        }
    }
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    let pixels = target.read(&gpu.device);
    for _ in draws {
        gpu.wait(&completion, std::time::Duration::from_secs(10)).expect("frame completion");
    }
    pixels
}
fn clear_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    clear: wgpu::Color,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
        })],
        timestamp_writes,
        ..Default::default()
    })
}
/// A pixel test on wgpu, and with `metal-backend` on Rune's native Metal backend too.
macro_rules! on_each_backend {
    ($wgpu:ident, $metal:ident, $body:ident) => {
        #[test]
        fn $wgpu() {
            let Some(gpu) = Gpu::headless_wgpu() else { return };
            $body(gpu);
        }
        #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
        #[test]
        fn $metal() {
            let Some(gpu) = Gpu::try_new(rune_gpu::metal::instance()) else { return };
            $body(gpu);
        }
    };
}
fn srgb(x: f32) -> u8 {
    let x = if x <= 0.0031308 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Buffer destruction and queue.present both acquire wgpu-core's device snatch write lock.
/// Exercise that lock without a window: it must remain available while the completion
/// worker services a submission whose GPU workload is deliberately still running.
///
/// Checked by ordering, not wall-clock time: the lock was taken and released, and a poll
/// after it still found the GPU work pending. A worker that held the snatch read lock through
/// a fence wait would block `destroy` until the work completed, and the poll would then fire
/// its callback. An attempt whose workload finished before the probe (a loaded machine
/// delaying this thread) proves nothing either way, so it retries with more work.
///
/// The pending work is a queue of submissions of one fixed size, `CHUNK` loop iterations per
/// invocation (1.07 billion in all, about 30 ms on a Ryzen 9950X3D's integrated GPU), ahead of
/// the submission with the surface's draw and the callback: a worker that waited on the
/// submission it serves, or on all of them, holds the lock until every one is done. Nothing
/// measured changes a submission's size; a timing decides only how many are queued. The timing
/// tells a submission's fixed round trip (a small submission's time) from its work (a full one's
/// time beyond that), so an adapter with a slow round trip (Metal on an Apple M4: 6,094 us for
/// a 64th of the work) isn't taken for a slow GPU. An overestimate only queues less, and a
/// timed attempt's own run lowers it for the next; an adapter is never refused for being slow,
/// and nothing it measures can size one submission past `CHUNK`. Before anything is known only
/// `CHUNK / 64` is issued (about 1,400 us on that integrated GPU, round trip included, and 4,700
/// to 6,700 us on WARP); each larger size, up by fours, only once four times the size below's
/// whole time fits in `PLANNED_MOST`. An adapter too slow for that queues the smaller size, as
/// WARP does. Windows resets a
/// GPU whose one submission runs 2 s, and an AMD GPU preempts nothing smaller than a
/// submission: 68.7 billion iterations in one submission ran past that on the same integrated
/// GPU, and the reset's completion read as an overlap.
#[test]
fn completion_worker_preserves_cpu_gpu_overlap() {
    use std::sync::mpsc::TryRecvError;
    use std::time::Duration;
    const INVOCATIONS: u32 = 1024 * 64;
    const CHUNK: u32 = 16_384;
    const PLANNED_MOST: Duration = Duration::from_millis(100);
    const CHUNKS_MOST: u32 = 512;
    /// However cheap a chunk's work measures, no attempt queues chunks whose full submit-to-done
    /// times add up past this.
    const QUEUED_MOST: Duration = Duration::from_secs(2);
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let surface = NativeSurface::new();
    surface.push_bytes(synthetic(16, 16, 0), 1).unwrap();
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("overlap workload"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
            @group(0) @binding(0) var<storage, read_write> result: array<u32>;
            @group(0) @binding(1) var<uniform> iterations: vec4<u32>;
            @compute @workgroup_size(64)
            fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                var x = id.x;
                for (var i = 0u; i < iterations.x; i++) {
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
        size: u64::from(INVOCATIONS) * 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let bind_for = |iterations: u32| {
        let work = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlap iterations"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&work, 0, bytemuck::cast_slice(&[iterations, 0, 0, 0]));
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &compute.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: work.as_entire_binding() },
            ],
        })
    };
    // A submission's sizes: `CHUNK / 64`, the only size issued before anything is known, then up
    // by fours to `CHUNK`, each only when the one below predicts it fits (see the calibration).
    let sizes = [CHUNK / 64, CHUNK / 16, CHUNK / 4, CHUNK];
    let binds = sizes.map(bind_for);
    let small = &binds[0];
    let submit = |bind: &wgpu::BindGroup| {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&compute);
            pass.set_bind_group(0, bind, &[]);
            pass.dispatch_workgroups(INVOCATIONS / 64, 1, 1);
        }
        gpu.queue.submit([encoder.finish()])
    };
    let time = |bind: &wgpu::BindGroup| {
        let started = rune_core::time::now();
        let index = submit(bind);
        if gpu.device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: Some(Duration::from_secs(5)) }).is_err() {
            gpu.assert_alive();
            panic!("a calibration submission didn't complete in 5 s");
        }
        gpu.assert_alive();
        started.elapsed()
    };
    // The first submission also pays for the pipeline's first use; it times nothing.
    time(small);
    let name = gpu.adapter.get_info().name;
    let floor = time(small);
    // Up by fours, while four times the last size's whole time (round trip and all, so an upper
    // bound on the next size's time whatever the round trip is) fits in `PLANNED_MOST`. No
    // submission is issued that a smaller one hasn't predicted fits. A software adapter (WARP)
    // runs the work on the CPU, with no GPU to reset; it stays at the first size.
    let software = gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu;
    let mut step = (0, floor);
    let mut steps = vec![floor.as_micros()];
    while !software && step.0 + 1 < sizes.len() && step.1 * 4 <= PLANNED_MOST {
        step = (step.0 + 1, time(&binds[step.0 + 1]));
        steps.push(step.1.as_micros());
    }
    let (index, chunk_alone) = step;
    let (chunk, size) = (&binds[index], sizes[index]);
    // A chunk queued behind others pays for its work, not for another round trip: what it took
    // beyond the first size's time, or all of it when it is the first size.
    let mut per_chunk = if index == 0 { chunk_alone } else { chunk_alone.saturating_sub(floor).max(chunk_alone / 64) };
    let chunks_most = ((QUEUED_MOST.as_secs_f64() / chunk_alone.as_secs_f64().max(1e-6)) as u32).clamp(1, CHUNKS_MOST);
    let why = if size == CHUNK { "the full chunk".to_string() } else if software { "a software adapter".to_string() } else { format!("{} us x 4 would pass {} us", chunk_alone.as_micros(), PLANNED_MOST.as_micros()) };
    eprintln!(
        "overlap probe: on {name}, {} to {} iterations per invocation took {steps:?} us; queueing {size} per invocation ({why}), {} us alone, {} us of it work; at most {chunks_most} queued",
        CHUNK / 64,
        sizes[index],
        chunk_alone.as_micros(),
        per_chunk.as_micros()
    );
    let target = Offscreen::new(&gpu.device, 16, 16);
    let mut outcomes = Vec::new();
    for planned in [25, 50, 100, 100, 100].map(Duration::from_millis) {
        let planned = planned.min(PLANNED_MOST);
        let chunks = (planned.as_secs_f64() / per_chunk.as_secs_f64().max(1e-6)).ceil().clamp(1.0, f64::from(chunks_most)) as u32;
        renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
        let probe = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("present lock probe"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let submitted = rune_core::time::now();
        for _ in 0..chunks {
            submit(chunk);
        }
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = clear_pass(&mut encoder, &target.view, wgpu::Color::BLACK, renderer.timestamp_writes());
            renderer.draw(&mut pass, 0);
        }
        let (done, completed) = std::sync::mpsc::channel();
        encoder.on_submitted_work_done(move || {
            done.send(rune_core::time::now()).ok();
        });
        renderer.submit(&gpu.queue, encoder);
        // Wait until the worker has taken submit's wakeup and entered its polling loop.
        renderer.completion.wait_polling();
        std::thread::sleep(Duration::from_millis(1));
        let (outcome, finished) = if let Ok(at) = completed.try_recv() {
            ("finished before the probe", at)
        } else {
            probe.destroy();
            gpu.device.poll(wgpu::PollType::Poll).unwrap();
            match completed.try_recv() {
                Err(TryRecvError::Empty) => {
                    let at = gpu.wait(&completed, Duration::from_secs(30));
                    ("overlapped", at.unwrap_or_else(|| panic!("{chunks} submissions planned at {} us never completed", planned.as_micros())))
                }
                Ok(at) => ("finished by the time the write lock returned", at),
                Err(TryRecvError::Disconnected) => panic!("the GPU workload's callback was dropped"),
            }
        };
        // A lost device completes its submissions too, which is no overlap.
        gpu.assert_alive();
        eprintln!("overlap probe: {chunks} submissions planned at {} us, submit to done {} us, {outcome}", planned.as_micros(), (finished - submitted).as_micros());
        if outcome == "overlapped" {
            return;
        }
        // Submit to done bounds the chunks' GPU time: the next plan can only come out shorter.
        per_chunk = per_chunk.min((finished - submitted) / chunks);
        outcomes.push(outcome);
    }
    // Five attempts, none overlapping: either the write lock waited for the work every time (the
    // fault this test is for), or the work never stayed pending long enough to tell.
    panic!("on {name}, no attempt found the snatch write lock free while GPU work was pending: {outcomes:?}");
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

on_each_backend!(queued_video_frames_then_readback, queued_video_frames_then_readback_on_metal, queued_video_frames_then_readback_on);
fn queued_video_frames_then_readback_on(gpu: Gpu) {
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
                    done.send(t).ok();
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
                let mut pass =
                    clear_pass(&mut encoder, &target.view, wgpu::Color::TRANSPARENT, renderer.timestamp_writes());
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

on_each_backend!(video_gpu_colour_sharpness_geometry_and_lifetime, video_gpu_colour_sharpness_geometry_and_lifetime_on_metal, video_gpu_colour_sharpness_geometry_and_lifetime_on);
fn video_gpu_colour_sharpness_geometry_and_lifetime_on(gpu: Gpu) {
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
            let mut pass = clear_pass(&mut encoder, &target.view, wgpu::Color::TRANSPARENT, None);
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
    }
}

/// BGRA whose every pixel differs, so any column swap shows. Returns the source and its RGBA readback.
pub(super) fn bgra_ramp(width: u32, height: u32) -> (ByteFrame, Vec<[u8; 4]>) {
    let mut planes = Vec::new();
    let mut rgba = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let v = (x * 13 + y * 61) as u8;
            planes.extend_from_slice(&[v, 255 - v, (y * 40) as u8, 255]);
            rgba.push([(y * 40) as u8, 255 - v, v, 255]);
        }
    }
    (ByteFrame { width, height, format: Format::Bgra, color: ColorInfo::default(), planes: [planes, vec![]] }, rgba)
}
pub(super) fn pixel(p: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    p[i..i + 4].try_into().unwrap()
}
/// A mirrored draw must equal the unmirrored draw with each row reversed (any centred image).
fn assert_row_reversed(mirrored: &[u8], plain: &[u8], size: (u32, u32), what: &str) {
    for y in 0..size.1 {
        for x in 0..size.0 {
            let (a, b) = (pixel(mirrored, size.0, x, y), pixel(plain, size.0, size.0 - 1 - x, y));
            assert!(a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 1), "{what} ({x},{y}): {a:?} != {b:?}");
        }
    }
}

on_each_backend!(mirrored_video_is_exact_cropped_centred_and_toggles_without_a_new_frame, mirrored_video_on_metal, mirrored_video_is_exact_cropped_centred_and_toggles_without_a_new_frame_on);
fn mirrored_video_is_exact_cropped_centred_and_toggles_without_a_new_frame_on(gpu: Gpu) {
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut id = 0;
    for native in [false, cfg!(target_os = "macos")] {
        let surface = NativeSurface::new();
        surface.set_mirrored(true);
        let mut publish_next = |b: ByteFrame| {
            id += 1;
            publish(&surface, b, id, native);
        };

        // 1:1 BGRA: each one-pixel column lands exactly on its mirror, also when the node's
        // odd spare pixel snaps the image origin to x = 1.
        let (b, src) = bgra_ramp(8, 4);
        publish_next(b);
        for (node_w, origin) in [(8u32, 0u32), (9, 1)] {
            let p = render(&gpu, &mut renderer, &[draw(&surface, [0.0, 0.0, node_w as f32, 4.0])], (node_w, 4));
            for y in 0..4 {
                for x in origin..origin + 8 {
                    let expected = src[(y * 8 + (7 - (x - origin))) as usize];
                    assert_eq!(pixel(&p, node_w, x, y), expected, "native={native} 1:1 mirror at ({x},{y}) in {node_w}");
                }
            }
        }

        // 1:1 Fill crop of a 16x8 source into 8x8: columns 4..12 show, mirrored about the centre.
        let (b, src) = bgra_ramp(16, 8);
        publish_next(b);
        let mut d = draw(&surface, [0.0, 0.0, 8.0, 8.0]);
        d.fit = SurfaceFit::Fill;
        let p = render(&gpu, &mut renderer, &[d], (8, 8));
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(pixel(&p, 8, x, y), src[(y * 16 + 11 - x) as usize], "native={native} Fill crop ({x},{y})");
            }
        }

        // NV12 at 1:1: luma per pixel and each 2x2 chroma block move together.
        let (w, h) = (8u32, 4u32);
        let luma: Vec<u8> = (0..w * h).map(|i| (16 + i * 6) as u8).collect();
        let chroma: Vec<u8> = (0..w * h / 4).flat_map(|i| [(40 + i * 20) as u8, (200 - i * 18) as u8]).collect();
        let color = ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Srgb };
        publish_next(ByteFrame {
            width: w,
            height: h,
            format: Format::Nv12Full,
            color,
            planes: [luma.clone(), chroma.clone()],
        });
        let p = render(&gpu, &mut renderer, &[draw(&surface, [0.0, 0.0, w as f32, h as f32])], (w, h));
        for y in 0..h {
            for x in 0..w {
                let sx = w - 1 - x;
                let c = ((y / 2) * (w / 2) + sx / 2) as usize * 2;
                let reference =
                    nv12_reference(luma[(y * w + sx) as usize], chroma[c], chroma[c + 1], Format::Nv12Full, color)
                        .map(srgb);
                let got = pixel(&p, w, x, y);
                for i in 0..3 {
                    assert!(got[i].abs_diff(reference[i]) <= 1, "native={native} NV12 ({x},{y}): {got:?} != {reference:?}");
                }
            }
        }

        // Filtered paths: bilinear Fill (2x, cropped) and the area-grid shrink, BGRA and NV12.
        let (bgra, _) = bgra_ramp(16, 8);
        for (frame, rect, fit, size, what) in [
            (bgra, [0.0, 0.0, 16.0, 16.0], SurfaceFit::Fill, (16, 16), "BGRA bilinear Fill"),
            (synthetic(32, 16, 3), [0.0, 0.0, 24.0, 20.0], SurfaceFit::Fill, (24, 20), "NV12 bilinear Fill"),
            (synthetic(128, 32, 5), [0.0, 0.0, 8.0, 8.0], SurfaceFit::Fit, (8, 8), "NV12 area shrink"),
        ] {
            publish_next(frame);
            let mut d = draw(&surface, rect);
            d.fit = fit;
            surface.set_mirrored(false);
            let plain = render(&gpu, &mut renderer, &[d.clone()], size);
            surface.set_mirrored(true);
            let mirrored = render(&gpu, &mut renderer, &[d], size);
            assert_ne!(plain, mirrored, "{what}: the source must not be symmetric");
            assert_row_reversed(&mirrored, &plain, size, &format!("native={native} {what}"));
        }

        // Rounded corners, clips and opacity stay put in screen space.
        let (b, _) = bgra_ramp(16, 16);
        publish_next(b);
        let mut d = draw(&surface, [0.0, 0.0, 16.0, 16.0]);
        d.radii = [8.0, 0.0, 0.0, 0.0];
        d.clip[2] = 12.0;
        d.opacity = 0.5;
        let p = render(&gpu, &mut renderer, &[d], (16, 16));
        assert_eq!(pixel(&p, 16, 0, 0)[3], 0, "top-left radius");
        assert_eq!(pixel(&p, 16, 15, 0)[3], 0, "right clip");
        assert!(pixel(&p, 16, 11, 0)[3].abs_diff(128) <= 1, "opacity");
    }

    // Toggling wakes once per change and redraws the same cached frame, with no new push.
    let surface = NativeSurface::new();
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = wakes.clone();
    surface.set_waker(Arc::new(move || {
        w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let (b, src) = bgra_ramp(8, 2);
    surface.push_bytes(b, 1).unwrap();
    let woken = wakes.load(std::sync::atomic::Ordering::SeqCst);
    let d = [draw(&surface, [0.0, 0.0, 8.0, 2.0])];
    let plain = render(&gpu, &mut renderer, &d, (8, 2));
    assert_eq!(pixel(&plain, 8, 0, 0), src[0]);
    let clone = surface.clone();
    clone.set_mirrored(true);
    clone.set_mirrored(true);
    assert!(surface.is_mirrored(), "clones share the flag");
    assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), woken + 1, "one wake per change");
    let mirrored = render(&gpu, &mut renderer, &d, (8, 2));
    assert_eq!(pixel(&mirrored, 8, 0, 0), src[7]);
    assert_row_reversed(&mirrored, &plain, (8, 2), "runtime toggle");
    surface.set_mirrored(false);
    assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), woken + 2);
    assert_eq!(render(&gpu, &mut renderer, &d, (8, 2)), plain, "unmirroring restores the frame");
    assert_eq!(surface.latest().unwrap().id, 1, "no new frame was pushed");
}

const ORIENTS: [[i8; 4]; 8] =
    [[1, 0, 0, 1], [-1, 0, 0, 1], [1, 0, 0, -1], [-1, 0, 0, -1], [0, 1, 1, 0], [0, -1, 1, 0], [0, 1, -1, 0], [0, -1, -1, 0]];

/// The buffer pixel an upright pixel shows, by rotating about the centre.
fn buffer_pixel(o: Orient, [w, h]: [u32; 2], x: u32, y: u32) -> (u32, u32) {
    let [uw, uh] = o.upright([w, h]);
    let (cx, cy) = (x as f64 + 0.5 - uw as f64 / 2.0, y as f64 + 0.5 - uh as f64 / 2.0);
    let m = o.0.map(f64::from);
    ((m[0] * cx + m[1] * cy + w as f64 / 2.0).floor() as u32, (m[2] * cx + m[3] * cy + h as f64 / 2.0).floor() as u32)
}
/// The frame turned upright on the CPU.
fn turned(b: &ByteFrame, o: Orient) -> ByteFrame {
    let plane = |p: &[u8], w: u32, h: u32, bpp: usize| {
        let [uw, uh] = o.upright([w, h]);
        let mut out = Vec::with_capacity(p.len());
        for y in 0..uh {
            for x in 0..uw {
                let (bx, by) = buffer_pixel(o, [w, h], x, y);
                let i = (by * w + bx) as usize * bpp;
                out.extend_from_slice(&p[i..i + bpp]);
            }
        }
        out
    };
    let [w, h] = [b.width, b.height];
    let planes = match b.format {
        Format::Bgra => [plane(&b.planes[0], w, h, 4), vec![]],
        _ => [plane(&b.planes[0], w, h, 1), plane(&b.planes[1], w / 2, h / 2, 2)],
    };
    let [width, height] = o.upright([w, h]);
    ByteFrame { width, height, format: b.format, color: b.color, planes }
}
fn publish_oriented(s: &NativeSurface, b: ByteFrame, id: u64, native: bool, o: Orient) {
    #[cfg(target_os = "macos")]
    if native {
        s.push_oriented(macos::PixelBuffer::from_bytes(&b).unwrap(), id, o).unwrap();
        return;
    }
    let _ = native;
    s.push_bytes_oriented(b, id, o).unwrap();
}

#[test]
fn orientations_come_from_quarter_turn_transforms_only() {
    // An iPhone portrait clip: buffer (x, y) shows at (h - y, x).
    assert_eq!(Orient::from_transform(0.0, 1.0, -1.0, 0.0), Orient([0, 1, -1, 0]));
    assert_eq!(Orient::from_transform(1.0, 0.0, 0.0, 1.0), Orient::UP);
    assert_eq!(Orient::from_transform(-1.0, 0.0, 0.0, -1.0), Orient([-1, 0, 0, -1]));
    let s = std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(Orient::from_transform(s, s, -s, s), Orient::UP, "45 degrees isn't a quarter turn");
    assert_eq!(Orient::from_transform(2.0, 0.0, 0.0, 2.0), Orient::UP, "scales are ignored");
    assert!(Orient([0, 1, -1, 0]).swaps());
    assert_eq!(Orient([0, 1, -1, 0]).upright([64, 32]), [32, 64]);
    // The buffer's top-left pixel ends up top-right, as it does on the phone.
    assert_eq!(buffer_pixel(Orient([0, 1, -1, 0]), [6, 4], 3, 0), (0, 0));
}

// A buffer drawn with an orientation equals the same picture turned upright on the CPU, for
// every quarter turn and flip, at 1:1, magnified and shrunk, BGRA and NV12.
on_each_backend!(oriented_frames_draw_like_frames_turned_upright, oriented_frames_on_metal, oriented_frames_draw_like_frames_turned_upright_on);
fn oriented_frames_draw_like_frames_turned_upright_on(gpu: Gpu) {
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut id = 0;
    for native in [false, cfg!(target_os = "macos")] {
        for m in ORIENTS {
            let o = Orient(m);
            for frame in [bgra_ramp(16, 8).0, synthetic(32, 16, 3)] {
                let upright = turned(&frame, o);
                let [uw, uh] = [upright.width, upright.height];
                let (oriented, reference) = (NativeSurface::new(), NativeSurface::new());
                id += 1;
                publish_oriented(&oriented, frame, id, native, o);
                publish(&reference, upright, id, native);
                for (w, h, what) in [(uw, uh, "1:1"), (uw * 2, uh * 2, "2x"), (uw / 2, uh / 2, "half")] {
                    let rect = [0.0, 0.0, w as f32, h as f32];
                    let got = render(&gpu, &mut renderer, &[draw(&oriented, rect)], (w, h));
                    let want = render(&gpu, &mut renderer, &[draw(&reference, rect)], (w, h));
                    for (i, (g, r)) in got.iter().zip(&want).enumerate() {
                        assert!(g.abs_diff(*r) <= 1, "native={native} {m:?} {what} byte {i}: {g} != {r}");
                    }
                }
            }
        }
    }
}

// A portrait clip stored sideways (display matrix rotate=90): its player buffers, drawn with
// the track's orientation, match the upright poster AVFoundation decodes, quadrant by quadrant.
#[cfg(target_os = "macos")]
on_each_backend!(rotated_clip_buffers_draw_upright_like_its_poster, rotated_clip_on_metal, rotated_clip_buffers_draw_upright_like_its_poster_on);
#[cfg(target_os = "macos")]
fn rotated_clip_buffers_draw_upright_like_its_poster_on(gpu: Gpu) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures/quadrants-rot90.mp4");
    let (upright, raw, o) = crate::video::test_frames(&crate::MediaSource::File(path), 0.125).unwrap();
    assert_eq!((raw.width, raw.height), (64, 32), "stored landscape");
    assert_eq!((upright.width, upright.height), (32, 64), "shown portrait");
    assert!(o.swaps(), "{o:?}");
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    for native in [false, true] {
        let (oriented, poster) = (NativeSurface::new(), NativeSurface::new());
        publish_oriented(&oriented, ByteFrame { planes: raw.planes.clone(), ..raw }, 1, native, o);
        publish(&poster, ByteFrame { planes: upright.planes.clone(), ..upright }, 1, native);
        for scale in [1, 2] {
            let (w, h) = (32 / scale, 64 / scale);
            let rect = [0.0, 0.0, w as f32, h as f32];
            let got = render(&gpu, &mut renderer, &[draw(&oriented, rect)], (w, h));
            let want = render(&gpu, &mut renderer, &[draw(&poster, rect)], (w, h));
            // Inside each quadrant, clear of chroma bleed at its edges.
            let mut colours = Vec::new();
            for (qx, qy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (x, y) = ((qx * 16 + 8) / scale, (qy * 32 + 16) / scale);
                let (g, r) = (pixel(&got, w, x, y), pixel(&want, w, x, y));
                assert!(g.iter().zip(r).all(|(g, r)| g.abs_diff(r) <= 8), "native={native} 1/{scale} quadrant ({qx},{qy}): {g:?} != {r:?}");
                colours.push(r);
            }
            colours.dedup();
            assert_eq!(colours.len(), 4, "four distinct quadrants: {colours:?}");
        }
    }
}
