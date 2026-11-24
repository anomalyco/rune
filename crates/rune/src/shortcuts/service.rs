use super::{NativeBackend, Shortcut, ShortcutDetails, ShortcutError, ShortcutId, Sink};
#[cfg(test)]
use super::Backend;
use crate::{Tx, app::Shared, runtime::Waker};
use machine::{AttachEffect, AttachEvent, Attachment, BindEffect, BindEvent, Bindings, step_attachment, step_bindings};
use std::{collections::HashMap, rc::Rc, sync::{Arc, Mutex, OnceLock, PoisonError, Weak, atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering}}};
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
type Pending<S> = (Result<Shortcut, ShortcutError>, ShortcutDetails, Handler<S>);
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

pub(crate) struct HubCore {
    control: Arc<Control>,
    errors: Mutex<Vec<ShortcutError>>,
    title: OnceLock<&'static str>,
    unmodified: AtomicBool,
}

impl Default for HubCore {
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
            errors: Mutex::default(),
            title: OnceLock::new(),
            unmodified: AtomicBool::new(false),
        }
    }
}

pub(crate) struct Hub<S> {
    pub(crate) core: Arc<HubCore>,
    pending: Mutex<Vec<Pending<S>>>,
    handlers: Arc<Mutex<HashMap<ShortcutId, (Stamp, Handler<S>)>>>,
}

impl<S> Default for Hub<S> {
    fn default() -> Self {
        Self { core: Arc::default(), pending: Mutex::default(), handlers: Arc::default() }
    }
}

impl<S> std::ops::Deref for Hub<S> {
    type Target = HubCore;
    fn deref(&self) -> &HubCore { &self.core }
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
/// activation passed along after it, wakes nothing. The lock is held only to clone the waker,
/// never through the wake, so a close never waits for a wake (whose callback may wait for the
/// close). A wake that took the waker just before the close still fires, and the closed host's
/// event loop ignores it.
struct HostWake(Mutex<Option<Waker>>);

impl HostWake {
    /// The waker the host's engine, activations and routing hold, and what its close ends.
    /// Only they keep the host's wake alive, so one nothing reaches any more lets it go.
    fn new(wake: Waker) -> (Waker, Weak<Self>) {
        let host = Arc::new(Self(Mutex::new(Some(wake))));
        let closer = Arc::downgrade(&host);
        (Arc::new(move || host.wake()), closer)
    }

    fn wake(&self) {
        #[cfg(test)]
        if let Some(pause) = BEFORE_WAKE.with(|hook| hook.borrow_mut().take()) { pause(); }
        let wake = self.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(wake) = wake { wake(); }
    }

    /// Ends the host's wake: a wake that starts after this fires nothing.
    fn close(host: &Weak<Self>) {
        let Some(host) = host.upgrade() else { return };
        let wake = host.0.lock().unwrap_or_else(PoisonError::into_inner).take();
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
    #[cfg(test)]
    fn register(&mut self, id: ShortcutId, shortcut: Shortcut) -> Result<(), ShortcutError> {
        self.register_with(id, shortcut, ShortcutDetails::default())
    }

    fn register_with(&mut self, id: ShortcutId, shortcut: Shortcut, details: ShortcutDetails) -> Result<(), ShortcutError> {
        // Another App in this process holds it; some backends (the portal) wouldn't refuse.
        let claimed = !matches!(self.bindings, Bindings::Virtual(_)) && NATIVE.lock().unwrap().iter().any(|(_, s, _)| s == &shortcut);
        self.apply(BindEvent::Register { id, shortcut, claimed }, &details)
    }

    fn register_batch(&mut self, items: Vec<(ShortcutId, Shortcut, ShortcutDetails)>) -> Vec<Result<(), ShortcutError>> {
        if items.is_empty() { return Vec::new(); }
        if items.len() == 1 {
            let (id, shortcut, details) = items.into_iter().next().unwrap();
            return vec![self.register_with(id, shortcut, details)];
        }
        if matches!(self.bindings, Bindings::Virtual(_)) {
            return items.into_iter().map(|(id, shortcut, details)| self.register_with(id, shortcut, details)).collect();
        }
        let mut results = vec![Ok(()); items.len()];
        let mut to_bind: Vec<(usize, ShortcutId, Shortcut, ShortcutDetails)> = Vec::new();
        {
            let native_claims = NATIVE.lock().unwrap();
            for (idx, (id, shortcut, details)) in items.into_iter().enumerate() {
                let duplicate = native_claims.iter().any(|(_, s, _)| s == &shortcut)
                    || self.bindings.find(&shortcut).is_some()
                    || to_bind.iter().any(|(_, _, s, _)| s == &shortcut);
                if duplicate {
                    results[idx] = Err(ShortcutError::Conflict);
                } else {
                    to_bind.push((idx, id, shortcut, details));
                }
            }
        }
        if to_bind.is_empty() { return results; }
        if matches!(self.bindings, Bindings::Disconnected) {
            match (self.connect)(self.wayland, self.sink.clone()) {
                Ok(native) => self.bindings = Bindings::Connected { native, registrations: HashMap::new() },
                Err(error) => {
                    for (idx, ..) in to_bind { results[idx] = Err(error.clone()); }
                    return results;
                }
            }
        }
        let batch_items: Vec<(ShortcutId, Shortcut, ShortcutDetails)> =
            to_bind.iter().map(|(_, id, s, d)| (*id, s.clone(), d.clone())).collect();
        let outcomes = self.native().register_batch(&batch_items);
        let empty = ShortcutDetails::default();
        for ((idx, id, shortcut, _), outcome) in to_bind.into_iter().zip(outcomes) {
            match outcome {
                Ok(()) => { self.apply(BindEvent::Bound { id, shortcut }, &empty).ok(); }
                Err(error) => results[idx] = Err(error),
            }
        }
        if matches!(&self.bindings, Bindings::Connected { registrations, .. } if registrations.is_empty()) {
            self.apply(BindEvent::Rejected(ShortcutError::Unsupported), &empty).ok();
        }
        results
    }

    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> {
        self.apply(BindEvent::Unregister(id), &ShortcutDetails::default())
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
        self.apply(BindEvent::Release, &ShortcutDetails::default()).ok();
    }

    fn native(&mut self) -> &mut NativeBackend {
        let Bindings::Connected { native, .. } = &mut self.bindings else { unreachable!("the machine only asks a connected engine") };
        native
    }

    /// Steps the bindings and carries out what they ask for, answering with what came of it.
    fn apply(&mut self, event: BindEvent<NativeBackend>, details: &ShortcutDetails) -> Result<(), ShortcutError> {
        let mut answer = Ok(());
        for effect in step_bindings(&mut self.bindings, event) {
            match effect {
                BindEffect::Refuse(error) => answer = Err(error),
                BindEffect::Connect { id, shortcut } => answer = match (self.connect)(self.wayland, self.sink.clone()) {
                    Ok(native) => self.apply(BindEvent::Opened { native, id, shortcut }, details),
                    Err(error) => Err(error),
                },
                BindEffect::Bind { id, shortcut } => answer = match self.native().register_with(id, &shortcut, details) {
                    Ok(()) => self.apply(BindEvent::Bound { id, shortcut }, details),
                    Err(error) => self.apply(BindEvent::Rejected(error), details),
                },
                BindEffect::Unbind(id) => answer = match self.native().unregister(id) {
                    Ok(()) => self.apply(BindEvent::Unbound(id), details),
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
        let stamps: Vec<Stamp> = stamp.into_iter().collect();
        self.call_stamped_many(&stamps, f)
    }

    /// Runs a call, stamping every registration in a batch with the latest host while routing can't change.
    fn call_stamped_many<T: Send + 'static>(&self, stamps: &[Stamp], f: impl FnOnce(&mut Engine) -> Result<T, ShortcutError> + Send + 'static) -> Result<T, ShortcutError> {
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        self.call_erased(stamps, Box::new(move |engine| {
            *out.lock().unwrap() = Some(f(engine));
        }))?;
        slot.lock().unwrap().take().unwrap_or(Err(ShortcutError::NotRunning))
    }

    fn call_erased(&self, stamps: &[Stamp], run: Box<dyn FnOnce(&mut Engine) + Send>) -> Result<(), ShortcutError> {
        let mut route = self.attachment.lock().unwrap();
        let call = AttachEvent::Call { from: std::thread::current().id(), shared: cfg!(target_os = "linux") };
        let effect = step_attachment(&mut route, call).into_iter().next();
        if !stamps.is_empty() {
            let host = self.hosts.load(Ordering::Relaxed);
            for stamp in stamps { stamp.store(host, Ordering::Relaxed); }
        }
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
            run(&mut engine);
            drop(engine);
            // A finished call must not keep a replaced engine, and its backend's grabs, alive.
            drop(shared);
            wake();
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        match effect {
            Some(AttachEffect::Run { wake }) => {
                drop(route);
                let engine = ENGINES.with(|engines| engines.borrow().get(&self.key).map(|(_, engine)| engine.clone())).ok_or(ShortcutError::NotRunning)?;
                run(&mut engine.borrow_mut());
                wake();
                Ok(())
            }
            Some(AttachEffect::Wait) => {
                let (done_tx, done_rx) = mpsc::sync_channel(1);
                let posted = step_attachment(&mut route, AttachEvent::Post(Box::new(move |engine| {
                    run(engine);
                    done_tx.send(()).ok();
                })));
                drop(route);
                for effect in posted { if let AttachEffect::Wake(wake) = effect { wake(); } }
                done_rx.recv().map_err(|_| ShortcutError::NotRunning)
            }
            _ => Err(ShortcutError::NotRunning),
        }
    }
}

impl HubCore {
    fn prepare_register(&self, shortcut: &Shortcut) -> Result<(ShortcutId, Stamp), ShortcutError> {
        if !self.unmodified.load(Ordering::Relaxed) { shortcut.require_modifier()?; }
        let id = NEXT_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map(ShortcutId).map_err(|_| ShortcutError::Backend("shortcut id space exhausted".into()))?;
        Ok((id, Stamp::default()))
    }

    fn commit_register(&self, id: ShortcutId, shortcut: Shortcut, details: ShortcutDetails, stamp: Stamp) -> Result<(), ShortcutError> {
        self.control.call_stamped(Some(stamp), move |engine| engine.register_with(id, shortcut, details))
    }

    fn commit_register_batch(
        &self,
        items: Vec<(ShortcutId, Shortcut, ShortcutDetails)>,
        stamps: &[Stamp],
    ) -> Result<Vec<Result<(), ShortcutError>>, ShortcutError> {
        self.control.call_stamped_many(stamps, move |engine| Ok(engine.register_batch(items)))
    }

    fn unregister_engine(&self, id: ShortcutId, remove: Box<dyn FnOnce() + Send>) -> Result<(), ShortcutError> {
        self.control.call(move |engine| {
            engine.unregister(id)?;
            remove();
            Ok(())
        })
    }

    pub(super) fn errors(&self) -> Vec<ShortcutError> { std::mem::take(&mut self.errors.lock().unwrap()) }

    fn record_error(&self, error: ShortcutError) {
        eprintln!("rune: {error}");
        self.errors.lock().unwrap().push(error);
    }

    pub(crate) fn named(&self, title: &'static str) { self.title.set(title).ok(); }

    pub(super) fn allow_unmodified(&self) { self.unmodified.store(true, Ordering::Relaxed); }

    pub(super) fn owner(&self, shortcut: &Shortcut) -> Option<String> {
        let shortcut = shortcut.clone();
        self.control.call(move |engine| Ok(engine.owner(&shortcut))).ok().flatten()
    }

    pub(crate) fn fire(&self, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        let shortcut = shortcut.clone();
        let (id, sink) = self.control.call(move |engine| {
            let id = engine.bindings.find(&shortcut).ok_or(ShortcutError::UnknownId)?;
            Ok((id, engine.sink.clone()))
        })?;
        // Pressed as a backend would, holding nothing, so the press's wake runs under no lock.
        sink(id, None);
        Ok(())
    }
}

impl<S: 'static> Hub<S> {
    pub(super) fn pending(&self, shortcut: Result<Shortcut, ShortcutError>, details: ShortcutDetails, handler: Handler<S>) {
        self.pending.lock().unwrap().push((shortcut, details, handler));
    }

    pub(super) fn register(&self, shortcut: Shortcut, details: ShortcutDetails, handler: Handler<S>) -> Result<ShortcutId, ShortcutError> {
        let (id, stamp) = self.core.prepare_register(&shortcut)?;
        self.handlers.lock().unwrap().insert(id, (stamp.clone(), handler));
        if let Err(e) = self.core.commit_register(id, shortcut, details, stamp) {
            self.handlers.lock().unwrap().remove(&id);
            return Err(e);
        }
        Ok(id)
    }

    pub(super) fn unregister(&self, id: ShortcutId) -> Result<(), ShortcutError> {
        let handlers = self.handlers.clone();
        self.core.unregister_engine(id, Box::new(move || { handlers.lock().unwrap().remove(&id); }))
    }

    fn register_pending(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        if pending.is_empty() { return; }
        let mut outcomes: Vec<Option<ShortcutError>> = vec![None; pending.len()];
        let mut ready_indices = Vec::new();
        let mut ready_ids = Vec::new();
        let mut ready_items = Vec::new();
        let mut ready_stamps = Vec::new();
        for (idx, (shortcut, details, handler)) in pending.into_iter().enumerate() {
            match shortcut.and_then(|s| self.core.prepare_register(&s).map(|(id, stamp)| (id, stamp, s))) {
                Ok((id, stamp, s)) => {
                    self.handlers.lock().unwrap().insert(id, (stamp.clone(), handler));
                    ready_indices.push(idx);
                    ready_ids.push(id);
                    ready_items.push((id, s, details));
                    ready_stamps.push(stamp);
                }
                Err(error) => outcomes[idx] = Some(error),
            }
        }
        if !ready_items.is_empty() {
            match self.core.commit_register_batch(ready_items, &ready_stamps) {
                Ok(results) => {
                    let mut handlers = self.handlers.lock().unwrap();
                    for ((idx, id), result) in ready_indices.into_iter().zip(ready_ids).zip(results) {
                        if let Err(error) = result {
                            handlers.remove(&id);
                            outcomes[idx] = Some(error);
                        }
                    }
                }
                Err(error) => {
                    let mut handlers = self.handlers.lock().unwrap();
                    for (idx, id) in ready_indices.into_iter().zip(ready_ids) {
                        handlers.remove(&id);
                        outcomes[idx] = Some(error.clone());
                    }
                }
            }
        }
        for error in outcomes.into_iter().flatten() {
            self.core.record_error(error);
        }
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

/// What a backend calls when its chord is pressed: it queues the activation and wakes the
/// engine's own host, never the window that connected last, which may have closed. A press for
/// a host that has gone queues nothing, as its activation would run nothing.
fn sink<S: 'static>(delivery: Arc<Delivery<S>>) -> Sink {
    Arc::new(move |id, token| {
        if delivery.live.strong_count() == 0 { return; }
        if let Some(shared) = delivery.shared.upgrade() {
            delivery.pressed.lock().unwrap().push_back((id, token));
            shared.queue_update(deliver(delivery.clone()));
            (delivery.wake)();
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
        Self::install_core(&shared.shortcuts.core, wake, headless, wayland, connect, &mut |generation, wake, live| {
            sink(Arc::new(Delivery { shared: Arc::downgrade(shared), generation, wake, live, pressed: Mutex::default() }))
        })
    }

    fn install_core(
        core: &HubCore,
        wake: Waker,
        headless: bool,
        wayland: bool,
        connect: Connect,
        make_sink: &mut dyn FnMut(u64, Waker, Weak<()>) -> Sink,
    ) -> Attaching {
        let control = core.control.clone();
        let title = core.title.get().copied().unwrap_or_default();
        let bindings = if headless { Bindings::Virtual(HashMap::new()) } else { Bindings::Disconnected };
        let live = Arc::new(());
        // Everything below wakes the host through this, so its close ends them all.
        let (wake, closes_wake) = HostWake::new(wake);
        let engine = |generation| {
            let sink = make_sink(generation, wake.clone(), Arc::downgrade(&live));
            Engine { title, wayland, sink, connect, bindings }
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
        let handlers = shared.shortcuts.handlers.clone();
        // A host that attached after the close took routing keeps its handlers.
        let cleanup = Box::new(move |ended: u64| handlers.lock().unwrap().retain(|_, (stamp, _)| stamp.load(Ordering::Relaxed) > ended));
        let guard = self.route_core(cleanup);
        shared.shortcuts.register_pending();
        guard
    }

    fn route_core(self, cleanup: Box<dyn FnOnce(u64)>) -> Guard {
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
        Guard { control, generation, cleanup: Some(cleanup), _live: live, closes_wake, _thread: std::marker::PhantomData }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // First, so nothing the close sets off wakes this host.
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
mod tests;
