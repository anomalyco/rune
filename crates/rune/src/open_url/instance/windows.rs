#![allow(unsafe_code)]

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, INFINITE, OpenEventW, OpenProcess, PROCESS_SYNCHRONIZE, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
};

use crate::machine::{Effects, Seq};

use super::handoff::{ACCEPTED, ANSWER_WITHIN, HANDED_WAIT, MAGIC, MAX_MESSAGE, QUITTING, REACH_FOR, relaunched};

#[path = "session.rs"]
mod session;

/// An older build's message: the arguments until the sender closes, and no answer.
const MAGIC_V1: &[u8] = b"rune-instance 1\0";
/// Names tried before a launch runs unclaimed (each held by a process that doesn't answer).
const GENERATIONS: u32 = 8;
/// How long a launch waits for a running copy's answer after its first, quick ask
/// (`ANSWER_WITHIN`) went unanswered: a copy that's alive is only slow (a loaded machine).
const ANSWER_LIVE: Duration = Duration::from_secs(1);
/// How long a launch keeps handing itself to a running copy that's alive but doesn't answer
/// before it takes it for hung and starts beside it on the next name, as before. While it
/// waits, the person sees the pointer Windows shows for an app that's starting; then the
/// running copy's window comes forward with what they clicked.
const LIVE_WAIT: Duration = Duration::from_secs(5);
/// How long the server waits for a connected client to send its launch message, or to read its
/// answer and close, before disconnecting it so later launches aren't held up.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(1);
/// Maximum wire size of a `MAGIC_V1` message (`MAGIC_V1` plus `MAX_MESSAGE` bytes of body).
const MAX_V1_WIRE: usize = MAGIC_V1.len() + MAX_MESSAGE;

/// The active listener for this process's claimed instance, stopped by `release`.
static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

/// A manual-reset Win32 event shared between `release` and the listener thread to wake and stop
/// overlapped pipe operations cleanly.
struct StopEvent(HANDLE);

// SAFETY: a Win32 event handle can be signalled and waited on from any thread, and is closed
// once when the last `Arc<StopEvent>` drops.
unsafe impl Send for StopEvent {}
// SAFETY: Win32 `SetEvent` and `WaitForSingleObject`/`WaitForMultipleObjects` are thread-safe.
unsafe impl Sync for StopEvent {}

impl StopEvent {
    fn new() -> Option<Self> {
        // SAFETY: creates an unnamed, unsignalled manual-reset event; closed in `Drop`.
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        (!handle.is_null()).then_some(Self(handle))
    }

    fn handle(&self) -> HANDLE {
        self.0
    }

    fn signal(&self) {
        // SAFETY: `self.0` is an open event handle until `Drop`.
        unsafe { SetEvent(self.0) };
    }

    fn is_set(&self) -> bool {
        // SAFETY: `self.0` is an open event handle until `Drop`.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for StopEvent {
    fn drop(&mut self) {
        // SAFETY: opened in `new`, closed once when the last `Arc` drops.
        unsafe { CloseHandle(self.0) };
    }
}

/// An active pipe listener thread and its stop signal.
struct Listener {
    stop: Arc<StopEvent>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Listener {
    fn start(pipe: HANDLE, timeout: Duration, on_launch: impl Fn(Vec<String>) -> bool + Send + 'static) -> Option<Self> {
        Self::start_inner(pipe, timeout, on_launch, #[cfg(test)] None)
    }

    #[cfg(test)]
    fn start_notifying_read(
        pipe: HANDLE,
        timeout: Duration,
        on_reading: std::sync::mpsc::Sender<()>,
        on_launch: impl Fn(Vec<String>) -> bool + Send + 'static,
    ) -> Option<Self> {
        Self::start_inner(pipe, timeout, on_launch, Some(on_reading))
    }

    fn start_inner(
        pipe: HANDLE,
        timeout: Duration,
        on_launch: impl Fn(Vec<String>) -> bool + Send + 'static,
        #[cfg(test)] on_reading: Option<std::sync::mpsc::Sender<()>>,
    ) -> Option<Self> {
        let Some(stop) = StopEvent::new().map(Arc::new) else {
            // SAFETY: the handle no thread took.
            unsafe { CloseHandle(pipe) };
            return None;
        };
        let pipe_raw = pipe as usize;
        let stop_thread = Arc::clone(&stop);
        let spawned = std::thread::Builder::new().name("rune-instance".into()).spawn(move || {
            serve(pipe_raw as HANDLE, timeout, &stop_thread, &on_launch, #[cfg(test)] on_reading.as_ref());
        });
        match spawned {
            Ok(thread) => Some(Self { stop, thread: Some(thread) }),
            Err(_) => {
                // SAFETY: the handle no thread took.
                unsafe { CloseHandle(pipe) };
                None
            }
        }
    }

    fn stop(mut self) {
        self.stop.signal();
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.signal();
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

/// Closes the name: the listener cancels any pending wait, finishes answering a launch that
/// already arrived, closes its handle and ends; this returns once it has.
pub(super) fn release() {
    let Some(listener) = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    listener.stop();
    // The name is free, and the app holds nothing a next process needs: say so, so that one
    // claims now rather than once this process has ended (`released`). A manual-reset event,
    // set; its handle stays open, so it lasts as long as this process.
    // SAFETY: the name is NUL-terminated and outlives the call.
    unsafe { CreateEventW(std::ptr::null(), 1, 1, released_name(GetCurrentProcessId()).as_ptr()) };
}

/// The event a process sets once it has let go of the name itself ([`release`]), per session.
fn released_name(pid: u32) -> Vec<u16> {
    wide(&format!(r"Local\rune-instance-released-{pid}"))
}

/// Whether process `pid` let go of the name itself ([`release`], `release_instance`): it holds
/// nothing a next process of the app needs, though it may not have ended yet.
pub(crate) fn released(pid: u32) -> bool {
    /// `SYNCHRONIZE`: enough to wait on the event.
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let name = released_name(pid);
    // SAFETY: a plain handle request by a NUL-terminated name; closed once.
    unsafe {
        let event = OpenEventW(SYNCHRONIZE, 0, name.as_ptr());
        if event.is_null() {
            return false;
        }
        let set = WaitForSingleObject(event, 0) == WAIT_OBJECT_0;
        CloseHandle(event);
        set
    }
}

pub(super) fn claim(id: &str) -> bool {
    // A launch a quitting process handed on (`relaunch::spawn_with`) waits for that process to
    // let go of the name (`release_instance`, or its end): sending to it would only come back.
    let handed = crate::relaunch::handed_from().map(|pid| (pid, crate::time::now() + HANDED_WAIT));
    // A name held by a process that doesn't answer (killed, and still being torn down: its
    // pipe stays until the last of its threads goes) is passed over for the next.
    for generation in 0..GENERATIONS {
        let name = wide(&pipe_name(id, generation));
        let deadline = crate::time::now() + REACH_FOR;
        // Set once a running copy that's alive doesn't answer in time.
        let mut slow: Option<Instant> = None;
        loop {
            match create(&name) {
                Ok(pipe) => return claimed(pipe),
                Err(ERROR_ACCESS_DENIED) => {}
                // Not a pipe name this process may use: run unclaimed rather than not at all.
                Err(_) => return true,
            }
            // Once it has let go itself, whoever holds the name now is asked.
            if let Some((pid, until)) = handed
                && crate::time::now() < until
                && crate::relaunch::alive(pid)
                && !released(pid)
            {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            match send(&name, if slow.is_some() { ANSWER_LIVE } else { ANSWER_WITHIN }) {
                Sent::Delivered => return false,
                // It quits: the name is free once it has closed what it holds.
                Sent::Quitting(server) => match wait_free(&name, server) {
                    Free::Claimed(pipe) => return claimed(pipe),
                    // Still not ended after the wait (a debugger holding an exiting thread, a
                    // driver that doesn't let go): pass this name over, as a quit's own wait does.
                    Free::Held => break,
                    Free::Ask => continue,
                },
                // The first process went between the two calls: claim the name again.
                Sent::Gone if crate::time::now() < deadline => continue,
                Sent::Gone => return true,
                // Alive, and slow: asked again, never started beside, until `LIVE_WAIT` says
                // it's hung. Each ask it didn't answer it also didn't act on (`listen`).
                // A relaunch has already waited `CLAIM_WAIT` for the copy it restarts (`single_instance`):
                // a copy that still holds the name and doesn't answer is hung, and is passed over now.
                Sent::Busy if crate::relaunch::relaunched_after().is_none() && crate::time::now() < *slow.get_or_insert_with(|| crate::time::now() + LIVE_WAIT) => {}
                Sent::Busy | Sent::Nobody => break,
            }
        }
    }
    // Every name is held by a process that doesn't answer: run unclaimed rather than not at all.
    true
}

fn claimed(pipe: HANDLE) -> bool {
    *LISTENER.lock().unwrap_or_else(|e| e.into_inner()) = Listener::start(pipe, CLIENT_TIMEOUT, relaunched);
    true
}

/// How waiting for a quitting process's name ended.
enum Free {
    /// The name is this process's.
    Claimed(HANDLE),
    /// The process still hasn't ended after `HANDED_WAIT`: the next name is tried.
    Held,
    /// The process has ended, or the name is someone else's now: the name is asked again.
    Ask,
}

/// Claims `name` as soon as the quitting `server` lets go of it, for at most `HANDED_WAIT`.
fn wait_free(name: &[u16], server: u32) -> Free {
    let until = crate::time::now() + HANDED_WAIT;
    // One handle for the whole wait: once `server` is gone its pid can be another process's.
    let process = Process::open(server);
    let mut gone_at = None;
    while crate::time::now() < until {
        match create(name) {
            // Let go by `release`: it holds nothing this process needs. Freed by its teardown
            // instead, the name can come free before the rest of what `server` holds (its
            // files): keep the name, but answer only once the process object is signalled.
            Ok(pipe) => {
                if !released(server) {
                    process.wait_until(until);
                }
                return Free::Claimed(pipe);
            }
            // It let go and another launch (the relaunch it started) took the name first: that
            // one is asked, rather than this launch passing the name over and starting beside it.
            Err(ERROR_ACCESS_DENIED) if released(server) => return Free::Ask,
            Err(ERROR_ACCESS_DENIED) => {}
            Err(_) => return Free::Ask,
        }
        // Gone, yet its name is still held: not for long, or not by it.
        if process.ended() && gone_at.get_or_insert_with(crate::time::now).elapsed() > ANSWER_WITHIN {
            return Free::Ask;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if process.ended() { Free::Ask } else { Free::Held }
}

/// A process to wait for, held by handle (`SYNCHRONIZE`). None when it couldn't be opened:
/// it has already ended.
struct Process(Option<HANDLE>);

impl Process {
    fn open(pid: u32) -> Process {
        // SAFETY: a plain handle request; a process that has ended answers null.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        Process((!handle.is_null()).then_some(handle))
    }

    /// Its process object is signalled: it has closed everything it held.
    fn ended(&self) -> bool {
        // SAFETY: the handle is open until `drop`.
        self.0.is_none_or(|h| unsafe { WaitForSingleObject(h, 0) } != WAIT_TIMEOUT)
    }

    fn wait_until(&self, until: Instant) {
        let Some(h) = self.0 else { return };
        let left = until.saturating_duration_since(crate::time::now()).as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: the handle is open until `drop`.
        unsafe { WaitForSingleObject(h, left) };
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(h) = self.0 {
            // SAFETY: opened in `open`, closed once.
            unsafe { CloseHandle(h) };
        }
    }
}

/// Per user and session, so another account's app, or this account's in another session,
/// never takes this one's launches. Generation 0 is the name every build used; a later one is
/// tried when the names before it are held by processes that don't answer.
fn pipe_name(id: &str, generation: u32) -> String {
    let mut session = 0;
    // SAFETY: writes the session id into `session`.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    let user = user_sid();
    let clean = |s: &str| s.chars().map(|c| if c == '\\' || c.is_control() { '-' } else { c }).collect::<String>();
    let suffix = if generation == 0 { String::new() } else { format!("-{generation}") };
    let name = format!(r"\\.\pipe\rune-instance-{session}-{}-{}{suffix}", clean(&user), clean(id));
    name.chars().take(256).collect()
}

/// The user's SID from this process's token, as hex: what names the account without asking the
/// LSA (`GetUserNameW` loads SspiCli.dll and calls into the LSA before the first frame).
fn user_sid() -> String {
    use windows_sys::Win32::Security::{GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = std::ptr::null_mut();
    let mut buf = [0u64; 64];
    let mut len = 0u32;
    // SAFETY: this process's token, queried into a buffer of `size_of_val(&buf)` bytes (8-aligned,
    // as TOKEN_USER needs), closed after; the SID it points at lives in that buffer.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 { return String::new(); }
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), std::mem::size_of_val(&buf) as u32, &mut len) != 0;
        CloseHandle(token);
        if !ok { return String::new(); }
        let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
        let bytes = std::slice::from_raw_parts(sid as *const u8, GetLengthSid(sid) as usize);
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The pipe's first instance, opened overlapped so `ConnectNamedPipe`, `ReadFile` and
/// `WriteFile` can be bounded by deadlines and cancelled on `release`; or the error that says
/// why not (`ERROR_ACCESS_DENIED`: another process has it).
fn create(name: &[u16]) -> Result<HANDLE, u32> {
    // SAFETY: `name` is NUL-terminated and outlives the call; no security attributes means
    // the default ACL, which lets only this user (and administrators) write to it.
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            16,
            64 * 1024,
            0,
            std::ptr::null(),
        )
    };
    // SAFETY: reads this thread's last error.
    if pipe == INVALID_HANDLE_VALUE { Err(unsafe { GetLastError() }) } else { Ok(pipe) }
}

/// Waits for one client to connect to the overlapped server `pipe`, or returns false when
/// `stop` is signalled. `ERROR_NO_DATA` means a client connected, wrote and closed before this
/// call: its buffered bytes are still readable and `DisconnectNamedPipe` must still reset the pipe.
fn connect(pipe: HANDLE, stop: &StopEvent) -> bool {
    if stop.is_set() {
        return false;
    }
    matches!(
        io_result(pipe, None, Some(stop.handle()), |o| {
            // SAFETY: `pipe` is an overlapped server handle; `o` outlives the wait in `io_result`.
            unsafe { ConnectNamedPipe(pipe, o) }
        }),
        Ok(_) | Err(ERROR_PIPE_CONNECTED) | Err(ERROR_NO_DATA)
    )
}

/// Reads one launch at a time from the pipe until `stop` (or the process ends), driven by
/// `session::step`. The handle stays open meanwhile, so the name stays this process's.
fn serve(
    pipe: HANDLE,
    timeout: Duration,
    stop: &StopEvent,
    on_launch: &impl Fn(Vec<String>) -> bool,
    #[cfg(test)] on_reading: Option<&std::sync::mpsc::Sender<()>>,
) {
    let mut seq = Seq::default();
    let mut state = session::State::default();
    let mut effects = Effects::one(session::Effect::Connect);
    let mut read_buf = [0u8; session::READ_CHUNK];
    let mut drain_buf = [0u8; 1];

    while !effects.is_empty() {
        let mut next = Effects::none();
        for effect in effects {
            match effect {
                session::Effect::Connect => {
                    let ev = if stop.is_set() {
                        session::Event::Stopped
                    } else if connect(pipe, stop) {
                        session::Event::Connected { now: crate::time::now() }
                    } else if stop.is_set() {
                        session::Event::Stopped
                    } else {
                        session::Event::ConnectFailed
                    };
                    next.extend(session::step(&mut seq, &mut state, timeout, ev));
                }
                session::Effect::Read { attempt, want, deadline } => {
                    #[cfg(test)]
                    if let Some(tx) = on_reading {
                        tx.send(()).ok();
                    }
                    let ev = if stop.is_set() {
                        session::Event::Stopped
                    } else {
                        match io_result(pipe, Some(deadline), Some(stop.handle()), |o| {
                            // SAFETY: reads at most `want <= read_buf.len()` bytes into `read_buf`, which outlives the wait.
                            unsafe { ReadFile(pipe, read_buf.as_mut_ptr(), want as u32, std::ptr::null_mut(), o) }
                        }) {
                            Ok(n) => session::Event::ReadBytes { attempt, bytes: &read_buf[..n as usize] },
                            Err(ERROR_BROKEN_PIPE) => session::Event::ReadEof { attempt },
                            Err(_) if stop.is_set() => session::Event::Stopped,
                            Err(err) if err == WAIT_TIMEOUT || crate::time::now() >= deadline => {
                                session::Event::TimedOut { attempt, err }
                            }
                            Err(_) => session::Event::ReadFailed { attempt },
                        }
                    };
                    next.extend(session::step(&mut seq, &mut state, timeout, ev));
                }
                session::Effect::Hold => {
                    hold();
                    let stopping = stop.is_set();
                    let quitting = crate::quit::quitting() || stopping;
                    next.extend(session::step(&mut seq, &mut state, timeout, session::Event::Held {
                        now: crate::time::now(),
                        stopping,
                        quitting,
                    }));
                }
                session::Effect::WriteAnswer { attempt, byte, deadline, cancellable } => {
                    let buf = [byte];
                    let stop_handle = cancellable.then(|| stop.handle());
                    let ev = if cancellable && stop.is_set() {
                        session::Event::Stopped
                    } else {
                        match io_result(pipe, Some(deadline), stop_handle, |o| {
                            // SAFETY: writes one byte from `buf`, which outlives the wait in `io_result`.
                            unsafe { WriteFile(pipe, buf.as_ptr(), 1, std::ptr::null_mut(), o) }
                        }) {
                            Ok(n) => session::Event::WroteAnswer { attempt, bytes: n as usize },
                            Err(_) if cancellable && stop.is_set() => session::Event::Stopped,
                            Err(err) if err == WAIT_TIMEOUT || crate::time::now() >= deadline => {
                                session::Event::TimedOut { attempt, err }
                            }
                            Err(err) => session::Event::WriteAnswerFailed { attempt, err },
                        }
                    };
                    next.extend(session::step(&mut seq, &mut state, timeout, ev));
                }
                session::Effect::DrainAnswer { attempt, deadline, cancellable } => {
                    let stop_handle = cancellable.then(|| stop.handle());
                    let ev = if cancellable && stop.is_set() {
                        session::Event::Stopped
                    } else {
                        match io_result(pipe, Some(deadline), stop_handle, |o| {
                            // SAFETY: reads at most one byte into `drain_buf`, which outlives the wait in `io_result`.
                            unsafe { ReadFile(pipe, drain_buf.as_mut_ptr(), 1, std::ptr::null_mut(), o) }
                        }) {
                            Err(ERROR_BROKEN_PIPE) => session::Event::DrainEof { attempt },
                            Ok(n) => session::Event::DrainRead { attempt, bytes: n as usize },
                            Err(_) if cancellable && stop.is_set() => session::Event::Stopped,
                            Err(err) if err == WAIT_TIMEOUT || crate::time::now() >= deadline => {
                                session::Event::TimedOut { attempt, err }
                            }
                            Err(err) => session::Event::DrainFailed { attempt, err },
                        }
                    };
                    next.extend(session::step(&mut seq, &mut state, timeout, ev));
                }
                session::Effect::AnswerFailed { phase, err } => {
                    eprintln!("rune-instance: a launch gave up before it read its answer ({phase} failed, error {err}); not acted on");
                }
                session::Effect::Deliver(args) => {
                    let acted = on_launch(args);
                    // Counted by the harness (no arguments: they may hold what the person typed).
                    if HOLD.get().is_some_and(|ms| *ms > 0) {
                        eprintln!("rune-instance: {}", if acted { "acted on a launch" } else { "a launch asked again after it was acted on; skipped" });
                    }
                }
                session::Effect::Disconnect => {
                    // SAFETY: ends this client's connection, so the next can connect.
                    unsafe { DisconnectNamedPipe(pipe) };
                }
                session::Effect::Close => {
                    // SAFETY: the server handle this thread owns, closed once: the name is free.
                    unsafe { CloseHandle(pipe) };
                }
            }
        }
        effects = next;
    }
    debug_assert!(matches!(state, session::State::Stopped));
}

/// A harness's stand-in for a busy machine: `RUNE_INSTANCE_HOLD_MS` holds each launch that long
/// before it's answered. Only under `RUNE_DIRS_ROOT`, a scratch run's.
static HOLD: OnceLock<u64> = OnceLock::new();

fn hold() {
    let ms = *HOLD.get_or_init(|| {
        rune_core::env::var("RUNE_DIRS_ROOT").and(rune_core::env::var("RUNE_INSTANCE_HOLD_MS")).and_then(|v| v.parse().ok()).unwrap_or(0)
    });
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

enum Sent {
    /// The running process took the launch.
    Delivered,
    /// The running process quits: it lets go of the name once it has closed what it holds.
    Quitting(u32),
    /// No process has the name now.
    Gone,
    /// A process has the name and doesn't answer (killed and still being torn down, or gone
    /// with an inherited handle left): nobody is there to take the launch.
    Nobody,
    /// A running process has the name and didn't answer in time (or talks to another launch):
    /// slow, or hung.
    Busy,
}

/// Sends this launch's arguments to the process that has the pipe and waits, at most
/// `within`, for its answer.
fn send(name: &[u16], within: Duration) -> Sent {
    let deadline = crate::time::now() + within;
    let left = || u32::try_from(deadline.saturating_duration_since(crate::time::now()).as_millis()).unwrap_or(u32::MAX);
    let pipe = loop {
        // SAFETY: `name` is NUL-terminated; opens the pipe's client end, overlapped so the
        // answer can be waited for with a deadline.
        let pipe = unsafe { CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED, std::ptr::null_mut()) };
        if pipe != INVALID_HANDLE_VALUE {
            break pipe;
        }
        // SAFETY: reads this thread's last error.
        match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => return Sent::Gone,
            // Another launch is talking to it: wait for the pipe to be free.
            // SAFETY: `name` is NUL-terminated.
            ERROR_PIPE_BUSY if left() > 0 => unsafe {
                WaitNamedPipeW(name.as_ptr(), left());
            },
            // Still talking to another launch.
            ERROR_PIPE_BUSY => return Sent::Busy,
            _ => return Sent::Nobody,
        }
    };
    let mut server = 0;
    // SAFETY: `pipe` is the open client handle. The first process may take the foreground
    // for the window this launch asks for, as this process, which the user just started, could.
    unsafe {
        if GetNamedPipeServerProcessId(pipe, &mut server) != 0 {
            AllowSetForegroundWindow(server);
        }
    }
    // A killed process's pipe outlasts its exit code until the kernel closes its handles:
    // nobody reads it, and the name comes free once it's gone. Wait for that (as for a quit)
    // rather than take the next name and start beside a process that still holds files.
    if server != 0 && crate::relaunch::ending(server) {
        // SAFETY: the client handle, closed once.
        unsafe { CloseHandle(pipe) };
        return Sent::Quitting(server);
    }
    // Held by a process that is gone (an inherited handle): nobody reads it.
    if server != 0 && !crate::relaunch::alive(server) {
        // SAFETY: the client handle, closed once.
        unsafe { CloseHandle(pipe) };
        return Sent::Nobody;
    }
    let message = super::handoff::message();
    let mut answer = [0u8; 1];

    let answered = io(pipe, deadline, |o| {
        // SAFETY: writes `message`, which outlives the wait in `io`, to the open pipe.
        unsafe { WriteFile(pipe, message.as_ptr(), message.len() as u32, std::ptr::null_mut(), o) }
    })
    .filter(|n| *n as usize == message.len())
    .inspect(|_| crate::marks::mark("instance.sent"))
    .and_then(|_| {
        io(pipe, deadline, |o| {
            // SAFETY: reads at most one byte into `answer`, which outlives the wait in `io`.
            unsafe { ReadFile(pipe, answer.as_mut_ptr(), 1, std::ptr::null_mut(), o) }
        })
    });
    // SAFETY: the client handle, closed once.
    unsafe { CloseHandle(pipe) };
    crate::marks::mark("instance.answered");
    match (answered, answer[0]) {
        (Some(1), ACCEPTED) => Sent::Delivered,
        (Some(1), QUITTING) => Sent::Quitting(server),
        // It's alive (checked above) and didn't answer in time.
        _ if server != 0 => Sent::Busy,
        _ => Sent::Nobody,
    }
}

/// One overlapped read or write on `pipe`, waited for until `deadline`: the bytes it moved, or
/// none when it failed. One that didn't finish in time is cancelled, but if it completed as the
/// wait ran out, what it moved counts: an answer read is an answer the other end saw taken.
fn io(pipe: HANDLE, deadline: Instant, start: impl FnOnce(*mut OVERLAPPED) -> i32) -> Option<u32> {
    io_result(pipe, Some(deadline), None, start).ok()
}

/// One overlapped operation on `pipe`, optionally bounded by `deadline` and cancellable by
/// `stop`. Returns the bytes moved, or the Win32 error code when it failed or was cancelled.
fn io_result(
    pipe: HANDLE,
    deadline: Option<Instant>,
    stop: Option<HANDLE>,
    start: impl FnOnce(*mut OVERLAPPED) -> i32,
) -> Result<u32, u32> {
    if let Some(until) = deadline && crate::time::now() >= until {
        return Err(WAIT_TIMEOUT);
    }
    // SAFETY: a manual-reset event, unnamed; closed once below.
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        // SAFETY: reads this thread's last error.
        return Err(unsafe { GetLastError() });
    }
    // SAFETY: an all-zero OVERLAPPED is a valid start for an operation; it lives until the
    // operation is done or cancelled and waited for.
    let mut o: OVERLAPPED = unsafe { std::mem::zeroed() };
    o.hEvent = event;
    // SAFETY: `o` outlives the operation: when `start` pends, it is waited for, or cancelled and
    // waited for via `GetOverlappedResult(..., 1)`, before returning.
    unsafe {
        if start(&mut o) == 0 {
            let err = GetLastError();
            if err != ERROR_IO_PENDING {
                CloseHandle(event);
                return Err(err);
            }
            let left = match deadline {
                Some(until) => u32::try_from(until.saturating_duration_since(crate::time::now()).as_millis())
                    .unwrap_or(u32::MAX - 1)
                    .clamp(1, u32::MAX - 1),
                None => INFINITE,
            };
            let waited = match stop {
                Some(stop) => {
                    let handles = [event, stop];
                    WaitForMultipleObjects(2, handles.as_ptr(), 0, left)
                }
                None => WaitForSingleObject(event, left),
            };
            if waited != WAIT_OBJECT_0 {
                CancelIoEx(pipe, &o);
            }
        }
        let mut n = 0;
        let ok = GetOverlappedResult(pipe, &o, &mut n, 1) != 0;
        let err = if ok { 0 } else { GetLastError() };
        CloseHandle(event);
        if ok { Ok(n) } else { Err(err) }
    }
}

/// Restores and activates this process's front app window: its topmost visible top-level
/// window that isn't owned or a tool window (an overlay).
pub(crate) fn raise_front() {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        // SAFETY: `found` points at the `HWND` `raise_front` passed, alive for the walk.
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0;
            let owned = !GetWindow(hwnd, GW_OWNER).is_null();
            if pid == GetCurrentProcessId() && IsWindowVisible(hwnd) != 0 && !tool && !owned {
                *(found as *mut HWND) = hwnd;
                return 0;
            }
            1
        }
    }
    let mut found: HWND = std::ptr::null_mut();
    // SAFETY: the callback only writes `found`, which outlives the walk, front to back.
    unsafe {
        EnumWindows(Some(each), &mut found as *mut HWND as LPARAM);
        if found.is_null() {
            return;
        }
        if IsIconic(found) != 0 {
            ShowWindow(found, SW_RESTORE);
        }
        SetForegroundWindow(found);
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
