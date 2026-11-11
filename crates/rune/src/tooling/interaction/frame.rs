use super::{Duration, Instant, Note, PHASES, Stacks, Timed};
use crate::machine::Effects;

#[derive(Default)]
pub(super) enum Frame {
    #[default]
    Idle,
    Begun { at: Instant, queued: bool, cpu: Option<f64>, why: u16 },
    Open { timed: Timed, input_start: Option<Instant>, cpu: Option<f64> },
    Built(Built),
}

pub(super) struct Built {
    pub timed: Timed,
    pub at: Instant,
    pub phases: [f64; PHASES],
    pub shaped: u32,
    pub cpu_ms: Option<f64>,
    pub stacks: Option<Stacks>,
}

pub(super) enum Event {
    Begin { at: Instant, queued: bool, cpu: Option<f64> },
    Note { note: Note, at: Option<Instant>, started: Instant, handler: Duration },
    Commit(u16),
    BuildUntimed,
    BuildTimed { at: Instant, phases: [f64; PHASES], shaped: u32, cpu: Option<f64>, stacks: Option<Stacks> },
    Finish,
}

pub(super) enum Effect { StartPhases, StopPhases, CheckFrame { start: Instant, why: u16 }, Count(Built) }

#[inline]
pub(super) fn step(frame: &mut Frame, event: Event) -> Effects<Effect> {
    match event {
        Event::Begin { at, queued, cpu } => match frame {
            // A second begin without a build leaves the first interaction open.
            Frame::Open { input_start, .. } => *input_start = queued.then_some(at),
            _ => *frame = Frame::Begun { at, queued, cpu, why: 0 },
        },
        Event::Note { note, at, started, handler } => {
            let handler_ms = handler.as_secs_f64() * 1000.0;
            if let Frame::Open { timed, input_start, .. } = frame {
                let start = *input_start.get_or_insert(started);
                timed.arrived = timed.arrived.min(at.unwrap_or(start).min(start));
                timed.handler_ms += handler_ms;
                if note.rank > timed.note.rank { timed.note = note; }
            } else {
                let (frame_start, cpu) = match *frame {
                    Frame::Begun { at, queued, cpu, .. } => (if queued { at } else { started }, cpu),
                    _ => (started, None),
                };
                let arrived = at.unwrap_or(frame_start).min(frame_start);
                let timed = Timed { arrived, frame_start, handler_ms, note, why: 0, commits: 0 };
                *frame = Frame::Open { timed, input_start: Some(frame_start), cpu };
                return Effects::one(Effect::StartPhases);
            }
        }
        Event::Commit(bits) => match frame {
            Frame::Begun { why, .. } => *why |= bits,
            Frame::Open { timed, .. } => { timed.why |= bits; timed.commits += 1; }
            _ => (),
        },
        Event::BuildUntimed => if let Frame::Begun { at: start, why, .. } = *frame {
            *frame = Frame::Idle;
            return Effects::one(Effect::CheckFrame { start, why });
        },
        Event::BuildTimed { at, phases, shaped, cpu, stacks } => {
            let Frame::Open { timed, cpu: from, .. } = std::mem::take(frame) else { unreachable!() };
            let cpu_ms = from.zip(cpu).map(|(from, to)| to - from);
            *frame = Frame::Built(Built { timed, at, phases, shaped, cpu_ms, stacks });
            return Effects::one(Effect::StopPhases);
        }
        Event::Finish => if matches!(frame, Frame::Built(_)) {
            let Frame::Built(built) = std::mem::take(frame) else { unreachable!() };
            return Effects::one(Effect::Count(built));
        },
    }
    Effects::none()
}
