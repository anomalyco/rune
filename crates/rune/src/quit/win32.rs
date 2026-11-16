//! A hidden top-level window that hears a Windows logoff or shutdown. Windows asks every
//! top-level window whether the session may end (`WM_QUERYENDSESSION`), then tells each
//! whether it does (`WM_ENDSESSION`); a message-only window hears neither, and the app's own
//! windows come and go. Restart Manager's requests to close the app (`ENDSESSION_CLOSEAPP`, an
//! installer or an update) arrive the same way.
//!
//! Neither message waits in the queue: Windows sends it and waits for the answer, and once every
//! window answered `WM_ENDSESSION` it may end the process at any moment. So the window starts
//! the quit (`quitting` holds from then on), wakes the loop and runs it here, as a modal loop
//! does, until `Windows` has run the hooks, for at most `WAIT`.
//!
//! A message can also come while one of the loop's handlers runs, from a modal loop inside it
//! or a synchronous send. winit then holds the wake until the handler returns, and a paint run
//! from here would re-enter it, so the window runs no loop: it ends a tracking menu (the usual
//! modal loop) and answers, and the hooks run as the handler returns, before `WM_ENDSESSION`.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, TRUE, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EndMenu, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW,
    PostQuitMessage, QS_ALLINPUT, RegisterClassW, TranslateMessage, WM_ENDSESSION, WM_QUERYENDSESSION, WM_QUIT, WNDCLASSW,
    WS_EX_TOOLWINDOW,
};

use super::{Event, Hooks, Loop, State};

/// How long the window runs the loop for the hooks before it answers anyway: Windows shows
/// that the app is preventing shutdown after five seconds.
const WAIT: Duration = Duration::from_secs(3);

type Deliver = Box<dyn Fn(bool) -> bool>;

thread_local! {
    /// Sends a quit to the loop (`true`: the session ends now); false once the loop is gone.
    static DELIVER: RefCell<Option<Deliver>> = const { RefCell::new(None) };
    static WINDOW: std::cell::Cell<HWND> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

pub(super) fn install(deliver: impl Fn(bool) -> bool + 'static) {
    DELIVER.with(|d| *d.borrow_mut() = Some(Box::new(deliver)));
    if !WINDOW.get().is_null() {
        return;
    }
    let class: Vec<u16> = "RuneSession\0".encode_utf16().collect();
    // SAFETY: the class name is NUL-terminated and outlives both calls; the window lives as
    // long as the process, on this (the UI) thread, and keeps no pointer to Rust state.
    let window = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        // Never shown, and a tool window, so nothing lists it: not the taskbar, Alt+Tab or a
        // relaunch looking for the app's front window.
        CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null())
    };
    if window.is_null() {
        eprintln!("rune: could not create the session window; a logoff won't run App::on_quit");
    }
    WINDOW.set(window);
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => {
            #[cfg(feature="deferred-quit")]
            if let Some(hooks) = super::deferred::hooks() {
                use super::deferred::session::{self,Query};
                if session::current()==Query::Allowed || super::deferred::exiting() { return TRUE as LRESULT; }
                use windows_sys::Win32::System::Shutdown::ShutdownBlockReasonCreate;
                let reason: Vec<u16>=(hooks.shutdown_reason)().encode_utf16().take(255).chain(Some(0)).collect();
                // SAFETY: the reason is NUL terminated; this is the session window's own thread.
                unsafe { ShutdownBlockReasonCreate(hwnd,reason.as_ptr()); }
                session::ask();
                // As for the existing session hook, do not dispatch a nested event while a Rune
                // handler is running. A query is permission, not the final end of the session.
                if super::loop_state() == Loop::Idle { run_query_loop(); } else {
                    // A menu owns the nested dispatch; don't borrow the handler
                    // twice. Unwind it and let the save complete after our veto.
                    unsafe { EndMenu(); }
                }
                return session::answered() as LRESULT;
            }
            answer(false);
            TRUE as LRESULT
        }
        WM_ENDSESSION => {
            #[cfg(feature="deferred-quit")]
            if super::deferred::hooks().is_some() {
                if w != 0 {
                    if super::deferred::exiting(){return 0}
                    if super::deferred::session::current()!=super::deferred::session::Query::Allowed { super::deferred::commit_exit();std::process::exit(2); }
                    // Windows may end the process as soon as this returns. Finish here, as the
                    // loop's end would: the app's last step closes storage with a final barrier
                    // (covering any write admitted since the query) and marks the journal clean.
                    // A consented session end never authorizes the updater (`Hooks::confirmed`
                    // is not called), so the app's last step only saves.
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(last) = super::fast_exit_hook() {
                        super::deferred::commit_exit();
                        crate::marks::mark("quit.session_end");
                        super::end_fast(last);
                    }
                    super::deferred::session_ends();
                    run_until_exit();
                } else { clear_reason(); super::deferred::session::cancel(); }
                return 0;
            }
            if w != 0 {
                answer(true);
            } else {
                // Another app refused, or the user cancelled: the app runs on.
                super::send(Event::Cancel);
            }
            0
        }
        // SAFETY: the parameters are the ones Windows passed.
        _ => unsafe { DefWindowProcW(hwnd, message, w, l) },
    }
}

#[cfg(feature="deferred-quit")]
fn run_query_loop() {
    let until=Instant::now()+Duration::from_secs(2);
    while super::deferred::session::current()==super::deferred::session::Query::Asking && Instant::now()<until {
        // SAFETY: same UI thread and message pump as run_loop. Preserve WM_QUIT for winit.
        unsafe {
            let mut msg:MSG=std::mem::zeroed();
            if PeekMessageW(&mut msg,std::ptr::null_mut(),0,0,PM_REMOVE)==0 {
                MsgWaitForMultipleObjects(0,std::ptr::null(),0,until.saturating_duration_since(Instant::now()).as_millis() as u32,QS_ALLINPUT);continue;
            }
            if msg.message==WM_QUIT {PostQuitMessage(msg.wParam as i32);return}
            TranslateMessage(&msg);DispatchMessageW(&msg);
        }
    }
}

/// An app with an orderly exit: run the posted session end for at most `WAIT` before answering.
#[cfg(feature="deferred-quit")]
fn run_until_exit() {
    let until=Instant::now()+WAIT;
    while Instant::now()<until {
        // SAFETY: same UI thread and message pump as run_loop. Preserve WM_QUIT for winit.
        unsafe {
            let mut msg:MSG=std::mem::zeroed();
            if PeekMessageW(&mut msg,std::ptr::null_mut(),0,0,PM_REMOVE)==0 {
                MsgWaitForMultipleObjects(0,std::ptr::null(),0,until.saturating_duration_since(Instant::now()).as_millis() as u32,QS_ALLINPUT);continue;
            }
            if msg.message==WM_QUIT {PostQuitMessage(msg.wParam as i32);return}
            TranslateMessage(&msg);DispatchMessageW(&msg);
        }
    }
}

#[cfg(feature="deferred-quit")]
pub(super) fn clear_reason() {
    let hwnd=WINDOW.get();
    if !hwnd.is_null() {
        // SAFETY: called on the UI thread which created the hidden session window.
        unsafe { windows_sys::Win32::System::Shutdown::ShutdownBlockReasonDestroy(hwnd); }
    }
}

/// Starts the quit and has the loop run the hooks, before the answer when the loop can run.
fn answer(ending: bool) {
    // The loop carries out the effects when the quit reaches it.
    let _ = super::send(Event::Start { ending, after: super::After::Exit });
    let sent = DELIVER.with(|d| d.try_borrow().ok().and_then(|d| d.as_ref().map(|d| d(ending)))).unwrap_or(false);
    match super::loop_state() {
        Loop::Handling => {
            // SAFETY: ends whatever menu this thread tracks; nothing when none does.
            unsafe { EndMenu() };
            if ending && !answered(ending) {
                eprintln!("rune: the session ended inside a loop handler, before the windows' App::on_quit ran");
            }
        }
        Loop::Idle if sent => run_loop(ending),
        Loop::Idle => {}
    }
}

/// The loop did what the message asks: ran the hooks, and when the session ends now, closed the
/// windows too. A cancelled or finished quit asks nothing more.
fn answered(ending: bool) -> bool {
    match super::state() {
        State::Quitting(Hooks::Ran, _) => !ending,
        State::Quitting(Hooks::Owed, _) => false,
        State::Running | State::Closed | State::Done => true,
    }
}

/// Runs the loop's messages until `Windows` answered the message, for at most `WAIT`.
fn run_loop(ending: bool) {
    let ran = std::panic::catch_unwind(|| {
        let deadline = Instant::now() + WAIT;
        while !answered(ending) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                eprintln!("rune: the session ends before the windows' App::on_quit ran");
                break;
            }
            // SAFETY: `msg` is written by PeekMessageW before it is read; dispatching on the
            // thread that owns the windows is what its own loop does.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                if PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) == 0 {
                    MsgWaitForMultipleObjects(0, std::ptr::null(), 0, left.as_millis() as u32, QS_ALLINPUT);
                    continue;
                }
                if msg.message == WM_QUIT {
                    PostQuitMessage(msg.wParam as i32);
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
    if ran.is_err() {
        eprintln!("rune: a quit hook panicked during a logoff");
    }
}
