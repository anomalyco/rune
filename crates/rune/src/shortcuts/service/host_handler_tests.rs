//! A close clears the handlers of the hosts it ends, and leaves a later host's alone, through the
//! real `Guard` and real threads.

use super::*;
use crate::{App, AppSpec, el};

fn app(title: &'static str) -> App<u32> { App::new(AppSpec::new(title, (10.0, 10.0), || 0, |_: &u32, _| el())) }

fn chord(text: &str) -> Shortcut { text.parse().unwrap() }

/// Runs every queued update, and returns how far the handlers moved the count.
fn deliver(app: &App<u32>) -> u32 {
    let mut state = 0;
    for f in app.shared.take().updates { f(&mut state, &mut Tx::default()); }
    state
}

#[test]
fn a_closing_host_leaves_a_later_hosts_handlers_alone() {
    let app = app("Successor");
    let handle = app.handle();
    // A runs on its own thread; its close stops just before its cleanup.
    let (entered, in_cleanup) = mpsc::channel();
    let (resume, resumed) = mpsc::channel::<()>();
    let (close, closes) = mpsc::channel::<()>();
    let first = {
        let (shared, handle) = (app.shared.clone(), handle.clone());
        std::thread::spawn(move || {
            let mut guard = Guard::attach(&shared, Arc::new(|| {}), true, false);
            handle.register_shortcut(chord("ctrl+alt+g"), |state, _| *state += 100).unwrap();
            let cleanup = guard.cleanup.take().unwrap();
            guard.cleanup = Some(Box::new(move |ended| {
                entered.send(()).unwrap();
                resumed.recv().unwrap();
                cleanup(ended);
            }));
            closes.recv().unwrap();
            drop(guard);
        })
    };
    close.send(()).unwrap();
    in_cleanup.recv().unwrap();

    // B attaches after A's close took routing, and registers.
    let guard = Guard::attach(&app.shared, Arc::new(|| {}), true, false);
    handle.register_shortcut(chord("ctrl+alt+h"), |state, _| *state += 1).unwrap();
    resume.send(()).unwrap();
    first.join().unwrap();

    assert_eq!(app.shared.shortcuts.handlers.lock().unwrap().len(), 1, "only A's handler went");
    app.shared.shortcuts.fire(&chord("ctrl+alt+h")).unwrap();
    assert_eq!(deliver(&app), 1, "B's shortcut still reaches its handler");
    drop(guard);
    assert!(app.shared.shortcuts.handlers.lock().unwrap().is_empty());
}

#[test]
fn closing_a_later_host_ends_the_handlers_of_the_host_it_replaced() {
    let app = app("Replaced");
    let (finish, finishes) = mpsc::channel::<()>();
    let (queued, is_queued) = mpsc::channel();
    let first = {
        let (shared, handle) = (app.shared.clone(), app.handle());
        std::thread::spawn(move || {
            let guard = Guard::attach(&shared, Arc::new(|| {}), true, false);
            handle.register_shortcut(chord("ctrl+alt+r"), |state, _| *state += 1).unwrap();
            shared.shortcuts.fire(&chord("ctrl+alt+r")).unwrap();
            queued.send(()).unwrap();
            finishes.recv().unwrap();
            drop(guard);
        })
    };
    is_queued.recv().unwrap();
    // B replaces A without A closing, then closes: the service is closed, as on dev.
    drop(Guard::attach(&app.shared, Arc::new(|| {}), true, false));
    assert!(app.shared.shortcuts.handlers.lock().unwrap().is_empty());
    assert_eq!(deliver(&app), 0, "A's queued activation runs nothing");
    finish.send(()).unwrap();
    first.join().unwrap();
}
