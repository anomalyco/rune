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
mod handoff;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(all(feature="deferred-quit",target_os="windows"))]
pub(crate) use windows::ui_claim;
#[cfg(all(feature="deferred-quit",windows))]
pub(crate) use windows::announce as announce_exit;
#[cfg(all(feature="deferred-quit",target_os="linux"))]
pub(crate) use linux::ui_claim;

#[cfg(target_os = "linux")]
mod linux;
