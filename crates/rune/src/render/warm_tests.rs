//! A device that died while it waited for a surface (Windows' Modern Standby, a driver reset) is
//! not handed to the next one: a lost device configures nothing, and wgpu panics on the drawable
//! its surface is asked for.

#[cfg(all(target_os = "linux", feature = "test-gpu-noop"))]
mod lost {
    use crate::render::prewarm::{PARKED, discard, park, take};
    use crate::render::{Gpu, Renderer};

    #[test]
    fn a_parked_device_that_died_is_not_handed_to_the_next_surface() {
        discard();
        let gpu = Gpu::test_noop();
        let renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
        assert!(!gpu.is_lost());
        gpu.device.destroy();
        // wgpu loses a destroyed device once its queues drain, when the device next maintains itself.
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        assert!(gpu.is_lost(), "the device's lost callback marks it");
        park(gpu, renderer, PARKED);
        assert!(take().is_none(), "a lost device waits for no surface");
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod warm_renderer {
    use crate::render::{Gpu, Renderer};

    #[test]
    fn warm_renderer_transfers_across_threads_and_rejects_surfaces() {
        let Some(gpu) = Gpu::test_headless_or_noop() else { return };
        let device = gpu.device.clone();
        let warm = std::thread::spawn(move || Renderer::new_warm(&device, wgpu::TextureFormat::Bgra8Unorm))
            .join()
            .expect("warm thread");
        let renderer = Renderer::from_warm(warm);
        assert_eq!(renderer.format(), wgpu::TextureFormat::Bgra8Unorm);

        #[cfg(not(target_os = "macos"))]
        {
            let warm = renderer.into_warm().expect("new renderer has no native surface renderer");
            let mut renderer = std::thread::spawn(move || Renderer::from_warm(warm))
                .join()
                .expect("attach thread");
            let draw = crate::native_surface::SurfaceDraw {
                surface: crate::native_surface::NativeSurface::new(),
                fit: crate::native_surface::SurfaceFit::Fill,
                before_instance: 0,
                rect: [0.0, 0.0, 16.0, 16.0],
                radii: [0.0; 4],
                clip: crate::render::NO_CLIP,
                fade: [0; 4],
                opacity: 1.0,
            };
            renderer.prepare_surfaces(&gpu.device, &gpu.queue, &[draw], (16, 16));
            assert!(renderer.into_warm().is_none(), "renderer with native surfaces must not convert to WarmRenderer");

            let mut direct = Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
            let surface = crate::native_surface::NativeSurface::new();
            assert!(direct.surface_texture(&gpu.device, &gpu.queue, &surface).unwrap().is_none());
            assert!(direct.into_warm().is_none(), "renderer with surface_importer must not convert to WarmRenderer");
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod shutdown_machine {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use crate::render::warm_machine::{
        Clock, Effect, Event, SHUTDOWN_TIMEOUT, ShutdownOutcome, State, SystemClock, WarmSlot, step,
    };
    use crate::time::Instant;

    struct FakeClock(Mutex<Instant>);
    impl FakeClock {
        fn new(at: Instant) -> Self { Self(Mutex::new(at)) }
        fn advance(&self, by: Duration) { *self.0.lock().unwrap() += by; }
    }
    impl Clock for FakeClock {
        fn now(&self) -> Instant { *self.0.lock().unwrap() }
    }

    #[test]
    fn state_machine_table_covers_all_five_states_with_injected_clock() {
        let t0 = crate::time::now();
        let t1 = t0 + Duration::from_secs(1);
        let t2 = t0 + Duration::from_secs(2);
        let t30 = t0 + Duration::from_secs(30);
        let t40 = t0 + Duration::from_secs(40);

        let cases: [(&str, State, Event, State, Vec<Effect>); 24] = [
            (
                "not started -> running on start",
                State::NotStarted { draining: 0 },
                Event::Start { until: t30 },
                State::Running { until: t30, workers: 1 },
                vec![Effect::SpawnWorker],
            ),
            (
                "running extends linger deadline without respawning",
                State::Running { until: t30, workers: 1 },
                Event::Start { until: t40 },
                State::Running { until: t40, workers: 1 },
                vec![],
            ),
            (
                "build finished ok before deadline -> parked",
                State::Running { until: t30, workers: 1 },
                Event::BuildFinished { ok: true, now: t1 },
                State::Parked { until: t30, workers: 1 },
                vec![Effect::StorePayload, Effect::NotifyWaiters],
            ),
            (
                "build finished after discard expiry -> drops payload",
                State::Running { until: t1, workers: 1 },
                Event::BuildFinished { ok: true, now: t2 },
                State::NotStarted { draining: 1 },
                vec![Effect::DropPayload, Effect::NotifyWaiters],
            ),
            (
                "parked -> running during warm frame",
                State::Parked { until: t30, workers: 2 },
                Event::BeginWarmFrame,
                State::Running { until: t30, workers: 2 },
                vec![Effect::TakePayload, Effect::NotifyWaiters],
            ),
            (
                "finish warm frame ok -> re-parks and notifies done",
                State::Running { until: t30, workers: 1 },
                Event::FinishWarmFrame { ok: true, now: t1 },
                State::Parked { until: t30, workers: 1 },
                vec![Effect::StorePayload, Effect::NotifyWaiters, Effect::NotifyDone],
            ),
            (
                "finish warm frame while cancelled -> drops payload and skips done",
                State::Cancelled { deadline: Some(t2), workers: 1 },
                Event::FinishWarmFrame { ok: true, now: t1 },
                State::Cancelled { deadline: Some(t2), workers: 1 },
                vec![Effect::DropPayload],
            ),
            (
                "take from parked claims payload",
                State::Parked { until: t30, workers: 1 },
                Event::Take,
                State::NotStarted { draining: 1 },
                vec![Effect::TakePayload, Effect::NotifyWaiters],
            ),
            (
                "parked tick past deadline expires and drops payload",
                State::Parked { until: t30, workers: 1 },
                Event::Tick { now: t30 },
                State::NotStarted { draining: 1 },
                vec![Effect::DropPayload, Effect::NotifyWaiters],
            ),
            (
                "shutdown when idle with zero workers -> done immediately",
                State::NotStarted { draining: 0 },
                Event::Shutdown { deadline: t2 },
                State::Done { timed_out_workers: 0 },
                vec![Effect::NotifyWaiters],
            ),
            (
                "early cancel while idle with zero workers -> done immediately",
                State::NotStarted { draining: 0 },
                Event::Cancel,
                State::Done { timed_out_workers: 0 },
                vec![Effect::NotifyWaiters],
            ),
            (
                "early cancel while running -> cancelled with no bound yet",
                State::Running { until: t30, workers: 2 },
                Event::Cancel,
                State::Cancelled { deadline: None, workers: 2 },
                vec![Effect::NotifyWaiters],
            ),
            (
                "early cancel while parked -> drops payload with no bound yet",
                State::Parked { until: t30, workers: 1 },
                Event::Cancel,
                State::Cancelled { deadline: None, workers: 1 },
                vec![Effect::DropPayload, Effect::NotifyWaiters],
            ),
            (
                "tick while early-cancelled (no deadline yet) does not time out",
                State::Cancelled { deadline: None, workers: 1 },
                Event::Tick { now: t40 },
                State::Cancelled { deadline: None, workers: 1 },
                vec![],
            ),
            (
                "shutdown installs bound on early-cancelled state",
                State::Cancelled { deadline: None, workers: 1 },
                Event::Shutdown { deadline: t30 },
                State::Cancelled { deadline: Some(t30), workers: 1 },
                vec![Effect::NotifyWaiters],
            ),
            (
                "shutdown when idle with draining worker -> cancelled",
                State::NotStarted { draining: 1 },
                Event::Shutdown { deadline: t2 },
                State::Cancelled { deadline: Some(t2), workers: 1 },
                vec![Effect::NotifyWaiters],
            ),
            (
                "shutdown while running -> cancelled waiting on workers",
                State::Running { until: t30, workers: 2 },
                Event::Shutdown { deadline: t2 },
                State::Cancelled { deadline: Some(t2), workers: 2 },
                vec![Effect::NotifyWaiters],
            ),
            (
                "shutdown while parked -> drops payload and cancels worker",
                State::Parked { until: t30, workers: 1 },
                Event::Shutdown { deadline: t2 },
                State::Cancelled { deadline: Some(t2), workers: 1 },
                vec![Effect::DropPayload, Effect::NotifyWaiters],
            ),
            (
                "worker exit in cancelled decrements count",
                State::Cancelled { deadline: Some(t2), workers: 2 },
                Event::WorkerExited,
                State::Cancelled { deadline: Some(t2), workers: 1 },
                vec![],
            ),
            (
                "last worker exit in cancelled -> done drained",
                State::Cancelled { deadline: Some(t2), workers: 1 },
                Event::WorkerExited,
                State::Done { timed_out_workers: 0 },
                vec![Effect::NotifyWaiters],
            ),
            (
                "shutdown bound expires with wedged worker -> done timed out",
                State::Cancelled { deadline: Some(t2), workers: 1 },
                Event::Tick { now: t2 },
                State::Done { timed_out_workers: 1 },
                vec![Effect::NotifyWaiters],
            ),
            (
                "start rejected while cancelled",
                State::Cancelled { deadline: Some(t2), workers: 1 },
                Event::Start { until: t30 },
                State::Cancelled { deadline: Some(t2), workers: 1 },
                vec![],
            ),
            (
                "attach worker rejected while done",
                State::Done { timed_out_workers: 0 },
                Event::AttachWorker,
                State::Done { timed_out_workers: 0 },
                vec![],
            ),
            (
                "reopen cleanly drained slot -> not started",
                State::Done { timed_out_workers: 0 },
                Event::Reopen,
                State::NotStarted { draining: 0 },
                vec![],
            ),
        ];

        for (name, initial, event, expected_state, expected_fx) in cases {
            let mut state = initial;
            let fx: Vec<Effect> = step(&mut state, event).into_iter().collect();
            assert_eq!(state, expected_state, "state mismatch in case: {name}");
            assert_eq!(fx, expected_fx, "effects mismatch in case: {name}");
        }

        // Verify FakeClock drives WarmSlot::discard / shutdown with zero wall-clock wait.
        let clock = Arc::new(FakeClock::new(t0));
        struct SharedClock(Arc<FakeClock>);
        impl Clock for SharedClock {
            fn now(&self) -> Instant { self.0.now() }
        }
        let slot = WarmSlot::<u32, SharedClock>::new(SharedClock(Arc::clone(&clock)));
        assert_eq!(slot.shutdown(Duration::from_secs(2)), ShutdownOutcome::Drained);
        assert_eq!(slot.state(), State::Done { timed_out_workers: 0 });
        clock.advance(Duration::from_secs(5));
        slot.reopen();
        assert_eq!(slot.state(), State::NotStarted { draining: 0 });
    }

    struct Gate {
        state: Mutex<(bool, bool)>, // (entered, released)
        cv: Condvar,
    }
    impl Gate {
        fn new() -> Arc<Self> {
            Arc::new(Self { state: Mutex::new((false, false)), cv: Condvar::new() })
        }
        fn enter_only(&self) {
            let mut s = self.state.lock().unwrap();
            s.0 = true;
            self.cv.notify_all();
        }
        fn enter_and_wait(&self) {
            let mut s = self.state.lock().unwrap();
            s.0 = true;
            self.cv.notify_all();
            while !s.1 {
                s = self.cv.wait(s).unwrap();
            }
        }
        fn wait_entered(&self) {
            let mut s = self.state.lock().unwrap();
            while !s.0 {
                s = self.cv.wait(s).unwrap();
            }
        }
        fn release(&self) {
            let mut s = self.state.lock().unwrap();
            s.1 = true;
            self.cv.notify_all();
        }
    }

    #[test]
    fn shutdown_waits_for_blocking_warm_job_skips_callbacks_and_leaves_zero_threads() {
        let slot = Arc::new(WarmSlot::<u32, SystemClock>::new(SystemClock));
        let gate = Gate::new();
        let gate_worker = Arc::clone(&gate);
        let frame_called = Arc::new(AtomicBool::new(false));
        let done_called = Arc::new(AtomicBool::new(false));
        let frame_flag = Arc::clone(&frame_called);
        let done_flag = Arc::clone(&done_called);

        WarmSlot::warm_then(
            Arc::clone(&slot),
            Duration::from_secs(30),
            move || {
                gate_worker.enter_and_wait();
                Some(42u32)
            },
            move |v| {
                frame_flag.store(true, Ordering::SeqCst);
                Some(v)
            },
            move || {
                done_flag.store(true, Ordering::SeqCst);
            },
        )
        .expect("spawn warm_then");

        gate.wait_entered();
        assert!(slot.live_workers() >= 1, "warm-up worker must be tracked as live");

        // Start shutdown on another thread and confirm it enters Cancelled and does NOT return
        // while the job is blocked.
        let shutdown_done = Arc::new(AtomicBool::new(false));
        let shutdown_done_bg = Arc::clone(&shutdown_done);
        let slot_bg = Arc::clone(&slot);
        let join = std::thread::spawn(move || {
            let out = slot_bg.shutdown(Duration::from_secs(10));
            shutdown_done_bg.store(true, Ordering::SeqCst);
            out
        });

        let stopped = slot.wait_until_stopped();
        assert!(
            matches!(stopped, State::Cancelled { workers, .. } if workers >= 1),
            "shutdown must stay in Cancelled while warm job is blocked, got {stopped:?}",
        );
        std::thread::sleep(Duration::from_millis(20));
        assert!(!shutdown_done.load(Ordering::SeqCst), "shutdown must wait while warm job is running");

        // Release the blocked warm job; shutdown must now drain cleanly with zero live workers.
        gate.release();
        let outcome = join.join().expect("shutdown thread");
        assert_eq!(outcome, ShutdownOutcome::Drained);
        assert_eq!(slot.live_workers(), 0, "no warm-up thread may outlive drained shutdown");
        assert!(!frame_called.load(Ordering::SeqCst), "cancelled warm_then must skip frame callback");
        assert!(!done_called.load(Ordering::SeqCst), "cancelled warm_then must skip done callback");
    }

    #[test]
    fn shutdown_cancelling_during_warm_frame_skips_done_and_waits_for_exit() {
        let slot = Arc::new(WarmSlot::<u32, SystemClock>::new(SystemClock));
        let gate = Gate::new();
        let gate_frame = Arc::clone(&gate);
        let done_called = Arc::new(AtomicBool::new(false));
        let done_flag = Arc::clone(&done_called);

        WarmSlot::warm_then(
            Arc::clone(&slot),
            Duration::from_secs(30),
            || Some(7u32),
            move |v| {
                gate_frame.enter_and_wait();
                Some(v + 1)
            },
            move || {
                done_flag.store(true, Ordering::SeqCst);
            },
        )
        .expect("spawn warm_then");

        gate.wait_entered();
        let slot_bg = Arc::clone(&slot);
        let join = std::thread::spawn(move || slot_bg.shutdown(Duration::from_secs(10)));
        let stopped = slot.wait_until_stopped();
        assert!(
            matches!(stopped, State::Cancelled { workers, .. } if workers >= 1),
            "shutdown must enter Cancelled while warm frame is blocked, got {stopped:?}",
        );
        gate.release();
        assert_eq!(join.join().expect("shutdown"), ShutdownOutcome::Drained);
        assert_eq!(slot.live_workers(), 0);
        assert!(!done_called.load(Ordering::SeqCst), "done must not run after cancellation during frame");
    }

    #[test]
    fn shutdown_bounds_wait_when_driver_call_never_returns() {
        let slot = Arc::new(WarmSlot::<u32, SystemClock>::new(SystemClock));
        let gate = Gate::new();
        let gate_worker = Arc::clone(&gate);

        WarmSlot::start_warm(
            Arc::clone(&slot),
            Duration::from_secs(30),
            move || {
                gate_worker.enter_and_wait();
                Some(1u32)
            },
            None,
        );

        gate.wait_entered();
        let started = crate::time::now();
        let outcome = slot.shutdown(Duration::from_millis(60));
        assert!(started.elapsed() >= Duration::from_millis(50), "must wait up to the bound");
        assert_eq!(outcome, ShutdownOutcome::TimedOut { remaining: 1 });

        // Release the fake wedged worker so the test process leaks no thread.
        gate.release();
        assert_eq!(slot.wait_until_zero_workers(Duration::from_secs(10)), 0);
    }

    #[test]
    fn shutdown_drains_parked_linger_thread_and_drops_payload_and_normal_launch_works() {
        let drops = Arc::new(AtomicUsize::new(0));
        struct Tracked(Arc<AtomicUsize>);
        impl Drop for Tracked {
            fn drop(&mut self) { self.0.fetch_add(1, Ordering::SeqCst); }
        }

        let slot = Arc::new(WarmSlot::<Tracked, SystemClock>::new(SystemClock));
        slot.park(Arc::clone(&slot), Tracked(Arc::clone(&drops)), Duration::from_secs(300));
        assert!(slot.has_ready());
        assert_eq!(slot.live_workers(), 1);

        assert_eq!(slot.shutdown(Duration::from_secs(10)), ShutdownOutcome::Drained);
        assert_eq!(slot.live_workers(), 0, "parked linger thread must exit on shutdown");
        assert_eq!(drops.load(Ordering::SeqCst), 1, "parked payload must be dropped");

        // Re-opening after a clean drain allows a subsequent launch cycle.
        slot.reopen();
        WarmSlot::start_warm(
            Arc::clone(&slot),
            Duration::from_secs(30),
            {
                let drops = Arc::clone(&drops);
                move || Some(Tracked(drops))
            },
            None,
        );
        let taken = slot.take().expect("normal launch claims warmed payload immediately");
        drop(taken);
        assert_eq!(slot.shutdown(Duration::from_secs(10)), ShutdownOutcome::Drained);
        assert_eq!(slot.live_workers(), 0);
    }

    /// Proves #2067 at the shutdown/exit seam with no GPU: an ordinary slow warm-up (2.5 s,
    /// e.g. parallel cold Vulkan/llvmpipe startup) must drain cleanly under `SHUTDOWN_TIMEOUT`
    /// so `orderly_exit()` chooses `ExitAction::Return` and post-`run()` code executes, instead
    /// of hitting `_exit(0)` at 2 s.
    #[test]
    fn orderly_exit_slow_warmup_beyond_two_seconds_drains_and_runs_post_run_code() {
        let slot = Arc::new(WarmSlot::<u32, SystemClock>::new(SystemClock));
        let entered = Gate::new();
        let entered_worker = Arc::clone(&entered);
        let frame_called = Arc::new(AtomicBool::new(false));
        let done_called = Arc::new(AtomicBool::new(false));
        let frame_flag = Arc::clone(&frame_called);
        let done_flag = Arc::clone(&done_called);

        WarmSlot::warm_then(
            Arc::clone(&slot),
            Duration::from_secs(30),
            move || {
                entered_worker.enter_only();
                // Simulates an ordinary slow cold Vulkan/shader-cache startup (> 2 s).
                std::thread::sleep(Duration::from_millis(2500));
                Some(42u32)
            },
            move |v| {
                frame_flag.store(true, Ordering::SeqCst);
                Some(v)
            },
            move || {
                done_flag.store(true, Ordering::SeqCst);
            },
        )
        .expect("spawn warm_then");

        entered.wait_entered();
        let outcome = slot.shutdown(SHUTDOWN_TIMEOUT);
        let action = crate::window::exit_action(outcome, false, None, false);
        let mut post_run_code_ran = false;
        if action == crate::window::ExitAction::Return {
            post_run_code_ran = true;
        }
        // Ensure the background worker finishes before asserting so no thread leaks if the test fails.
        let remaining = slot.wait_until_zero_workers(Duration::from_secs(10));
        assert_eq!(outcome, ShutdownOutcome::Drained, "2.5 s slow warm-up must not hit last-resort timeout");
        assert_eq!(action, crate::window::ExitAction::Return, "orderly_exit must return from run()");
        assert!(post_run_code_ran, "code after App::run() must execute");
        assert_eq!(remaining, 0);
        assert!(!frame_called.load(Ordering::SeqCst), "cancelled warm_then must skip frame");
        assert!(!done_called.load(Ordering::SeqCst), "cancelled warm_then must skip done");
    }

    /// Early non-blocking `cancel()` (called when `Windows::exiting` starts) tells in-flight
    /// `warm_then` to skip `frame` and `done` immediately while teardown runs, so `shutdown()`
    /// drains cleanly once `make` finishes.
    #[test]
    fn early_cancel_during_exiting_skips_follow_up_work_and_drains_before_shutdown() {
        let slot = Arc::new(WarmSlot::<u32, SystemClock>::new(SystemClock));
        let gate = Gate::new();
        let gate_worker = Arc::clone(&gate);
        let frame_called = Arc::new(AtomicBool::new(false));
        let done_called = Arc::new(AtomicBool::new(false));
        let frame_flag = Arc::clone(&frame_called);
        let done_flag = Arc::clone(&done_called);

        WarmSlot::warm_then(
            Arc::clone(&slot),
            Duration::from_secs(30),
            move || {
                gate_worker.enter_and_wait();
                Some(9u32)
            },
            move |v| {
                frame_flag.store(true, Ordering::SeqCst);
                Some(v)
            },
            move || {
                done_flag.store(true, Ordering::SeqCst);
            },
        )
        .expect("spawn warm_then");

        gate.wait_entered();
        // Non-blocking early cancel as Windows::exiting begins.
        slot.cancel();
        assert!(matches!(slot.state(), State::Cancelled { deadline: None, workers } if workers >= 1));

        // Worker finishes during exiting teardown before shutdown() is even called.
        gate.release();
        assert_eq!(slot.wait_until_zero_workers(Duration::from_secs(5)), 0);
        assert_eq!(slot.state(), State::Done { timed_out_workers: 0 });
        assert_eq!(slot.shutdown(SHUTDOWN_TIMEOUT), ShutdownOutcome::Drained);
        assert!(!frame_called.load(Ordering::SeqCst), "early cancel must skip frame");
        assert!(!done_called.load(Ordering::SeqCst), "early cancel must skip done");
    }
}
