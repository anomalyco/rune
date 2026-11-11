//! A secret copy's life, as two machines. A [`Secret`] is the copy itself, shared by every clone
//! of its write: what it reports, and whether `clear_now` ran. A [`Hold`] is one clipboard's
//! record of the secret it wrote last, so a clone written to two clipboards is cleaned up on each
//! and neither's report can disarm the other.
use std::sync::{Arc, Mutex, OnceLock, Weak};
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
    /// The write failed, and the clipboard didn't change; or the clipboard refused Rune's wipe,
    /// and may still hold the secret.
    Failed(String),
}

/// A secret copy: the last thing reported about it, whether it may be written again, and how many
/// clipboards have a [`Hold`] on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Secret {
    /// `clear_now` hasn't run: a clone of its write can still be written, even after it expired.
    Live { report: Report, holds: u32 },
    /// `clear_now` ran: no clone of its write is written again, whatever it last reported.
    Cancelled { report: Report, holds: u32 },
}
impl Default for Secret {
    fn default() -> Self { Self::Live { report: Report::Said(SecretState::Pending), holds: 0 } }
}
impl Secret {
    fn report(&self) -> &Report {
        let (Self::Live { report, .. } | Self::Cancelled { report, .. }) = self;
        report
    }
}

/// What a secret copy last reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Report {
    Said(SecretState),
    /// The clipboard refused Rune's wipe. Its handle says `Failed`, but this is no failed write:
    /// nothing is left for `clear_now` to recover.
    Refused,
}
impl Report {
    fn shown(&self) -> SecretState {
        match self {
            Self::Said(state) => state.clone(),
            Self::Refused => SecretState::Failed(REFUSED.into()),
        }
    }
    fn failed_write(&self) -> bool { matches!(self, Self::Said(SecretState::Failed(_))) }
}

/// What a secret reports when the clipboard refused Rune's wipe.
pub(super) const REFUSED: &str = "the clipboard refused the wipe";

#[derive(Debug)]
pub(super) enum SecretEvent {
    /// `clear_now` ran.
    Cancel,
    /// A clipboard took a hold on this copy.
    Held,
    /// A clipboard's hold ended, with this outcome.
    Released(Report),
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
            let held = report == Report::Said(SecretState::Holding) || (report.failed_write() && holds > 0);
            *secret = Secret::Cancelled { report, holds };
            if held { Effects::one(SecretEffect::Clear) } else { Effects::none() }
        }
        SecretEvent::Held => {
            let (Secret::Live { report, holds } | Secret::Cancelled { report, holds }) = secret;
            *report = Report::Said(SecretState::Holding);
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
            *report = Report::Said(next);
            Effects::none()
        }
        SecretEvent::Dropped => {
            match secret {
                Secret::Live { report: report @ Report::Said(SecretState::Pending), .. } => *report = Report::Said(SecretState::Replaced),
                Secret::Cancelled { report: report @ Report::Said(SecretState::Pending), .. } => *report = Report::Said(SecretState::Cleared),
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
    /// A write of it failed: a refused wipe isn't one.
    pub(super) fn failed(&self) -> bool { lock(&self.secret).report().failed_write() }
    pub(super) fn held(&self) { self.step(SecretEvent::Held); }
    pub(super) fn released(&self, report: Report) { self.step(SecretEvent::Released(report)); }
    pub(super) fn cancelled(&self) -> bool { matches!(*lock(&self.secret), Secret::Cancelled { .. }) }
    fn state(&self) -> SecretState { lock(&self.secret).report().shown() }
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

/// One clipboard's record of the secret Rune wrote to it last.
#[derive(Default)]
pub(super) enum Hold {
    /// Nothing of ours to clean up.
    #[default]
    Free,
    /// The clipboard may still hold this secret.
    Holding {
        attempt: Attempt,
        /// The slot owns the first clipboard's route. A hold must wipe even if nobody
        /// is left to receive its report, but must not keep that clipboard alive.
        slot: Weak<Slot>,
        notify: Option<Notify>,
        /// `None` only when the delay is too long for the clock to represent.
        due: Option<Instant>,
        /// The clipboard's change count after the write, where it has one.
        revision: Option<u64>,
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
            Self::Holding { slot, .. } => which.is_none_or(|w| std::ptr::eq(slot.as_ptr(), Arc::as_ptr(w))),
        }
    }
    pub(super) fn armed(&self, alarm: Attempt) -> bool {
        matches!(self, Self::Holding { attempt, .. } if *attempt == alarm)
    }
    pub(super) fn next_check(&self, watch: Instant) -> Option<Instant> {
        let Self::Holding { due, .. } = self else { return None };
        Some(due.map_or(watch, |d| d.min(watch)))
    }
}

pub(super) enum HoldEvent {
    /// A secret reached the clipboard and has been reported as holding it.
    Written { slot: Arc<Slot>, notify: Option<Notify>, due: Option<Instant>, revision: Option<u64>, print: u64 },
    /// A later write took its place, or the clipboard changed under it.
    Lost,
    /// A clear was asked for; `ours` says whether the clipboard still has what Rune wrote.
    Clear { ours: bool },
    /// The caller wiped the clipboard itself and confirmed it.
    Wiped,
    /// An alarm came due.
    Due { attempt: Attempt, ours: bool },
    /// The answer to [`HoldEffect::Wipe`]: whether the clipboard took it.
    Swept { ok: bool },
}

pub(super) enum HoldEffect {
    /// Wipe the clipboard, and answer with [`HoldEvent::Swept`].
    Wipe,
    Release(Arc<Slot>, Report),
    Notify(Notify),
    /// Replace the alarm being waited on: a new hold never inherits the last one's.
    Arm(Option<Alarm>),
}

pub(super) fn step_hold(seq: &mut Seq, hold: &mut Hold, event: HoldEvent) -> Effects<HoldEffect> {
    match event {
        HoldEvent::Written { slot, notify, due, revision, print } => {
            let mut effects = release(hold, said(SecretState::Replaced));
            let attempt = seq.next();
            *hold = Hold::Holding { attempt, slot: Arc::downgrade(&slot), notify: notify.clone(), due, revision, print };
            effects.push(HoldEffect::Arm(due.map(|due| Alarm { attempt, due })));
            if let Some(notify) = notify { effects.push(HoldEffect::Notify(notify)); }
            effects
        }
        HoldEvent::Lost => release(hold, said(SecretState::Replaced)),
        HoldEvent::Clear { ours } => settle(hold, ours),
        HoldEvent::Wiped => release(hold, said(SecretState::Cleared)),
        HoldEvent::Due { attempt, ours } if hold.armed(attempt) => settle(hold, ours),
        HoldEvent::Due { .. } => Effects::none(),
        HoldEvent::Swept { ok: true } => release(hold, said(SecretState::Cleared)),
        // Final: trying again could erase a copy another app made after the check.
        HoldEvent::Swept { ok: false } => release(hold, Report::Refused),
    }
}

/// Ends a hold on a clear: wipes only what is still ours.
fn settle(hold: &mut Hold, ours: bool) -> Effects<HoldEffect> {
    if hold.is_free() || !ours { return release(hold, said(SecretState::Replaced)); }
    Effects::one(HoldEffect::Wipe)
}

fn said(state: SecretState) -> Report { Report::Said(state) }

fn release(hold: &mut Hold, report: Report) -> Effects<HoldEffect> {
    let Hold::Holding { slot, notify, .. } = std::mem::take(hold) else { return Effects::none() };
    let mut effects = Effects::none();
    if let Some(slot) = slot.upgrade() { effects.push(HoldEffect::Release(slot, report)); }
    if let Some(notify) = notify { effects.push(HoldEffect::Notify(notify)); }
    effects
}
