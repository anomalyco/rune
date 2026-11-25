//! Off-screen AVFoundation verification of `Player::step` after exact seeks (CFR and VFR).
//! Runs on the main thread without opening a window, activating AppKit, or touching a GPU.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn main() {
    use rune_media::video::{Clock, enable_playback};
    use rune_media::{Player, PlayerState};
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
    }

    fn pump(secs: f64) {
        // SAFETY: called on the main thread; `kCFRunLoopDefaultMode` is a static CoreFoundation constant.
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, secs, 0) };
    }

    fn pull(v: &Player, wake: &Arc<rune_core::live::Wake>) {
        let _ = v.surface().pull_video(Clock::Display(rune_core::time::now()), wake);
    }

    fn wait_until(what: &str, v: &Player, wake: &Arc<rune_core::live::Wake>, limit: Duration, mut pred: impl FnMut() -> bool) {
        let start = rune_core::time::now();
        while !pred() {
            assert!(
                start.elapsed() < limit,
                "timed out waiting for {what}: state={:?}, time={:?}, presented={:?}, seeking={}",
                v.state(),
                v.time(),
                v.presented_time(),
                v.is_seeking(),
            );
            pump(0.005);
            pull(v, wake);
        }
    }

    /// Rewrites `rgbw.mp4`'s `stts` table from 4x4096 (constant 0.25 s) to [2048, 6144, 2048, 6144]
    /// (0.125 s, 0.375 s, 0.125 s, 0.375 s; same 16384-tick total duration) so AVFoundation decodes
    /// a real H.264 variable-frame-rate clip with frames at 0.000, 0.125, 0.500, 0.625 s.
    fn write_vfr_fixture(src: &Path) -> PathBuf {
        let raw = std::fs::read(src).expect("read rgbw.mp4");
        let u32_be = |b: &[u8], o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let put_u32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_be_bytes());
        // Locate the stts box and its enclosing ancestors plus stco.
        let mut out = raw.clone();
        let stts_off = out.windows(4).position(|w| w == b"stts").expect("stts box") - 4;
        let stts_size = u32_be(&out, stts_off) as usize;
        assert_eq!(stts_size, 24, "expected single-entry stts");
        let mut new_stts = Vec::with_capacity(48);
        new_stts.extend_from_slice(&48u32.to_be_bytes());
        new_stts.extend_from_slice(b"stts");
        new_stts.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        new_stts.extend_from_slice(&4u32.to_be_bytes()); // 4 entries
        for delta in [2048u32, 6144, 2048, 6144] {
            new_stts.extend_from_slice(&1u32.to_be_bytes());
            new_stts.extend_from_slice(&delta.to_be_bytes());
        }
        let delta_len = (new_stts.len() - stts_size) as u32;
        out.splice(stts_off..stts_off + stts_size, new_stts);
        // Bump sizes of all enclosing container boxes and the stco chunk offset.
        for tag in [b"moov", b"trak", b"mdia", b"minf", b"stbl"] {
            let off = out.windows(4).position(|w| w == tag).expect("container box") - 4;
            let sz = u32_be(&out, off);
            put_u32(&mut out, off, sz + delta_len);
        }
        let stco_off = out.windows(4).position(|w| w == b"stco").expect("stco box") - 4;
        let count = u32_be(&out, stco_off + 12) as usize;
        for i in 0..count {
            let entry = stco_off + 16 + i * 4;
            let val = u32_be(&out, entry);
            put_u32(&mut out, entry, val + delta_len);
        }
        let path = std::env::temp_dir().join(format!("rune-media-vfr-{}.mp4", std::process::id()));
        std::fs::write(&path, out).expect("write vfr fixture");
        path
    }

    fn open_paused_avplayer(path: &Path) -> (Player, Arc<rune_core::live::Wake>, Arc<AtomicUsize>) {
        let v = Player::file(path);
        let pushes = Arc::new(AtomicUsize::new(0));
        let p = pushes.clone();
        v.surface().set_waker(Arc::new(move || {
            p.fetch_add(1, Ordering::SeqCst);
        }));
        let wake = rune_core::live::Wake::new(Arc::new(|| {}));
        // Create and warm the real AVPlayer (Stage::Ready) off-screen, then pause.
        v.play();
        wait_until("initial frame", &v, &wake, Duration::from_secs(10), || {
            v.presented_time().is_some() && v.state() == PlayerState::Playing
        });
        v.pause();
        pull(&v, &wake);
        pump(0.05);
        pull(&v, &wake);
        assert_eq!(v.state(), PlayerState::Ready);
        (v, wake, pushes)
    }

    fn exact_seek_and_settle(
        v: &Player,
        wake: &Arc<rune_core::live::Wake>,
        target: f64,
        expected_pts: f64,
        other_pts_time: f64,
    ) {
        // Ensure we enter the target frame fresh so AVPlayer produces a landed picture at expected_pts.
        if v.presented_time().is_some_and(|p| (p - expected_pts).abs() < 1e-3) {
            v.seek(other_pts_time);
            pull(v, wake);
            wait_until("move away before seek", v, wake, Duration::from_secs(5), || {
                !v.is_seeking() && v.presented_time().is_some_and(|p| (p - expected_pts).abs() > 0.05)
            });
        }
        v.seek(target);
        pull(v, wake);
        wait_until("seek land", v, wake, Duration::from_secs(5), || {
            !v.is_seeking() && v.presented_time().is_some_and(|p| (p - expected_pts).abs() < 1e-3)
        });
        // Let AVPlayer settle its internal seek queue, matching a paused user pause before stepping.
        for _ in 0..8 {
            pump(0.01);
            pull(v, wake);
        }
        let pts = v.presented_time().expect("presented_time after seek");
        assert!((pts - expected_pts).abs() < 1e-3, "seek({target}) landed on {pts}, expected {expected_pts}");
    }

    enable_playback();

    let cfr_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures/rgbw.mp4");
    let (v, wake, pushes) = open_paused_avplayer(&cfr_path);
    let fd = v.frame_duration().expect("frame_duration");
    assert!((fd - 0.25).abs() < 1e-3, "expected 0.25 s frame_duration, got {fd}");

    // 1. CFR table: exact seek to boundary, first half, midpoint, second half, and 1 tick before
    // next boundary, followed by step(+1) and step(-1).
    let tick = 1.0 / 16_384.0;
    let cfr_cases: &[(&str, f64, f64, i32, f64)] = &[
        ("f0 boundary +1", 0.00, 0.00, 1, 0.25),
        ("f0 first-half +1", 0.06, 0.00, 1, 0.25),
        ("f0 midpoint +1", 0.125, 0.00, 1, 0.25),
        ("f0 second-half +1", 0.18, 0.00, 1, 0.25),
        ("f0 1-tick-before-f1 +1", 0.25 - tick, 0.00, 1, 0.25),
        ("f1 boundary +1", 0.25, 0.25, 1, 0.50),
        ("f1 first-half +1", 0.31, 0.25, 1, 0.50),
        ("f1 second-half +1", 0.43, 0.25, 1, 0.50),
        ("f1 1-tick-before-f2 +1", 0.50 - tick, 0.25, 1, 0.50),
        ("f1 boundary -1", 0.25, 0.25, -1, 0.00),
        ("f1 first-half -1", 0.31, 0.25, -1, 0.00),
        ("f1 second-half -1", 0.43, 0.25, -1, 0.00),
        ("f1 1-tick-before-f2 -1", 0.50 - tick, 0.25, -1, 0.00),
        ("f2 first-half +1", 0.56, 0.50, 1, 0.75),
        ("f2 second-half +1", 0.68, 0.50, 1, 0.75),
        ("f2 first-half -1", 0.56, 0.50, -1, 0.25),
        ("f2 second-half -1", 0.68, 0.50, -1, 0.25),
        ("f3 boundary -1", 0.75, 0.75, -1, 0.50),
        ("f3 first-half -1", 0.81, 0.75, -1, 0.50),
        ("f3 second-half -1", 0.93, 0.75, -1, 0.50),
        ("f3 1-tick-before-end -1", 1.00 - tick, 0.75, -1, 0.50),
    ];

    let mut failures = Vec::new();
    for &(label, seek_t, from_pts, delta, want_pts) in cfr_cases {
        let other = if from_pts < 0.5 { 0.75 } else { 0.0 };
        exact_seek_and_settle(&v, &wake, seek_t, from_pts, other);
        let before_pushes = pushes.load(Ordering::SeqCst);
        v.step(delta);
        pull(&v, &wake);
        wait_until(label, &v, &wake, Duration::from_secs(5), || !v.is_seeking());
        for _ in 0..8 {
            pump(0.01);
            pull(&v, &wake);
        }
        let got_pts = v.presented_time().expect("presented_time after step");
        let pushed = pushes.load(Ordering::SeqCst) - before_pushes;
        let got_k = (got_pts / fd).round() as i32;
        let want_k = (want_pts / fd).round() as i32;
        println!("CFR {label:26} | seek={seek_t:.6} from={from_pts:.2} step={delta:+} -> got={got_pts:.2} (k={got_k}) want={want_pts:.2} (k={want_k}) pushes={pushed}");
        if (got_pts - want_pts).abs() >= 1e-3 || pushed != 1 {
            failures.push(format!("CFR {label}: got={got_pts:.4} want={want_pts:.4} pushes={pushed}"));
        }
    }

    // 2. First and last frame clamps.
    exact_seek_and_settle(&v, &wake, 0.18, 0.0, 0.75);
    let p0 = pushes.load(Ordering::SeqCst);
    v.step(-1);
    pull(&v, &wake);
    assert!(!v.is_seeking(), "step(-1) at frame 0 must not seek");
    assert_eq!(pushes.load(Ordering::SeqCst), p0, "step(-1) at frame 0 pushes no frame");

    exact_seek_and_settle(&v, &wake, 0.93, 0.75, 0.0);
    let p3 = pushes.load(Ordering::SeqCst);
    v.step(1);
    pull(&v, &wake);
    assert!(!v.is_seeking(), "step(+1) at last frame must not seek");
    assert_eq!(pushes.load(Ordering::SeqCst), p3, "step(+1) at last frame pushes no frame");

    // 3. Step after step (forward and backward across all 4 frames).
    exact_seek_and_settle(&v, &wake, 0.0, 0.0, 0.75);
    for (delta, want) in [(1, 0.25), (1, 0.50), (1, 0.75), (-1, 0.50), (-1, 0.25), (-1, 0.00)] {
        let before = pushes.load(Ordering::SeqCst);
        v.step(delta);
        pull(&v, &wake);
        wait_until("step-after-step", &v, &wake, Duration::from_secs(5), || !v.is_seeking());
        for _ in 0..5 {
            pump(0.01);
            pull(&v, &wake);
        }
        let got = v.presented_time().unwrap();
        let pushed = pushes.load(Ordering::SeqCst) - before;
        println!("CFR step-after-step        | step={delta:+} -> got={got:.2} want={want:.2} pushes={pushed}");
        if (got - want).abs() >= 1e-3 || pushed != 1 {
            failures.push(format!("CFR step-after-step ({delta:+}): got={got:.4} want={want:.4} pushes={pushed}"));
        }
    }

    // 3b. A seek arriving while a snap-before-step is in flight cancels the snap cleanly.
    exact_seek_and_settle(&v, &wake, 0.18, 0.0, 0.75);
    v.step(1);
    pull(&v, &wake);
    v.seek(0.75);
    pull(&v, &wake);
    wait_until("seek overrides in-flight snap", &v, &wake, Duration::from_secs(5), || {
        !v.is_seeking() && v.presented_time().is_some_and(|p| (p - 0.75).abs() < 1e-3)
    });
    for _ in 0..8 {
        pump(0.01);
        pull(&v, &wake);
    }
    let after_override = v.presented_time().unwrap();
    assert!((after_override - 0.75).abs() < 1e-3, "cancelled snap must not step off 0.75, got {after_override}");

    // 4. VFR clip: frames at 0.000, 0.125, 0.500, 0.625 s.
    let vfr_path = write_vfr_fixture(&cfr_path);
    let (vfr, vfr_wake, vfr_pushes) = open_paused_avplayer(&vfr_path);
    let vfr_cases: &[(&str, f64, f64, i32, f64)] = &[
        ("vfr f1 second-half +1", 0.40, 0.125, 1, 0.500),
        ("vfr f1 second-half -1", 0.40, 0.125, -1, 0.000),
        ("vfr f3 second-half -1", 0.90, 0.625, -1, 0.500),
        ("vfr f0 second-half +1", 0.10, 0.000, 1, 0.125),
    ];
    for &(label, seek_t, from_pts, delta, want_pts) in vfr_cases {
        let other = if from_pts < 0.5 { 0.625 } else { 0.0 };
        exact_seek_and_settle(&vfr, &vfr_wake, seek_t, from_pts, other);
        let before = vfr_pushes.load(Ordering::SeqCst);
        vfr.step(delta);
        pull(&vfr, &vfr_wake);
        wait_until(label, &vfr, &vfr_wake, Duration::from_secs(5), || !vfr.is_seeking());
        for _ in 0..8 {
            pump(0.01);
            pull(&vfr, &vfr_wake);
        }
        let got = vfr.presented_time().unwrap();
        let pushed = vfr_pushes.load(Ordering::SeqCst) - before;
        println!("VFR {label:26} | seek={seek_t:.6} from={from_pts:.3} step={delta:+} -> got={got:.3} want={want_pts:.3} pushes={pushed}");
        if (got - want_pts).abs() >= 1e-3 || pushed != 1 {
            failures.push(format!("VFR {label}: got={got:.4} want={want_pts:.4} pushes={pushed}"));
        }
    }
    drop(vfr);
    std::fs::remove_file(&vfr_path).expect("remove temp vfr fixture");

    assert!(failures.is_empty(), "step-after-seek failures:\n{}", failures.join("\n"));
}
