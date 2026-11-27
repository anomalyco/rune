//! Windows DX12 / DXGI actual presentation timestamps for `App::on_frame_presented`.
//!
//! Reads `IDXGISwapChain::GetLastPresentCount` after each flip-model `Present` and polls
//! `IDXGISwapChain::GetFrameStatistics` (falling back to `IDXGISwapChainMedia::GetFrameStatisticsMedia`
//! if `GetFrameStatistics` returns `DXGI_ERROR_UNSUPPORTED`) to obtain the hardware `SyncQPCTime`
//! when that frame actually scanned out to the display.
//!
//! Because DXGI's frame-statistics API exposes only the most recently completed present rather
//! than a per-frame callback queue:
//! - An exact `PresentCount` match reports `Some(Instant)` mapped from `SyncQPCTime` onto
//!   `crate::time::now()` via `QueryPerformanceCounter` / `QueryPerformanceFrequency`.
//! - Any older pending frame strictly behind `stats.PresentCount` (dropped by DWM before scanout
//!   or overwritten in DXGI's single slot before polling) reports `None` rather than fabricating a
//!   timestamp, unblocking newer landed frames in strict submission order.
//! - `DXGI_ERROR_FRAME_STATISTICS_DISJOINT` (expected on the first call or after a mode change)
//!   retries once immediately to clear the latch.
//! - `DXGI_ERROR_UNSUPPORTED` disables polling for the current swap chain and drops pending entries
//!   without faking callbacks.
//! - While frames are in flight, a bounded poll deadline (`4..=8 ms`) wakes the loop until either
//!   all frames land or `SETTLE_TIMEOUT` (150 ms) expires; once idle, zero wake deadlines exist.

#![allow(unsafe_code)]

use std::time::Duration;

use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_FRAME_STATISTICS_DISJOINT, DXGI_ERROR_UNSUPPORTED, IDXGISwapChain3,
    IDXGISwapChainMedia,
};
use windows_core::Interface;
use windows_sys::Win32::System::Performance::{
    QueryPerformanceCounter, QueryPerformanceFrequency,
};

use crate::time::Instant;

/// Sanity bound on QPC-to-now delta to reject bogus or uninitialized driver timestamps.
const MAX_CLOCK_SKEW: Duration = Duration::from_secs(60);

/// Maximum forward `PresentCount` jump accepted in one step before treating the counter as
/// reset or out-of-window.
const MAX_COUNT_WINDOW: u32 = 1024;

/// Bounds on the idle-settle poll cadence while waiting for an in-flight DXGI present to scan out.
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(4);
const MAX_POLL_INTERVAL: Duration = Duration::from_millis(8);

/// Maximum wait after the most recent tracked present before giving up (for example when the
/// window is occluded, minimized, or on a disconnected/RDP output) so idle CPU stays zero.
pub(crate) const SETTLE_TIMEOUT: Duration = crate::frame::present_time::SETTLE;

/// Raw DXGI frame statistics fields needed for presentation correlation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawDxgiStats {
    pub(crate) present_count: u32,
    pub(crate) sync_qpc_time: i64,
}

/// Result of one non-blocking DXGI frame-statistics query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatsRead {
    /// A populated sample returned by DXGI.
    Sample(RawDxgiStats),
    /// No completed presentation available yet (or transient disjoint/occlusion error).
    Pending,
    /// This swap chain does not support DXGI frame statistics (`DXGI_ERROR_UNSUPPORTED`).
    Unsupported,
}

/// Narrow testable seam over `IDXGISwapChain3` and the Windows QPC clock.
pub(crate) trait DxgiStatsSeam {
    fn last_present_count(&self) -> Option<u32>;
    fn frame_statistics(&self) -> StatsRead;
    fn qpc_clock(&self) -> Option<(i64, i64)>;
}

/// Live `IDXGISwapChain3` implementation of [`DxgiStatsSeam`].
struct SwapChainSeam<'a>(&'a IDXGISwapChain3);

impl DxgiStatsSeam for SwapChainSeam<'_> {
    fn last_present_count(&self) -> Option<u32> {
        // SAFETY: `self.0` is a live `IDXGISwapChain3` borrowed while the surface is alive.
        unsafe { self.0.GetLastPresentCount() }.ok()
    }

    fn frame_statistics(&self) -> StatsRead {
        // SAFETY: `self.0` is a live `IDXGISwapChain3` with valid out-parameter.
        let read_std = || unsafe {
            let mut stats = std::mem::zeroed();
            self.0.GetFrameStatistics(&mut stats).map(|()| RawDxgiStats {
                present_count: stats.PresentCount,
                sync_qpc_time: stats.SyncQPCTime,
            })
        };
        match read_std() {
            Ok(sample) => StatsRead::Sample(sample),
            Err(err) if err.code() == DXGI_ERROR_FRAME_STATISTICS_DISJOINT => match read_std() {
                Ok(sample) => StatsRead::Sample(sample),
                Err(_) => StatsRead::Pending,
            },
            Err(err) if err.code() == DXGI_ERROR_UNSUPPORTED => {
                // DirectComposition flip swap chains may expose `IDXGISwapChainMedia`.
                let Ok(media) = self.0.cast::<IDXGISwapChainMedia>() else {
                    return StatsRead::Unsupported;
                };
                // SAFETY: `media` is a live `IDXGISwapChainMedia` interface on the same swap chain.
                let read_media = || unsafe {
                    let mut stats = std::mem::zeroed();
                    media.GetFrameStatisticsMedia(&mut stats).map(|()| RawDxgiStats {
                        present_count: stats.PresentCount,
                        sync_qpc_time: stats.SyncQPCTime,
                    })
                };
                match read_media() {
                    Ok(sample) => StatsRead::Sample(sample),
                    Err(e) if e.code() == DXGI_ERROR_FRAME_STATISTICS_DISJOINT => {
                        match read_media() {
                            Ok(sample) => StatsRead::Sample(sample),
                            Err(_) => StatsRead::Pending,
                        }
                    }
                    Err(e) if e.code() == DXGI_ERROR_UNSUPPORTED => StatsRead::Unsupported,
                    Err(_) => StatsRead::Pending,
                }
            }
            Err(_) => StatsRead::Pending,
        }
    }

    fn qpc_clock(&self) -> Option<(i64, i64)> {
        let (mut qpc, mut freq) = (0i64, 0i64);
        // SAFETY: plain out-pointers; both Win32 APIs have no preconditions.
        let ok = unsafe {
            QueryPerformanceCounter(&mut qpc) != 0 && QueryPerformanceFrequency(&mut freq) != 0
        };
        (ok && qpc > 0 && freq > 0).then_some((qpc, freq))
    }
}

/// Converts a DXGI `SyncQPCTime` tick count to a Rust [`Instant`] using paired `now` /
/// `QueryPerformanceCounter` / `QueryPerformanceFrequency` readings.
///
/// Rejects non-positive frequencies or timestamps and deltas exceeding [`MAX_CLOCK_SKEW`].
pub(crate) fn qpc_to_instant(
    now: Instant,
    qpc_now: i64,
    qpc_freq: i64,
    sync_qpc: i64,
) -> Option<Instant> {
    if qpc_freq <= 0 || qpc_now <= 0 || sync_qpc <= 0 {
        return None;
    }
    let delta_ticks = i128::from(sync_qpc) - i128::from(qpc_now);
    let delta_ns = delta_ticks.unsigned_abs().checked_mul(1_000_000_000)? / (qpc_freq as u128);
    if delta_ns > MAX_CLOCK_SKEW.as_nanos() {
        return None;
    }
    let delta = Duration::from_nanos(delta_ns as u64);
    if delta_ticks >= 0 {
        now.checked_add(delta)
    } else {
        now.checked_sub(delta)
    }
}

/// Poll cadence for waiting on an in-flight DXGI frame: half a display refresh clamped to
/// `4..=8 ms` so 60 Hz and 120 Hz+ panels settle within 1–2 wakes without spinning.
pub(crate) fn poll_interval(refresh_hz: Option<f64>) -> Duration {
    let Some(hz) = refresh_hz.filter(|hz| hz.is_finite() && *hz > 0.0) else {
        return MAX_POLL_INTERVAL;
    };
    let half_period_ns = (500_000_000.0 / hz).round().clamp(0.0, u64::MAX as f64) as u64;
    Duration::from_nanos(half_period_ns).clamp(MIN_POLL_INTERVAL, MAX_POLL_INTERVAL)
}

/// Mode of [`DxgiPresentTracker`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// No in-flight tracked presents; idle with no wake deadline.
    #[default]
    Idle,
    /// At least one tracked present is waiting for `GetFrameStatistics` to advance.
    Waiting {
        last_submitted: Instant,
        next_poll: Instant,
    },
    /// This swap chain reported `DXGI_ERROR_UNSUPPORTED`; disabled until the swap chain resets.
    Unsupported,
}

/// Outcome of polling [`DxgiPresentTracker`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PollAction {
    /// No new completions; keep waiting if entries remain.
    KeepWaiting,
    /// One or more completions were pushed to `out`.
    Completed,
    /// Swap chain does not support DXGI stats or `SETTLE_TIMEOUT` elapsed: caller should drop
    /// remaining waiting entries without faking callbacks so the window stays idle.
    StopWaiting,
}

/// Per-window DXGI presentation timestamp tracker owned by Windows `Win::native`.
#[derive(Default)]
pub(crate) struct DxgiPresentTracker {
    state: State,
    /// Most recent `GetLastPresentCount()` mapped to a monotonic `u64` across `u32` wraparound.
    last_present_raw: Option<(u32, u64)>,
    /// Highest unwrapped `PresentCount` already resolved from `GetFrameStatistics`.
    last_stats_count: Option<u64>,
    /// Scratch buffer for `waiting_drawables`, reused to avoid per-frame allocations.
    pending_scratch: Vec<u64>,
    /// Scratch buffer for completions, reused to avoid per-frame allocations.
    completions_scratch: Vec<(u64, Option<Instant>)>,
}

impl DxgiPresentTracker {
    /// Records the `PresentCount` of a just-presented frame on `surface`.
    pub(crate) fn record_present(
        &mut self,
        surface: &wgpu::Surface<'_>,
        now: Instant,
        refresh_hz: Option<f64>,
    ) -> Option<u64> {
        if matches!(self.state, State::Unsupported) {
            return None;
        }
        // SAFETY: borrows the live DX12 swap chain only for the duration of `GetLastPresentCount`.
        let swap_chain = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>()?.swap_chain()? };
        self.record_present_with(&SwapChainSeam(&swap_chain), now, refresh_hz)
    }

    /// Testable core of [`Self::record_present`].
    pub(crate) fn record_present_with(
        &mut self,
        seam: &impl DxgiStatsSeam,
        now: Instant,
        refresh_hz: Option<f64>,
    ) -> Option<u64> {
        if matches!(self.state, State::Unsupported) {
            return None;
        }
        let raw = seam.last_present_count()?;
        let unwrapped = match self.last_present_raw {
            None => {
                if raw == 0 {
                    return None;
                }
                u64::from(raw)
            }
            Some((prev_raw, prev_u64)) => {
                let delta = raw.wrapping_sub(prev_raw);
                if delta == 0 || delta > MAX_COUNT_WINDOW {
                    return None;
                }
                prev_u64 + u64::from(delta)
            }
        };
        self.last_present_raw = Some((raw, unwrapped));
        self.state = State::Waiting {
            last_submitted: now,
            next_poll: now + poll_interval(refresh_hz),
        };
        Some(unwrapped)
    }

    /// Polls `surface`'s DXGI swap chain and applies any landed presentation timestamps directly
    /// to `reports`.
    pub(crate) fn poll_surface(
        &mut self,
        surface: &wgpu::Surface<'_>,
        reports: &mut crate::frame::presented::FramePresentReports,
        now: Instant,
        refresh_hz: Option<f64>,
    ) {
        if !reports.has_pending() {
            if matches!(self.state, State::Waiting { .. }) {
                self.state = State::Idle;
            }
            return;
        }
        if matches!(self.state, State::Unsupported) {
            reports.drop_pending();
            return;
        }
        // SAFETY: borrows the live DX12 swap chain only for the duration of `GetFrameStatistics`.
        let Some(swap_chain) =
            (unsafe { surface.as_hal::<wgpu::hal::api::Dx12>().and_then(|h| h.swap_chain()) })
        else {
            return;
        };
        let mut pending = std::mem::take(&mut self.pending_scratch);
        let mut completions = std::mem::take(&mut self.completions_scratch);
        reports.waiting_drawables(&mut pending);
        completions.clear();
        let action = self.poll_with(
            &SwapChainSeam(&swap_chain),
            now,
            refresh_hz,
            &pending,
            &mut completions,
        );
        match action {
            PollAction::Completed => {
                reports.apply_completions(completions.drain(..));
            }
            PollAction::StopWaiting => {
                reports.drop_pending();
            }
            PollAction::KeepWaiting => {}
        }
        self.pending_scratch = pending;
        self.completions_scratch = completions;
    }

    /// Testable core of [`Self::poll_surface`].
    pub(crate) fn poll_with(
        &mut self,
        seam: &impl DxgiStatsSeam,
        now: Instant,
        refresh_hz: Option<f64>,
        pending: &[u64],
        out: &mut Vec<(u64, Option<Instant>)>,
    ) -> PollAction {
        if pending.is_empty() {
            if matches!(self.state, State::Waiting { .. }) {
                self.state = State::Idle;
            }
            return PollAction::KeepWaiting;
        }
        if matches!(self.state, State::Unsupported) {
            return PollAction::StopWaiting;
        }
        match seam.frame_statistics() {
            StatsRead::Unsupported => {
                self.state = State::Unsupported;
                return PollAction::StopWaiting;
            }
            StatsRead::Sample(stats) => {
                if let Some(stats_u64) = self.unwrap_stats_count(stats)
                    && self.last_stats_count.is_none_or(|prev| stats_u64 >= prev)
                    && pending.iter().any(|&p| p <= stats_u64)
                {
                    let presented_at = seam.qpc_clock().and_then(|(qpc_now, qpc_freq)| {
                        qpc_to_instant(now, qpc_now, qpc_freq, stats.sync_qpc_time)
                    });
                    let mut all_resolved = true;
                    for &p in pending {
                        if p == stats_u64 {
                            out.push((p, presented_at));
                        } else if p < stats_u64 {
                            // Superseded/dropped before scanout or overwritten before poll:
                            // never fabricate a timestamp, and unblock newer landed frames.
                            out.push((p, None));
                        } else {
                            all_resolved = false;
                        }
                    }
                    self.last_stats_count = Some(stats_u64);
                    if all_resolved {
                        self.state = State::Idle;
                    } else if let State::Waiting { last_submitted, .. } = self.state {
                        let interval = poll_interval(refresh_hz);
                        self.state = State::Waiting {
                            last_submitted,
                            next_poll: (now + interval).min(last_submitted + SETTLE_TIMEOUT),
                        };
                    }
                    return PollAction::Completed;
                }
            }
            StatsRead::Pending => {}
        }

        if let State::Waiting { last_submitted, next_poll } = self.state {
            let expire = last_submitted + SETTLE_TIMEOUT;
            if now >= expire {
                self.state = State::Idle;
                return PollAction::StopWaiting;
            }
            if now >= next_poll {
                self.state = State::Waiting {
                    last_submitted,
                    next_poll: (now + poll_interval(refresh_hz)).min(expire),
                };
            }
        }
        PollAction::KeepWaiting
    }

    fn unwrap_stats_count(&self, stats: RawDxgiStats) -> Option<u64> {
        if stats.sync_qpc_time <= 0 {
            return None;
        }
        let (last_raw, last_u64) = self.last_present_raw?;
        // A fresh swap chain reports PresentCount == 0 before any frame has scanned out; only
        // accept 0 after the u32 counter has wrapped past u32::MAX.
        if stats.present_count == 0 && last_u64 <= u64::from(u32::MAX) {
            return None;
        }
        let signed_diff = stats.present_count.wrapping_sub(last_raw) as i32;
        if signed_diff <= 0 {
            let behind = u32::try_from(-signed_diff).ok()?;
            if behind > MAX_COUNT_WINDOW {
                return None;
            }
            last_u64.checked_sub(u64::from(behind))
        } else {
            let ahead = u32::try_from(signed_diff).ok()?;
            if ahead > MAX_COUNT_WINDOW {
                return None;
            }
            last_u64.checked_add(u64::from(ahead))
        }
    }

    /// Next event-loop wake deadline while waiting for an in-flight DXGI frame to scan out, or
    /// `None` when idle or unsupported.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            State::Waiting { next_poll, .. } => Some(next_poll),
            State::Idle | State::Unsupported => None,
        }
    }

    /// Transitions `Waiting` back to `Idle` once no pending frames remain, preserving
    /// `Unsupported` and the swap chain's `PresentCount` baseline.
    pub(crate) fn clear_waiting(&mut self) {
        if matches!(self.state, State::Waiting { .. }) {
            self.state = State::Idle;
        }
    }

    /// Clears all state when the swap chain is replaced, lost, hidden, or released.
    pub(crate) fn reset(&mut self) {
        self.state = State::Idle;
        self.last_present_raw = None;
        self.last_stats_count = None;
    }

    #[cfg(test)]
    pub(crate) fn is_unsupported(&self) -> bool {
        matches!(self.state, State::Unsupported)
    }
}

#[cfg(test)]
#[path = "present_time_tests.rs"]
mod tests;
