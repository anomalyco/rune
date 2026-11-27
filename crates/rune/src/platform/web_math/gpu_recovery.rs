//! Web GPU adapter software-fallback rejection and bounded WebGPU device-loss recovery.
//!
//! Kept under `platform/web_math` so every transition and policy decision compiles and runs in
//! native unit tests off `wasm32`.

use std::time::Duration;

use crate::gpu::fallback::{GpuMode, LossEffect, LossEvent, LossState, Policy, step_loss};
use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;
use crate::window::WebStart;

/// Backoff base delay before attempting to re-acquire a lost WebGPU device.
/// Mirrors native `SURFACE_BACKOFF` (100 ms, scaling with recent losses in the forgiveness window).
pub(crate) const WEBGPU_RECOVERY_BACKOFF: Duration = Duration::from_millis(100);

/// Known software rasterizer substrings that can appear in WebGPU or WebGL2 adapter metadata
/// even when a browser does not set `GPUAdapterInfo.isFallbackAdapter`.
const SOFTWARE_ADAPTER_TOKENS: &[&str] = &[
    "swiftshader",
    "llvmpipe",
    "lavapipe",
    "mesa offscreen",
    "microsoft basic render driver",
    "software rasterizer",
];

/// Whether a web page in `start` mode should reject software/fallback GPU adapters and stay on
/// `CpuCanvas`.
///
/// Mirrors native `CpuFirst::Auto` on Windows and Linux: full-window `CpuFirst::Auto` pages
/// already render live on `CpuCanvas`, so upgrading onto SwiftShader / `llvmpipe` is slower and
/// uses more memory. `CpuFirst::Off`, `GpuLossPolicy::Terminal`, `App::web_document`, and inline
/// `App::canvas_id` pages (`WebStart::GpuFirst`) have no CPU canvas and keep accepting software
/// adapters so headless/VM hosts still render.
pub(crate) const fn reject_software_adapters(start: WebStart) -> bool {
    matches!(start, WebStart::CpuFirst)
}

/// Returns `true` if `text` names a known software rasterizer (case-insensitive).
pub(crate) fn is_software_adapter_name(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    SOFTWARE_ADAPTER_TOKENS.iter().any(|token| lower.contains(token))
}

/// Returns `true` if `info` describes a software/fallback adapter (`DeviceType::Cpu`, which wgpu
/// maps from WebGPU `GPUAdapterInfo.isFallbackAdapter` and WebGL2 software renderer strings, or
/// a known software rasterizer name/driver).
pub(crate) fn is_software_adapter(info: &wgpu::AdapterInfo) -> bool {
    info.device_type == wgpu::DeviceType::Cpu
        || is_software_adapter_name(&info.name)
        || is_software_adapter_name(&info.driver)
        || is_software_adapter_name(&info.driver_info)
}

/// Validates an acquired web adapter against the `reject_software` policy before creating a
/// device or locking the canvas to a GPU context.
pub(crate) fn check_adapter(info: &wgpu::AdapterInfo, reject_software: bool) -> Result<(), String> {
    if reject_software && is_software_adapter(info) {
        let label = if info.name.is_empty() { "fallback" } else { info.name.as_str() };
        return Err(format!("software/fallback GPU adapter rejected ({label})"));
    }
    Ok(())
}

/// Whether exhausting WebGPU device-loss recovery terminates the app (`Fatal`) instead of
/// staying on `CpuCanvas`.
pub(crate) const fn webgpu_exhausted_is_fatal(gpu_first: bool, embedded: bool, document: bool) -> bool {
    gpu_first || embedded || document
}

/// Explicit state machine for bounded WebGPU device-loss re-acquisition under
/// `GpuLossPolicy::Recover`.
///
/// Uses [`rune_gpu::fallback::LossState`] (3 losses within a 5-minute forgiveness window, matching
/// Chromium's `GpuProcessHost` crash accounting and Rune's native `SurfaceRetries`). A healthy
/// recovery keeps the accumulated loss count in `Idle` so a crash loop still exhausts its budget
/// instead of retrying forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebGpuRecovery {
    /// Device is live (or not yet lost); holds accumulated recent losses in the forgiveness window.
    Idle { losses: LossState },
    /// Waiting for backoff timer `attempt` (due at `due`) before re-requesting a WebGPU adapter.
    WaitingBackoff { losses: LossState, attempt: Attempt, due: Instant },
    /// Async WebGPU re-acquisition `attempt` is in flight.
    Acquiring { losses: LossState, attempt: Attempt },
    /// Retry budget exhausted or cancelled (`GpuLossPolicy::Terminal`, shader failure, shutdown).
    Exhausted,
}

impl Default for WebGpuRecovery {
    fn default() -> Self {
        Self::Idle { losses: LossState::for_mode(GpuMode::Hardware) }
    }
}

/// Inputs that advance [`WebGpuRecovery`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WebGpuRecoveryEvent {
    /// The browser reported `device.lost` at `now`.
    DeviceLost { now: Instant, reason: String },
    /// The scheduled backoff timer fired for `attempt`.
    RetryDue { attempt: Attempt },
    /// Async re-acquisition `attempt` built and attached a replacement WebGPU `GpuSide`.
    AcquireSucceeded { attempt: Attempt },
    /// Async re-acquisition `attempt` failed at `now` (adapter unavailable, rejected software
    /// adapter, `requestDevice` failed, or surface/renderer creation failed).
    AcquireFailed { attempt: Attempt, now: Instant, reason: String },
    /// Permanently stop any in-flight or future recovery (`GpuLossPolicy::Terminal`, shader
    /// rejection, hand-over timeout, or app shutdown).
    Cancel,
}

/// Side effects the web host executes for a [`WebGpuRecovery`] transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WebGpuRecoveryEffect {
    /// Schedule a `Wake::WebGpuRecover(attempt)` timer after `delay`.
    ScheduleRetry { attempt: Attempt, delay: Duration },
    /// Spawn async `try_webgpu` re-acquisition tagged with `attempt`.
    Acquire { attempt: Attempt },
    /// Bounded recovery exhausted: stay permanently on CPU (full-window `CpuFirst`) or terminate
    /// (`GpuFirst`, inline, document), and emit `GpuFallbackStage::DeviceLost`.
    GiveUp { reason: String },
}

impl WebGpuRecovery {
    /// Whether `attempt` is the currently in-flight re-acquisition.
    pub(crate) fn is_acquiring(&self, attempt: Attempt) -> bool {
        matches!(self, Self::Acquiring { attempt: expected, .. } if *expected == attempt)
    }

    /// Whether recovery has permanently given up or been cancelled.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn is_exhausted(&self) -> bool {
        matches!(self, Self::Exhausted)
    }

    /// Advances the WebGPU device-loss recovery state machine by one `event`.
    pub(crate) fn step(
        &mut self,
        seq: &mut Seq,
        event: WebGpuRecoveryEvent,
    ) -> Effects<WebGpuRecoveryEffect> {
        match (*self, event) {
            (_, WebGpuRecoveryEvent::Cancel) => {
                *self = Self::Exhausted;
                Effects::none()
            }
            (Self::Idle { mut losses }, WebGpuRecoveryEvent::DeviceLost { now, reason }) => {
                self.record_loss_and_schedule(seq, &mut losses, now, reason)
            }
            (
                Self::WaitingBackoff { losses, attempt: expected, .. },
                WebGpuRecoveryEvent::RetryDue { attempt },
            ) if attempt == expected => {
                let probe = seq.next();
                *self = Self::Acquiring { losses, attempt: probe };
                Effects::one(WebGpuRecoveryEffect::Acquire { attempt: probe })
            }
            (
                Self::Acquiring { losses, attempt: expected },
                WebGpuRecoveryEvent::AcquireSucceeded { attempt },
            ) if attempt == expected => {
                *self = Self::Idle { losses };
                Effects::none()
            }
            (
                Self::Acquiring { mut losses, attempt: expected },
                WebGpuRecoveryEvent::AcquireFailed { attempt, now, reason },
            ) if attempt == expected => {
                self.record_loss_and_schedule(seq, &mut losses, now, reason)
            }
            _ => Effects::none(),
        }
    }

    fn record_loss_and_schedule(
        &mut self,
        seq: &mut Seq,
        losses: &mut LossState,
        now: Instant,
        reason: String,
    ) -> Effects<WebGpuRecoveryEffect> {
        let effect = step_loss(
            losses,
            LossEvent::Replace { now, policy: Policy::Automatic, software_available: false },
        )
        .into_iter()
        .next();
        match effect {
            Some(LossEffect::Replace { .. }) => {
                let count = losses.effective_losses(now).max(1);
                let delay = WEBGPU_RECOVERY_BACKOFF * u32::from(count);
                let attempt = seq.next();
                *self = Self::WaitingBackoff { losses: *losses, attempt, due: now + delay };
                Effects::one(WebGpuRecoveryEffect::ScheduleRetry { attempt, delay })
            }
            Some(LossEffect::GiveUp) | None => {
                *self = Self::Exhausted;
                Effects::one(WebGpuRecoveryEffect::GiveUp { reason })
            }
        }
    }
}

#[cfg(test)]
#[path = "gpu_recovery_tests.rs"]
mod tests;
