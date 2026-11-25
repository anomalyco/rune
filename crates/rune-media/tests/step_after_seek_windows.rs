//! Off-screen Windows Media Foundation verification of `Player::step` after exact seeks on the
//! real `Clock::Display` playback path. Decodes through Media Foundation without opening a window
//! or presenting a swap chain.

#[cfg(target_os = "windows")]
mod windows_display_step {
    use rune_media::video::{Clock, enable_playback};
    use rune_media::{Player, PlayerState};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn pull(v: &Player, wake: &Arc<rune_core::live::Wake>) {
        let _ = v.surface().pull_video(Clock::Display(rune_core::time::now()), wake);
    }

    fn wait_until(
        what: &str,
        v: &Player,
        wake: &Arc<rune_core::live::Wake>,
        limit: Duration,
        mut pred: impl FnMut() -> bool,
    ) {
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
            std::thread::sleep(Duration::from_millis(5));
            pull(v, wake);
        }
    }

    fn exact_seek_and_settle(
        v: &Player,
        wake: &Arc<rune_core::live::Wake>,
        target: f64,
        expected_pts: f64,
        other_pts_time: f64,
    ) {
        if v.presented_time().is_some_and(|p| (p - expected_pts).abs() < 1e-3) {
            v.seek(other_pts_time);
            pull(v, wake);
            wait_until("move away before seek", v, wake, Duration::from_secs(10), || {
                !v.is_seeking() && v.presented_time().is_some_and(|p| (p - expected_pts).abs() > 0.05)
            });
        }
        v.seek(target);
        pull(v, wake);
        wait_until("seek land", v, wake, Duration::from_secs(10), || {
            !v.is_seeking() && v.presented_time().is_some_and(|p| (p - expected_pts).abs() < 1e-3)
        });
        let Some(pts) = v.presented_time() else {
            panic!("missing presented_time after seek({target})");
        };
        assert!(
            (pts - expected_pts).abs() < 1e-3,
            "seek({target}) landed on {pts}, expected {expected_pts}"
        );
    }

    #[test]
    fn windows_display_step_after_exact_seek_completes_and_lands_on_neighbour_frame() {
        enable_playback();

        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures/rgbw.mp4");
        let v = Player::file(path);
        let pushes = Arc::new(AtomicUsize::new(0));
        let p = pushes.clone();
        v.surface().set_waker(Arc::new(move || {
            p.fetch_add(1, Ordering::SeqCst);
        }));
        let wake = rune_core::live::Wake::new(Arc::new(|| {}));

        // Open the real Media Foundation decoder off-screen (falls back to CPU when no window
        // presenting device is registered) and wait for frame 0 to land paused.
        pull(&v, &wake);
        wait_until("initial frame", &v, &wake, Duration::from_secs(15), || {
            v.presented_time().is_some() && v.state() == PlayerState::Ready && !v.is_seeking()
        });

        let fd = v.frame_duration().expect("frame_duration");
        assert!((fd - 0.25).abs() < 1e-3, "expected 0.25 s frame_duration, got {fd}");

        // Exact seeks across boundary, first half, midpoint, second half, and 1 tick before the
        // next boundary, forward and backward. Windows `Landing::keeps` selects the nearest frame
        // (`time + frame * 0.5 >= target`), and `Player::step` steps by `delta` from that
        // presented frame.
        let tick = 1.0 / 16_384.0;
        let cases: &[(&str, f64, f64, i32, f64)] = &[
            ("f0 boundary +1", 0.00, 0.00, 1, 0.25),
            ("f0 first-half +1", 0.06, 0.00, 1, 0.25),
            ("f0 midpoint +1", 0.125, 0.00, 1, 0.25),
            ("f0 second-half +1", 0.18, 0.25, 1, 0.50),
            ("f0 second-half -1", 0.18, 0.25, -1, 0.00),
            ("f0 1-tick-before-f1 +1", 0.25 - tick, 0.25, 1, 0.50),
            ("f0 1-tick-before-f1 -1", 0.25 - tick, 0.25, -1, 0.00),
            ("f1 boundary +1", 0.25, 0.25, 1, 0.50),
            ("f1 first-half +1", 0.31, 0.25, 1, 0.50),
            ("f1 first-half -1", 0.31, 0.25, -1, 0.00),
            ("f1 second-half +1", 0.43, 0.50, 1, 0.75),
            ("f1 second-half -1", 0.43, 0.50, -1, 0.25),
            ("f1 1-tick-before-f2 +1", 0.50 - tick, 0.50, 1, 0.75),
            ("f1 1-tick-before-f2 -1", 0.50 - tick, 0.50, -1, 0.25),
            ("f2 first-half +1", 0.56, 0.50, 1, 0.75),
            ("f2 first-half -1", 0.56, 0.50, -1, 0.25),
            ("f2 second-half -1", 0.68, 0.75, -1, 0.50),
            ("f3 boundary -1", 0.75, 0.75, -1, 0.50),
            ("f3 first-half -1", 0.81, 0.75, -1, 0.50),
            ("f3 second-half -1", 0.93, 0.75, -1, 0.50),
            ("f3 1-tick-before-end -1", 1.00 - tick, 0.75, -1, 0.50),
        ];

        for &(label, seek_t, from_pts, delta, want_pts) in cases {
            let other = if from_pts < 0.5 { 0.75 } else { 0.0 };
            exact_seek_and_settle(&v, &wake, seek_t, from_pts, other);
            let before_pushes = pushes.load(Ordering::SeqCst);
            v.step(delta);
            pull(&v, &wake);
            wait_until(label, &v, &wake, Duration::from_secs(10), || !v.is_seeking());
            let got = v.presented_time().expect("presented_time after step");
            let pushed = pushes.load(Ordering::SeqCst) - before_pushes;
            assert!(
                (got - want_pts).abs() < 1e-3 && pushed == 1,
                "{label}: seek={seek_t} from={from_pts} step({delta:+}) -> got={got} want={want_pts} pushes={pushed}"
            );
        }

        // First and last frame clamps complete without seeking or pushing a frame.
        exact_seek_and_settle(&v, &wake, 0.06, 0.0, 0.75);
        let p0 = pushes.load(Ordering::SeqCst);
        v.step(-1);
        pull(&v, &wake);
        assert!(!v.is_seeking(), "step(-1) at frame 0 must not seek");
        assert_eq!(pushes.load(Ordering::SeqCst), p0);

        exact_seek_and_settle(&v, &wake, 0.93, 0.75, 0.0);
        let p3 = pushes.load(Ordering::SeqCst);
        v.step(1);
        pull(&v, &wake);
        assert!(!v.is_seeking(), "step(+1) at last frame must not seek");
        assert_eq!(pushes.load(Ordering::SeqCst), p3);

        // Consecutive steps forward and backward across all 4 frames.
        exact_seek_and_settle(&v, &wake, 0.0, 0.0, 0.75);
        for (delta, want) in [(1, 0.25), (1, 0.50), (1, 0.75), (-1, 0.50), (-1, 0.25), (-1, 0.00)] {
            let before = pushes.load(Ordering::SeqCst);
            v.step(delta);
            pull(&v, &wake);
            wait_until("step-after-step", &v, &wake, Duration::from_secs(10), || !v.is_seeking());
            let got = v.presented_time().expect("presented_time after step-after-step");
            let pushed = pushes.load(Ordering::SeqCst) - before;
            assert!(
                (got - want).abs() < 1e-3 && pushed == 1,
                "step-after-step({delta:+}): got={got} want={want} pushes={pushed}"
            );
        }

        // A seek arriving while a step is in flight overrides it cleanly.
        exact_seek_and_settle(&v, &wake, 0.0, 0.0, 0.75);
        v.step(1);
        pull(&v, &wake);
        v.seek(0.75);
        pull(&v, &wake);
        wait_until("seek overrides in-flight step", &v, &wake, Duration::from_secs(10), || {
            !v.is_seeking() && v.presented_time().is_some_and(|p| (p - 0.75).abs() < 1e-3)
        });
    }
}
