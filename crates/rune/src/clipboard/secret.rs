//! A secret copy's life, as two machines. A [`Secret`] is the copy itself, shared by every clone
//! of its write: what it reports, and whether `clear_now` ran. A [`Hold`] is one clipboard's
//! record of the secret it wrote last, so a clone written to two clipboards is cleaned up on each
//! and neither's report can disarm the other.
use std::sync::{Arc, Mutex, OnceLock};
use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;
use super::{Notify, Request, Route, lock};

/// What became of a secret copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SecretState {
    /// Not written yet.
    #[default]
    Pending,
    /// On the clipboard: the last copy Rune wrote, and the clipboard hasn't changed since.
    Holding,
    /// Cleared by Rune: its delay ran out, or `clear_now` or `Tx::clear_secret_clipboard` ran.
    Cleared,
    /// Something else took its place: a later copy, a change by another app, or a newer copy
    /// before this one was written. Its countdown should end quietly.
    Replaced,
    /// The write failed; the clipboard didn't change.
    Failed(String),
}

/// A secret copy: the last thing reported about it, whether it may be written again, and how many
/// clipboards have a [`Hold`] on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Secret {
    /// `clear_now` hasn't run: a clone of its write can still be written, even after it expired.
    Live { report: SecretState, holds: u32 },
    /// `clear_now` ran: no clone of its write is written again, whatever it last reported.
    Cancelled { report: SecretState, holds: u32 },
}
impl Default for Secret {
    fn default() -> Self { Self::Live { report: SecretState::Pending, holds: 0 } }
}
impl Secret {
    fn report(&self) -> &SecretState {
        let (Self::Live { report, .. } | Self::Cancelled { report, .. }) = self;
        report
    }
}

#[derive(Debug)]
pub(super) enum SecretEvent {
    /// `clear_now` ran.
    Cancel,
    /// A clipboard took a hold on this copy.
    Held,
    /// A clipboard's hold ended, with this outcome.
    Released(SecretState),
    /// A write of this copy failed; the latest report stands.
    Report(SecretState),
    /// The last clone of its write was dropped.
    Dropped,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SecretEffect {
    /// Ask the clipboard this copy was first written to for a clear.
    Clear,
}

pub(super) fn step(secret: &mut Secret, event: SecretEvent) -> Effects<SecretEffect> {
    match event {
        SecretEvent::Cancel => {
            let (Secret::Live { report, holds } | Secret::Cancelled { report, holds }) = std::mem::take(secret);
            // A failed replay reports `Failed` while its earlier write is still on the clipboard.
            let held = report == SecretState::Holding || (matches!(report, SecretState::Failed(_)) && holds > 0);
            *secret = Secret::Cancelled { report, holds };
            if held { Effects::one(SecretEffect::Clear) } else { Effects::none() }
        }
        SecretEvent::Held => {
            let (Secret::Live { report, holds } | Secret::Cancelled { report, holds }) = secret;
            *report = SecretState::Holding;
            *holds += 1;
            Effects::none()
        }
        SecretEvent::Released(next) => {
            let (Secret::Live { report, holds } | Secret::Cancelled { report, holds }) = secret;
            *report = next;
            *holds = holds.saturating_sub(1);
            Effects::none()
        }
        SecretEvent::Report(next) => {
            let (Secret::Live { report, .. } | Secret::Cancelled { report, .. }) = secret;
            *report = next;
            Effects::none()
        }
        SecretEvent::Dropped => {
            match secret {
                Secret::Live { report: report @ SecretState::Pending, .. } => *report = SecretState::Replaced,
                Secret::Cancelled { report: report @ SecretState::Pending, .. } => *report = SecretState::Cleared,
                _ => {}
            }
            Effects::none()
        }
    }
}

#[derive(Default)]
pub(super) struct Slot {
    secret: Mutex<Secret>,
    pub(super) route: OnceLock<Route>,
}
impl Slot {
    fn step(&self, event: SecretEvent) -> Effects<SecretEffect> { step(&mut lock(&self.secret), event) }
    pub(super) fn set(&self, state: SecretState) { self.step(SecretEvent::Report(state)); }
    pub(super) fn failed(&self) -> bool { matches!(lock(&self.secret).report(), SecretState::Failed(_)) }
    pub(super) fn held(&self) { self.step(SecretEvent::Held); }
    pub(super) fn released(&self, state: SecretState) { self.step(SecretEvent::Released(state)); }
    pub(super) fn cancelled(&self) -> bool { matches!(*lock(&self.secret), Secret::Cancelled { .. }) }
    fn state(&self) -> SecretState { lock(&self.secret).report().clone() }
}

/// Settles a secret write that was dropped before reaching a clipboard (replaced in its
/// handler, cancelled, or left in a headless runtime with no clipboard).
pub(super) struct Unwritten(pub(super) Arc<Slot>);
impl Drop for Unwritten {
    fn drop(&mut self) { self.0.step(SecretEvent::Dropped); }
}

/// A secret copy from `Tx::copy_secret`. It holds no text, and is cheap to clone and send.
/// Its state is updated by the clipboard worker; asking never reads the OS clipboard.
#[derive(Clone)]
pub struct SecretCopy(pub(super) Arc<Slot>);
impl SecretCopy {
    /// A copy that is never written and stays `Pending`, for scenes and tests that show a copy
    /// in flight without a `Tx`.
    pub fn pending() -> Self { Self(Arc::default()) }
    pub fn state(&self) -> SecretState {
        if let Some(Route::Memory(m)) = self.0.route.get() { m.tick(); }
        self.0.state()
    }
    /// Still the last copy Rune wrote, and the clipboard hasn't changed since. The OS
    /// clipboard is checked a few times a second while a secret is held.
    pub fn holds(&self) -> bool { self.state() == SecretState::Holding }
    /// Clear the clipboard now if this copy still holds it, on the clipboard worker. A copy not
    /// yet written is never written. Idempotent.
    pub fn clear_now(&self) {
        for effect in self.0.step(SecretEvent::Cancel) {
            match effect {
                SecretEffect::Clear => {
                    if let Some(route) = self.0.route.get() { route.send(Request::Clear(Some(self.0.clone()))); }
                }
            }
        }
    }
}
impl std::fmt::Debug for SecretCopy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SecretCopy").field(&self.0.state()).finish()
    }
}

/// A timed clear the clipboard's owner is waiting on. It only counts while its attempt is the
/// one the [`Hold`] still has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Alarm {
    pub(super) attempt: Attempt,
    pub(super) due: Instant,
}

/// How a clipboard answered a wipe of Rune's secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wipe {
    /// It held the secret and doesn't any more.
    Done,
    /// Something else is on it now; it was left alone.
    NotOurs,
    /// It couldn't be read or wiped just now: the secret may still be on it.
    Failed,
}

/// One clipboard's record of the secret Rune wrote to it last.
#[derive(Default)]
pub(super) enum Hold {
    /// Nothing of ours to clean up.
    #[default]
    Free,
    /// The clipboard may still hold this secret.
    Holding {
        attempt: Attempt,
        slot: Arc<Slot>,
        notify: Option<Notify>,
        /// The clipboard's change count from the write, where it has one.
        revision: Option<u64>,
        /// Who the OS says put the write there (an X11 selection owner), where it says.
        writer: Option<u64>,
        /// A keyed hash of the text, so the text needn't stay in memory until the clear.
        print: u64,
    },
}
impl Hold {
    pub(super) fn is_free(&self) -> bool { matches!(self, Self::Free) }
    /// Whether a clear of `which`, or of whichever secret is held, applies here.
    pub(super) fn answers(&self, which: Option<&Arc<Slot>>) -> bool {
        match self {
            Self::Free => false,
            Self::Holding { slot, .. } => which.is_none_or(|w| Arc::ptr_eq(slot, w)),
        }
    }
    pub(super) fn armed(&self, alarm: Attempt) -> bool {
        matches!(self, Self::Holding { attempt, .. } if *attempt == alarm)
    }
}

pub(super) enum HoldEvent {
    /// A secret reached the clipboard and has been reported as holding it.
    Written { slot: Arc<Slot>, notify: Option<Notify>, due: Option<Instant>, revision: Option<u64>, writer: Option<u64>, print: u64 },
    /// A later write took its place, or the clipboard changed under it.
    Lost,
    /// A clear was asked for. A `patient` one leaves a secret that turns out to be gone for the
    /// periodic check to settle, as if it had never been asked.
    Clear { patient: bool },
    /// An alarm came due at `now`.
    Due { attempt: Attempt, now: Instant },
    /// The clipboard's answer to a wipe this hold asked for. A failed one is tried again at
    /// `again`, if it was the delay's.
    Swept { wipe: Wipe, patient: bool, again: Option<Instant> },
}

pub(super) enum HoldEffect {
    /// Wipe the clipboard if it still holds the secret, and answer with [`HoldEvent::Swept`]. A
    /// wipe the delay asked for at `expiring` is tried again a watch later if it fails.
    WipeIfOurs { patient: bool, expiring: Option<Instant> },
    Release(Arc<Slot>, SecretState),
    Notify(Notify),
    /// Replace the alarm being waited on: a new hold never inherits the last one's.
    Arm(Option<Alarm>),
}

pub(super) fn step_hold(seq: &mut Seq, hold: &mut Hold, event: HoldEvent) -> Effects<HoldEffect> {
    match event {
        HoldEvent::Written { slot, notify, due, revision, writer, print } => {
            let mut effects = release(hold, SecretState::Replaced);
            let attempt = seq.next();
            *hold = Hold::Holding { attempt, slot, notify: notify.clone(), revision, writer, print };
            effects.push(HoldEffect::Arm(due.map(|due| Alarm { attempt, due })));
            if let Some(notify) = notify { effects.push(HoldEffect::Notify(notify)); }
            effects
        }
        HoldEvent::Lost => release(hold, SecretState::Replaced),
        HoldEvent::Clear { patient } if !hold.is_free() => Effects::one(HoldEffect::WipeIfOurs { patient, expiring: None }),
        HoldEvent::Due { attempt, now } if hold.armed(attempt) => Effects::one(HoldEffect::WipeIfOurs { patient: false, expiring: Some(now) }),
        HoldEvent::Clear { .. } | HoldEvent::Due { .. } => Effects::none(),
        HoldEvent::Swept { wipe: Wipe::Done, .. } => release(hold, SecretState::Cleared),
        HoldEvent::Swept { wipe: Wipe::NotOurs, patient: false, .. } => release(hold, SecretState::Replaced),
        HoldEvent::Swept { wipe: Wipe::NotOurs, patient: true, .. } => Effects::none(),
        // The hold and its delay stay, so the delay still clears the secret; once the delay
        // has run out, a retry takes its place.
        HoldEvent::Swept { wipe: Wipe::Failed, again, .. } => match (hold, again) {
            (Hold::Holding { attempt, .. }, Some(due)) => Effects::one(HoldEffect::Arm(Some(Alarm { attempt: *attempt, due }))),
            _ => Effects::none(),
        },
    }
}

fn release(hold: &mut Hold, state: SecretState) -> Effects<HoldEffect> {
    let Hold::Holding { slot, notify, .. } = std::mem::take(hold) else { return Effects::none() };
    let mut effects = Effects::one(HoldEffect::Release(slot, state));
    if let Some(notify) = notify { effects.push(HoldEffect::Notify(notify)); }
    effects
}
