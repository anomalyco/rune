use super::*;

fn meta() -> Meta {
    Meta { duration: 1.0, size: Some((64, 36)), frame: Some(0.25), sound: false }
}

#[test]
fn new_videos_are_muted_paused_and_loading() {
    let v = Video::file("missing.mp4");
    assert!(v.muted());
    assert!(!v.looping());
    assert!(!v.is_playing());
    assert!(!v.is_seeking());
    assert_eq!(v.rate(), 1.0);
    assert_eq!(v.output_device(), None);
    assert_eq!(v.time(), 0.0);
    assert_eq!(v.duration(), None);
    assert_eq!(v.frame_duration(), None);
    assert_eq!(v.presented_time(), None);
    assert_eq!(v.natural_size(), None);
    assert!(!v.is_audio());
    #[cfg(target_os = "macos")]
    assert_eq!(v.state(), PlayerState::Loading);
    assert_eq!(v.source(), &MediaSource::File("missing.mp4".into()));
}

#[test]
fn sources_tell_urls_from_paths() {
    assert_eq!(MediaSource::from("https://example.com/a.mp3"), MediaSource::Url("https://example.com/a.mp3".into()));
    assert_eq!(MediaSource::from("file:///tmp/a.wav".to_string()), MediaSource::Url("file:///tmp/a.wav".into()));
    assert_eq!(MediaSource::from("sounds/a.m4a"), MediaSource::File("sounds/a.m4a".into()));
    assert_eq!(MediaSource::from(std::path::Path::new("a.mp3")), MediaSource::File("a.mp3".into()));
}

#[test]
fn audio_starts_unmuted_with_no_size() {
    let a = Player::audio("missing.m4a");
    assert!(a.is_audio());
    assert!(!a.muted(), "audio is audible by default");
    assert_eq!(a.output_device(), None);
    assert_eq!(a.natural_size(), None);
}

#[test]
fn output_device_defaults_to_none_and_round_trips_across_clones() {
    let a = Player::audio("missing.m4a");
    let clone = a.clone();
    assert_eq!(a.output_device(), None);
    assert_eq!(a.shared.ctl.lock().unwrap().output_generation, 0);

    // Empty and unchanged None are no-ops.
    a.set_output_device(None);
    a.set_output_device(Some(""));
    assert_eq!(a.output_device(), None);
    assert_eq!(a.shared.ctl.lock().unwrap().output_generation, 0);

    // Setting a UID updates all clones and bumps the generation once.
    clone.set_output_device(Some("BuiltInSpeakerDevice"));
    assert_eq!(a.output_device().as_deref(), Some("BuiltInSpeakerDevice"));
    assert_eq!(clone.output_device().as_deref(), Some("BuiltInSpeakerDevice"));
    assert_eq!(a.shared.ctl.lock().unwrap().output_generation, 1);

    // Repeating the same UID is a no-op.
    a.set_output_device(Some("BuiltInSpeakerDevice"));
    assert_eq!(a.shared.ctl.lock().unwrap().output_generation, 1);

    // Switching to a long / non-ASCII UID works cleanly.
    let long_uid = format!("USB-DAC-🎧-{}", "x".repeat(512));
    a.set_output_device(Some(&long_uid));
    assert_eq!(clone.output_device().as_deref(), Some(long_uid.as_str()));
    assert_eq!(a.shared.ctl.lock().unwrap().output_generation, 2);

    // Empty string normalizes to system default (None).
    a.set_output_device(Some(""));
    assert_eq!(clone.output_device(), None);
    assert_eq!(a.shared.ctl.lock().unwrap().output_generation, 3);

    // Explicit None when already None is a no-op.
    a.set_output_device(None);
    assert_eq!(a.shared.ctl.lock().unwrap().output_generation, 3);
}

/// A clip's extension comes from the last segment of its path, never the host or query.
#[cfg(target_os = "macos")]
#[test]
fn extensions_come_from_the_last_path_segment() {
    let ext = |s: &str| super::macos::extension(&MediaSource::from(s));
    assert_eq!(ext("https://example.com/media/clip"), None);
    assert_eq!(ext("https://host.ogg/clip"), None);
    assert_eq!(ext("https://host.ogg"), None);
    assert_eq!(ext("https://example.com/a.b/clip?x=1.ogg#t=2.webm"), None);
    assert_eq!(ext("https://example.com/media/clip.OGG?v=2#t"), Some("ogg".into()));
    assert_eq!(ext("file:///tmp/a.b/clip.opus"), Some("opus".into()));
    assert_eq!(ext("sounds.d/clip"), None);
    assert_eq!(ext("sounds/clip.webm"), Some("webm".into()));
    assert_eq!(ext("sounds/clip."), None);
}

/// Windows reads WebM and Ogg when the system has their extensions, so it finds out on open.
#[cfg(not(target_os = "windows"))]
#[test]
fn webm_video_fails_at_once() {
    let a = Player::url("https://example.com/clips/tap.webm?v=2");
    let PlayerState::Failed(message) = a.state() else { panic!("{:?}", a.state()) };
    #[cfg(target_os = "macos")]
    assert!(message.contains(".webm isn't supported on macOS"), "{message}");
    #[cfg(not(target_os = "macos"))]
    assert_eq!(message, "video playback isn't supported on this platform yet");
    a.play();
    assert!(!a.is_playing(), "a failed clip doesn't play");
}

/// An audio clip no backend has seen, so nothing has changed before the test starts.
fn bare_audio() -> Arc<Shared> {
    Arc::new(Shared {
        source: MediaSource::from("missing.m4a"),
        headers: Vec::new(),
        kind: Kind::Audio,
        ctl: Mutex::new(Control { muted: false, kind: Kind::Audio, ..Control::default() }),
        wakes: Mutex::new(Vec::new()),
        waker: WakerSlot::default(),
        notified: AtomicU64::new(0),
        native: Mutex::new(backend::Native::default()),
        backend: &BACKEND,
    })
}

fn quiet_audio() -> Player {
    let shared = bare_audio();
    Player { surface: NativeSurface::with_video(shared.clone()), shared }
}

/// A video clip no backend has seen, so platform constructors (such as the unsupported backend's
/// immediate failure) don't pre-fail it before a test drives `Control`.
fn bare_video() -> Arc<Shared> {
    Arc::new(Shared {
        source: MediaSource::from("missing.mp4"),
        headers: Vec::new(),
        kind: Kind::Video,
        ctl: Mutex::new(Control { muted: true, kind: Kind::Video, ..Control::default() }),
        wakes: Mutex::new(Vec::new()),
        waker: WakerSlot::default(),
        notified: AtomicU64::new(0),
        native: Mutex::new(backend::Native::default()),
        backend: &BACKEND,
    })
}

fn quiet_video() -> Player {
    let shared = bare_video();
    Player { surface: NativeSurface::with_video(shared.clone()), shared }
}

#[test]
fn the_waker_hears_each_state_change_once() {
    let a = quiet_audio();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    a.set_waker(Arc::new(move || {
        seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    a.shared.settle(false);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0, "nothing changed yet");
    a.shared.ctl.lock().unwrap().open(Ok(Meta { duration: 2.0, size: None, frame: None, sound: false }));
    a.shared.settle(false);
    a.shared.settle(false);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    a.shared.ctl.lock().unwrap().fail("gone".into());
    a.shared.settle(false);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
}

/// An unsupported format fails inside the constructor, before any waker can be set: setting
/// one afterwards still delivers that failure, once.
#[cfg(not(target_os = "windows"))]
#[test]
fn a_waker_set_after_a_failure_hears_it_once() {
    let a = Player::url("tap.ogv");
    assert!(matches!(a.shared.snapshot().state, PlayerState::Failed(_)));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    a.set_waker(Arc::new(move || {
        seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    a.shared.settle(true);
    a.play();
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn a_seek_before_the_clip_opens_is_clamped_when_it_does() {
    let v = Video::file("missing.mp4");
    v.seek(30.0);
    assert_eq!(v.shared.ctl.lock().unwrap().pending, Some(Pending::Seek(Seek::exact(30.0))), "no duration yet");
    v.shared.ctl.lock().unwrap().open(Ok(meta()));
    let c = v.shared.ctl.lock().unwrap();
    assert_eq!((c.pending, c.time), (Some(Pending::Seek(Seek::exact(1.0))), 1.0));
}

#[test]
fn seek_fast_records_keyframe_seek_clamps_inputs_and_preserves_state() {
    let v = quiet_video();
    let other = v.clone();
    // Before opening: records exact=false, clamps negative/non-finite to 0.
    v.seek_fast(-5.0);
    assert_eq!(v.shared.ctl.lock().unwrap().pending, Some(Pending::Seek(Seek { at: 0.0, exact: false })));
    v.seek_fast(f64::NAN);
    assert_eq!(v.shared.ctl.lock().unwrap().pending, Some(Pending::Seek(Seek { at: 0.0, exact: false })));
    v.seek_fast(f64::INFINITY);
    assert_eq!(v.shared.ctl.lock().unwrap().pending, Some(Pending::Seek(Seek { at: 0.0, exact: false })));
    // Past duration before open: clamps when metadata arrives and keeps exact=false.
    other.seek_fast(30.0);
    assert_eq!(v.shared.ctl.lock().unwrap().pending, Some(Pending::Seek(Seek { at: 30.0, exact: false })));
    v.shared.ctl.lock().unwrap().open(Ok(meta()));
    {
        let c = v.shared.ctl.lock().unwrap();
        assert_eq!((c.pending, c.time), (Some(Pending::Seek(Seek { at: 1.0, exact: false })), 1.0));
    }
    // Headless runs report is_seeking() == false.
    assert!(!v.is_seeking());
    assert_eq!(v.state(), PlayerState::Ready);

    // Seeking while playing stays Playing; seeking after Ended returns to Ready.
    v.play();
    v.seek_fast(0.5);
    assert_eq!(v.state(), PlayerState::Playing);
    assert_eq!(v.shared.ctl.lock().unwrap().pending, Some(Pending::Seek(Seek { at: 0.5, exact: false })));
    assert!(!v.is_seeking());

    v.shared.ctl.lock().unwrap().run = Run::Ended;
    assert_eq!(v.state(), PlayerState::Ended);
    v.seek_fast(0.25);
    assert_eq!(v.state(), PlayerState::Ready);
    assert_eq!(v.shared.ctl.lock().unwrap().pending, Some(Pending::Seek(Seek { at: 0.25, exact: false })));

    // A subsequent exact seek replaces the pending keyframe seek.
    v.seek(0.75);
    assert_eq!(v.shared.ctl.lock().unwrap().pending, Some(Pending::Seek(Seek::exact(0.75))));

    // Failed clips never report seeking.
    v.shared.ctl.lock().unwrap().fail("boom".into());
    v.seek_fast(0.5);
    assert!(!v.is_seeking());
}

/// On platforms without a video backend, `seek_fast`, `seek`, and `step` do not panic, keep
/// the clip in `PlayerState::Failed`, and never report a pending seek or presented frame.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn unsupported_backend_handles_seek_fast_and_step_without_panicking() {
    let v = Video::file("missing.mp4");
    assert!(matches!(v.state(), PlayerState::Failed(_)));
    assert!(!v.is_seeking());
    assert_eq!(v.frame_duration(), None);
    assert_eq!(v.presented_time(), None);

    v.seek_fast(1.5);
    v.seek(0.5);
    v.step(1);
    v.step(-1);
    v.step(0);

    assert!(matches!(v.state(), PlayerState::Failed(_)));
    assert!(!v.is_seeking());
    assert_eq!(v.frame_duration(), None);
    assert_eq!(v.presented_time(), None);
}

#[test]
fn frame_duration_and_presented_time_handle_unopened_unknown_audio_and_failure() {
    let v = Video::file("missing.mp4");
    assert_eq!(v.frame_duration(), None);
    assert_eq!(v.presented_time(), None);

    // Stream with unknown frame rate keeps None for frame_duration() while snapping internally.
    let unknown = Meta { duration: 2.0, size: Some((64, 36)), frame: None, sound: false };
    v.shared.ctl.lock().unwrap().open(Ok(unknown));
    assert_eq!(v.frame_duration(), None);
    assert_eq!(v.presented_time(), None);
    assert!((unknown.frame_step() - DEFAULT_FRAME).abs() < 1e-12);

    // Known positive rate is exposed as-is.
    let known = Meta { duration: 1.0, size: Some((64, 36)), frame: Some(0.25), sound: false };
    v.shared.ctl.lock().unwrap().open(Ok(known));
    assert_eq!(v.frame_duration(), Some(0.25));

    // Audio always returns None for both and ignores step().
    let a = quiet_audio();
    a.shared.ctl.lock().unwrap().open(Ok(Meta { duration: 2.0, size: None, frame: Some(0.25), sound: true }));
    assert_eq!(a.frame_duration(), None);
    assert_eq!(a.presented_time(), None);
    a.play();
    a.step(1);
    assert!(a.is_playing(), "audio ignores Player::step");
    assert_eq!(a.shared.ctl.lock().unwrap().pending, None);
}

/// Runs a synchronous headless decode test behind a hard deadline so a backend deadlock fails
/// with a message instead of hanging the test suite.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn run_with_timeout(name: &str, limit: std::time::Duration, body: impl FnOnce() + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        // The test thread may have already timed out and dropped the receiver.
        tx.send(res).ok();
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(())) => {}
        Ok(Err(payload)) => std::panic::resume_unwind(payload),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("{name} timed out after {}s", limit.as_secs());
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("{name} worker exited without reporting a result");
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn headless_step_and_presented_time_walk_every_frame_of_real_mp4_and_clamp() {
    run_with_timeout("headless_step_and_presented_time_walk_every_frame_of_real_mp4_and_clamp", std::time::Duration::from_secs(20), || {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures/rgbw.mp4");
    let v = Video::file(path);
    let wake = rune_core::live::Wake::new(Arc::new(|| {}));
    let pull = |clock: f64| v.shared.pull(v.surface(), Clock::Virtual(clock), &wake).active;

    // Before first pull: unopened, no presented picture.
    assert_eq!(v.frame_duration(), None);
    assert_eq!(v.presented_time(), None);

    // First pull opens and decodes frame 0 (PTS 0.0, 4 fps -> 0.25 s/frame).
    assert!(!pull(0.0));
    assert!((v.frame_duration().unwrap() - 0.25).abs() < 1e-3, "{:?}", v.frame_duration());
    assert!(v.presented_time().unwrap().abs() < 1e-3, "{:?}", v.presented_time());
    assert!(!v.is_seeking());

    // Stepping backward at frame 0 stays on frame 0 without seeking.
    v.step(-1);
    assert!(!pull(0.0));
    assert!(v.presented_time().unwrap().abs() < 1e-3, "{:?}", v.presented_time());

    // Step forward through frames 1, 2, 3 and verify exact PTS at each step.
    for expected in [0.25, 0.5, 0.75] {
        v.step(1);
        assert!(!v.is_playing());
        assert!(!pull(0.0));
        let pts = v.presented_time().expect("presented_time after step");
        assert!((pts - expected).abs() < 1e-3, "got {pts}, want {expected}");
    }

    // Stepping forward past last frame clamps on frame 3 (0.75 s).
    v.step(10);
    assert!(!pull(0.0));
    assert!((v.presented_time().unwrap() - 0.75).abs() < 1e-3, "{:?}", v.presented_time());

    // Multi-frame backward step from frame 3 to frame 1 (0.25 s).
    v.step(-2);
    assert!(!pull(0.0));
    assert!((v.presented_time().unwrap() - 0.25).abs() < 1e-3, "{:?}", v.presented_time());

    // Seeking to the middle of frame 2 (0.6 s): time() reports 0.6 while presented_time() reports 0.5.
    v.seek(0.6);
    assert!(!pull(0.0));
    assert!((v.time() - 0.6).abs() < 1e-6, "{}", v.time());
    assert!((v.presented_time().unwrap() - 0.5).abs() < 1e-3, "{:?}", v.presented_time());

    // Stepping while playing pauses playback and advances one frame to frame 3 (0.75 s).
    v.play();
    assert!(pull(0.0));
    v.step(1);
    assert!(!v.is_playing());
    assert!(!pull(0.5), "paused after step requests no further frames");
    assert!((v.presented_time().unwrap() - 0.75).abs() < 1e-3, "{:?}", v.presented_time());

    // step(0) while playing pauses without moving off the current frame.
    v.play();
    assert!(pull(0.5));
    v.step(0);
    assert!(!v.is_playing());
    assert!(!pull(0.6));
    assert!((v.presented_time().unwrap() - 0.75).abs() < 1e-3, "{:?}", v.presented_time());
    });
}

/// The parked list is process-wide: these tests take turns so each sees only its own clips.
#[cfg(target_os = "macos")]
static PARKED_TESTS: Mutex<()> = Mutex::new(());

/// `play()` on a parked clip only unparks it on its next pull or edit on the main thread;
/// until then, parking others must not release it.
#[cfg(target_os = "macos")]
#[test]
fn parking_never_releases_a_playing_clip() {
    let _only = PARKED_TESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let clips: Vec<Video> = (0..6).map(|_| Video::file(fixture())).collect();
    let mut released = Vec::new();
    released.extend(backend::parked_with(&clips[0].shared));
    clips[0].play();
    for clip in &clips[1..] {
        released.extend(backend::parked_with(&clip.shared));
    }
    assert!(!released.iter().any(|r| Arc::ptr_eq(r, &clips[0].shared)), "the playing clip stays");
    assert_eq!(released.len(), 1, "four of the five paused clips stay parked");
    assert!(Arc::ptr_eq(&released[0], &clips[1].shared), "the oldest paused clip goes");
}

/// Records, on drop, whether the parking pass's lock was free: a destructor that takes it would
/// deadlock in the app.
#[cfg(target_os = "macos")]
struct DroppedDuringParking;

#[cfg(target_os = "macos")]
thread_local! {
    /// None until a clip's destructor runs: Some(free) for whether PARKED was free then.
    static DROPPED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(target_os = "macos")]
impl Drop for DroppedDuringParking {
    fn drop(&mut self) {
        DROPPED.with(|flag| flag.set(Some(macos::parked_free())));
    }
}

/// A clip's last owner can go away while a parking pass holds PARKED. Its destructor runs app
/// code (the waker captures), which may play another parked clip and re-enter `unpark`, so it
/// must run after the lock is released. // differs from dev: dev dropped the upgrade in the pass.
#[cfg(target_os = "macos")]
#[test]
fn a_clip_dropped_during_a_parking_pass_is_destroyed_outside_the_lock() {
    let _only = PARKED_TESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let target = bare_audio();
    let probe = DroppedDuringParking;
    target.waker.set(Arc::new(move || {
        let _ = &probe;
    }));
    let other = bare_audio();
    // Nothing to release: the list holds one paused clip.
    assert!(backend::parked_with(&target).is_empty());
    // The pass over `other` drops the target's last owner while it holds the target's upgrade.
    macos::on_kept(move || drop(target));
    assert!(backend::parked_with(&other).is_empty());
    assert_eq!(
        DROPPED.with(|flag| flag.take()),
        Some(true),
        "the clip's destructor ran while PARKED was held (it would deadlock here)"
    );
}

/// A redraw of a parked clip re-parks it every frame, where dev allocated nothing: the pass's
/// upgraded owners keep their capacity across passes. // differs from dev: dev's temporary
/// upgrades needed no buffer at all.
#[cfg(target_os = "macos")]
#[test]
fn a_warm_parking_pass_keeps_its_buffers_capacity() {
    let _only = PARKED_TESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let clips: Vec<Video> = (0..4).map(|_| Video::file(fixture())).collect();
    // Park all four, then pass over them until the buffer fits them.
    for _ in 0..5 {
        for clip in &clips {
            assert!(backend::parked_with(&clip.shared).is_empty(), "four parked clips fit");
        }
    }
    let warm = macos::held_capacity();
    assert!(warm >= clips.len(), "the pass keeps every upgraded clip: {warm}");
    for _ in 0..100 {
        for clip in &clips {
            assert!(backend::parked_with(&clip.shared).is_empty(), "nothing is evicted");
        }
    }
    assert_eq!(macos::held_capacity(), warm, "a warm redraw of parked clips allocates nothing");
}

/// Only formats that no build reads fail at once; with `ogg-audio`, Ogg audio opens later.
#[cfg(target_os = "macos")]
#[test]
fn only_formats_no_build_reads_fail_at_once() {
    let ogv = Player::audio("https://example.com/clip.ogv");
    assert!(matches!(ogv.shared.snapshot().state, PlayerState::Failed(e) if e.contains(".ogv")));
    let ogg = Player::audio("https://example.com/clip.ogg");
    let failed = matches!(ogg.shared.snapshot().state, PlayerState::Failed(_));
    assert_eq!(failed, !cfg!(feature = "ogg-audio"), "{:?}", ogg.shared.snapshot().state);
}

#[test]
fn handle_edits_are_shared_and_validated() {
    let v = Video::url("https://example.com/clip.mp4");
    let other = v.clone();
    other.set_rate(2.0);
    other.set_rate(-1.0);
    other.set_rate(f32::NAN);
    other.set_looping(true);
    other.set_muted(false);
    other.seek(-3.0);
    assert_eq!(v.rate(), 2.0);
    assert!(v.looping());
    assert!(!v.muted());
    assert_eq!(v.time(), 0.0);
    v.play();
    #[cfg(target_os = "macos")]
    {
        assert!(other.is_playing());
        // Still loading: playing is intent until the clip opens.
        assert_eq!(v.state(), PlayerState::Loading);
    }
}

#[test]
fn state_follows_open_play_end_and_failure() {
    let mut c = Control::default();
    c.open(Ok(meta()));
    assert_eq!(c.state(), PlayerState::Ready);
    assert_eq!((c.duration, c.size), (Some(1.0), Some((64, 36))));
    step(&mut c, Event::Play);
    assert_eq!(c.state(), PlayerState::Playing);
    c.advance_virtual(10.0);
    c.advance_virtual(10.5);
    assert_eq!(c.time, 0.5);
    let generation = c.generation;
    c.advance_virtual(11.25);
    assert_eq!((c.time, &c.run), (1.0, &Run::Ended));
    assert_eq!(c.state(), PlayerState::Ended);
    assert!(c.generation > generation, "ending re-runs views");
    c.fail("boom".into());
    c.fail("second".into());
    assert_eq!(c.state(), PlayerState::Failed("boom".into()));
}

#[test]
fn virtual_time_loops_rates_and_seeks() {
    let mut c = Control::default();
    c.open(Ok(meta()));
    step(&mut c, Event::Play);
    c.looping = true;
    c.rate = 2.0;
    c.advance_virtual(0.0);
    c.advance_virtual(0.6);
    assert!((c.time - 0.2).abs() < 1e-9, "1.2 s wraps to 0.2: {}", c.time);
    assert!(c.playing());
    step(&mut c, Event::Seek { to: 0.75, exact: true });
    c.advance_virtual(5.0);
    assert_eq!(c.time, 0.75);
    c.advance_virtual(5.1);
    assert!((c.time - 0.95).abs() < 1e-9);
    // Paused time stays put whatever the clock does.
    step(&mut c, Event::Pause);
    c.advance_virtual(9.0);
    assert!((c.time - 0.95).abs() < 1e-9);
}

#[test]
fn playing_after_the_end_restarts_from_zero() {
    let v = Video::file("missing.mp4");
    {
        let mut c = v.shared.ctl.lock().unwrap();
        c.open(Ok(meta()));
        c.time = 1.0;
        c.run = Run::Ended;
    }
    assert_eq!(v.state(), PlayerState::Ended);
    v.play();
    let c = v.shared.ctl.lock().unwrap();
    assert_eq!(c.pending, Some(Pending::Seek(Seek::exact(0.0))));
    assert!(c.playing());
}

#[test]
fn display_positions_loop_or_end() {
    let mut c = Control::default();
    c.open(Ok(meta()));
    step(&mut c, Event::Play);
    assert!(!c.observe(0.4, false));
    assert_eq!(c.time, 0.4);
    c.looping = true;
    assert!(c.observe(1.0, true), "looping restarts the player");
    assert_eq!((c.time, c.playing()), (0.0, true));
    c.looping = false;
    assert!(!c.observe(1.2, true));
    assert_eq!((c.time, c.playing(), c.state()), (1.0, false, PlayerState::Ended));
}

#[test]
fn frames_snap_to_whole_frames_inside_the_clip() {
    let m = meta();
    assert_eq!(m.frames(), 4);
    assert_eq!(m.frame_index(0.0), 0);
    assert_eq!(m.frame_index(0.2499), 0);
    assert_eq!(m.frame_index(0.25), 1);
    assert_eq!(m.frame_index(1.0), 3, "the end shows the last frame");
    assert_eq!(m.frame_index(-1.0), 0);
    assert_eq!(m.frame_time(1), 0.375);
    assert_eq!(m.frame_time(3), 0.875);
}

#[test]
fn the_last_handle_or_element_frees_the_player() {
    let v = Video::file("missing.mp4");
    let shared = Arc::downgrade(&v.shared);
    // An element keeps the surface, and through it the player.
    let in_tree = v.surface().clone();
    drop(v);
    assert!(shared.upgrade().is_some());
    drop(in_tree);
    assert!(shared.upgrade().is_none());
}

#[cfg(target_os = "macos")]
fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures/rgbw.mp4")
}

#[cfg(target_os = "macos")]
fn sound(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures").join(name)
}

/// Headless audio opens on first read and never plays: no player, no sound. With no runtime
/// stepping on its thread, time holds.
#[cfg(target_os = "macos")]
#[test]
fn headless_audio_opens_on_read_and_holds_time() {
    let a = Player::audio(sound("silence.m4a"));
    a.set_muted(true);
    assert_eq!(a.state(), PlayerState::Ready);
    let d = a.duration().unwrap();
    assert!((d - 1.0).abs() < 0.1, "{d}");
    assert_eq!(a.natural_size(), None);
    a.play();
    assert_eq!(a.state(), PlayerState::Playing);
    assert_eq!(a.time(), 0.0);
    a.seek(0.4);
    assert_eq!(a.time(), 0.4);

    let wav = Player::audio(sound("silence.wav"));
    assert!((wav.duration().unwrap() - 0.5).abs() < 0.01, "{:?}", wav.duration());
    let video = Player::audio(fixture());
    assert!(matches!(video.state(), PlayerState::Failed(e) if e.contains("no playable audio track")));
}

#[test]
fn requests_carry_headers_without_printing_their_values() {
    let request = Player::request("https://example.com/media/a.m4a")
        .header("Authorization", "Bearer secret-token")
        .header("authorization", "Bearer newer-token")
        .header("X-Board", "1");
    let printed = format!("{request:?}");
    assert!(printed.contains("Authorization") || printed.contains("authorization"), "{printed}");
    assert!(!printed.contains("token"), "header values never print: {printed}");
    let a = request.audio();
    assert!(a.is_audio());
    assert_eq!(a.source(), &MediaSource::Url("https://example.com/media/a.m4a".into()));
    assert_eq!(a.shared.headers, [("authorization".to_string(), "Bearer newer-token".to_string()), ("X-Board".into(), "1".into())]);
    assert!(!format!("{a:?}").contains("token"));
    let v = Player::request("https://example.com/media/a.mp4").header("Authorization", "Bearer t").video();
    assert!(!v.is_audio() && v.muted());
}

/// A local HTTP server that serves `body` (with byte ranges) only to requests carrying
/// `Authorization: Bearer let-me-in`, and counts the ones that didn't.
#[cfg(target_os = "macos")]
fn serve_with_auth(body: Vec<u8>, content_type: &'static str) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/media/clip", listener.local_addr().unwrap().port());
    let refused = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = refused.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let (body, count) = (body.clone(), count.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                loop {
                    let (mut authorized, mut range) = (false, None);
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 { return; }
                        let header = header.trim_end();
                        if header.is_empty() { break; }
                        let (name, value) = header.split_once(':').unwrap_or((header, ""));
                        match name.trim().to_ascii_lowercase().as_str() {
                            "authorization" => authorized = value.trim() == "Bearer let-me-in",
                            "range" => range = value.trim().strip_prefix("bytes=").map(str::to_owned),
                            _ => {}
                        }
                    }
                    if !authorized {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n").ok();
                        continue;
                    }
                    let len = body.len();
                    let (start, end) = range.and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        let a: usize = a.parse().ok()?;
                        let b = b.parse::<usize>().map_or(len - 1, |b| b.min(len - 1));
                        Some((a, b))
                    }).unwrap_or((0, len - 1));
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}\r\n\r\n",
                        end + 1 - start
                    );
                    if stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body[start..=end])).is_err() { return; }
                }
            });
        }
    });
    (url, refused)
}

/// AVFoundation sends the request's headers: the clip opens from a server that refuses
/// anything without them, and fails without them.
#[cfg(target_os = "macos")]
#[test]
fn headless_audio_opens_with_request_headers() {
    let (url, refused) = serve_with_auth(std::fs::read(sound("silence.wav")).unwrap(), "audio/wav");
    let a = Player::request(format!("{url}.wav")).header("Authorization", "Bearer let-me-in").audio();
    assert_eq!(a.state(), PlayerState::Ready);
    assert!((a.duration().unwrap() - 0.5).abs() < 0.01, "{:?}", a.duration());
    assert_eq!(refused.load(std::sync::atomic::Ordering::SeqCst), 0, "every request carried the header");

    let without = Player::audio(format!("{url}-2.wav"));
    assert!(matches!(without.state(), PlayerState::Failed(_)), "{:?}", without.state());
    assert!(refused.load(std::sync::atomic::Ordering::SeqCst) > 0);
}

/// A replaced waker's captures drop after the slot's lock: one may read the clip's state and
/// would deadlock there. // differs from dev: dev dropped the old waker under the lock.
#[test]
fn a_replaced_waker_is_dropped_outside_the_slots_lock() {
    static FREED: Mutex<Option<bool>> = Mutex::new(None);
    struct Probe(Arc<WakerSlot>);
    impl Drop for Probe {
        fn drop(&mut self) {
            *FREED.lock().unwrap() = Some(self.0.free());
        }
    }
    let slot = Arc::new(WakerSlot::default());
    let first = Probe(slot.clone());
    slot.set(Arc::new(move || {
        let _ = &first;
    }));
    slot.set(Arc::new(|| {}));
    assert_eq!(*FREED.lock().unwrap(), Some(true), "the replaced waker dropped while the slot's lock was held");
}

#[cfg(target_os = "macos")]
#[test]
fn preload_decodes_the_first_frame_before_anything_draws_it() {
    let v = Video::file(fixture());
    assert_eq!(macos::test_poster(&v.shared), None);
    // Preload is a no-op headless (frames decode as they draw), so call the backend directly.
    macos::preload(&v.shared);
    let start = rune_core::time::now();
    while macos::test_poster(&v.shared).is_none() && start.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(macos::test_poster(&v.shared), Some(true), "the first frame decoded off the main thread");
    v.preload();
    let audio = Player::audio(sound("silence.wav"));
    macos::preload(&audio.shared);
    assert_eq!(macos::test_poster(&audio.shared), None, "audio has no poster to warm");
}

/// Exercises the synchronous headless virtual-clock (`Clock::Virtual`) path across exact seeks
/// spanning the full width of each frame followed by `Player::step`. Display playback paths
/// (`Clock::Display`) are verified separately in `tests/step_after_seek_macos.rs` and
/// `tests/step_after_seek_windows.rs`.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn headless_step_after_exact_seek_uses_presented_frame_across_whole_frame_width() {
    run_with_timeout("headless_step_after_exact_seek_uses_presented_frame_across_whole_frame_width", std::time::Duration::from_secs(20), || {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures/rgbw.mp4");
    let v = Video::file(path);
    let wake = rune_core::live::Wake::new(Arc::new(|| {}));
    let pull = |clock: f64| v.shared.pull(v.surface(), Clock::Virtual(clock), &wake).active;
    assert!(!pull(0.0));
    let tick = 1.0 / 16_384.0;
    for (seek_t, from_pts, delta, want_pts) in [
        (0.00, 0.00, 1, 0.25),
        (0.06, 0.00, 1, 0.25),
        (0.125, 0.00, 1, 0.25),
        (0.18, 0.00, 1, 0.25),
        (0.25 - tick, 0.00, 1, 0.25),
        (0.25, 0.25, -1, 0.00),
        (0.43, 0.25, 1, 0.50),
        (0.43, 0.25, -1, 0.00),
        (0.50 - tick, 0.25, 1, 0.50),
        (0.50 - tick, 0.25, -1, 0.00),
        (0.68, 0.50, 1, 0.75),
        (0.68, 0.50, -1, 0.25),
        (0.93, 0.75, -1, 0.50),
        (1.00 - tick, 0.75, -1, 0.50),
    ] {
        v.seek(seek_t);
        assert!(!pull(0.0));
        let pts = v.presented_time().unwrap();
        assert!((pts - from_pts).abs() < 1e-3, "seek({seek_t}): got {pts}, want {from_pts}");
        v.step(delta);
        assert!(!pull(0.0));
        let after = v.presented_time().unwrap();
        assert!((after - want_pts).abs() < 1e-3, "seek({seek_t}) then step({delta:+}): got {after}, want {want_pts}");
    }
    // First and last frame clamps after seeking into the second half of frame 0 / frame 3.
    v.seek(0.18);
    assert!(!pull(0.0));
    v.step(-1);
    assert!(!pull(0.0));
    assert!(v.presented_time().unwrap().abs() < 1e-3);

    v.seek(0.93);
    assert!(!pull(0.0));
    v.step(1);
    assert!(!pull(0.0));
    assert!((v.presented_time().unwrap() - 0.75).abs() < 1e-3);
    });
}
