use super::{Shortcut, ShortcutError, ShortcutId, Sink, NativeBackend};
#[cfg(test)]
use super::Backend;
use crate::{Tx, app::Shared, runtime::Waker};
use machine::{AttachEffect, AttachEvent, Attachment, BindEffect, BindEvent, Bindings, step_attachment, step_bindings};
use std::{collections::HashMap, rc::Rc, sync::{Arc, Mutex, OnceLock, PoisonError, RwLock, Weak, atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering}}};
#[cfg(not(target_os = "linux"))]
use std::cell::RefCell;
#[cfg(not(target_os = "linux"))]
use std::sync::atomic::AtomicUsize;
#[cfg(any(test, not(target_os = "linux")))]
use std::sync::mpsc;

mod machine;

type Handler<S> = Arc<dyn Fn(&mut S, &mut Tx) + Send + Sync>;
/// The host routing handed a registration to (0 until then), so a close clears only the hosts it ends.
type Stamp = Arc<AtomicU64>;
type Pending<S> = (Result<Shortcut, ShortcutError>, Handler<S>);
#[cfg(not(target_os = "linux"))]
type Job = Box<dyn FnOnce(&mut Engine) + Send>;
#[cfg(target_os = "linux")]
type Job = std::convert::Infallible;
type Connect = fn(bool, Sink) -> Result<NativeBackend, ShortcutError>;
#[cfg(not(target_os = "linux"))]
static NEXT_OWNER: AtomicUsize = AtomicUsize::new(1);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// Host generations, unique across Apps, so the receiving host a thread names is never another
/// App's.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
/// Every App's native registrations in this process, with the App's title, so a conflict
/// between two Apps can name its owner. Headless registrations stay out of it.
static NATIVE: Mutex<Vec<(ShortcutId, Shortcut, &'static str)>> = Mutex::new(Vec::new());

/// The Linux engine and the host that installed it. The wake travels with the engine, so a call
/// that reaches the engine a newer host installed wakes that host, not the one it captured.
#[cfg(target_os = "linux")]
struct Slot {
    engine: Arc<Mutex<Engine>>,
    wake: Waker,
    /// The generation of the host that installed it: only that host's close takes it.
    generation: u64,
}

/// An owner thread's engine and the generation of the host that installed it.
#[cfg(not(target_os = "linux"))]
type Installed = (u64, Rc<RefCell<Engine>>);

struct Control {
    #[cfg(not(target_os = "linux"))]
    key: usize,
    attachment: Mutex<Attachment<Job>>,
    #[cfg(target_os = "linux")]
    engine: Mutex<Option<Slot>>,
    /// How many hosts have attached; changed and read under the routing lock.
    hosts: AtomicU64,
    /// The newest host's generation. On Linux it is stored as a host installs its engine, under
    /// the slot lock, so the slot always holds the newest. Elsewhere it is stored as a host takes
    /// routing, under the routing lock, so it always names the host routing hands calls to. A
    /// guard, engine or activation holding another is stale, and changes nothing the newest owns.
    generation: AtomicU64,
}

pub(crate) struct Hub<S> {
    control: Arc<Control>,
    pending: Mutex<Vec<Pending<S>>>,
    handlers: Arc<Mutex<HashMap<ShortcutId, (Stamp, Handler<S>)>>>,
    errors: Mutex<Vec<ShortcutError>>,
    title: OnceLock<&'static str>,
    unmodified: AtomicBool,
}

impl<S> Default for Hub<S> {
    fn default() -> Self {
        Self {
            control: Arc::new(Control {
                #[cfg(not(target_os = "linux"))]
                key: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
                attachment: Mutex::new(Attachment::Detached),
                #[cfg(target_os = "linux")]
                engine: Mutex::default(),
                hosts: AtomicU64::new(0),
                generation: AtomicU64::new(0),
            }),
            pending: Mutex::default(), handlers: Arc::default(), errors: Mutex::default(), title: OnceLock::new(), unmodified: AtomicBool::new(false),
        }
    }
}

#[cfg(not(target_os = "linux"))]
thread_local! {
    // Native handles never cross threads, even though AppHandle can.
    static ENGINES: RefCell<HashMap<usize, Installed>> = RefCell::default();
}

thread_local! {
    /// The generation of the host delivering updates on this thread, while it delivers them.
    static RECEIVING: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    /// Runs as a drain is about to take the waiting calls, so a test can pause it there.
    #[cfg(all(test, not(target_os = "linux")))]
    static BEFORE_DEQUEUE: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::default();
    /// Runs as this thread is about to wake a host, so a test can pause it there.
    #[cfg(test)]
    static BEFORE_WAKE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::default();
}

/// A host's wake, which stops for good as the host closes. Its engine, its activations and
/// routing all wake the host through it, so a call that finishes after the close, or an
/// activation passed along after it, wakes nothing. A close waits out a wake already running,
/// so once a host's `Guard` has dropped its wake never fires; a host's wake must not wait for
/// that host's thread.
struct HostWake(RwLock<Option<Waker>>);

impl HostWake {
    /// The waker the host's engine, activations and routing hold, and what its close ends.
    /// Only they keep the host's wake alive, so one nothing reaches any more lets it go.
    fn new(wake: Waker) -> (Waker, Weak<Self>) {
        let host = Arc::new(Self(RwLock::new(Some(wake))));
        let closer = Arc::downgrade(&host);
        (Arc::new(move || host.wake()), closer)
    }

    fn wake(&self) {
        #[cfg(test)]
        if let Some(pause) = BEFORE_WAKE.with(|hook| hook.borrow_mut().take()) { pause(); }
        // Held through the wake, so a close can't finish while it runs. Wakes share it.
        let wake = self.0.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(wake) = wake.as_ref() { wake(); }
    }

    /// Ends the host's wake, once any wake under way has finished.
    fn close(host: &Weak<Self>) {
        let Some(host) = host.upgrade() else { return };
        let wake = host.0.write().unwrap_or_else(PoisonError::into_inner).take();
        // The host's own waker goes once the lock is free.
        drop(wake);
    }
}

fn next_generation() -> u64 { NEXT_GENERATION.fetch_add(1, Ordering::Relaxed) }

struct Engine {
    title: &'static str,
    wayland: bool,
    sink: Sink,
    connect: Connect,
    bindings: Bindings<NativeBackend>,
}

impl Engine {
    fn register(&mut self, id: ShortcutId, shortcut: Shortcut) -> Result<(), ShortcutError> {
        // Another App in this process holds it; some backends (the portal) wouldn't refuse.
        let claimed = !matches!(self.bindings, Bindings::Virtual(_)) && NATIVE.lock().unwrap().iter().any(|(_, s, _)| s == &shortcut);
        self.apply(BindEvent::Register { id, shortcut, claimed })
    }

    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> {
        self.apply(BindEvent::Unregister(id))
    }

    /// Who holds `shortcut`: this App, another App in this process, or the platform.
    /// Headless engines answer only for their own virtual registrations.
    fn owner(&self, shortcut: &Shortcut) -> Option<String> {
        if self.bindings.find(shortcut).is_some() { return Some(self.title.into()); }
        if matches!(self.bindings, Bindings::Virtual(_)) { return None; }
        let other = NATIVE.lock().unwrap().iter().find(|(_, s, _)| s == shortcut).map(|(.., title)| (*title).into());
        other.or_else(|| super::system_owner(shortcut))
    }

    fn release(&mut self) {
        let _ = self.apply(BindEvent::Release);
    }

    fn native(&mut self) -> &mut NativeBackend {
        let Bindings::Connected { native, .. } = &mut self.bindings else { unreachable!("the machine only asks a connected engine") };
        native
    }

    /// Steps the bindings and carries out what they ask for, answering with what came of it.
    fn apply(&mut self, event: BindEvent<NativeBackend>) -> Result<(), ShortcutError> {
        let mut answer = Ok(());
        for effect in step_bindings(&mut self.bindings, event) {
            match effect {
                BindEffect::Refuse(error) => answer = Err(error),
                BindEffect::Connect { id, shortcut } => answer = match (self.connect)(self.wayland, self.sink.clone()) {
                    Ok(native) => self.apply(BindEvent::Opened { native, id, shortcut }),
                    Err(error) => Err(error),
                },
                BindEffect::Bind { id, shortcut } => answer = match self.native().register(id, &shortcut) {
                    Ok(()) => self.apply(BindEvent::Bound { id, shortcut }),
                    Err(error) => self.apply(BindEvent::Rejected(error)),
                },
                BindEffect::Unbind(id) => answer = match self.native().unregister(id) {
                    Ok(()) => self.apply(BindEvent::Unbound(id)),
                    Err(error) => Err(error),
                },
                BindEffect::Claim { id, shortcut } => NATIVE.lock().unwrap().push((id, shortcut, self.title)),
                BindEffect::Disclaim(id) => NATIVE.lock().unwrap().retain(|(native, ..)| *native != id),
                BindEffect::DisclaimAll(registrations) => NATIVE.lock().unwrap().retain(|(id, ..)| !registrations.contains_key(id)),
                // Drop the connection/listener when the last registration goes away.
                BindEffect::Disconnect(native) => drop(native),
            }
        }
        answer
    }
}

impl Drop for Engine {
    fn drop(&mut self) { self.release(); }
}

impl Control {
    /// Whether `generation` belongs to the newest host.
    fn current(&self, generation: u64) -> bool { self.generation.load(Ordering::Relaxed) == generation }

    fn call<T: Send + 'static>(&self, f: impl FnOnce(&mut Engine) -> Result<T, ShortcutError> + Send + 'static) -> Result<T, ShortcutError> {
        self.call_stamped(None, f)
    }

    /// Runs a call, stamping it with the latest host while routing can't change.
    fn call_stamped<T: Send + 'static>(&self, stamp: Option<Stamp>, f: impl FnOnce(&mut Engine) -> Result<T, ShortcutError> + Send + 'static) -> Result<T, ShortcutError> {
        let mut route = self.attachment.lock().unwrap();
        let call = AttachEvent::Call { from: std::thread::current().id(), shared: cfg!(target_os = "linux") };
        let effect = step_attachment(&mut route, call).into_iter().next();
        if let Some(stamp) = stamp { stamp.store(self.hosts.load(Ordering::Relaxed), Ordering::Relaxed); }
        #[cfg(target_os = "linux")]
        {
            let Some(AttachEffect::Run { .. }) = effect else { return Err(ShortcutError::NotRunning) };
            // differs from dev: the routing wake only gates the call; the slot's host is woken.
            // Letting routing's copy go here means a host the call no longer reaches isn't kept
            // alive through another host's callback.
            drop(effect);
            let (shared, wake) = {
                let slot = self.engine.lock().unwrap();
                let slot = slot.as_ref().ok_or(ShortcutError::NotRunning)?;
                (slot.engine.clone(), slot.wake.clone())
            };
            // Acquire the engine before releasing routing, so shutdown cannot race a new call.
            let mut engine = shared.lock().unwrap();
            drop(route);
            let result = f(&mut engine);
            drop(engine);
            // A finished call must not keep a replaced engine, and its backend's grabs, alive.
            drop(shared);
            wake();
            result
        }
        #[cfg(not(target_os = "linux"))]
        match effect {
            Some(AttachEffect::Run { wake }) => {
                drop(route);
                let engine = ENGINES.with(|engines| engines.borrow().get(&self.key).map(|(_, engine)| engine.clone())).ok_or(ShortcutError::NotRunning)?;
                let result = f(&mut engine.borrow_mut());
                wake();
                result
            }
            Some(AttachEffect::Wait) => {
                let (tx, rx) = mpsc::sync_channel(1);
                let posted = step_attachment(&mut route, AttachEvent::Post(Box::new(move |engine| { let _ = tx.send(f(engine)); })));
                drop(route);
                for effect in posted { if let AttachEffect::Wake(wake) = effect { wake(); } }
                rx.recv().unwrap_or(Err(ShortcutError::NotRunning))
            }
            _ => Err(ShortcutError::NotRunning),
        }
    }
}

impl<S: 'static> Hub<S> {
    pub(super) fn pending(&self, shortcut: Result<Shortcut, ShortcutError>, handler: Handler<S>) {
        self.pending.lock().unwrap().push((shortcut, handler));
    }

    pub(super) fn register(&self, shortcut: Shortcut, handler: Handler<S>) -> Result<ShortcutId, ShortcutError> {
        if !self.unmodified.load(Ordering::Relaxed) { shortcut.require_modifier()?; }
        let id = NEXT_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map(ShortcutId).map_err(|_| ShortcutError::Backend("shortcut id space exhausted".into()))?;
        let stamp = Stamp::default();
        self.handlers.lock().unwrap().insert(id, (stamp.clone(), handler));
        if let Err(e) = self.control.call_stamped(Some(stamp), move |engine| engine.register(id, shortcut)) {
            self.handlers.lock().unwrap().remove(&id);
            return Err(e);
        }
        Ok(id)
    }

    pub(super) fn unregister(&self, id: ShortcutId) -> Result<(), ShortcutError> {
        let handlers = self.handlers.clone();
        self.control.call(move |engine| {
            engine.unregister(id)?;
            handlers.lock().unwrap().remove(&id);
            Ok(())
        })
    }

    pub(super) fn errors(&self) -> Vec<ShortcutError> { std::mem::take(&mut self.errors.lock().unwrap()) }

    pub(crate) fn named(&self, title: &'static str) { let _ = self.title.set(title); }

    pub(super) fn allow_unmodified(&self) { self.unmodified.store(true, Ordering::Relaxed); }

    pub(super) fn owner(&self, shortcut: &Shortcut) -> Option<String> {
        let shortcut = shortcut.clone();
        self.control.call(move |engine| Ok(engine.owner(&shortcut))).ok().flatten()
    }

    pub(crate) fn fire(&self, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        let shortcut = shortcut.clone();
        self.control.call(move |engine| {
            let id = engine.bindings.find(&shortcut).ok_or(ShortcutError::UnknownId)?;
            (engine.sink)(id, None);
            Ok(())
        })
    }
}

/// Owned by the host, not AppHandle: closing/dropping the app releases native registrations.
pub(crate) struct Guard {
    control: Arc<Control>,
    /// The generation this host's engine was installed with.
    generation: u64,
    /// Told the last host the close ends.
    cleanup: Option<Box<dyn FnOnce(u64)>>,
    /// Held while the host lives, so an activation left for it is dropped once it has gone.
    _live: Arc<()>,
    /// Ended first as the host closes, so nothing wakes it once the guard has dropped.
    closes_wake: Weak<HostWake>,
    // The guard must be destroyed on the owning event-loop thread.
    _thread: std::marker::PhantomData<Rc<()>>,
}

/// A host whose engine is installed, before it takes routing.
struct Attaching {
    control: Arc<Control>,
    generation: u64,
    /// The host's wake, which `closes_wake` ends.
    wake: Waker,
    closes_wake: Weak<HostWake>,
    live: Arc<()>,
    // Routing names this thread as the owner, and a macOS or Windows engine never leaves it.
    _thread: std::marker::PhantomData<Rc<()>>,
}

/// While held, this thread delivers updates for the host that made it.
pub(crate) struct Receiving(Option<u64>);

impl Drop for Receiving {
    fn drop(&mut self) { RECEIVING.with(|receiving| receiving.set(self.0)); }
}

/// The host an engine's activations belong to.
struct Delivery<S> {
    shared: std::sync::Weak<Shared<S>>,
    generation: u64,
    wake: Waker,
    live: std::sync::Weak<()>,
    /// The host's presses, oldest first. Each activation queued for them runs the oldest, so
    /// however hosts pass the activations along, this host runs its presses in the order they came.
    pressed: Mutex<std::collections::VecDeque<(ShortcutId, Option<String>)>>,
}

/// A close part-way through.
struct Closing {
    /// The last host the close ends: handlers stamped after it stay.
    ended: u64,
    /// This host was still the newest once routing closed. A stale close clears no handlers.
    current: bool,
}

/// What a backend calls when its chord is pressed: it queues the activation and wakes the App.
fn sink<S: 'static>(delivery: Arc<Delivery<S>>) -> Sink {
    Arc::new(move |id, token| {
        if let Some(shared) = delivery.shared.upgrade() {
            delivery.pressed.lock().unwrap().push_back((id, token));
            shared.queue_update(deliver(delivery.clone()));
            shared.wake();
        }
    })
}

/// An activation, as an update: it runs the host's oldest press. It runs only on its own host's
/// state, and only while that host is the newest. Another host delivering it leaves it for its own
/// host, and wakes that host; an activation whose host was replaced or has gone runs nothing, and
/// discards a press, so the host's presses are only those that can still run.
fn deliver<S: 'static>(delivery: Arc<Delivery<S>>) -> Box<dyn FnOnce(&mut S, &mut Tx) -> bool + Send> {
    Box::new(move |state, tx| {
        // Once stale, a host's presses all are, so which one goes doesn't matter.
        let discard = || { delivery.pressed.lock().unwrap().pop_front(); false };
        let Some(shared) = delivery.shared.upgrade() else { return discard() };
        if !shared.shortcuts.control.current(delivery.generation) { return discard(); }
        if RECEIVING.with(|receiving| receiving.get()) != Some(delivery.generation) {
            if delivery.live.strong_count() == 0 { return discard(); }
            let wake = delivery.wake.clone();
            shared.queue_update(deliver(delivery));
            wake();
            return false;
        }
        let Some((id, token)) = delivery.pressed.lock().unwrap().pop_front() else { return false };
        let Some(handler) = shared.shortcuts.handlers.lock().unwrap().get(&id).map(|(_, handler)| handler.clone()) else { return false };
        tx.shortcut_activation_token = token;
        handler(state, tx);
        tx.shortcut_activation_token = None;
        true
    })
}

impl Guard {
    pub(crate) fn attach<S: 'static>(shared: &Arc<Shared<S>>, wake: Waker, headless: bool, wayland: bool) -> Self {
        Self::attach_with(shared, wake, headless, wayland, super::backend)
    }

    fn attach_with<S: 'static>(shared: &Arc<Shared<S>>, wake: Waker, headless: bool, wayland: bool, connect: Connect) -> Self {
        Self::install(shared, wake, headless, wayland, connect).route(shared)
    }

    /// The first half of an attach: a new engine, with the next generation, replaces the one this
    /// thread (or, on Linux, the App) had.
    fn install<S: 'static>(shared: &Arc<Shared<S>>, wake: Waker, headless: bool, wayland: bool, connect: Connect) -> Attaching {
        let control = shared.shortcuts.control.clone();
        let title = shared.shortcuts.title.get().copied().unwrap_or_default();
        let bindings = if headless { Bindings::Virtual(HashMap::new()) } else { Bindings::Disconnected };
        let live = Arc::new(());
        // Everything below wakes the host through this, so its close ends them all.
        let (wake, closes_wake) = HostWake::new(wake);
        let engine = |generation| {
            let delivery = Delivery { shared: Arc::downgrade(shared), generation, wake: wake.clone(), live: Arc::downgrade(&live), pressed: Mutex::default() };
            Engine { title, wayland, sink: sink(Arc::new(delivery)), connect, bindings }
        };
        // Published when this host takes routing.
        #[cfg(not(target_os = "linux"))]
        let generation = {
            let generation = next_generation();
            ENGINES.with(|engines| engines.borrow_mut().insert(control.key, (generation, Rc::new(RefCell::new(engine(generation))))));
            generation
        };
        // A replaced engine closes with the slot locked, so callers wait until its chords are free.
        #[cfg(target_os = "linux")]
        let generation = {
            let mut slot = control.engine.lock().unwrap();
            let generation = next_generation();
            control.generation.store(generation, Ordering::Relaxed);
            *slot = Some(Slot { engine: Arc::new(Mutex::new(engine(generation))), wake: wake.clone(), generation });
            generation
        };
        Attaching { control, generation, wake, closes_wake, live, _thread: std::marker::PhantomData }
    }

    /// Names this host as the one delivering updates on this thread until the result drops.
    pub(crate) fn receiving(&self) -> Receiving {
        Receiving(RECEIVING.with(|receiving| receiving.replace(Some(self.generation))))
    }

    pub(crate) fn drain(&self) {
        #[cfg(not(target_os = "linux"))]
        {
            #[cfg(test)]
            if let Some(pause) = BEFORE_DEQUEUE.with(|hook| hook.borrow_mut().take()) { pause(); }
            // Calls wait for the routed host, so another host leaves them for it. Routing changes
            // only under this lock, so the host it names is the one the jobs were posted to.
            let drained = step_attachment(&mut self.control.attachment.lock().unwrap(), AttachEvent::Drain { host: self.generation });
            let Some(AttachEffect::Jobs(jobs)) = drained.into_iter().next() else { return };
            // The routed host's thread holds its engine.
            let engine = ENGINES.with(|engines| engines.borrow().get(&self.control.key).filter(|(generation, _)| *generation == self.generation).map(|(_, engine)| engine.clone()));
            if let Some(engine) = engine { for job in jobs { job(&mut engine.borrow_mut()); } }
        }
    }

    /// Ends routing, whichever host holds it, and releases the callers waiting on it.
    fn close_routing(&self) -> Closing {
        let mut route = self.control.attachment.lock().unwrap();
        let closed = step_attachment(&mut route, AttachEvent::Close);
        let ended = self.control.hosts.load(Ordering::Relaxed);
        // A host that installs after this reads newer, and counts after `ended`.
        let current = self.control.current(self.generation);
        drop(route);
        // Dropping queued reply senders releases waiting callers with NotRunning.
        for effect in closed { if let AttachEffect::Release(jobs) = effect { drop(jobs); } }
        Closing { ended, current }
    }

    /// Takes and releases this host's own engine, and never one a newer host installed.
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
    fn release_engine(&self, closing: &mut Closing) {
        #[cfg(not(target_os = "linux"))]
        ENGINES.with(|engines| {
            let own = engines.borrow().get(&self.control.key).is_some_and(|(generation, _)| *generation == self.generation);
            if own { engines.borrow_mut().remove(&self.control.key); }
        });
        // The slot stays locked through the release, so a new host can't start while this one's
        // backend still holds its chords.
        #[cfg(target_os = "linux")]
        {
            let mut slot = self.control.engine.lock().unwrap();
            if slot.as_ref().is_some_and(|slot| slot.generation == self.generation) && let Some(own) = slot.take() {
                // As on dev, the close ends every host counted by now. None installed since this
                // one, or the slot would hold its engine.
                closing.ended = self.control.hosts.load(Ordering::Relaxed);
                own.engine.lock().unwrap().release();
            }
        }
    }

    /// Clears the handlers of the hosts the close ended, unless a newer host owns them now.
    fn end(&mut self, closing: Closing) {
        if !closing.current { return; }
        if let Some(cleanup) = self.cleanup.take() { cleanup(closing.ended); }
    }
}

impl Attaching {
    /// The second half of an attach: this host takes routing, then registers what waited for it.
    fn route<S: 'static>(self, shared: &Arc<Shared<S>>) -> Guard {
        let Self { control, generation, wake, closes_wake, live, .. } = self;
        let routed = {
            let mut route = control.attachment.lock().unwrap();
            control.hosts.fetch_add(1, Ordering::Relaxed);
            // The routed host is the newest, in the same step that routes to it.
            #[cfg(not(target_os = "linux"))]
            control.generation.store(generation, Ordering::Relaxed);
            step_attachment(&mut route, AttachEvent::Attach { owner: std::thread::current().id(), wake, host: generation })
        };
        // Calls this host inherited wait for it, so it is woken for them, once routing is free.
        for effect in routed { if let AttachEffect::Wake(wake) = effect { wake(); } }
        let handlers = shared.shortcuts.handlers.clone();
        // A host that attached after the close took routing keeps its handlers.
        let cleanup = move |ended: u64| handlers.lock().unwrap().retain(|_, (stamp, _)| stamp.load(Ordering::Relaxed) > ended);
        let guard = Guard { control, generation, cleanup: Some(Box::new(cleanup)), _live: live, closes_wake, _thread: std::marker::PhantomData };
        for (shortcut, handler) in std::mem::take(&mut *shared.shortcuts.pending.lock().unwrap()) {
            if let Err(error) = shortcut.and_then(|s| shared.shortcuts.register(s, handler)) {
                eprintln!("rune: {error}");
                shared.shortcuts.errors.lock().unwrap().push(error);
            }
        }
        guard
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Holding nothing, so a wake under way can finish.
        HostWake::close(&self.closes_wake);
        let mut closing = self.close_routing();
        self.release_engine(&mut closing);
        self.end(closing);
    }
}

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod routing_tests;
#[cfg(test)]
mod host_handler_tests;
#[cfg(test)]
mod generation_tests;
#[cfg(test)]
mod closed_wake_tests;
#[cfg(all(test, target_os = "linux"))]
mod teardown_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{App, AppSpec, el};
    #[test]
    fn mock_backend_errors_are_not_confused_with_conflicts() {
        struct Mock(ShortcutError);
        impl Backend for Mock {
            fn register(&mut self, _: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> { Err(self.0.clone()) }
            fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
        }
        for error in [ShortcutError::Conflict, ShortcutError::Unsupported, ShortcutError::PermissionDenied, ShortcutError::Backend("disconnected".into())] {
            let bindings = Bindings::Connected { native: Box::new(Mock(error.clone())) as NativeBackend, registrations: HashMap::new() };
            let mut engine = Engine { title: "mock", wayland: false, sink: Arc::new(|_, _| {}), connect: super::super::backend, bindings };
            assert_eq!(engine.register(ShortcutId(1), "ctrl+a".parse().unwrap()), Err(error));
            assert!(matches!(engine.bindings, Bindings::Disconnected));
        }
        assert_eq!(super::super::native_status(-9878, -9878), Err(ShortcutError::Conflict));
        assert_eq!(super::super::native_status(1409, 1409), Err(ShortcutError::Conflict));
        assert!(matches!(super::super::native_status(5, 1409), Err(ShortcutError::Backend(_))));
    }

    #[test]
    fn conflicts_name_the_app_that_holds_the_chord() {
        struct Accept;
        impl Backend for Accept {
            fn register(&mut self, _: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> { Ok(()) }
            fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
        }
        let chord: Shortcut = "ctrl+alt+shift+f17".parse().unwrap();
        let native = |title| Engine { title, wayland: false, sink: Arc::new(|_, _| {}), connect: super::super::backend, bindings: Bindings::Connected { native: Box::new(Accept) as NativeBackend, registrations: HashMap::new() } };
        let (mut first, mut second) = (native("First"), native("Second"));
        first.register(ShortcutId(u32::MAX - 1), chord.clone()).unwrap();
        assert_eq!(second.register(ShortcutId(u32::MAX - 2), chord.clone()), Err(ShortcutError::Conflict),
            "refused even when the backend accepts a same-process duplicate");
        assert_eq!(first.owner(&chord).as_deref(), Some("First"));
        assert_eq!(second.owner(&chord).as_deref(), Some("First"));
        drop(first);
        // Off the main thread the system lookup answers None, so the claim is gone entirely.
        assert_eq!(second.owner(&chord), None, "a dropped App releases its claim");
        second.register(ShortcutId(u32::MAX - 2), chord.clone()).unwrap();

        let app = App::new(AppSpec::new("Owner", (10.0, 10.0), || (), |_, _| el()));
        let handle = app.handle();
        let chord: Shortcut = "ctrl+alt+o".parse().unwrap();
        assert_eq!(handle.shortcut_owner(&chord), None, "not running");
        let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
        handle.register_shortcut(chord.clone(), |_, _| {}).unwrap();
        assert_eq!(handle.register_shortcut(chord.clone(), |_, _| {}), Err(ShortcutError::Conflict));
        assert_eq!(handle.shortcut_owner(&chord).as_deref(), Some("Owner"));
        // Headless runs never ask the desktop, so stills are the same everywhere.
        assert_eq!(handle.shortcut_owner(&"super+space".parse().unwrap()), None);
        drop(guard);
    }

    #[test]
    fn headless_handlers_share_state_and_unregister_cancels_queued_activation() {
        let app = App::new(AppSpec::new("shortcuts", (100.0, 100.0), || 0, |_: &u32, _| el()))
            .global_shortcut("cmd+shift+space", |s, tx| { *s += 1; tx.instant(); });
        let handle = app.handle();
        let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
        let shortcut = "cmd+shift+space".parse().unwrap();
        app.shared.shortcuts.fire(&shortcut).unwrap();
        let mut state = 0;
        let receiving = guard.receiving();
        for f in app.shared.take().updates { f(&mut state, &mut Tx::default()); }
        assert_eq!(state, 1);
        assert_eq!(handle.register_shortcut(shortcut, |_, _| {}), Err(ShortcutError::Conflict));
        let id = handle.register_shortcut("ctrl+a".parse().unwrap(), |s, _| *s += 10).unwrap();
        app.shared.shortcuts.fire(&"ctrl+a".parse().unwrap()).unwrap();
        handle.unregister_shortcut(id).unwrap();
        for f in app.shared.take().updates { f(&mut state, &mut Tx::default()); }
        assert_eq!(state, 1);
        drop(receiving);
        drop(guard);
        assert_eq!(handle.unregister_shortcut(id), Err(ShortcutError::NotRunning));
    }

    #[test]
    fn a_chord_without_a_modifier_is_refused_unless_it_is_a_function_key() {
        let app = App::new(AppSpec::new("bare", (10.0, 10.0), || (), |_, _| el())).global_shortcut("space", |_, _| {});
        let handle = app.handle();
        let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
        assert!(matches!(handle.take_shortcut_errors().as_slice(), [ShortcutError::InvalidAccelerator(_)]));
        for bare in ["space", "enter", "a", "shift+a", "shift+tab"] {
            assert!(matches!(handle.register_shortcut(bare.parse().unwrap(), |_, _| {}), Err(ShortcutError::InvalidAccelerator(_))), "{bare}");
        }
        for allowed in ["f5", "f24", "shift+f1", "ctrl+space", "alt+a", "super+enter"] {
            assert!(handle.register_shortcut(allowed.parse().unwrap(), |_, _| {}).is_ok(), "{allowed}");
        }
        drop(guard);

        let app = App::new(AppSpec::new("bare", (10.0, 10.0), || (), |_, _| el())).global_shortcut("space", |_, _| {})
            .allow_unmodified_shortcuts();
        let handle = app.handle();
        let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
        assert!(handle.take_shortcut_errors().is_empty());
        assert!(handle.register_shortcut("shift+a".parse().unwrap(), |_, _| {}).is_ok());
        drop(guard);
    }

    #[test]
    fn background_registration_wakes_and_returns_the_backend_result() {
        let app = App::new(AppSpec::new("shortcuts", (10.0, 10.0), || (), |_, _| el()));
        let (wake, woke) = mpsc::channel();
        let guard = Guard::attach(&app.shared, Arc::new(move || { wake.send(()).unwrap(); }), true, false);
        let handle = app.handle();
        let thread = std::thread::spawn(move || handle.register_shortcut("ctrl+space".parse().unwrap(), |_, _| {}));
        woke.recv().unwrap();
        guard.drain();
        assert!(thread.join().unwrap().is_ok());
    }

    #[test]
    fn activation_tokens_are_scoped_to_the_handler_and_shutdown_breaks_handle_cycles() {
        let app = App::new(AppSpec::new("shortcuts", (10.0, 10.0), || false, |_, _| el()));
        let weak = Arc::downgrade(&app.shared);
        let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
        let captured = app.handle();
        let id = app.handle().register_shortcut("ctrl+space".parse().unwrap(), move |state, tx| {
            assert_eq!(tx.activation_token(), Some("compositor-token"));
            captured.activate(tx.activation_token().unwrap());
            *state = true;
        }).unwrap();
        app.shared.shortcuts.control.call(move |engine| {
            (engine.sink)(id, Some("compositor-token".into()));
            Ok(())
        }).unwrap();
        let mut state = false;
        let mut tx = Tx::default();
        let receiving = guard.receiving();
        for f in app.shared.take().updates { f(&mut state, &mut tx); }
        drop(receiving);
        assert!(state);
        assert_eq!(tx.activation_token(), None);
        assert_eq!(app.shared.take().window, Some(crate::app::WindowRequest::Open(Some("compositor-token".into()))));
        drop(app);
        drop(guard);
        assert!(weak.upgrade().is_none(), "handler captures must be released at host shutdown");
    }

    /// Whether a backend saw its chord still claimed as it was dropped.
    static RETIRED_WITH_CLAIM: Mutex<Vec<bool>> = Mutex::new(Vec::new());
    const GRAB_CHORD: &str = "ctrl+alt+shift+f11";

    struct Grab;

    impl Backend for Grab {
        fn register(&mut self, _: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> { Ok(()) }
        fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
    }

    // A desktop grab stands until the backend is gone, so the claim that names it must stand too.
    impl Drop for Grab {
        fn drop(&mut self) {
            let held = NATIVE.lock().unwrap().iter().any(|(.., s, _)| *s == GRAB_CHORD.parse::<Shortcut>().unwrap());
            RETIRED_WITH_CLAIM.lock().unwrap().push(held);
        }
    }

    fn grab(_: bool, _: Sink) -> Result<NativeBackend, ShortcutError> { Ok(Box::new(Grab)) }

    #[test]
    fn a_backend_retires_before_its_chord_stops_naming_an_owner() {
        RETIRED_WITH_CLAIM.lock().unwrap().clear();
        let shortcut = GRAB_CHORD.parse::<Shortcut>().unwrap();
        let mut engine = Engine { title: "Grab", wayland: false, sink: Arc::new(|_, _| {}), connect: grab, bindings: Bindings::Disconnected };
        let id = ShortcutId(0x7000_0001);
        engine.register(id, shortcut.clone()).unwrap();
        engine.unregister(id).unwrap();
        assert!(engine.bindings.find(&shortcut).is_none(), "the registration went");
        assert!(!NATIVE.lock().unwrap().iter().any(|(.., s, _)| *s == shortcut), "the claim went with the registration");
        assert_eq!(*RETIRED_WITH_CLAIM.lock().unwrap(), vec![true], "the backend saw its chord still claimed while it was alive");
    }
}
