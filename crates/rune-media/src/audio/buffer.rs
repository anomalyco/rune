//! Bounded pre-gain PCM ring buffer, rolling written-tail buffer for pause rewind, rewound front
//! queue, and post-buffer linear volume/mute scaling for `I16` and `F32` interleaved audio.
//!
//! All storage is allocated once at construction so steady-state push and read perform zero
//! heap allocations.

use super::{AudioPushError, SampleFormat};
use std::time::Duration;

/// Default queued PCM duration (2.5 s) when the caller does not override `buffer_duration`.
pub(crate) const DEFAULT_BUFFER_DURATION: Duration = Duration::from_millis(2_500);
/// Minimum buffer duration (100 ms).
pub(crate) const MIN_BUFFER_DURATION: Duration = Duration::from_millis(100);
/// Maximum buffer duration (10 s).
pub(crate) const MAX_BUFFER_DURATION: Duration = Duration::from_secs(10);
/// Hard upper bound on queued PCM bytes (16 MiB).
pub(crate) const MAX_BUFFER_BYTES: usize = 16 * 1024 * 1024;
/// Rolling written-tail retention (~500 ms) used to rewind unheard device frames on pause.
const TAIL_DURATION: Duration = Duration::from_millis(500);
/// Hard cap on the rolling tail and rewound front buffers (2 MiB each).
const MAX_TAIL_BYTES: usize = 2 * 1024 * 1024;

/// Computes the bounded ring capacity in whole frames for `sample_rate`, `frame_bytes`, and
/// requested `duration`.
pub(crate) fn bounded_capacity_frames(sample_rate: u32, frame_bytes: usize, duration: Duration) -> usize {
    let clamped = duration.clamp(MIN_BUFFER_DURATION, MAX_BUFFER_DURATION);
    let requested_frames = ((clamped.as_nanos() * u128::from(sample_rate)) / 1_000_000_000) as usize;
    let max_frames_by_bytes = (MAX_BUFFER_BYTES / frame_bytes.max(1)).max(1);
    requested_frames.clamp(1, max_frames_by_bytes)
}

/// Computes the ~10 ms write quantum in frames so volume and mute edits take effect promptly.
pub(crate) fn quantum_frames(sample_rate: u32) -> usize {
    ((sample_rate as usize) / 100).clamp(64, 2_048)
}

/// Fixed-capacity circular byte queue (zero allocations after creation).
struct ByteRing {
    data: Box<[u8]>,
    head: usize,
    len: usize,
}

impl ByteRing {
    fn new(capacity_bytes: usize) -> Self {
        Self {
            data: vec![0u8; capacity_bytes].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.data.len()
    }

    fn len(&self) -> usize {
        self.len
    }

    fn free(&self) -> usize {
        self.data.len() - self.len
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Appends `src` to the back of the ring (caller guarantees `src.len() <= self.free()`).
    fn push_back(&mut self, src: &[u8]) {
        let n = src.len();
        if n == 0 {
            return;
        }
        debug_assert!(n <= self.free());
        let cap = self.data.len();
        let tail = (self.head + self.len) % cap;
        let first = n.min(cap - tail);
        self.data[tail..tail + first].copy_from_slice(&src[..first]);
        if first < n {
            self.data[..n - first].copy_from_slice(&src[first..]);
        }
        self.len += n;
    }

    /// Appends `src` to the back of a rolling ring, overwriting the oldest bytes when full.
    fn push_rolling(&mut self, src: &[u8]) {
        let cap = self.data.len();
        if cap == 0 || src.is_empty() {
            return;
        }
        if src.len() >= cap {
            let start = src.len() - cap;
            self.data.copy_from_slice(&src[start..]);
            self.head = 0;
            self.len = cap;
            return;
        }
        let overflow = (self.len + src.len()).saturating_sub(cap);
        if overflow > 0 {
            self.head = (self.head + overflow) % cap;
            self.len -= overflow;
        }
        self.push_back(src);
    }

    /// Prepends `src` ahead of existing bytes (used when rewinding unheard tail frames).
    /// Keeps the newest bytes at the front if `src.len() + self.len > cap`.
    fn push_front_clamped(&mut self, src: &[u8]) {
        let cap = self.data.len();
        if cap == 0 || src.is_empty() {
            return;
        }
        let src = if src.len() > cap { &src[src.len() - cap..] } else { src };
        let keep_old = self.len.min(cap - src.len());
        self.len = keep_old;
        let n = src.len();
        self.head = (self.head + cap - n) % cap;
        let first = n.min(cap - self.head);
        self.data[self.head..self.head + first].copy_from_slice(&src[..first]);
        if first < n {
            self.data[..n - first].copy_from_slice(&src[first..]);
        }
        self.len += n;
    }

    /// Copies up to `dst.len()` bytes from the front without consuming them.
    fn peek_front(&self, dst: &mut [u8]) -> usize {
        let n = dst.len().min(self.len);
        if n == 0 {
            return 0;
        }
        let cap = self.data.len();
        let first = n.min(cap - self.head);
        dst[..first].copy_from_slice(&self.data[self.head..self.head + first]);
        if first < n {
            dst[first..n].copy_from_slice(&self.data[..n - first]);
        }
        n
    }

    /// Copies the last `n` bytes of the ring into `dst[..n]` (caller guarantees `n <= self.len`).
    fn copy_suffix(&self, n: usize, dst: &mut [u8]) {
        if n == 0 {
            return;
        }
        debug_assert!(n <= self.len && n <= dst.len());
        let cap = self.data.len();
        let start = (self.head + self.len - n) % cap;
        let first = n.min(cap - start);
        dst[..first].copy_from_slice(&self.data[start..start + first]);
        if first < n {
            dst[first..n].copy_from_slice(&self.data[..n - first]);
        }
    }

    /// Drops `n` bytes from the front of the ring.
    fn consume_front(&mut self, n: usize) {
        let n = n.min(self.len);
        if n == 0 {
            return;
        }
        self.head = (self.head + n) % self.data.len();
        self.len -= n;
        if self.len == 0 {
            self.head = 0;
        }
    }
}

/// Bounded PCM queue with rolling written tail, rewound front queue, and post-buffer scaling.
pub(crate) struct PcmBuffer {
    format: SampleFormat,
    channels: usize,
    frame_bytes: usize,
    capacity_frames: usize,
    ring: ByteRing,
    tail: ByteRing,
    front: ByteRing,
    /// Pre-allocated scratch for `rewind_tail` so rewinding allocates nothing.
    rewind_scratch: Box<[u8]>,
}

impl PcmBuffer {
    pub(crate) fn new(format: SampleFormat, sample_rate: u32, channels: u16, capacity_frames: usize) -> Self {
        let channels = usize::from(channels);
        let frame_bytes = format.bytes_per_sample() * channels;
        let ring_bytes = capacity_frames * frame_bytes;
        let tail_target_frames = ((TAIL_DURATION.as_nanos() * u128::from(sample_rate)) / 1_000_000_000) as usize;
        let tail_max_frames = (MAX_TAIL_BYTES / frame_bytes.max(1)).max(1);
        let tail_frames = tail_target_frames.clamp(1, capacity_frames.min(tail_max_frames));
        let tail_bytes = tail_frames * frame_bytes;
        Self {
            format,
            channels,
            frame_bytes,
            capacity_frames,
            ring: ByteRing::new(ring_bytes),
            tail: ByteRing::new(tail_bytes),
            front: ByteRing::new(tail_bytes),
            rewind_scratch: vec![0u8; tail_bytes].into_boxed_slice(),
        }
    }

    pub(crate) fn queued_frames(&self) -> usize {
        (self.front.len() + self.ring.len()) / self.frame_bytes
    }

    pub(crate) fn free_frames(&self) -> usize {
        self.capacity_frames
            .saturating_sub(self.queued_frames())
            .min(self.ring.free() / self.frame_bytes)
    }

    pub(crate) fn free_bytes(&self) -> usize {
        self.free_frames() * self.frame_bytes
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.front.len() == 0 && self.ring.len() == 0
    }

    /// Discards all queued, rewound, and tail audio.
    pub(crate) fn clear(&mut self) {
        self.ring.clear();
        self.tail.clear();
        self.front.clear();
    }

    /// Pushes interleaved PCM bytes (must be frame-aligned). Returns bytes accepted.
    pub(crate) fn push_bytes(&mut self, pcm: &[u8]) -> Result<usize, AudioPushError> {
        if !pcm.len().is_multiple_of(self.frame_bytes) {
            return Err(AudioPushError::Unaligned);
        }
        if pcm.is_empty() {
            return Ok(0);
        }
        let free = self.free_frames();
        if free == 0 {
            return Err(AudioPushError::Full);
        }
        let frames = (pcm.len() / self.frame_bytes).min(free);
        let bytes = frames * self.frame_bytes;
        self.ring.push_back(&pcm[..bytes]);
        Ok(bytes)
    }

    /// Pushes interleaved `i16` samples. Returns frames accepted.
    pub(crate) fn push_i16(&mut self, samples: &[i16]) -> Result<usize, AudioPushError> {
        if self.format != SampleFormat::I16 {
            return Err(AudioPushError::FormatMismatch);
        }
        if !samples.len().is_multiple_of(self.channels) {
            return Err(AudioPushError::Unaligned);
        }
        if samples.is_empty() {
            return Ok(0);
        }
        let free = self.free_frames();
        if free == 0 {
            return Err(AudioPushError::Full);
        }
        let frames = (samples.len() / self.channels).min(free);
        let sample_count = frames * self.channels;
        #[cfg(target_endian = "little")]
        {
            let bytes = bytemuck::cast_slice(&samples[..sample_count]);
            self.ring.push_back(bytes);
        }
        #[cfg(target_endian = "big")]
        {
            for &s in &samples[..sample_count] {
                self.ring.push_back(&s.to_le_bytes());
            }
        }
        Ok(frames)
    }

    /// Pushes interleaved `f32` samples. Returns frames accepted.
    pub(crate) fn push_f32(&mut self, samples: &[f32]) -> Result<usize, AudioPushError> {
        if self.format != SampleFormat::F32 {
            return Err(AudioPushError::FormatMismatch);
        }
        if !samples.len().is_multiple_of(self.channels) {
            return Err(AudioPushError::Unaligned);
        }
        if samples.is_empty() {
            return Ok(0);
        }
        let free = self.free_frames();
        if free == 0 {
            return Err(AudioPushError::Full);
        }
        let frames = (samples.len() / self.channels).min(free);
        let sample_count = frames * self.channels;
        #[cfg(target_endian = "little")]
        {
            let bytes = bytemuck::cast_slice(&samples[..sample_count]);
            self.ring.push_back(bytes);
        }
        #[cfg(target_endian = "big")]
        {
            for &s in &samples[..sample_count] {
                self.ring.push_back(&s.to_le_bytes());
            }
        }
        Ok(frames)
    }

    /// Copies up to `max_frames` from `front` then `ring` into `out`, applying post-buffer
    /// linear `volume` and `muted` in place without consuming the bytes yet. Returns the number
    /// of frames written into `out`. Pair with [`commit_read`](Self::commit_read) once the
    /// device accepts `accepted <= frames`.
    pub(crate) fn peek_scaled(
        &self,
        max_frames: usize,
        volume: f32,
        muted: bool,
        out: &mut [u8],
    ) -> usize {
        let cap_frames = (out.len() / self.frame_bytes).min(max_frames);
        let avail_frames = self.queued_frames().min(cap_frames);
        if avail_frames == 0 {
            return 0;
        }
        let want_bytes = avail_frames * self.frame_bytes;
        let from_front = self.front.peek_front(&mut out[..want_bytes]);
        if from_front < want_bytes {
            let from_ring = self.ring.peek_front(&mut out[from_front..want_bytes]);
            debug_assert_eq!(from_front + from_ring, want_bytes);
        }
        apply_gain_in_place(self.format, &mut out[..want_bytes], volume, muted);
        avail_frames
    }

    /// Commits `accepted_frames` previously peeked via [`peek_scaled`](Self::peek_scaled): moves
    /// their raw pre-gain bytes from `front`/`ring` into the rolling `tail` buffer.
    pub(crate) fn commit_read(&mut self, accepted_frames: usize) {
        let mut remaining = (accepted_frames * self.frame_bytes).min(self.front.len() + self.ring.len());
        while remaining > 0 {
            let chunk = remaining.min(self.rewind_scratch.len().max(1));
            if self.front.len() > 0 {
                let n = self.front.peek_front(&mut self.rewind_scratch[..chunk]);
                self.front.consume_front(n);
                self.tail.push_rolling(&self.rewind_scratch[..n]);
                remaining -= n;
            } else {
                let n = self.ring.peek_front(&mut self.rewind_scratch[..chunk]);
                self.ring.consume_front(n);
                self.tail.push_rolling(&self.rewind_scratch[..n]);
                remaining -= n;
            }
        }
    }

    /// Rewinds the last `unheard_frames` written to the device from the rolling `tail` buffer
    /// onto the front of the unread queue so resume replays them with current volume/mute.
    /// Returns the actual number of frames rewound (capped by the rolling tail's retention).
    pub(crate) fn rewind_tail(&mut self, unheard_frames: u64) -> usize {
        let avail_frames = self.tail.len() / self.frame_bytes;
        let max_front = self.front.capacity() / self.frame_bytes;
        let count = (unheard_frames.min(usize::MAX as u64) as usize)
            .min(avail_frames)
            .min(max_front);
        if count == 0 {
            self.tail.clear();
            return 0;
        }
        let bytes = count * self.frame_bytes;
        self.tail.copy_suffix(bytes, &mut self.rewind_scratch[..bytes]);
        self.front.push_front_clamped(&self.rewind_scratch[..bytes]);
        self.tail.clear();
        count
    }

    #[cfg(test)]
    pub(crate) fn tail_frames(&self) -> usize {
        self.tail.len() / self.frame_bytes
    }
}

/// Applies post-buffer linear `volume` (clamped `0.0..=1.0`) and `muted` directly in place.
/// Full-scale (`!muted && volume == 1.0`) leaves bytes untouched bit-for-bit.
pub(crate) fn apply_gain_in_place(format: SampleFormat, pcm: &mut [u8], volume: f32, muted: bool) {
    let gain = if muted || !volume.is_finite() { 0.0 } else { volume.clamp(0.0, 1.0) };
    if gain == 0.0 {
        pcm.fill(0);
        return;
    }
    if gain == 1.0 {
        return;
    }
    match format {
        SampleFormat::I16 => {
            for chunk in pcm.as_chunks_mut::<2>().0 {
                let sample = i16::from_le_bytes(*chunk);
                let scaled = (f32::from(sample) * gain)
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
                *chunk = scaled.to_le_bytes();
            }
        }
        SampleFormat::F32 => {
            for chunk in pcm.as_chunks_mut::<4>().0 {
                let sample = f32::from_le_bytes(*chunk);
                let scaled = if sample.is_finite() { sample * gain } else { 0.0 };
                *chunk = scaled.to_le_bytes();
            }
        }
    }
}
