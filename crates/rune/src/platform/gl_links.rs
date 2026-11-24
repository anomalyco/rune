//! Explicit state machine for polling deferred WebGL2 shader program links
//! (`KHR_parallel_shader_compile`) across browser turns, coordinating fallback from
//! Windfoil to Classic and stopping cleanly on completion, rejection, context loss,
//! timeout, or page quit.
//!
//! Kept outside `platform/web/` (which only builds on `wasm32`) so every transition,
//! early-use outcome, and idle poll count runs in headless native tests.
#![cfg_attr(not(test), allow(dead_code))]

use std::time::Duration;
use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;

/// Interval between non-blocking `COMPLETION_STATUS_KHR` polls while WebGL2 links are in flight.
pub(crate) const GL_LINK_POLL_INTERVAL: Duration = Duration::from_millis(8);
/// Maximum time to wait for deferred WebGL2 shader links before falling back to CPU or failing.
pub(crate) const GL_LINK_TIMEOUT: Duration = Duration::from_secs(5);

/// Which uber shader variant's deferred links are currently in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkPhase {
    /// Primary Windfoil shader (outlines + uber): a link rejection falls back to Classic.
    Windfoil,
    /// Last-resort Classic shader: a link rejection fails WebGL2 startup (CPU fallback or error).
    Classic,
}

/// Platform-independent snapshot of one `wgpu_hal::gles::Device::poll_deferred_links` call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LinkPollOutcome {
    /// Live programs still compiling or linking in the driver.
    pub pending: usize,
    /// First compile/link failure observed during this poll (including an earlier failure
    /// materialized when a pending pipeline was used early).
    pub error: Option<String>,
    /// True when the WebGL2 context was lost while links were in flight.
    pub context_lost: bool,
}

/// Explicit state machine for deferred WebGL2 shader linking during `deliver_gpu`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum GlLinkState {
    /// No deferred links have been submitted; nothing is scheduled.
    #[default]
    Idle,
    /// Waiting for `phase`'s deferred links to finish; one timer is armed for `attempt`.
    Polling {
        attempt: Attempt,
        phase: LinkPhase,
        deadline: Instant,
    },
    /// Windfoil's deferred link failed; waiting for the caller to submit Classic's deferred build.
    AwaitingClassic,
    /// Every deferred link completed without error; the prebuilt renderer is ready to deliver.
    Ready,
    /// The last-resort shader was rejected by the GL driver.
    Failed { reason: String },
    /// The WebGL2 context was lost while links were pending.
    ContextLost,
    /// Links did not finish within `GL_LINK_TIMEOUT`.
    TimedOut,
    /// The page quit while links were pending.
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GlLinkEvent {
    /// Built a renderer for `phase` with deferred linking enabled; `pending` programs still
    /// await `COMPLETION_STATUS_KHR` completion (0 means already ready with nothing to poll).
    Submitted { phase: LinkPhase, pending: usize, now: Instant },
    /// The scheduled poll timer for `attempt` fired and `poll_deferred_links` returned `outcome`.
    PollDue { attempt: Attempt, now: Instant, outcome: LinkPollOutcome },
    /// Synchronous submission failed before any GL program was queued (e.g. Naga validation/OOM).
    SubmitFailed { phase: LinkPhase, reason: String },
    /// The page is quitting; abandon pending work immediately.
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GlLinkEffect {
    /// Schedule a `PollDue` check for `attempt` after `delay`.
    SchedulePoll { attempt: Attempt, delay: Duration },
    /// Drop the failed Windfoil renderer and build Classic with deferred links enabled.
    BuildClassic,
    /// Deliver the prebuilt, verified renderer into `gpu_side`.
    Deliver,
    /// Destroy the WebGL2 device and report `reason` (CPU fallback on CPU-first, error on GPU-first).
    Fail(String),
    /// Destroy the WebGL2 device silently because the page quit.
    Abort,
}

impl GlLinkState {
    /// True while a poll timer is armed for pending links.
    pub(crate) fn is_polling(&self) -> bool {
        matches!(self, Self::Polling { .. })
    }

    pub(crate) fn step(
        &mut self,
        seq: &mut Seq,
        event: GlLinkEvent,
    ) -> Effects<GlLinkEffect> {
        match (&*self, event) {
            (
                Self::Idle,
                GlLinkEvent::Submitted { phase, pending, now },
            )
            | (
                Self::AwaitingClassic,
                GlLinkEvent::Submitted { phase: phase @ LinkPhase::Classic, pending, now },
            ) => {
                if pending == 0 {
                    *self = Self::Ready;
                    Effects::one(GlLinkEffect::Deliver)
                } else {
                    let attempt = seq.next();
                    *self = Self::Polling {
                        attempt,
                        phase,
                        deadline: now + GL_LINK_TIMEOUT,
                    };
                    Effects::one(GlLinkEffect::SchedulePoll {
                        attempt,
                        delay: Duration::ZERO,
                    })
                }
            }
            (Self::Idle, GlLinkEvent::SubmitFailed { phase: LinkPhase::Windfoil, .. }) => {
                *self = Self::AwaitingClassic;
                Effects::one(GlLinkEffect::BuildClassic)
            }
            (
                Self::Idle | Self::AwaitingClassic,
                GlLinkEvent::SubmitFailed { phase: LinkPhase::Classic, reason },
            ) => {
                *self = Self::Failed { reason: reason.clone() };
                Effects::one(GlLinkEffect::Fail(reason))
            }
            (
                Self::Polling { attempt: expected, phase, deadline },
                GlLinkEvent::PollDue { attempt, now, outcome },
            ) if *expected == attempt => {
                let phase = *phase;
                let deadline = *deadline;
                if outcome.context_lost {
                    *self = Self::ContextLost;
                    Effects::one(GlLinkEffect::Fail(
                        "WebGL2 context lost while compiling shaders".into(),
                    ))
                } else if let Some(why) = outcome.error {
                    match phase {
                        LinkPhase::Windfoil => {
                            *self = Self::AwaitingClassic;
                            Effects::one(GlLinkEffect::BuildClassic)
                        }
                        LinkPhase::Classic => {
                            let reason = format!("the GPU rejected Rune's shader: {why}");
                            *self = Self::Failed { reason: reason.clone() };
                            Effects::one(GlLinkEffect::Fail(reason))
                        }
                    }
                } else if outcome.pending == 0 {
                    *self = Self::Ready;
                    Effects::one(GlLinkEffect::Deliver)
                } else if now >= deadline {
                    *self = Self::TimedOut;
                    Effects::one(GlLinkEffect::Fail(
                        "WebGL2 shader compilation timed out".into(),
                    ))
                } else {
                    let next = seq.next();
                    let remaining = deadline.saturating_duration_since(now);
                    let delay = GL_LINK_POLL_INTERVAL.min(remaining);
                    *self = Self::Polling { attempt: next, phase, deadline };
                    Effects::one(GlLinkEffect::SchedulePoll { attempt: next, delay })
                }
            }
            (Self::Idle | Self::Polling { .. } | Self::AwaitingClassic, GlLinkEvent::Quit) => {
                *self = Self::Aborted;
                Effects::one(GlLinkEffect::Abort)
            }
            _ => Effects::none(),
        }
    }
}

#[cfg(test)]
#[path = "gl_links_tests.rs"]
mod tests;
