use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

struct FakeClock {
    now: Mutex<Instant>,
}

impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self { now: Mutex::new(rune_core::time::now()) })
    }

    fn advance(&self, d: Duration) {
        *self.now.lock().unwrap() += d;
    }

    fn clock_fn(self: &Arc<Self>) -> ClockFn {
        let c = self.clone();
        Arc::new(move || *c.now.lock().unwrap())
    }
}

struct FakeState {
    frame_bytes: usize,
    /// Post-gain bytes captured from every accepted `write`.
    captured: Vec<u8>,
    /// Number of write calls received.
    writes: usize,
    /// Number of flush calls received.
    flushes: usize,
    /// Number of latency queries received.
    latency_calls: usize,
    /// Device latency reported on `latency_frames()`.
    latency_frames: u64,
    /// Optional permit budget: when `Some(0)`, `write` returns `Ok(0)` (paced) until refilled.
    write_permits: Option<usize>,
    /// Next error to inject on `write`.
    fail_write: Option<SinkError>,
    /// Next error to inject on `latency_frames`.
    fail_latency: Option<SinkError>,
    /// True once the device has been dropped (proves worker exited).
    dropped: bool,
}

#[derive(Clone)]
struct FakeHandle {
    state: Arc<(Mutex<FakeState>, Condvar)>,
}

impl FakeHandle {
    fn new(frame_bytes: usize) -> (Self, Box<dyn SinkDevice>) {
        let state = Arc::new((
            Mutex::new(FakeState {
                frame_bytes,
                captured: Vec::new(),
                writes: 0,
                flushes: 0,
                latency_calls: 0,
                latency_frames: 0,
                write_permits: None,
                fail_write: None,
                fail_latency: None,
                dropped: false,
            }),
            Condvar::new(),
        ));
        let handle = Self { state: state.clone() };
        (handle, Box::new(FakeDevice { state }))
    }

    fn set_permits(&self, permits: Option<usize>) {
        let mut s = self.state.0.lock().unwrap();
        s.write_permits = permits;
        self.state.1.notify_all();
    }

    fn set_latency(&self, frames: u64) {
        self.state.0.lock().unwrap().latency_frames = frames;
    }

    fn fail_next_write(&self, err: SinkError) {
        let mut s = self.state.0.lock().unwrap();
        s.fail_write = Some(err);
        s.write_permits = None;
        self.state.1.notify_all();
    }

    fn captured_i16(&self) -> Vec<i16> {
        let s = self.state.0.lock().unwrap();
        s.captured.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect()
    }

    fn clear_captured(&self) {
        self.state.0.lock().unwrap().captured.clear();
    }

    fn wait_for(&self, what: &str, pred: impl Fn(&FakeState) -> bool) {
        let deadline = rune_core::time::now() + Duration::from_secs(5);
        let mut s = self.state.0.lock().unwrap();
        while !pred(&s) {
            let left = deadline.saturating_duration_since(rune_core::time::now());
            assert!(!left.is_zero(), "timed out waiting for fake device: {what}");
            let (guard, _) = self.state.1.wait_timeout(s, left.min(Duration::from_millis(25))).unwrap();
            s = guard;
        }
    }
}

struct FakeDevice {
    state: Arc<(Mutex<FakeState>, Condvar)>,
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        self.state.0.lock().unwrap().dropped = true;
        self.state.1.notify_all();
    }
}

impl SinkDevice for FakeDevice {
    fn backend(&self) -> AudioBackend {
        AudioBackend::PulseAudio
    }

    fn write(&mut self, pcm: &[u8]) -> Result<usize, SinkError> {
        let mut s = self.state.0.lock().unwrap();
        if let Some(err) = s.fail_write.take() {
            self.state.1.notify_all();
            return Err(err);
        }
        if let Some(permits) = &mut s.write_permits {
            if *permits == 0 {
                return Ok(0);
            }
            *permits -= 1;
        }
        s.writes += 1;
        s.captured.extend_from_slice(pcm);
        let frames = pcm.len() / s.frame_bytes;
        self.state.1.notify_all();
        Ok(frames)
    }

    fn latency_frames(&mut self) -> Result<u64, SinkError> {
        let mut s = self.state.0.lock().unwrap();
        s.latency_calls += 1;
        if let Some(err) = s.fail_latency.take() {
            self.state.1.notify_all();
            return Err(err);
        }
        let lat = s.latency_frames;
        self.state.1.notify_all();
        Ok(lat)
    }

    fn flush(&mut self) -> Result<(), SinkError> {
        let mut s = self.state.0.lock().unwrap();
        s.flushes += 1;
        s.latency_frames = 0;
        self.state.1.notify_all();
        Ok(())
    }

    fn pace(&self) -> Duration {
        let s = self.state.0.lock().unwrap();
        if s.write_permits == Some(0) {
            Duration::from_millis(2)
        } else {
            Duration::ZERO
        }
    }
}

fn wait_until(what: &str, mut pred: impl FnMut() -> bool) {
    let deadline = rune_core::time::now() + Duration::from_secs(5);
    while !pred() {
        assert!(rune_core::time::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn open_returns_unsupported_and_validates_spec_ranges() {
    #[cfg(not(target_os = "linux"))]
    assert_eq!(
        AudioOutput::open(AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)).unwrap_err(),
        AudioOutputError::Unsupported
    );
    assert!(matches!(
        AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 7_999, 2)),
        Err(AudioOutputError::InvalidSpec(_))
    ));
    assert!(matches!(
        AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 192_001, 2)),
        Err(AudioOutputError::InvalidSpec(_))
    ));
    assert!(matches!(
        AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 48_000, 0)),
        Err(AudioOutputError::InvalidSpec(_))
    ));
    assert!(matches!(
        AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 48_000, 9)),
        Err(AudioOutputError::InvalidSpec(_))
    ));
}

#[test]
fn post_buffer_volume_and_mute_apply_immediately_with_2_4_seconds_queued() {
    let clock = FakeClock::new();
    let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)
        .buffer_duration(Duration::from_millis(2_500))
        .volume(1.0)
        .validate()
        .unwrap();
    let (fake, dev) = FakeHandle::new(spec.frame_bytes);
    fake.set_permits(Some(1));
    let audio = AudioOutput::from_device(spec, dev, clock.clock_fn()).unwrap();

    // Queue 2.4 s (115_200 frames) of constant +20_000 PCM BEFORE playing.
    let two_point_four_s = vec![20_000i16; 115_200 * 2];
    assert_eq!(audio.push_i16(&two_point_four_s), Ok(115_200));
    audio.play();
    fake.wait_for("first quantum at volume 1.0", |s| s.writes >= 1);
    let q1 = fake.captured_i16();
    assert_eq!(q1.len(), 480 * 2);
    assert!(q1.iter().all(|&s| s == 20_000));

    // Change volume to 0.25 while >2.3 s remain buffered -> very next quantum is 5_000.
    fake.clear_captured();
    audio.set_volume(0.25);
    assert!((audio.volume() - 0.25).abs() < 1e-6);
    fake.set_permits(Some(1));
    fake.wait_for("second quantum at volume 0.25", |s| s.writes >= 2);
    let q2 = fake.captured_i16();
    assert_eq!(q2.len(), 480 * 2);
    assert!(q2.iter().all(|&s| s == 5_000));

    // Mute -> very next quantum is exact silence without dropping queued audio.
    fake.clear_captured();
    audio.set_muted(true);
    assert!(audio.muted());
    fake.set_permits(Some(1));
    fake.wait_for("third quantum muted", |s| s.writes >= 3);
    let q3 = fake.captured_i16();
    assert_eq!(q3.len(), 480 * 2);
    assert!(q3.iter().all(|&s| s == 0));

    // Unmute -> very next quantum resumes at 5_000.
    fake.clear_captured();
    audio.set_muted(false);
    fake.set_permits(Some(1));
    fake.wait_for("fourth quantum unmuted", |s| s.writes >= 4);
    let q4 = fake.captured_i16();
    assert!(q4.iter().all(|&s| s == 5_000));
}

#[test]
fn pause_flushes_rewinds_exact_tail_and_resumes_monotonically_with_new_volume() {
    let clock = FakeClock::new();
    let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 1)
        .buffer_duration(Duration::from_millis(500))
        .validate()
        .unwrap();
    let (fake, dev) = FakeHandle::new(spec.frame_bytes);
    // Allow 4 quanta (4 * 480 = 1_920 frames) to be written to the device, with 960 unheard.
    fake.set_permits(Some(4));
    fake.set_latency(960);
    let audio = AudioOutput::from_device(spec, dev, clock.clock_fn()).unwrap();

    // Push 2_880 mono frames: frame i carries value ((i + 1) * 10) as i16.
    let samples: Vec<i16> = (0..2_880).map(|i| ((i + 1) * 10) as i16).collect();
    assert_eq!(audio.push_i16(&samples), Ok(2_880));
    audio.play();
    fake.wait_for("4 quanta written", |s| s.writes == 4);
    wait_until("heard reaches 960", || audio.heard_frames() == 960);

    // Pause: device flushes immediately and 960 unheard frames (indices 960..1920) rewind ahead of
    // unread ring frames (1920..2880), leaving 1_920 queued frames.
    audio.pause();
    assert_eq!(audio.state(), AudioOutputState::Paused);
    fake.wait_for("device flushed on pause", |s| s.flushes == 1);
    wait_until("rewound tail queued", || audio.position().queued_frames() == 1_920);
    assert_eq!(audio.heard_frames(), 960);

    // Adjust volume to 0.5 while paused, then resume: first sample written must be frame 960 at 0.5x!
    fake.clear_captured();
    audio.set_volume(0.5);
    fake.set_latency(480);
    fake.set_permits(Some(1));
    audio.play();
    fake.wait_for("resumed quantum written", |s| s.writes == 5);

    let resumed = fake.captured_i16();
    assert_eq!(resumed.len(), 480);
    for (offset, &got) in resumed.iter().enumerate() {
        let frame_idx = 960 + offset;
        let want = (((frame_idx + 1) * 10) as f32 * 0.5).round() as i16;
        assert_eq!(got, want, "mismatch at resumed frame {frame_idx}");
    }
    assert!(audio.heard_frames() >= 960, "heard_frames stayed monotonic across pause/resume");
}

#[test]
fn backpressure_underrun_single_wake_recovery_and_non_blocking_drain() {
    let clock = FakeClock::new();
    let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)
        .buffer_duration(Duration::from_millis(100)) // 4_800 frames
        .validate()
        .unwrap();
    let (fake, dev) = FakeHandle::new(spec.frame_bytes);
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let audio = AudioOutput::from_device(spec, dev, clock.clock_fn()).unwrap();
    audio.set_waker(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }));

    // Fill ring completely while paused, test partial push and Full.
    let cap = audio.capacity_frames();
    assert_eq!(cap, 4_800);
    assert!(audio.has_capacity(cap));
    let chunk = vec![0i16; 3_000 * 2];
    assert_eq!(audio.push_i16(&chunk), Ok(3_000));
    assert_eq!(audio.push_i16(&chunk), Ok(1_800));
    assert!(!audio.has_capacity(1));
    assert_eq!(audio.push_i16(&chunk), Err(AudioPushError::Full));
    assert_eq!(wakes.load(Ordering::SeqCst), 0, "no wakes while paused");

    // Play and let buffer drain to 0 with device latency 0 -> single Underrun wake.
    fake.set_latency(0);
    audio.play();
    wait_until("single underrun event", || wakes.load(Ordering::SeqCst) == 1);
    assert_eq!(audio.take_events(), vec![AudioEvent::Underrun]);

    // Remain dry for 40 ms: zero extra wakes or events.
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert!(audio.take_events().is_empty());

    // Push recovery chunk and call finish(): rejects new pushes, drains to Ended.
    let tail = vec![100i16; 480 * 2];
    assert_eq!(audio.push_i16(&tail), Ok(480));
    fake.set_latency(480);
    audio.finish();
    assert_eq!(audio.push_i16(&tail), Err(AudioPushError::Draining));
    wait_until("ring emptied during drain", || audio.position().queued_frames() == 0);
    assert_eq!(audio.state(), AudioOutputState::Draining);

    // Drop device latency to 0 -> transitions to Ended and emits AudioEvent::Ended once.
    fake.set_latency(0);
    wait_until("ended after drain", || audio.state() == AudioOutputState::Ended);
    assert_eq!(audio.take_events(), vec![AudioEvent::Ended]);
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
}

#[test]
fn flush_resets_clock_and_device_lost_exits_worker() {
    let clock = FakeClock::new();
    let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)
        .buffer_duration(Duration::from_millis(200))
        .validate()
        .unwrap();
    let (fake, dev) = FakeHandle::new(spec.frame_bytes);
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let audio = AudioOutput::from_device(spec, dev, clock.clock_fn()).unwrap();
    audio.set_waker(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }));

    fake.set_latency(480);
    assert_eq!(audio.push_i16(&vec![1i16; 1_440 * 2]), Ok(1_440));
    audio.play();
    wait_until("heard advances", || audio.heard_frames() >= 960);
    assert!(audio.take_events().is_empty());

    // Flush while playing resets clock to 0, keeps Playing, and emits NO Underrun.
    audio.flush();
    assert_eq!(audio.state(), AudioOutputState::Playing);
    assert_eq!(audio.heard_frames(), 0);
    fake.wait_for("device flushed", |s| s.flushes >= 1);
    std::thread::sleep(Duration::from_millis(30));
    assert!(audio.take_events().is_empty(), "flush must not cause spurious underrun");

    // Inject unrecoverable device error -> DeviceLost, event delivered, worker exits and drops device.
    fake.fail_next_write(SinkError::Lost("headphones unplugged".into()));
    assert_eq!(audio.push_i16(&vec![2i16; 480 * 2]), Ok(480));
    wait_until("device lost state", || matches!(audio.state(), AudioOutputState::DeviceLost(_)));
    assert_eq!(audio.state(), AudioOutputState::DeviceLost("headphones unplugged".into()));
    assert_eq!(audio.take_events(), vec![AudioEvent::DeviceLost("headphones unplugged".into())]);
    assert_eq!(audio.push_i16(&vec![0i16; 480 * 2]), Err(AudioPushError::DeviceLost));
    fake.wait_for("worker exited and dropped device", |s| s.dropped);
}

#[test]
fn idle_paused_and_ended_outputs_perform_zero_wakeups() {
    let clock = FakeClock::new();
    let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)
        .buffer_duration(Duration::from_millis(200))
        .validate()
        .unwrap();
    let (fake, dev) = FakeHandle::new(spec.frame_bytes);
    let audio = AudioOutput::from_device(spec, dev, clock.clock_fn()).unwrap();

    std::thread::sleep(Duration::from_millis(40));
    {
        let s = fake.state.0.lock().unwrap();
        assert_eq!((s.writes, s.flushes, s.latency_calls), (0, 0, 0));
    }

    // Empty finish -> Ended immediately with zero device touches.
    audio.finish();
    assert_eq!(audio.state(), AudioOutputState::Ended);
    std::thread::sleep(Duration::from_millis(40));
    {
        let s = fake.state.0.lock().unwrap();
        assert_eq!((s.writes, s.flushes, s.latency_calls), (0, 0, 0));
    }
}

#[test]
fn silent_backend_paced_by_fake_clock_advances_and_drains_end_to_end() {
    let clock = FakeClock::new();
    let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)
        .buffer_duration(Duration::from_millis(200))
        .validate()
        .unwrap();
    let dev = Box::new(silent::SilentDevice::new(&spec, clock.clock_fn()));
    let audio = AudioOutput::from_device(spec, dev, clock.clock_fn()).unwrap();
    assert_eq!(audio.backend(), AudioBackend::Silent);

    // Push 100 ms (4_800 frames), play, and step the fake clock until drain completes.
    assert_eq!(audio.push_i16(&vec![0i16; 4_800 * 2]), Ok(4_800));
    audio.play();
    audio.finish();
    wait_until("silent backend drains to Ended", || {
        clock.advance(Duration::from_millis(20));
        audio.state() == AudioOutputState::Ended
    });
    assert_eq!(audio.heard_frames(), 4_800);
    assert_eq!(audio.heard_duration(), Duration::from_millis(100));
    assert_eq!(audio.take_events(), vec![AudioEvent::Ended]);
}

#[test]
fn open_silent_works_with_real_clock_and_replaced_waker_drops_outside_lock() {
    let audio = AudioOutput::open_silent(
        AudioOutputSpec::new(SampleFormat::F32, 48_000, 2).buffer_duration(Duration::from_millis(200)),
    )
    .unwrap();
    assert_eq!(audio.backend(), AudioBackend::Silent);

    // Replacing a waker whose drop reads `audio.state()` must never deadlock.
    struct Probe(AudioOutput);
    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.0.state();
        }
    }
    let probe = Probe(audio.clone());
    audio.set_waker(Arc::new(move || {
        let _ = &probe;
    }));
    audio.set_waker(Arc::new(|| {}));

    assert_eq!(audio.push_f32(&vec![0.0f32; 480 * 2]), Ok(480));
    audio.play();
    audio.finish();
    wait_until("real-clock silent drain ends", || audio.state() == AudioOutputState::Ended);
    assert_eq!(audio.heard_frames(), 480);
}

#[test]
fn concurrent_push_and_position_reads_stay_monotonic_within_timeout() {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let audio = AudioOutput::open_silent(
            AudioOutputSpec::new(SampleFormat::I16, 48_000, 2).buffer_duration(Duration::from_millis(200)),
        )
        .unwrap();
        audio.play();
        let producer_audio = audio.clone();
        let producer = std::thread::spawn(move || {
            let chunk = vec![1234i16; 240 * 2];
            for i in 0..80 {
                if i % 20 == 10 {
                    producer_audio.set_volume(0.5);
                } else if i % 20 == 15 {
                    producer_audio.set_volume(1.0);
                }
                match producer_audio.push_i16(&chunk) {
                    Ok(_) | Err(AudioPushError::Full) => {}
                    Err(e) => panic!("unexpected push error: {e}"),
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            producer_audio.finish();
        });

        let mut last_heard = 0u64;
        for _ in 0..120 {
            let pos = audio.position();
            assert!(
                pos.heard_frames() >= last_heard,
                "heard_frames regressed: {} < {last_heard}",
                pos.heard_frames()
            );
            last_heard = pos.heard_frames();
            let _ = audio.take_events();
            std::thread::sleep(Duration::from_millis(1));
        }
        producer.join().unwrap();
        tx.send(()).ok();
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("concurrent push/position stress test timed out");
}
