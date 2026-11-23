//! Submissions the GPU aborted (a Rune patch, `vendor/wgpu-hal.patch`). A GPU that recovers from a
//! hang, its own work's or another process's, ends every command buffer it was running or held with
//! an error. Each such submission is kept with how long its command buffers had executed, so a
//! host can tell work that hung (it ran until the GPU stopped it) from work that only waited
//! behind it (it never ran): [`super::Queue::aborted`].

use alloc::collections::VecDeque;
use core::time::Duration;

use objc2::{msg_send, runtime::ProtocolObject};
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus};
use parking_lot::Mutex;

/// Aborted submissions kept per queue, the oldest dropped first.
pub(super) const KEPT: usize = 256;

/// A queue's aborted submissions, by fence value.
#[derive(Debug, Default)]
pub(super) struct Aborts {
    record: Mutex<VecDeque<(crate::FenceValue, Duration)>>,
}

impl Aborts {
    /// Submission `value` ended in error after executing `executed`: kept once.
    pub(super) fn record(&self, value: crate::FenceValue, executed: Duration) {
        let mut record = self.record.lock();
        if record.iter().any(|&(v, _)| v == value) {
            return;
        }
        if record.len() == KEPT {
            record.pop_front();
        }
        record.push_back((value, executed));
    }

    /// How long submission `value` had executed when the GPU aborted it, if it did.
    pub(super) fn get(&self, value: crate::FenceValue) -> Option<Duration> {
        self.record.lock().iter().find(|&&(v, _)| v == value).map(|&(_, d)| d)
    }
}

/// Of a submission's finished command buffers, the longest execution of those that ended in error;
/// `None` when none did. A buffer the GPU discarded before running it executed for zero.
pub(super) fn aborted<'a>(
    buffers: impl IntoIterator<Item = &'a ProtocolObject<dyn MTLCommandBuffer>>,
) -> Option<Duration> {
    let mut longest = None;
    for cb in buffers {
        if cb.status() != MTLCommandBufferStatus::Error {
            continue;
        }
        // SAFETY: `MTLCommandBuffer` has both, returning `CFTimeInterval` (objc2-metal declares
        // them behind a feature this crate doesn't enable).
        let (start, end): (f64, f64) = unsafe { (msg_send![cb, GPUStartTime], msg_send![cb, GPUEndTime]) };
        let ran = if start > 0.0 && end > start { Duration::from_secs_f64(end - start) } else { Duration::ZERO };
        longest = Some(longest.map_or(ran, |l: Duration| l.max(ran)));
    }
    longest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_each_aborted_submission_once_and_keeps_the_newest() {
        let aborts = Aborts::default();
        assert_eq!(aborts.get(1), None);
        aborts.record(1, Duration::from_millis(54));
        aborts.record(1, Duration::ZERO);
        aborts.record(2, Duration::ZERO);
        assert_eq!(aborts.get(1), Some(Duration::from_millis(54)), "the first record stands");
        assert_eq!(aborts.get(2), Some(Duration::ZERO), "queued work that never ran executed for zero");
        assert_eq!(aborts.get(3), None);
        for v in 3..(3 + KEPT as u64) {
            aborts.record(v, Duration::from_millis(1));
        }
        assert_eq!(aborts.get(1), None, "the oldest go first");
        assert_eq!(aborts.get(2 + KEPT as u64), Some(Duration::from_millis(1)));
        assert_eq!(aborts.record.lock().len(), KEPT);
    }
}
