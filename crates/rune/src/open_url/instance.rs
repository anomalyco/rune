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
            waiting.launches.push((crate::time::now(), args));
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
mod windows;

#[cfg(target_os = "linux")]
mod linux;