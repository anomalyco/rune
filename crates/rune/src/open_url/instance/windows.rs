#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
};

use super::handoff::{ACCEPTED, ANSWER_WITHIN, HANDED_WAIT, MAGIC, MAX_MESSAGE, QUITTING, REACH_FOR, relaunched};

/// An older build's message: the arguments until the sender closes, and no answer.
const MAGIC_V1: &[u8] = b"rune-instance 1\0";
/// Names tried before a launch runs unclaimed (each held by a process that doesn't answer).
const GENERATIONS: u32 = 8;

/// The claimed name, and the listener's thread, for `release`.
static CLAIMED: OnceLock<Vec<u16>> = OnceLock::new();
static LISTENER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
/// `release` asked the listener to close the name.
static RELEASED: AtomicBool = AtomicBool::new(false);

/// Closes the name: the listener takes what's connected (a launch then is handed on, since
/// the app is quitting), closes its handle and ends; this returns once it has.
pub(super) fn release() {
    let Some(name) = CLAIMED.get() else { return };
    if RELEASED.swap(true, Ordering::AcqRel) {
        return;
    }
    // An empty connection wakes the listener from ConnectNamedPipe; one that's busy with a
    // launch sees the flag once it's done with it.
    // SAFETY: `name` is NUL-terminated; the handle, when opened, is closed once.
    let woken = unsafe {
        let wake = CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut());
        if wake != INVALID_HANDLE_VALUE {
            CloseHandle(wake);
            true
        } else {
            GetLastError() == ERROR_PIPE_BUSY
        }
    };
    // A listener that can't be woken keeps the name until the process ends.
    if let Some(listener) = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).take().filter(|_| woken) {
        listener.join().ok();
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
        loop {
            match create(&name) {
                Ok(pipe) => return claimed(name, pipe),
                Err(ERROR_ACCESS_DENIED) => {}
                // Not a pipe name this process may use: run unclaimed rather than not at all.
                Err(_) => return true,
            }
            if let Some((pid, until)) = handed
                && crate::time::now() < until
                && crate::relaunch::alive(pid)
            {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            match send(&name) {
                Sent::Delivered => return false,
                // It quits: the name is free once it has closed what it holds.
                Sent::Quitting(server) => match wait_free(&name, server) {
                    Free::Claimed(pipe) => return claimed(name, pipe),
                    // Still not ended after the wait (a debugger holding an exiting thread, a
                    // driver that doesn't let go): pass this name over, as a quit's own wait does.
                    Free::Held => break,
                    Free::Ask => continue,
                },
                // The first process went between the two calls: claim the name again.
                Sent::Gone if crate::time::now() < deadline => continue,
                Sent::Gone => return true,
                Sent::Nobody => break,
            }
        }
    }
    // Every name is held by a process that doesn't answer: run unclaimed rather than not at all.
    true
}

fn claimed(name: Vec<u16>, pipe: HANDLE) -> bool {
    CLAIMED.set(name).ok();
    listen(pipe);
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
            // The name can come free before the rest of what `server` holds (its files):
            // keep the name, but answer only once the process object is signalled.
            Ok(pipe) => {
                process.wait_until(until);
                return Free::Claimed(pipe);
            }
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

/// The pipe's first instance, or the error that says why not (`ERROR_ACCESS_DENIED`: another
/// process has it).
fn create(name: &[u16]) -> Result<HANDLE, u32> {
    // SAFETY: `name` is NUL-terminated and outlives the call; no security attributes means
    // the default ACL, which lets only this user (and administrators) write to it.
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
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

/// Reads one launch at a time from the pipe until `release` (or the process ends), and answers
/// each (`ACCEPTED`, or `QUITTING` while the app quits: the launch then claims the instance
/// itself once this process lets go). The handle stays open meanwhile, so the name stays this
/// process's.
fn listen(pipe: HANDLE) {
    let pipe = pipe as usize;
    let spawned = std::thread::Builder::new().name("rune-instance".into()).spawn(move || {
        let pipe = pipe as HANDLE;
        loop {
            // SAFETY: `pipe` is the open server handle; a client that connected before this
            // call answers ERROR_PIPE_CONNECTED.
            let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if connected && let Some(launch) = read(pipe) {
                let quitting = crate::quit::quitting();
                if launch.answers {
                    let answer = [if quitting { QUITTING } else { ACCEPTED }];
                    let mut written = 0;
                    // SAFETY: one byte from `answer` to the connected client; the flush waits
                    // for the client to read it (or to close), so the disconnect below
                    // doesn't discard it.
                    unsafe {
                        WriteFile(pipe, answer.as_ptr(), 1, &mut written, std::ptr::null_mut());
                        FlushFileBuffers(pipe);
                    }
                }
                // A launch told the app quits claims the instance itself; one that couldn't
                // be told (an older sender) is started again once this process exits.
                if !(quitting && launch.answers) {
                    relaunched(launch.args);
                }
            }
            // SAFETY: ends this client's connection, so the next can connect.
            unsafe { DisconnectNamedPipe(pipe) };
            if RELEASED.load(Ordering::Acquire) {
                // SAFETY: the server handle this thread owns, closed once: the name is free.
                unsafe { CloseHandle(pipe) };
                return;
            }
        }
    });
    match spawned {
        Ok(listener) => *LISTENER.lock().unwrap_or_else(|e| e.into_inner()) = Some(listener),
        // SAFETY: the handle no thread took.
        Err(_) => unsafe {
            CloseHandle(pipe as HANDLE);
        },
    }
}

/// A launch read from the pipe: its arguments, and whether its sender waits for an answer.
struct Launch {
    args: Vec<String>,
    answers: bool,
}

/// One client's message: `MAGIC`, the body's length (u32, little-endian) and the body, after
/// which the client waits for its answer; or, from an older sender, `MAGIC_V1` and the body
/// until it closes its end. None for anything else.
fn read(pipe: HANDLE) -> Option<Launch> {
    let mut message = Vec::new();
    let mut buf = [0u8; 4096];
    let complete = |m: &[u8]| {
        let len = m.strip_prefix(MAGIC)?.get(..4)?;
        let len = u32::from_le_bytes(len.try_into().ok()?) as usize;
        (m.len() >= MAGIC.len() + 4 + len).then_some(len)
    };
    let (body, answers) = loop {
        if let Some(len) = complete(&message) {
            break (message[MAGIC.len() + 4..MAGIC.len() + 4 + len].to_vec(), true);
        }
        let mut got = 0;
        // SAFETY: reads at most `buf.len()` bytes into `buf`.
        let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr(), buf.len() as u32, &mut got, std::ptr::null_mut()) } != 0;
        message.extend_from_slice(&buf[..got as usize]);
        if message.len() > MAX_MESSAGE {
            return None;
        }
        // SAFETY: reads this thread's last error.
        if !ok && unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
            break (message.strip_prefix(MAGIC_V1)?.to_vec(), false);
        }
        if !ok {
            return None;
        }
    };
    let text = std::str::from_utf8(&body).ok()?;
    Some(Launch { args: text.split('\0').filter(|a| !a.is_empty()).map(str::to_owned).collect(), answers })
}

enum Sent {
    /// The running process took the launch.
    Delivered,
    /// The running process quits: it lets go of the name once it has closed what it holds.
    Quitting(u32),
    /// No process has the name now.
    Gone,
    /// A process has the name and doesn't answer (killed and still being torn down, or hung):
    /// nobody is there to take the launch.
    Nobody,
}

/// Sends this launch's arguments to the process that has the pipe and waits, at most
/// `ANSWER_WITHIN`, for its answer.
fn send(name: &[u16]) -> Sent {
    let deadline = crate::time::now() + ANSWER_WITHIN;
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
        _ => Sent::Nobody,
    }
}

/// One overlapped read or write on `pipe`, waited for until `deadline`: the bytes it moved, or
/// none when it failed or didn't finish in time (then it's cancelled).
fn io(pipe: HANDLE, deadline: Instant, start: impl FnOnce(*mut OVERLAPPED) -> i32) -> Option<u32> {
    // SAFETY: a manual-reset event, unnamed; closed once below.
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return None;
    }
    // SAFETY: an all-zero OVERLAPPED is a valid start for an operation; it lives until the
    // operation is done or cancelled and waited for.
    let mut o: OVERLAPPED = unsafe { std::mem::zeroed() };
    o.hEvent = event;
    let left = u32::try_from(deadline.saturating_duration_since(crate::time::now()).as_millis()).unwrap_or(u32::MAX);
    // SAFETY: `o` outlives the operation: it's waited for, or cancelled and waited for, below.
    unsafe {
        let started = start(&mut o) != 0 || GetLastError() == ERROR_IO_PENDING;
        let finished = started && WaitForSingleObject(event, left) == WAIT_OBJECT_0;
        if started && !finished {
            CancelIoEx(pipe, &o);
        }
        let mut n = 0;
        let ok = started && GetOverlappedResult(pipe, &o, &mut n, 1) != 0;
        CloseHandle(event);
        (finished && ok).then_some(n)
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
