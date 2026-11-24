//! Pure state machine, RFC 2483 `text/uri-list` parser, surface routing table, and bounded
//! pipe-read loop for native Wayland file drops (`wl_data_device` / `wl_data_offer`).
//!
//! Kept cross-platform so every transition, URI edge case, timeout/size bound, and multi-window
//! routing rule runs unit tests headless on every host without a Wayland compositor.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::machine::{Attempt, Effects, Seq};
use crate::window::FileStep;

pub(crate) const URI_LIST_MIME: &str = "text/uri-list";
/// Hard upper bound on a single `text/uri-list` transfer (1 MiB).
pub(crate) const MAX_URI_LIST_BYTES: usize = 1024 * 1024;
/// Eager enter-read timeout before treating the source as deferring writes until `drop`.
pub(crate) const ENTER_READ_TIMEOUT: Duration = Duration::from_millis(500);
/// Post-drop fallback pipe-read timeout before giving up on a silent peer.
pub(crate) const DROP_READ_TIMEOUT: Duration = Duration::from_millis(2_000);
/// Cap on unclaimed `wl_data_offer` proxies retained between `data_offer` and `enter`/`selection`.
pub(crate) const MAX_PENDING_OFFERS: usize = 16;

/// Wayland wire object ID of a `wl_data_offer`.
pub(crate) type OfferId = u32;

/// Wayland wire object ID of a `wl_surface`.
pub(crate) type SurfaceId = u32;

/// Monotonic token distinguishing one window registration from a later window whose `wl_surface`
/// reuses the same wire object ID.
pub(crate) type WindowToken = u64;

/// Which drag phase requested a pipe read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadPhase {
    Enter,
    Drop,
}

impl ReadPhase {
    pub(crate) fn timeout(self) -> Duration {
        match self {
            Self::Enter => ENTER_READ_TIMEOUT,
            Self::Drop => DROP_READ_TIMEOUT,
        }
    }
}

/// Result of one background pipe read for `text/uri-list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadOutcome {
    /// Pipe reached EOF within `MAX_URI_LIST_BYTES`.
    Bytes(Vec<u8>),
    /// Peer wrote more than `MAX_URI_LIST_BYTES`; the pipe was closed early.
    TooLarge,
    /// Peer did not finish writing before the phase deadline.
    TimedOut,
    /// Pipe creation, worker spawn, or read failed.
    Error,
}

/// Bit values of `wl_data_device_manager.dnd_action` from the Wayland core protocol.
pub(crate) const DND_ACTION_NONE: u32 = 0;
pub(crate) const DND_ACTION_COPY: u32 = 1;
pub(crate) const DND_ACTION_MOVE: u32 = 2;
pub(crate) const DND_ACTION_ASK: u32 = 4;

/// Selected drag-and-drop action reported by `wl_data_offer.action` (protocol v3+).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SelectedAction {
    /// No action received yet, `none` (`0`), or an unknown/multi-bit mask.
    #[default]
    None,
    /// Concrete `copy` action.
    Copy,
    /// Concrete `move` action (never requested by Rune's copy-only drop target).
    Move,
    /// Non-final `ask` action; calling `wl_data_offer.finish` while `Ask` is selected raises a
    /// fatal `invalid_finish` protocol error under `wl_data_offer` v3+.
    Ask,
}

impl SelectedAction {
    /// Decodes a `wl_data_offer.action` wire bitmask. Per spec only a single action bit or `0`
    /// is valid; combined or unknown bits map to `None`.
    pub(crate) fn from_wayland_mask(bits: u32) -> Self {
        match bits {
            DND_ACTION_COPY => Self::Copy,
            DND_ACTION_MOVE => Self::Move,
            DND_ACTION_ASK => Self::Ask,
            _ => Self::None,
        }
    }

    /// Whether the Wayland v3+ protocol considers this a concrete final action (as opposed to
    /// `None` or `Ask`, which cannot be finished without raising `invalid_finish`).
    #[cfg(test)]
    pub(crate) fn is_final_action(self) -> bool {
        matches!(self, Self::Copy | Self::Move)
    }

    /// Whether this copy-only drop target may complete the drop and send `wl_data_offer.finish`.
    /// Only `Copy` is accepted: `None` and `Ask` are fatal under v3+, and finishing `Move`
    /// would tell a file manager to delete the source file.
    pub(crate) fn allows_copy_finish(self) -> bool {
        matches!(self, Self::Copy)
    }
}

/// Metadata collected for a `wl_data_offer` between `wl_data_device.data_offer` and `enter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OfferInfo {
    pub(crate) id: OfferId,
    pub(crate) version: u32,
    pub(crate) has_uri_list: bool,
    pub(crate) selected_action: SelectedAction,
}

/// Bounded queue of `wl_data_offer` objects announced by `wl_data_device.data_offer` that have
/// not yet been claimed by `enter` or `selection`.
#[derive(Clone, Debug, Default)]
pub(crate) struct OfferSet {
    pending: Vec<OfferInfo>,
}

impl OfferSet {
    /// Records a newly announced offer. If the table is already at `MAX_PENDING_OFFERS`, evicts
    /// and returns the oldest unclaimed offer ID so the caller can destroy it.
    pub(crate) fn insert(&mut self, id: OfferId, version: u32) -> Option<OfferId> {
        let evicted = (self.pending.len() >= MAX_PENDING_OFFERS).then(|| self.pending.remove(0).id);
        self.pending.push(OfferInfo {
            id,
            version,
            has_uri_list: false,
            selected_action: SelectedAction::None,
        });
        evicted
    }

    /// Records a `wl_data_offer.offer` MIME type on a pending offer.
    pub(crate) fn add_mime(&mut self, id: OfferId, mime: &str) {
        if mime == URI_LIST_MIME
            && let Some(entry) = self.pending.iter_mut().find(|o| o.id == id)
        {
            entry.has_uri_list = true;
        }
    }

    /// Records a `wl_data_offer.action` update on a pending offer.
    pub(crate) fn set_action(&mut self, id: OfferId, action: SelectedAction) {
        if let Some(entry) = self.pending.iter_mut().find(|o| o.id == id) {
            entry.selected_action = action;
        }
    }

    /// Removes and returns a pending offer by ID.
    pub(crate) fn take(&mut self, id: OfferId) -> Option<OfferInfo> {
        let idx = self.pending.iter().position(|o| o.id == id)?;
        Some(self.pending.remove(idx))
    }

    /// Drains all unclaimed pending offer IDs for teardown.
    pub(crate) fn drain_ids(&mut self) -> Vec<OfferId> {
        self.pending.drain(..).map(|o| o.id).collect()
    }
}

/// Active drag-and-drop mode on one `wl_data_device`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum State {
    /// No drag is over any window owned by this controller.
    #[default]
    Idle,
    /// An offer arrived that does not carry local files (or entered an unregistered surface); kept
    /// alive until `leave` or replacement so its proxy is destroyed cleanly.
    Rejected { offer: OfferId },
    /// Accepted `text/uri-list` on `enter` and eagerly reading the pipe on a background worker.
    Fetching {
        offer: OfferId,
        serial: u32,
        version: u32,
        selected_action: SelectedAction,
        surface: SurfaceId,
        token: WindowToken,
        pos: (f32, f32),
        attempt: Attempt,
    },
    /// Enter-time read yielded no bytes (empty, timed out, or errored); keeping the offer accepted
    /// so a source that defers writes until `drop` gets one post-drop fallback `receive`.
    Deferred {
        offer: OfferId,
        serial: u32,
        version: u32,
        selected_action: SelectedAction,
        surface: SurfaceId,
        token: WindowToken,
        pos: (f32, f32),
    },
    /// Enter-time read produced non-empty local paths and `FileStep::Enter` has been enqueued.
    Hovering {
        offer: OfferId,
        serial: u32,
        version: u32,
        selected_action: SelectedAction,
        surface: SurfaceId,
        token: WindowToken,
        pos: (f32, f32),
        paths: Vec<PathBuf>,
    },
    /// `wl_data_device.drop` arrived while waiting for a pipe read (either the in-flight enter
    /// read or the single post-drop fallback). Ignores immediate post-drop `leave` events.
    Dropping {
        offer: OfferId,
        version: u32,
        selected_action: SelectedAction,
        surface: SurfaceId,
        token: WindowToken,
        pos: (f32, f32),
        attempt: Attempt,
        fallback_available: bool,
    },
}

/// Events stepping one `wl_data_device`'s drag state machine.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// `wl_data_device.enter` for `surface` at logical surface-local `(x, y)`.
    /// `target` is `Some(token)` when `surface` belongs to a registered window.
    Enter {
        serial: u32,
        surface: SurfaceId,
        target: Option<WindowToken>,
        x: f32,
        y: f32,
        offer: Option<OfferInfo>,
    },
    /// `wl_data_device.motion` at logical surface-local `(x, y)`.
    Motion { x: f32, y: f32 },
    /// `wl_data_device.leave`.
    Leave,
    /// `wl_data_device.drop`.
    Drop,
    /// `wl_data_offer.action` reported the currently selected DnD action (v3+).
    OfferAction { offer: OfferId, action: SelectedAction },
    /// Background pipe worker finished reading for `attempt`.
    ReadComplete { attempt: Attempt, outcome: ReadOutcome },
    /// A registered window's `DropTarget` was dropped.
    UnregisterWindow { surface: SurfaceId, token: WindowToken },
    /// The seat was removed or the controller is tearing down.
    Gone,
}

/// Wire requests, worker actions, and host `FileStep`s produced by one transition, in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    AcceptOffer { offer: OfferId, serial: u32, mime: Option<&'static str> },
    /// Calls `wl_data_offer.set_actions`: `Copy/Copy` when `copy` is true, `None/None` when false.
    SetActions { offer: OfferId, copy: bool },
    StartRead { offer: OfferId, attempt: Attempt, phase: ReadPhase },
    CancelRead(Attempt),
    FinishOffer(OfferId),
    DestroyOffer(OfferId),
    Flush,
    EnqueueStep { surface: SurfaceId, token: WindowToken, step: FileStep },
}

fn can_deliver_drop(version: u32, action: SelectedAction) -> bool {
    version < 3 || action.allows_copy_finish()
}

fn should_finish(version: u32, action: SelectedAction) -> bool {
    version >= 3 && action.allows_copy_finish()
}

fn reject_offer(effects: &mut Effects<Effect>, offer: OfferId, serial: u32, version: u32) {
    effects.push(Effect::AcceptOffer { offer, serial, mime: None });
    if version >= 3 {
        effects.push(Effect::SetActions { offer, copy: false });
    }
    effects.push(Effect::Flush);
}

fn tear_down_active(effects: &mut Effects<Effect>, old: State, emit_leave: bool) {
    match old {
        State::Idle => {}
        State::Rejected { offer } | State::Deferred { offer, .. } => {
            effects.push(Effect::DestroyOffer(offer));
            effects.push(Effect::Flush);
        }
        State::Fetching { offer, attempt, .. } | State::Dropping { offer, attempt, .. } => {
            effects.push(Effect::CancelRead(attempt));
            effects.push(Effect::DestroyOffer(offer));
            effects.push(Effect::Flush);
        }
        State::Hovering { offer, surface, token, .. } => {
            if emit_leave {
                effects.push(Effect::EnqueueStep { surface, token, step: FileStep::Leave });
            }
            effects.push(Effect::DestroyOffer(offer));
            effects.push(Effect::Flush);
        }
    }
}

/// Advances `state` by `event` and returns the ordered effects to execute.
pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
    match event {
        Event::Enter { serial, surface, target, x, y, offer } => {
            let mut effects = Effects::none();
            let old = std::mem::take(state);
            tear_down_active(&mut effects, old, true);
            let Some(info) = offer else {
                return effects;
            };
            let Some(token) = target.filter(|_| info.has_uri_list) else {
                reject_offer(&mut effects, info.id, serial, info.version);
                *state = State::Rejected { offer: info.id };
                return effects;
            };
            let attempt = seq.next();
            effects.push(Effect::AcceptOffer { offer: info.id, serial, mime: Some(URI_LIST_MIME) });
            if info.version >= 3 {
                effects.push(Effect::SetActions { offer: info.id, copy: true });
            }
            effects.push(Effect::StartRead { offer: info.id, attempt, phase: ReadPhase::Enter });
            effects.push(Effect::Flush);
            *state = State::Fetching {
                offer: info.id,
                serial,
                version: info.version,
                selected_action: info.selected_action,
                surface,
                token,
                pos: (x, y),
                attempt,
            };
            effects
        }
        Event::Motion { x, y } => match state {
            State::Fetching { pos, .. } | State::Deferred { pos, .. } => {
                *pos = (x, y);
                Effects::none()
            }
            State::Hovering { surface, token, pos, .. } => {
                *pos = (x, y);
                Effects::one(Effect::EnqueueStep {
                    surface: *surface,
                    token: *token,
                    step: FileStep::Move((x, y)),
                })
            }
            State::Idle | State::Rejected { .. } | State::Dropping { .. } => Effects::none(),
        },
        Event::Leave => match state {
            // Compositors may send `leave` immediately after `drop`; keep waiting for the read.
            State::Idle | State::Dropping { .. } => Effects::none(),
            _ => {
                let mut effects = Effects::none();
                let old = std::mem::take(state);
                tear_down_active(&mut effects, old, true);
                effects
            }
        },
        Event::Drop => match std::mem::take(state) {
            State::Idle => Effects::none(),
            State::Rejected { offer } => Effects::two(Effect::DestroyOffer(offer), Effect::Flush),
            State::Hovering { offer, version, selected_action, surface, token, pos, paths, .. } => {
                let mut effects = Effects::none();
                if can_deliver_drop(version, selected_action) {
                    effects.push(Effect::EnqueueStep {
                        surface,
                        token,
                        step: FileStep::Drop(paths, Some(pos)),
                    });
                    if should_finish(version, selected_action) {
                        effects.push(Effect::FinishOffer(offer));
                    }
                } else {
                    // Dismiss unnegotiated / Ask / Move drops on v3+ without finish() and clear hover.
                    effects.push(Effect::EnqueueStep { surface, token, step: FileStep::Leave });
                }
                effects.push(Effect::DestroyOffer(offer));
                effects.push(Effect::Flush);
                effects
            }
            State::Fetching { offer, version, selected_action, surface, token, pos, attempt, .. } => {
                if !can_deliver_drop(version, selected_action) {
                    let mut effects = Effects::two(Effect::CancelRead(attempt), Effect::DestroyOffer(offer));
                    effects.push(Effect::Flush);
                    return effects;
                }
                *state = State::Dropping {
                    offer,
                    version,
                    selected_action,
                    surface,
                    token,
                    pos,
                    attempt,
                    fallback_available: true,
                };
                Effects::none()
            }
            State::Deferred { offer, version, selected_action, surface, token, pos, .. } => {
                if !can_deliver_drop(version, selected_action) {
                    return Effects::two(Effect::DestroyOffer(offer), Effect::Flush);
                }
                let attempt = seq.next();
                *state = State::Dropping {
                    offer,
                    version,
                    selected_action,
                    surface,
                    token,
                    pos,
                    attempt,
                    fallback_available: false,
                };
                Effects::two(
                    Effect::StartRead { offer, attempt, phase: ReadPhase::Drop },
                    Effect::Flush,
                )
            }
            dropping @ State::Dropping { .. } => {
                *state = dropping;
                Effects::none()
            }
        },
        Event::OfferAction { offer, action } => {
            match state {
                State::Fetching { offer: id, selected_action, .. }
                | State::Deferred { offer: id, selected_action, .. }
                | State::Hovering { offer: id, selected_action, .. }
                | State::Dropping { offer: id, selected_action, .. }
                    if *id == offer =>
                {
                    *selected_action = action;
                }
                _ => {}
            }
            Effects::none()
        }
        Event::ReadComplete { attempt, outcome } => match std::mem::take(state) {
            State::Fetching {
                offer,
                serial,
                version,
                selected_action,
                surface,
                token,
                pos,
                attempt: active,
            } if active == attempt => match outcome {
                ReadOutcome::Bytes(bytes) if !bytes.is_empty() => {
                    let paths = parse_uri_list(&bytes);
                    if !paths.is_empty() {
                        *state = State::Hovering {
                            offer,
                            serial,
                            version,
                            selected_action,
                            surface,
                            token,
                            pos,
                            paths: paths.clone(),
                        };
                        Effects::one(Effect::EnqueueStep {
                            surface,
                            token,
                            step: FileStep::Enter(paths, Some(pos)),
                        })
                    } else {
                        let mut effects = Effects::none();
                        reject_offer(&mut effects, offer, serial, version);
                        *state = State::Rejected { offer };
                        effects
                    }
                }
                ReadOutcome::TooLarge => {
                    let mut effects = Effects::none();
                    reject_offer(&mut effects, offer, serial, version);
                    *state = State::Rejected { offer };
                    effects
                }
                ReadOutcome::Bytes(_) | ReadOutcome::TimedOut | ReadOutcome::Error => {
                    *state = State::Deferred { offer, serial, version, selected_action, surface, token, pos };
                    Effects::none()
                }
            },
            State::Dropping {
                offer,
                version,
                selected_action,
                surface,
                token,
                pos,
                attempt: active,
                fallback_available,
            } if active == attempt => match outcome {
                ReadOutcome::Bytes(bytes) if !bytes.is_empty() => {
                    let paths = parse_uri_list(&bytes);
                    let mut effects = Effects::none();
                    if !paths.is_empty() && can_deliver_drop(version, selected_action) {
                        effects.push(Effect::EnqueueStep {
                            surface,
                            token,
                            step: FileStep::Drop(paths, Some(pos)),
                        });
                        if should_finish(version, selected_action) {
                            effects.push(Effect::FinishOffer(offer));
                        }
                    }
                    effects.push(Effect::DestroyOffer(offer));
                    effects.push(Effect::Flush);
                    effects
                }
                ReadOutcome::Bytes(_) | ReadOutcome::TimedOut | ReadOutcome::Error
                    if fallback_available && can_deliver_drop(version, selected_action) =>
                {
                    let next_attempt = seq.next();
                    *state = State::Dropping {
                        offer,
                        version,
                        selected_action,
                        surface,
                        token,
                        pos,
                        attempt: next_attempt,
                        fallback_available: false,
                    };
                    Effects::two(
                        Effect::StartRead { offer, attempt: next_attempt, phase: ReadPhase::Drop },
                        Effect::Flush,
                    )
                }
                ReadOutcome::Bytes(_)
                | ReadOutcome::TimedOut
                | ReadOutcome::Error
                | ReadOutcome::TooLarge => Effects::two(Effect::DestroyOffer(offer), Effect::Flush),
            },
            other => {
                // Stale worker reply whose attempt no longer matches the active state.
                *state = other;
                Effects::none()
            }
        },
        Event::UnregisterWindow { surface, token } => match std::mem::take(state) {
            State::Fetching { offer, serial, version, attempt, surface: s, token: t, .. }
                if s == surface && t == token =>
            {
                // Reject rather than destroying mid-drag (wlroots cancels the source on mid-drag destroy).
                let mut effects = Effects::one(Effect::CancelRead(attempt));
                reject_offer(&mut effects, offer, serial, version);
                *state = State::Rejected { offer };
                effects
            }
            State::Deferred { offer, serial, version, surface: s, token: t, .. }
            | State::Hovering { offer, serial, version, surface: s, token: t, .. }
                if s == surface && t == token =>
            {
                let mut effects = Effects::none();
                reject_offer(&mut effects, offer, serial, version);
                *state = State::Rejected { offer };
                effects
            }
            State::Dropping { offer, attempt, surface: s, token: t, .. }
                if s == surface && t == token =>
            {
                let mut effects = Effects::two(Effect::CancelRead(attempt), Effect::DestroyOffer(offer));
                effects.push(Effect::Flush);
                effects
            }
            other => {
                *state = other;
                Effects::none()
            }
        },
        Event::Gone => {
            let mut effects = Effects::none();
            let old = std::mem::take(state);
            tear_down_active(&mut effects, old, false);
            effects
        }
    }
}

/// Registered Wayland window surfaces and their pending `FileStep` queues, keyed by `wl_surface`
/// object ID and guarded by a monotonic `WindowToken`.
#[derive(Default)]
pub(crate) struct WindowMap {
    next_token: u64,
    windows: HashMap<SurfaceId, WindowEntry>,
}

struct WindowEntry {
    token: WindowToken,
    steps: Vec<FileStep>,
}

impl WindowMap {
    /// Registers `surface` and returns its unique token. Replaces any stale entry for the same
    /// surface ID.
    pub(crate) fn register(&mut self, surface: SurfaceId) -> WindowToken {
        self.next_token = self.next_token.wrapping_add(1).max(1);
        let token = self.next_token;
        self.windows.insert(surface, WindowEntry { token, steps: Vec::new() });
        token
    }

    /// Removes `surface` only if its current token still matches `token`, so an older guard
    /// dropping late cannot remove a newer window reusing the same surface ID.
    pub(crate) fn unregister(&mut self, surface: SurfaceId, token: WindowToken) -> bool {
        if self.windows.get(&surface).is_some_and(|w| w.token == token) {
            self.windows.remove(&surface);
            true
        } else {
            false
        }
    }

    /// Returns the active token for `surface`, if registered.
    pub(crate) fn token(&self, surface: SurfaceId) -> Option<WindowToken> {
        self.windows.get(&surface).map(|w| w.token)
    }

    /// Appends `step` to `surface` if its token still matches, coalescing consecutive
    /// `FileStep::Move` events to keep only the latest point.
    pub(crate) fn push(&mut self, surface: SurfaceId, token: WindowToken, step: FileStep) -> bool {
        let Some(entry) = self.windows.get_mut(&surface).filter(|w| w.token == token) else {
            return false;
        };
        if let FileStep::Move(pos) = step
            && let Some(FileStep::Move(prev)) = entry.steps.last_mut()
        {
            *prev = pos;
            return true;
        }
        entry.steps.push(step);
        true
    }

    /// Drains pending `FileStep`s for `surface`.
    pub(crate) fn take(&mut self, surface: SurfaceId) -> Vec<FileStep> {
        self.windows.get_mut(&surface).map(|w| std::mem::take(&mut w.steps)).unwrap_or_default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

pub(crate) use super::uri::parse_uri_list;

/// Bounded, cancellable, deadline-enforced pipe read loop, kept cross-platform so timeouts,
/// size bounds, cancellation, and `EINTR`/`WouldBlock` retries run unit tests on every host.
pub(crate) mod pipe_read {
    use std::time::Duration;

    use super::ReadOutcome;
    use crate::platform::layer_shell_machine::configure_wait::timeout_ms;
    use crate::time::Instant;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Poll {
        Readable,
        Cancelled,
        TimedOut,
        Interrupted,
        Error,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Read {
        Bytes(usize),
        Eof,
        WouldBlock,
        Interrupted,
        Error,
    }

    pub(crate) trait Driver {
        fn now(&mut self) -> Instant;
        fn cancelled(&self) -> bool;
        fn poll(&mut self, timeout_ms: i32) -> Poll;
        fn read(&mut self, buf: &mut [u8]) -> Read;
    }

    /// Reads up to `max_bytes` from `driver` within `timeout`. Returns `None` when cancelled,
    /// `Some(ReadOutcome::TooLarge)` as soon as the peer sends byte `max_bytes + 1`, and
    /// `Some(ReadOutcome::TimedOut)` when `timeout` elapses before EOF.
    pub(crate) fn read_bounded<D: Driver>(
        driver: &mut D,
        max_bytes: usize,
        timeout: Duration,
    ) -> Option<ReadOutcome> {
        let deadline = driver.now() + timeout;
        let mut out = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if driver.cancelled() {
                return None;
            }
            let remaining = deadline.saturating_duration_since(driver.now());
            if remaining.is_zero() {
                return Some(ReadOutcome::TimedOut);
            }
            match driver.poll(timeout_ms(remaining)) {
                Poll::Cancelled => return None,
                Poll::Interrupted => continue,
                Poll::TimedOut => {
                    if driver.now() >= deadline {
                        return Some(ReadOutcome::TimedOut);
                    }
                    continue;
                }
                Poll::Error => return Some(ReadOutcome::Error),
                Poll::Readable => loop {
                    if driver.cancelled() {
                        return None;
                    }
                    let cap = (max_bytes - out.len()).saturating_add(1).min(chunk.len());
                    match driver.read(&mut chunk[..cap]) {
                        Read::Bytes(0) | Read::Eof => return Some(ReadOutcome::Bytes(out)),
                        Read::Bytes(n) => {
                            if out.len() + n > max_bytes {
                                return Some(ReadOutcome::TooLarge);
                            }
                            out.extend_from_slice(&chunk[..n]);
                        }
                        Read::WouldBlock => break,
                        Read::Interrupted => continue,
                        Read::Error => return Some(ReadOutcome::Error),
                    }
                },
            }
        }
    }
}
