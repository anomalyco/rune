//! Env-gated Linux null-sink integration tests for `AudioOutput::open` against a real
//! container-local PulseAudio daemon (`module-null-sink`) and ALSA `null` device.
//!
//! Skipped unless `RUNE_LINUX_AUDIO_NULL_TEST=1` (PulseAudio) or `RUNE_LINUX_AUDIO_ALSA_TEST=1`
//! (ALSA fallback) is set so normal `cargo test` runs never open any OS audio daemon or produce
//! sound.

#[cfg(target_os = "linux")]
mod linux_null_sink {
    use rune_media::{
        AudioBackend, AudioEvent, AudioOutput, AudioOutputSpec, AudioOutputState, AudioPushError,
        SampleFormat,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sine_chunk(start_frame: u64, frames: usize, sample_rate: u32) -> Vec<i16> {
        let mut out = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = (start_frame + i as u64) as f64 / f64::from(sample_rate);
            let s = (2.0 * std::f64::consts::PI * 440.0 * t).sin();
            let v = (s * 16_000.0).round() as i16;
            out.push(v);
            out.push(v);
        }
        out
    }

    fn wait_until(what: &str, limit: Duration, mut pred: impl FnMut() -> bool) {
        let deadline = rune_core::time::now() + limit;
        while !pred() {
            assert!(rune_core::time::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn pulse_null_sink_10s_clock_drift_pause_resume_flush_and_drain() {
        if rune_core::env::var("RUNE_LINUX_AUDIO_NULL_TEST").as_deref() != Some("1") {
            return;
        }
        let rate = 48_000u32;
        let spec = AudioOutputSpec::new(SampleFormat::I16, rate, 2)
            .buffer_duration(Duration::from_millis(2_500))
            .app_name("rune-null-test")
            .stream_name("10s-sine")
            .volume(0.8);
        let audio = AudioOutput::open(spec).expect("open PulseAudio null sink");
        assert_eq!(audio.backend(), AudioBackend::PulseAudio);

        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        audio.set_waker(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));

        // Pre-fill 2.0 s before play.
        let mut produced = 0u64;
        while produced < 96_000 {
            let chunk = sine_chunk(produced, 960, rate);
            let n = audio.push_i16(&chunk).expect("prefill push");
            produced += n as u64;
        }

        audio.play();
        // Wait until steady playback is underway (past initial null-sink wakeup) before starting
        // the 10-second drift span.
        wait_until("steady playback start", Duration::from_secs(5), || {
            audio.heard_frames() >= u64::from(rate) / 4
        });
        let start_wall = rune_core::time::now();
        let start_heard = audio.heard_frames();
        let mut last_heard = start_heard;
        let mut max_latency = Duration::ZERO;
        let mut min_latency = Duration::MAX;
        let mut latency_samples = 0u64;
        let mut latency_sum_us = 0u128;
        let mut next_log_sec = 1u64;

        while start_wall.elapsed() < Duration::from_secs(10) {
            while audio.has_capacity(960) {
                let chunk = sine_chunk(produced, 960, rate);
                match audio.push_i16(&chunk) {
                    Ok(n) => produced += n as u64,
                    Err(AudioPushError::Full) => break,
                    Err(e) => panic!("unexpected push error during 10 s run: {e}"),
                }
            }
            let pos = audio.position();
            assert!(
                pos.heard_frames() >= last_heard,
                "heard_frames regressed: {} < {last_heard}",
                pos.heard_frames()
            );
            last_heard = pos.heard_frames();
            let lat = pos.latency();
            if !lat.is_zero() {
                max_latency = max_latency.max(lat);
                min_latency = min_latency.min(lat);
                latency_sum_us += lat.as_micros();
                latency_samples += 1;
            }
            let elapsed_s = start_wall.elapsed().as_secs();
            if elapsed_s >= next_log_sec {
                let d_frames = pos.heard_frames() - start_heard;
                let d_sec = d_frames as f64 / f64::from(rate);
                println!(
                    "  t={:.3}s heard_delta={:.3}s queued={} latency={:.1}ms events={:?}",
                    start_wall.elapsed().as_secs_f64(),
                    d_sec,
                    pos.queued_frames(),
                    lat.as_secs_f64() * 1_000.0,
                    audio.take_events(),
                );
                next_log_sec = elapsed_s + 1;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let wall_elapsed = start_wall.elapsed();
        let heard_delta_frames = audio.heard_frames() - start_heard;
        let heard_elapsed = Duration::from_nanos(
            ((u128::from(heard_delta_frames) * 1_000_000_000) / u128::from(rate)) as u64,
        );
        let drift_ms = (heard_elapsed.as_secs_f64() - wall_elapsed.as_secs_f64()) * 1_000.0;
        let avg_latency_ms = if latency_samples > 0 {
            (latency_sum_us as f64 / latency_samples as f64) / 1_000.0
        } else {
            0.0
        };
        println!(
            "PULSE_NULL_10S: wall={:.3}s heard={:.3}s drift={:+.1}ms avg_latency={:.1}ms min_latency={:.1}ms max_latency={:.1}ms",
            wall_elapsed.as_secs_f64(),
            heard_elapsed.as_secs_f64(),
            drift_ms,
            avg_latency_ms,
            if min_latency == Duration::MAX { 0.0 } else { min_latency.as_secs_f64() * 1_000.0 },
            max_latency.as_secs_f64() * 1_000.0,
        );
        assert!(
            drift_ms.abs() < 250.0,
            "10 s clock drift too large: {drift_ms:+.1} ms"
        );

        // Pause: heard_frames freezes; adjust volume while paused; resume without jump.
        audio.pause();
        assert_eq!(audio.state(), AudioOutputState::Paused);
        std::thread::sleep(Duration::from_millis(40));
        let paused_heard = audio.heard_frames();
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(audio.heard_frames(), paused_heard, "heard_frames must stay frozen while paused");
        audio.set_volume(0.4);
        audio.play();
        let resumed_start = rune_core::time::now();
        wait_until("resume advances", Duration::from_secs(2), || audio.heard_frames() > paused_heard);
        let resumed_heard = audio.heard_frames();
        let resume_jump_ms =
            ((resumed_heard - paused_heard) as f64 / f64::from(rate)) * 1_000.0;
        let resume_wall_ms = resumed_start.elapsed().as_secs_f64() * 1_000.0;
        assert!(
            resume_jump_ms <= resume_wall_ms + 120.0,
            "resume jumped ahead: heard +{resume_jump_ms:.1} ms in {resume_wall_ms:.1} ms wall"
        );

        // Flush while playing: resets heard_frames to 0, stays Playing, no spurious underrun.
        audio.flush();
        assert_eq!(audio.state(), AudioOutputState::Playing);
        assert_eq!(audio.heard_frames(), 0);
        std::thread::sleep(Duration::from_millis(40));
        assert!(
            !audio.take_events().contains(&AudioEvent::Underrun),
            "flush must not emit spurious Underrun"
        );

        // Push 150 ms and finish(): drains cleanly to Ended and wakes once.
        let wakes_before = wakes.load(Ordering::SeqCst);
        let tail = sine_chunk(0, 7_200, rate);
        assert_eq!(audio.push_i16(&tail), Ok(7_200));
        audio.finish();
        wait_until("drain to Ended", Duration::from_secs(5), || {
            audio.state() == AudioOutputState::Ended
        });
        assert_eq!(audio.heard_frames(), 7_200);
        assert!(audio.take_events().contains(&AudioEvent::Ended));
        assert!(wakes.load(Ordering::SeqCst) > wakes_before);
    }

    #[test]
    fn pulse_daemon_kill_while_playing_reports_device_lost_once_without_hang() {
        if rune_core::env::var("RUNE_LINUX_AUDIO_NULL_TEST").as_deref() != Some("1") {
            return;
        }
        let Some(pid_str) = rune_core::env::var("RUNE_LINUX_AUDIO_KILL_PID") else {
            return;
        };
        let pid: i32 = pid_str.trim().parse().expect("valid RUNE_LINUX_AUDIO_KILL_PID");
        let rate = 48_000u32;
        let spec = AudioOutputSpec::new(SampleFormat::I16, rate, 2)
            .buffer_duration(Duration::from_millis(1_000))
            .app_name("rune-kill-test");
        let audio = AudioOutput::open(spec).expect("open PulseAudio for kill test");
        assert_eq!(audio.backend(), AudioBackend::PulseAudio);

        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        audio.set_waker(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        let chunk = sine_chunk(0, 24_000, rate);
        assert_eq!(audio.push_i16(&chunk), Ok(24_000));
        audio.play();
        wait_until("playing before kill", Duration::from_secs(3), || audio.heard_frames() > 0);

        // Terminate the container-local PulseAudio daemon mid-playback (via libc::kill so
        // minimal containers without /bin/kill work).
        #[allow(unsafe_code)]
        let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
        assert_eq!(rc, 0, "libc::kill({pid}, SIGKILL) failed");

        wait_until("DeviceLost after daemon kill", Duration::from_secs(5), || {
            matches!(audio.state(), AudioOutputState::DeviceLost(_))
        });
        let events = audio.take_events();
        let lost_count = events
            .iter()
            .filter(|e| matches!(e, AudioEvent::DeviceLost(_)))
            .count();
        assert_eq!(lost_count, 1, "expected exactly one DeviceLost event, got {events:?}");
        let wakes_after = wakes.load(Ordering::SeqCst);
        assert!(wakes_after >= 1);

        // No busy loop or repeated DeviceLost wakes afterwards.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(wakes.load(Ordering::SeqCst), wakes_after);
        assert!(audio.take_events().is_empty());
        assert_eq!(audio.push_i16(&[0i16; 96]), Err(AudioPushError::DeviceLost));
        println!("PULSE_KILL_TEST: DeviceLost reported once cleanly ({:?})", audio.state());
    }

    #[test]
    fn alsa_null_device_fallback_plays_pauses_flushes_and_drains() {
        if rune_core::env::var("RUNE_LINUX_AUDIO_ALSA_TEST").as_deref() != Some("1") {
            return;
        }
        let rate = 48_000u32;
        let spec = AudioOutputSpec::new(SampleFormat::I16, rate, 2)
            .buffer_duration(Duration::from_millis(1_000))
            .app_name("rune-alsa-null");
        let audio = AudioOutput::open(spec).expect("open ALSA null fallback");
        assert_eq!(audio.backend(), AudioBackend::Alsa);

        let chunk = sine_chunk(0, 9_600, rate); // 200 ms
        assert_eq!(audio.push_i16(&chunk), Ok(9_600));
        audio.play();
        wait_until("ALSA null advances", Duration::from_secs(3), || audio.heard_frames() > 0);
        audio.pause();
        assert_eq!(audio.state(), AudioOutputState::Paused);
        let frozen = audio.heard_frames();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(audio.heard_frames(), frozen);

        audio.flush();
        assert_eq!(audio.heard_frames(), 0);
        assert_eq!(audio.push_i16(&chunk), Ok(9_600));
        audio.play();
        audio.finish();
        wait_until("ALSA null drains to Ended", Duration::from_secs(5), || {
            audio.state() == AudioOutputState::Ended
        });
        assert_eq!(audio.heard_frames(), 9_600);
        assert!(audio.take_events().contains(&AudioEvent::Ended));
        println!("ALSA_NULL_TEST: backend={:?} heard_frames={}", audio.backend(), audio.heard_frames());
    }

    #[test]
    fn missing_libraries_returns_open_failed_and_open_silent_still_works() {
        if rune_core::env::var("RUNE_LINUX_AUDIO_MISSING_LIBS_TEST").as_deref() != Some("1") {
            return;
        }
        let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2);
        let err = AudioOutput::open(spec.clone()).unwrap_err();
        let rune_media::AudioOutputError::OpenFailed(msg) = err else {
            panic!("expected AudioOutputError::OpenFailed, got {err:?}");
        };
        assert!(msg.contains("PulseAudio") && msg.contains("ALSA"), "unexpected error: {msg}");

        // open_silent still works with zero audio libraries installed.
        let silent = AudioOutput::open_silent(spec).expect("open_silent must succeed");
        assert_eq!(silent.backend(), AudioBackend::Silent);
        assert_eq!(silent.push_i16(&[0i16; 960]), Ok(480));
        silent.play();
        silent.finish();
        wait_until("silent drains to Ended", Duration::from_secs(2), || {
            silent.state() == AudioOutputState::Ended
        });
        assert_eq!(silent.heard_frames(), 480);
        println!("MISSING_LIBS_TEST: OpenFailed({msg}) and open_silent drained 480 frames");
    }
}
