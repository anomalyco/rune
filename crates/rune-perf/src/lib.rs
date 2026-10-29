//! Bounded local diagnostics. Apps explicitly choose a directory and install their hooks.
//! No network, global allocator installation, redraws, or per-frame formatting.
#![cfg_attr(not(test), deny(unsafe_code))]
#![cfg_attr(test, allow(clippy::disallowed_methods))]

mod incident;
mod recorder;
mod stats;
mod store;

pub use recorder::Recorder;

use std::{cell::Cell, path::PathBuf, sync::mpsc::SyncSender};

pub(crate) const QUEUE: usize = 256;
pub(crate) const RECENT: usize = 600;
pub(crate) const WINDOW_MS: u64 = 60_000;
pub(crate) const SLOW_MS: f64 = 1000.0 / 60.0;
pub(crate) const LARGE_BYTES: u64 = 8 << 20;

/// One process/launch per directory. Use separate directories for dev and captures.
#[derive(Clone, Debug)]
pub struct Config {
    directory: PathBuf,
    app: String,
    build: String,
    incidents: Option<SyncSender<PathBuf>>,
}

impl Config {
    pub fn new(directory: impl Into<PathBuf>, app: impl Into<String>) -> Self {
        Self { directory: directory.into(), app: app.into(), build: String::new(), incidents: None }
    }

    /// Build identity written into snapshots and incidents, such as a version or commit.
    pub fn build(mut self, build: impl Into<String>) -> Self {
        self.build = build.into();
        self
    }

    /// Bounded notification queue for incident paths. A full/disconnected receiver loses a
    /// notification, never the incident file. Consumers may route the path into their own
    /// task system.
    pub fn incidents(mut self, tx: SyncSender<PathBuf>) -> Self {
        self.incidents = Some(tx);
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    Drawn,
    Unchanged,
    #[default]
    Skipped,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self { Self::Drawn => "drawn", Self::Unchanged => "unchanged", Self::Skipped => "skipped" }
    }
}

/// Fixed-size report; an unset measurement is unavailable, never an invented zero.
/// Times describe work, not gaps between frames. The caller owns window/source IDs.
/// Each setter takes a value or an `Option`, so a missing measurement passes straight through.
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    window: u64,
    seq: u64,
    outcome: Outcome,
    update_ms: Option<f64>,
    render_ms: Option<f64>,
    input_to_frame_ms: Option<f64>,
    gpu_ms: Option<f64>,
    allocated_bytes: Option<u64>,
    source: Option<u64>,
}

impl Frame {
    pub fn new(window: u64, seq: u64, outcome: Outcome) -> Self {
        Self { window, seq, outcome, ..Self::default() }
    }

    pub fn update_ms(mut self, ms: impl Into<Option<f64>>) -> Self {
        self.update_ms = ms.into();
        self
    }

    /// Compositor (render and encode) wall time.
    pub fn render_ms(mut self, ms: impl Into<Option<f64>>) -> Self {
        self.render_ms = ms.into();
        self
    }

    /// Input receipt to the encoding boundary, NOT input-to-screen latency.
    pub fn input_to_frame_ms(mut self, ms: impl Into<Option<f64>>) -> Self {
        self.input_to_frame_ms = ms.into();
        self
    }

    /// Resolved GPU timestamp duration, only when the backend supplies one.
    pub fn gpu_ms(mut self, ms: impl Into<Option<f64>>) -> Self {
        self.gpu_ms = ms.into();
        self
    }

    /// Successful Rust allocation requests on the reporting thread since its last report,
    /// usually [`take_allocated_bytes`].
    pub fn allocated_bytes(mut self, bytes: impl Into<Option<u64>>) -> Self {
        self.allocated_bytes = bytes.into();
        self
    }

    /// Stable numeric source identifier; never user text or content.
    pub fn source(mut self, id: impl Into<Option<u64>>) -> Self {
        self.source = id.into();
        self
    }

    /// Update plus compositor time, when both were measured.
    pub fn work_ms(self) -> Option<f64> { self.update_ms.zip(self.render_ms).map(|(u, r)| u + r).filter(|v| v.is_finite()) }

    fn sanitize(mut self) -> Self {
        for t in [&mut self.update_ms, &mut self.render_ms, &mut self.input_to_frame_ms, &mut self.gpu_ms] {
            *t = t.filter(|v| v.is_finite() && *v >= 0.0);
        }
        self
    }
}

thread_local! { static ALLOCATED_AT: Cell<Option<u64>> = const { Cell::new(None) }; }

/// Bytes this thread asked the allocator for since its previous call, for
/// [`Frame::allocated_bytes`]. Needs `procwatch::alloc::Counting` as the global allocator.
/// `None` when it isn't installed, and on a thread's first call, which only sets the baseline.
/// Never allocates.
pub fn take_allocated_bytes() -> Option<u64> {
    let now = procwatch::alloc::thread_allocated_bytes()?;
    let last = ALLOCATED_AT.try_with(|at| at.replace(Some(now))).ok()??;
    Some(now.wrapping_sub(last))
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample { at_ms: u64, frame: Frame }

impl Sample {
    fn anomaly(self) -> bool {
        self.frame.work_ms().is_some_and(|ms| ms >= SLOW_MS)
            || self.frame.allocated_bytes.is_some_and(|n| n >= LARGE_BYTES)
    }
    fn json(self) -> serde_json::Value {
        let f = self.frame;
        serde_json::json!({"at_ms": self.at_ms, "window": f.window, "source": f.source,
            "seq": f.seq, "outcome": f.outcome.name(), "update_ms": f.update_ms,
            "render_ms": f.render_ms, "work_ms": f.work_ms(),
            "input_to_frame_ms": f.input_to_frame_ms, "gpu_ms": f.gpu_ms,
            "ui_allocated_bytes": f.allocated_bytes})
    }
}

#[cfg(test)]
mod tests;
