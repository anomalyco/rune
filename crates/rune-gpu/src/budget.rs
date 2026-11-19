//! The longest one GPU submission may hold the GPU in Rune's debug builds and tests. Windows
//! resets a GPU whose submission runs 2 s (TDR), and an AMD GPU preempts nothing smaller than a
//! submission, so a test that asks for too much in one submission resets the GPU on a slow
//! adapter and passes on a fast one. `BUDGET` is far under the limit; the time is the adapter's
//! own, so a suite run on the slowest adapter it can land on is what finds every such test.
//!
//! On Windows the vendored wgpu-hal times each D3D12 submission (`dx12::Queue::watch_submissions`,
//! `vendor/wgpu-hal/src/dx12/submission_watch.rs`). Release builds time nothing. Elsewhere
//! nothing is timed yet.

use std::sync::Mutex;
use std::time::Duration;

/// The longest one submission may hold the GPU.
pub const BUDGET: Duration = Duration::from_millis(250);

/// Each overrun's queue and its line, until `overrun` takes it.
static OVERRUNS: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
static ABORT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Starts timing every D3D12 submission of this process, in debug builds. An overrun is
/// printed and kept for `overrun`; with `abort` the process then ends, for test binaries whose
/// devices have no test to fail (it fails the whole run, loudly).
pub fn watch(abort: bool) {
    if abort {
        ABORT.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(target_os = "windows")]
    {
        #[allow(unused_imports)]
        use without_patch::WatchSubmissions as _;
        wgpu::hal::dx12::Queue::watch_submissions(BUDGET, report);
    }
}

/// The overrun reported on `queue` since the last call, if any.
pub fn overrun(queue: &wgpu::Queue) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        use windows::core::Interface as _;
        // SAFETY: the queue's D3D12 interface, read for its address.
        let raw = unsafe { queue.as_hal::<wgpu::hal::api::Dx12>() }.map(|q| q.as_raw().as_raw() as usize)?;
        let mut overruns = OVERRUNS.lock().unwrap_or_else(|e| e.into_inner());
        let index = overruns.iter().position(|(q, _)| *q == raw)?;
        return Some(overruns.remove(index).1);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = queue;
        None
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn report(queue: usize, value: u64, held: Duration, running: bool) {
    let line = format!(
        "rune: one GPU submission {} the GPU {} us, past Rune's {} us budget (Windows resets a GPU at 2 s; fence value {value})",
        if running { "has held" } else { "held" },
        held.as_micros(),
        BUDGET.as_micros()
    );
    eprintln!("{line}");
    OVERRUNS.lock().unwrap_or_else(|e| e.into_inner()).push((queue, line));
    if ABORT.load(std::sync::atomic::Ordering::Relaxed) {
        std::process::abort();
    }
}

/// An app whose workspace doesn't patch wgpu-hal with Rune's copy gets wgpu-hal as published,
/// which times nothing: the call lands here (an inherent function wins over a trait's).
#[cfg(target_os = "windows")]
mod without_patch {
    #[allow(dead_code)]
    pub(super) trait WatchSubmissions {
        fn watch_submissions(_: std::time::Duration, _: fn(usize, u64, std::time::Duration, bool)) {}
    }
    impl WatchSubmissions for wgpu::hal::dx12::Queue {}
}
