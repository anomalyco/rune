use super::gpu_tests::{Gpu, OFFSCREEN_FORMAT, Offscreen, draw};
use super::pipeline::completion::{CompletionWorker, Parked, ReadWorker};
use std::ops::ControlFlow;
use rune_gpu::configure::Gate;
use super::*;
use rune_gpu::health::PollFailed;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// Drop witness representing the worker's `wgpu::Device` clone: records the instant the worker
/// releases it so tests can assert the device is dropped before `CompletionWorker::drop` returns.
#[derive(Debug)]
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

#[test]
fn parked_work_is_submitted_at_once_when_no_surface_is_being_configured() {
    let (gate, parked, submitted) = (Gate::new(), Parked::default(), Mutex::new(Vec::new()));
    assert!(parked.flush(&gate, |_: Vec<u32>| panic!("nothing was parked")));
    parked.park(1);
    parked.park(2);
    assert!(parked.flush(&gate, |work| submitted.lock().unwrap().extend(work)));
    assert_eq!(*submitted.lock().unwrap(), vec![1, 2]);
    assert!(parked.flush(&gate, |_| panic!("submitted twice")));
}

#[test]
fn parked_work_waits_out_a_surface_configure() {
    let (gate, parked, submitted) = (Gate::new(), Parked::default(), Mutex::new(Vec::new()));
    // A completion callback that wgpu runs inside `Surface::configure`, on the configuring
    // thread, and one on another thread meanwhile: neither submits.
    gate.configuring(|| {
        parked.park(1);
        assert!(!parked.flush(&gate, |_| panic!("submitted inside the configure")));
        std::thread::scope(|scope| {
            scope.spawn(|| {
                parked.park(2);
                assert!(!parked.flush(&gate, |_| panic!("submitted beside the configure")));
            });
        });
    });
    // The surface is configured: everything kept back goes, in order.
    assert!(parked.flush(&gate, |work| submitted.lock().unwrap().extend(work)));
    assert_eq!(*submitted.lock().unwrap(), vec![1, 2]);
}

/// The completion thread as `CompletionWorker::spawn` runs it, over a gate and a submit of the
/// test's own: flush what is parked, then poll.
fn worker_over(gate: Arc<Gate>, parked: Arc<Parked<u32>>, submitted: mpsc::Sender<u32>) -> CompletionWorker {
    CompletionWorker::spawn_with(move || {
        let flushed = parked.flush(&gate, |work| work.into_iter().for_each(|w| submitted.send(w).unwrap()));
        Ok(if flushed { wgpu::PollStatus::QueueEmpty } else { wgpu::PollStatus::Poll })
    })
}

#[test]
fn the_completion_thread_submits_what_a_configure_kept_back_once_it_is_done() {
    let (gate, parked) = (Arc::new(Gate::new()), Arc::new(Parked::default()));
    let (submitted_tx, submitted) = mpsc::channel();
    let worker = worker_over(gate.clone(), parked.clone(), submitted_tx);
    gate.configuring(|| {
        // The callback's part: park, try, wake the worker.
        parked.park(7);
        assert!(!parked.flush(&gate, |_| panic!("submitted inside the configure")));
        worker.wake();
        // The worker keeps coming back, and keeps being refused, while the surface configures.
        assert!(submitted.recv_timeout(Duration::from_millis(60)).is_err(), "submitted while a surface was being configured");
    });
    assert_eq!(submitted.recv_timeout(Duration::from_secs(5)), Ok(7), "the parked work was never submitted");
    drop(worker);
}

/// A frame drawn with timestamp queries, whose resolve a completion callback submits after it.
fn timed_frame_resolves(configure_meanwhile: bool) {
    let gpu = Gpu::headless_wgpu();
    if !gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        eprintln!("adapter has no timestamp queries: {:?}", gpu.adapter.get_info());
        return;
    }
    let surface = NativeSurface::new();
    surface.push_bytes(synthetic(16, 16, 0), 1).unwrap();
    let (completed_tx, completed) = mpsc::channel();
    surface.set_observer(Arc::new(move |t| {
        if t.completed_at.is_some() {
            completed_tx.send(t.gpu_end_seconds.is_some()).ok();
        }
    }));
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
    let target = Offscreen::new(&gpu.device, 16, 16);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            timestamp_writes: renderer.timestamp_writes(),
            ..Default::default()
        });
        renderer.draw(&mut pass, 0);
    }
    if configure_meanwhile {
        // What `Surface::configure` does, in the window's place: it holds off other threads'
        // submissions, waits for the frame, and runs the frame's completion callback itself.
        rune_gpu::configure::configuring(|| {
            renderer.submit(&gpu.queue, encoder);
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            // The frame is done and its callback has run, here or on the completion thread: the
            // resolve it has to submit stays parked, so nothing lands in the queue wgpu is
            // about to check, and the frame isn't reported complete yet.
            assert!(completed.recv_timeout(Duration::from_millis(150)).is_err(), "the resolve was submitted while a surface was being configured");
        });
    } else {
        renderer.submit(&gpu.queue, encoder);
    }
    // With no configure in the way (any more), the resolve is submitted and brings the GPU's times.
    assert_eq!(gpu.wait(&completed, Duration::from_secs(10)), Some(true), "the frame's times never came");
}

#[test]
fn a_timestamp_resolve_is_not_submitted_while_a_surface_is_being_configured() {
    timed_frame_resolves(true);
}

#[test]
fn a_timestamp_resolve_is_submitted_at_once_with_no_configure_in_the_way() {
    timed_frame_resolves(false);
}

#[test]
fn read_worker_idle_joins_and_releases_device_on_drop() {
    let released = Arc::new(AtomicBool::new(false));
    let waits = Arc::new(AtomicUsize::new(0));
    let witness = DeviceWitness(released.clone());
    let waits_for = waits.clone();
    let worker = ReadWorker::<u32>::spawn("test-dmabuf-read", move |_| {
        let _ = &witness;
        waits_for.fetch_add(1, Ordering::SeqCst);
        ControlFlow::Continue(())
    })
    .unwrap();
    assert!(!released.load(Ordering::SeqCst));
    drop(worker);
    assert!(released.load(Ordering::SeqCst), "worker must release its device before drop returns");
    assert_eq!(waits.load(Ordering::SeqCst), 0, "idle worker should never wait");
}

#[test]
fn read_worker_drop_waits_for_in_flight_wait_and_skips_queued_items() {
    let released = Arc::new(AtomicBool::new(false));
    let waits = Arc::new(AtomicUsize::new(0));
    let skipped_a = Arc::new(AtomicBool::new(false));
    let skipped_b = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let witness = DeviceWitness(released.clone());
    let waits_for = waits.clone();
    let worker = ReadWorker::spawn("test-dmabuf-read", move |(id, _item): (u32, DeviceWitness)| {
        let _ = &witness;
        waits_for.fetch_add(1, Ordering::SeqCst);
        if id == 1 {
            entered_tx.send(()).ok();
            let _ = release_rx.recv();
        }
        ControlFlow::Continue(())
    })
    .unwrap();

    let first_dropped = Arc::new(AtomicBool::new(false));
    worker.send((1, DeviceWitness(first_dropped))).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("worker entered wait");
    // Enqueue two more items while the first wait is blocked in flight.
    worker.send((2, DeviceWitness(skipped_a.clone()))).unwrap();
    worker.send((3, DeviceWitness(skipped_b.clone()))).unwrap();

    let (dropped_tx, dropped_rx) = mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(worker);
        dropped_tx.send(()).ok();
    });

    assert!(
        dropped_rx.recv_timeout(Duration::from_millis(40)).is_err(),
        "drop must wait for the in-flight wait to finish"
    );
    release_tx.send(()).ok();
    dropped_rx.recv_timeout(Duration::from_secs(5)).expect("drop finishes once wait returns");
    dropper.join().expect("dropper thread");

    assert!(released.load(Ordering::SeqCst), "device witness must drop before join returns");
    assert_eq!(waits.load(Ordering::SeqCst), 1, "worker must not wait on queued items after stop");
    assert!(skipped_a.load(Ordering::SeqCst) && skipped_b.load(Ordering::SeqCst), "unpolled queued items must be dropped on stop");
}

#[test]
fn read_worker_lost_device_exits_releases_device_and_rejects_further_sends() {
    let released = Arc::new(AtomicBool::new(false));
    let waits = Arc::new(AtomicUsize::new(0));
    let queued_dropped = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let witness = DeviceWitness(released.clone());
    let waits_for = waits.clone();
    let worker = ReadWorker::spawn("test-dmabuf-read", move |(id, _item): (u32, DeviceWitness)| {
        let _ = &witness;
        waits_for.fetch_add(1, Ordering::SeqCst);
        if id == 1 {
            entered_tx.send(()).ok();
            let _ = release_rx.recv();
        }
        ControlFlow::Break(())
    })
    .unwrap();

    worker.send((1, DeviceWitness(Arc::new(AtomicBool::new(false))))).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("worker entered wait");
    worker.send((2, DeviceWitness(queued_dropped.clone()))).unwrap();
    release_tx.send(()).ok();
    worker.wait_stopped();

    assert!(released.load(Ordering::SeqCst), "lost device must exit worker and release device before drop");
    assert!(queued_dropped.load(Ordering::SeqCst), "queued items after lost device must be dropped");
    assert!(worker.send((3, DeviceWitness(Arc::new(AtomicBool::new(false))))).is_err(), "sends after exit must fail");
    drop(worker);
    assert_eq!(waits.load(Ordering::SeqCst), 1);
}

#[test]
fn read_worker_continues_after_non_fatal_wait_error() {
    let (done_tx, done_rx) = mpsc::channel();
    let worker = ReadWorker::spawn("test-dmabuf-read", move |id: u32| {
        done_tx.send(id).ok();
        ControlFlow::Continue(())
    })
    .unwrap();

    worker.send(1).unwrap();
    worker.send(2).unwrap();
    assert_eq!(done_rx.recv_timeout(Duration::from_secs(5)), Ok(1));
    assert_eq!(done_rx.recv_timeout(Duration::from_secs(5)), Ok(2));
    drop(worker);
}

#[test]
fn read_worker_panic_releases_device_and_drop_does_not_panic() {
    let released = Arc::new(AtomicBool::new(false));
    let queued_dropped = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let witness = DeviceWitness(released.clone());
    let worker = ReadWorker::spawn("test-dmabuf-read", move |(_id, _item): (u32, DeviceWitness)| {
        let _ = &witness;
        entered_tx.send(()).ok();
        let _ = release_rx.recv();
        panic!("simulated on_read panic");
    })
    .unwrap();

    worker.send((1, DeviceWitness(Arc::new(AtomicBool::new(false))))).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("worker entered wait");
    worker.send((2, DeviceWitness(queued_dropped.clone()))).unwrap();
    release_tx.send(()).ok();
    worker.wait_stopped();

    assert!(released.load(Ordering::SeqCst), "panicking worker must release its device");
    assert!(queued_dropped.load(Ordering::SeqCst), "panicking worker must drop queued items");
    assert!(worker.send((3, DeviceWitness(Arc::new(AtomicBool::new(false))))).is_err());
    drop(worker);
}

#[test]
fn read_worker_caller_unwind_joins_worker_without_aborting() {
    let released = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let witness = DeviceWitness(released.clone());
    let worker = ReadWorker::spawn("test-dmabuf-read", move |(): ()| {
        let _ = &witness;
        entered_tx.send(()).ok();
        let _ = release_rx.recv();
        ControlFlow::Continue(())
    })
    .unwrap();

    worker.send(()).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("worker entered wait");
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
fn read_worker_self_drop_from_callback_does_not_deadlock() {
    let released = Arc::new(AtomicBool::new(false));
    let waits = Arc::new(AtomicUsize::new(0));
    let queued_dropped = Arc::new(AtomicBool::new(false));
    let slot: Arc<Mutex<Option<ReadWorker<(u32, DeviceWitness)>>>> = Arc::new(Mutex::new(None));
    let slot_for = slot.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let witness = DeviceWitness(released.clone());
    let waits_for = waits.clone();
    let worker = ReadWorker::spawn("test-dmabuf-read", move |(id, _item): (u32, DeviceWitness)| {
        let _ = &witness;
        waits_for.fetch_add(1, Ordering::SeqCst);
        if id == 1 {
            entered_tx.send(()).ok();
            let _ = release_rx.recv();
            // Drop the worker from inside its own callback (e.g. an on_read closure).
            slot_for.lock().unwrap().take();
        }
        ControlFlow::Continue(())
    })
    .unwrap();

    worker.send((1, DeviceWitness(Arc::new(AtomicBool::new(false))))).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("worker entered wait");
    worker.send((2, DeviceWitness(queued_dropped.clone()))).unwrap();
    *slot.lock().unwrap() = Some(worker);
    release_tx.send(()).ok();

    let deadline = rune_core::time::now() + Duration::from_secs(5);
    while !released.load(Ordering::SeqCst) && rune_core::time::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(released.load(Ordering::SeqCst), "worker dropped from its own callback must exit cleanly");
    assert!(queued_dropped.load(Ordering::SeqCst), "self-drop must discard unpolled items");
    assert_eq!(waits.load(Ordering::SeqCst), 1, "worker must not wait again after self-drop");
}

#[test]
fn read_worker_stop_is_idempotent() {
    let released = Arc::new(AtomicBool::new(false));
    let witness = DeviceWitness(released.clone());
    let mut worker = ReadWorker::<()>::spawn("test-dmabuf-read", move |()| {
        let _ = &witness;
        ControlFlow::Continue(())
    })
    .unwrap();
    worker.stop();
    assert!(released.load(Ordering::SeqCst));
    worker.stop();
    drop(worker);
}

#[test]
fn read_worker_device_notifies_on_read_and_drop_joins_in_flight_callback() {
    let gpu = Gpu::headless_wgpu();
    let worker = ReadWorker::spawn_device("rune-dmabuf-read", gpu.device.clone(), Duration::from_secs(10)).unwrap();
    let encoder = gpu.device.create_command_encoder(&Default::default());
    let submission = gpu.queue.submit([encoder.finish()]);

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    assert!(
        worker
            .send((
                submission,
                Box::new(move |result| {
                    assert!(result.is_ok(), "submitted copy should succeed: {result:?}");
                    let current = std::thread::current();
                    entered_tx.send((current.id(), current.name().map(str::to_owned))).ok();
                    let _ = release_rx.recv();
                }),
            ))
            .is_ok()
    );

    let (worker_tid, worker_name) = gpu.wait(&entered_rx, Duration::from_secs(10)).expect("on_read ran on worker");
    assert_ne!(worker_tid, std::thread::current().id(), "on_read must run on the worker thread");
    assert_eq!(worker_name.as_deref(), Some("rune-dmabuf-read"));

    let (released_tx, released_rx) = mpsc::channel();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        released_tx.send(rune_core::time::now()).ok();
        release_tx.send(()).ok();
    });
    drop(worker);
    let dropped = rune_core::time::now();
    releaser.join().expect("releaser thread");
    let released = released_rx.recv().expect("callback was let go");

    assert!(dropped >= released, "ReadWorker::drop returned while the read thread was still running");
}
