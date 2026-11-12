//! The app quitting, as one signal whatever starts it: `Tx::quit` or `AppHandle::quit` from any
//! window, the last window closing, macOS Quit (?Q, the Dock, logout and shutdown, which AppKit
//! asks the app delegate about with `applicationShouldTerminate:`), and a Windows logoff or
//! shutdown (`WM_QUERYENDSESSION`, then `WM_ENDSESSION`). Every open window's `App::on_quit`
//! runs once when it starts, before any window closes for it, and [`quitting`] holds from then
//! on, so a window's `on_close` can tell the app ending from the user closing that one window.
//!
//! One machine ([`State`], [`step`]) holds where the quit is: running, quitting (the windows'
//! hooks owed, then run, and whether the app starts again after, `Tx::relaunch`), done. The hooks run in the event loop (`Windows`, which carries out
//! the effects). Windows sends its session messages to a window and waits for the answer, and
//! may end the process once every `WM_ENDSESSION` has returned, so the session window (`win32`)
//! wakes the loop and runs it until the hooks have run before it answers; when it comes while
//! a loop handler runs (a modal loop inside one), it can only end that loop and let the hooks
//! run once the handler returns.

use std::sync::Mutex;

#[cfg(target_os = "macos")]
mod mac;
pub(crate) mod relaunch;
#[cfg(target_os = "windows")]
mod win32;

/// Where the app's quit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// The app runs.
    Running,
    /// A quit started; the windows' hooks are owed or ran, and what follows the exit.
    Quitting(Hooks, After),
    /// The session ends now (a logoff): the windows closed as their close buttons would, and
    /// the loop is ending.
    Closed,
    /// The loop ended.
    Done,
}

/// The windows' `App::on_quit` hooks, for a quit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hooks {
    /// Not run yet.
    Owed,
    /// Every open window ran its hook.
    Ran,
}

/// What follows a quit's exit: nothing, or the app starting again (`Tx::relaunch`). A relaunch
/// asked during a quit outranks the plain exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum After {
    Exit,
    Relaunch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Something asks to quit: `Tx::quit` or `Tx::relaunch`, the last window closing, macOS
    /// terminating, a Windows session query (`ending: false`) or its end (`ending: true`).
    Start { ending: bool, after: After },
    /// The loop ran every window's hook.
    HooksRan,
    /// The loop closed the windows for the session's end (`Effect::CloseAndExit`).
    WindowsClosed,
    /// The system took the quit back (another app refused a logoff): the app runs on.
    Cancel,
    /// The event loop ended.
    Exited,
}

/// What the loop does for a step, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Every open window runs `App::on_quit`, then the loop sends `Event::HooksRan`.
    RunHooks,
    /// Every open window closes as its close button would, the loop sends
    /// `Event::WindowsClosed` and ends.
    CloseAndExit,
    /// The loop ended for a relaunch: start the app again (`crate::relaunch`).
    Relaunch,
}

pub(crate) fn step(state: State, event: Event) -> (State, Vec<Effect>) {
    use {Effect::*, Event::*, State::*};
    let close = |ending: bool| if ending { vec![CloseAndExit] } else { vec![] };
    match (state, event) {
        (Done, _) => (Done, vec![]),
        (Quitting(_, After::Relaunch), Exited) => (Done, vec![Relaunch]),
        (_, Exited) => (Done, vec![]),
        // A session ending closed the windows: the system ends the app, and nothing restarts it.
        (Closed, _) => (Closed, vec![]),
        (Running, Start { ending, after }) => (Quitting(Hooks::Owed, after), [vec![RunHooks], close(ending)].concat()),
        (Quitting(Hooks::Owed, was), Start { ending, after }) => (Quitting(Hooks::Owed, was.max(after)), [vec![RunHooks], close(ending)].concat()),
        (Quitting(Hooks::Ran, was), Start { ending, after }) => (Quitting(Hooks::Ran, was.max(after)), close(ending)),
        (Quitting(_, after), HooksRan) => (Quitting(Hooks::Ran, after), vec![]),
        (Quitting(..), WindowsClosed) => (Closed, vec![]),
        (Running | Quitting(..), Cancel) => (Running, vec![]),
        (Running, HooksRan | WindowsClosed) => (Running, vec![]),
    }
}

static STATE: Mutex<State> = Mutex::new(State::Running);
static FAST_EXIT: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// How a quitting app ends: fast, unless it asked for an orderly exit (`orderly_exit`).
static ORDERLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What a fast exit runs last: once the app has quit, every window has hidden (as the quit
/// started) and run its `App::on_quit`, the clipboard has handed over what it holds, tray icons
/// have left the notification area and a relaunch has started, `last` runs (the app's final
/// durable write, a trace) and the process ends at once. Windows, GPU devices, threads and memory
/// aren't dropped: the OS takes them back, which on Windows skips tens to hundreds of milliseconds
/// of destructors and graphics-driver `DllMain` detach, and a window is gone the moment its quit
/// is done. Every app ends this way unless it calls [`orderly_exit`]; `fast_exit` only adds
/// `last`. Nothing after the event loop returns runs, so whatever the app does there belongs in
/// `last` or its quit hooks. Set it once, before the app runs; a later call does nothing. On the
/// web the page's own lifetime applies instead.
pub fn fast_exit(last: fn()) {
    let _ = FAST_EXIT.set(last);
}

/// Tears the process down in order when the app quits instead of ending it at once (the default,
/// [`fast_exit`]): windows, GPU devices and threads are dropped and `run` returns, for an app with
/// work after `run` or destructors that must run. Call it before the app runs.
pub fn orderly_exit() {
    ORDERLY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The app's last step before a fast exit: its `fast_exit`, or nothing; none for an orderly exit.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn fast_exit_hook() -> Option<fn()> {
    if ORDERLY.load(std::sync::atomic::Ordering::Relaxed) { return None; }
    Some(FAST_EXIT.get().copied().unwrap_or(|| {}))
}
/// A fast exit, once the windows' hooks ran and the loop is ending: what must leave the system
/// tidy goes in order (the clipboard's hand-over, tray icons, a relaunch), then the app's
/// `last`, then the process ends without dropping a window, a device or an allocation.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn end_fast(last: fn()) -> ! {
    // The windows go now, not when the process's threads are torn down.
    #[cfg(target_os = "windows")]
    crate::platform::win32::destroy_own_windows();
    // What the loop's end does for an orderly exit (`event_loop::run_configured`).
    crate::tooling::interaction::flush();
    crate::platform::ready_signal::clear();
    crate::clipboard::shutdown();
    crate::tray::remove_all();
    if send(Event::Exited).contains(&Effect::Relaunch) {
        relaunch::spawn();
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    crate::open_url::instance::exited();
    crate::marks::mark("quit.last");
    last();
    end_process()
}

/// Ends this process now with exit code 0: no destructors, no `atexit`, no DLL detach.
#[cfg(not(target_arch = "wasm32"))]
fn end_process() -> ! {
    crate::marks::mark("quit.end");
    // A process ends once each of its threads has run to its end: an idle-priority one (D3D12's
    // shader threads) on a busy machine can hold it for seconds.
    #[cfg(target_os = "windows")]
    crate::platform::win32::raise_threads(|_| true, windows_sys::Win32::System::Threading::THREAD_PRIORITY_NORMAL);
    #[cfg(target_os = "windows")]
    // SAFETY: the pseudo-handle names this process; TerminateProcess on it doesn't return.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), 0);
    }
    #[cfg(unix)]
    // SAFETY: `_exit` ends the process without running anything of this process's.
    unsafe { libc::_exit(0) }
    #[allow(unreachable_code)]
    std::process::exit(0)
}

/// Steps the app's quit, returning what the loop must do.
pub(crate) fn send(event: Event) -> Vec<Effect> {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let (next, effects) = step(*state, event);
    if *state == State::Running && matches!(next, State::Quitting(..)) {
        crate::marks::mark("quit.start");
        #[cfg(not(target_arch = "wasm32"))]
        watch_quit();
    }
    *state = next;
    effects
}

/// A quit still running this long after it started says where it is on stderr, and again as long
/// as it runs: the last step it reached and, on Windows, the threads still alive.
#[cfg(not(target_arch = "wasm32"))]
const QUIT_REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

#[cfg(not(target_arch = "wasm32"))]
fn watch_quit() {
    let started = std::time::Instant::now();
    let _ = std::thread::Builder::new().name("rune-quit-watch".into()).spawn(move || loop {
        std::thread::sleep(QUIT_REPORT_EVERY);
        eprintln!("rune: still quitting {} ms after the quit started: {}", started.elapsed().as_millis(), where_the_quit_is());
    });
}

/// Ends the process `after` from now if it's still alive then, saying on stderr what it was
/// waiting for (the last step it reached, the threads still alive) and marking `quit.deadline`.
/// For an app to call once nothing of its own is left to lose, after its storage has closed, so
/// that a quit always ends.
#[cfg(not(target_arch = "wasm32"))]
pub fn exit_deadline(after: std::time::Duration) {
    let _ = std::thread::Builder::new().name("rune-exit-deadline".into()).spawn(move || {
        std::thread::sleep(after);
        eprintln!("rune: the quit didn't end within {} ms of the app's last step, so the process ends itself: {}", after.as_millis(), where_the_quit_is());
        crate::marks::mark("quit.deadline");
        end_process();
    });
}

/// The quit's last step reached and how long ago, and the process's threads.
#[cfg(not(target_arch = "wasm32"))]
fn where_the_quit_is() -> String {
    let last = crate::marks::marks().into_iter().last().map_or_else(|| "no step".to_owned(), |(name, at)| format!("last step {name}, {} ms ago", at.elapsed().as_millis()));
    #[cfg(target_os = "windows")]
    return format!("{last}; threads: {}", crate::platform::win32::threads_summary());
    #[cfg(not(target_os = "windows"))]
    last
}

pub(crate) fn state() -> State { *STATE.lock().unwrap_or_else(|e| e.into_inner()) }

/// Whether the app is quitting: a quit started and the system didn't cancel it. A window's
/// `on_close` that runs while this holds closes because the app ends, so it keeps what it would
/// forget when the user closes it alone (an app reopening its windows next launch keeps that
/// window's place in the list). False while the app runs, including when its last window
/// closes, whose `on_close` runs before the quit starts.
pub fn quitting() -> bool {
    !matches!(state(), State::Running)
}

/// Hooks the system's quit requests into the loop: `send` delivers a quit (`true` when the
/// session ends now and the windows must close) to `Windows`. Once per process, on the UI thread.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn install(send: impl Fn(bool) -> bool + 'static) {
    #[cfg(target_os = "windows")]
    win32::install(send);
    #[cfg(target_os = "macos")]
    { let _ = send; mac::install(); }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = send;
}

/// Whether the event loop is inside one of its handlers, on the UI thread: a session message
/// that arrives then (from a modal loop the handler runs) can't run the windows' hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Loop {
    Idle,
    Handling,
}

thread_local! {
    static LOOP: std::cell::Cell<Loop> = const { std::cell::Cell::new(Loop::Idle) };
}

/// Marks the loop handling an event until the guard drops.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn handling() -> Handling { Handling(LOOP.replace(Loop::Handling)) }

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn loop_state() -> Loop { LOOP.get() }

pub(crate) struct Handling(Loop);

impl Drop for Handling {
    fn drop(&mut self) { LOOP.set(self.0); }
}
