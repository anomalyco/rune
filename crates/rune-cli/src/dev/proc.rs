//! Cross-platform child process management with clean process-tree teardown.

use std::io;
use std::process::{Child, Command, ExitStatus};

/// Owns a spawned child process and guarantees it (and its process tree on Windows) is stopped
/// when dropped or explicitly terminated.
pub struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        let child = cmd.spawn()?;
        Ok(Self { child: Some(child) })
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
        let Some(mut child) = self.child.take() else {
            return;
        };
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        terminate_tree(&mut child);
        child.wait().ok();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

fn terminate_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let pid = child.id();
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok();
        child.kill().ok();
    }
    #[cfg(not(windows))]
    {
        child.kill().ok();
    }
}
