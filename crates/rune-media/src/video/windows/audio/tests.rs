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
    queue.push_pcm(9.98, samples(50, 9980), 10.0);
    assert_eq!(queue.feed.lock().unwrap().samples, 50);
    let mut out = [0.0; 10];
    fill(&mut queue.feed.lock().unwrap(), &mut out, 1, 1000.0, 10.0);
    assert_eq!(out[0], 10_000.0);
}
