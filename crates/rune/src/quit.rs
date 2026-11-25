//! The app quitting, as one signal whatever starts it: `Tx::quit` or `AppHandle::quit` from any
//! window, the last window closing, macOS Quit (?Q, the Dock, logout and shutdown, which AppKit
//! asks the app delegate about with `applicationShouldTerminate:`), and a Windows logoff or
//! shutdown (`WM_QUERYENDSESSION`, then `WM_ENDSESSION`). Every open window's `App::on_quit`
//! (and that of the last main window whose close ended the app) runs once when it starts, before
//! any remaining window closes for it, and [`quitting`] holds from then
//! on, so a window's `on_close` can tell the app ending from the user closing that one window.
//!
//! One machine ([`State`], [`step`]) holds where the quit is: running, quitting (the windows'
//! hooks owed, then run, and whether the app starts again after, `Tx::relaunch`), done. The hooks run in the event loop (`Windows`, which carries out
//! the effects). Windows sends its session messages to a window and waits for the answer, and
//! may end the process once every `WM_ENDSESSION` has returned, so the session window (`win32`)
//! wakes the loop and runs it until the hooks have run before it answers; when it comes while
//! a loop handler runs (a modal loop inside one), it can only end that loop and let the hooks
//! run once the handler returns.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod relaunch;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod request;
#[cfg(target_os = "windows")]
mod win32;

/// Where the app's quit is.
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(target_os = "windows")]
    Cancel,
    /// The event loop ended.
    Exited,
}

/// What the loop does for a step, in order.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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
        #[cfg(target_os = "windows")]
        (Running | Quitting(..), Cancel) => (Running, vec![]),
        (Running, HooksRan | WindowsClosed) => (Running, vec![]),
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
/// is done. Every normal quit ends this way unless the app calls [`orderly_exit`]; `fast_exit`
/// only adds `last`. Startup window creation failures and lost display connections bypass fast
/// exit so `try_run` returns the error and `run` prints it to stderr and exits 1. Nothing after
/// a normal event loop exit runs, so whatever the app does there belongs in `last` or its quit
/// hooks. Set it once, before the app runs; a later call does nothing. On the web the page's own
/// lifetime applies instead.
pub fn fast_exit(last: fn()) {
    FAST_EXIT.set(last).ok();
}

/// Tears the process down in order when the app quits instead of ending it at once (the default,
/// [`fast_exit`]): windows, GPU devices and threads are dropped and `run` returns, for an app with
/// work after `run` or destructors that must run. Call it before the app runs.
///
/// Windows, overlays and tray icons are hidden and removed before exit waits for any in-flight
/// background GPU warm-up. Warm-up is cancelled as soon as the event loop begins exiting, and
/// exit waits up to 10 seconds for those warm-up threads to finish so slow driver or shader
/// startup under load drains cleanly and `run` returns. As a last resort against a driver call
/// that is truly hung, if a warm-up thread has still not exited after 10 seconds, Rune logs to
/// stderr and ends the process immediately (`_exit` on Unix, `TerminateProcess` on Windows)
/// without running destructors under the stuck thread or returning from `run`.
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

/// Ends this process now with exit code 0 (3 when `RUNE_LOOP_CHECK=2` found a differing picture,
/// `render::pixel_check`): no destructors, no `atexit`, no DLL detach.
#[cfg(not(target_arch = "wasm32"))]
fn end_process() -> ! {
    let code = if crate::render::loop_pixel_check_failed() { 3 } else { 0 };
    end_process_with_code(code)
}

/// Ends this process immediately with `code`: no destructors, no `atexit`, no DLL detach.
/// Used by fast exit, `exit_deadline`, and orderly/error exits whose GPU warm-up threads stayed
/// wedged in a driver call past the bounded shutdown wait.
#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
pub(crate) fn end_process_with_code(code: i32) -> ! {
    crate::marks::mark("quit.end");
    // A process ends once each of its threads has run to its end: an idle-priority one (D3D12's
    // shader threads) on a busy machine can hold it for seconds.
    #[cfg(target_os = "windows")]
    crate::platform::win32::raise_threads(|_| true, windows_sys::Win32::System::Threading::THREAD_PRIORITY_NORMAL);
    #[cfg(target_os = "windows")]
    // SAFETY: the pseudo-handle names this process; TerminateProcess on it doesn't return.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), code as u32);
    }
    #[cfg(unix)]
    // SAFETY: `_exit` ends the process without running anything of this process's.
    unsafe { libc::_exit(code) }
    #[allow(unreachable_code)]
    std::process::exit(code)
}

/// Steps the app's quit, returning what the loop must do.
#[cfg(not(target_arch = "wasm32"))]
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
    let started = crate::time::now();
    std::thread::Builder::new().name("rune-quit-watch".into()).spawn(move || loop {
        std::thread::sleep(QUIT_REPORT_EVERY);
        eprintln!("rune: still quitting {} ms after the quit started: {}", started.elapsed().as_millis(), where_the_quit_is());
    }).ok();
}

/// Ends the process `after` from now if it's still alive then, saying on stderr what it was
/// waiting for (the last step it reached, the threads still alive) and marking `quit.deadline`.
/// For an app to call once nothing of its own is left to lose, after its storage has closed, so
/// that a quit always ends.
#[cfg(not(target_arch = "wasm32"))]
pub fn exit_deadline(after: std::time::Duration) {
    std::thread::Builder::new().name("rune-exit-deadline".into()).spawn(move || {
        std::thread::sleep(after);
        eprintln!("rune: the quit didn't end within {} ms of the app's last step, so the process ends itself: {}", after.as_millis(), where_the_quit_is());
        crate::marks::mark("quit.deadline");
        end_process();
    }).ok();
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn state() -> State { *STATE.lock().unwrap_or_else(|e| e.into_inner()) }

/// Whether the app is quitting: a quit started and the system didn't cancel it. A window's
/// `on_close` that runs while this holds closes because the app ends, so it keeps what it would
/// forget when the user closes it alone (an app reopening its windows next launch keeps that
/// window's place in the list). False while the app runs, including when its last window
/// closes, whose `on_close` runs before the quit starts.
pub fn quitting() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    { !matches!(state(), State::Running) }
    #[cfg(target_arch = "wasm32")]
    { false }
}

/// Hooks the system's quit requests into the loop: `send` delivers a quit (`true` when the
/// session ends now and the windows must close) to `Windows`, and `request` wakes the loop on
/// macOS when `applicationShouldTerminate:` defers to registered `App::on_quit_request` hooks.
/// Once per process, on the UI thread.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn install(
    send: impl Fn(bool) -> bool + 'static,
    request: impl Fn() -> bool + 'static,
) {
    #[cfg(target_os = "windows")]
    { let _ = request; win32::install(send); }
    #[cfg(target_os = "macos")]
    { let _ = send; mac::install(request); }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = (send, request);
}

/// Updates whether any live non-finished host currently has `App::on_quit_request` registered.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn set_has_quit_request_hooks(any: bool) {
    request::set_has_hooks(any);
    #[cfg(target_os = "macos")]
    if any {
        mac::warn_if_foreign_delegate();
    }
}

/// Whether an OS quit request is currently waiting for hosts' `on_quit_request` hooks to decide.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn quit_request_asking() -> bool {
    request::is_asking()
}

/// Records the hooks' combined decision (`true` = allow quit, `false` = defer) for an in-flight
/// macOS `NSTerminateLater` quit request and schedules the single common-mode reply.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn finish_quit_request(allow: bool) {
    #[cfg(target_os = "macos")]
    mac::finish_quit_request(allow);
    #[cfg(not(target_os = "macos"))]
    let _ = allow;
}

/// Notifies the quit-request machine that a programmatic quit/relaunch or last-window exit is
/// starting. Returns `true` when a pending macOS `NSTerminateLater` reply was upgraded to YES
/// and will end the app, so the caller should skip a redundant `event_loop.exit()`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn programmatic_quit() -> bool {
    #[cfg(target_os = "macos")]
    { mac::programmatic_quit() }
    #[cfg(not(target_os = "macos"))]
    { false }
}

/// Whether the event loop is inside one of its handlers, on the UI thread: a session message
/// that arrives then (from a modal loop the handler runs) can't run the windows' hooks, and on
/// macOS a queued `replyToApplicationShouldTerminate:` waits until the outermost handler returns.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Loop {
    Idle,
    Handling,
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static LOOP: std::cell::Cell<Loop> = const { std::cell::Cell::new(Loop::Idle) };
}

/// Marks the loop handling an event until the guard drops.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn handling() -> Handling { Handling(LOOP.replace(Loop::Handling)) }

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn loop_state() -> Loop { LOOP.get() }

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Handling(Loop);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Handling {
    fn drop(&mut self) {
        LOOP.set(self.0);
        #[cfg(target_os = "macos")]
        if self.0 == Loop::Idle && !std::thread::panicking() {
            mac::on_loop_idle();
        }
    }
}
