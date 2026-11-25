//! Pure playback state machine for [`AudioOutput`](super::AudioOutput): one `Phase` enum
//! holding only the state valid in each mode, and one `step` function returning
//! [`Effects`](rune_core::machine::Effects) against an injected monotonic clock.

use super::{AudioEvent, AudioOutputState, AudioPosition};
use rune_core::machine::Effects;
use rune_core::time::Instant;
use std::time::Duration;

/// Safety bound while draining if a device stops reporting decreasing latency.
pub(crate) const DRAIN_STALL_PATIENCE: Duration = Duration::from_secs(2);

/// Device-side action that the worker must execute before writing the next audio quantum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingDevice {
    /// Discard all buffered device audio with no tail rewind (from `flush`).
    FlushOnly,
    /// Query device latency, flush the device immediately, and rewind unheard frames from the
    /// tail buffer back ahead of the unread ring (from `pause`).
    PauseRewind {
        /// Cumulative frames written in the segment before pause (`segment_base + written`).
        total_written: u64,
        /// Monotonic floor of frames already reported as heard before pause.
        heard_floor: u64,
    },
}

/// Playback lifecycle phase. Each variant carries only the fields valid in that phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Output is paused; the worker parks with zero wakeups once `pending` is `None`.
    Paused {
        heard: u64,
        /// True when `finish()` was requested before pausing so `play()` resumes draining.
        resume_draining: bool,
        pending: Option<PendingDevice>,
    },
    /// Streaming queued PCM to the output device.
    Playing {
        heard: u64,
        written: u64,
        segment_base: u64,
        latency: u64,
        anchor_at: Instant,
        anchor_heard: u64,
        /// True once at least one frame has been written since open or the last `flush()`.
        started: bool,
        /// Edge latch: prevents duplicate `AudioEvent::Underrun` notifications while dry.
        underrun: bool,
        pending: Option<PendingDevice>,
    },
    /// End-of-stream drain: rejects new pushes and drains queued + device-buffered audio until
    /// empty and device latency reaches zero, then transitions to `Ended`.
    Draining {
        heard: u64,
        written: u64,
        segment_base: u64,
        latency: u64,
        anchor_at: Instant,
        anchor_heard: u64,
        /// Lowest device latency seen after the app ring emptied, and when it last decreased.
        drain_progress: Option<(u64, Instant)>,
        pending: Option<PendingDevice>,
    },
    /// Stream finished draining cleanly; worker parks until `flush()`.
    Ended {
        heard: u64,
    },
    /// Terminal unrecoverable device error; worker exits.
    DeviceLost {
        heard: u64,
        reason: String,
    },
}

impl Default for Phase {
    fn default() -> Self {
        Self::Paused {
            heard: 0,
            resume_draining: false,
            pending: None,
        }
    }
}

/// Inputs to the audio output state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Play { now: Instant },
    Pause { now: Instant },
    Flush { now: Instant },
    /// `has_buffered` is true when unread frames remain in the app ring or a pause-rewind is
    /// still pending.
    Finish { now: Instant, has_buffered: bool },
    /// Caller pushed new PCM frames into the app buffer.
    Pushed,
    /// Caller sampled the playback clock at `now`.
    Observe { now: Instant },
    /// Worker completed a `PendingDevice::FlushOnly` device flush.
    DeviceFlushed,
    /// Worker queried `latency` and flushed the device for `PendingDevice::PauseRewind`.
    PauseFlushed { latency: u64 },
    /// Worker wrote `frames` to the device and observed `latency` buffered device frames.
    Wrote { frames: u64, latency: u64, now: Instant },
    /// Worker sampled device latency while `Playing` with an empty app ring.
    DryPoll { latency: u64, now: Instant },
    /// Backend reported a recoverable underrun (e.g. ALSA `-EPIPE` after prepare).
    DeviceUnderrun { now: Instant },
    /// Worker sampled device latency while `Draining` with an empty app ring.
    DrainPoll { latency: u64, now: Instant },
    /// Backend reported an unrecoverable device error.
    DeviceLost { reason: String, now: Instant },
}

/// Side effects requested by [`step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    WakeWorker,
    ClearBuffer,
    RewindTail(u64),
    Emit(AudioEvent),
}

impl Phase {
    /// Public output state corresponding to this phase.
    pub(crate) fn public_state(&self) -> AudioOutputState {
        match self {
            Self::Paused { .. } => AudioOutputState::Paused,
            Self::Playing { .. } => AudioOutputState::Playing,
            Self::Draining { .. } => AudioOutputState::Draining,
            Self::Ended { .. } => AudioOutputState::Ended,
            Self::DeviceLost { reason, .. } => AudioOutputState::DeviceLost(reason.clone()),
        }
    }

    /// True while audio is actively playing or draining out to the device.
    pub(crate) fn is_playing(&self) -> bool {
        matches!(self, Self::Playing { .. } | Self::Draining { .. })
    }

    /// True when `push_*` must reject new frames with `AudioPushError::Draining`.
    pub(crate) fn rejects_push_draining(&self) -> bool {
        matches!(
            self,
            Self::Draining { .. } | Self::Ended { .. } | Self::Paused { resume_draining: true, .. }
        )
    }

    /// True when the device is permanently lost.
    pub(crate) fn is_lost(&self) -> bool {
        matches!(self, Self::DeviceLost { .. })
    }

    /// Pending device flush/rewind action, if any.
    pub(crate) fn pending_device(&self) -> Option<PendingDevice> {
        match *self {
            Self::Paused { pending, .. }
            | Self::Playing { pending, .. }
            | Self::Draining { pending, .. } => pending,
            Self::Ended { .. } | Self::DeviceLost { .. } => None,
        }
    }

    /// Monotonic heard frame count evaluated at `now` without mutating state.
    pub(crate) fn heard_at(&self, now: Instant, sample_rate: u32) -> u64 {
        match *self {
            Self::Paused { heard, .. }
            | Self::Ended { heard }
            | Self::DeviceLost { heard, .. } => heard,
            Self::Playing {
                heard,
                written,
                segment_base,
                anchor_at,
                anchor_heard,
                started,
                underrun,
                pending,
                ..
            } => {
                if !started || underrun || pending.is_some() {
                    return heard;
                }
                let elapsed = duration_to_frames(now.saturating_duration_since(anchor_at), sample_rate);
                let cap = segment_base.saturating_add(written);
                anchor_heard.saturating_add(elapsed).min(cap).max(heard)
            }
            Self::Draining {
                heard,
                written,
                segment_base,
                anchor_at,
                anchor_heard,
                pending,
                ..
            } => {
                if written == 0 || pending.is_some() {
                    return heard;
                }
                let elapsed = duration_to_frames(now.saturating_duration_since(anchor_at), sample_rate);
                let cap = segment_base.saturating_add(written);
                anchor_heard.saturating_add(elapsed).min(cap).max(heard)
            }
        }
    }

    /// Current device latency estimate in frames at `now`.
    pub(crate) fn latency_at(&self, now: Instant, sample_rate: u32) -> u64 {
        match *self {
            Self::Paused { .. } | Self::Ended { .. } | Self::DeviceLost { .. } => 0,
            Self::Playing {
                written,
                segment_base,
                latency,
                started,
                underrun,
                pending,
                ..
            } => {
                if !started || underrun || pending.is_some() {
                    return 0;
                }
                let heard_now = self.heard_at(now, sample_rate);
                let total = segment_base.saturating_add(written);
                total.saturating_sub(heard_now).min(latency)
            }
            Self::Draining {
                written,
                segment_base,
                latency,
                pending,
                ..
            } => {
                if written == 0 || pending.is_some() {
                    return 0;
                }
                let heard_now = self.heard_at(now, sample_rate);
                let total = segment_base.saturating_add(written);
                total.saturating_sub(heard_now).min(latency)
            }
        }
    }

    /// Builds an `AudioPosition` snapshot at `now`.
    pub(crate) fn position(&self, now: Instant, sample_rate: u32, queued_frames: u64) -> AudioPosition {
        let heard_frames = self.heard_at(now, sample_rate);
        let latency_frames = self.latency_at(now, sample_rate);
        AudioPosition {
            heard_frames,
            sample_rate,
            queued_frames,
            latency: frames_to_duration(latency_frames, sample_rate),
        }
    }
}

/// Advances `phase` by one `event` and returns the side effects the caller must carry out.
pub(crate) fn step(phase: &mut Phase, sample_rate: u32, event: Event) -> Effects<Effect> {
    match event {
        Event::Play { now } => match *phase {
            Phase::Paused { heard, resume_draining, pending } => {
                if resume_draining {
                    *phase = Phase::Draining {
                        heard,
                        written: 0,
                        segment_base: heard,
                        latency: 0,
                        anchor_at: now,
                        anchor_heard: heard,
                        drain_progress: None,
                        pending,
                    };
                } else {
                    *phase = Phase::Playing {
                        heard,
                        written: 0,
                        segment_base: heard,
                        latency: 0,
                        anchor_at: now,
                        anchor_heard: heard,
                        started: false,
                        underrun: false,
                        pending,
                    };
                }
                Effects::one(Effect::WakeWorker)
            }
            Phase::Playing { .. }
            | Phase::Draining { .. }
            | Phase::Ended { .. }
            | Phase::DeviceLost { .. } => Effects::none(),
        },

        Event::Pause { now } => match *phase {
            Phase::Playing {
                written,
                segment_base,
                pending,
                ..
            } => {
                let snapped = phase.heard_at(now, sample_rate);
                let next_pending = pause_pending(pending, segment_base, written, snapped);
                *phase = Phase::Paused {
                    heard: snapped,
                    resume_draining: false,
                    pending: next_pending,
                };
                if next_pending.is_some() {
                    Effects::one(Effect::WakeWorker)
                } else {
                    Effects::none()
                }
            }
            Phase::Draining {
                written,
                segment_base,
                pending,
                ..
            } => {
                let snapped = phase.heard_at(now, sample_rate);
                let next_pending = pause_pending(pending, segment_base, written, snapped);
                *phase = Phase::Paused {
                    heard: snapped,
                    resume_draining: true,
                    pending: next_pending,
                };
                if next_pending.is_some() {
                    Effects::one(Effect::WakeWorker)
                } else {
                    Effects::none()
                }
            }
            Phase::Paused { .. } | Phase::Ended { .. } | Phase::DeviceLost { .. } => Effects::none(),
        },

        Event::Flush { now } => match *phase {
            Phase::DeviceLost { .. } => Effects::one(Effect::ClearBuffer),
            Phase::Playing { .. } => {
                *phase = Phase::Playing {
                    heard: 0,
                    written: 0,
                    segment_base: 0,
                    latency: 0,
                    anchor_at: now,
                    anchor_heard: 0,
                    started: false,
                    underrun: false,
                    pending: Some(PendingDevice::FlushOnly),
                };
                Effects::two(Effect::ClearBuffer, Effect::WakeWorker)
            }
            Phase::Paused { .. } | Phase::Draining { .. } | Phase::Ended { .. } => {
                *phase = Phase::Paused {
                    heard: 0,
                    resume_draining: false,
                    pending: Some(PendingDevice::FlushOnly),
                };
                Effects::two(Effect::ClearBuffer, Effect::WakeWorker)
            }
        },

        Event::Finish { now, has_buffered } => match *phase {
            Phase::Paused { heard, pending, .. } => {
                if !has_buffered && pending.is_none() {
                    *phase = Phase::Ended { heard };
                    Effects::one(Effect::Emit(AudioEvent::Ended))
                } else {
                    *phase = Phase::Paused {
                        heard,
                        resume_draining: true,
                        pending,
                    };
                    Effects::none()
                }
            }
            Phase::Playing {
                written,
                segment_base,
                latency,
                anchor_at,
                anchor_heard,
                pending,
                ..
            } => {
                let snapped = phase.heard_at(now, sample_rate);
                if !has_buffered && written == 0 && pending.is_none() {
                    *phase = Phase::Ended { heard: snapped };
                    Effects::one(Effect::Emit(AudioEvent::Ended))
                } else {
                    *phase = Phase::Draining {
                        heard: snapped,
                        written,
                        segment_base,
                        latency,
                        anchor_at,
                        anchor_heard: anchor_heard.max(snapped),
                        drain_progress: None,
                        pending,
                    };
                    Effects::one(Effect::WakeWorker)
                }
            }
            Phase::Draining { .. } | Phase::Ended { .. } | Phase::DeviceLost { .. } => {
                Effects::none()
            }
        },

        Event::Pushed => match *phase {
            Phase::Playing { .. } => Effects::one(Effect::WakeWorker),
            Phase::Paused { .. }
            | Phase::Draining { .. }
            | Phase::Ended { .. }
            | Phase::DeviceLost { .. } => Effects::none(),
        },

        Event::Observe { now } => {
            let observed = phase.heard_at(now, sample_rate);
            match phase {
                Phase::Playing { heard, .. } | Phase::Draining { heard, .. } => {
                    *heard = (*heard).max(observed);
                }
                Phase::Paused { .. } | Phase::Ended { .. } | Phase::DeviceLost { .. } => {}
            }
            Effects::none()
        }

        Event::DeviceFlushed => {
            match phase {
                Phase::Paused { pending, .. }
                | Phase::Playing { pending, .. }
                | Phase::Draining { pending, .. } => {
                    if matches!(*pending, Some(PendingDevice::FlushOnly)) {
                        *pending = None;
                    }
                }
                Phase::Ended { .. } | Phase::DeviceLost { .. } => {}
            }
            Effects::none()
        }

        Event::PauseFlushed { latency } => match phase {
            Phase::Paused { heard, pending, .. } => {
                if let Some(PendingDevice::PauseRewind { total_written, heard_floor }) = *pending {
                    let final_heard = total_written
                        .saturating_sub(latency)
                        .max(heard_floor)
                        .max(*heard)
                        .min(total_written);
                    let unheard = total_written.saturating_sub(final_heard);
                    *heard = final_heard;
                    *pending = None;
                    if unheard > 0 {
                        Effects::one(Effect::RewindTail(unheard))
                    } else {
                        Effects::none()
                    }
                } else {
                    Effects::none()
                }
            }
            Phase::Playing {
                heard,
                segment_base,
                anchor_heard,
                pending,
                ..
            }
            | Phase::Draining {
                heard,
                segment_base,
                anchor_heard,
                pending,
                ..
            } => {
                if let Some(PendingDevice::PauseRewind { total_written, heard_floor }) = *pending {
                    let final_heard = total_written
                        .saturating_sub(latency)
                        .max(heard_floor)
                        .max(*heard)
                        .min(total_written);
                    let unheard = total_written.saturating_sub(final_heard);
                    *heard = final_heard;
                    *segment_base = final_heard;
                    *anchor_heard = final_heard;
                    *pending = None;
                    if unheard > 0 {
                        Effects::one(Effect::RewindTail(unheard))
                    } else {
                        Effects::none()
                    }
                } else {
                    Effects::none()
                }
            }
            Phase::Ended { .. } | Phase::DeviceLost { .. } => Effects::none(),
        },

        Event::Wrote { frames, latency, now } => match phase {
            Phase::Playing {
                heard,
                written,
                segment_base,
                latency: cur_latency,
                anchor_at,
                anchor_heard,
                started,
                underrun,
                pending,
            } => {
                if pending.is_some() {
                    return Effects::none();
                }
                *written = written.saturating_add(frames);
                let total = segment_base.saturating_add(*written);
                let estimated = total.saturating_sub(latency);
                *heard = (*heard).max(estimated).min(total);
                *cur_latency = latency;
                *anchor_at = now;
                *anchor_heard = *heard;
                if frames > 0 {
                    *started = true;
                    *underrun = false;
                }
                Effects::none()
            }
            Phase::Draining {
                heard,
                written,
                segment_base,
                latency: cur_latency,
                anchor_at,
                anchor_heard,
                drain_progress,
                pending,
            } => {
                if pending.is_some() {
                    return Effects::none();
                }
                *written = written.saturating_add(frames);
                let total = segment_base.saturating_add(*written);
                let estimated = total.saturating_sub(latency);
                *heard = (*heard).max(estimated).min(total);
                *cur_latency = latency;
                *anchor_at = now;
                *anchor_heard = *heard;
                *drain_progress = None;
                Effects::none()
            }
            Phase::Paused { .. } | Phase::Ended { .. } | Phase::DeviceLost { .. } => {
                Effects::none()
            }
        },

        Event::DryPoll { latency, now } => match phase {
            Phase::Playing {
                heard,
                written,
                segment_base,
                latency: cur_latency,
                anchor_at,
                anchor_heard,
                started,
                underrun,
                pending,
            } => {
                if pending.is_some() {
                    return Effects::none();
                }
                let total = segment_base.saturating_add(*written);
                let estimated = total.saturating_sub(latency);
                *heard = (*heard).max(estimated).min(total);
                *cur_latency = latency;
                *anchor_at = now;
                *anchor_heard = *heard;
                if *started && latency == 0 && !*underrun {
                    *underrun = true;
                    Effects::one(Effect::Emit(AudioEvent::Underrun))
                } else {
                    Effects::none()
                }
            }
            Phase::Paused { .. }
            | Phase::Draining { .. }
            | Phase::Ended { .. }
            | Phase::DeviceLost { .. } => Effects::none(),
        },

        Event::DeviceUnderrun { now } => match phase {
            Phase::Playing {
                heard,
                written,
                segment_base,
                latency,
                anchor_at,
                anchor_heard,
                started,
                underrun,
                pending,
            } => {
                if pending.is_some() {
                    return Effects::none();
                }
                let total = segment_base.saturating_add(*written);
                *heard = (*heard).max(total);
                *latency = 0;
                *anchor_at = now;
                *anchor_heard = *heard;
                if *started && !*underrun {
                    *underrun = true;
                    Effects::one(Effect::Emit(AudioEvent::Underrun))
                } else {
                    Effects::none()
                }
            }
            Phase::Draining {
                heard,
                written,
                segment_base,
                ..
            } => {
                let final_heard = (*heard).max(segment_base.saturating_add(*written));
                *phase = Phase::Ended { heard: final_heard };
                Effects::one(Effect::Emit(AudioEvent::Ended))
            }
            Phase::Paused { .. } | Phase::Ended { .. } | Phase::DeviceLost { .. } => {
                Effects::none()
            }
        },

        Event::DrainPoll { latency, now } => match phase {
            Phase::Draining {
                heard,
                written,
                segment_base,
                latency: cur_latency,
                anchor_at,
                anchor_heard,
                drain_progress,
                pending,
            } => {
                if pending.is_some() {
                    return Effects::none();
                }
                let total = segment_base.saturating_add(*written);
                let estimated = total.saturating_sub(latency);
                *heard = (*heard).max(estimated).min(total);
                *cur_latency = latency;
                *anchor_at = now;
                *anchor_heard = *heard;
                let stalled = match *drain_progress {
                    None => {
                        *drain_progress = Some((latency, now));
                        false
                    }
                    Some((best, since)) => {
                        if latency < best {
                            *drain_progress = Some((latency, now));
                            false
                        } else {
                            now.saturating_duration_since(since) >= DRAIN_STALL_PATIENCE
                        }
                    }
                };
                if latency == 0 || stalled {
                    let final_heard = (*heard).max(total);
                    *phase = Phase::Ended { heard: final_heard };
                    Effects::one(Effect::Emit(AudioEvent::Ended))
                } else {
                    Effects::none()
                }
            }
            Phase::Paused { .. }
            | Phase::Playing { .. }
            | Phase::Ended { .. }
            | Phase::DeviceLost { .. } => Effects::none(),
        },

        Event::DeviceLost { reason, now } => {
            if phase.is_lost() {
                return Effects::none();
            }
            let final_heard = phase.heard_at(now, sample_rate);
            *phase = Phase::DeviceLost {
                heard: final_heard,
                reason: reason.clone(),
            };
            Effects::two(Effect::ClearBuffer, Effect::Emit(AudioEvent::DeviceLost(reason)))
        }
    }
}

fn pause_pending(
    existing: Option<PendingDevice>,
    segment_base: u64,
    written: u64,
    snapped_heard: u64,
) -> Option<PendingDevice> {
    if written > 0 {
        Some(PendingDevice::PauseRewind {
            total_written: segment_base.saturating_add(written),
            heard_floor: snapped_heard,
        })
    } else {
        existing
    }
}

/// Converts a frame count at `sample_rate` Hz into an exact `Duration` using integer math.
pub(crate) fn frames_to_duration(frames: u64, sample_rate: u32) -> Duration {
    if sample_rate == 0 || frames == 0 {
        return Duration::ZERO;
    }
    let rate = u128::from(sample_rate);
    let f = u128::from(frames);
    let secs = f / rate;
    let rem = f % rate;
    let nanos = (rem * 1_000_000_000) / rate;
    Duration::new(secs.min(u128::from(u64::MAX)) as u64, nanos as u32)
}

/// Converts a `Duration` into whole frames at `sample_rate` Hz using integer math.
pub(crate) fn duration_to_frames(duration: Duration, sample_rate: u32) -> u64 {
    if sample_rate == 0 || duration.is_zero() {
        return 0;
    }
    let rate = u128::from(sample_rate);
    let whole = u128::from(duration.as_secs()).saturating_mul(rate);
    let sub = (u128::from(duration.subsec_nanos()).saturating_mul(rate)) / 1_000_000_000;
    whole.saturating_add(sub).min(u128::from(u64::MAX)) as u64
}
