//! Every order of clipboard events, through the machines and through a transcription of the
//! flags they replaced. The two have to agree on everything an app or another app can see.
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::AtomicUsize;
use crate::machine::Effects;
use super::secret::{Secret, SecretEffect, SecretEvent, step};
use super::*;

#[path = "hold_tests.rs"]
mod hold;

const LONG: Duration = Duration::from_secs(3600);
const PAST_LONG: Duration = Duration::from_secs(3601);
const SECRET: &str = "fixture-secret";
const PILLS: &str = "a <span data-pill=\"agent:x\">@x</span> b";

/// The logic before the machines, kept as it was: a state, a cancel flag beside it, and a held
/// record per clipboard.
mod flags {
    use super::*;

    pub type Queue = mpsc::Sender<Arc<Slot>>;

    #[derive(Default)]
    pub struct Slot {
        pub state: Mutex<SecretState>,
        pub cancel: AtomicBool,
        pub route: OnceLock<Queue>,
        // differs from dev: counts the clipboards holding this secret, for `clear_now`.
        pub holders: AtomicUsize,
    }
    impl Slot {
        pub fn set(&self, state: SecretState) { *lock(&self.state) = state; }
    }

    pub struct Unwritten(pub Arc<Slot>);
    impl Drop for Unwritten {
        fn drop(&mut self) {
            let mut state = lock(&self.0.state);
            if *state == SecretState::Pending {
                *state = if self.0.cancel.load(Ordering::SeqCst) { SecretState::Cleared } else { SecretState::Replaced };
            }
        }
    }

    #[derive(Clone)]
    pub struct Write {
        pub content: ClipboardContent,
        pub clear_after: Option<Duration>,
        pub secret: Option<Arc<Unwritten>>,
    }
    impl Write {
        pub fn slot(&self) -> Option<&Arc<Slot>> { self.secret.as_ref().map(|u| &u.0) }
    }

    #[derive(Clone)]
    pub struct Copy(pub Arc<Slot>);
    impl Copy {
        pub fn clear_now(&self) {
            self.0.cancel.store(true, Ordering::SeqCst);
            let holding = *lock(&self.0.state) == SecretState::Holding;
            // differs from dev: a failed replay leaves its earlier write held, and is cleared too.
            let holding = holding || (matches!(*lock(&self.0.state), SecretState::Failed(_)) && self.0.holders.load(Ordering::SeqCst) > 0);
            if holding && let Some(route) = self.0.route.get() { let _ = route.send(self.0.clone()); }
        }
    }

    struct Held {
        slot: Option<Arc<Slot>>,
        notify: Option<Notify>,
        deadline: Option<Instant>,
        // differs from dev (r128): after a wipe at the deadline fails, when to try again.
        retry_at: Option<Instant>,
        revision: Option<u64>,
        // differs from dev (r127): who the OS says wrote it (X11's selection owner).
        writer: Option<u64>,
        print: u64,
    }

    struct OwnPills {
        revision: Option<u64>,
        text_print: u64,
        pills: Vec<(std::ops::Range<usize>, String)>,
    }

    #[derive(Default)]
    pub struct Core { keys: RandomState, held: Option<Held>, own_pills: Option<OwnPills> }
    impl Core {
        pub fn watching(&self) -> bool { self.held.is_some() }

        pub fn answer(&mut self, cb: &mut Fake, purpose: Purpose) -> Option<Reply> {
            let mut reply = purpose.answer(cb)?;
            if let Some(own) = &self.own_pills {
                let rev_ok = match (own.revision, cb.revision()) {
                    (Some(a), Some(b)) => a == b,
                    (None, None) => true,
                    _ => false,
                };
                let html_ok = reply.content.html().is_some_and(|h| {
                    let frag = crate::TextEdit::from_html(h);
                    frag.pills().count() == own.pills.len()
                        && frag.pills().zip(&own.pills).all(|((r1, id1), (r2, id2))| r1 == *r2 && id1 == id2)
                });
                if rev_ok && self.keys.hash_one(reply.content.text()) == own.text_print && html_ok {
                    reply.content.own_pills = true;
                }
            }
            Some(reply)
        }

        pub fn write(&mut self, cb: &mut Fake, copy: Write, notify: Option<Notify>, steps: &dyn WriteSteps) {
            let slot = copy.slot().cloned();
            let notify = notify.filter(|_| slot.is_some());
            let cancelled = |slot: &Option<Arc<Slot>>| slot.as_ref().is_some_and(|s| s.cancel.load(Ordering::SeqCst));
            if cancelled(&slot) { return; }
            if let Err(error) = cb.land(&copy.content, copy.clear_after.is_some()) {
                if let Some(s) = &slot { s.set(SecretState::Failed(error)); }
                if let Some(n) = &notify { n.send(); }
                return;
            }
            self.release(SecretState::Replaced);
            let rev = cb.revision();
            self.own_pills = copy
                .clear_after
                .is_none()
                .then(|| copy.content.html())
                .flatten()
                .filter(|h| h.contains("<span data-pill="))
                .map(|h| {
                    let frag = crate::TextEdit::from_html(h);
                    OwnPills {
                        revision: rev,
                        text_print: self.keys.hash_one(copy.content.text()),
                        pills: frag.pills().map(|(r, id)| (r, id.to_owned())).collect(),
                    }
                })
                .filter(|o| !o.pills.is_empty());
            if let Some(delay) = copy.clear_after {
                if let Some(s) = &slot {
                    // differs from dev: counts the hold.
                    s.holders.fetch_add(1, Ordering::SeqCst);
                    s.set(SecretState::Holding);
                }
                let deadline = steps.now().checked_add(delay);
                steps.hashing();
                let print = self.keys.hash_one(copy.content.text());
                // differs from dev (r130): the change count comes from the write itself where the OS
                // allows (macOS), and (r127) the writer with it, rather than the count after it.
                let (revision, writer) = cb.stamp();
                self.held = Some(Held { slot: slot.clone(), notify: notify.clone(), deadline, retry_at: None, revision, writer, print });
                if let Some(n) = &notify { n.send(); }
                if cancelled(&slot) { self.clear(cb, slot.as_ref()); }
            }
        }

        fn release(&mut self, state: SecretState) {
            let Some(held) = self.held.take() else { return };
            if let Some(s) = &held.slot {
                // differs from dev: counts the hold ending.
                s.holders.fetch_sub(1, Ordering::SeqCst);
                s.set(state);
            }
            if let Some(n) = &held.notify { n.send(); }
        }

        fn mark(&self) -> Option<Mark<'_>> {
            let held = self.held.as_ref()?;
            Some(Mark { revision: held.revision, writer: held.writer, keys: &self.keys, print: held.print })
        }

        pub fn clear(&mut self, cb: &mut Fake, which: Option<&Arc<Slot>>) { self.clear_at(cb, which, None); }

        /// A clear; `expiring` is the time when it's the deadline's.
        fn clear_at(&mut self, cb: &mut Fake, which: Option<&Arc<Slot>>, expiring: Option<Instant>) {
            let Some(held) = &self.held else { return };
            if which.is_some_and(|w| !held.slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, w))) { return; }
            // differs from dev: dev sent no clear for a failed replay. This one keeps the hold and
            // its deadline unless the wipe is confirmed, so the timeout still clears the secret.
            let patient = which.is_some_and(|w| matches!(*lock(&w.state), SecretState::Failed(_)));
            // differs from dev (r130): the clipboard checks it still holds the secret and wipes it
            // as one step where the OS allows (Windows, X11), narrowed where it doesn't (macOS
            // rereads the count right before the wipe); dev checked, then wiped. This covers the
            // failed-replay wipe too.
            let wipe = cb.clear_if(&self.mark().expect("held"));
            match wipe {
                Wipe::Done => {
                    self.own_pills = None;
                    self.release(SecretState::Cleared);
                }
                Wipe::NotOurs if patient => {}
                Wipe::NotOurs => self.release(SecretState::Replaced),
                // differs from dev (r128, r129): a clipboard that couldn't be read or wiped keeps the
                // hold, and a wipe at the deadline is tried again a watch later; dev released it,
                // as Cleared after a failed wipe and as Replaced after a failed read.
                Wipe::Failed => if let (Some(now), Some(held)) = (expiring, self.held.as_mut()) { held.retry_at = Some(now + WATCH); },
            }
        }

        pub fn tick(&mut self, cb: &mut Fake, now: Instant) {
            if let Some(own) = &self.own_pills
                && let Some(rev) = own.revision
                && cb.revision() != Some(rev)
            {
                self.own_pills = None;
            }
            let Some(held) = &self.held else { return };
            // differs from dev (r128): a failed wipe at the deadline is tried again at `retry_at`.
            if held.retry_at.or(held.deadline).is_some_and(|d| now >= d) { return self.clear_at(cb, None, Some(now)); }
            let changed = match held.revision {
                Some(revision) => cb.revision() != Some(revision),
                // differs from dev (r127, r129): judged by the writer too where the OS says (X11), and
                // a clipboard that can't be read keeps the hold; dev let it go.
                None => cb.owner(&self.mark().expect("held")) == Owner::Theirs,
            };
            if changed { self.release(SecretState::Replaced); }
        }
    }
}

/// What a sequence can see happen, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Logged {
    Wrote(usize),
    Wiped(usize),
    Woke(usize, SecretState, SecretState),
    /// A write read the clock, with what `S` and `T` reported then.
    ReadClock(SecretState, SecretState),
    /// A write began hashing its secret.
    Hashing(SecretState, SecretState),
}
type Log = Arc<Mutex<Vec<Logged>>>;

/// How a clipboard responds to being wiped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WipeMode {
    Works,
    Refused,
    /// Says it worked, and changes nothing.
    NoEffect,
    /// Refused, and the clipboard is locked for reading from then on.
    RefusedAndLocks,
}

/// The clipboard a fake stands for: what it can tell Rune and how its wipe works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A change count, read from the write itself; a wipe that rereads the count first.
    Mac,
    /// A change count read after the write; a wipe under the clipboard's lock.
    Windows,
    /// No count, but an owner; a wipe with the server grabbed.
    X11,
    /// Neither; a secret's password hint, and a wipe that can lose a race.
    Wayland,
}
use Kind::*;

/// The window arboard owns the X11 clipboard with.
const RUNE_WINDOW: u64 = 1;

/// A clipboard with a scripted interruption during its next write.
struct Fake {
    id: usize,
    content: Option<ClipboardContent>,
    count: u64,
    kind: Kind,
    /// Who wrote what it holds (X11), 0 for nobody.
    owner: u64,
    /// What it holds carries the password hint Rune's secret copies carry.
    hinted: bool,
    /// The change count `SECRET` was written at, as the write itself saw it.
    stamped: Option<u64>,
    /// The change count when `SECRET` last landed.
    landed: Option<u64>,
    /// The last change was another app's.
    others: bool,
    /// Another app's copy of the same text landed between Rune's write and its stamp, where the
    /// stamp comes after the write (Windows, X11): Rune takes it for its own.
    taken: bool,
    in_write: bool,
    fail: Option<String>,
    unreadable: bool,
    wipe: WipeMode,
    /// Another app copies during the next wipe.
    race: bool,
    log: Log,
    during_write: Option<Box<dyn FnOnce(&mut Fake)>>,
}
impl Fake {
    fn new(id: usize, kind: Kind, log: Log) -> Self {
        Fake {
            id, content: None, count: 0, kind, owner: 0, hinted: false, stamped: None, landed: None, others: false, taken: false,
            in_write: false, fail: None, unreadable: false, wipe: WipeMode::Works, race: false, log, during_write: None,
        }
    }
    fn land(&mut self, content: &ClipboardContent, secret: bool) -> Result<(), String> {
        let result = match &self.fail {
            Some(error) => Err(error.clone()),
            None => {
                self.content = Some(content.clone());
                self.count += 1;
                self.owner = RUNE_WINDOW;
                self.hinted = secret;
                self.stamped = Some(self.count);
                self.others = false;
                self.taken = false;
                if content.text() == SECRET { self.landed = Some(self.count); }
                lock(&self.log).push(Logged::Wrote(self.id));
                Ok(())
            }
        };
        if let Some(hook) = self.during_write.take() {
            self.in_write = true;
            hook(self);
            self.in_write = false;
        }
        result
    }
    fn other_app(&mut self, text: &str) {
        self.content = Some(ClipboardContent::new(text));
        self.count += 1;
        self.owner = 100 + self.count;
        self.hinted = false;
        self.others = true;
        self.taken = self.in_write && text == SECRET && matches!(self.kind, Windows | X11);
    }
    /// Something will stop the next wipe going cleanly: a lock, a refusal, another app's copy.
    fn failing(&self) -> bool { self.unreadable || self.wipe != WipeMode::Works || self.race }
    /// Still exactly the secret Rune wrote.
    fn ours(&self) -> bool { self.landed == Some(self.count) }
    /// Another app's copy that Rune should be able to tell from its own.
    fn others_copy(&self) -> Option<&ClipboardContent> { self.content.as_ref().filter(|_| self.others && !self.taken) }
    /// Another app's copy, if one is armed to land now.
    fn race_now(&mut self) {
        if std::mem::take(&mut self.race) { self.other_app("theirs"); }
    }
    /// The wipe and the check after it, once ownership is settled.
    fn wipe_now(&mut self, mark: &Mark) -> Wipe {
        if self.clear().is_err() { return Wipe::Failed; }
        if self.owner(mark) == Owner::Ours { Wipe::Failed } else { Wipe::Done }
    }
}
impl Backend for Fake {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> { self.land(&copy.content, copy.is_secret()) }
    fn text(&mut self) -> Option<Zeroizing<String>> {
        if self.unreadable { return None; }
        self.content.as_ref().filter(|c| c.has_plain()).map(|c| c.text.clone())
    }
    fn read_text(&mut self) -> Result<Option<Zeroizing<String>>, String> {
        if self.unreadable { return Err("locked".into()); }
        Ok(self.text())
    }
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        if self.unreadable { return None; }
        self.content.clone().and_then(|c| if all { Some(c) } else { c.text_forms() })
    }
    fn revision(&self) -> Option<u64> { matches!(self.kind, Mac | Windows).then_some(self.count) }
    /// The X server answers who owns the clipboard itself, so an owner that won't hand over its
    /// text doesn't stop it.
    fn writer(&mut self) -> Result<Option<u64>, String> { Ok((self.kind == X11).then_some(self.owner)) }
    fn stamp(&mut self) -> (Option<u64>, Option<u64>) {
        match self.kind {
            Mac => (self.stamped, None),
            _ => (self.revision(), self.writer().ok().flatten().filter(|&w| w != 0)),
        }
    }
    fn clear(&mut self) -> Result<(), String> {
        match self.wipe {
            WipeMode::Works => {}
            WipeMode::Refused => return Err("locked".into()),
            WipeMode::NoEffect => return Ok(()),
            WipeMode::RefusedAndLocks => {
                self.unreadable = true;
                return Err("locked".into());
            }
        }
        self.content = None;
        self.count += 1;
        self.owner = 0;
        self.hinted = false;
        self.others = false;
        self.taken = false;
        lock(&self.log).push(Logged::Wiped(self.id));
        Ok(())
    }
    fn clear_if(&mut self, mark: &Mark) -> Wipe {
        match self.owner(mark) {
            Owner::Ours => {}
            Owner::Theirs => { self.race_now(); return Wipe::NotOurs; }
            Owner::Unknown => { self.race_now(); return Wipe::Failed; }
        }
        match self.kind {
            // The lock or the grab holds another app's copy back until the wipe is done.
            Windows | X11 => {
                let wipe = self.wipe_now(mark);
                self.race_now();
                wipe
            }
            Mac => {
                self.race_now();
                if self.revision() != mark.revision { return Wipe::NotOurs; }
                self.wipe_now(mark)
            }
            Wayland => {
                if !self.hinted { self.race_now(); return Wipe::NotOurs; }
                self.race_now();
                self.wipe_now(mark)
            }
        }
    }
}

/// The two implementations, behind what the rig drives.
trait Engine: Default {
    type Write: Clone;
    type Copy: Clone + 'static;
    type Queued;
    fn secret(text: &str, after: Duration) -> (Self::Write, Self::Copy);
    fn pills() -> Self::Write;
    fn bind(write: &Self::Write, queue: &mpsc::Sender<Self::Queued>);
    fn clear_now(copy: &Self::Copy);
    fn state(copy: &Self::Copy) -> SecretState;
    fn cancelled(copy: &Self::Copy) -> bool;
    fn write(&mut self, cb: &mut Fake, write: Self::Write, notify: Notify, steps: &dyn WriteSteps);
    fn tick(&mut self, cb: &mut Fake, now: Instant);
    fn read(&mut self, cb: &mut Fake) -> Option<Reply>;
    fn clear_held(&mut self, cb: &mut Fake);
    fn run(&mut self, cb: &mut Fake, queued: Self::Queued);
    fn watching(&self) -> bool;
}

impl Engine for Core {
    type Write = ClipboardWrite;
    type Copy = SecretCopy;
    type Queued = Request;
    fn secret(text: &str, after: Duration) -> (ClipboardWrite, SecretCopy) { ClipboardWrite::secret(text.into(), after) }
    fn pills() -> ClipboardWrite { ClipboardWrite::new(ClipboardContent::new("a @x b").with_html(PILLS)) }
    fn bind(write: &ClipboardWrite, queue: &mpsc::Sender<Request>) { write.bind(Route::System(queue.clone())); }
    fn clear_now(copy: &SecretCopy) { copy.clear_now(); }
    fn state(copy: &SecretCopy) -> SecretState { copy.state() }
    fn cancelled(copy: &SecretCopy) -> bool { copy.0.cancelled() }
    fn write(&mut self, cb: &mut Fake, write: ClipboardWrite, notify: Notify, steps: &dyn WriteSteps) { Core::write(self, cb, write, Some(notify), steps); }
    fn tick(&mut self, cb: &mut Fake, now: Instant) { self.tick_at(cb, now); }
    fn read(&mut self, cb: &mut Fake) -> Option<Reply> { self.answer(cb, Purpose::Read(1)) }
    fn clear_held(&mut self, cb: &mut Fake) { self.clear(cb, None); }
    fn run(&mut self, cb: &mut Fake, queued: Request) { self.handle(cb, queued); }
    fn watching(&self) -> bool { !self.hold.is_free() }
}

impl Engine for flags::Core {
    type Write = flags::Write;
    type Copy = flags::Copy;
    type Queued = Arc<flags::Slot>;
    fn secret(text: &str, after: Duration) -> (flags::Write, flags::Copy) {
        let slot = Arc::new(flags::Slot::default());
        let write = flags::Write { content: ClipboardContent::new(text), clear_after: Some(after), secret: Some(Arc::new(flags::Unwritten(slot.clone()))) };
        (write, flags::Copy(slot))
    }
    fn pills() -> flags::Write {
        flags::Write { content: ClipboardContent::new("a @x b").with_html(PILLS), clear_after: None, secret: None }
    }
    fn bind(write: &flags::Write, queue: &flags::Queue) {
        if let Some(slot) = write.slot() { let _ = slot.route.set(queue.clone()); }
    }
    fn clear_now(copy: &flags::Copy) { copy.clear_now(); }
    fn state(copy: &flags::Copy) -> SecretState { lock(&copy.0.state).clone() }
    fn cancelled(copy: &flags::Copy) -> bool { copy.0.cancel.load(Ordering::SeqCst) }
    fn write(&mut self, cb: &mut Fake, write: flags::Write, notify: Notify, steps: &dyn WriteSteps) { flags::Core::write(self, cb, write, Some(notify), steps); }
    fn tick(&mut self, cb: &mut Fake, now: Instant) { flags::Core::tick(self, cb, now); }
    fn read(&mut self, cb: &mut Fake) -> Option<Reply> { self.answer(cb, Purpose::Read(1)) }
    fn clear_held(&mut self, cb: &mut Fake) { self.clear(cb, None); }
    fn run(&mut self, cb: &mut Fake, queued: Arc<flags::Slot>) { self.clear(cb, Some(&queued)); }
    fn watching(&self) -> bool { flags::Core::watching(self) }
}

/// One thing that can happen. `A` and `B` are two clipboards; `S` is a secret whose write is
/// cloned for every replay, `T` a different one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    WriteSA,
    WriteSB,
    WriteTA,
    /// A secret whose delay is too long for the clock, so it has no deadline.
    WriteForeverA,
    WritePillsA,
    /// `S` to A, with the backend taking longer than the secret's delay.
    WriteSASlow,
    /// `S` to A, with hashing it taking longer than its delay.
    WriteSASlowHash,
    /// `S` to A, with `clear_now` landing during the backend write.
    WriteSACancelInWrite,
    /// `S` to A, with another app's copy landing right after the backend write.
    WriteSAOtherInWrite,
    /// `S` to A, with `clear_now` and then another app's copy landing during the backend write.
    WriteSACancelOtherInWrite,
    /// `S` to A, with `clear_now` landing during the write's notification.
    WriteSACancelInWake,
    /// `S` to A, with another app copying the same text right after the backend write.
    WriteSASameInWrite,
    ReadA,
    ReadB,
    CancelS,
    /// The clock passes every delay so far; each worker sees it on its next turn.
    Timeout,
    OtherA,
    OtherB,
    /// Another app copies the secret's exact text onto both.
    OtherSameText,
    /// A's writes start or stop failing, with a new error each time.
    ToggleFailA,
    /// A's reads start or stop failing.
    ToggleUnreadableA,
    /// A's wipes move on to their next way of failing, or back to working.
    CycleWipeA,
    /// Another app copies during A's next wipe.
    RaceA,
    ClearHeldA,
    ShutdownA,
    /// The workers reach the clears `clear_now` queued. Until then, reads are ahead of them.
    Pump,
    /// The app's own copies of the writes go away.
    DropWrites,
}
use Ev::*;

/// The rig's clock, as a write sees it.
struct Steps<'a, E: Engine> {
    clock: &'a Cell<Instant>,
    slow_hash: bool,
    log: &'a Log,
    s: &'a E::Copy,
    t: &'a E::Copy,
}
impl<E: Engine> WriteSteps for Steps<'_, E> {
    fn now(&self) -> Instant {
        lock(self.log).push(Logged::ReadClock(E::state(self.s), E::state(self.t)));
        self.clock.get()
    }
    fn hashing(&self) {
        lock(self.log).push(Logged::Hashing(E::state(self.s), E::state(self.t)));
        if self.slow_hash { self.clock.set(self.clock.get() + PAST_LONG); }
    }
}

struct Rig<E: Engine> {
    boards: [Fake; 2],
    engines: [E; 2],
    alive: [bool; 2],
    queues: [(mpsc::Sender<E::Queued>, mpsc::Receiver<E::Queued>); 2],
    notify: [Notify; 2],
    s: E::Copy,
    t: E::Copy,
    forever: E::Copy,
    s_write: Option<E::Write>,
    t_write: Option<E::Write>,
    forever_write: Option<E::Write>,
    cancel_on_wake: Arc<AtomicBool>,
    log: Log,
    reads: [Option<(ClipboardContent, bool)>; 2],
    /// The board `S` was first sent to: the only one `clear_now` reaches.
    first: Option<usize>,
    clock: Rc<Cell<Instant>>,
    fails: usize,
}

impl<E: Engine> Rig<E> {
    fn new(kinds: [Kind; 2], start: Instant) -> Self
    where E::Copy: Send + Sync {
        let log = Log::default();
        let (s_write, s) = E::secret(SECRET, LONG);
        let (t_write, t) = E::secret("fixture-other", LONG);
        let (forever_write, forever) = E::secret("fixture-forever", Duration::MAX);
        let cancel_on_wake = Arc::new(AtomicBool::new(false));
        let board = |id| Fake::new(id, kinds[id], log.clone());
        let notify = |id: usize| {
            let (log, s, t, arm) = (log.clone(), s.clone(), t.clone(), cancel_on_wake.clone());
            Notify {
                changed: Arc::default(),
                wake: Arc::new(move || {
                    if arm.swap(false, Ordering::SeqCst) { E::clear_now(&s); }
                    lock(&log).push(Logged::Woke(id, E::state(&s), E::state(&t)));
                }),
            }
        };
        Self {
            boards: [board(0), board(1)],
            engines: [E::default(), E::default()],
            alive: [true, true],
            queues: [mpsc::channel(), mpsc::channel()],
            notify: [notify(0), notify(1)],
            s_write: Some(s_write),
            t_write: Some(t_write),
            forever_write: Some(forever_write),
            s,
            t,
            forever,
            cancel_on_wake,
            log,
            reads: [None, None],
            first: None,
            clock: Rc::new(Cell::new(start)),
            fails: 0,
        }
    }

    /// The worker's turn on board `b`: it looks at the clipboard before every request.
    fn turn(&mut self, b: usize) -> bool {
        if self.alive[b] { self.engines[b].tick(&mut self.boards[b], self.clock.get()); }
        self.alive[b]
    }

    fn write(&mut self, b: usize, write: Option<E::Write>) { self.write_with(b, write, false); }

    fn write_with(&mut self, b: usize, write: Option<E::Write>, slow_hash: bool) {
        let Some(write) = write else { return };
        E::bind(&write, &self.queues[b].0);
        if !self.turn(b) { return; }
        let steps = Steps::<E> { clock: &self.clock, slow_hash, log: &self.log, s: &self.s, t: &self.t };
        self.engines[b].write(&mut self.boards[b], write, self.notify[b].clone(), &steps);
    }

    fn read(&mut self, b: usize) {
        if !self.turn(b) { return; }
        self.reads[b] = self.engines[b].read(&mut self.boards[b]).map(|r| {
            let own = r.content.has_own_pills();
            (r.content, own)
        });
    }

    fn apply(&mut self, ev: Ev) where E::Copy: 'static {
        let target = match ev {
            WriteSA | WriteSASlow | WriteSASlowHash | WriteSACancelInWrite | WriteSAOtherInWrite | WriteSACancelOtherInWrite | WriteSACancelInWake | WriteSASameInWrite => Some(0),
            WriteSB => Some(1),
            _ => None,
        };
        if self.first.is_none() && self.s_write.is_some() { self.first = target; }
        match ev {
            WriteSA => self.write(0, self.s_write.clone()),
            WriteSB => self.write(1, self.s_write.clone()),
            WriteTA => self.write(0, self.t_write.clone()),
            WriteForeverA => self.write(0, self.forever_write.clone()),
            WritePillsA => self.write(0, Some(E::pills())),
            WriteSASlow => {
                let clock = self.clock.clone();
                self.boards[0].during_write = Some(Box::new(move |_| clock.set(clock.get() + PAST_LONG)));
                self.write(0, self.s_write.clone());
                self.boards[0].during_write = None;
            }
            WriteSASlowHash => self.write_with(0, self.s_write.clone(), true),
            WriteSACancelInWrite => {
                let s = self.s.clone();
                self.boards[0].during_write = Some(Box::new(move |_| E::clear_now(&s)));
                self.write(0, self.s_write.clone());
                self.boards[0].during_write = None;
            }
            WriteSAOtherInWrite => {
                self.boards[0].during_write = Some(Box::new(|f| f.other_app("theirs")));
                self.write(0, self.s_write.clone());
                self.boards[0].during_write = None;
            }
            WriteSACancelOtherInWrite => {
                let s = self.s.clone();
                self.boards[0].during_write = Some(Box::new(move |f| { E::clear_now(&s); f.other_app("theirs"); }));
                self.write(0, self.s_write.clone());
                self.boards[0].during_write = None;
            }
            WriteSACancelInWake => {
                self.cancel_on_wake.store(true, Ordering::SeqCst);
                self.write(0, self.s_write.clone());
                self.cancel_on_wake.store(false, Ordering::SeqCst);
            }
            WriteSASameInWrite => {
                self.boards[0].during_write = Some(Box::new(|f| f.other_app(SECRET)));
                self.write(0, self.s_write.clone());
                self.boards[0].during_write = None;
            }
            ReadA => self.read(0),
            ReadB => self.read(1),
            CancelS => E::clear_now(&self.s),
            Timeout => self.clock.set(self.clock.get() + PAST_LONG),
            OtherA => self.boards[0].other_app("theirs"),
            OtherB => self.boards[1].other_app("theirs"),
            OtherSameText => for board in &mut self.boards { board.other_app(SECRET); },
            ToggleFailA => {
                self.fails += 1;
                self.boards[0].fail = self.boards[0].fail.is_none().then(|| format!("denied {}", self.fails));
            }
            ToggleUnreadableA => self.boards[0].unreadable = !self.boards[0].unreadable,
            RaceA => self.boards[0].race = true,
            CycleWipeA => self.boards[0].wipe = match self.boards[0].wipe {
                WipeMode::Works => WipeMode::Refused,
                WipeMode::Refused => WipeMode::NoEffect,
                WipeMode::NoEffect => WipeMode::RefusedAndLocks,
                WipeMode::RefusedAndLocks => WipeMode::Works,
            },
            ClearHeldA => if self.turn(0) { self.engines[0].clear_held(&mut self.boards[0]); },
            ShutdownA => if self.turn(0) {
                self.engines[0].clear_held(&mut self.boards[0]);
                self.alive[0] = false;
            },
            Pump => for b in 0..2 {
                let queued: Vec<_> = self.queues[b].1.try_iter().collect();
                for request in queued {
                    if self.turn(b) { self.engines[b].run(&mut self.boards[b], request); }
                }
            },
            DropWrites => { self.s_write = None; self.t_write = None; self.forever_write = None; }
        }
    }
}

/// Fails if the two rigs differ in anything observable, and in what decides their futures.
fn same(a: &Rig<Core>, b: &Rig<flags::Core>) -> Result<(), String> {
    let differ = |what: &str, x: &dyn std::fmt::Debug, y: &dyn std::fmt::Debug| Err(format!("{what}: machines {x:?}, flags {y:?}"));
    for i in 0..2 {
        if a.boards[i].content != b.boards[i].content { return differ("content", &text_of(&a.boards[i]), &text_of(&b.boards[i])); }
        if a.boards[i].count != b.boards[i].count { return differ("change count", &a.boards[i].count, &b.boards[i].count); }
        if a.reads[i] != b.reads[i] { return differ("read", &a.reads[i].as_ref().map(|r| r.1), &b.reads[i].as_ref().map(|r| r.1)); }
        if a.engines[i].watching() != Engine::watching(&b.engines[i]) { return differ("watching", &a.engines[i].watching(), &Engine::watching(&b.engines[i])); }
    }
    let (sa, sb) = (Core::state(&a.s), <flags::Core as Engine>::state(&b.s));
    if sa != sb { return differ("S reported", &sa, &sb); }
    let (ta, tb) = (Core::state(&a.t), <flags::Core as Engine>::state(&b.t));
    if ta != tb { return differ("T reported", &ta, &tb); }
    let (fa, fb) = (Core::state(&a.forever), <flags::Core as Engine>::state(&b.forever));
    if fa != fb { return differ("forever reported", &fa, &fb); }
    let (ca, cb) = (Core::cancelled(&a.s), <flags::Core as Engine>::cancelled(&b.s));
    if ca != cb { return differ("S cancelled", &ca, &cb); }
    let (la, lb) = (lock(&a.log), lock(&b.log));
    if *la != *lb { return differ("order of events", &*la, &*lb); }
    Ok(())
}
fn text_of(board: &Fake) -> Option<&str> { board.content.as_ref().map(|c| c.text()) }

/// Drives one sequence through both, comparing after every event.
fn check(sequence: &[Ev], kinds: [Kind; 2], start: Instant) -> Result<(), String> {
    let mut machines = Rig::<Core>::new(kinds, start);
    let mut reference = Rig::<flags::Core>::new(kinds, start);
    for (i, &ev) in sequence.iter().enumerate() {
        machines.apply(ev);
        reference.apply(ev);
        same(&machines, &reference).map_err(|why| format!("{:?} on {kinds:?}, after event {i}: {why}", sequence))?;
    }
    keeps_clear_nows_promise(&mut machines, &mut reference).map_err(|why| format!("{:?} on {kinds:?}, then clear_now: {why}", sequence))
}

/// Whatever came before: `clear_now` wipes the board it reaches if it still holds the secret and
/// the copy reports `Holding` or `Failed`, takes no other app's copy, and leaves other boards be.
fn keeps_clear_nows_promise(machines: &mut Rig<Core>, reference: &mut Rig<flags::Core>) -> Result<(), String> {
    let reached = machines.first.filter(|&b| machines.alive[b]);
    let reachable = matches!(machines.s.state(), SecretState::Holding | SecretState::Failed(_));
    let ours = reachable && reached.is_some_and(|b| machines.boards[b].ours() && machines.engines[b].watching() && !machines.boards[b].failing());
    let others = [0, 1].map(|b| machines.boards[b].others_copy().cloned());
    let before = [0, 1].map(|b| machines.boards[b].content.clone());
    for ev in [CancelS, Pump] {
        machines.apply(ev);
        reference.apply(ev);
        same(machines, reference)?;
    }
    for b in 0..2 {
        if ours && reached == Some(b) && machines.boards[b].ours() { return Err(format!("board {b} still holds the secret")); }
        // Only Rune's wipe ends another app's copy; another app may replace it.
        if others[b].is_some() && !machines.boards[b].others { return Err(format!("board {b} lost another app's copy")); }
        if machines.first != Some(b) && machines.boards[b].content != before[b] { return Err(format!("board {b} was out of reach but changed")); }
    }
    let state = machines.s.state();
    if ours && state != SecretState::Cleared { return Err(format!("reported {state:?} after a wipe")); }
    Ok(())
}

/// Every sequence of up to six events, on both layouts: each kind of clipboard as A once and as
/// B once.
fn exhaust_all(alphabet: &[Ev]) {
    for kinds in LAYOUTS {
        for len in 1..=6 { assert_eq!(exhaust(alphabet, len, kinds), alphabet.len().pow(len)); }
    }
}

/// Every sequence of exactly `len` events, across threads. Returns how many ran.
fn exhaust(alphabet: &[Ev], len: u32, kinds: [Kind; 2]) -> usize {
    let total = alphabet.len().pow(len);
    let start = Instant::now();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    const CHUNK: usize = 4096;
    let failure = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut sequence = vec![alphabet[0]; len as usize];
                loop {
                    let from = next.fetch_add(CHUNK, Ordering::Relaxed);
                    if from >= total || lock(&failure).is_some() { return; }
                    for index in from..(from + CHUNK).min(total) {
                        let mut rest = index;
                        for slot in sequence.iter_mut() {
                            *slot = alphabet[rest % alphabet.len()];
                            rest /= alphabet.len();
                        }
                        if let Err(why) = check(&sequence, kinds, start) {
                            *lock(&failure) = Some(why);
                            return;
                        }
                    }
                }
            });
        }
    });
    if let Some(why) = failure.into_inner().unwrap() { panic!("{why}"); }
    total
}

const LAYOUTS: [[Kind; 2]; 2] = [[Mac, X11], [Wayland, Windows]];

/// Writes and replays across boards against everything that can end a hold.
const BOARDS: &[Ev] = &[WriteSA, WriteSB, WriteTA, ReadA, ReadB, CancelS, Pump, Timeout, OtherA, OtherB, ClearHeldA, ToggleFailA];
/// Interruptions during a write, against what follows them on the same board.
const IN_FLIGHT: &[Ev] = &[WriteSA, WriteSASlow, WriteSASlowHash, WriteSACancelInWrite, WriteSAOtherInWrite, WriteSACancelOtherInWrite, WriteSACancelInWake, WriteSASameInWrite, ReadA, CancelS, Pump, OtherA, ToggleFailA, Timeout];
/// A failed replay, and clipboards that won't read or wipe or that another app copies to mid-wipe.
const FAILURES: &[Ev] = &[WriteSA, WriteSB, ToggleFailA, ToggleUnreadableA, CycleWipeA, RaceA, CancelS, Pump, Timeout, ReadA, OtherA, ClearHeldA];
/// How a copy's life ends: shutdown, dropped writes, pills, a look-alike from another app.
const ENDINGS: &[Ev] = &[WriteSA, WriteSB, WriteTA, WriteForeverA, WritePillsA, ReadA, CancelS, Pump, Timeout, OtherSameText, ShutdownA, DropWrites];

#[test]
fn every_six_events_across_boards_match_the_flags() {
    exhaust_all(BOARDS);
}

#[test]
fn every_six_events_around_a_write_match_the_flags() {
    exhaust_all(IN_FLIGHT);
}

#[test]
fn every_six_events_with_a_failing_clipboard_match_the_flags() {
    exhaust_all(FAILURES);
}

#[test]
fn every_six_events_ending_a_copy_match_the_flags() {
    exhaust_all(ENDINGS);
}

/// The whole alphabet at once, to five events on every layout; too long for the normal run. Run
/// with `--ignored`.
const ALL: &[Ev] = &[
    WriteSA, WriteSB, WriteTA, WriteForeverA, WritePillsA, WriteSASlow, WriteSASlowHash, WriteSACancelInWrite, WriteSAOtherInWrite, WriteSACancelOtherInWrite, WriteSACancelInWake, WriteSASameInWrite,
    ReadA, ReadB, CancelS, Timeout, OtherA, OtherB, OtherSameText, ToggleFailA, ToggleUnreadableA, CycleWipeA, RaceA, ClearHeldA, ShutdownA, Pump, DropWrites,
];

#[test]
#[ignore]
fn every_five_events_from_the_whole_alphabet_match_the_flags() {
    for kinds in [[Mac, X11], [Wayland, Windows], [X11, Wayland], [Windows, Mac]] {
        for len in 1..=5 { assert_eq!(exhaust(ALL, len, kinds), ALL.len().pow(len)); }
    }
}
