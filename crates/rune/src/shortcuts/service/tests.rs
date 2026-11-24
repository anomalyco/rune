use super::*;
use crate::{App, AppSpec, el};

fn connect_unused(_: bool, _: Sink) -> Result<NativeBackend, ShortcutError> {
    panic!("unit test must not open a native shortcut backend");
}

#[test]
fn mock_backend_errors_are_not_confused_with_conflicts() {
    struct Mock(ShortcutError);
    impl Backend for Mock {
        fn register(&mut self, _: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> { Err(self.0.clone()) }
        fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
    }
    for error in [ShortcutError::Conflict, ShortcutError::Unsupported, ShortcutError::PermissionDenied, ShortcutError::Backend("disconnected".into())] {
        let bindings = Bindings::Connected { native: Box::new(Mock(error.clone())) as NativeBackend, registrations: HashMap::new() };
        let mut engine = Engine { title: "mock", wayland: false, sink: Arc::new(|_, _| {}), connect: connect_unused, bindings };
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
    let native = |title| Engine { title, wayland: false, sink: Arc::new(|_, _| {}), connect: connect_unused, bindings: Bindings::Connected { native: Box::new(Accept) as NativeBackend, registrations: HashMap::new() } };
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
    for allowed in ["f5", "f24", "shift+f1", "printscreen", "shift+printscreen", "ctrl+printscreen", "alt+printscreen", "ctrl+space", "alt+a", "super+enter"] {
        assert!(handle.register_shortcut(allowed.parse().unwrap(), |_, _| {}).is_ok(), "{allowed}");
    }
    // PrintScreen aliases normalize to the same chord and conflict with the existing bare registration.
    assert_eq!(handle.register_shortcut("print".parse().unwrap(), |_, _| {}), Err(ShortcutError::Conflict));
    assert_eq!(handle.register_shortcut("prtsc".parse().unwrap(), |_, _| {}), Err(ShortcutError::Conflict));
    drop(guard);

    let app = App::new(AppSpec::new("capture", (10.0, 10.0), || 0u32, |_: &u32, _| el()))
        .global_shortcut("printscreen", |s, _| *s += 1)
        .global_shortcut("ctrl+print", |s, _| *s += 10)
        .global_shortcut("alt+prtsc", |s, _| *s += 100);
    let handle = app.handle();
    let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
    assert!(handle.take_shortcut_errors().is_empty());
    for chord in ["print", "ctrl+printscreen", "alt+printscreen"] {
        app.shared.shortcuts.fire(&chord.parse().unwrap()).unwrap();
    }
    let mut state = 0u32;
    let receiving = guard.receiving();
    for f in app.shared.take().updates { f(&mut state, &mut Tx::default()); }
    drop(receiving);
    assert_eq!(state, 111);
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

static BATCHES: Mutex<Vec<Vec<(String, Option<String>, Option<String>)>>> = Mutex::new(Vec::new());

struct BatchRecorder;

impl Backend for BatchRecorder {
    fn register(&mut self, _: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        BATCHES.lock().unwrap().push(vec![(shortcut.to_string(), None, None)]);
        Ok(())
    }
    fn register_with(&mut self, _: ShortcutId, shortcut: &Shortcut, details: &super::super::ShortcutDetails) -> Result<(), ShortcutError> {
        BATCHES.lock().unwrap().push(vec![(shortcut.to_string(), details.id.clone(), details.description.clone())]);
        Ok(())
    }
    fn register_batch(&mut self, items: &[(ShortcutId, Shortcut, super::super::ShortcutDetails)]) -> Vec<Result<(), ShortcutError>> {
        BATCHES.lock().unwrap().push(
            items.iter().map(|(_, s, d)| (s.to_string(), d.id.clone(), d.description.clone())).collect()
        );
        vec![Ok(()); items.len()]
    }
    fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
}

fn connect_batch(_: bool, _: Sink) -> Result<NativeBackend, ShortcutError> { Ok(Box::new(BatchRecorder)) }

#[test]
fn startup_global_shortcuts_register_in_one_batch() {
    BATCHES.lock().unwrap().clear();
    let app = App::new(AppSpec::new("Batch", (10.0, 10.0), || (), |_, _| el()))
        .global_shortcut("ctrl+alt+super+f13", |_, _| {})
        .global_shortcut_named("ctrl+alt+super+f14", "quick-panel", "Open quick panel", |_, _| {});
    let handle = app.handle();
    let guard = Guard::attach_with(&app.shared, Arc::new(|| {}), false, true, connect_batch);
    let runtime_id = handle
        .register_shortcut_named("ctrl+alt+super+f15".parse().unwrap(), "capture", "Capture screen", |_, _| {})
        .unwrap();
    let recorded = BATCHES.lock().unwrap().clone();
    handle.unregister_shortcut(runtime_id).unwrap();
    drop(guard);
    assert_eq!(recorded.len(), 2, "expected one startup batch plus one runtime call, got: {recorded:?}");
    assert_eq!(
        recorded[0],
        vec![
            ("ctrl+alt+super+f13".into(), None, None),
            ("ctrl+alt+super+f14".into(), Some("quick-panel".into()), Some("Open quick panel".into())),
        ]
    );
    assert_eq!(
        recorded[1],
        vec![("ctrl+alt+super+f15".into(), Some("capture".into()), Some("Capture screen".into()))]
    );
}

#[test]
fn startup_batch_records_partial_failures_and_preserves_error_order() {
    struct Partial;
    impl Backend for Partial {
        fn register(&mut self, _: ShortcutId, _: &Shortcut) -> Result<(), ShortcutError> { Ok(()) }
        fn register_batch(&mut self, items: &[(ShortcutId, Shortcut, super::super::ShortcutDetails)]) -> Vec<Result<(), ShortcutError>> {
            items.iter().map(|(_, s, _)| {
                if s.to_string() == "ctrl+alt+super+f17" { Err(ShortcutError::PermissionDenied) } else { Ok(()) }
            }).collect()
        }
        fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
    }
    fn connect_partial(_: bool, _: Sink) -> Result<NativeBackend, ShortcutError> { Ok(Box::new(Partial)) }

    let app = App::new(AppSpec::new("Partial", (10.0, 10.0), || 0u32, |_: &u32, _| el()))
        .global_shortcut("bad+", |s, _| *s += 1)
        .global_shortcut("ctrl+alt+super+f16", |s, _| *s += 10)
        .global_shortcut("ctrl+alt+super+f17", |s, _| *s += 100)
        .global_shortcut("ctrl+alt+super+f16", |s, _| *s += 1000);
    let handle = app.handle();
    let guard = Guard::attach_with(&app.shared, Arc::new(|| {}), false, true, connect_partial);
    let errors = handle.take_shortcut_errors();
    assert_eq!(errors.len(), 3, "expected InvalidAccelerator, PermissionDenied, Conflict in order: {errors:?}");
    assert!(matches!(errors[0], ShortcutError::InvalidAccelerator(_)));
    assert_eq!(errors[1], ShortcutError::PermissionDenied);
    assert_eq!(errors[2], ShortcutError::Conflict);
    // Only the accepted shortcut stays registered and owned.
    assert_eq!(handle.shortcut_owner(&"ctrl+alt+super+f16".parse().unwrap()).as_deref(), Some("Partial"));
    assert_eq!(handle.shortcut_owner(&"ctrl+alt+super+f17".parse().unwrap()), None);
    assert_eq!(app.shared.shortcuts.handlers.lock().unwrap().len(), 1);
    drop(guard);
}

static CONFLICT_SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Uses the default `Backend::register_batch` loop (as macOS, Windows and X11 do) and refuses one
/// chord with `ShortcutError::Conflict`, verifying the other chords still register and fire.
#[test]
fn startup_batch_keeps_registering_after_backend_conflict() {
    struct ConflictOnSecond;
    impl Backend for ConflictOnSecond {
        fn register(&mut self, _: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> {
            let name = shortcut.to_string();
            CONFLICT_SEEN.lock().unwrap().push(name.clone());
            if name == "ctrl+alt+super+f19" { Err(ShortcutError::Conflict) } else { Ok(()) }
        }
        fn unregister(&mut self, _: ShortcutId) -> Result<(), ShortcutError> { Ok(()) }
    }
    fn connect_conflict(_: bool, _: Sink) -> Result<NativeBackend, ShortcutError> { Ok(Box::new(ConflictOnSecond)) }

    CONFLICT_SEEN.lock().unwrap().clear();
    let app = App::new(AppSpec::new("ConflictBatch", (10.0, 10.0), || 0u32, |_: &u32, _| el()))
        .global_shortcut("ctrl+alt+super+f18", |s, _| *s += 1)
        .global_shortcut("ctrl+alt+super+f19", |s, _| *s += 10)
        .global_shortcut("ctrl+alt+super+f20", |s, _| *s += 100);
    let handle = app.handle();
    let guard = Guard::attach_with(&app.shared, Arc::new(|| {}), false, false, connect_conflict);
    assert_eq!(
        *CONFLICT_SEEN.lock().unwrap(),
        vec!["ctrl+alt+super+f18", "ctrl+alt+super+f19", "ctrl+alt+super+f20"],
        "a backend Conflict on one chord must not abort the rest of the startup batch"
    );
    assert_eq!(handle.take_shortcut_errors(), vec![ShortcutError::Conflict]);
    assert_eq!(handle.shortcut_owner(&"ctrl+alt+super+f18".parse().unwrap()).as_deref(), Some("ConflictBatch"));
    assert_eq!(handle.shortcut_owner(&"ctrl+alt+super+f19".parse().unwrap()), None);
    assert_eq!(handle.shortcut_owner(&"ctrl+alt+super+f20".parse().unwrap()).as_deref(), Some("ConflictBatch"));

    app.shared.shortcuts.fire(&"ctrl+alt+super+f18".parse().unwrap()).unwrap();
    assert_eq!(app.shared.shortcuts.fire(&"ctrl+alt+super+f19".parse().unwrap()), Err(ShortcutError::UnknownId));
    app.shared.shortcuts.fire(&"ctrl+alt+super+f20".parse().unwrap()).unwrap();
    let mut state = 0;
    let receiving = guard.receiving();
    for f in app.shared.take().updates { f(&mut state, &mut Tx::default()); }
    drop(receiving);
    assert_eq!(state, 101);
    drop(guard);
}
