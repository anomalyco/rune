use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{INITIAL_RETRY, MAX_RETRY, Retire, RetireEffect, RetireEvent};
use crate::overlay::{AppSpec, Overlay, OverlaySpec};
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::{Cx, El, Pixels};

#[test]
fn retire_state_machine_backs_off_and_clears_on_external_completion() {
    let start = crate::time::now();
    let mut retire: Retire<()> = Retire::default();
    assert_eq!(retire.deadline(), None);

    let done = Arc::new(AtomicBool::new(false));
    assert!(retire.step(RetireEvent::Busy { gpu: (), done: done.clone(), now: start }).is_empty());
    let mut due = start + INITIAL_RETRY;
    assert_eq!(retire.deadline(), Some(due));

    // Before due, nothing polls.
    assert!(retire.step(RetireEvent::Due { now: start + Duration::from_millis(8) }).is_empty());

    // At each due time while still busy, polls once and doubles the delay up to MAX_RETRY.
    let mut expected_delay = INITIAL_RETRY;
    for _ in 0..6 {
        let effects: Vec<_> = retire.step(RetireEvent::Due { now: due }).into_iter().collect();
        assert_eq!(effects, vec![RetireEffect::PollNow]);
        assert!(retire.step(RetireEvent::StillBusy { now: due }).is_empty());
        expected_delay = (expected_delay * 2).min(MAX_RETRY);
        due += expected_delay;
        assert_eq!(retire.deadline(), Some(due));
    }
    assert_eq!(expected_delay, MAX_RETRY);

    // Another surface's submit retiring the submission flips `done`: deadline clears immediately
    // and the next Due skips polling.
    done.store(true, Ordering::Release);
    assert_eq!(retire.deadline(), None);
    assert!(retire.step(RetireEvent::Due { now: due }).is_empty());
    assert!(matches!(retire, Retire::Idle));
}

#[test]
fn retire_state_machine_clears_on_finished() {
    let start = crate::time::now();
    let mut retire: Retire<()> = Retire::default();
    let done = Arc::new(AtomicBool::new(false));
    retire.step(RetireEvent::Busy { gpu: (), done, now: start });
    assert!(retire.deadline().is_some());
    retire.clear();
    assert_eq!(retire.deadline(), None);
    assert!(matches!(retire, Retire::Idle));
}

/// Whether a software adapter probe showed a genuine asynchronous busy window: work remained
/// in flight after an immediate non-blocking poll, at least half of `planned` stayed pending
/// after `queue.submit` returned, and `queue.submit` itself did not consume more time than the
/// remaining in-flight work.
fn has_async_window(
    in_flight: bool,
    submit_elapsed: Duration,
    remaining: Duration,
    planned: Duration,
) -> bool {
    in_flight && remaining >= planned / 2 && submit_elapsed < remaining
}

#[test]
fn has_async_window_separates_async_software_queues_from_inline_cpu_vulkan() {
    let planned = Duration::from_millis(100);
    // Async software queue (e.g. DX12 WARP): submit returns quickly, ~100 ms remains in flight.
    assert!(has_async_window(true, Duration::from_millis(3), Duration::from_millis(95), planned));
    // Synchronous CPU Vulkan (e.g. llvmpipe): submit ran inline and Poll saw work done.
    assert!(!has_async_window(false, Duration::from_millis(120), Duration::from_millis(1), planned));
    // Shallow CPU queue (e.g. llvmpipe draining chunks during submit): submit took ~120 ms and
    // only one chunk (~15 ms) remained.
    assert!(!has_async_window(true, Duration::from_millis(120), Duration::from_millis(15), planned));
    // Slow CPU VM where submit blocked for half the work: submit_elapsed >= remaining.
    assert!(!has_async_window(true, Duration::from_millis(60), Duration::from_millis(60), planned));
}

struct BusyGpu {
    gpu: Arc<Gpu>,
    compute: wgpu::ComputePipeline,
    chunk: wgpu::BindGroup,
    per_chunk: Duration,
    chunks_most: u32,
    async_window: bool,
}

impl BusyGpu {
    const INVOCATIONS: u32 = 1024 * 64;
    const CHUNK: u32 = 16_384;
    const PLANNED_MOST: Duration = Duration::from_millis(100);
    const CHUNKS_MOST: u32 = 512;
    const QUEUED_MOST: Duration = Duration::from_secs(2);

    fn try_new() -> Option<Self> {
        let gpu = Arc::new(Gpu::test_headless()?);
        let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay busy workload"),
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
                "#.into(),
            ),
        });
        let compute = gpu.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("overlay busy workload"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: u64::from(Self::INVOCATIONS) * 4,
            usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false,
        });
        let bind_for = |iterations: u32| {
            let work = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay busy iterations"), size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
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
        let binds = [Self::CHUNK / 64, Self::CHUNK / 16, Self::CHUNK / 4, Self::CHUNK].map(bind_for);
        let submit = |bind: &wgpu::BindGroup| {
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&compute);
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(Self::INVOCATIONS / 64, 1, 1);
            }
            gpu.queue.submit([enc.finish()])
        };
        let time = |bind: &wgpu::BindGroup| {
            let started = crate::time::now();
            let index = submit(bind);
            gpu.device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: Some(Duration::from_secs(5)) })
                .expect("calibration submission completed");
            assert!(!gpu.is_lost(), "GPU lost during calibration");
            started.elapsed()
        };
        time(&binds[0]);
        let floor = time(&binds[0]);
        let software = gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu;
        let mut step = (0, floor);
        while !software && step.0 + 1 < binds.len() && step.1 * 4 <= Self::PLANNED_MOST {
            step = (step.0 + 1, time(&binds[step.0 + 1]));
        }
        let (index, chunk_alone) = step;
        let per_chunk = if index == 0 { chunk_alone } else { chunk_alone.saturating_sub(floor).max(chunk_alone / 64) };
        let chunks_most = ((Self::QUEUED_MOST.as_secs_f64() / chunk_alone.as_secs_f64().max(1e-6)) as u32).clamp(1, Self::CHUNKS_MOST);
        let mut busy = BusyGpu {
            gpu,
            compute,
            chunk: binds.into_iter().nth(index).unwrap(),
            per_chunk,
            chunks_most,
            async_window: true,
        };
        if software {
            busy.async_window = busy.probe_async_window();
        }
        Some(busy)
    }

    /// Probes a software adapter (`DeviceType::Cpu`) to see whether `submit_busy` leaves a
    /// controlled in-flight window past an immediate non-blocking `PollType::Poll` (true on
    /// async software queues such as DX12 WARP; false on synchronous or shallow-queue CPU
    /// Vulkan drivers such as Mesa llvmpipe).
    fn probe_async_window(&self) -> bool {
        let submit_start = crate::time::now();
        let (_, done) = self.submit_busy(Self::PLANNED_MOST);
        let submit_elapsed = submit_start.elapsed();
        let after_submit = crate::time::now();
        rune_gpu::health::poll(&self.gpu.device, wgpu::PollType::Poll).ok();
        let in_flight = !done.load(Ordering::Acquire);
        self.drain();
        let remaining = after_submit.elapsed();
        has_async_window(in_flight, submit_elapsed, remaining, Self::PLANNED_MOST)
    }

    fn attempts(&self) -> &'static [Duration] {
        const RETRIES: [Duration; 5] = [
            Duration::from_millis(25),
            Duration::from_millis(50),
            Duration::from_millis(100),
            Duration::from_millis(100),
            Duration::from_millis(100),
        ];
        if self.async_window { &RETRIES } else { &RETRIES[..1] }
    }

    fn submit_busy(&self, planned: Duration) -> (u32, Arc<AtomicBool>) {
        let planned = planned.min(Self::PLANNED_MOST);
        let chunks = (planned.as_secs_f64() / self.per_chunk.as_secs_f64().max(1e-6))
            .ceil().clamp(1.0, f64::from(self.chunks_most)) as u32;
        for _ in 0..chunks {
            let mut enc = self.gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.compute);
                pass.set_bind_group(0, &self.chunk, &[]);
                pass.dispatch_workgroups(Self::INVOCATIONS / 64, 1, 1);
            }
            self.gpu.queue.submit([enc.finish()]);
        }
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        self.gpu.queue.on_submitted_work_done(move || flag.store(true, Ordering::Release));
        (chunks, done)
    }

    fn drain(&self) {
        rune_gpu::health::poll(&self.gpu.device, wgpu::PollType::wait_indefinitely()).ok();
        assert!(!self.gpu.is_lost(), "GPU lost during test");
    }
}

#[test]
fn drop_overlay_core_polls_shared_device_without_blocking() {
    fn view(_: &(), _: &mut Cx<()>) -> El { crate::el().size(16.0, 16.0) }
    let app = AppSpec::new("drop-poll", (16.0, 16.0), || (), view);
    let make = || Overlay::new(OverlaySpec::new("drop-poll", app.size, view).options(Default::default()), (), &app).0;

    let Some(mut busy) = BusyGpu::try_new() else { return };
    busy.drain();

    // Idle shared device: Drop drops the renderer then polls non-blockingly so queued work retires.
    let mut overlay = make();
    overlay.core.gpu = Some(busy.gpu.clone());
    overlay.core.renderer = Some(Renderer::new(&busy.gpu.device, OFFSCREEN_FORMAT));
    // Flush any mapped_at_creation buffer uploads queued by Renderer::new so the queue is truly idle.
    busy.gpu.queue.submit(std::iter::empty());
    busy.drain();
    let polled = Arc::new(AtomicBool::new(false));
    let flag = polled.clone();
    busy.gpu.queue.on_submitted_work_done(move || flag.store(true, Ordering::Release));
    assert!(!polled.load(Ordering::Acquire), "callback waits for a device poll");
    drop(overlay);
    assert!(polled.load(Ordering::Acquire), "Drop for OverlayCore must poll a shared device after dropping its renderer");

    // Busy shared device: Drop returns without waiting for in-flight work.
    let mut overlapped = false;
    for &planned in busy.attempts() {
        let mut overlay = make();
        overlay.core.gpu = Some(busy.gpu.clone());
        overlay.core.renderer = Some(Renderer::new(&busy.gpu.device, OFFSCREEN_FORMAT));
        let started = crate::time::now();
        let (chunks, sibling_done) = busy.submit_busy(planned);
        drop(overlay);
        assert!(!busy.gpu.is_lost());
        if !sibling_done.load(Ordering::Acquire) {
            overlapped = true;
            busy.drain();
            break;
        }
        busy.per_chunk = busy.per_chunk.min(started.elapsed() / chunks.max(1));
    }
    if !overlapped {
        busy.drain();
        assert!(!busy.async_window, "Drop for OverlayCore must not wait for in-flight work on a shared GPU");
        eprintln!(
            "note: software adapter ({}) could not keep queued work in flight; skipping busy-overlap assertion",
            busy.gpu.adapter.get_info().name,
        );
    }
}

#[test]
fn drop_hidden_images_and_park_do_not_block_on_busy_shared_gpu_and_retire_on_later_turn() {
    fn view(px: &Option<Pixels>, _: &mut Cx<Option<Pixels>>) -> El {
        let mut root = crate::el().size(16.0, 16.0);
        if let Some(px) = px { root = root.child(crate::el().key("shot").size(4.0, 4.0).pixels(px)); }
        root
    }
    let app = AppSpec::new("busy-hide", (16.0, 16.0), || None::<Pixels>, view);
    let Some(mut busy) = BusyGpu::try_new() else { return };

    // 1. drop_hidden_images on a busy shared device returns immediately and retires on later wake_due.
    let mut overlapped = false;
    for &planned in busy.attempts() {
        let px = Pixels::new();
        px.set(4, 4, [200, 40, 20, 255].repeat(16));
        let weak_px = Arc::downgrade(&px.source);
        let (mut overlay, _) = Overlay::new(OverlaySpec::new("busy-hide", app.size, view).options(Default::default()), Some(px), &app);
        overlay.core.gpu = Some(busy.gpu.clone());
        overlay.core.renderer = Some(Renderer::new(&busy.gpu.device, OFFSCREEN_FORMAT));
        let target = Offscreen::new(&busy.gpu.device, 16, 16);
        busy.drain();
        #[cfg(feature = "test-gpu-counters")]
        let base_textures = busy.gpu.device.get_internal_counters().hal.textures.read();
        overlay.rt.frame(0.0);
        let mut enc = busy.gpu.device.create_command_encoder(&Default::default());
        overlay.render_offscreen(&busy.gpu, &mut enc, &target.view, OFFSCREEN_FORMAT, (16, 16), 0.0);
        overlay.core.renderer.as_mut().unwrap().submit(&busy.gpu.queue, enc);
        let frame_done = Arc::new(AtomicBool::new(false));
        let ff = frame_done.clone();
        busy.gpu.queue.on_submitted_work_done(move || ff.store(true, Ordering::Release));
        assert!(overlay.core.renderer.as_ref().unwrap().has_images());
        #[cfg(feature = "test-gpu-counters")]
        assert!(busy.gpu.device.get_internal_counters().hal.textures.read() > base_textures);

        let started = crate::time::now();
        let (chunks, sibling_done) = busy.submit_busy(planned);
        overlay.drop_hidden_images();
        overlay.rt.state = None;
        assert!(!busy.gpu.is_lost());
        assert!(overlay.rt.images.pages.is_empty());
        assert!(!overlay.core.renderer.as_ref().unwrap().has_images());
        assert!(weak_px.upgrade().is_none(), "tree no longer pins Pixels after hide");
        if !sibling_done.load(Ordering::Acquire) {
            assert!(overlay.wake_deadline().is_some(), "busy shared GPU schedules deferred retirement");
            let wait = crate::time::now();
            while let Some(due) = overlay.wake_deadline() {
                assert!(wait.elapsed() < Duration::from_secs(5), "deferred retirement did not finish");
                std::thread::sleep(Duration::from_millis(2));
                overlay.wake_due(due);
            }
            assert!(frame_done.load(Ordering::Acquire), "overlay frame submission retired via wake_due");
            assert!(sibling_done.load(Ordering::Acquire), "shared queue retired via wake_due");
            #[cfg(feature = "test-gpu-counters")]
            assert_eq!(busy.gpu.device.get_internal_counters().hal.textures.read(), base_textures,
                "GPU image texture freed once in-flight work completes");
            overlapped = true;
            break;
        }
        if !busy.async_window {
            busy.drain();
            assert!(overlay.wake_deadline().is_none(), "drained queue leaves no pending retirement");
            assert!(frame_done.load(Ordering::Acquire), "overlay frame submission retired");
            assert!(sibling_done.load(Ordering::Acquire), "shared queue retired");
            #[cfg(feature = "test-gpu-counters")]
            assert_eq!(busy.gpu.device.get_internal_counters().hal.textures.read(), base_textures,
                "GPU image texture freed once work completes");
            break;
        }
        busy.per_chunk = busy.per_chunk.min(started.elapsed() / chunks.max(1));
    }
    if !overlapped {
        assert!(!busy.async_window, "drop_hidden_images must not wait for in-flight work on a shared GPU");
        eprintln!(
            "note: software adapter ({}) could not keep queued work in flight; skipping drop_hidden_images overlap assertion",
            busy.gpu.adapter.get_info().name,
        );
    }

    // 2. park (with renderer, and without renderer) on a busy shared device returns immediately
    //    without pinning the shared Arc<Gpu>, and retires on later wake_due.
    for with_renderer in [true, false] {
        let mut overlapped = false;
        for &planned in busy.attempts() {
            busy.drain();
            #[cfg(feature = "test-gpu-counters")]
            let base_textures = busy.gpu.device.get_internal_counters().hal.textures.read();
            let spec = OverlaySpec::new("busy-park", app.size, view).options(Default::default()).release_gpu_when_hidden();
            let (mut overlay, _) = Overlay::new(spec, None, &app);
            overlay.core.gpu = Some(busy.gpu.clone());
            if with_renderer {
                overlay.core.renderer = Some(Renderer::new(&busy.gpu.device, OFFSCREEN_FORMAT));
            }
            let started = crate::time::now();
            let (chunks, sibling_done) = busy.submit_busy(planned);
            overlay.core.park();
            assert!(!busy.gpu.is_lost());
            assert!(overlay.core.gpu.is_none());
            assert!(overlay.core.renderer.is_none());
            assert_eq!(Arc::strong_count(&busy.gpu), 1, "park releases strong Arc<Gpu> even while retire is pending");
            if !sibling_done.load(Ordering::Acquire) {
                assert!(overlay.wake_deadline().is_some(), "busy park schedules deferred retirement");
                let wait = crate::time::now();
                while let Some(due) = overlay.wake_deadline() {
                    assert!(wait.elapsed() < Duration::from_secs(5), "deferred park retirement did not finish");
                    std::thread::sleep(Duration::from_millis(2));
                    overlay.wake_due(due);
                }
                assert!(sibling_done.load(Ordering::Acquire), "park retired shared queue via wake_due");
                #[cfg(feature = "test-gpu-counters")]
                assert_eq!(busy.gpu.device.get_internal_counters().hal.textures.read(), base_textures,
                    "parked renderer textures freed once in-flight work completes");
                overlapped = true;
                break;
            }
            if !busy.async_window {
                busy.drain();
                assert!(overlay.wake_deadline().is_none(), "drained park leaves no pending retirement");
                assert!(sibling_done.load(Ordering::Acquire), "shared queue retired");
                #[cfg(feature = "test-gpu-counters")]
                assert_eq!(busy.gpu.device.get_internal_counters().hal.textures.read(), base_textures,
                    "parked renderer textures freed once work completes");
                break;
            }
            busy.per_chunk = busy.per_chunk.min(started.elapsed() / chunks.max(1));
        }
        if !overlapped {
            assert!(!busy.async_window, "park (with_renderer={with_renderer}) must not wait for in-flight work on a shared GPU");
            eprintln!(
                "note: software adapter ({}) could not keep queued work in flight; skipping park overlap assertion (with_renderer={with_renderer})",
                busy.gpu.adapter.get_info().name,
            );
        }
    }
}
