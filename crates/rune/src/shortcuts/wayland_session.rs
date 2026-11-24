//! Explicit state machine for a Wayland `org.freedesktop.portal.GlobalShortcuts` session.
//!
//! The portal allows at most one `BindShortcuts` call per session, and both GNOME and KDE replace
//! the app's stored shortcut set on every `BindShortcuts`. One session therefore holds the app's
//! entire active set: startup shortcuts bind together in one call, adding or removing a shortcut
//! closes the current session and opens a fresh session with the new full set, and removing the
//! last shortcut closes the session without rebinding.
//!
//! While a bind request is in flight (potentially waiting on a user consent dialog), later
//! register and unregister commands queue instead of opening overlapping dialogs, and completions
//! whose `Attempt` no longer matches the active request are ignored (with any returned session
//! closed).

use super::{ShortcutError, ShortcutId};
use crate::machine::{Attempt, Effects, Seq};
use std::{collections::{HashSet, VecDeque}, mem};

/// One shortcut entry sent to `GlobalShortcuts.BindShortcuts`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PortalShortcut {
    pub(super) id: ShortcutId,
    pub(super) portal_id: String,
    pub(super) description: String,
    pub(super) preferred_trigger: String,
}

/// Successful outcome of `CreateSession` + `BindShortcuts`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BindOutcome<H> {
    pub(super) session: H,
    /// The `shortcut_id` strings the portal reported back as bound.
    pub(super) bound_portal_ids: Vec<String>,
}

/// A command that arrived while another bind request was in flight.
#[derive(Debug)]
pub(super) enum QueuedOp<R, U> {
    Register { items: Vec<PortalShortcut>, reply: R },
    Unregister { id: ShortcutId, reply: U },
}

/// One caller's register batch inside an in-flight bind request.
#[derive(Debug)]
pub(super) struct PendingRegister<R> {
    /// Per-item status: `Ok(shortcut)` was included in `BindShortcuts`, `Err(error)` was
    /// rejected locally before contacting the portal.
    pub(super) entries: Vec<Result<PortalShortcut, ShortcutError>>,
    pub(super) reply: R,
}

/// The operation that an in-flight bind request is carrying out.
#[derive(Debug)]
pub(super) enum InFlight<R, U> {
    /// Binding one or more coalesced register batches on top of `committed`.
    Register { batches: Vec<PendingRegister<R>> },
    /// Rebinding `target` after one or more coalesced unregistrations.
    Unregister { target: Vec<PortalShortcut>, replies: Vec<U> },
    /// Restoring `committed` after a failed register closed the previous session.
    Restore,
}

#[derive(Debug)]
pub(super) enum State<H, R, U> {
    /// No portal session is open and no request is in flight.
    /// `committed` is empty except when the portal closed the session externally or a restore
    /// bind failed after a declined runtime registration.
    Idle { committed: Vec<PortalShortcut> },
    /// One portal session is open holding `committed` (non-empty).
    Bound { session: H, committed: Vec<PortalShortcut> },
    /// A `CreateSession` + `BindShortcuts` request (`attempt`) is in flight.
    Binding {
        attempt: Attempt,
        committed: Vec<PortalShortcut>,
        in_flight: InFlight<R, U>,
        queued: VecDeque<QueuedOp<R, U>>,
    },
    /// Shutting down; no further requests start and late commands receive `NotRunning`.
    Closed,
}

impl<H, R, U> Default for State<H, R, U> {
    fn default() -> Self { Self::Idle { committed: Vec::new() } }
}

#[derive(Debug)]
pub(super) enum Event<H, R, U> {
    Register { items: Vec<PortalShortcut>, reply: R },
    Unregister { id: ShortcutId, reply: U },
    BindFinished { attempt: Attempt, result: Result<BindOutcome<H>, ShortcutError> },
    Activated { session: H, portal_id: String, token: Option<String> },
    SessionClosed { session: H },
    Shutdown,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect<H, R, U> {
    /// Close an existing portal session before opening a new one or shutting down.
    CloseSession(H),
    /// Start `CreateSession` + `BindShortcuts` for the complete `shortcuts` list.
    Bind { attempt: Attempt, shortcuts: Vec<PortalShortcut> },
    /// Cancel an in-flight bind request on shutdown (`Request::Close`).
    CancelBind(Attempt),
    /// Deliver per-item results to a `Register` caller.
    ReplyRegister { reply: R, results: Vec<Result<(), ShortcutError>> },
    /// Deliver the result to an `Unregister` caller.
    ReplyUnregister { reply: U, result: Result<(), ShortcutError> },
    /// Dispatch a shortcut activation to the app's sink.
    Activate { id: ShortcutId, token: Option<String> },
}

pub(super) fn step<H: PartialEq, R, U>(
    seq: &mut Seq,
    state: &mut State<H, R, U>,
    event: Event<H, R, U>,
) -> Effects<Effect<H, R, U>> {
    let mut effects = Effects::none();
    match event {
        Event::Register { items, reply } => match state {
            State::Closed => {
                let results = vec![Err(ShortcutError::NotRunning); items.len()];
                effects.push(Effect::ReplyRegister { reply, results });
            }
            State::Binding { queued, .. } => {
                queued.push_back(QueuedOp::Register { items, reply });
            }
            State::Idle { .. } | State::Bound { .. } => {
                let _ = start_register_batches(seq, state, vec![(items, reply)], VecDeque::new(), &mut effects);
            }
        },
        Event::Unregister { id, reply } => match state {
            State::Closed => effects.push(Effect::ReplyUnregister { reply, result: Err(ShortcutError::NotRunning) }),
            State::Binding { queued, .. } => {
                queued.push_back(QueuedOp::Unregister { id, reply });
            }
            State::Idle { .. } | State::Bound { .. } => {
                let _ = start_unregisters(seq, state, vec![(id, reply)], VecDeque::new(), &mut effects);
            }
        },
        Event::BindFinished { attempt, result } => {
            let matches = matches!(state, State::Binding { attempt: current, .. } if *current == attempt);
            if !matches {
                if let Ok(outcome) = result {
                    effects.push(Effect::CloseSession(outcome.session));
                }
                return effects;
            }
            let State::Binding { mut committed, in_flight, queued, .. } = mem::replace(state, State::default()) else {
                unreachable!()
            };
            let mut should_restore = false;
            match in_flight {
                InFlight::Register { batches } => match result {
                    Ok(outcome) => {
                        let bound: HashSet<&str> = outcome.bound_portal_ids.iter().map(String::as_str).collect();
                        for batch in batches {
                            let mut results = Vec::with_capacity(batch.entries.len());
                            for entry in batch.entries {
                                match entry {
                                    Ok(shortcut) if bound.contains(shortcut.portal_id.as_str()) => {
                                        committed.push(shortcut);
                                        results.push(Ok(()));
                                    }
                                    Ok(_) => results.push(Err(ShortcutError::PermissionDenied)),
                                    Err(err) => results.push(Err(err)),
                                }
                            }
                            effects.push(Effect::ReplyRegister { reply: batch.reply, results });
                        }
                        if bound.is_empty() || committed.is_empty() {
                            effects.push(Effect::CloseSession(outcome.session));
                            *state = State::Idle { committed };
                        } else {
                            *state = State::Bound { session: outcome.session, committed };
                        }
                    }
                    Err(err) => {
                        for batch in batches {
                            let results = batch.entries.into_iter().map(|entry| match entry {
                                Ok(_) => Err(err.clone()),
                                Err(local) => Err(local),
                            }).collect();
                            effects.push(Effect::ReplyRegister { reply: batch.reply, results });
                        }
                        should_restore = !committed.is_empty();
                        *state = State::Idle { committed };
                    }
                },
                InFlight::Unregister { target, replies } => match result {
                    Ok(outcome) => {
                        for reply in replies {
                            effects.push(Effect::ReplyUnregister { reply, result: Ok(()) });
                        }
                        if outcome.bound_portal_ids.is_empty() || target.is_empty() {
                            effects.push(Effect::CloseSession(outcome.session));
                            *state = State::Idle { committed: target };
                        } else {
                            *state = State::Bound { session: outcome.session, committed: target };
                        }
                    }
                    Err(err) => {
                        for reply in replies {
                            effects.push(Effect::ReplyUnregister { reply, result: Err(err.clone()) });
                        }
                        *state = State::Idle { committed };
                    }
                },
                InFlight::Restore => match result {
                    Ok(outcome) => {
                        if outcome.bound_portal_ids.is_empty() || committed.is_empty() {
                            effects.push(Effect::CloseSession(outcome.session));
                            *state = State::Idle { committed };
                        } else {
                            *state = State::Bound { session: outcome.session, committed };
                        }
                    }
                    Err(_) => {
                        *state = State::Idle { committed };
                    }
                },
            }

            drain_queued(seq, state, queued, &mut effects);

            if should_restore && matches!(state, State::Idle { committed } if !committed.is_empty()) {
                let State::Idle { committed } = mem::replace(state, State::default()) else { unreachable!() };
                let attempt = seq.next();
                let shortcuts = committed.clone();
                *state = State::Binding { attempt, committed, in_flight: InFlight::Restore, queued: VecDeque::new() };
                effects.push(Effect::Bind { attempt, shortcuts });
            }
        }
        Event::Activated { session, portal_id, token } => {
            if let State::Bound { session: current, committed } = state
                && *current == session
                && let Some(item) = committed.iter().find(|s| s.portal_id == portal_id)
            {
                effects.push(Effect::Activate { id: item.id, token });
            }
        }
        Event::SessionClosed { session } => {
            if matches!(state, State::Bound { session: current, .. } if *current == session) {
                let State::Bound { committed, .. } = mem::replace(state, State::default()) else { unreachable!() };
                *state = State::Idle { committed };
            }
        }
        Event::Shutdown => match mem::replace(state, State::Closed) {
            State::Idle { .. } | State::Closed => {}
            State::Bound { session, .. } => effects.push(Effect::CloseSession(session)),
            State::Binding { attempt, in_flight, queued, .. } => {
                effects.push(Effect::CancelBind(attempt));
                fail_in_flight(in_flight, &mut effects);
                for op in queued {
                    match op {
                        QueuedOp::Register { items, reply } => {
                            let results = vec![Err(ShortcutError::NotRunning); items.len()];
                            effects.push(Effect::ReplyRegister { reply, results });
                        }
                        QueuedOp::Unregister { reply, .. } => {
                            effects.push(Effect::ReplyUnregister { reply, result: Err(ShortcutError::NotRunning) });
                        }
                    }
                }
            }
        },
    }
    effects
}

fn fail_in_flight<H, R, U>(in_flight: InFlight<R, U>, effects: &mut Effects<Effect<H, R, U>>) {
    match in_flight {
        InFlight::Register { batches } => {
            for batch in batches {
                let results = batch.entries.into_iter().map(|entry| match entry {
                    Ok(_) => Err(ShortcutError::NotRunning),
                    Err(local) => Err(local),
                }).collect();
                effects.push(Effect::ReplyRegister { reply: batch.reply, results });
            }
        }
        InFlight::Unregister { replies, .. } => {
            for reply in replies {
                effects.push(Effect::ReplyUnregister { reply, result: Err(ShortcutError::NotRunning) });
            }
        }
        InFlight::Restore => {}
    }
}

fn drain_queued<H, R, U>(
    seq: &mut Seq,
    state: &mut State<H, R, U>,
    mut queued: VecDeque<QueuedOp<R, U>>,
    effects: &mut Effects<Effect<H, R, U>>,
) {
    while let Some(front) = queued.pop_front() {
        match front {
            QueuedOp::Register { items, reply } => {
                let mut raw = vec![(items, reply)];
                while matches!(queued.front(), Some(QueuedOp::Register { .. })) {
                    let Some(QueuedOp::Register { items, reply }) = queued.pop_front() else { unreachable!() };
                    raw.push((items, reply));
                }
                if let Some(rest) = start_register_batches(seq, state, raw, queued, effects) {
                    queued = rest;
                } else {
                    return;
                }
            }
            QueuedOp::Unregister { id, reply } => {
                let mut ops = vec![(id, reply)];
                while matches!(queued.front(), Some(QueuedOp::Unregister { .. })) {
                    let Some(QueuedOp::Unregister { id, reply }) = queued.pop_front() else { unreachable!() };
                    ops.push((id, reply));
                }
                if let Some(rest) = start_unregisters(seq, state, ops, queued, effects) {
                    queued = rest;
                } else {
                    return;
                }
            }
        }
    }
}

/// Validates and starts a bind for one or more coalesced register batches.
/// Returns `None` if `state` transitioned into `State::Binding` (consuming `queued`), or
/// `Some(queued)` if all items settled locally so the caller can keep draining.
fn start_register_batches<H, R, U>(
    seq: &mut Seq,
    state: &mut State<H, R, U>,
    raw_batches: Vec<(Vec<PortalShortcut>, R)>,
    queued: VecDeque<QueuedOp<R, U>>,
    effects: &mut Effects<Effect<H, R, U>>,
) -> Option<VecDeque<QueuedOp<R, U>>> {
    let committed = match state {
        State::Idle { committed } | State::Bound { committed, .. } => committed.as_slice(),
        _ => return Some(queued),
    };
    let mut seen_ids: HashSet<ShortcutId> = committed.iter().map(|s| s.id).collect();
    let mut seen_portal_ids: HashSet<String> = committed.iter().map(|s| s.portal_id.clone()).collect();
    let mut pending_batches: Vec<PendingRegister<R>> = Vec::new();
    let mut to_bind: Vec<PortalShortcut> = Vec::new();

    for (items, reply) in raw_batches {
        if items.is_empty() {
            effects.push(Effect::ReplyRegister { reply, results: Vec::new() });
            continue;
        }
        let mut entries = Vec::with_capacity(items.len());
        let mut accepted_any = false;
        for item in items {
            if item.portal_id.trim().is_empty() {
                entries.push(Err(ShortcutError::InvalidAccelerator("empty portal shortcut id".into())));
            } else if !seen_ids.insert(item.id) || !seen_portal_ids.insert(item.portal_id.clone()) {
                entries.push(Err(ShortcutError::Conflict));
            } else {
                to_bind.push(item.clone());
                entries.push(Ok(item));
                accepted_any = true;
            }
        }
        if accepted_any {
            pending_batches.push(PendingRegister { entries, reply });
        } else {
            let results = entries.into_iter().map(|e| e.map(|_| ())).collect();
            effects.push(Effect::ReplyRegister { reply, results });
        }
    }

    if to_bind.is_empty() {
        return Some(queued);
    }

    let (old_session, committed) = match mem::replace(state, State::default()) {
        State::Idle { committed } => (None, committed),
        State::Bound { session, committed } => (Some(session), committed),
        _ => unreachable!(),
    };
    if let Some(session) = old_session {
        effects.push(Effect::CloseSession(session));
    }
    let mut shortcuts = committed.clone();
    shortcuts.extend(to_bind);
    let attempt = seq.next();
    *state = State::Binding {
        attempt,
        committed,
        in_flight: InFlight::Register { batches: pending_batches },
        queued,
    };
    effects.push(Effect::Bind { attempt, shortcuts });
    None
}

/// Processes one or more coalesced unregistrations.
/// Returns `None` if `state` transitioned into `State::Binding` (consuming `queued`), or
/// `Some(queued)` if the unregistrations settled immediately so the caller can keep draining.
fn start_unregisters<H, R, U>(
    seq: &mut Seq,
    state: &mut State<H, R, U>,
    ops: Vec<(ShortcutId, U)>,
    queued: VecDeque<QueuedOp<R, U>>,
    effects: &mut Effects<Effect<H, R, U>>,
) -> Option<VecDeque<QueuedOp<R, U>>> {
    let committed = match state {
        State::Idle { committed } | State::Bound { committed, .. } => committed.as_slice(),
        _ => return Some(queued),
    };
    let mut target = committed.to_vec();
    let mut removed_replies = Vec::new();
    for (id, reply) in ops {
        if let Some(pos) = target.iter().position(|s| s.id == id) {
            target.remove(pos);
            removed_replies.push(reply);
        } else {
            effects.push(Effect::ReplyUnregister { reply, result: Err(ShortcutError::UnknownId) });
        }
    }
    if removed_replies.is_empty() {
        return Some(queued);
    }

    match mem::replace(state, State::default()) {
        State::Idle { .. } => {
            *state = State::Idle { committed: target };
            for reply in removed_replies {
                effects.push(Effect::ReplyUnregister { reply, result: Ok(()) });
            }
            Some(queued)
        }
        State::Bound { session, committed } => {
            effects.push(Effect::CloseSession(session));
            if target.is_empty() {
                *state = State::Idle { committed: Vec::new() };
                for reply in removed_replies {
                    effects.push(Effect::ReplyUnregister { reply, result: Ok(()) });
                }
                Some(queued)
            } else {
                let attempt = seq.next();
                let shortcuts = target.clone();
                *state = State::Binding {
                    attempt,
                    committed,
                    in_flight: InFlight::Unregister { target, replies: removed_replies },
                    queued,
                };
                effects.push(Effect::Bind { attempt, shortcuts });
                None
            }
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
#[path = "wayland_session_tests.rs"]
mod tests;
