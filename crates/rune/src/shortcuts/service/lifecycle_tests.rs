//! A host attaching, closing and attaching again, through the real `Guard` and real threads.

use super::*;
use crate::{App, AppSpec, el};

fn app(title: &'static str) -> App<()> { App::new(AppSpec::new(title, (10.0, 10.0), || (), |_, _| el())) }

fn chord(text: &str) -> Shortcut { text.parse().unwrap() }

#[test]
fn a_closed_service_refuses_and_a_new_host_starts_empty() {
    let app = app("Reattach");
    let handle = app.handle();
    let chord = chord("ctrl+alt+r");
    assert_eq!(handle.register_shortcut(chord.clone(), |_, _| {}), Err(ShortcutError::NotRunning), "before a host");
    assert_eq!(app.shared.shortcuts.fire(&chord), Err(ShortcutError::NotRunning));

    let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
    let id = handle.register_shortcut(chord.clone(), |_, _| {}).unwrap();
    assert!(app.shared.shortcuts.handlers.lock().unwrap().contains_key(&id));
    drop(guard);

    assert!(app.shared.shortcuts.handlers.lock().unwrap().is_empty(), "closing clears the handlers");
    assert_eq!(handle.register_shortcut(chord.clone(), |_, _| {}), Err(ShortcutError::NotRunning), "after the host");
    assert_eq!(handle.unregister_shortcut(id), Err(ShortcutError::NotRunning));
    assert_eq!(handle.shortcut_owner(&chord), None);
    assert_eq!(app.shared.shortcuts.fire(&chord), Err(ShortcutError::NotRunning));
    assert!(app.shared.shortcuts.handlers.lock().unwrap().is_empty(), "a refused registration leaves no handler");

    let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
    assert_eq!(handle.unregister_shortcut(id), Err(ShortcutError::UnknownId), "registrations die with their host");
    assert_eq!(app.shared.shortcuts.fire(&chord), Err(ShortcutError::UnknownId));
    let again = handle.register_shortcut(chord.clone(), |_, _| {}).unwrap();
    assert_ne!(again, id);
    assert_eq!(handle.shortcut_owner(&chord).as_deref(), Some("Reattach"));
    drop(guard);
}

#[test]
fn static_shortcuts_wait_for_the_next_attach_whatever_the_state() {
    let app = app("Pending").global_shortcut("ctrl+alt+p", |_, _| {});
    let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
    app.shared.shortcuts.fire(&chord("ctrl+alt+p")).unwrap();

    let app = app.global_shortcut("ctrl+alt+w", |_, _| {});
    assert_eq!(app.shared.shortcuts.fire(&chord("ctrl+alt+w")), Err(ShortcutError::UnknownId), "queued while attached");
    drop(guard);

    let app = app.global_shortcut("ctrl+alt+q", |_, _| {}).global_shortcut("q", |_, _| {});
    let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
    app.shared.shortcuts.fire(&chord("ctrl+alt+w")).unwrap();
    app.shared.shortcuts.fire(&chord("ctrl+alt+q")).unwrap();
    assert_eq!(app.shared.shortcuts.fire(&chord("ctrl+alt+p")), Err(ShortcutError::UnknownId), "consumed by the first host");
    assert!(matches!(app.handle().take_shortcut_errors().as_slice(), [ShortcutError::InvalidAccelerator(_)]));
    drop(guard);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn closing_releases_callers_waiting_for_the_owner() {
    let app = app("Waiting");
    let (wake, woke) = mpsc::channel();
    let guard = Guard::attach(&app.shared, Arc::new(move || { let _ = wake.send(()); }), true, false);
    let handle = app.handle();
    let caller = std::thread::spawn(move || handle.register_shortcut("ctrl+alt+c".parse().unwrap(), |_, _| {}));
    woke.recv().unwrap();
    drop(guard);
    assert_eq!(caller.join().unwrap(), Err(ShortcutError::NotRunning));
    assert!(app.shared.shortcuts.handlers.lock().unwrap().is_empty());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn a_second_host_keeps_waiting_callers_and_either_close_ends_the_service() {
    let app = app("Second");
    let (first_wake, first_woke) = mpsc::channel();
    let first = Guard::attach(&app.shared, Arc::new(move || { let _ = first_wake.send(()); }), true, false);
    let handle = app.handle();
    let chord = chord("ctrl+alt+s");
    let caller = { let (handle, chord) = (handle.clone(), chord.clone()); std::thread::spawn(move || handle.register_shortcut(chord, |_, _| {})) };
    first_woke.recv().unwrap();

    let second = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
    second.drain();
    caller.join().unwrap().unwrap();
    assert_eq!(handle.shortcut_owner(&chord).as_deref(), Some("Second"), "the waiting call ran on the newer engine");

    drop(first);
    assert_eq!(handle.register_shortcut(chord.clone(), |_, _| {}), Err(ShortcutError::NotRunning));
    second.drain();
    drop(second);
    assert_eq!(handle.shortcut_owner(&chord), None);
}

#[cfg(target_os = "linux")]
#[test]
fn off_thread_calls_run_without_waiting_for_the_owner() {
    let app = app("Direct");
    let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
    let handle = app.handle();
    let caller = std::thread::spawn(move || handle.register_shortcut("ctrl+alt+d".parse().unwrap(), |_, _| {}));
    caller.join().unwrap().unwrap();
    drop(guard);
}
