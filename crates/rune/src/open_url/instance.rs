//! One running process per app: [`single_instance`](crate::single_instance).
//!
//! Windows starts a new process for every launch: a Start menu click, a `myapp://` link, a
//! clicked notification that opens a link. The first process of the app claims a named pipe
//! for the signed-in user's session (`\\.\pipe\rune-instance-<session>-<user>-<id>`); a later
//! one finds it taken, lets the first take the foreground (`AllowSetForegroundWindow`, since
//! Windows gives it only to the process the user just started), sends its launch arguments down
//! the pipe and exits. The first process reads them on a thread of its own: the links among
//! them go to `App::on_open_url` as a link the system opened with the app would, and a launch
//! without links reopens the app, which brings its front window forward.
//!
//! Linux starts a new process for every launch too (a desktop entry, `xdg-open myapp://…`). The
//! first process binds a Unix socket in the user's runtime folder
//! (`$XDG_RUNTIME_DIR/rune-instance-<id>.sock`, which only the user can reach); a later one
//! connects, sends the same message and waits for the same answer. A socket left by a process
//! that died refuses the connection: the launch removes it and binds its own. One held by a
//! process that doesn't answer is taken over the same way, as Windows passes over its name.
//! Launches claim one at a time (a lock beside the socket), so two can't both take one over.
//!
//! The links of the launch that started the first process come from its arguments
//! ([`launch_links`]), so `App::on_open_url` gets those too.
//!
//! macOS keeps one process per app itself and hands links to it, so there this only says yes.

/// Makes this process the app's only running one, before it opens a window (Windows, Linux).
///
/// Returns false when another process of the app already runs: this launch's links went to
/// its `App::on_open_url`, or, without links, it brought its front window forward, so return
/// from `main` without opening a window. Returns true when this is the first process, which
/// from now on receives the launches that follow, and also when the first can't be reached
/// (it hangs or is exiting), so the launch still opens a window. A launch that reaches the first
/// while it quits (its windows closing) starts the app again once that process has exited, as
/// `Tx::relaunch` does, instead of being lost with it. `id` names the app (its
/// reverse-DNS identifier); builds that should run side by side use different ids.
///
/// macOS and other platforms: always true. macOS already reaches the running app.
pub fn single_instance(id: &str) -> bool {
    // A relaunch (`Tx::relaunch`) claims once the process it restarts has let go.
    crate::relaunch::wait_for_previous();
    #[cfg(target_os = "windows")]
    {
        windows::claim(id)
    }
    #[cfg(target_os = "linux")]
    {
        linux::claim(id)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = id;
        true
    }
}

/// Lets go of the app's single-instance name before the process ends (Windows, Linux): call it
/// once the app holds nothing a next process of it needs (its files closed). A launch that came
/// while the app quit was handed on to a new process (`single_instance`), which claims the name as
/// soon as this returns rather than when this process is gone. Without it the name goes with the
/// process. Elsewhere it does nothing.
pub fn release_instance() {
    #[cfg(target_os = "windows")]
    windows::release();
    #[cfg(target_os = "linux")]
    linux::release();
}

/// Whether a launch argument is a link: `scheme://…`, with a scheme of at least two letters,
/// digits, `+`, `-` or `.`, starting with a letter. A Windows path (`C:\…`) and a flag aren't.
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
pub(crate) fn is_link(arg: &str) -> bool {
    let Some((scheme, _)) = arg.split_once("://") else { return false };
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The links among this process's launch arguments (Windows passes a link to the process it
/// starts for it as an argument).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn launch_links() -> Vec<String> {
    std::env::args().skip(1).filter(|a| is_link(a)).collect()
}

/// The loop's side of a later launch without links (`on_relaunch`), and the window it brings
/// forward (Windows).
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) use handoff::{exited, launches_waiting, on_relaunch, served};
#[cfg(target_os = "windows")]
pub(crate) use windows::raise_front;

/// What Windows' pipe and Linux's socket share: the message, its answers and their waits, and
/// what the first process does with a launch it took.
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod handoff {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// Opens every message, so a stray client's bytes aren't taken for arguments: a length and
    /// the arguments follow, and the sender waits for one byte back (`ACCEPTED`, `QUITTING`).
    pub(super) const MAGIC: &[u8] = b"rune-instance 2\0";
    /// The running process took the launch.
    pub(super) const ACCEPTED: u8 = b'A';
    /// The running process quits: the launch claims the instance once it lets go.
    pub(super) const QUITTING: u8 = b'Q';
    /// How long a launch waits for the running process to answer before it takes the name for
    /// held by nobody. A healthy process answers in 63-82 µs (p50-p95, Windows, release); this
    /// leaves room for one that waits for a busy CPU, and is what a launch pays while a killed
    /// process's pipe lingers and its pid can't be read.
    pub(super) const ANSWER_WITHIN: Duration = Duration::from_millis(50);
    /// The most a launch's arguments may take.
    pub(super) const MAX_MESSAGE: usize = 1 << 20;
    /// How long a later launch keeps trying to reach the first process.
    pub(super) const REACH_FOR: Duration = Duration::from_secs(5);
    /// How long a launch handed on by a quitting process waits for it to let go of the name.
    pub(super) const HANDED_WAIT: Duration = Duration::from_secs(10);

    /// This launch's message: `MAGIC`, the body's length (u32, little-endian), and the body (the
    /// arguments, NUL-separated).
    pub(super) fn message() -> Vec<u8> {
        let body = std::env::args().skip(1).collect::<Vec<_>>().join("\0").into_bytes();
        let mut message = MAGIC.to_vec();
        message.extend((body.len() as u32).to_le_bytes());
        message.extend(body);
        message
    }

    /// A message's body as the launch's arguments; none when it isn't UTF-8.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(super) fn arguments(body: &[u8]) -> Option<Vec<String>> {
        Some(std::str::from_utf8(body).ok()?.split('\0').filter(|a| !a.is_empty()).map(str::to_owned).collect())
    }
    type Relaunch = Box<dyn Fn() -> bool + Send>;

    /// Sends a launch without links to the loop as a reopen; set once the loop runs.
    static RELAUNCH: Mutex<Option<Relaunch>> = Mutex::new(None);

    /// Launches handed to the loop (a reopen, or links for `App::on_open_url`) that no shown
    /// window has served yet; once the loop has ended (`exited`), a launch starts the app again
    /// instead.
    struct Waiting {
        launches: Vec<(Instant, Vec<String>)>,
        exited: bool,
    }

    static WAITING: Mutex<Waiting> = Mutex::new(Waiting { launches: Vec::new(), exited: false });
    /// Whether `WAITING` holds a launch: the loop's per-turn check costs a load.
    static PENDING: AtomicBool = AtomicBool::new(false);
    /// A launch older than this when the loop ends was served by a window that went later (an app
    /// that runs on without windows, then quits): it doesn't start the app again.
    const UNSERVED_FOR: Duration = Duration::from_secs(5);

    /// Whether a launch waits for a shown window to serve it.
    pub(crate) fn launches_waiting() -> bool {
        PENDING.load(Ordering::Acquire)
    }

    /// The loop goes on with a window shown: the launches so far reached it.
    pub(crate) fn served() {
        WAITING.lock().unwrap_or_else(|e| e.into_inner()).launches.clear();
        PENDING.store(false, Ordering::Release);
    }

    /// The loop ended: each launch no window served starts the app again once this process lets
    /// go of the instance, as does every launch from now on.
    pub(crate) fn exited() {
        let launches = {
            let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
            waiting.exited = true;
            std::mem::take(&mut waiting.launches)
        };
        for (at, args) in launches {
            if at.elapsed() < UNSERVED_FOR {
                crate::relaunch::spawn_with(args);
            }
        }
    }

    /// The loop runs: a launch without links calls `relaunch` from now on (`WakeEvent::Reopen`).
    pub(crate) fn on_relaunch(relaunch: impl Fn() -> bool + Send + 'static) {
        *RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(relaunch));
    }

    /// A later launch's arguments: its links to `App::on_open_url`, or, without links, a reopen.
    /// One that comes while the app quits, or that the loop hadn't taken when it ended, starts the
    /// app again with them once this process has exited, as `Tx::relaunch` does: the windows it
    /// would reach are closing, and the launch would be lost.
    pub(super) fn relaunched(args: Vec<String>) {
        if crate::quit::quitting() {
            return crate::relaunch::spawn_with(args);
        }
        let links: Vec<String> = args.iter().filter(|a| super::is_link(a)).cloned().collect();
        {
            let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
            if waiting.exited {
                drop(waiting);
                return crate::relaunch::spawn_with(args);
            }
            waiting.launches.push((Instant::now(), args));
            PENDING.store(true, Ordering::Release);
        }
        if links.is_empty() {
            let sent = RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|relaunch| relaunch());
            if !sent {
                exited();
            }
            return;
        }
        for link in links {
            super::super::incoming::receive(link);
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WAIT_OBJECT_0,
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
    use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, WaitForSingleObject};
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
        let handed = crate::relaunch::handed_from().map(|pid| (pid, Instant::now() + HANDED_WAIT));
        // A name held by a process that doesn't answer (killed, and still being torn down: its
        // pipe stays until the last of its threads goes) is passed over for the next.
        for generation in 0..GENERATIONS {
            let name = wide(&pipe_name(id, generation));
            let deadline = Instant::now() + REACH_FOR;
            loop {
                match create(&name) {
                    Ok(pipe) => return claimed(name, pipe),
                    Err(ERROR_ACCESS_DENIED) => {}
                    // Not a pipe name this process may use: run unclaimed rather than not at all.
                    Err(_) => return true,
                }
                if let Some((pid, until)) = handed
                    && Instant::now() < until
                    && crate::relaunch::alive(pid)
                {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                match send(&name) {
                    Sent::Delivered => return false,
                    // It quits: the name is free once it has closed what it holds.
                    Sent::Quitting(server) => match wait_free(&name, server) {
                        Some(pipe) => return claimed(name, pipe),
                        None => continue,
                    },
                    // The first process went between the two calls: claim the name again.
                    Sent::Gone if Instant::now() < deadline => continue,
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

    /// Claims `name` as soon as the quitting `server` lets go of it, for at most `HANDED_WAIT`;
    /// none when it doesn't (then the name is asked again, and passed over if nobody answers).
    fn wait_free(name: &[u16], server: u32) -> Option<HANDLE> {
        let until = Instant::now() + HANDED_WAIT;
        let mut gone_at = None;
        while Instant::now() < until {
            match create(name) {
                Ok(pipe) => return Some(pipe),
                Err(ERROR_ACCESS_DENIED) => {}
                Err(_) => return None,
            }
            // Gone, yet its name is still held: not for long, or not by it.
            if !crate::relaunch::alive(server) && gone_at.get_or_insert_with(Instant::now).elapsed() > ANSWER_WITHIN {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
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
        let deadline = Instant::now() + ANSWER_WITHIN;
        let left = || u32::try_from(deadline.saturating_duration_since(Instant::now()).as_millis()).unwrap_or(u32::MAX);
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
        // A killed process's pipe can outlast it (its kernel objects go after its exit): nobody
        // reads it, so don't wait for an answer.
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
        let left = u32::try_from(deadline.saturating_duration_since(Instant::now()).as_millis()).unwrap_or(u32::MAX);
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
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{ErrorKind, Read, Write};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use super::handoff::{ACCEPTED, ANSWER_WITHIN, HANDED_WAIT, MAGIC, MAX_MESSAGE, QUITTING, REACH_FOR, arguments, message, relaunched};

    /// `sockaddr_un`'s path holds 108 bytes, its NUL included.
    const PATH_MAX: usize = 107;

    /// The claimed socket's path and inode (a launch that took it over binds its own there, which
    /// `release` leaves alone), and the listener's thread.
    static CLAIMED: OnceLock<(PathBuf, u64)> = OnceLock::new();
    static LISTENER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
    /// `release` asked the listener to stop.
    static RELEASED: AtomicBool = AtomicBool::new(false);

    pub(super) fn claim(id: &str) -> bool {
        // No runtime folder of the user's own: run unclaimed rather than not at all.
        let Some(path) = socket_path(id) else { return true };
        let handed = crate::relaunch::handed_from().map(|pid| (pid, Instant::now() + HANDED_WAIT));
        // One launch at a time decides: between finding the socket stale (or its process hung) and
        // binding its own, no other launch looks. The kernel lets go of a killed holder's lock.
        let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path.with_extension("lock")) else { return true };
        if lock.lock().is_err() {
            return true;
        }
        let deadline = Instant::now() + REACH_FOR;
        loop {
            match UnixListener::bind(&path) {
                Ok(listener) => return claimed(path, listener),
                Err(e) if e.kind() == ErrorKind::AddrInUse => {}
                Err(_) => return true,
            }
            // A launch a quitting process handed on waits for it to let go (`release_instance`).
            if let Some((pid, until)) = handed
                && Instant::now() < until
                && crate::relaunch::alive(pid)
            {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            match send(&path) {
                Sent::Delivered => return false,
                Sent::Quitting => wait_free(&path),
                // Nobody listens there: the process that bound it is gone. Or one listens and
                // doesn't answer (hung): the launches from now on are this process's.
                Sent::Refused | Sent::Nobody => {
                    std::fs::remove_file(&path).ok();
                }
                // Removed between the bind and the connect: bind again.
                Sent::Gone if Instant::now() < deadline => {}
                Sent::Gone => return true,
            }
        }
    }

    /// The socket's path: in `$XDG_RUNTIME_DIR` (the user's own, mode 0700), else in a folder of
    /// the user's own under the temporary folder. None when neither can be had, or the path is
    /// too long for a socket.
    fn socket_path(id: &str) -> Option<PathBuf> {
        let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '-' }).collect();
        let dir = match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute() && d.is_dir()) {
            Some(dir) => dir,
            None => own_temp_dir()?,
        };
        let path = dir.join(format!("rune-instance-{clean}.sock"));
        (path.as_os_str().len() <= PATH_MAX).then_some(path)
    }

    /// `<temp>/rune-<uid>`, made 0700; none when it's someone else's or others may write to it.
    fn own_temp_dir() -> Option<PathBuf> {
        // SAFETY: getuid has no preconditions and can't fail.
        let uid = unsafe { libc::getuid() };
        let dir = std::env::temp_dir().join(format!("rune-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        let meta = std::fs::symlink_metadata(&dir).ok()?;
        (meta.is_dir() && meta.uid() == uid && meta.permissions().mode() & 0o077 == 0).then_some(dir)
    }

    fn claimed(path: PathBuf, listener: UnixListener) -> bool {
        let inode = std::fs::metadata(&path).map_or(0, |m| m.ino());
        CLAIMED.set((path, inode)).ok();
        let spawned = std::thread::Builder::new().name("rune-instance".into()).spawn(move || {
            for stream in listener.incoming() {
                if RELEASED.load(Ordering::Acquire) {
                    return;
                }
                let Ok(mut stream) = stream else { continue };
                // A client that connects and sends nothing doesn't hold the next launch up for long.
                stream.set_read_timeout(Some(Duration::from_secs(1))).ok();
                let Some(args) = read(&mut stream) else { continue };
                let quitting = crate::quit::quitting();
                // A sender that stopped waiting (this process didn't answer in time, and the launch
                // went on without it) has closed its end: the answer fails and the launch is its own.
                let told = stream.write_all(&[if quitting { QUITTING } else { ACCEPTED }]).is_ok();
                drop(stream);
                // A launch told the app quits claims the instance itself.
                if told && !quitting {
                    relaunched(args);
                }
            }
        });
        if let Ok(listener) = spawned {
            *LISTENER.lock().unwrap_or_else(|e| e.into_inner()) = Some(listener);
        }
        true
    }

    /// Stops listening: a launch from now on binds the name itself (the socket is removed if it's
    /// still this process's), and one already connected is told the app quits. Returns once the
    /// listener has ended.
    pub(super) fn release() {
        let Some((path, inode)) = CLAIMED.get() else { return };
        if RELEASED.swap(true, Ordering::AcqRel) {
            return;
        }
        // A launch took the name over while this process didn't answer: the socket there is its,
        // and this listener, which nothing reaches any more, goes with the process.
        if !std::fs::metadata(path).is_ok_and(|m| m.ino() == *inode) {
            return;
        }
        // An empty connection wakes the listener from `accept`; it's made before the socket goes.
        let wake = UnixStream::connect(path).ok();
        std::fs::remove_file(path).ok();
        if let Some(listener) = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).take().filter(|_| wake.is_some()) {
            listener.join().ok();
        }
    }

    /// One client's message (`MAGIC`, the body's length, the body); none for anything else.
    fn read(stream: &mut UnixStream) -> Option<Vec<String>> {
        let mut head = [0u8; MAGIC.len() + 4];
        stream.read_exact(&mut head).ok()?;
        let len = u32::from_le_bytes(head.strip_prefix(MAGIC)?.try_into().ok()?) as usize;
        if len > MAX_MESSAGE {
            return None;
        }
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).ok()?;
        arguments(&body)
    }

    enum Sent {
        /// The running process took the launch.
        Delivered,
        /// The running process quits: it removes the socket once it has closed what it holds.
        Quitting,
        /// No socket there now.
        Gone,
        /// A socket and nobody listening: its process died without removing it.
        Refused,
        /// A process listens and doesn't answer within `ANSWER_WITHIN` (hung, or stopped).
        Nobody,
    }

    /// Sends this launch's arguments to the process listening at `path` and waits, at most
    /// `ANSWER_WITHIN`, for its answer.
    fn send(path: &Path) -> Sent {
        let deadline = Instant::now() + ANSWER_WITHIN;
        let mut stream = match UnixStream::connect(path) {
            Ok(stream) => stream,
            Err(e) if e.kind() == ErrorKind::ConnectionRefused => return Sent::Refused,
            Err(e) if e.kind() == ErrorKind::NotFound => return Sent::Gone,
            Err(_) => return Sent::Nobody,
        };
        let left = || Some(deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)));
        stream.set_write_timeout(left()).ok();
        let mut answer = [0u8; 1];
        let answered = stream.write_all(&message()).is_ok() && {
            crate::marks::mark("instance.sent");
            stream.set_read_timeout(left()).ok();
            stream.read_exact(&mut answer).is_ok()
        };
        crate::marks::mark("instance.answered");
        match (answered, answer[0]) {
            (true, ACCEPTED) => Sent::Delivered,
            (true, QUITTING) => Sent::Quitting,
            _ => Sent::Nobody,
        }
    }

    /// Waits, at most `HANDED_WAIT`, for a quitting process to remove its socket (or to die with
    /// it); a socket still there then is taken over.
    fn wait_free(path: &Path) {
        let until = Instant::now() + HANDED_WAIT;
        while Instant::now() < until {
            match UnixStream::connect(path) {
                Err(e) if e.kind() == ErrorKind::NotFound => return,
                Err(e) if e.kind() == ErrorKind::ConnectionRefused => break,
                _ => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        std::fs::remove_file(path).ok();
    }
}