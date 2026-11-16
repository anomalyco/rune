//! An opt-in, reversible quit. Install before claiming the instance and running its event loop.
//! `begin` starts a save of the current revision without closing the store. Only `confirmed`
//! authorizes irreversible work. Cancelling the intent to exit does not cancel a disk write.

use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use super::After;
use crate::machine::{Attempt, Effects, Seq};

#[cfg(windows)]
pub(crate) mod session;

/// An affirmative query may still be cancelled. Only `Exit` closes write admission.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Seal { Query, Exit }

/// Callbacks run on the UI thread and must not wait for a worker. `overdue` presents or updates
/// one app-owned surface describing what is unconfirmed. Waiting is the default. Only a user's
/// explicit choice on that surface should call [`Ticket::quit_anyway`]. `resumed` dismisses it.
pub struct Hooks {
    pub begin: fn(Ticket),
    /// Non-link activation data delivered to the owner. The UI thread must not wait for
    /// platform or application work here. Rune delivers links separately via `on_open_url`.
    pub activated: fn(&[String]),
    /// On the UI loop, atomically compare the durable revision with the last admitted edit
    /// and close admission if they match. Never wait for a lock, producer or disk. On false,
    /// arrange another receipt; the retained hosts and original overdue deadline stay live.
    /// A cancelled session-end query calls `resumed` to reopen even a successful seal.
    pub try_seal: fn(Ticket, u64, Seal) -> bool,
    /// Return the status host. While overdue its native Close means Keep waiting, never
    /// discard. Use `status_key` for a new host so an older dismissal cannot address it.
    pub overdue: fn(Ticket) -> crate::WindowHandle,
    pub resumed: fn(Ticket),
    pub confirmed: fn(Ticket),
    /// Localized, short, non-sensitive shutdown-block reason (also used by the app's surface).
    pub shutdown_reason: fn() -> String,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();
type Post = dyn Fn(Message) -> bool + Send + Sync;
static POST: OnceLock<Box<Post>> = OnceLock::new();
// Visible to the instance listener before irreversible teardown (including a force).
static EXITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn commit_exit() {
    EXITING.store(true, std::sync::atomic::Ordering::Release);
    #[cfg(windows)]
    crate::open_url::instance::announce_exit();
}
pub(crate) fn exiting() -> bool { EXITING.load(std::sync::atomic::Ordering::Acquire) }

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
    /// The owner announced an irreversible exit but still held its claim at the
    /// handoff deadline. Not a silent UI, not permission to start any services.
    StillClosing,
    Unresponsive,
}

/// Claim, or ask the existing UI loop to resume. macOS normally receives Dock and Finder
/// reopen events directly from AppKit. Explicitly starting another executable there does not
/// replace an app's file lock.
pub fn claim(id: &str) -> Claim {
    if hooks().is_none() {
        return Claim::Unresponsive;
    }
    crate::relaunch::wait_for_previous();
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
pub(super) fn session_ends() {
    post(Message::Event(Event::EndSession));
}

/// A save attempt, safe to send back from a writer. Messages for an abandoned attempt do nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket(Attempt);
impl Ticket {
    pub fn status_key(self) -> String { format!("quit-status-{:?}", self.0) }
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
    /// Withdraw this particular exit intent without replaying an external activation.
    pub fn return_to_workspace(self) { post(Message::Event(Event::Return(self))); }
    /// A read-only or untouched volatile workspace has no persistence obligation. The app
    /// must still validate and seal the exact revision on the UI thread. This disposition
    /// never calls `confirmed`, authorizes an updater or claims that data reached disk.
    pub fn nothing_to_save(self, revision:u64) { post(Message::Event(Event::Empty(self,revision))); }
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
    AwaitSessionEnd(Ticket, u64),
    Resumed,
    Confirmed,
    LeftEmpty,
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
    Sealed(Ticket, u64),
    #[cfg(windows)] LateSystemSeal(Ticket),
    Empty(Ticket,u64),
    EmptySealed(Ticket,u64),
    Return(Ticket),
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
    CheckEmpty(Ticket,u64),
    ExitEmpty,
    Restore(Ticket),
    Finalize(Ticket, After),
    #[cfg(target_os = "windows")]
    Consent,
    #[cfg(target_os = "windows")]
    SessionExit(Ticket,u64),
    #[cfg(windows)] LeaveSystem,
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
        (State::Quitting { .. } | State::Saving { .. } | State::Overdue { .. }, Event::SystemQuery(_)) => {
            match state { State::Quitting{after,..}|State::Saving{after,..}|State::Overdue{after,..}=>*after=Intent::System,_=>unreachable!() }
            Effects::none()
        }
        #[cfg(windows)]
        (State::Saving { k, after: Intent::System, .. } | State::Overdue { k, after: Intent::System }, Event::LateSystemSeal(t)) if *k==t => {
            *state=State::Confirmed; Effects::one(Effect::LeaveSystem)
        }
        #[cfg(windows)]
        (State::Saving { k, after: Intent::System, .. } | State::Overdue { k, after: Intent::System }, Event::Sealed(t,r)|Event::EmptySealed(t,r)) if *k == t => {
            *state = State::AwaitSessionEnd(t,r);
            Effects::one(Effect::Consent)
        }
        #[cfg(target_os = "windows")]
        (
            State::AwaitSessionEnd(k,_)
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
        (State::AwaitSessionEnd(k,r), Event::EndSession) => {
            let fx=Effect::SessionExit(*k,*r);
            *state = State::Confirmed;
            Effects::one(fx)
        }
        (State::Quitting { k, due, after }, Event::Prepared(t)) if *k == t => {
            *state = State::Saving { k: t, due: *due, after: *after };
            Effects::none()
        }
        (State::Quitting { after: Intent::User(was), .. } | State::Saving { after: Intent::User(was), .. } | State::Overdue { after: Intent::User(was), .. }, Event::Quit(_, next)) if next > *was => {
            match state { State::Quitting { after, .. } | State::Saving { after, .. } | State::Overdue { after, .. } => *after=Intent::User(next), _ => unreachable!() }
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
        (State::Quitting { k, .. } | State::Saving { k, .. } | State::Overdue { k, .. }, Event::Return(t)) if *k==t => {
            *state=State::Resumed; Effects::one(Effect::Restore(t))
        }
        (State::Saving { k, .. } | State::Overdue { k, .. }, Event::Empty(t,r)) if *k==t => Effects::one(Effect::CheckEmpty(t,r)),
        (State::Saving { k, after:Intent::User(_), .. } | State::Overdue { k, after:Intent::User(_) }, Event::EmptySealed(t,_)) if *k==t => {
            *state=State::LeftEmpty; Effects::one(Effect::ExitEmpty)
        }
        (State::Quitting { k, .. } | State::Saving { k, .. } | State::Overdue { k, .. }, Event::Reopen) => {
            let k = *k;
            *state = State::Resumed;
            Effects::one(Effect::Restore(k))
        }
        // A stale attempt never even asks the app to seal. A valid attempt with an obsolete
        // revision stays reversible; another save does not reset the two-second clock.
        (State::Saving { k, .. } | State::Overdue { k, .. }, Event::Receipt(t, rev)) if *k == t => Effects::one(Effect::CheckRevision(t, rev)),
        (State::Saving { k, after: Intent::User(after), .. } | State::Overdue { k, after: Intent::User(after) }, Event::Sealed(t,_)) if *k == t => {
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
    #[cfg(windows)] SessionQuery(u64, Instant),
    #[cfg(windows)] SessionCancel(u64),
    Legacy(Vec<String>),
    Event(Event),
    Close(usize),
    Activate(Arc<Activation>),
    #[cfg(target_os = "macos")]
    Link(String),
}

#[cfg(any(windows,target_os="linux"))]
pub(crate) fn legacy_activation(args:&[String]) -> bool {
    hooks().is_some() && !exiting() && post(Message::Legacy(args.to_vec()))
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
    pub claim: Box<dyn Fn()->bool + Send + Sync>,
    reply: mpsc::SyncSender<bool>,
}
impl Activation {
    pub(crate) fn answer(&self, served: bool) {
        let _ = self.reply.try_send(served);
    }
}
#[cfg(target_os="linux")]
pub(crate) fn activate(args: Vec<String>, until: Instant) -> bool { activate_if(args,until,||true) }
pub(crate) fn activate_if(args: Vec<String>, until: Instant,claim:impl Fn()->bool+Send+Sync+'static) -> bool {
    let (reply, received) = mpsc::sync_channel(1);
    until > Instant::now()
        && post(Message::Activate(Arc::new(Activation { args, until, reply,claim:Box::new(claim) })))
        && received.recv_timeout(until.saturating_duration_since(Instant::now())) == Ok(true)
}
