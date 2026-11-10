//! The recorder's state machine. Each phase holds exactly what is valid in it (the device
//! session, the fake input and its clock anchor, what an interruption saved), and `step` turns
//! an event into the next phase and the effects for [`Shared`](super::Shared) to carry out.
//! An answer from the device or a permission prompt carries the [`Attempt`] that asked for it,
//! so one for an attempt the phase no longer holds changes nothing.

use super::fake::{self, Input};
use super::meter::{Level, Meter};
use super::{RecorderState, Recording, backend};
use crate::machine::{Attempt, Effects, Seq};
use std::path::PathBuf;

/// A recording from a [`FakeMicrophone`](super::FakeMicrophone).
pub(super) struct FakeSession {
    input: Input,
    path: PathBuf,
    meter: Meter,
    /// Samples folded into the meter.
    metered: u64,
    unplug: Option<f64>,
}

pub(super) enum Source {
    Device(backend::Session),
    Fake(FakeSession),
}

pub(super) enum Phase {
    Idle,
    /// Checking or asking for microphone access: nothing is open yet.
    Asking { attempt: Attempt },
    /// The device is opening its input.
    Opening { attempt: Attempt, session: backend::Session },
    Recording {
        attempt: Attempt,
        source: Source,
        /// (clock, elapsed) when recording last (re)started; set by the first clock reading.
        anchor: Option<(f64, f64)>,
    },
    Paused { attempt: Attempt, source: Source },
    /// `stop` is waiting for the device to finish its file. Reads as the phase it left.
    Stopping { attempt: Attempt, paused: bool },
    /// The input went away; `saved` is what it captured.
    Interrupted { attempt: Attempt, reason: String, saved: Recording },
    Stopped,
    Failed(String),
}

pub(super) struct Control {
    pub phase: Phase,
    pub seq: Seq,
    /// Bumped whenever the public state changes, for the app's waker.
    pub generation: u64,
    /// Seconds recorded, pauses excluded: held while paused and after stopping.
    pub elapsed: f64,
    /// Elapsed follows the headless clock of the starting thread, not the wall clock.
    pub virtual_clock: bool,
    /// The file the current attempt writes.
    pub path: PathBuf,
}

pub(super) enum Event {
    Start { path: PathBuf, virtual_clock: bool },
    /// Microphone access for `attempt`: granted, or why not.
    Permission { attempt: Attempt, granted: Result<(), String> },
    FakeOpened { attempt: Attempt, opened: Result<FakeSession, String> },
    DeviceOpened { attempt: Attempt, opened: Result<backend::Session, String> },
    /// The device started capturing.
    Began { attempt: Attempt },
    /// The device couldn't open.
    Failed { attempt: Attempt, error: String },
    /// The device went away after `duration` seconds of audio.
    Interrupted { attempt: Attempt, reason: String, duration: f64 },
    Metered { attempt: Attempt, level: Level },
    /// Catch up to the clock: elapsed time, and a fake input's levels and unplugging.
    Tick,
    Pause,
    Resume,
    Stop,
    /// The device finished its file after `Effect::StopDevice`.
    Stopped { attempt: Attempt, result: Result<f64, String> },
    Cancel,
}

pub(super) enum Effect {
    /// Publish a silent level.
    Silence,
    Publish(Level),
    /// Check (or ask for) microphone access, then answer with `Event::Permission`.
    Ask { attempt: Attempt },
    /// Open the input: a fake one, answering with `Event::FakeOpened`, or the device,
    /// answering with `Event::DeviceOpened` before anything else steps.
    Open { attempt: Attempt },
    /// Catch this recorder up on every headless frame of the current thread.
    FollowHeadless,
    PauseDevice,
    ResumeDevice,
    /// Finish the device's file (blocking), answering with `Event::Stopped`.
    StopDevice { attempt: Attempt, session: backend::Session },
    /// Stop the device and delete its file.
    CancelDevice(backend::Session),
    RemoveFile(PathBuf),
    /// What `stop` returns.
    Reply(Result<Recording, String>),
}

/// Clock sums drift by a few ulps (60 steps of 1/60 s fall just short of 1 s): treat elapsed
/// time this close to a boundary as on it, so frame rate never moves a sample or an unplug.
const CLOCK_SLACK: f64 = 1e-9;

/// Whole samples recorded after `seconds` at `rate`.
pub(super) fn frames_at(seconds: f64, rate: u32) -> u64 {
    ((seconds + CLOCK_SLACK) * rate as f64).floor().max(0.0) as u64
}

impl FakeSession {
    pub(super) fn new(input: Input, path: PathBuf, unplug: Option<f64>) -> Self {
        Self { input, path, meter: Meter::default(), metered: 0, unplug }
    }

    /// When the input goes away, if `elapsed` has reached it.
    fn unplugged(&self, elapsed: f64) -> Option<f64> {
        self.unplug.filter(|t| elapsed + CLOCK_SLACK >= *t)
    }

    /// Writes the first `seconds` of the input, stopping where it was unplugged.
    pub(super) fn write(self, seconds: f64) -> Result<Recording, String> {
        let seconds = self.unplug.map_or(seconds, |t| seconds.min(t));
        let frames = frames_at(seconds, self.input.rate);
        fake::write_wav(&self.path, &self.input, frames)?;
        Ok(Recording { path: self.path, duration: frames as f64 / self.input.rate as f64 })
    }

    /// Folds the samples up to `elapsed` into the meter, in 10 ms blocks.
    fn meter_to(&mut self, elapsed: f64) -> Level {
        let rate = self.input.rate;
        let target = frames_at(elapsed, rate);
        let block = (rate / 100).max(1) as u64;
        // Only the last few seconds still show in the meter.
        self.metered = self.metered.max(target.saturating_sub(10 * rate as u64) / block * block);
        while self.metered + block <= target {
            let input = &self.input;
            self.meter.push_samples((self.metered..self.metered + block).map(|i| input.sample(i)), rate);
            self.metered += block;
        }
        self.meter.level()
    }
}

/// Elapsed follows the clock from `anchor`. A clock behind the anchor (a new runtime on this
/// thread) restarts from here.
pub(super) fn advance(elapsed: &mut f64, anchor: &mut Option<(f64, f64)>, now: Option<f64>) {
    let Some(now) = now else { return };
    match *anchor {
        Some((clock, at)) if clock <= now => *elapsed = at + (now - clock),
        _ => *anchor = Some((now, *elapsed)),
    }
}

pub(super) const NOT_RECORDING: &str = "not recording";

impl Control {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { phase: Phase::Idle, seq: Seq::default(), generation: 0, elapsed: 0.0, virtual_clock: false, path }
    }

    pub(super) fn status(&self) -> RecorderState {
        match &self.phase {
            Phase::Idle => RecorderState::Idle,
            Phase::Asking { .. } | Phase::Opening { .. } => RecorderState::Starting,
            Phase::Recording { .. } | Phase::Stopping { paused: false, .. } => RecorderState::Recording,
            Phase::Paused { .. } | Phase::Stopping { paused: true, .. } => RecorderState::Paused,
            Phase::Interrupted { reason, .. } => RecorderState::Interrupted(reason.clone()),
            Phase::Stopped => RecorderState::Stopped,
            Phase::Failed(e) => RecorderState::Failed(e.clone()),
        }
    }

    /// Recording or paused from a fake microphone.
    pub(super) fn is_fake(&self) -> bool {
        matches!(&self.phase, Phase::Recording { source: Source::Fake(_), .. } | Phase::Paused { source: Source::Fake(_), .. })
    }

    /// The device recording or paused now.
    pub(super) fn device(&self) -> Option<&backend::Session> {
        match &self.phase {
            Phase::Recording { source: Source::Device(s), .. } | Phase::Paused { source: Source::Device(s), .. } => Some(s),
            _ => None,
        }
    }

    /// A new runtime on this thread starts its clock at 0: a recording goes on from there.
    pub(super) fn restart_clock(&mut self) {
        if let Phase::Recording { anchor: anchor @ Some(_), .. } = &mut self.phase {
            *anchor = Some((0.0, self.elapsed));
        }
    }

    /// One transition, at clock reading `now` (`None` when this thread's headless clock isn't
    /// running).
    pub(super) fn step(&mut self, event: Event, now: Option<f64>) -> Effects<Effect> {
        let before = self.status();
        let effects = self.transition(event, now);
        if self.status() != before {
            self.generation += 1;
        }
        effects
    }

    fn transition(&mut self, event: Event, now: Option<f64>) -> Effects<Effect> {
        use Effect::*;
        let phase = std::mem::replace(&mut self.phase, Phase::Idle);
        let (phase, effects) = match (phase, event) {
            (
                phase @ (Phase::Asking { .. } | Phase::Opening { .. } | Phase::Recording { .. } | Phase::Paused { .. } | Phase::Stopping { .. }),
                Event::Start { .. },
            ) => (phase, Effects::none()),
            (_, Event::Start { path, virtual_clock }) => {
                let attempt = self.seq.next();
                (self.elapsed, self.path, self.virtual_clock) = (0.0, path, virtual_clock);
                (Phase::Asking { attempt }, Effects::two(Silence, Ask { attempt }))
            }

            (Phase::Asking { attempt }, Event::Permission { attempt: a, granted }) if a == attempt => match granted {
                Ok(()) => (Phase::Asking { attempt }, Effects::one(Open { attempt })),
                Err(e) => (Phase::Failed(e), Effects::none()),
            },
            (Phase::Asking { attempt }, Event::FakeOpened { attempt: a, opened }) if a == attempt => match opened {
                Ok(fake) => {
                    let anchor = now.map(|now| (now, 0.0));
                    let effects = if self.virtual_clock { Effects::one(FollowHeadless) } else { Effects::none() };
                    (Phase::Recording { attempt, source: Source::Fake(fake), anchor }, effects)
                }
                Err(e) => (Phase::Failed(e), Effects::none()),
            },
            (Phase::Asking { attempt }, Event::DeviceOpened { attempt: a, opened }) if a == attempt => match opened {
                Ok(session) => (Phase::Opening { attempt, session }, Effects::none()),
                Err(e) => (Phase::Failed(e), Effects::none()),
            },
            // An input opened for an attempt that's gone records for nobody.
            (phase, Event::DeviceOpened { opened: Ok(session), .. }) => (phase, Effects::one(CancelDevice(session))),
            (Phase::Opening { attempt, session }, Event::Began { attempt: a }) if a == attempt => {
                (Phase::Recording { attempt, source: Source::Device(session), anchor: now.map(|now| (now, 0.0)) }, Effects::none())
            }
            (Phase::Asking { attempt } | Phase::Opening { attempt, .. }, Event::Failed { attempt: a, error }) if a == attempt => {
                (Phase::Failed(error), Effects::none())
            }

            (Phase::Recording { attempt, source, mut anchor }, Event::Interrupted { attempt: a, reason, duration }) if a == attempt => {
                advance(&mut self.elapsed, &mut anchor, now);
                drop(source);
                let saved = Recording { path: self.path.clone(), duration };
                (Phase::Interrupted { attempt, reason, saved }, Effects::one(Silence))
            }
            (Phase::Paused { attempt, .. } | Phase::Stopping { attempt, .. }, Event::Interrupted { attempt: a, reason, duration }) if a == attempt => {
                let saved = Recording { path: self.path.clone(), duration };
                (Phase::Interrupted { attempt, reason, saved }, Effects::one(Silence))
            }
            (phase @ Phase::Recording { attempt, source: Source::Device(_), .. }, Event::Metered { attempt: a, level }) if a == attempt => {
                (phase, Effects::one(Publish(level)))
            }

            (Phase::Recording { attempt, source, mut anchor }, Event::Tick) => {
                advance(&mut self.elapsed, &mut anchor, now);
                self.catch_up(attempt, source, anchor)
            }
            (Phase::Recording { attempt, source, mut anchor }, Event::Pause) => {
                advance(&mut self.elapsed, &mut anchor, now);
                match self.catch_up(attempt, source, anchor) {
                    (Phase::Recording { attempt, source, .. }, mut effects) => {
                        effects.push(Silence);
                        if matches!(source, Source::Device(_)) {
                            effects.push(PauseDevice);
                        }
                        (Phase::Paused { attempt, source }, effects)
                    }
                    other => other,
                }
            }
            (Phase::Paused { attempt, source }, Event::Resume) => {
                let effects = if matches!(source, Source::Device(_)) { Effects::one(ResumeDevice) } else { Effects::none() };
                (Phase::Recording { attempt, source, anchor: now.map(|now| (now, self.elapsed)) }, effects)
            }

            (Phase::Recording { attempt, source, mut anchor }, Event::Stop) => {
                advance(&mut self.elapsed, &mut anchor, now);
                let (phase, mut effects) = self.catch_up(attempt, source, anchor);
                let (phase, more) = self.stop(phase);
                effects.extend(more);
                (phase, effects)
            }
            (phase, Event::Stop) => self.stop(phase),

            (Phase::Stopping { attempt, .. }, Event::Stopped { attempt: a, result }) if a == attempt => {
                let result = result.map(|duration| Recording { path: self.path.clone(), duration });
                let phase = match &result {
                    Ok(_) => Phase::Stopped,
                    Err(e) => Phase::Failed(e.clone()),
                };
                (phase, Effects::two(Silence, Reply(result)))
            }
            // The device went away while it finished: what it saved is the recording.
            (Phase::Interrupted { attempt, saved, .. }, Event::Stopped { attempt: a, .. }) if a == attempt => {
                (Phase::Stopped, Effects::two(Silence, Reply(Ok(saved))))
            }

            (Phase::Asking { .. }, Event::Cancel) => (Phase::Idle, Effects::one(Silence)),
            (Phase::Opening { session, .. }, Event::Cancel)
            | (Phase::Recording { source: Source::Device(session), .. } | Phase::Paused { source: Source::Device(session), .. }, Event::Cancel) => {
                (Phase::Idle, Effects::two(Silence, CancelDevice(session)))
            }
            (Phase::Recording { source: Source::Fake(fake), .. } | Phase::Paused { source: Source::Fake(fake), .. }, Event::Cancel) => {
                (Phase::Idle, Effects::two(Silence, RemoveFile(fake.path)))
            }
            (Phase::Interrupted { saved, .. }, Event::Cancel) => (Phase::Idle, Effects::two(Silence, RemoveFile(saved.path))),

            (phase, _) => (phase, Effects::none()),
        };
        self.phase = phase;
        effects
    }

    /// `stop` from `phase`, already caught up to the clock.
    fn stop(&mut self, phase: Phase) -> (Phase, Effects<Effect>) {
        use Effect::*;
        let (attempt, source, paused) = match phase {
            Phase::Recording { attempt, source, .. } => (attempt, source, false),
            Phase::Paused { attempt, source } => (attempt, source, true),
            Phase::Interrupted { saved, .. } => return (Phase::Stopped, Effects::one(Reply(Ok(saved)))),
            Phase::Asking { .. } => return (Phase::Idle, Effects::one(Reply(Err(STOPPED_EARLY.into())))),
            Phase::Opening { session, .. } => return (Phase::Idle, Effects::two(CancelDevice(session), Reply(Err(STOPPED_EARLY.into())))),
            Phase::Failed(e) => return (Phase::Failed(e.clone()), Effects::one(Reply(Err(e)))),
            phase @ (Phase::Idle | Phase::Stopped | Phase::Stopping { .. }) => return (phase, Effects::one(Reply(Err(NOT_RECORDING.into())))),
        };
        match source {
            Source::Fake(fake) => {
                let result = fake.write(self.elapsed);
                let phase = match &result {
                    Ok(_) => Phase::Stopped,
                    Err(e) => Phase::Failed(e.clone()),
                };
                (phase, Effects::two(Silence, Reply(result)))
            }
            Source::Device(session) => (Phase::Stopping { attempt, paused }, Effects::one(StopDevice { attempt, session })),
        }
    }

    /// A recording caught up to `self.elapsed`: a fake input meters its samples, or goes away
    /// once it reaches its unplug time.
    fn catch_up(&mut self, attempt: Attempt, source: Source, anchor: Option<(f64, f64)>) -> (Phase, Effects<Effect>) {
        let mut fake = match source {
            Source::Fake(fake) => fake,
            Source::Device(session) => return (Phase::Recording { attempt, source: Source::Device(session), anchor }, Effects::none()),
        };
        if let Some(unplug) = fake.unplugged(self.elapsed) {
            self.elapsed = unplug;
            let phase = match fake.write(unplug) {
                Ok(saved) => Phase::Interrupted { attempt, reason: "the microphone went away".into(), saved },
                Err(e) => Phase::Failed(e),
            };
            return (phase, Effects::one(Effect::Silence));
        }
        let level = fake.meter_to(self.elapsed);
        (Phase::Recording { attempt, source: Source::Fake(fake), anchor }, Effects::one(Effect::Publish(level)))
    }
}

const STOPPED_EARLY: &str = "stopped before recording started";

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
