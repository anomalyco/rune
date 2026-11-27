use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{HeadlessWake, try_headless, wait_realtime_tick};
use crate::{App, AppSpec, Color, Cx, El, col, el};

#[test]
fn wait_realtime_tick_wakes_before_deadline_and_finishes_at_deadline() {
    let wake = HeadlessWake::default();
    let waker = wake.waker();
    let start = Instant::now();
    let deadline = start + Duration::from_millis(160);

    let sent_at = Arc::new(Mutex::new(None::<Instant>));
    let sent_worker = Arc::clone(&sent_at);
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        *sent_worker.lock().unwrap() = Some(Instant::now());
        waker();
    });

    let mut received_at = None;
    wait_realtime_tick(&wake, deadline, || {
        received_at = Some(Instant::now());
    });
    let finished_at = Instant::now();
    worker.join().unwrap();

    let sent = sent_at.lock().unwrap().expect("worker sent wake");
    let received = received_at.expect("on_wake ran on early notification");
    let wake_latency = received.saturating_duration_since(sent);
    let margin_before_deadline = deadline.saturating_duration_since(received);

    assert!(
        wake_latency < Duration::from_millis(60),
        "expected prompt wake on notification, got {wake_latency:?}"
    );
    assert!(
        margin_before_deadline >= Duration::from_millis(50),
        "expected wake well before tick deadline, margin was {margin_before_deadline:?}"
    );
    assert!(
        finished_at >= deadline,
        "tick wait must still hold until deadline (short by {:?})",
        deadline.saturating_duration_since(finished_at)
    );
}

#[test]
fn wait_realtime_tick_drains_pre_notified_wake_before_sleeping() {
    let wake = HeadlessWake::default();
    wake.waker()();

    let start = Instant::now();
    let deadline = start + Duration::from_millis(100);
    let mut received_at = None;
    wait_realtime_tick(&wake, deadline, || {
        received_at = Some(Instant::now());
    });
    let finished_at = Instant::now();

    let received = received_at.expect("pending notification must drain immediately");
    assert!(
        received.saturating_duration_since(start) < Duration::from_millis(40),
        "pending notification should drain before sleeping, took {:?}",
        received.saturating_duration_since(start)
    );
    assert!(finished_at >= deadline, "tick wait must still finish at deadline");
}

#[test]
fn wait_realtime_tick_handles_multiple_wakes_without_drifting_deadline() {
    let wake = HeadlessWake::default();
    let waker = wake.waker();
    let start = Instant::now();
    let deadline = start + Duration::from_millis(180);

    let worker = std::thread::spawn(move || {
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(25));
            waker();
        }
    });

    let mut wakes = Vec::new();
    wait_realtime_tick(&wake, deadline, || {
        wakes.push(Instant::now());
    });
    let finished_at = Instant::now();
    worker.join().unwrap();

    assert!(
        !wakes.is_empty(),
        "expected at least one early wake during the tick interval"
    );
    let last_wake = *wakes.last().unwrap();
    assert!(
        deadline.saturating_duration_since(last_wake) >= Duration::from_millis(30),
        "all mid-interval wakes should complete well before deadline, last margin {:?}",
        deadline.saturating_duration_since(last_wake)
    );
    assert!(finished_at >= deadline, "tick wait must reach deadline");
    let overshoot = finished_at.saturating_duration_since(deadline);
    assert!(
        overshoot < Duration::from_millis(100),
        "repeated wakes must not restart or drift the deadline (overshoot {overshoot:?})"
    );
}

#[test]
fn wait_realtime_tick_returns_immediately_when_deadline_already_passed() {
    let wake = HeadlessWake::default();
    let deadline = Instant::now();
    let mut called = false;
    wait_realtime_tick(&wake, deadline, || called = true);
    assert!(!called, "already-reached deadline must not sleep or invoke on_wake");
}

#[test]
fn wait_realtime_tick_multi_frame_cadence_stays_anchored_to_wall_clock() {
    let wake = HeadlessWake::default();
    let wall = Instant::now();
    let dt = Duration::from_millis(40);
    let frames = 4u32;

    let mut early_margins = Vec::new();
    for k in 1..=frames {
        let deadline = wall + dt * k;
        let waker = wake.waker();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            waker();
        });
        let mut woken_at = None;
        wait_realtime_tick(&wake, deadline, || {
            woken_at = Some(Instant::now());
        });
        worker.join().unwrap();
        let woken = woken_at.expect("mid-frame wake should run");
        early_margins.push(deadline.saturating_duration_since(woken));
    }

    let elapsed = wall.elapsed();
    let target = dt * frames;
    assert!(
        elapsed >= target,
        "anchored cadence must not finish before target {target:?} (elapsed {elapsed:?})"
    );
    assert!(
        elapsed < target + Duration::from_millis(140),
        "anchored cadence must not drift across frames (target {target:?}, elapsed {elapsed:?})"
    );
    assert!(
        early_margins.iter().any(|m| *m >= Duration::from_millis(10)),
        "expected wakes before frame deadlines, got margins {early_margins:?}"
    );
}

#[test]
fn headless_realtime_update_if_wakes_mid_frame_before_tick_deadline() {
    if !crate::render::Gpu::test_adapter_available() {
        return;
    }
    let _turn = crate::tooling::headless_run::test_turn();

    struct State {
        delivered: bool,
        rendered_delivered: Arc<AtomicBool>,
    }

    let rendered_delivered = Arc::new(AtomicBool::new(false));
    let rendered_in_view = Arc::clone(&rendered_delivered);

    let spec = AppSpec::new(
        "realtime-wake",
        (80.0, 80.0),
        || State {
            delivered: false,
            rendered_delivered: Arc::new(AtomicBool::new(false)),
        },
        |s: &State, _: &mut Cx<State>| -> El {
            if s.delivered {
                s.rendered_delivered.store(true, Ordering::Release);
            }
            col().w_full().h_full().fill(Color::BLACK).child(
                el().key(if s.delivered { "delivered" } else { "pending" })
                    .size(40.0, 40.0)
                    .fill(Color::WHITE),
            )
        },
    );

    let ready_at = Arc::new(Mutex::new(None::<Instant>));
    let sent_at = Arc::new(Mutex::new(None::<Instant>));
    let received_at = Arc::new(Mutex::new(None::<Instant>));
    let worker_handle = Arc::new(Mutex::new(None::<std::thread::JoinHandle<()>>));

    let app = App::new(spec).init(move || State {
        delivered: false,
        rendered_delivered: Arc::clone(&rendered_in_view),
    });
    let handle = app.handle();
    let ready_hook = Arc::clone(&ready_at);
    let sent_hook = Arc::clone(&sent_at);
    let recv_hook = Arc::clone(&received_at);
    let worker_slot = Arc::clone(&worker_handle);

    // At 5 fps (dt = 200 ms) and --at 0.4, frames are k = 0 (0 ms), k = 1 (200 ms, runs on_ready),
    // and k = 2 (400 ms). The worker delivers ~30 ms after on_ready, leaving ~170 ms before k = 2.
    let app = app.on_ready(move |_| {
        *ready_hook.lock().unwrap() = Some(Instant::now());
        let handle = handle.clone();
        let sent_worker = Arc::clone(&sent_hook);
        let recv_worker = Arc::clone(&recv_hook);
        *worker_slot.lock().unwrap() = Some(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            *sent_worker.lock().unwrap() = Some(Instant::now());
            handle.update_if(move |s, _| {
                *recv_worker.lock().unwrap() = Some(Instant::now());
                s.delivered = true;
                true
            });
        }));
    });

    let args = crate::tooling::Args::parse_from(vec![
        "--realtime".into(),
        "--fps".into(),
        "5".into(),
        "--at".into(),
        "0.4".into(),
        "--scale".into(),
        "1".into(),
    ]);
    try_headless(app, args)
        .map_err(|e| e.message())
        .expect("realtime headless run succeeds");
    let finished_at = Instant::now();
    if let Some(worker) = worker_handle.lock().unwrap().take() {
        worker.join().unwrap();
    }

    let ready = ready_at.lock().unwrap().expect("on_ready ran in --realtime mode");
    let sent = sent_at.lock().unwrap().expect("worker sent update_if");
    let received = received_at.lock().unwrap().expect("update_if ran on UI thread");

    let delivery_latency = received.saturating_duration_since(sent);
    let before_finish = finished_at.saturating_duration_since(received);
    let tick_span = finished_at.saturating_duration_since(ready);

    assert!(
        rendered_delivered.load(Ordering::Acquire),
        "updated state must be rendered on frame k = 2"
    );
    assert!(
        delivery_latency < Duration::from_millis(80),
        "update_if must wake --realtime promptly instead of waiting for the 200 ms tick (latency {delivery_latency:?})"
    );
    assert!(
        before_finish >= Duration::from_millis(80),
        "update_if must drain mid-interval well before the next frame tick (remaining {before_finish:?})"
    );
    assert!(
        tick_span >= Duration::from_millis(180),
        "frame k = 2 must still hold until its 200 ms wall-clock tick (span {tick_span:?})"
    );
}
