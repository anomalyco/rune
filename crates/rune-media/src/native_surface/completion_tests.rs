use super::gpu_tests::{Gpu, OFFSCREEN_FORMAT, Offscreen, draw};
use super::pipeline::completion::CompletionWorker;
use super::*;
use rune_gpu::health::PollFailed;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// Drop witness representing the worker's `wgpu::Device` clone: records the instant the worker
/// releases it so tests can assert the device is dropped before `CompletionWorker::drop` returns.
struct DeviceWitness(Arc<AtomicBool>);

impl Drop for DeviceWitness {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn idle_worker_joins_and_releases_device_on_drop() {
    let released = Arc::new(AtomicBool::new(false));
    let polls = Arc::new(AtomicUsize::new(0));
    let witness = DeviceWitness(released.clone());
    let polls_for = polls.clone();
    let worker = CompletionWorker::spawn_with(move || {
        let _ = &witness;
        polls_for.fetch_add(1, Ordering::SeqCst);
        Ok(wgpu::PollStatus::QueueEmpty)
    });
    assert!(!released.load(Ordering::SeqCst));
    drop(worker);
    assert!(released.load(Ordering::SeqCst), "worker must release its device before drop returns");
    assert_eq!(polls.load(Ordering::SeqCst), 0, "idle worker should never poll");
}

#[test]
fn drop_waits_for_in_flight_poll_then_exits_without_polling_again() {
    let released = Arc::new(AtomicBool::new(false));
    let polls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let witness = DeviceWitness(released.clone());
    let polls_for = polls.clone();
    let worker = CompletionWorker::spawn_with(move || {
        let _ = &witness;
        polls_for.fetch_add(1, Ordering::SeqCst);
        entered_tx.send(()).ok();
        let _ = release_rx.recv();
        // Report work still pending; shutdown must still exit without another poll.
        Ok(wgpu::PollStatus::Poll)
    });

    worker.wake();
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("worker entered poll");

    let (dropped_tx, dropped_rx) = mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(worker);
        dropped_tx.send(()).ok();
    });

    assert!(
        dropped_rx.recv_timeout(Duration::from_millis(40)).is_err(),
        "drop must wait for the in-flight poll to finish"
    );
    release_tx.send(()).ok();
    dropped_rx.recv_timeout(Duration::from_secs(5)).expect("drop finishes once poll returns");
    dropper.join().expect("dropper thread");

    assert!(released.load(Ordering::SeqCst), "device witness must drop before join returns");
    assert_eq!(polls.load(Ordering::SeqCst), 1, "worker must not poll again after stop");
}

#[test]
fn drop_wakes_worker_sleeping_between_polls() {
    let released = Arc::new(AtomicBool::new(false));
    let (polled_tx, polled_rx) = mpsc::channel();
    let witness = DeviceWitness(released.clone());
    let worker = CompletionWorker::spawn_with(move || {
        let _ = &witness;
        polled_tx.send(()).ok();
        Ok(wgpu::PollStatus::Poll)
    });

    worker.wake();
    polled_rx.recv_timeout(Duration::from_secs(5)).expect("first poll");
    let started = std::time::Instant::now();
    drop(worker);
    assert!(released.load(Ordering::SeqCst));
    assert!(started.elapsed() < Duration::from_secs(2), "drop should wake and join promptly");
}

#[test]
fn external_waker_clone_does_not_block_shutdown() {
    let released = Arc::new(AtomicBool::new(false));
    let witness = DeviceWitness(released.clone());
    let worker = CompletionWorker::spawn_with(move || {
        let _ = &witness;
        Ok(wgpu::PollStatus::QueueEmpty)
    });
    // Simulates an unfired on_submitted_work_done closure still holding a waker clone.
    let lingering_waker = worker.waker();
    drop(worker);
    assert!(released.load(Ordering::SeqCst));
    // Waking after shutdown is a safe no-op.
    lingering_waker.wake();
}

#[test]
fn lost_device_exits_worker_and_releases_device() {
    let released = Arc::new(AtomicBool::new(false));
    let polls = Arc::new(AtomicUsize::new(0));
    let witness = DeviceWitness(released.clone());
    let polls_for = polls.clone();
    let worker = CompletionWorker::spawn_with(move || {
        let _ = &witness;
        polls_for.fetch_add(1, Ordering::SeqCst);
        Err(PollFailed::Lost)
    });

    worker.wake();
    worker.wait_polling();
    // Wait briefly for the worker to exit on Lost even before drop.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !released.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(released.load(Ordering::SeqCst), "lost device must exit worker and release device");
    // Further wakes after exit are no-ops and drop joins cleanly.
    worker.wake();
    worker.wait_polling();
    drop(worker);
    assert_eq!(polls.load(Ordering::SeqCst), 1);
}

#[test]
fn worker_panic_releases_device_and_drop_does_not_panic() {
    let released = Arc::new(AtomicBool::new(false));
    let witness = DeviceWitness(released.clone());
    let worker = CompletionWorker::spawn_with(move || {
        let _ = &witness;
        panic!("simulated observer panic inside poll");
    });

    worker.wake();
    worker.wait_polling();
    drop(worker);
    assert!(released.load(Ordering::SeqCst), "panicking worker must release its device before drop returns");
}

#[test]
fn caller_unwind_joins_worker_without_aborting() {
    let released = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let witness = DeviceWitness(released.clone());
    let worker = CompletionWorker::spawn_with(move || {
        let _ = &witness;
        entered_tx.send(()).ok();
        let _ = release_rx.recv();
        Ok(wgpu::PollStatus::QueueEmpty)
    });

    worker.wake();
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("worker entered poll");
    let unblocker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        release_tx.send(()).ok();
    });

    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _keep = worker;
        panic!("simulated caller panic");
    }));
    unblocker.join().expect("unblocker thread");
    assert!(unwound.is_err());
    assert!(released.load(Ordering::SeqCst), "unwind drop must join worker and release device");
}

#[test]
fn dropping_worker_from_inside_poll_callback_does_not_self_join_deadlock() {
    let released = Arc::new(AtomicBool::new(false));
    let polls = Arc::new(AtomicUsize::new(0));
    let slot: Arc<Mutex<Option<CompletionWorker>>> = Arc::new(Mutex::new(None));
    let slot_for = slot.clone();
    let witness = DeviceWitness(released.clone());
    let polls_for = polls.clone();
    let worker = CompletionWorker::spawn_with(move || {
        let _ = &witness;
        polls_for.fetch_add(1, Ordering::SeqCst);
        // Drop the worker on the worker thread itself (e.g. from a FrameObserver).
        slot_for.lock().unwrap().take();
        Ok(wgpu::PollStatus::Poll)
    });
    let waker = worker.waker();
    *slot.lock().unwrap() = Some(worker);
    waker.wake();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !released.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(released.load(Ordering::SeqCst), "worker dropped from its own callback must exit cleanly");
    assert_eq!(polls.load(Ordering::SeqCst), 1, "worker must not poll again after self-drop");
}

#[test]
fn rewake_during_active_poll_runs_another_poll_before_parking() {
    let polls = Arc::new(AtomicUsize::new(0));
    let (first_tx, first_rx) = mpsc::channel();
    let (second_tx, second_rx) = mpsc::channel();
    let (continue_tx, continue_rx) = mpsc::channel();
    let polls_for = polls.clone();
    let worker = CompletionWorker::spawn_with(move || {
        let n = polls_for.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            first_tx.send(()).ok();
            let _ = continue_rx.recv();
        } else if n == 2 {
            second_tx.send(()).ok();
        }
        Ok(wgpu::PollStatus::QueueEmpty)
    });

    worker.wake();
    first_rx.recv_timeout(Duration::from_secs(5)).expect("first poll");
    // A second submission arrives while the first poll is still running.
    worker.wake();
    continue_tx.send(()).ok();
    second_rx.recv_timeout(Duration::from_secs(5)).expect("rewake must trigger second poll");
    drop(worker);
    assert_eq!(polls.load(Ordering::SeqCst), 2);
}

fn drop_joins_completion_thread(with_timestamps: bool) {
    let gpu = Gpu::headless_wgpu();
    let surface = NativeSurface::new();
    surface.push_bytes(synthetic(16, 16, 0), 1).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    surface.set_observer(Arc::new(move |t| {
        if t.completed_at.is_some() {
            entered_tx.send(std::thread::current().id()).ok();
            let _ = release_rx.lock().unwrap().recv();
        }
    }));

    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
    let target = Offscreen::new(&gpu.device, 16, 16);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let timestamp_writes = with_timestamps.then(|| renderer.timestamp_writes()).flatten();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            timestamp_writes,
            ..Default::default()
        });
        renderer.draw(&mut pass, 0);
    }
    renderer.submit(&gpu.queue, encoder);

    // Wait until the completion worker is inside device.poll() running our observer.
    let worker_tid = gpu.wait(&entered_rx, Duration::from_secs(10)).expect("completion callback on worker");
    assert_ne!(worker_tid, std::thread::current().id(), "completion should run on the worker thread");

    // Dropping the renderer must block until the worker finishes its in-flight poll and exits,
    // rather than returning while the worker is still polling the device. The renderer stays on
    // this thread (on macOS it isn't `Send`): another thread lets the observer go after a moment.
    let (released_tx, released_rx) = mpsc::channel();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        released_tx.send(std::time::Instant::now()).ok();
        release_tx.send(()).ok();
    });
    drop(renderer);
    let dropped = std::time::Instant::now();
    releaser.join().expect("releaser thread");
    let released = released_rx.recv().expect("the observer was let go");

    assert!(dropped >= released, "SurfaceRenderer::drop returned while the completion thread was still polling");
}

#[test]
fn dropping_surface_renderer_joins_completion_thread_before_returning() {
    drop_joins_completion_thread(false);
}

#[test]
fn dropping_surface_renderer_with_timestamps_joins_completion_thread_without_deadlock() {
    drop_joins_completion_thread(true);
}
