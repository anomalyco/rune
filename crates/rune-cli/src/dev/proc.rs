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
///
/// When spawned via [`Self::spawn_interactive`], also snapshots the host terminal mode (Windows
/// console input/output modes or Unix `termios` + controlling-terminal foreground process group)
/// before launch and unconditionally restores it when the child exits or is stopped.
pub struct ChildGuard {
    child: Option<Child>,
    terminal: Option<TerminalState>,
}

impl ChildGuard {
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        isolate_process_group(cmd);
        let child = cmd.spawn()?;
        Ok(Self {
            child: Some(child),
            terminal: None,
        })
    }

    /// Spawns a child that may own the interactive terminal (raw mode, alternate screen, key
    /// input). Snapshots terminal state beforehand, hands off the Unix foreground process group
    /// when this process owns the controlling TTY so the child does not stall on `SIGTTIN` /
    /// `SIGTTOU`, and restores everything on exit or teardown.
    pub fn spawn_interactive(cmd: &mut Command) -> io::Result<Self> {
        let terminal = TerminalState::capture();
        isolate_process_group(cmd);
        let child = cmd.spawn()?;
        terminal.hand_off_foreground(child.id());
        Ok(Self {
            child: Some(child),
            terminal: Some(terminal),
        })
    }

    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.child.as_mut() {
            Some(c) => {
                let status = c.try_wait()?;
                if let Some(exit) = status {
                    self.child = None;
                    if let Some(mut term) = self.terminal.take() {
                        term.restore();
                        if !exit.success() {
                            restore_terminal_if_tty();
                        }
                    }
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
        let mut stopped_live_child = false;
        if let Some(mut child) = self.child.take()
            && child.try_wait().ok().flatten().is_none()
        {
            terminate_tree(&mut child, grace);
            child.wait().ok();
            stopped_live_child = true;
        }
        let had_terminal = if let Some(mut term) = self.terminal.take() {
            term.restore();
            true
        } else {
            false
        };
        if stopped_live_child || had_terminal {
            restore_terminal_if_tty();
        }
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

#[cfg(windows)]
#[derive(Default)]
struct TerminalState {
    stdin_mode: Option<u32>,
    stdout_mode: Option<u32>,
}

#[cfg(windows)]
impl TerminalState {
    fn capture() -> Self {
        Self {
            stdin_mode: win_console::get_mode(win_console::STD_INPUT_HANDLE),
            stdout_mode: win_console::get_mode(win_console::STD_OUTPUT_HANDLE),
        }
    }

    fn hand_off_foreground(&self, _child_pid: u32) {}

    fn restore(&mut self) {
        if let Some(mode) = self.stdin_mode.take() {
            win_console::set_mode(win_console::STD_INPUT_HANDLE, mode);
        }
        if let Some(mode) = self.stdout_mode.take() {
            win_console::set_mode(win_console::STD_OUTPUT_HANDLE, mode);
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win_console {
    use core::ffi::c_void;

    type Handle = *mut c_void;

    pub const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6;
    pub const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    unsafe extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> Handle;
        fn GetConsoleMode(h_console_handle: Handle, lp_mode: *mut u32) -> i32;
        fn SetConsoleMode(h_console_handle: Handle, dw_mode: u32) -> i32;
    }

    pub fn get_mode(std_handle: u32) -> Option<u32> {
        // SAFETY: `GetStdHandle` takes a standard handle constant; we validate the returned
        // handle against null and `INVALID_HANDLE_VALUE` before passing it and a stack
        // pointer to `GetConsoleMode`.
        unsafe {
            let handle = GetStdHandle(std_handle);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) != 0 {
                Some(mode)
            } else {
                None
            }
        }
    }

    pub fn set_mode(std_handle: u32, mode: u32) {
        // SAFETY: `GetStdHandle` takes a standard handle constant; we validate the handle
        // before calling `SetConsoleMode` with a mode previously obtained from
        // `GetConsoleMode`.
        unsafe {
            let handle = GetStdHandle(std_handle);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return;
            }
            let _ = SetConsoleMode(handle, mode);
        }
    }
}

#[cfg(unix)]
#[derive(Default)]
struct TerminalState {
    termios: Option<libc::termios>,
    parent_pgid: Option<libc::pid_t>,
}

#[cfg(unix)]
#[allow(unsafe_code)]
impl TerminalState {
    fn capture() -> Self {
        // SAFETY: `isatty`, `tcgetattr`, `getpgrp`, and `tcgetpgrp` operate on `STDIN_FILENO`
        // and a stack-allocated `MaybeUninit<libc::termios>` initialized only when
        // `tcgetattr` returns 0.
        unsafe {
            if libc::isatty(libc::STDIN_FILENO) != 1 {
                return Self::default();
            }
            let mut slot = core::mem::MaybeUninit::<libc::termios>::uninit();
            let termios = if libc::tcgetattr(libc::STDIN_FILENO, slot.as_mut_ptr()) == 0 {
                Some(slot.assume_init())
            } else {
                None
            };
            let our_pgid = libc::getpgrp();
            let fg_pgid = libc::tcgetpgrp(libc::STDIN_FILENO);
            let parent_pgid = (our_pgid > 0 && fg_pgid == our_pgid).then_some(our_pgid);
            Self {
                termios,
                parent_pgid,
            }
        }
    }

    fn hand_off_foreground(&self, child_pid: u32) {
        if self.parent_pgid.is_none() {
            return;
        }
        let Ok(child_pgid) = libc::pid_t::try_from(child_pid) else {
            return;
        };
        if child_pgid <= 0 {
            return;
        }
        set_foreground_pgrp(libc::STDIN_FILENO, child_pgid);
        // Resume the child group in case it touched the controlling terminal before we
        // finished handing over the foreground process group and stopped on SIGTTIN/SIGTTOU.
        send_unix_group_signal(child_pid, libc::SIGCONT);
    }

    fn restore(&mut self) {
        if let Some(parent_pgid) = self.parent_pgid.take() {
            set_foreground_pgrp(libc::STDIN_FILENO, parent_pgid);
        }
        if let Some(termios) = self.termios.take() {
            // SAFETY: `termios` was populated by `tcgetattr` on `STDIN_FILENO`; temporarily
            // ignoring `SIGTTOU` prevents stopping if the foreground group changed.
            unsafe {
                let prev = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &termios);
                if prev != libc::SIG_ERR {
                    let _ = libc::signal(libc::SIGTTOU, prev);
                }
            }
        }
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn set_foreground_pgrp(fd: libc::c_int, pgid: libc::pid_t) {
    // SAFETY: Temporarily setting `SIGTTOU` to `SIG_IGN` prevents the kernel from stopping
    // this process when calling `tcsetpgrp` from a background process group, and the
    // previous signal disposition is restored immediately afterward.
    unsafe {
        let prev = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        let _ = libc::tcsetpgrp(fd, pgid);
        if prev != libc::SIG_ERR {
            let _ = libc::signal(libc::SIGTTOU, prev);
        }
    }
}

#[cfg(not(any(windows, unix)))]
#[derive(Default)]
struct TerminalState;

#[cfg(not(any(windows, unix)))]
impl TerminalState {
    fn capture() -> Self {
        Self
    }

    fn hand_off_foreground(&self, _child_pid: u32) {}

    fn restore(&mut self) {}
}
