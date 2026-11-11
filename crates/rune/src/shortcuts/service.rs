use super::{Shortcut, ShortcutError, ShortcutId, Sink, NativeBackend};
#[cfg(test)]
use super::Backend;
use crate::{Tx, app::Shared, runtime::Waker};
use machine::{AttachEffect, AttachEvent, Attachment, BindEffect, BindEvent, Bindings, step_attachment, step_bindings};
use std::{collections::HashMap, rc::Rc, sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, AtomicU32, Ordering}}};
#[cfg(not(target_os = "linux"))]
use std::cell::RefCell;
#[cfg(not(target_os = "linux"))]
use std::sync::atomic::AtomicUsize;
#[cfg(any(test, not(target_os = "linux")))]
use std::sync::mpsc;

mod machine;

type Handler<S> = Arc<dyn Fn(&mut S, &mut Tx) + Send + Sync>;
type Pending<S> = (Result<Shortcut, ShortcutError>, Handler<S>);
#[cfg(not(target_os = "linux"))]
type Job = Box<dyn FnOnce(&mut Engine) + Send>;
#[cfg(target_os = "linux")]
type Job = std::convert::Infallible;
type Connect = fn(bool, Sink) -> Result<NativeBackend, ShortcutError>;
#[cfg(not(target_os = "linux"))]
static NEXT_OWNER: AtomicUsize = AtomicUsize::new(1);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// Every App's native registrations in this process, with the App's title, so a conflict
/// between two Apps can name its owner. Headless registrations stay out of it.
static NATIVE: Mutex<Vec<(ShortcutId, Shortcut, &'static str)>> = Mutex::new(Vec::new());

struct Control {
    #[cfg(not(target_os = "linux"))]
    key: usize,
    attachment: Mutex<Attachment<Job>>,
    #[cfg(target_os = "linux")]
    engine: Mutex<Option<Arc<Mutex<Engine>>>>,
}

pub(crate) struct Hub<S> {
    control: Arc<Control>,
    pending: Mutex<Vec<Pending<S>>>,
    handlers: Arc<Mutex<HashMap<ShortcutId, Handler<S>>>>,
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
            }),
            pending: Mutex::default(), handlers: Arc::default(), errors: Mutex::default(), title: OnceLock::new(), unmodified: AtomicBool::new(false),
        }
    }
}

#[cfg(not(target_os = "linux"))]
thread_local! {
    // Native handles never cross threads, even though AppHandle can.
    static ENGINES: RefCell<HashMap<usize, Rc<RefCell<Engine>>>> = RefCell::default();
}

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
    fn invoke<T: Send + 'static>(&self, f: impl FnOnce(&mut Engine) -> Result<T, ShortcutError> + Send + 'static) -> Result<T, ShortcutError> {
        let mut route = self.attachment.lock().unwrap();
        let call = AttachEvent::Call { from: std::thread::current().id(), shared: cfg!(target_os = "linux") };
        let effect = step_attachment(&mut route, call).into_iter().next();
        #[cfg(target_os = "linux")]
        {
            let Some(AttachEffect::Run { wake }) = effect else { return Err(ShortcutError::NotRunning) };
            let shared = self.engine.lock().unwrap().clone().ok_or(ShortcutError::NotRunning)?;
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
                let engine = ENGINES.with(|engines| engines.borrow().get(&self.key).cloned()).ok_or(ShortcutError::NotRunning)?;
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
        self.handlers.lock().unwrap().insert(id, handler);
        if let Err(e) = self.control.invoke(move |engine| engine.register(id, shortcut)) {
            self.handlers.lock().unwrap().remove(&id);
            return Err(e);
        }
        Ok(id)
    }

    pub(super) fn unregister(&self, id: ShortcutId) -> Result<(), ShortcutError> {
        let handlers = self.handlers.clone();
        self.control.invoke(move |engine| {
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
        self.control.invoke(move |engine| Ok(engine.owner(&shortcut))).ok().flatten()
    }

    pub(crate) fn fire(&self, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        let shortcut = shortcut.clone();
        self.control.invoke(move |engine| {
            let id = engine.bindings.find(&shortcut).ok_or(ShortcutError::UnknownId)?;
            (engine.sink)(id, None);
            Ok(())
        })
    }
}

/// Owned by the host, not AppHandle: closing/dropping the app releases native registrations.
pub(crate) struct Guard {
    control: Arc<Control>,
    cleanup: Option<Box<dyn FnOnce()>>,
    // The guard must be destroyed on the owning event-loop thread.
    _thread: std::marker::PhantomData<Rc<()>>,
}

impl Guard {
    pub(crate) fn attach<S: 'static>(shared: &Arc<Shared<S>>, wake: Waker, headless: bool, wayland: bool) -> Self {
        Self::attach_with(shared, wake, headless, wayland, super::backend)
    }

    fn attach_with<S: 'static>(shared: &Arc<Shared<S>>, wake: Waker, headless: bool, wayland: bool, connect: Connect) -> Self {
        let weak = Arc::downgrade(shared);
        let sink: Sink = Arc::new(move |id, token| {
            if let Some(shared) = weak.upgrade() {
                let weak = Arc::downgrade(&shared);
                crate::AppHandle { shared }.update(move |state, tx| {
                    let handler = weak.upgrade().and_then(|s| s.shortcuts.handlers.lock().unwrap().get(&id).cloned());
                    if let Some(handler) = handler {
                        tx.shortcut_activation_token = token;
                        handler(state, tx);
                        tx.shortcut_activation_token = None;
                    }
                });
            }
        });
        let control = shared.shortcuts.control.clone();
        let title = shared.shortcuts.title.get().copied().unwrap_or_default();
        let bindings = if headless { Bindings::Virtual(HashMap::new()) } else { Bindings::Disconnected };
        let engine = Engine { title, wayland, sink, connect, bindings };
        #[cfg(not(target_os = "linux"))]
        ENGINES.with(|engines| engines.borrow_mut().insert(control.key, Rc::new(RefCell::new(engine))));
        // A replaced engine closes with the slot locked, so callers wait until its chords are free.
        #[cfg(target_os = "linux")]
        { *control.engine.lock().unwrap() = Some(Arc::new(Mutex::new(engine))); }
        step_attachment(&mut control.attachment.lock().unwrap(), AttachEvent::Attach { owner: std::thread::current().id(), wake });
        let handlers = shared.shortcuts.handlers.clone();
        let guard = Self { control, cleanup: Some(Box::new(move || handlers.lock().unwrap().clear())), _thread: std::marker::PhantomData };
        for (shortcut, handler) in std::mem::take(&mut *shared.shortcuts.pending.lock().unwrap()) {
            if let Err(error) = shortcut.and_then(|s| shared.shortcuts.register(s, handler)) {
                eprintln!("rune: {error}");
                shared.shortcuts.errors.lock().unwrap().push(error);
            }
        }
        guard
    }

    pub(crate) fn drain(&self) {
        #[cfg(not(target_os = "linux"))]
        {
            let drained = step_attachment(&mut self.control.attachment.lock().unwrap(), AttachEvent::Drain);
            let Some(AttachEffect::Jobs(jobs)) = drained.into_iter().next() else { return };
            let engine = ENGINES.with(|engines| engines.borrow().get(&self.control.key).cloned());
            if let Some(engine) = engine { for job in jobs { job(&mut engine.borrow_mut()); } }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let closed = step_attachment(&mut self.control.attachment.lock().unwrap(), AttachEvent::Close);
        // Dropping queued reply senders releases waiting callers with NotRunning.
        for effect in closed { if let AttachEffect::Release(jobs) = effect { drop(jobs); } }
        #[cfg(not(target_os = "linux"))]
        ENGINES.with(|engines| engines.borrow_mut().remove(&self.control.key));
        // The slot stays locked through the release, so a new host can't start while this one's
        // backend still holds its chords.
        #[cfg(target_os = "linux")]
        if let Some(engine) = self.control.engine.lock().unwrap().take() {
            engine.lock().unwrap().release();
        }
        if let Some(cleanup) = self.cleanup.take() { cleanup(); }
    }
}

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod machine_tests;
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
        for f in app.shared.take().updates { f(&mut state, &mut Tx::default()); }
        assert_eq!(state, 1);
        assert_eq!(handle.register_shortcut(shortcut, |_, _| {}), Err(ShortcutError::Conflict));
        let id = handle.register_shortcut("ctrl+a".parse().unwrap(), |s, _| *s += 10).unwrap();
        app.shared.shortcuts.fire(&"ctrl+a".parse().unwrap()).unwrap();
        handle.unregister_shortcut(id).unwrap();
        for f in app.shared.take().updates { f(&mut state, &mut Tx::default()); }
        assert_eq!(state, 1);
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
        app.shared.shortcuts.control.invoke(move |engine| {
            (engine.sink)(id, Some("compositor-token".into()));
            Ok(())
        }).unwrap();
        let mut state = false;
        let mut tx = Tx::default();
        for f in app.shared.take().updates { f(&mut state, &mut tx); }
        assert!(state);
        assert_eq!(tx.activation_token(), None);
        assert_eq!(app.shared.take().window, Some(crate::app::WindowRequest::Open(Some("compositor-token".into()))));
        drop(app);
        drop(guard);
        assert!(weak.upgrade().is_none(), "handler captures must be released at host shutdown");
    }
}
