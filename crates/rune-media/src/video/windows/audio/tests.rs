use super::*;

const SOUND: Sound = Sound { channels: 1, rate: 1000 };

fn samples(len: usize, start: usize) -> Vec<f32> {
    (start..start + len).map(|i| i as f32).collect()
}

#[test]
fn muted_drain_drops_only_samples_behind_playback_and_keeps_future_audio_for_unmute() {
    let queue = Queue::new(SOUND);
    // Sound was buffered 1 s ahead (10.0..11.0) while unmuted, then muted at 10.0.
    queue.push_pcm(10.0, samples(500, 10_000), 0.0);
    queue.push_pcm(10.5, samples(500, 10_500), 0.0);
    assert!(queue.full());

    // Playback moves to 10.3 while muted (target 10.2): only chunks ending <= 10.2 drop.
    queue.discard_before(10.2);
    assert_eq!(queue.feed.lock().unwrap().chunks.len(), 2);

    // Playback moves past 10.5 (target 10.5): the first chunk drops, 10.5..11.0 stays ready.
    queue.discard_before(10.5);
    assert_eq!(queue.feed.lock().unwrap().chunks.len(), 1);
    assert_eq!(queue.feed.lock().unwrap().samples, 500);

    // Unmuting at 10.6 plays the already-decoded samples immediately without a gap.
    let mut out = [0.0; 10];
    fill(&mut queue.feed.lock().unwrap(), &mut out, 1, 1000.0, 10.6);
    assert_eq!(out[0], 10_600.0);
    assert_eq!(out[9], 10_609.0);

    // Staying muted past 11.0 empties the queue completely.
    queue.discard_before(11.0);
    assert_eq!(queue.feed.lock().unwrap().samples, 0);
    assert!(queue.feed.lock().unwrap().chunks.is_empty());
}

#[test]
fn push_drops_samples_ending_at_or_before_unmute_skip_before() {
    let queue = Queue::new(SOUND);
    // Sample 9.90..9.95 is behind the unmute time 10.0 and dropped.
    queue.push_pcm(9.90, samples(50, 9900), 10.0);
    assert_eq!(queue.feed.lock().unwrap().samples, 0);
    // Sample 9.98..10.03 straddles 10.0 and is kept so 10.00..10.03 plays cleanly.
    let end = queue.push_pcm(9.98, samples(50, 9980), 10.0).unwrap();
    assert!((end - 10.03).abs() < 1e-9, "{end}");
    assert_eq!(queue.feed.lock().unwrap().samples, 50);
    let mut out = [0.0; 10];
    assert_eq!(fill(&mut queue.feed.lock().unwrap(), &mut out, 1, 1000.0, 10.0), None);
    assert_eq!(out[0], 10_000.0);
}

#[test]
fn fill_continues_without_skipping_within_slop_and_jumps_past_slop() {
    let queue = Queue::new(SOUND);
    queue.push_pcm(0.0, samples(1000, 0), 0.0);
    let mut feed = queue.feed.lock().unwrap();
    let mut out = [0.0; 10];
    // First write starts at want and reports None.
    assert_eq!(fill(&mut feed, &mut out, 1, 1000.0, 0.1), None);
    assert_eq!(out[0], 100.0);
    assert_eq!(out[9], 109.0);
    // Small offset within SLOP (5 ms): continues at 0.110 without skipping, reports cursor - want.
    let off = fill(&mut feed, &mut out, 1, 1000.0, 0.105).unwrap();
    assert!((off - 0.005).abs() < 1e-9, "{off}");
    assert_eq!(out[0], 110.0);
    // Large offset past SLOP (seek/stall): jumps to want and reports None.
    assert_eq!(fill(&mut feed, &mut out, 1, 1000.0, 0.5), None);
    assert_eq!(out[0], 500.0);
}

#[test]
fn clock_nudge_updates_matching_anchor_and_preserves_ui_edits() {
    let clock = Clock::default();
    let anchor = Anchor { at: rune_core::time::now(), pos: 3.0, rate: 1.0 };
    clock.set(Some(anchor));
    clock.nudge(anchor, 0.005);
    assert!((clock.get().unwrap().pos - 3.005).abs() < 1e-9);

    // UI replaced the clock (seek/pause) before the nudge landed: unchanged.
    let replaced = Anchor { pos: 9.0, ..anchor };
    clock.set(Some(replaced));
    clock.nudge(anchor, 0.005);
    assert_eq!(clock.get(), Some(replaced));

    clock.set(None);
    clock.nudge(anchor, 0.005);
    assert_eq!(clock.get(), None);
}

#[test]
fn heard_wait_uses_audio_clock_with_qpc_extrapolation_and_falls_back_when_invalid() {
    let latency = Duration::from_millis(10);
    let rate = 48_000.0;
    let padding = 3_840; // 80 ms -> buffered = 90 ms (with bias = 0).
    let written = 48_000; // 1.000 s written since Start().

    // Stopped / no device clock reading: falls back to latency + padding / rate + bias.
    let stopped = heard_wait(latency, padding, rate, 0.015, written, None, None);
    assert!((stopped - 0.105).abs() < 1e-9, "{stopped}");

    // Running with IAudioClock: played = 0.905 s, QPC age = 2 ms -> wait = 1.0 - 0.905 - 0.002 = 0.093 s.
    let qpc_freq = 10_000_000i64;
    let qpc_100ns = 100_000_000u64; // 10.000 s in 100 ns units
    let qpc_now = 100_020_000i64;   // 10.002 s at 10 MHz (age = 2 ms)
    let reading = Some((905, 1_000, qpc_100ns));
    let wait = heard_wait(latency, padding, rate, 0.0, written, reading, Some((qpc_now, qpc_freq)));
    assert!((wait - 0.093).abs() < 1e-9, "{wait}");

    // Non-10MHz QPC frequency (e.g. 3 MHz): 6_000 ticks = 2 ms past qpc_100ns = 10.000 s.
    let qpc_freq_3m = 3_000_000i64;
    let qpc_now_3m = 30_006_000i64;
    let wait_3m = heard_wait(latency, padding, rate, 0.0, written, reading, Some((qpc_now_3m, qpc_freq_3m)));
    assert!((wait_3m - 0.093).abs() < 1e-9, "{wait_3m}");

    // Negative wait (e.g. stale QPC far in the past) falls back to buffer estimate.
    let neg = heard_wait(latency, padding, rate, 0.0, written, Some((999, 1_000, qpc_100ns)), Some((qpc_now + 5_000_000, qpc_freq)));
    assert!((neg - 0.090).abs() < 1e-9, "{neg}");

    // Wait exceeding buffered + POSITION_SLACK (0.09 + 0.50 = 0.59 s) falls back to buffer estimate.
    let stuck = heard_wait(latency, padding, rate, 0.0, written, Some((0, 1_000, qpc_100ns)), Some((qpc_now, qpc_freq)));
    assert!((stuck - 0.090).abs() < 1e-9, "{stuck}");

    // Negative bias never yields a negative fallback wait.
    let clamped = heard_wait(latency, 0, rate, -0.5, 0, None, None);
    assert_eq!(clamped, 0.0);
}

#[test]
fn drift_smooths_before_nudging_and_learns_start_bias_during_settle_only() {
    let mut drift = Drift::default();
    // 2 ms steady offset never crosses NUDGE (4 ms).
    for _ in 0..50 {
        assert_eq!(drift.observe(Some(0.002), Some(Duration::from_millis(200))), None);
    }
    // A jump resets the smoothed average.
    assert_eq!(drift.observe(None, Some(Duration::from_millis(200))), None);
    assert_eq!(drift.avg, 0.0);

    // -10 ms offset during SETTLE (< 1 s): smooths up and nudges once past 4 ms, learning +bias.
    let mut nudged = None;
    for _ in 0..20 {
        if let Some(n) = drift.observe(Some(-0.010), Some(Duration::from_millis(300))) {
            nudged = Some(n);
            break;
        }
    }
    let n = nudged.expect("expected drift to cross NUDGE");
    assert!(n < -NUDGE && n > -0.010, "{n}");
    assert!((drift.bias - (-n)).abs() < 1e-9, "bias should absorb -nudge during SETTLE: {}", drift.bias);
    assert_eq!(drift.avg, 0.0);

    // After SETTLE (>= 1 s), nudges still fire for clock drift, but start bias stays frozen.
    let settled_bias = drift.bias;
    let mut later = None;
    for _ in 0..20 {
        if let Some(n) = drift.observe(Some(0.010), Some(Duration::from_secs(2))) {
            later = Some(n);
            break;
        }
    }
    assert!(later.is_some_and(|n| n > NUDGE));
    assert_eq!(drift.bias, settled_bias);
}
