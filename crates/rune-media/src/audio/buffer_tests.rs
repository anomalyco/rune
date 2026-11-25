use super::buffer::*;
use super::{AudioPushError, SampleFormat};
use std::time::Duration;

#[test]
fn capacity_bounds_clamp_duration_and_enforce_16_mib_cap() {
    // 48 kHz stereo I16 (4 B/frame): default 2.5 s -> 120_000 frames.
    assert_eq!(bounded_capacity_frames(48_000, 4, DEFAULT_BUFFER_DURATION), 120_000);
    // Below 100 ms clamps to 100 ms (4_800 frames).
    assert_eq!(bounded_capacity_frames(48_000, 4, Duration::from_millis(5)), 4_800);
    // Above 10 s clamps to 10 s (480_000 frames = 1.92 MiB < 16 MiB).
    assert_eq!(bounded_capacity_frames(48_000, 4, Duration::from_secs(60)), 480_000);
    // 192 kHz 8ch F32 (32 B/frame): 10 s would be 61.4 MiB -> capped at 16 MiB / 32 = 524_288 frames.
    let capped = bounded_capacity_frames(192_000, 32, Duration::from_secs(10));
    assert_eq!(capped, MAX_BUFFER_BYTES / 32);
    assert!(capped * 32 <= MAX_BUFFER_BYTES);
}

#[test]
fn ring_wrap_around_preserves_exact_fifo_order() {
    let mut buf = PcmBuffer::new(SampleFormat::I16, 48_000, 2, 16);
    let mut out = vec![0u8; 16 * 4];
    let mut next_push = 1i16;
    let mut next_expect = 1i16;

    // Repeatedly push 7 frames and read 5 frames across many full wrap-arounds.
    for _ in 0..40 {
        let mut chunk = Vec::with_capacity(7 * 2);
        for _ in 0..7 {
            chunk.push(next_push);
            chunk.push(-next_push);
            next_push = next_push.wrapping_add(1);
        }
        let pushed = buf.push_i16(&chunk).unwrap();
        assert!(pushed > 0);
        // Rewind next_push for any unaccepted tail.
        next_push = next_push.wrapping_sub((7 - pushed) as i16);

        let read = buf.peek_scaled(5, 1.0, false, &mut out);
        assert_eq!(read, 5);
        buf.commit_read(read);
        for f in 0..read {
            let l = i16::from_le_bytes([out[f * 4], out[f * 4 + 1]]);
            let r = i16::from_le_bytes([out[f * 4 + 2], out[f * 4 + 3]]);
            assert_eq!((l, r), (next_expect, -next_expect));
            next_expect = next_expect.wrapping_add(1);
        }
    }
}

#[test]
fn push_reports_full_partial_unaligned_and_format_mismatch() {
    let mut i16_buf = PcmBuffer::new(SampleFormat::I16, 48_000, 2, 4);
    assert_eq!(i16_buf.push_f32(&[0.0, 0.0]), Err(AudioPushError::FormatMismatch));
    assert_eq!(i16_buf.push_bytes(&[0u8; 3]), Err(AudioPushError::Unaligned));
    assert_eq!(i16_buf.push_i16(&[1, 2, 3]), Err(AudioPushError::Unaligned));
    assert_eq!(i16_buf.push_bytes(&[]), Ok(0));

    // Push 3 of 4 frames, then push 3 more -> partial accept of 1 frame, then Full.
    assert_eq!(i16_buf.push_i16(&[10, 20, 30, 40, 50, 60]), Ok(3));
    assert_eq!(i16_buf.free_frames(), 1);
    assert_eq!(i16_buf.free_bytes(), 4);
    assert_eq!(i16_buf.push_i16(&[70, 80, 90, 100, 110, 120]), Ok(1));
    assert_eq!(i16_buf.free_frames(), 0);
    assert_eq!(i16_buf.push_i16(&[1, 2]), Err(AudioPushError::Full));
    assert_eq!(i16_buf.push_bytes(&[0u8; 4]), Err(AudioPushError::Full));

    let mut f32_buf = PcmBuffer::new(SampleFormat::F32, 48_000, 2, 4);
    assert_eq!(f32_buf.push_i16(&[0, 0]), Err(AudioPushError::FormatMismatch));
    assert_eq!(f32_buf.push_f32(&[0.5]), Err(AudioPushError::Unaligned));
    assert_eq!(f32_buf.push_f32(&[0.25, -0.25, 0.5, -0.5]), Ok(2));
}

#[test]
fn tail_rewind_replays_exact_unheard_frames_ahead_of_ring_with_new_gain() {
    let mut buf = PcmBuffer::new(SampleFormat::I16, 48_000, 1, 32);
    let samples: Vec<i16> = (1..=10).map(|i| i * 1_000).collect();
    assert_eq!(buf.push_i16(&samples), Ok(10));

    // Read 6 frames (1000..=6000) at full volume.
    let mut out = vec![0u8; 10 * 2];
    let n = buf.peek_scaled(6, 1.0, false, &mut out);
    assert_eq!(n, 6);
    buf.commit_read(n);
    assert_eq!(buf.tail_frames(), 6);
    assert_eq!(buf.queued_frames(), 4);

    // Pause: 4 frames (3000, 4000, 5000, 6000) were unheard in device -> rewind them.
    assert_eq!(buf.rewind_tail(4), 4);
    assert_eq!(buf.queued_frames(), 8);
    assert_eq!(buf.tail_frames(), 0);

    // Resume at volume 0.5: must deliver 3000..=10000 scaled to 1500..=5000 in exact order.
    let n2 = buf.peek_scaled(8, 0.5, false, &mut out);
    assert_eq!(n2, 8);
    buf.commit_read(n2);
    let got: Vec<i16> = out[..16]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| i16::from_le_bytes(c))
        .collect();
    assert_eq!(got, vec![1_500, 2_000, 2_500, 3_000, 3_500, 4_000, 4_500, 5_000]);
    assert!(buf.is_empty());
}

#[test]
fn gain_and_mute_are_exact_at_full_scale_zero_and_fractional_for_both_formats() {
    // I16 full scale is bit-exact including i16::MIN (-32768) and i16::MAX (32767).
    let src_i16 = [i16::MIN, -1_000, 0, 1_000, i16::MAX];
    let mut bytes = Vec::new();
    for s in src_i16 {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let mut full = bytes.clone();
    apply_gain_in_place(SampleFormat::I16, &mut full, 1.0, false);
    assert_eq!(full, bytes);

    // I16 half gain.
    let mut half = bytes.clone();
    apply_gain_in_place(SampleFormat::I16, &mut half, 0.5, false);
    let half_i16: Vec<i16> = half.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect();
    assert_eq!(half_i16, vec![-16_384, -500, 0, 500, 16_384]);

    // I16 mute and volume 0.0 -> all zero bytes.
    let mut muted = bytes.clone();
    apply_gain_in_place(SampleFormat::I16, &mut muted, 1.0, true);
    assert!(muted.iter().all(|&b| b == 0));
    let mut zero_vol = bytes;
    apply_gain_in_place(SampleFormat::I16, &mut zero_vol, 0.0, false);
    assert!(zero_vol.iter().all(|&b| b == 0));

    // F32 full scale is bit-exact.
    let src_f32 = [-1.0f32, -0.25, 0.0, 0.5, 1.0];
    let mut fbytes = Vec::new();
    for s in src_f32 {
        fbytes.extend_from_slice(&s.to_le_bytes());
    }
    let mut ffull = fbytes.clone();
    apply_gain_in_place(SampleFormat::F32, &mut ffull, 1.0, false);
    assert_eq!(ffull, fbytes);

    // F32 half gain.
    let mut fhalf = fbytes.clone();
    apply_gain_in_place(SampleFormat::F32, &mut fhalf, 0.5, false);
    let half_f32: Vec<f32> = fhalf.as_chunks::<4>().0.iter().map(|&c| f32::from_le_bytes(c)).collect();
    assert_eq!(half_f32, vec![-0.5, -0.125, 0.0, 0.25, 0.5]);

    // F32 mute -> exact 0.0.
    let mut fmuted = fbytes;
    apply_gain_in_place(SampleFormat::F32, &mut fmuted, 1.0, true);
    assert!(fmuted.iter().all(|&b| b == 0));

    // Non-finite F32 input samples clamp to 0.0 when scaled.
    let mut nan_bytes = f32::NAN.to_le_bytes().to_vec();
    apply_gain_in_place(SampleFormat::F32, &mut nan_bytes, 0.5, false);
    assert_eq!(f32::from_le_bytes(nan_bytes.try_into().unwrap()), 0.0);
}
