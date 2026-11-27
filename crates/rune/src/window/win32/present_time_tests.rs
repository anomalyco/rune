//! Windows DXGI presentation timestamp correlation, QPC mapping, wraparound, superseded
//! frames, unsupported fallback, and idle-clean settle expiry.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use super::*;
use crate::frame::encoded::{FrameReports, OnFrameEncoded};
use crate::frame::presented::{FramePresentReports, FramePresented, OnFramePresented};
use crate::FrameEncoded;

struct FakeDxgi {
    last_present: Cell<Option<u32>>,
    stats: Cell<StatsRead>,
    qpc: Cell<Option<(i64, i64)>>,
}

impl FakeDxgi {
    fn new(qpc_now: i64, qpc_freq: i64) -> Self {
        Self {
            last_present: Cell::new(Some(0)),
            stats: Cell::new(StatsRead::Pending),
            qpc: Cell::new(Some((qpc_now, qpc_freq))),
        }
    }

    fn present(&self, count: u32) {
        self.last_present.set(Some(count));
    }

    fn land(&self, count: u32, sync_qpc: i64) {
        self.stats.set(StatsRead::Sample(RawDxgiStats {
            present_count: count,
            sync_qpc_time: sync_qpc,
        }));
    }
}

impl DxgiStatsSeam for FakeDxgi {
    fn last_present_count(&self) -> Option<u32> { self.last_present.get() }
    fn frame_statistics(&self) -> StatsRead { self.stats.get() }
    fn qpc_clock(&self) -> Option<(i64, i64)> { self.qpc.get() }
}

fn logging() -> (Rc<RefCell<Vec<FramePresented>>>, OnFramePresented) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    (seen, Box::new(move |p: &FramePresented| sink.borrow_mut().push(p.clone())))
}

#[test]
fn qpc_to_instant_maps_past_and_future_ticks_and_rejects_garbage() {
    let now = crate::time::now() + Duration::from_secs(10);
    let freq = 10_000_000i64; // 10 MHz standard Windows QPC
    let qpc_now = 1_000_000_000i64;
    // 8 ms before now
    assert_eq!(
        qpc_to_instant(now, qpc_now, freq, qpc_now - 80_000),
        Some(now - Duration::from_millis(8))
    );
    // 4.5 ms after now
    assert_eq!(
        qpc_to_instant(now, qpc_now, freq, qpc_now + 45_000),
        Some(now + Duration::from_micros(4_500))
    );
    // Reject zero/negative QPC or frequency
    assert_eq!(qpc_to_instant(now, qpc_now, 0, qpc_now), None);
    assert_eq!(qpc_to_instant(now, qpc_now, freq, 0), None);
    assert_eq!(qpc_to_instant(now, qpc_now, freq, -1), None);
    // Reject > 60s skew
    assert_eq!(qpc_to_instant(now, qpc_now, freq, qpc_now - 61 * freq), None);
}

#[test]
fn poll_interval_scales_with_refresh_and_clamps() {
    assert_eq!(poll_interval(None), MAX_POLL_INTERVAL);
    assert_eq!(poll_interval(Some(60.0)), MAX_POLL_INTERVAL);
    assert_eq!(poll_interval(Some(120.0)), Duration::from_nanos(4_166_667));
    assert_eq!(poll_interval(Some(240.0)), MIN_POLL_INTERVAL);
    assert_eq!(poll_interval(Some(f64::NAN)), MAX_POLL_INTERVAL);
}

#[test]
fn correlates_dxgi_present_count_to_frame_encoded_and_goes_idle_when_landed() {
    let base = crate::time::now() + Duration::from_secs(10);
    let freq = 10_000_000i64;
    let qpc_base = 5_000_000_000i64;
    let dxgi = FakeDxgi::new(qpc_base, freq);
    let mut tracker = DxgiPresentTracker::default();
    let encoded_log = Rc::new(RefCell::new(Vec::<FrameEncoded>::new()));
    let enc_sink = encoded_log.clone();
    let enc_hook: OnFrameEncoded = Box::new(move |f| enc_sink.borrow_mut().push(f.clone()));
    let mut encoded = FrameReports::with_presented_seq(Some(enc_hook), true, Some("main"));
    let (seen, pres_hook) = logging();
    let mut presented = FramePresentReports::new(Some(pres_hook), Some("main"));

    assert_eq!(tracker.next_deadline(), None, "idle before any present");

    // Frame 0 presented: DXGI reports PresentCount=1.
    dxgi.present(1);
    let d0 = tracker.record_present_with(&dxgi, base, Some(120.0)).unwrap();
    let s0 = encoded.drawn(Some(base), None, base, 0.5).unwrap();
    presented.push_pending(d0, s0);
    assert!(tracker.next_deadline().is_some(), "waiting while frame 0 is in flight");

    // Frame 1 is Unchanged (no present).
    encoded.unchanged(None);

    // Poll before vblank: still Pending.
    let mut pending = Vec::new();
    let mut out = Vec::new();
    presented.waiting_drawables(&mut pending);
    assert_eq!(
        tracker.poll_with(&dxgi, base + Duration::from_millis(4), Some(120.0), &pending, &mut out),
        PollAction::KeepWaiting
    );
    assert!(out.is_empty());

    // At +9 ms, DXGI reports PresentCount=1 scanned out at +8 ms.
    let t_poll = base + Duration::from_millis(9);
    let qpc_poll = qpc_base + 90_000;
    let qpc_scanout = qpc_base + 80_000;
    dxgi.qpc.set(Some((qpc_poll, freq)));
    dxgi.land(1, qpc_scanout);
    assert_eq!(
        tracker.poll_with(&dxgi, t_poll, Some(120.0), &pending, &mut out),
        PollAction::Completed
    );
    presented.apply_completions(out.drain(..));

    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(seen.borrow()[0].seq(), s0);
    assert_eq!(seen.borrow()[0].window(), Some("main"));
    assert_eq!(seen.borrow()[0].presented_at(), Some(base + Duration::from_millis(8)));
    assert!(!presented.has_pending());
    assert_eq!(tracker.next_deadline(), None, "idle window must clear wake deadline");
}

#[test]
fn skipped_or_overwritten_older_frames_report_none_without_faking_timestamps() {
    let base = crate::time::now() + Duration::from_secs(10);
    let freq = 10_000_000i64;
    let qpc_base = 5_000_000_000i64;
    let dxgi = FakeDxgi::new(qpc_base, freq);
    let mut tracker = DxgiPresentTracker::default();
    let (seen, pres_hook) = logging();
    let mut presented = FramePresentReports::new(Some(pres_hook), Some("main"));

    for (count, seq) in [(1u32, 10u64), (2, 11), (3, 12)] {
        dxgi.present(count);
        let d = tracker.record_present_with(&dxgi, base, Some(60.0)).unwrap();
        presented.push_pending(d, seq);
    }

    // DXGI's single-slot stats jumped straight to PresentCount=3 (frames 1 and 2 were dropped
    // before scanout or overwritten before poll).
    let t_poll = base + Duration::from_millis(20);
    dxgi.qpc.set(Some((qpc_base + 200_000, freq)));
    dxgi.land(3, qpc_base + 160_000);
    let mut pending = Vec::new();
    let mut out = Vec::new();
    presented.waiting_drawables(&mut pending);
    assert_eq!(
        tracker.poll_with(&dxgi, t_poll, Some(60.0), &pending, &mut out),
        PollAction::Completed
    );
    presented.apply_completions(out.drain(..));

    let got = seen.borrow();
    assert_eq!(got.len(), 3);
    assert_eq!((got[0].seq(), got[0].presented_at()), (10, None));
    assert_eq!((got[1].seq(), got[1].presented_at()), (11, None));
    assert_eq!((got[2].seq(), got[2].presented_at()), (12, Some(base + Duration::from_millis(16))));
    assert_eq!(tracker.next_deadline(), None);
}

#[test]
fn present_count_u32_wraparound_preserves_monotonic_correlation() {
    let base = crate::time::now() + Duration::from_secs(10);
    let freq = 10_000_000i64;
    let qpc_base = 5_000_000_000i64;
    let dxgi = FakeDxgi::new(qpc_base, freq);
    let mut tracker = DxgiPresentTracker::default();
    let (seen, pres_hook) = logging();
    let mut presented = FramePresentReports::new(Some(pres_hook), Some("main"));

    for (raw, seq) in [(u32::MAX, 1u64), (0u32, 2u64), (1u32, 3u64)] {
        dxgi.present(raw);
        let d = tracker.record_present_with(&dxgi, base, Some(60.0)).unwrap();
        presented.push_pending(d, seq);
    }

    // Stats reports wrapped count 0 (between u32::MAX and 1).
    dxgi.qpc.set(Some((qpc_base + 100_000, freq)));
    dxgi.land(0, qpc_base + 80_000);
    let mut pending = Vec::new();
    let mut out = Vec::new();
    presented.waiting_drawables(&mut pending);
    assert_eq!(
        tracker.poll_with(&dxgi, base + Duration::from_millis(10), Some(60.0), &pending, &mut out),
        PollAction::Completed
    );
    presented.apply_completions(out.drain(..));

    {
        let got = seen.borrow();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].seq(), got[0].presented_at()), (1, None));
        assert_eq!((got[1].seq(), got[1].presented_at()), (2, Some(base + Duration::from_millis(8))));
    }
    assert!(presented.has_pending(), "frame 3 (count 1) is still waiting");
    assert!(tracker.next_deadline().is_some());

    // Then stats reports wrapped count 1.
    dxgi.qpc.set(Some((qpc_base + 200_000, freq)));
    dxgi.land(1, qpc_base + 160_000);
    presented.waiting_drawables(&mut pending);
    assert_eq!(
        tracker.poll_with(&dxgi, base + Duration::from_millis(20), Some(60.0), &pending, &mut out),
        PollAction::Completed
    );
    presented.apply_completions(out.drain(..));

    let got = seen.borrow();
    assert_eq!(got.len(), 3);
    assert_eq!((got[2].seq(), got[2].presented_at()), (3, Some(base + Duration::from_millis(16))));
    assert_eq!(tracker.next_deadline(), None);
}

#[test]
fn unsupported_swap_chain_stops_polling_without_faking_callbacks_until_reset() {
    let base = crate::time::now() + Duration::from_secs(10);
    let dxgi = FakeDxgi::new(5_000_000_000, 10_000_000);
    let mut tracker = DxgiPresentTracker::default();
    let (seen, pres_hook) = logging();
    let mut presented = FramePresentReports::new(Some(pres_hook), Some("main"));

    dxgi.present(1);
    let d0 = tracker.record_present_with(&dxgi, base, Some(60.0)).unwrap();
    presented.push_pending(d0, 0);

    dxgi.stats.set(StatsRead::Unsupported);
    let mut pending = Vec::new();
    let mut out = Vec::new();
    presented.waiting_drawables(&mut pending);
    assert_eq!(
        tracker.poll_with(&dxgi, base + Duration::from_millis(8), Some(60.0), &pending, &mut out),
        PollAction::StopWaiting
    );
    presented.drop_pending();

    assert!(tracker.is_unsupported());
    assert_eq!(tracker.next_deadline(), None);
    assert!(seen.borrow().is_empty(), "unsupported swap chain must never fabricate a callback");
    dxgi.present(2);
    assert_eq!(
        tracker.record_present_with(&dxgi, base + Duration::from_millis(16), Some(60.0)),
        None,
        "stay disabled until swap chain replacement resets tracker"
    );

    // Replacing the swap chain resets unsupported status.
    tracker.reset();
    assert!(!tracker.is_unsupported());
    dxgi.stats.set(StatsRead::Pending);
    dxgi.present(1);
    assert!(tracker.record_present_with(&dxgi, base + Duration::from_millis(32), Some(60.0)).is_some());
}

#[test]
fn settle_timeout_clears_deadline_without_faking_callbacks() {
    let base = crate::time::now() + Duration::from_secs(10);
    let dxgi = FakeDxgi::new(5_000_000_000, 10_000_000);
    let mut tracker = DxgiPresentTracker::default();
    let (seen, pres_hook) = logging();
    let mut presented = FramePresentReports::new(Some(pres_hook), Some("main"));

    dxgi.present(1);
    let d0 = tracker.record_present_with(&dxgi, base, Some(60.0)).unwrap();
    presented.push_pending(d0, 0);

    let mut pending = Vec::new();
    let mut out = Vec::new();
    presented.waiting_drawables(&mut pending);
    // Still pending before SETTLE_TIMEOUT.
    assert_eq!(
        tracker.poll_with(&dxgi, base + Duration::from_millis(50), Some(60.0), &pending, &mut out),
        PollAction::KeepWaiting
    );
    assert!(tracker.next_deadline().is_some());

    // Once SETTLE_TIMEOUT elapses without DXGI reporting the frame (e.g. occluded/minimized),
    // stop polling cleanly without fabricating a timestamp.
    assert_eq!(
        tracker.poll_with(&dxgi, base + SETTLE_TIMEOUT, Some(60.0), &pending, &mut out),
        PollAction::StopWaiting
    );
    presented.drop_pending();
    assert_eq!(tracker.next_deadline(), None);
    assert!(seen.borrow().is_empty());
    assert!(!presented.has_pending());
}
