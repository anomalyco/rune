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
