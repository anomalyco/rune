//! Surface configures, and the submissions that must not land inside one.
//!
//! wgpu waits for a device's last submission before it reconfigures a surface, and then
//! requires the device's queue to be empty. A submission from another thread that lands between
//! the wait and the check makes it refuse the configuration (`GpuWaitTimeout`), and the window
//! is left with a surface that has no swap chain. So every `Surface::configure` runs inside
//! [`configuring`], and a thread other than the one that configures surfaces submits through
//! [`try_submit`], which does nothing while a surface is being configured.

use std::sync::{Mutex, TryLockError};

/// Keeps surface configures and other threads' submissions apart.
pub struct Gate {
    held: Mutex<()>,
}

impl Gate {
    pub const fn new() -> Self {
        Self { held: Mutex::new(()) }
    }

    /// Runs `configure` (a `Surface::configure`) once no thread is inside [`Self::try_submit`],
    /// and keeps them out until it returns.
    pub fn configuring<R>(&self, configure: impl FnOnce() -> R) -> R {
        let _held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        configure()
    }

    /// Runs `submit` (a `Queue::submit`) unless a surface is being configured right now; None
    /// then, and the caller submits later. It never waits: wgpu also runs completion callbacks
    /// on the configuring thread, inside `Surface::configure`, where a wait would never end.
    pub fn try_submit<R>(&self, submit: impl FnOnce() -> R) -> Option<R> {
        let _held = match self.held.try_lock() {
            Ok(held) => held,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        Some(submit())
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

static GATE: Gate = Gate::new();

/// The process's gate: Rune's surfaces and its threads' submissions all go through it.
pub fn gate() -> &'static Gate {
    &GATE
}

/// [`Gate::configuring`] on the process's gate.
pub fn configuring<R>(configure: impl FnOnce() -> R) -> R {
    GATE.configuring(configure)
}

#[cfg(test)]
#[path = "configure_tests.rs"]
mod tests;
