//! Pure state machine for a pending OS-initiated application quit request (macOS
//! `applicationShouldTerminate:` returning `NSTerminateLater`).
//!
//! Keeps pending hook dispatch, queued replies, re-entrancy deferral (`Loop::Handling`), and
//! programmatic-quit overrides in one explicit state enum so the native reply is sent at most
//! once and never while a winit handler holds its mutable borrow.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::machine::Effects;
use super::Loop;

/// Where a pending OS application-level quit request stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RequestState {
    /// No OS quit request is awaiting a reply.
    #[default]
    Idle,
    /// `applicationShouldTerminate:` returned `NSTerminateLater` and woke the event loop to
    /// consult live hosts' `App::on_quit_request` hooks.
    Asking,
    /// A decision has been reached and one common-mode main-run-loop block is queued to send
    /// `replyToApplicationShouldTerminate:allow` once the loop is idle.
    ReplyQueued { allow: bool },
    /// The queued block fired while `Loop::Handling` was active; the outermost `Handling` drop
    /// will re-schedule the common-mode block once `LOOP` returns to `Loop::Idle`.
    WaitingForIdle { allow: bool },
}

/// Inputs to the quit-request state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum RequestEvent {
    /// AppKit invoked `applicationShouldTerminate:`.
    OsQuitRequested { has_hooks: bool, quitting: bool },
    /// Waking the event loop from `applicationShouldTerminate:` failed.
    WakeFailed,
    /// Live hosts' `on_quit_request` hooks finished deciding (`true` = allow quit, `false` = defer).
    Decided { allow: bool },
    /// A programmatic quit/relaunch (`Tx::quit`, `AppHandle::quit`, `Tx::relaunch`,
    /// `AppHandle::relaunch`) or last-main-window exit started while an OS quit may be pending.
    ProgrammaticQuit,
    /// The queued common-mode main-run-loop block fired with the current loop state.
    FlushReply { loop_state: Loop },
    /// The outermost `Handling` guard dropped and returned `LOOP` to `Loop::Idle`.
    LoopIdle,
}

/// Side effects the caller carries out for one transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum RequestEffect {
    /// Return `NSTerminateNow` immediately from `applicationShouldTerminate:`.
    ReturnNow,
    /// Wake the event loop with `WakeEvent::QuitRequest` and return `NSTerminateLater`.
    WakeAndReturnLater,
    /// Coalesce a duplicate `applicationShouldTerminate:` and return `NSTerminateLater`.
    ReturnLater,
    /// Queue one common-mode `CFRunLoopPerformBlock` + `CFRunLoopWakeUp` on the main run loop.
    ScheduleReply,
    /// Send `[NSApp replyToApplicationShouldTerminate:allow]` exactly once outside any handler.
    SendReply { allow: bool },
    /// Suppress a redundant `event_loop.exit()` because AppKit is awaiting the pending reply.
    SuppressExit,
}

/// The single transition function for OS quit requests.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn step(state: &mut RequestState, event: RequestEvent) -> Effects<RequestEffect> {
    use RequestEffect::*;
    use RequestEvent::*;
    use RequestState::*;

    match (*state, event) {
        (Idle, OsQuitRequested { has_hooks: false, .. })
        | (Idle, OsQuitRequested { quitting: true, .. }) => Effects::one(ReturnNow),
        (Idle, OsQuitRequested { has_hooks: true, quitting: false }) => {
            *state = Asking;
            Effects::one(WakeAndReturnLater)
        }
        (Asking | ReplyQueued { .. } | WaitingForIdle { .. }, OsQuitRequested { .. }) => {
            Effects::one(ReturnLater)
        }
        (Asking, WakeFailed) => {
            *state = Idle;
            Effects::one(ReturnNow)
        }
        (_, WakeFailed) => Effects::none(),
        (Asking, Decided { allow }) => {
            *state = ReplyQueued { allow };
            Effects::one(ScheduleReply)
        }
        (Idle | ReplyQueued { .. } | WaitingForIdle { .. }, Decided { .. }) => Effects::none(),
        (Idle, ProgrammaticQuit) => Effects::none(),
        (Asking, ProgrammaticQuit) => {
            *state = ReplyQueued { allow: true };
            Effects::two(ScheduleReply, SuppressExit)
        }
        (ReplyQueued { .. }, ProgrammaticQuit) => {
            *state = ReplyQueued { allow: true };
            Effects::one(SuppressExit)
        }
        (WaitingForIdle { .. }, ProgrammaticQuit) => {
            *state = WaitingForIdle { allow: true };
            Effects::one(SuppressExit)
        }
        (ReplyQueued { allow }, FlushReply { loop_state: Loop::Idle }) => {
            *state = Idle;
            Effects::one(SendReply { allow })
        }
        (ReplyQueued { allow }, FlushReply { loop_state: Loop::Handling }) => {
            *state = WaitingForIdle { allow };
            Effects::none()
        }
        (Idle | Asking | WaitingForIdle { .. }, FlushReply { .. }) => Effects::none(),
        (WaitingForIdle { allow }, LoopIdle) => {
            *state = ReplyQueued { allow };
            Effects::one(ScheduleReply)
        }
        (Idle | Asking | ReplyQueued { .. }, LoopIdle) => Effects::none(),
    }
}

static REQUEST: Mutex<RequestState> = Mutex::new(RequestState::Idle);
static HAS_HOOKS: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_has_hooks(any: bool) {
    HAS_HOOKS.store(any, Ordering::Release);
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn has_hooks() -> bool {
    HAS_HOOKS.load(Ordering::Acquire)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn is_asking() -> bool {
    *REQUEST.lock().unwrap_or_else(|e| e.into_inner()) == RequestState::Asking
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn send(event: RequestEvent) -> Effects<RequestEffect> {
    let mut state = REQUEST.lock().unwrap_or_else(|e| e.into_inner());
    step(&mut state, event)
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
