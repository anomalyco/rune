//! Silent clock-paced [`SinkDevice`](super::SinkDevice) implementation. Works on every platform
//! (including `wasm32`) without opening any OS audio device or producing sound.

use super::{AudioBackend, ClockFn, SinkDevice, SinkError, ValidatedSpec};
use rune_core::time::Instant;
use std::time::Duration;

/// Simulated device buffer target (~40 ms), matching a low-latency OS audio stream so
/// `AudioPosition::latency`, pause-rewind, and drain behave like a real backend.
const VIRTUAL_BUFFER_MS: u64 = 40;
/// Maximum sleep between silent-sink writes while playing.
const MAX_PACE: Duration = Duration::from_millis(10);

pub(crate) struct SilentDevice {
    sample_rate: u32,
    frame_bytes: usize,
    cap_frames: u64,
    buffered_frames: u64,
    frac_nanos: u128,
    last_at: Option<Instant>,
    clock: ClockFn,
}

impl SilentDevice {
    pub(crate) fn new(spec: &ValidatedSpec, clock: ClockFn) -> Self {
        let cap_frames = ((u64::from(spec.sample_rate) * VIRTUAL_BUFFER_MS) / 1_000).max(64);
        Self {
            sample_rate: spec.sample_rate,
            frame_bytes: spec.frame_bytes,
            cap_frames,
            buffered_frames: 0,
            frac_nanos: 0,
            last_at: None,
            clock,
        }
    }

    fn advance(&mut self) {
        let now = (self.clock)();
        let Some(prev) = self.last_at else {
            self.last_at = Some(now);
            return;
        };
        if self.buffered_frames == 0 {
            self.last_at = Some(now);
            self.frac_nanos = 0;
            return;
        }
        let dt_nanos = now.saturating_duration_since(prev).as_nanos();
        let total = dt_nanos.saturating_mul(u128::from(self.sample_rate)) + self.frac_nanos;
        let consumed = (total / 1_000_000_000).min(u128::from(u64::MAX)) as u64;
        self.frac_nanos = total % 1_000_000_000;
        self.buffered_frames = self.buffered_frames.saturating_sub(consumed);
        if self.buffered_frames == 0 {
            self.frac_nanos = 0;
        }
        self.last_at = Some(now);
    }
}

impl SinkDevice for SilentDevice {
    fn backend(&self) -> AudioBackend {
        AudioBackend::Silent
    }

    fn write(&mut self, pcm: &[u8]) -> Result<usize, SinkError> {
        self.advance();
        let frames = (pcm.len() / self.frame_bytes) as u64;
        let free = self.cap_frames.saturating_sub(self.buffered_frames);
        let accepted = frames.min(free);
        self.buffered_frames = self.buffered_frames.saturating_add(accepted);
        Ok(accepted as usize)
    }

    fn latency_frames(&mut self) -> Result<u64, SinkError> {
        self.advance();
        Ok(self.buffered_frames)
    }

    fn flush(&mut self) -> Result<(), SinkError> {
        self.buffered_frames = 0;
        self.frac_nanos = 0;
        self.last_at = None;
        Ok(())
    }

    fn pace(&self) -> Duration {
        if self.buffered_frames < self.cap_frames {
            Duration::ZERO
        } else {
            MAX_PACE
        }
    }
}
