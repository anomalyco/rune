//! An opt-in, reversible quit. Install before claiming the instance and running its event loop.
//! `begin` starts a save of the current revision without closing the store. Only `confirmed`
//! authorizes irreversible work. Cancelling the intent to exit does not cancel a disk write.

use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use super::After;
use crate::machine::{Attempt, Effects, Seq};

/// Callbacks run on the UI thread and must not wait for a worker. `overdue` presents or updates
/// one app-owned surface describing what is unconfirmed. Waiting is the default. Only a user's
/// explicit choice on that surface should call [`Ticket::quit_anyway`]. `resumed` dismisses it.
pub struct Hooks {
    pub begin: fn(Ticket),
    /// On the UI loop, atomically compare the durable revision with the last admitted edit
    /// and close admission if they match. Never wait for a lock, producer or disk. On false,
    /// arrange another receipt; the retained hosts and original overdue deadline stay live.
    /// A cancelled session-end query calls `resumed` to reopen even a successful seal.
    pub try_seal: fn(Ticket, u64) -> bool,
    pub overdue: fn(Ticket),
    pub resumed: fn(Ticket),
    pub confirmed: fn(Ticket),
    /// Localized, short, non-sensitive shutdown-block reason (also used by the app's surface).
    pub shutdown_reason: fn() -> String,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();
type Post = dyn Fn(Message) -> bool + Send + Sync;
static POST: OnceLock<Box<Post>> = OnceLock::new();

/// Register once. An application which never registers keeps its usual quit policy.
pub fn install(hooks: Hooks) -> Result<(), Hooks> {
    HOOKS.set(hooks)
}

/// A timeout is not evidence that the original process is dead. In `Unresponsive` the new app
/// may display an unavailable/read-only window but must not perform any persistent writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    Primary,
    Delivered,
    LegacyDelivery,
    Unresponsive,
}

/// Claim, or ask the existing UI loop to resume. macOS normally receives Dock and Finder
/// reopen events directly from AppKit. Explicitly starting another executable there does not
/// replace an app's file lock.
pub fn claim(id: &str) -> Claim {
    if hooks().is_none() {
        return Claim::Unresponsive;
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        crate::open_url::instance::ui_claim(id)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = id;
        Claim::Primary
    }
}
pub(crate) fn hooks() -> Option<&'static Hooks> {
    HOOKS.get()
}
pub(crate) fn connect(post: impl Fn(Message) -> bool + Send + Sync + 'static) {
    let _ = POST.set(Box::new(post));
}
fn post(message: Message) -> bool {
    POST.get().is_some_and(|f| f(message))
}

#[cfg(target_os = "windows")]
thread_local! { static FINISHED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(target_os = "windows")]
pub(super) fn terminal() -> bool {
    FINISHED.get()
}
#[cfg(target_os = "windows")]
pub(crate) fn resolved(confirmed: bool) {
    FINISHED.set(confirmed);
    super::win32::clear_reason();
}
#[cfg(target_os = "windows")]
pub(super) fn cancel_system_query() {
    post(Message::Event(Event::CancelSession));
}
#[cfg(target_os = "windows")]
pub(super) fn system_query() {
    post(Message::Event(Event::SystemQuery(Instant::now())));
}
#[cfg(target_os = "windows")]
pub(super) fn session_ends() {
    post(Message::Event(Event::EndSession));
}

/// A save attempt, safe to send back from a writer. Messages for an abandoned attempt do nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket(Attempt);
impl Ticket {
    /// The writer has made edits through `revision` durable. Posting this receipt is not
    /// permission to exit: the live attempt must still pass [`Hooks::try_seal`] on the loop.
    pub fn saved_through(self, revision: u64) {
        post(Message::Event(Event::Receipt(self, revision)));
    }
    /// Saving failed. Present the overdue surface without waiting for its clock.
    pub fn failed(self) {
        post(Message::Event(Event::Failed(self)));
    }
    pub fn quit_anyway(self) {
        post(Message::Event(Event::Force(self)));
    }
}

pub(crate) const ACK_WITHIN: Duration = Duration::from_millis(250);
const SAVE_WITHIN: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) enum State {
    Running,
    Quitting {
        k: Ticket,
        due: Instant,
        after: Intent,
    },
    Saving {
        k: Ticket,
        due: Instant,
        after: Intent,
    },
    Overdue {
        k: Ticket,
        after: Intent,
    },
    #[cfg(target_os = "windows")]
    AwaitSessionEnd(Ticket),
    Resumed,
    Confirmed,
    Forced,
}
impl Default for State {
    fn default() -> Self {
        Self::Running
    }
}
impl State {
    pub(crate) fn accepts_activation(&self) -> bool {
        matches!(self, Self::Running | Self::Resumed | Self::Quitting { .. } | Self::Saving { .. } | Self::Overdue { .. })
    }
    pub(crate) fn due(&self) -> Option<(Ticket, Instant)> {
        match *self {
            Self::Quitting { k, due, .. } | Self::Saving { k, due, .. } => Some((k, due)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Event {
    Quit(Instant, After),
    #[cfg(target_os = "windows")]
    SystemQuery(Instant),
    #[cfg(target_os = "windows")]
    CancelSession,
    #[cfg(target_os = "windows")]
    EndSession,
    Prepared(Ticket),
    Due(Ticket, Instant),
    Failed(Ticket),
    Receipt(Ticket, u64),
    Sealed(Ticket),
    Reopen,
    Force(Ticket),
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Intent {
    User(After),
    #[cfg(target_os = "windows")]
    System,
}
#[derive(Debug)]
pub(crate) enum Effect {
    HideAndPrepare(Ticket),
    Status(Ticket),
    CheckRevision(Ticket, u64),
    Restore(Ticket),
    Finalize(Ticket, After),
    #[cfg(target_os = "windows")]
    Consent,
    #[cfg(target_os = "windows")]
    SessionExit,
    ForceExit,
}

pub(crate) fn step(state: &mut State, seq: &mut Seq, event: Event) -> Effects<Effect> {
    match (&*state, event) {
        (State::Running | State::Resumed, Event::Quit(now, after)) => {
            let k = Ticket(seq.next());
            *state = State::Quitting { k, after: Intent::User(after), due: now + SAVE_WITHIN };
            Effects::one(Effect::HideAndPrepare(k))
        }
        #[cfg(target_os = "windows")]
        (State::Running | State::Resumed, Event::SystemQuery(now)) => {
            let k = Ticket(seq.next());
            *state = State::Quitting { k, due: now + SAVE_WITHIN, after: Intent::System };
            Effects::one(Effect::HideAndPrepare(k))
        }
        #[cfg(target_os = "windows")]
        (State::Saving { k, after: Intent::System, .. } | State::Overdue { k, after: Intent::System }, Event::Sealed(t)) if *k == t => {
            *state = State::AwaitSessionEnd(t);
            Effects::one(Effect::Consent)
        }
        #[cfg(target_os = "windows")]
        (
            State::AwaitSessionEnd(k)
            | State::Quitting { k, after: Intent::System, .. }
            | State::Saving { k, after: Intent::System, .. }
            | State::Overdue { k, after: Intent::System },
            Event::CancelSession,
        ) => {
            let k = *k;
            *state = State::Resumed;
            Effects::one(Effect::Restore(k))
        }
        #[cfg(target_os = "windows")]
        (State::AwaitSessionEnd(_), Event::EndSession) => {
            *state = State::Confirmed;
            Effects::one(Effect::SessionExit)
        }
        (State::Quitting { k, due, after }, Event::Prepared(t)) if *k == t => {
            *state = State::Saving { k: t, due: *due, after: *after };
            Effects::none()
        }
        (State::Saving { k, due, after }, Event::Due(t, now)) if *k == t && now >= *due => {
            *state = State::Overdue { k: t, after: *after };
            Effects::one(Effect::Status(t))
        }
        (State::Saving { k, after, .. }, Event::Failed(t)) if *k == t => {
            *state = State::Overdue { k: t, after: *after };
            Effects::one(Effect::Status(t))
        }
        (State::Quitting { k, .. } | State::Saving { k, .. } | State::Overdue { k, .. }, Event::Reopen) => {
            let k = *k;
            *state = State::Resumed;
            Effects::one(Effect::Restore(k))
        }
        // A stale attempt never even asks the app to seal. A valid attempt with an obsolete
        // revision stays reversible; another save does not reset the two-second clock.
        (State::Saving { k, .. } | State::Overdue { k, .. }, Event::Receipt(t, rev)) if *k == t => Effects::one(Effect::CheckRevision(t, rev)),
        (State::Saving { k, after: Intent::User(after), .. } | State::Overdue { k, after: Intent::User(after) }, Event::Sealed(t)) if *k == t => {
            let after = *after;
            *state = State::Confirmed;
            Effects::one(Effect::Finalize(t, after))
        }
        (State::Overdue { k, .. }, Event::Force(t)) if *k == t => {
            *state = State::Forced;
            Effects::one(Effect::ForceExit)
        }
        _ => Effects::none(),
    }
}

pub(crate) enum Message {
    Event(Event),
    Close(usize),
    Activate(Arc<Activation>),
    #[cfg(target_os = "macos")]
    Link(String),
}
#[cfg(target_os = "macos")]
pub(crate) fn appkit_link(url: String) {
    if let Some(p) = POST.get() {
        p(Message::Link(url));
    } else {
        crate::open_url::incoming::receive(url);
    }
}
pub(crate) fn request(after: After) -> bool {
    hooks().is_some() && post(Message::Event(Event::Quit(Instant::now(), after)))
}
pub(crate) fn close(slot: usize) -> bool {
    hooks().is_some() && post(Message::Close(slot))
}

/// A listener waking a UI thread isn't an acknowledgement. The loop checks the absolute expiry
/// before using the launch's arguments. Its reply is sent after issuing the native show.
pub(crate) struct Activation {
    pub args: Vec<String>,
    pub until: Instant,
    reply: mpsc::SyncSender<bool>,
}
impl Activation {
    pub(crate) fn answer(&self, served: bool) {
        let _ = self.reply.try_send(served);
    }
}
pub(crate) fn activate(args: Vec<String>, until: Instant) -> bool {
    let (reply, received) = mpsc::sync_channel(1);
    until > Instant::now()
        && post(Message::Activate(Arc::new(Activation { args, until, reply })))
        && received.recv_timeout(until.saturating_duration_since(Instant::now())) == Ok(true)
}
