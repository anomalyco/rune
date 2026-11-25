//! Background device driver (native worker thread on `Condvar`, inline clock-driven step on
//! `wasm32`) bridging [`AudioOutput`](super::AudioOutput), [`PcmBuffer`](super::buffer::PcmBuffer),
//! [`machine::step`](super::machine::step), and [`SinkDevice`](super::SinkDevice).

use super::buffer::{PcmBuffer, quantum_frames};
use super::machine::{Effect, Event, PendingDevice, Phase, step};
use super::{
    AudioBackend, AudioEvent, AudioOutputError, AudioPushError, ClockFn, DRAIN_POLL_INTERVAL,
    MAX_EVENTS, SampleFormat, SinkDevice, SinkError, ValidatedSpec,
};
use crate::video::WakerSlot;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Condvar;
#[cfg(not(target_arch = "wasm32"))]
use std::thread::JoinHandle;

pub(super) struct Inner {
    pub(super) phase: Phase,
    pub(super) buffer: PcmBuffer,
    pub(super) volume: f32,
    pub(super) muted: bool,
    /// Monotonic counter incremented on every `flush()` so an in-flight `device.write` started
    /// before the flush does not commit stale samples into the cleared buffer.
    pub(super) flush_seq: u64,
    pub(super) events: VecDeque<AudioEvent>,
    pub(super) event_generation: u64,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) stop: bool,
}

impl Inner {
    pub(super) fn push_event(&mut self, event: AudioEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
        self.event_generation = self.event_generation.wrapping_add(1).max(1);
    }
}

#[cfg(target_arch = "wasm32")]
struct InlineDriver {
    device: Box<dyn SinkDevice>,
    scratch: Box<[u8]>,
}

pub(super) struct Shared {
    pub(super) format: SampleFormat,
    pub(super) sample_rate: u32,
    pub(super) channels: u16,
    pub(super) frame_bytes: usize,
    pub(super) capacity_frames: usize,
    pub(super) quantum_frames: usize,
    pub(super) backend: AudioBackend,
    pub(super) clock: ClockFn,
    pub(super) inner: Mutex<Inner>,
    pub(super) waker: WakerSlot,
    #[cfg(not(target_arch = "wasm32"))]
    condvar: Condvar,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Mutex<Option<JoinHandle<()>>>,
    #[cfg(target_arch = "wasm32")]
    inline: Mutex<InlineDriver>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Shared {
    fn drop(&mut self) {
        {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            g.stop = true;
        }
        self.condvar.notify_all();
        let handle = self.worker.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(handle) = handle {
            // Best-effort join so the worker exits and releases its `SinkDevice` before drop ends.
            handle.join().ok();
        }
    }
}

pub(super) struct StepOutcome {
    pub(super) wake_worker: bool,
    pub(super) tell_waker: Option<u64>,
}

impl Shared {
    pub(super) fn new(
        spec: ValidatedSpec,
        device: Box<dyn SinkDevice>,
        clock: ClockFn,
    ) -> Result<Arc<Self>, AudioOutputError> {
        let backend = device.backend();
        let q_frames = quantum_frames(spec.sample_rate);
        let buffer = PcmBuffer::new(spec.format, spec.sample_rate, spec.channels, spec.capacity_frames);
        let waker = WakerSlot::default();
        if let Some(w) = spec.waker {
            waker.set(w);
        }
        let inner = Inner {
            phase: Phase::default(),
            buffer,
            volume: spec.volume,
            muted: spec.muted,
            flush_seq: 0,
            events: VecDeque::new(),
            event_generation: 0,
            #[cfg(not(target_arch = "wasm32"))]
            stop: false,
        };

        #[cfg(target_arch = "wasm32")]
        {
            let scratch = vec![0u8; q_frames * spec.frame_bytes].into_boxed_slice();
            Ok(Arc::new(Self {
                format: spec.format,
                sample_rate: spec.sample_rate,
                channels: spec.channels,
                frame_bytes: spec.frame_bytes,
                capacity_frames: spec.capacity_frames,
                quantum_frames: q_frames,
                backend,
                clock,
                inner: Mutex::new(inner),
                waker,
                inline: Mutex::new(InlineDriver { device, scratch }),
            }))
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let shared = Arc::new(Self {
                format: spec.format,
                sample_rate: spec.sample_rate,
                channels: spec.channels,
                frame_bytes: spec.frame_bytes,
                capacity_frames: spec.capacity_frames,
                quantum_frames: q_frames,
                backend,
                clock,
                inner: Mutex::new(inner),
                waker,
                condvar: Condvar::new(),
                worker: Mutex::new(None),
            });
            let worker_shared = Arc::downgrade(&shared);
            let scratch_bytes = q_frames * spec.frame_bytes;
            let handle = std::thread::Builder::new()
                .name("rune-audio-output".into())
                .spawn(move || run_worker(worker_shared, device, scratch_bytes))
                .map_err(|e| AudioOutputError::OpenFailed(format!("couldn't spawn audio worker: {e}")))?;
            *shared.worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
            Ok(shared)
        }
    }

    pub(super) fn push_with(
        &self,
        check: impl FnOnce() -> Result<(), AudioPushError>,
        push: impl FnOnce(&mut PcmBuffer) -> Result<usize, AudioPushError>,
    ) -> Result<usize, AudioPushError> {
        self.sync_inline();
        check()?;
        let (result, wake_worker) = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if g.phase.is_lost() {
                return Err(AudioPushError::DeviceLost);
            }
            if g.phase.rejects_push_draining() {
                return Err(AudioPushError::Draining);
            }
            let accepted = push(&mut g.buffer)?;
            let wake = if accepted > 0 {
                apply_event(&mut g, self.sample_rate, Event::Pushed).wake_worker
            } else {
                false
            };
            (Ok(accepted), wake)
        };
        if wake_worker {
            self.notify_worker();
        }
        self.sync_inline();
        result
    }

    pub(super) fn finish_outcome(&self, outcome: StepOutcome) {
        if outcome.wake_worker {
            self.notify_worker();
        }
        if let Some(gen_id) = outcome.tell_waker {
            self.waker.tell(gen_id);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn notify_worker(&self) {
        self.condvar.notify_all();
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn notify_worker(&self) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn sync_inline(&self) {}

    #[cfg(target_arch = "wasm32")]
    pub(super) fn sync_inline(&self) {
        let mut driver = self.inline.lock().unwrap_or_else(PoisonError::into_inner);
        let InlineDriver { device, scratch } = &mut *driver;
        // Bound inline iterations so a call never loops indefinitely even if the clock is frozen.
        for _ in 0..64 {
            match step_device_once(self, device.as_mut(), scratch) {
                WorkerAction::Continue => {}
                WorkerAction::Wait | WorkerAction::Park | WorkerAction::Exit => break,
            }
        }
    }
}

pub(super) fn apply_event(g: &mut Inner, sample_rate: u32, event: Event) -> StepOutcome {
    let mut wake_worker = false;
    let mut tell_waker = None;
    for effect in step(&mut g.phase, sample_rate, event) {
        match effect {
            Effect::WakeWorker => wake_worker = true,
            Effect::ClearBuffer => g.buffer.clear(),
            Effect::RewindTail(unheard) => {
                let _ = g.buffer.rewind_tail(unheard);
            }
            Effect::Emit(audio_event) => {
                g.push_event(audio_event);
                tell_waker = Some(g.event_generation);
            }
        }
    }
    StepOutcome { wake_worker, tell_waker }
}

enum WorkerAction {
    Continue,
    #[cfg(not(target_arch = "wasm32"))]
    Wait(Duration),
    #[cfg(target_arch = "wasm32")]
    Wait,
    Park,
    Exit,
}

impl WorkerAction {
    fn wait(delay: Duration) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::Wait(delay)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = delay;
            Self::Wait
        }
    }
}

fn step_device_once(shared: &Shared, device: &mut dyn SinkDevice, scratch: &mut [u8]) -> WorkerAction {
    let (pending, is_draining, is_playing, started, underrun, frames, flush_seq) = {
        let g = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        #[cfg(not(target_arch = "wasm32"))]
        if g.stop {
            return WorkerAction::Exit;
        }
        if g.phase.is_lost() {
            return WorkerAction::Exit;
        }
        let pending = g.phase.pending_device();
        if pending.is_some() {
            (pending, false, false, false, false, 0, g.flush_seq)
        } else {
            match g.phase {
                Phase::Paused { .. } | Phase::Ended { .. } => return WorkerAction::Park,
                Phase::DeviceLost { .. } => return WorkerAction::Exit,
                Phase::Playing { started, underrun, .. } => {
                    let frames = g.buffer.peek_scaled(shared.quantum_frames, g.volume, g.muted, scratch);
                    (None, false, true, started, underrun, frames, g.flush_seq)
                }
                Phase::Draining { .. } => {
                    let frames = g.buffer.peek_scaled(shared.quantum_frames, g.volume, g.muted, scratch);
                    (None, true, false, true, false, frames, g.flush_seq)
                }
            }
        }
    };

    if let Some(pending) = pending {
        return handle_pending_device(shared, device, pending);
    }

    if frames > 0 {
        return write_quantum(shared, device, scratch, frames, flush_seq);
    }

    // App buffer is empty.
    if is_playing {
        if !started || underrun {
            return WorkerAction::Park;
        }
        return poll_empty(shared, device, false);
    }
    if is_draining {
        return poll_empty(shared, device, true);
    }
    WorkerAction::Park
}

fn write_quantum(
    shared: &Shared,
    device: &mut dyn SinkDevice,
    scratch: &[u8],
    frames: usize,
    flush_seq: u64,
) -> WorkerAction {
    let bytes = frames * shared.frame_bytes;
    let write_res = device.write(&scratch[..bytes]);
    let latency_res = if write_res.is_ok() { Some(device.latency_frames()) } else { None };
    let pace = device.pace();
    let now = (shared.clock)();
    let (outcome, has_pending, accepted_zero, lost) = {
        let mut g = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let accepted_zero = matches!(write_res, Ok(0));
        let lost = matches!(write_res, Err(SinkError::Lost(_)));
        let outcome = match write_res {
            Ok(accepted) => {
                let accepted = accepted.min(frames);
                if g.flush_seq == flush_seq {
                    if accepted > 0 {
                        g.buffer.commit_read(accepted);
                    }
                    match &mut g.phase {
                        Phase::Paused { heard, pending: pending @ None, .. } if accepted > 0 => {
                            *pending = Some(PendingDevice::PauseRewind {
                                total_written: heard.saturating_add(accepted as u64),
                                heard_floor: *heard,
                            });
                        }
                        Phase::Paused { pending: Some(PendingDevice::PauseRewind { total_written, .. }), .. }
                        | Phase::Playing { pending: Some(PendingDevice::PauseRewind { total_written, .. }), .. }
                        | Phase::Draining { pending: Some(PendingDevice::PauseRewind { total_written, .. }), .. } => {
                            *total_written = total_written.saturating_add(accepted as u64);
                        }
                        _ => {}
                    }
                    match latency_res {
                        Some(Ok(latency)) => apply_event(
                            &mut g,
                            shared.sample_rate,
                            Event::Wrote { frames: accepted as u64, latency, now },
                        ),
                        Some(Err(SinkError::Underrun)) => {
                            apply_event(&mut g, shared.sample_rate, Event::DeviceUnderrun { now })
                        }
                        Some(Err(SinkError::Lost(reason))) => {
                            apply_event(&mut g, shared.sample_rate, Event::DeviceLost { reason, now })
                        }
                        None => StepOutcome { wake_worker: false, tell_waker: None },
                    }
                } else {
                    StepOutcome { wake_worker: false, tell_waker: None }
                }
            }
            Err(SinkError::Underrun) => {
                if g.flush_seq == flush_seq {
                    apply_event(&mut g, shared.sample_rate, Event::DeviceUnderrun { now })
                } else {
                    StepOutcome { wake_worker: false, tell_waker: None }
                }
            }
            Err(SinkError::Lost(reason)) => {
                apply_event(&mut g, shared.sample_rate, Event::DeviceLost { reason, now })
            }
        };
        let has_pending = g.phase.pending_device().is_some();
        let lost = lost || g.phase.is_lost();
        (outcome, has_pending, accepted_zero, lost)
    };
    if let Some(gen_id) = outcome.tell_waker {
        shared.waker.tell(gen_id);
    }
    if lost {
        WorkerAction::Exit
    } else if has_pending {
        WorkerAction::Continue
    } else if accepted_zero {
        WorkerAction::wait(pace.max(Duration::from_millis(1)))
    } else if !pace.is_zero() {
        WorkerAction::wait(pace)
    } else {
        WorkerAction::Continue
    }
}

fn poll_empty(shared: &Shared, device: &mut dyn SinkDevice, draining: bool) -> WorkerAction {
    let lat_res = device.latency_frames();
    let now = (shared.clock)();
    let (tell_waker, next_action) = {
        let mut g = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let outcome = match lat_res {
            Ok(latency) if draining => apply_event(&mut g, shared.sample_rate, Event::DrainPoll { latency, now }),
            Ok(latency) => apply_event(&mut g, shared.sample_rate, Event::DryPoll { latency, now }),
            Err(SinkError::Underrun) => apply_event(&mut g, shared.sample_rate, Event::DeviceUnderrun { now }),
            Err(SinkError::Lost(reason)) => apply_event(&mut g, shared.sample_rate, Event::DeviceLost { reason, now }),
        };
        let action = match g.phase {
            Phase::DeviceLost { .. } => WorkerAction::Exit,
            Phase::Playing { underrun: true, .. } | Phase::Paused { .. } | Phase::Ended { .. } => WorkerAction::Park,
            _ => WorkerAction::wait(DRAIN_POLL_INTERVAL),
        };
        (outcome.tell_waker, action)
    };
    if let Some(gen_id) = tell_waker {
        shared.waker.tell(gen_id);
    }
    next_action
}

fn handle_pending_device(shared: &Shared, device: &mut dyn SinkDevice, pending: PendingDevice) -> WorkerAction {
    match pending {
        PendingDevice::FlushOnly => {
            let res = device.flush();
            let now = (shared.clock)();
            let (tell_waker, lost) = {
                let mut g = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
                let outcome = match res {
                    Ok(()) | Err(SinkError::Underrun) => apply_event(&mut g, shared.sample_rate, Event::DeviceFlushed),
                    Err(SinkError::Lost(reason)) => apply_event(&mut g, shared.sample_rate, Event::DeviceLost { reason, now }),
                };
                (outcome.tell_waker, g.phase.is_lost())
            };
            if let Some(gen_id) = tell_waker {
                shared.waker.tell(gen_id);
            }
            if lost { WorkerAction::Exit } else { WorkerAction::Continue }
        }
        PendingDevice::PauseRewind { .. } => {
            let lat_res = device.latency_frames();
            let flush_res = match &lat_res {
                Err(SinkError::Lost(_)) => Ok(()),
                _ => device.flush(),
            };
            let now = (shared.clock)();
            let (tell_waker, lost) = {
                let mut g = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
                let outcome = match (lat_res, flush_res) {
                    (Err(SinkError::Lost(reason)), _) | (_, Err(SinkError::Lost(reason))) => {
                        apply_event(&mut g, shared.sample_rate, Event::DeviceLost { reason, now })
                    }
                    (Ok(latency), _) => apply_event(&mut g, shared.sample_rate, Event::PauseFlushed { latency }),
                    (Err(SinkError::Underrun), _) => apply_event(&mut g, shared.sample_rate, Event::PauseFlushed { latency: 0 }),
                };
                (outcome.tell_waker, g.phase.is_lost())
            };
            if let Some(gen_id) = tell_waker {
                shared.waker.tell(gen_id);
            }
            if lost { WorkerAction::Exit } else { WorkerAction::Continue }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_worker(weak: std::sync::Weak<Shared>, mut device: Box<dyn SinkDevice>, scratch_bytes: usize) {
    let mut scratch = vec![0u8; scratch_bytes].into_boxed_slice();
    loop {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        match step_device_once(&shared, device.as_mut(), &mut scratch) {
            WorkerAction::Continue => {}
            WorkerAction::Wait(delay) => {
                let g = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
                if g.stop || g.phase.is_lost() {
                    return;
                }
                if g.phase.pending_device().is_some() {
                    continue;
                }
                drop(shared.condvar.wait_timeout(g, delay).unwrap_or_else(PoisonError::into_inner));
            }
            WorkerAction::Park => {
                let g = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
                if g.stop || g.phase.is_lost() {
                    return;
                }
                if g.phase.pending_device().is_some() {
                    continue;
                }
                let still_idle = match g.phase {
                    Phase::Paused { .. } | Phase::Ended { .. } => true,
                    Phase::Playing { started, underrun, .. } => g.buffer.is_empty() && (!started || underrun),
                    Phase::Draining { .. } => false,
                    Phase::DeviceLost { .. } => return,
                };
                if still_idle {
                    drop(shared.condvar.wait(g).unwrap_or_else(PoisonError::into_inner));
                }
            }
            WorkerAction::Exit => return,
        }
    }
}
