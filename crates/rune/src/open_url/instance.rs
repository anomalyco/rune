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
//! A later launch also sends its activation token (`XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID`),
//! which the window the launch brings forward (or the next one the app opens) is activated with.
//!
//! The first Linux process also owns the app's id as a name on the session bus and serves
//! `org.freedesktop.Application` there (`dbus`), as a `DBusActivatable` desktop entry and
//! `gio launch`/`gio open` expect: `Activate` and `ActivateAction` reopen the app, and `Open`
//! sends its links to `App::on_open_url` and its `file://` URIs to `App::on_open_files`. The
//! `activation-token` (or `desktop-startup-id`) in their platform data is kept as a socket
//! launch's token is. The socket stays what decides which process is the first.
//!
//! The links of the launch that started the first process come from its arguments
//! ([`launch_arguments`]), so `App::on_open_url` gets those too (and on Linux
//! `App::on_open_files` its `file://` URIs).
//!
//! Which arguments are links: every `scheme://…` one, until the app names the schemes it
//! registered ([`url_schemes`](crate::url_schemes)); from then on only an argument on one of
//! them. Without that a flag's value (`--server http://localhost:4096`) is taken for a link, and
//! a later launch that carries one doesn't reopen the app.
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
/// Linux: the first process also owns `id` on the session bus (when it's a valid bus name and the
/// `dbus` feature is on) and serves `org.freedesktop.Application` at `/` plus `id` with its dots
/// as slashes, so a desktop entry with `DBusActivatable=true` (named `<id>.desktop`) reaches it.
/// `Activate` and `ActivateAction` reopen the app (`App::on_reopen`; the action's name isn't
/// passed on), and `Open` sends links to `App::on_open_url` and `file://` URIs to
/// `App::on_open_files`. The launch's activation token is passed on to the window it brings
/// forward, or else to the next window the app opens (`AppHandle::open`). The name is taken
/// without queueing: while an earlier process still holds it (it hasn't exited or called
/// `release_instance`, or it hangs and this one took its socket over), this process serves only
/// the socket. A `DBusActivatable` entry also needs the desktop file named `<id>.desktop` and a
/// session service file (`dbus-1/services/<id>.service`) that starts the app.
///
/// macOS and other platforms: always true. macOS already reaches the running app.
pub fn single_instance(id: &str) -> bool {
    // A relaunch (`Tx::relaunch`) claims once the process it restarts has let go.
    #[cfg(target_os = "windows")]
    {
        crate::relaunch::wait_for_previous_release();
        windows::claim(id)
    }
    #[cfg(not(any(target_os = "windows", target_arch = "wasm32")))]
    crate::relaunch::wait_for_previous();
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

/// The schemes the app named as its own (`url_schemes`), lowercased; none while it hasn't.
static SCHEMES: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// Names the URL schemes the app registered for itself (Windows, Linux): from now on only a
/// launch argument on one of them is a link for `App::on_open_url`, as macOS hands the app only
/// links on the schemes its bundle declares. Call it first thing in `main`, before
/// `single_instance`.
///
/// Until it is called every `scheme://…` argument is a link. An app with flags that take a URL
/// (`myapp --server http://localhost:4096`) then gets the flag's value in `App::on_open_url`,
/// and with `single_instance` a later launch with such a flag isn't a launch without links, so
/// it doesn't reopen the app.
///
/// It registers nothing with the system: an installer does that, or
/// AppHandle::register_url_scheme. Schemes compare without case. macOS and other platforms:
/// does nothing.
pub fn url_schemes<I, S>(schemes: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let schemes = schemes.into_iter().map(|s| s.as_ref().to_ascii_lowercase()).collect();
    *SCHEMES.lock().unwrap_or_else(|e| e.into_inner()) = Some(schemes);
}

/// Whether `arg` is a link the app opens: a link ([`is_link`]) and, when the app named its
/// schemes, on one of them.
pub(crate) fn opens(arg: &str, schemes: Option<&[String]>) -> bool {
    if !is_link(arg) {
        return false;
    }
    let Some(schemes) = schemes else { return true };
    arg.split_once("://").is_some_and(|(scheme, _)| schemes.iter().any(|s| s.eq_ignore_ascii_case(scheme)))
}

/// A launch's arguments as links for `App::on_open_url` and files for `App::on_open_files`, in
/// order. Linux: a local `file://` URI (what a desktop entry's `%U` passes for a file) is a file,
/// and no `file:` URI is a link. Every other argument on a scheme the app opens is a link.
/// A plain path is neither: the app reads it from its arguments itself.
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
pub(crate) fn sort_arguments(args: &[String]) -> (Vec<String>, Vec<std::path::PathBuf>) {
    let schemes = SCHEMES.lock().unwrap_or_else(|e| e.into_inner());
    sort_with(args, schemes.as_deref())
}

/// `sort_arguments` with the app's schemes (`url_schemes`) given.
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
pub(crate) fn sort_with(args: &[String], schemes: Option<&[String]>) -> (Vec<String>, Vec<std::path::PathBuf>) {
    #[cfg(target_os = "linux")]
    {
        let mut links = Vec::new();
        let mut files = Vec::new();
        for arg in args {
            if arg.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")) {
                files.extend(file_uri::file_path(arg));
            } else if opens(arg, schemes) {
                links.push(arg.clone());
            }
        }
        (links, files)
    }
    #[cfg(not(target_os = "linux"))]
    {
        (args.iter().filter(|a| opens(a, schemes)).cloned().collect(), Vec::new())
    }
}

/// The links and files among this process's launch arguments (Windows passes a link to the
/// process it starts for it as an argument, Linux a link or a file's URI).
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
pub(crate) fn launch_arguments() -> (Vec<String>, Vec<std::path::PathBuf>) {
    sort_arguments(&std::env::args().skip(1).collect::<Vec<_>>())
}

/// The loop's side of a later launch without links (`on_relaunch`), and the window it brings
/// forward (Windows).
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) use handoff::{exited, launches_waiting, on_relaunch, served};
#[cfg(target_os = "linux")]
pub(crate) use handoff::token_waiting;

/// A reopen is over (Linux): a token its launch left that no window took goes, so an unrelated
/// window opened later doesn't take it. A launch with links keeps its token for the window the
/// hook opens (`AppHandle::open` from `on_open_url`).
#[cfg(target_os = "linux")]
pub(crate) fn reopen_done() {
    handoff::take_token();
}

/// The token a window opens with: its own, or else (Linux) the one a launch handed to this
/// process (`single_instance`) left for the window it brings forward, used once.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn handed_token(own: Option<String>) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        own.or_else(handoff::take_token)
    }
    #[cfg(not(target_os = "linux"))]
    own
}
#[cfg(target_os = "windows")]
pub(crate) use windows::{raise_front, released};

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

    /// The argument that carries a launch's id: each launch sends one, the same on every ask, and
    /// the running process acts on an id once (`relaunched`). A launch it acted on whose answer
    /// never reached it asks again, and that ask is a no-op, so one click opens once whatever the
    /// pipe's flush reports. A process from before it takes it for an argument it doesn't know.
    pub(super) const ID_ARG: &str = "--rune-launch-id=";
    /// The argument that carries a Linux launch's activation token (`XDG_ACTIVATION_TOKEN`,
    /// `DESKTOP_STARTUP_ID`), so the running process can activate a window with it. A process
    /// from before it takes it for an argument it doesn't know.
    pub(super) const TOKEN_ARG: &str = "--rune-activation-token=";
    /// How long a handed-on activation token is kept for a window to use; compositors let them
    /// lapse soon after.
    const TOKEN_FOR: Duration = Duration::from_secs(20);
    /// How many acted-on launch ids the running process remembers.
    const IDS_KEPT: usize = 64;

    /// This launch's id: random, once per process.
    fn launch_id() -> &'static str {
        static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        ID.get_or_init(|| {
            use std::hash::{BuildHasher, Hash, Hasher};
            let half = |salt: u8| {
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                (salt, std::process::id(), crate::time::now()).hash(&mut h);
                h.finish()
            };
            format!("{:016x}{:016x}", half(0), half(1))
        })
    }

    /// This launch's message: `MAGIC`, the body's length (u32, little-endian), and the body (its
    /// id, then the arguments, NUL-separated).
    pub(super) fn message() -> Vec<u8> {
        let id = format!("{ID_ARG}{}", launch_id());
        let token = launch_token().map(|t| format!("{TOKEN_ARG}{t}"));
        let args = std::env::args().skip(1).filter(|a| !a.starts_with(ID_ARG) && !a.starts_with(TOKEN_ARG));
        let body = std::iter::once(id).chain(token).chain(args).collect::<Vec<_>>().join("\0").into_bytes();
        let mut message = MAGIC.to_vec();
        message.extend((body.len() as u32).to_le_bytes());
        message.extend(body);
        message
    }

    /// The activation token the desktop started this launch with (Linux).
    fn launch_token() -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"].into_iter().find_map(|k| crate::env::var(k).filter(|t| !t.is_empty()))
        }
        #[cfg(not(target_os = "linux"))]
        None
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
        launches: Vec<(Instant, Vec<String>, Option<String>)>,
        exited: bool,
    }

    static WAITING: Mutex<Waiting> = Mutex::new(Waiting { launches: Vec::new(), exited: false });
    /// The ids of the last launches acted on (`ID_ARG`), oldest first.
    static ACTED: Mutex<std::collections::VecDeque<String>> = Mutex::new(std::collections::VecDeque::new());
    /// Whether `WAITING` holds a launch: the loop's per-turn check costs a load.
    static PENDING: AtomicBool = AtomicBool::new(false);
    /// A launch older than this when the loop ends was served by a window that went later (an app
    /// that runs on without windows, then quits): it doesn't start the app again.
    const UNSERVED_FOR: Duration = Duration::from_secs(5);

    /// The activation token of the last launch handed to this process, and when it came: used
    /// once, and only within `TOKEN_FOR`.
    #[derive(Default)]
    pub(super) struct KeptToken(Option<(Instant, String)>);

    impl KeptToken {
        pub(super) fn keep(&mut self, token: Option<String>, now: Instant) {
            if let Some(token) = token.filter(|t| !t.is_empty()) { self.0 = Some((now, token)); }
        }
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        pub(super) fn waiting(&self, now: Instant) -> bool {
            self.0.as_ref().is_some_and(|(at, _)| now.saturating_duration_since(*at) < TOKEN_FOR)
        }
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        pub(super) fn take(&mut self, now: Instant) -> Option<String> {
            self.0.take().filter(|(at, _)| now.saturating_duration_since(*at) < TOKEN_FOR).map(|(_, t)| t)
        }
    }

    static TOKEN: Mutex<KeptToken> = Mutex::new(KeptToken(None));

    /// Keeps a handed-on launch's activation token for the window it brings forward.
    pub(super) fn keep_token(token: Option<String>) {
        TOKEN.lock().unwrap_or_else(|e| e.into_inner()).keep(token, crate::time::now());
    }

    /// Whether a handed-on launch's activation token waits for a window, unlapsed.
    #[cfg(target_os = "linux")]
    pub(crate) fn token_waiting() -> bool {
        TOKEN.lock().unwrap_or_else(|e| e.into_inner()).waiting(crate::time::now())
    }

    /// A handed-on launch's activation token, once: the next window opened activates with it.
    #[cfg(target_os = "linux")]
    pub(super) fn take_token() -> Option<String> {
        TOKEN.lock().unwrap_or_else(|e| e.into_inner()).take(crate::time::now())
    }

    /// Whether the loop has started (`on_relaunch`), whether or not it has ended since (`exited`).
    #[cfg_attr(not(all(target_os = "linux", feature = "dbus")), allow(dead_code))]
    pub(super) fn loop_started() -> bool {
        let started = RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        started || WAITING.lock().unwrap_or_else(|e| e.into_inner()).exited
    }

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
        for (at, args, token) in launches {
            if at.elapsed() < UNSERVED_FOR {
                crate::relaunch::spawn_with(args, token);
            }
        }
    }

    /// The loop runs: a launch without links calls `relaunch` from now on (`WakeEvent::Reopen`).
    pub(crate) fn on_relaunch(relaunch: impl Fn() -> bool + Send + 'static) {
        *RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(relaunch));
    }

    /// What a later launch hands this process: its arguments as received (what starts the app
    /// again with it, if it must; for `Open`, its URIs), its links and files, and its activation
    /// token (`XDG_ACTIVATION_TOKEN` for a process it starts).
    pub(super) struct Launch {
        pub(super) args: Vec<String>,
        pub(super) links: Vec<String>,
        pub(super) files: Vec<std::path::PathBuf>,
        pub(super) token: Option<String>,
    }

    /// A later launch's arguments: its links to `App::on_open_url`, or, without links, a reopen.
    /// One that comes while the app quits, or that the loop hadn't taken when it ended, starts the
    /// app again with them once this process has exited, as `Tx::relaunch` does: the windows it
    /// would reach are closing, and the launch would be lost. A launch whose id was acted on
    /// already (it asked again because its answer didn't reach it) is skipped: false.
    pub(super) fn relaunched(args: Vec<String>) -> bool {
        let (ids, args): (Vec<String>, Vec<String>) = args.into_iter().partition(|a| a.starts_with(ID_ARG));
        if let Some(id) = ids.into_iter().next() {
            let mut acted = ACTED.lock().unwrap_or_else(|e| e.into_inner());
            if acted.contains(&id) {
                return false;
            }
            acted.push_back(id);
            if acted.len() > IDS_KEPT {
                acted.pop_front();
            }
        }
        deliver(launch(args));
        true
    }

    /// A socket launch's arguments (its id taken out) as a `Launch`: the token argument becomes
    /// its token and leaves the arguments, so a relaunch doesn't pass it on.
    pub(super) fn launch(args: Vec<String>) -> Launch {
        let (tokens, args): (Vec<String>, Vec<String>) = args.into_iter().partition(|a| a.starts_with(TOKEN_ARG));
        let token = tokens.into_iter().next().map(|t| t[TOKEN_ARG.len()..].to_owned()).filter(|t| !t.is_empty());
        let (links, files) = super::sort_arguments(&args);
        Launch { args, links, files, token }
    }

    /// Whether a launch reopens the app: it brings neither links nor files for the hooks.
    pub(super) fn reopens(links: &[String], files: &[std::path::PathBuf]) -> bool {
        links.is_empty() && files.is_empty()
    }

    /// Hands a launch to the loop: its links and files to their hooks, or, with neither, a reopen.
    /// While the app quits, or once the loop has ended, it starts the app again instead.
    pub(super) fn deliver(launch: Launch) {
        let Launch { args, links, files, token } = launch;
        if crate::quit::quitting() {
            crate::relaunch::spawn_with(args, token);
            return;
        }
        {
            let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
            if waiting.exited {
                drop(waiting);
                crate::relaunch::spawn_with(args, token);
                return;
            }
            waiting.launches.push((crate::time::now(), args, token.clone()));
            PENDING.store(true, Ordering::Release);
        }
        keep_token(token);
        if reopens(&links, &files) {
            let sent = RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|relaunch| relaunch());
            if !sent {
                exited();
            }
            return;
        }
        for link in links {
            super::super::incoming::receive(link);
        }
        if !files.is_empty() {
            #[cfg(target_os = "linux")]
            super::super::incoming::receive_files(files);
        }
    }
}

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod dbus;

#[cfg(test)]
mod tests;

#[cfg(all(test, target_os = "linux"))]
mod handoff_tests;

#[cfg(target_os = "linux")]
mod file_uri;
