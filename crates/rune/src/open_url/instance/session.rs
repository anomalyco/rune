//! Connection, framing and deadline state machine for the Windows single-instance pipe server.

use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::ERROR_OPERATION_ABORTED;

use crate::machine::{Attempt, Effects, Seq};

use super::{ACCEPTED, ANSWER_WITHIN, MAGIC, MAGIC_V1, MAX_MESSAGE, MAX_V1_WIRE, QUITTING};

/// Maximum bytes requested per overlapped pipe read.
pub(super) const READ_CHUNK: usize = 4096;
/// Byte length of the v2 header (`MAGIC` plus little-endian `u32` body length).
const V2_HEAD: usize = MAGIC.len() + 4;

/// A launch parsed from a client: its arguments, and whether the client waits for an answer byte.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Launch {
    pub(super) args: Vec<String>,
    pub(super) answers: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Frame {
    Detecting,
    V2 { body_len: usize },
}

/// One client lifecycle on the server pipe.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) enum State {
    /// Waiting for a client to connect.
    #[default]
    Connecting,
    /// Reading the client's v1 or v2 launch message under one total `deadline`.
    Reading {
        attempt: Attempt,
        deadline: Instant,
        message: Vec<u8>,
        frame: Frame,
    },
    /// Message read; waiting for any harness `hold()` before deciding the answer.
    Holding {
        launch: Launch,
    },
    /// Writing the one-byte v2 answer under `deadline`.
    WritingAnswer {
        attempt: Attempt,
        deadline: Instant,
        cancellable: bool,
        launch: Launch,
        quitting: bool,
    },
    /// Waiting for the v2 client to read its answer and close its end under `deadline`.
    DrainingAnswer {
        attempt: Attempt,
        deadline: Instant,
        cancellable: bool,
        launch: Launch,
        quitting: bool,
    },
    /// Server stopped and pipe closed.
    Stopped,
}

/// Inputs fed by the overlapped pipe loop (or a unit test with a synthetic clock).
#[derive(Debug)]
pub(super) enum Event<'a> {
    Connected { now: Instant },
    ConnectFailed,
    ReadBytes { attempt: Attempt, bytes: &'a [u8] },
    ReadEof { attempt: Attempt },
    ReadFailed { attempt: Attempt },
    Held { now: Instant, stopping: bool, quitting: bool },
    WroteAnswer { attempt: Attempt, bytes: usize },
    WriteAnswerFailed { attempt: Attempt, err: u32 },
    DrainRead { attempt: Attempt, bytes: usize },
    DrainEof { attempt: Attempt },
    DrainFailed { attempt: Attempt, err: u32 },
    TimedOut { attempt: Attempt, err: u32 },
    Stopped,
}

/// Operations the pipe loop carries out in order.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect {
    Connect,
    Read { attempt: Attempt, want: usize, deadline: Instant },
    Hold,
    WriteAnswer { attempt: Attempt, byte: u8, deadline: Instant, cancellable: bool },
    DrainAnswer { attempt: Attempt, deadline: Instant, cancellable: bool },
    AnswerFailed { phase: &'static str, err: u32 },
    Deliver(Vec<String>),
    Disconnect,
    Close,
}

fn next_want(frame: &Frame, len: usize) -> usize {
    match frame {
        Frame::V2 { body_len } => (V2_HEAD + body_len - len).min(READ_CHUNK),
        Frame::Detecting => (MAX_V1_WIRE + 1 - len).min(READ_CHUNK),
    }
}

fn parse_args(body: &[u8]) -> Option<Vec<String>> {
    let text = std::str::from_utf8(body).ok()?;
    Some(text.split('\0').filter(|a| !a.is_empty()).map(str::to_owned).collect())
}

fn disconnect_and_connect(state: &mut State) -> Effects<Effect> {
    *state = State::Connecting;
    Effects::two(Effect::Disconnect, Effect::Connect)
}

fn finish_answer_failed(state: &mut State, phase: &'static str, err: u32, cancellable: bool) -> Effects<Effect> {
    let mut effects = Effects::two(Effect::AnswerFailed { phase, err }, Effect::Disconnect);
    if cancellable {
        *state = State::Connecting;
        effects.push(Effect::Connect);
    } else {
        *state = State::Stopped;
        effects.push(Effect::Close);
    }
    effects
}

/// Advances the pipe server state machine by one event. Stale events whose `Attempt` does not
/// match the active phase change nothing.
pub(super) fn step(seq: &mut Seq, state: &mut State, timeout: Duration, event: Event<'_>) -> Effects<Effect> {
    match (std::mem::take(state), event) {
        (State::Connecting, Event::Connected { now }) => {
            let attempt = seq.next();
            let deadline = now + timeout;
            let want = next_want(&Frame::Detecting, 0);
            *state = State::Reading { attempt, deadline, message: Vec::new(), frame: Frame::Detecting };
            Effects::one(Effect::Read { attempt, want, deadline })
        }
        (State::Connecting, Event::ConnectFailed) => {
            *state = State::Connecting;
            Effects::one(Effect::Connect)
        }
        (State::Connecting, Event::Stopped) => {
            *state = State::Stopped;
            Effects::one(Effect::Close)
        }

        (State::Reading { attempt, deadline, mut message, mut frame }, Event::ReadBytes { attempt: a, bytes }) if a == attempt => {
            let limit = match frame {
                Frame::V2 { body_len } => V2_HEAD + body_len,
                Frame::Detecting => MAX_V1_WIRE,
            };
            if bytes.len() > limit - message.len() {
                return disconnect_and_connect(state);
            }
            message.extend_from_slice(bytes);
            if matches!(frame, Frame::Detecting) {
                let v2_prefix = MAGIC.starts_with(&message[..message.len().min(MAGIC.len())]);
                let v1_prefix = MAGIC_V1.starts_with(&message[..message.len().min(MAGIC_V1.len())]);
                if !v2_prefix && !v1_prefix {
                    return disconnect_and_connect(state);
                }
                if message.len() >= V2_HEAD && message.starts_with(MAGIC) {
                    let Some(len_bytes) = message[MAGIC.len()..V2_HEAD].try_into().ok() else {
                        return disconnect_and_connect(state);
                    };
                    let body_len = u32::from_le_bytes(len_bytes) as usize;
                    if body_len > MAX_MESSAGE {
                        return disconnect_and_connect(state);
                    }
                    frame = Frame::V2 { body_len };
                }
            }
            if let Frame::V2 { body_len } = frame && message.len() >= V2_HEAD + body_len {
                let Some(args) = parse_args(&message[V2_HEAD..V2_HEAD + body_len]) else {
                    return disconnect_and_connect(state);
                };
                *state = State::Holding { launch: Launch { args, answers: true } };
                return Effects::one(Effect::Hold);
            }
            let want = next_want(&frame, message.len());
            *state = State::Reading { attempt, deadline, message, frame };
            Effects::one(Effect::Read { attempt, want, deadline })
        }
        (State::Reading { attempt, message, frame, .. }, Event::ReadEof { attempt: a }) if a == attempt => {
            if matches!(frame, Frame::Detecting)
                && let Some(body) = message.strip_prefix(MAGIC_V1)
                && let Some(args) = parse_args(body)
            {
                *state = State::Holding { launch: Launch { args, answers: false } };
                Effects::one(Effect::Hold)
            } else {
                disconnect_and_connect(state)
            }
        }
        (State::Reading { attempt, .. }, Event::ReadFailed { attempt: a } | Event::TimedOut { attempt: a, .. }) if a == attempt => {
            disconnect_and_connect(state)
        }
        (State::Reading { .. }, Event::Stopped) => {
            *state = State::Stopped;
            Effects::two(Effect::Disconnect, Effect::Close)
        }

        (State::Holding { launch }, Event::Held { now, stopping, quitting }) => {
            if !launch.answers {
                let mut effects = Effects::two(Effect::Deliver(launch.args), Effect::Disconnect);
                if stopping {
                    *state = State::Stopped;
                    effects.push(Effect::Close);
                } else {
                    *state = State::Connecting;
                    effects.push(Effect::Connect);
                }
                return effects;
            }
            let byte = if quitting { QUITTING } else { ACCEPTED };
            let wait = if stopping { timeout.min(ANSWER_WITHIN) } else { timeout };
            let deadline = now + wait;
            let attempt = seq.next();
            let cancellable = !stopping;
            *state = State::WritingAnswer { attempt, deadline, cancellable, launch, quitting };
            Effects::one(Effect::WriteAnswer { attempt, byte, deadline, cancellable })
        }

        (State::WritingAnswer { attempt, deadline, cancellable, launch, quitting }, Event::WroteAnswer { attempt: a, bytes }) if a == attempt => {
            if bytes == 1 {
                *state = State::DrainingAnswer { attempt, deadline, cancellable, launch, quitting };
                Effects::one(Effect::DrainAnswer { attempt, deadline, cancellable })
            } else {
                finish_answer_failed(state, "write", 0, cancellable)
            }
        }
        (State::WritingAnswer { attempt, cancellable, .. }, Event::WriteAnswerFailed { attempt: a, err } | Event::TimedOut { attempt: a, err }) if a == attempt => {
            finish_answer_failed(state, "write", err, cancellable)
        }
        (State::WritingAnswer { cancellable: true, .. }, Event::Stopped) => {
            finish_answer_failed(state, "write", ERROR_OPERATION_ABORTED, false)
        }

        (State::DrainingAnswer { attempt, cancellable, launch, quitting, .. }, Event::DrainEof { attempt: a }) if a == attempt => {
            let mut effects = Effects::none();
            if !quitting {
                effects.push(Effect::Deliver(launch.args));
            }
            effects.push(Effect::Disconnect);
            if cancellable {
                *state = State::Connecting;
                effects.push(Effect::Connect);
            } else {
                *state = State::Stopped;
                effects.push(Effect::Close);
            }
            effects
        }
        (State::DrainingAnswer { attempt, deadline, cancellable, launch, quitting }, Event::DrainRead { attempt: a, bytes }) if a == attempt => {
            if bytes == 0 {
                *state = State::DrainingAnswer { attempt, deadline, cancellable, launch, quitting };
                Effects::one(Effect::DrainAnswer { attempt, deadline, cancellable })
            } else {
                finish_answer_failed(state, "flush", 0, cancellable)
            }
        }
        (State::DrainingAnswer { attempt, cancellable, .. }, Event::DrainFailed { attempt: a, err } | Event::TimedOut { attempt: a, err }) if a == attempt => {
            finish_answer_failed(state, "flush", err, cancellable)
        }
        (State::DrainingAnswer { cancellable: true, .. }, Event::Stopped) => {
            finish_answer_failed(state, "flush", ERROR_OPERATION_ABORTED, false)
        }

        (other, _) => {
            *state = other;
            Effects::none()
        }
    }
}
