//! Pure `wp_presentation` clock-conversion math and bounded in-flight feedback tracking.

use std::collections::VecDeque;
use std::time::Duration;

use crate::frame::presented::MAX_PENDING;
use crate::time::Instant;

/// Sanity bound on compositor-clock-to-now delta to reject corrupted timestamps.
const MAX_CLOCK_SKEW: Duration = Duration::from_secs(60);

const NANOS_PER_SEC: u32 = 1_000_000_000;

/// Converts a Wayland `wp_presentation_feedback.presented` `(tv_sec_hi, tv_sec_lo, tv_nsec)`
/// timestamp in the compositor's `clock_id` domain onto a Rust [`Instant`] using a paired
/// `(now, clock_now_sec, clock_now_nsec)` reading from `clock_gettime(clock_id)`.
///
/// Rejects invalid nanosecond fields (`>= 1_000_000_000`) and deltas exceeding [`MAX_CLOCK_SKEW`].
pub(crate) fn timespec_to_instant(
    now: Instant,
    clock_now_sec: i64,
    clock_now_nsec: i64,
    tv_sec_hi: u32,
    tv_sec_lo: u32,
    tv_nsec: u32,
) -> Option<Instant> {
    if tv_nsec >= NANOS_PER_SEC
        || clock_now_nsec < 0
        || clock_now_nsec >= i64::from(NANOS_PER_SEC)
    {
        return None;
    }
    let tv_sec = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
    let sec_delta = i128::from(tv_sec) - i128::from(clock_now_sec);
    let nsec_delta = i128::from(tv_nsec) - i128::from(clock_now_nsec);
    let total_ns = sec_delta.checked_mul(i128::from(NANOS_PER_SEC))?.checked_add(nsec_delta)?;
    if total_ns.unsigned_abs() > MAX_CLOCK_SKEW.as_nanos() {
        return None;
    }
    let delta = Duration::from_nanos(total_ns.unsigned_abs() as u64);
    if total_ns >= 0 {
        now.checked_add(delta)
    } else {
        now.checked_sub(delta)
    }
}

/// Bounded per-surface feedback tracker for `wp_presentation`.
#[derive(Default)]
pub(crate) struct SurfacePresentationState {
    next_seq: u64,
    in_flight: VecDeque<u64>,
    completed: VecDeque<(u64, Option<Instant>)>,
}

impl SurfacePresentationState {
    /// Allocates the next monotonic feedback sequence number when `clock_id` is known.
    pub(crate) fn request_feedback(&mut self, has_clock_id: bool) -> Option<u64> {
        if !has_clock_id {
            return None;
        }
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        let seq = self.next_seq;
        if self.in_flight.len() >= MAX_PENDING {
            self.in_flight.pop_front();
        }
        self.in_flight.push_back(seq);
        Some(seq)
    }

    /// Records a compositor `presented` event for `seq`.
    pub(crate) fn on_presented(&mut self, seq: u64, at: Option<Instant>) {
        let Some(pos) = self.in_flight.iter().position(|&s| s == seq) else {
            return;
        };
        self.in_flight.remove(pos);
        if self.completed.len() >= MAX_PENDING {
            self.completed.pop_front();
        }
        self.completed.push_back((seq, at));
    }

    /// Records a compositor `discarded` event for `seq`.
    pub(crate) fn on_discarded(&mut self, seq: u64) {
        self.on_presented(seq, None);
    }

    /// Drains ready `(feedback_seq, presented_at)` pairs into `out`.
    pub(crate) fn drain(&mut self, out: &mut Vec<(u64, Option<Instant>)>) {
        out.extend(self.completed.drain(..));
    }

    /// Whether any feedback objects are still awaiting `presented` or `discarded`.
    pub(crate) fn needs_wake(&self) -> bool {
        !self.in_flight.is_empty()
    }

    /// Drops in-flight and undrained entries after a surface replacement, hide, or device loss.
    /// `next_seq` keeps advancing so late events for old feedback objects are ignored.
    pub(crate) fn reset(&mut self) {
        self.in_flight.clear();
        self.completed.clear();
    }

    #[cfg(test)]
    pub(crate) fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }
}
