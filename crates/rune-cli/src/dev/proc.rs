//! Cross-platform child process management with process-group isolation and graceful teardown.

use std::io::{self, IsTerminal, Write};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::Duration;

const GRACEFUL_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_STEP: Duration = Duration::from_millis(25);

/// ANSI backstop to leave any alternate screen, restore cursor visibility, and reset attributes
/// if a child process had to be force-killed while holding terminal state.
const ANSI_TERMINAL_RESTORE: &str = "\x1b[?1049l\x1b[?25h\x1b[0m";

/// Owns a spawned child process in its own process group and guarantees it (and its process tree)
/// is stopped gracefully before escalating to a forced kill on drop or explicit teardown.
pub struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        isolate_process_group(cmd);
        let child = cmd.spawn()?;
        Ok(Self { child: Some(child) })
    }

    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.child.as_mut() {
            Some(c) => {
                let status = c.try_wait()?;
                if status.is_some() {
                    self.child = None;
                }
                Ok(status)
            }
            None => Ok(None),
        }
    }

    pub fn stop(&mut self) {
        self.stop_with_timeout(GRACEFUL_TIMEOUT);
    }

    pub fn stop_with_timeout(&mut self, grace: Duration) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        terminate_tree(&mut child, grace);
        child.wait().ok();
        restore_terminal_if_tty();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

fn isolate_process_group(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Put the child in its own console process group so GenerateConsoleCtrlEvent targets only
        // the child tree and never signals the parent `rune dev` process.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Place the child in a new process group whose pgid equals its pid so signals reach the
        // entire process tree (`dx`, `cargo`, and the launched app) without affecting `rune dev`.
        cmd.process_group(0);
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
    let mut waited = Duration::ZERO;
    loop {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        if waited >= timeout {
            return false;
        }
        let step = POLL_STEP.min(timeout.saturating_sub(waited));
        thread::sleep(step);
        waited = waited.saturating_add(step);
    }
}

fn restore_terminal_if_tty() {
    let mut stderr = io::stderr();
    if stderr.is_terminal() {
        stderr.write_all(ANSI_TERMINAL_RESTORE.as_bytes()).ok();
        stderr.flush().ok();
    }
}

#[cfg(windows)]
fn terminate_tree(child: &mut Child, grace: Duration) {
    let pid = child.id();
    if pid != 0 && !grace.is_zero() {
        // First request a graceful Ctrl+C for children that re-enabled Ctrl+C handling, then
        // follow with Ctrl+Break (which Windows always delivers to a CREATE_NEW_PROCESS_GROUP).
        send_console_ctrl_event(CTRL_C_EVENT, pid);
        let first_wait = grace.min(Duration::from_millis(500));
        if wait_for_exit(child, first_wait) {
            return;
        }
        send_console_ctrl_event(CTRL_BREAK_EVENT, pid);
        let remaining = grace.saturating_sub(first_wait);
        if !remaining.is_zero() && wait_for_exit(child, remaining) {
            return;
        }
    }

    Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok();
    child.kill().ok();
}

#[cfg(windows)]
const CTRL_C_EVENT: u32 = 0;

#[cfg(windows)]
const CTRL_BREAK_EVENT: u32 = 1;

#[cfg(windows)]
#[allow(unsafe_code)]
fn send_console_ctrl_event(event: u32, process_group_id: u32) -> bool {
    if process_group_id == 0 {
        return false;
    }
    unsafe extern "system" {
        fn GenerateConsoleCtrlEvent(dw_ctrl_event: u32, dw_process_group_id: u32) -> i32;
    }
    // SAFETY: `GenerateConsoleCtrlEvent` takes plain integer arguments with no pointers or
    // borrowed memory. `process_group_id` is non-zero and belongs to a child spawned with
    // `CREATE_NEW_PROCESS_GROUP`, so the signal targets only that group (or fails harmlessly
    // with 0 if no console is attached).
    unsafe { GenerateConsoleCtrlEvent(event, process_group_id) != 0 }
}

#[cfg(unix)]
fn terminate_tree(child: &mut Child, grace: Duration) {
    let pid = child.id();
    if !grace.is_zero() {
        send_unix_group_signal(pid, libc::SIGINT);
        let first_wait = grace
            .saturating_sub(Duration::from_millis(500))
            .max(grace / 2);
        if wait_for_exit(child, first_wait) {
            return;
        }
        send_unix_group_signal(pid, libc::SIGTERM);
        let remaining = grace.saturating_sub(first_wait);
        if !remaining.is_zero() && wait_for_exit(child, remaining) {
            return;
        }
    }

    send_unix_group_signal(pid, libc::SIGKILL);
    child.kill().ok();
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn send_unix_group_signal(pid: u32, signal: libc::c_int) {
    let Ok(pgid) = libc::pid_t::try_from(pid) else {
        return;
    };
    if pgid <= 0 {
        return;
    }
    // SAFETY: `pgid > 0`, so `-pgid` is strictly less than `-1` and targets only the child's
    // own process group created via `CommandExt::process_group(0)`.
    unsafe {
        let _ = libc::kill(-pgid, signal);
    }
}

#[cfg(not(any(windows, unix)))]
fn terminate_tree(child: &mut Child, _grace: Duration) {
    child.kill().ok();
}
