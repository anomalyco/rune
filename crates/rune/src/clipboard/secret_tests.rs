//! Every order of clipboard events, through the machines and through a transcription of the
//! flags they replaced. The two have to agree on everything an app or another app can see.
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::AtomicUsize;
use crate::machine::Effects;
use super::secret::{Secret, SecretEffect, SecretEvent, step};
use super::*;

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
            if holding && let Some(route) = self.0.route.get() { let _ = route.send(self.0.clone()); }
        }
    }

    struct Held {
        slot: Option<Arc<Slot>>,
        notify: Option<Notify>,
        deadline: Option<Instant>,
        revision: Option<u64>,
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

        pub fn write(&mut self, cb: &mut Fake, copy: Write, notify: Option<Notify>, now: &dyn Fn() -> Instant) {
            let slot = copy.slot().cloned();
            let notify = notify.filter(|_| slot.is_some());
            let cancelled = |slot: &Option<Arc<Slot>>| slot.as_ref().is_some_and(|s| s.cancel.load(Ordering::SeqCst));
            if cancelled(&slot) { return; }
            if let Err(error) = cb.land(&copy.content) {
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
                if let Some(s) = &slot { s.set(SecretState::Holding); }
                self.held = Some(Held {
                    slot: slot.clone(),
                    notify: notify.clone(),
                    deadline: now().checked_add(delay),
                    revision: rev,
                    print: self.keys.hash_one(copy.content.text()),
                });
                if let Some(n) = &notify { n.send(); }
                if cancelled(&slot) { self.clear(cb, slot.as_ref()); }
            }
        }

        fn release(&mut self, state: SecretState) {
            let Some(held) = self.held.take() else { return };
            if let Some(s) = &held.slot { s.set(state); }
            if let Some(n) = &held.notify { n.send(); }
        }

        fn unchanged(&self, cb: &mut Fake) -> bool {
            let Some(held) = &self.held else { return false };
            held.revision == cb.revision() && cb.text().is_some_and(|t| self.keys.hash_one(t.as_str()) == held.print)
        }

        pub fn clear(&mut self, cb: &mut Fake, which: Option<&Arc<Slot>>) {
            let Some(held) = &self.held else { return };
            if which.is_some_and(|w| !held.slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, w))) { return; }
            if self.unchanged(cb) {
                cb.clear();
                self.own_pills = None;
                self.release(SecretState::Cleared);
            } else {
                self.release(SecretState::Replaced);
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
            if held.deadline.is_some_and(|d| now >= d) { return self.clear(cb, None); }
            let changed = match held.revision {
                Some(revision) => cb.revision() != Some(revision),
                None => !self.unchanged(cb),
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
}
type Log = Arc<Mutex<Vec<Logged>>>;

/// A clipboard with a scripted interruption during its next write.
struct Fake {
    id: usize,
    content: Option<ClipboardContent>,
    count: u64,
    counted: bool,
    fail: Option<String>,
    log: Log,
    during_write: Option<Box<dyn FnOnce(&mut Fake)>>,
}
impl Fake {
    fn land(&mut self, content: &ClipboardContent) -> Result<(), String> {
        let result = match &self.fail {
            Some(error) => Err(error.clone()),
            None => {
                self.content = Some(content.clone());
                self.count += 1;
                lock(&self.log).push(Logged::Wrote(self.id));
                Ok(())
            }
        };
        if let Some(hook) = self.during_write.take() { hook(self); }
        result
    }
    fn other_app(&mut self, text: &str) {
        self.content = Some(ClipboardContent::new(text));
        self.count += 1;
    }
}
impl Backend for Fake {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> { self.land(&copy.content) }
    fn text(&mut self) -> Option<Zeroizing<String>> {
        self.content.as_ref().filter(|c| c.has_plain()).map(|c| c.text.clone())
    }
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        self.content.clone().and_then(|c| if all { Some(c) } else { c.text_forms() })
    }
    fn revision(&self) -> Option<u64> { self.counted.then_some(self.count) }
    fn clear(&mut self) {
        self.content = None;
        self.count += 1;
        lock(&self.log).push(Logged::Wiped(self.id));
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
    fn write(&mut self, cb: &mut Fake, write: Self::Write, notify: Notify, now: &dyn Fn() -> Instant);
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
    fn write(&mut self, cb: &mut Fake, write: ClipboardWrite, notify: Notify, now: &dyn Fn() -> Instant) { Core::write(self, cb, write, Some(notify), now); }
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
    fn write(&mut self, cb: &mut Fake, write: flags::Write, notify: Notify, now: &dyn Fn() -> Instant) { flags::Core::write(self, cb, write, Some(notify), now); }
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
    /// `S` to A, with `clear_now` landing during the backend write.
    WriteSACancelInWrite,
    /// `S` to A, with another app's copy landing right after the backend write.
    WriteSAOtherInWrite,
    /// `S` to A, with `clear_now` and then another app's copy landing during the backend write.
    WriteSACancelOtherInWrite,
    /// `S` to A, with `clear_now` landing during the write's notification.
    WriteSACancelInWake,
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
    ClearHeldA,
    ShutdownA,
    /// The workers reach the clears `clear_now` queued. Until then, reads are ahead of them.
    Pump,
    /// The app's own copies of the writes go away.
    DropWrites,
}
use Ev::*;

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
    clock: Rc<Cell<Instant>>,
    fails: usize,
}

impl<E: Engine> Rig<E> {
    fn new(counted: [bool; 2], start: Instant) -> Self
    where E::Copy: Send + Sync {
        let log = Log::default();
        let (s_write, s) = E::secret(SECRET, LONG);
        let (t_write, t) = E::secret("fixture-other", LONG);
        let (forever_write, forever) = E::secret("fixture-forever", Duration::MAX);
        let cancel_on_wake = Arc::new(AtomicBool::new(false));
        let board = |id| Fake { id, content: None, count: 0, counted: counted[id], fail: None, log: log.clone(), during_write: None };
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
            clock: Rc::new(Cell::new(start)),
            fails: 0,
        }
    }

    /// The worker's turn on board `b`: it looks at the clipboard before every request.
    fn turn(&mut self, b: usize) -> bool {
        if self.alive[b] { self.engines[b].tick(&mut self.boards[b], self.clock.get()); }
        self.alive[b]
    }

    fn write(&mut self, b: usize, write: Option<E::Write>) {
        let Some(write) = write else { return };
        E::bind(&write, &self.queues[b].0);
        if !self.turn(b) { return; }
        let clock = self.clock.clone();
        self.engines[b].write(&mut self.boards[b], write, self.notify[b].clone(), &move || clock.get());
    }

    fn read(&mut self, b: usize) {
        if !self.turn(b) { return; }
        self.reads[b] = self.engines[b].read(&mut self.boards[b]).map(|r| {
            let own = r.content.has_own_pills();
            (r.content, own)
        });
    }

    fn apply(&mut self, ev: Ev) where E::Copy: 'static {
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
fn check(sequence: &[Ev], counted: [bool; 2], start: Instant) -> Result<(), String> {
    let mut machines = Rig::<Core>::new(counted, start);
    let mut reference = Rig::<flags::Core>::new(counted, start);
    for (i, &ev) in sequence.iter().enumerate() {
        machines.apply(ev);
        reference.apply(ev);
        same(&machines, &reference).map_err(|why| format!("{:?} with change counts {counted:?}, after event {i}: {why}", sequence))?;
    }
    Ok(())
}

/// Every sequence of exactly `len` events, which covers each shorter one as a prefix, across
/// threads. Returns how many ran.
fn exhaust(alphabet: &[Ev], len: u32, counted: [bool; 2]) -> usize {
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
                        if let Err(why) = check(&sequence, counted, start) {
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

const COUNTS: [[bool; 2]; 2] = [[true, false], [false, true]];

/// Writes and replays across boards against everything that can end a hold.
const BOARDS: &[Ev] = &[WriteSA, WriteSB, WriteTA, ReadA, ReadB, CancelS, Timeout, OtherA, OtherB, ClearHeldA, ToggleFailA];
/// Interruptions during a write, against what follows them on the same board.
const IN_FLIGHT: &[Ev] = &[WriteSA, WriteSASlow, WriteSACancelInWrite, WriteSAOtherInWrite, WriteSACancelOtherInWrite, WriteSACancelInWake, ReadA, CancelS, Pump, OtherA, ToggleFailA, Timeout];
/// How a copy's life ends: shutdown, dropped writes, pills, a look-alike from another app.
const ENDINGS: &[Ev] = &[WriteSA, WriteSB, WriteTA, WriteForeverA, WritePillsA, ReadA, CancelS, Pump, Timeout, OtherSameText, ShutdownA, DropWrites];

#[test]
fn every_six_events_across_boards_match_the_flags() {
    for counted in COUNTS { assert_eq!(exhaust(BOARDS, 6, counted), BOARDS.len().pow(6)); }
}

#[test]
fn every_six_events_around_a_write_match_the_flags() {
    for counted in COUNTS { assert_eq!(exhaust(IN_FLIGHT, 6, counted), IN_FLIGHT.len().pow(6)); }
}

#[test]
fn every_six_events_ending_a_copy_match_the_flags() {
    for counted in COUNTS { assert_eq!(exhaust(ENDINGS, 6, counted), ENDINGS.len().pow(6)); }
}

/// The whole alphabet at once; too long for the normal run. Run with `--ignored --release`.
#[test]
#[ignore]
fn every_six_events_from_the_whole_alphabet_match_the_flags() {
    const ALL: &[Ev] = &[
        WriteSA, WriteSB, WriteTA, WriteForeverA, WritePillsA, WriteSASlow, WriteSACancelInWrite, WriteSAOtherInWrite, WriteSACancelOtherInWrite, WriteSACancelInWake,
        ReadA, ReadB, CancelS, Timeout, OtherA, OtherB, OtherSameText, ToggleFailA, ClearHeldA, ShutdownA, Pump, DropWrites,
    ];
    for counted in COUNTS { assert_eq!(exhaust(ALL, 6, counted), ALL.len().pow(6)); }
}

/// Runs `sequence` through the machines and hands back the rig to look at.
fn run(sequence: &[Ev]) -> Rig<Core> {
    let start = Instant::now();
    check(sequence, [true, true], start).unwrap();
    let mut rig = Rig::<Core>::new([true, true], start);
    for &ev in sequence { rig.apply(ev); }
    rig
}
fn wipes(rig: &Rig<Core>, board: usize) -> usize {
    lock(&rig.log).iter().filter(|e| **e == Logged::Wiped(board)).count()
}

#[test]
fn a_cancel_during_the_write_is_wiped_before_a_queued_read() {
    let rig = run(&[WriteSACancelInWrite, ReadA]);
    assert_eq!(text_of(&rig.boards[0]), None);
    assert_eq!(rig.reads[0].as_ref().map(|r| r.0.text()), Some(""));
    assert_eq!(rig.s.state(), SecretState::Cleared);
}

#[test]
fn a_cancel_during_the_notification_is_wiped_before_a_queued_read() {
    let rig = run(&[WriteSACancelInWake, ReadA]);
    assert_eq!(rig.reads[0].as_ref().map(|r| r.0.text()), Some(""));
    let woke = lock(&rig.log).iter().filter(|e| matches!(e, Logged::Woke(..))).count();
    assert_eq!(woke, 2, "one for the hold, one for the clear");
}

#[test]
fn a_cancel_in_flight_spares_another_apps_copy() {
    let rig = run(&[WriteSACancelOtherInWrite]);
    assert_eq!(text_of(&rig.boards[0]), Some("theirs"));
    assert_eq!(wipes(&rig, 0), 0);
    assert_eq!(rig.s.state(), SecretState::Replaced);
}

#[test]
fn a_replay_after_clear_now_is_never_written() {
    let rig = run(&[WriteSA, CancelS, Pump, WriteSA]);
    assert_eq!(text_of(&rig.boards[0]), None);
    assert_eq!(rig.s.state(), SecretState::Cleared);
    let rig = run(&[WriteSA, Timeout, ReadA, WriteSA]);
    assert_eq!(text_of(&rig.boards[0]), Some(SECRET), "expiry alone doesn't stop a replay");
    assert_eq!(rig.s.state(), SecretState::Holding);
}

#[test]
fn a_replay_while_holding_stays_covered_by_cleanup() {
    let rig = run(&[WriteSA, WriteSA, Timeout, ReadA]);
    assert_eq!((text_of(&rig.boards[0]), wipes(&rig, 0)), (None, 1));
    let rig = run(&[WriteSA, WriteSA, ShutdownA]);
    assert_eq!((text_of(&rig.boards[0]), wipes(&rig, 0)), (None, 1));
}

#[test]
fn the_latest_failure_is_reported_and_keeps_earlier_cleanup() {
    let rig = run(&[ToggleFailA, WriteSA, ToggleFailA, ToggleFailA, WriteSA]);
    assert_eq!(rig.s.state(), SecretState::Failed("denied 3".into()));
    let rig = run(&[WriteSA, ToggleFailA, WriteSA, Timeout, ReadA]);
    assert_eq!(rig.s.state(), SecretState::Cleared);
    assert_eq!((text_of(&rig.boards[0]), wipes(&rig, 0)), (None, 1));
}

#[test]
fn a_clone_on_two_boards_is_cleaned_up_on_each() {
    let rig = run(&[WriteSA, WriteSB, WriteTA, Timeout, ReadA, ReadB]);
    assert_eq!(text_of(&rig.boards[1]), None, "A's later write doesn't disarm B");
    assert_eq!(wipes(&rig, 1), 1);
    let rig = run(&[WriteSA, WriteSB, OtherA, ReadA, Timeout, ReadB]);
    assert_eq!((text_of(&rig.boards[0]), text_of(&rig.boards[1])), (Some("theirs"), None));
}

#[test]
fn cancel_latches_in_every_report_and_asks_for_a_clear_only_while_holding() {
    for report in [SecretState::Pending, SecretState::Holding, SecretState::Cleared, SecretState::Replaced, SecretState::Failed("x".into())] {
        for mut secret in [Secret::Live(report.clone()), Secret::Cancelled(report.clone())] {
            let effects = step(&mut secret, SecretEvent::Cancel);
            assert_eq!(secret, Secret::Cancelled(report.clone()));
            let expected = if report == SecretState::Holding { Effects::one(SecretEffect::Clear) } else { Effects::none() };
            assert_eq!(effects, expected);
        }
    }
}

#[test]
fn the_delay_starts_once_a_slow_write_is_done() {
    let rig = run(&[WriteSASlow, ReadA]);
    assert_eq!(rig.reads[0].as_ref().map(|r| r.0.text()), Some(SECRET));
    assert_eq!(rig.s.state(), SecretState::Holding);
    let rig = run(&[WriteSASlow, Timeout, ReadA]);
    assert_eq!((text_of(&rig.boards[0]), rig.s.state()), (None, SecretState::Cleared));
}

#[test]
fn a_hold_with_no_deadline_keeps_no_earlier_alarm() {
    let mut cb = Fake { id: 0, content: None, count: 0, counted: true, fail: None, log: Log::default(), during_write: None };
    let mut core = Core::default();
    core.write(&mut cb, ClipboardWrite::secret("first".into(), LONG).0, None, Instant::now);
    assert!(core.alarm.is_some());
    core.write(&mut cb, ClipboardWrite::secret("second".into(), Duration::MAX).0, None, Instant::now);
    assert!(core.alarm.is_none(), "a wrapped attempt could otherwise match it");
}

#[test]
fn a_timer_from_an_earlier_hold_changes_nothing() {
    let slot = Arc::new(Slot::default());
    let (mut seq, mut hold) = (Seq::default(), Hold::default());
    let due = Some(Instant::now());
    let written = |slot: &Arc<Slot>| HoldEvent::Written { slot: slot.clone(), notify: None, due, revision: None, print: 0 };
    drop(step_hold(&mut seq, &mut hold, written(&slot)));
    let stale = seq.current();
    drop(step_hold(&mut seq, &mut hold, written(&slot)));
    assert!(step_hold(&mut seq, &mut hold, HoldEvent::Due { attempt: stale, ours: true }).is_empty());
    assert!(!hold.is_free());
    let attempt = seq.current();
    assert!(!step_hold(&mut seq, &mut hold, HoldEvent::Due { attempt, ours: true }).is_empty());
    assert!(hold.is_free());
}

#[test]
fn no_clipboard_reports_a_failed_write_and_wakes() {
    let worker = native::Worker::spawn(|| None::<Board>);
    let (woke, wakes) = mpsc::channel();
    let notify = Notify { changed: Arc::default(), wake: Arc::new(move || { let _ = woke.send(()); }) };
    let (write, handle) = ClipboardWrite::secret("fixture".into(), LONG);
    worker.sender.send(Request::Write(write, Some(notify))).unwrap();
    wakes.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(handle.state(), SecretState::Failed("no clipboard".into()));
    worker.stop();
}
