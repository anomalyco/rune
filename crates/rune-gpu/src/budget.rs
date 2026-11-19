//! The longest one GPU submission may hold the GPU in Rune's debug builds and tests. Windows
//! resets a GPU whose submission runs 2 s (TDR), and an AMD GPU preempts nothing smaller than a
//! submission, so a test that asks for too much in one submission resets the GPU on a slow
//! adapter and passes on a fast one. `BUDGET` is far under the limit; the time is the GPU's own
//! (timestamps around each submission), so a suite run on the slowest adapter it can land on is
//! what finds every such test.
//!
//! On Windows the vendored wgpu-hal times each D3D12 submission on the GPU
//! (`dx12::Queue::watch_submissions`, `vendor/wgpu-hal/src/dx12/submission_watch.rs`) and reports
//! an overrun from a thread of its own. Here it is printed and kept for the queue's owner, which
//! fails on its own thread: a `Strict` guard when its GPU drops (Rune's tests, `test-strict`
//! builds), or a test's own check (`overrun`). Nothing aborts, and no callback panics. Release
//! builds compile none of it. Elsewhere nothing is timed yet.

use std::time::Duration;

/// The longest one submission may hold the GPU.
pub const BUDGET: Duration = Duration::from_millis(250);

/// Each overrun's queue serial and its line, until its owner takes it.
#[cfg(all(target_os = "windows", debug_assertions))]
static OVERRUNS: std::sync::Mutex<Vec<(u64, String)>> = std::sync::Mutex::new(Vec::new());

/// Starts timing every D3D12 submission of this process, in debug builds.
pub fn watch() {
    #[cfg(all(target_os = "windows", debug_assertions))]
    {
        #[allow(unused_imports)]
        use without_patch::WatchSubmissions as _;
        wgpu::hal::dx12::Queue::watch_submissions(BUDGET, report);
    }
}

/// The overrun reported on `queue` since the last call, if any.
pub fn overrun(queue: &wgpu::Queue) -> Option<String> {
    #[cfg(all(target_os = "windows", debug_assertions))]
    {
        let serial = serial(queue)?;
        let mut overruns = OVERRUNS.lock().unwrap_or_else(|e| e.into_inner());
        let index = overruns.iter().position(|(q, _)| *q == serial)?;
        return Some(overruns.remove(index).1);
    }
    #[cfg(not(all(target_os = "windows", debug_assertions)))]
    {
        let _ = queue;
        None
    }
}

/// Forgets every overrun of `queue`: its owner is done with it.
pub fn forget(queue: &wgpu::Queue) {
    #[cfg(all(target_os = "windows", debug_assertions))]
    if let Some(serial) = serial(queue) {
        OVERRUNS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(q, _)| *q != serial);
    }
    #[cfg(not(all(target_os = "windows", debug_assertions)))]
    let _ = queue;
}

/// Held with a GPU's queue: when the GPU drops, a `strict` guard fails its owner (a test, on the
/// test's own thread) with the first overrun its queue had, unless the thread is already
/// panicking. A guard that isn't strict only forgets them.
/// Release builds keep nothing in it.
pub struct Strict {
    #[cfg(debug_assertions)]
    queue: wgpu::Queue,
    #[cfg(debug_assertions)]
    strict: bool,
}

impl Strict {
    pub fn new(queue: &wgpu::Queue, strict: bool) -> Strict {
        #[cfg(debug_assertions)]
        return Strict { queue: queue.clone(), strict };
        #[cfg(not(debug_assertions))]
        {
            let _ = (queue, strict);
            Strict {}
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for Strict {
    fn drop(&mut self) {
        let first = overrun(&self.queue);
        forget(&self.queue);
        if let Some(line) = first.filter(|_| self.strict && !std::thread::panicking()) {
            panic!("{line}");
        }
    }
}

/// The queue's number in the hal's reports; None without Rune's wgpu-hal or off D3D12.
#[cfg(all(target_os = "windows", debug_assertions))]
fn serial(queue: &wgpu::Queue) -> Option<u64> {
    #[allow(unused_imports)]
    use without_patch::WatchSerial as _;
    // SAFETY: the queue's hal object, asked for its number.
    unsafe { queue.as_hal::<wgpu::hal::api::Dx12>() }.map(|q| q.submission_watch_serial()).filter(|serial| *serial != 0)
}

#[cfg(all(target_os = "windows", debug_assertions))]
fn report(queue: u64, value: u64, held: Duration, running: bool) {
    let line = format!(
        "rune: one GPU submission {} the GPU {} us, past Rune's {} us budget (Windows resets a GPU at 2 s; fence value {value})",
        if running { "has held" } else { "held" },
        held.as_micros(),
        BUDGET.as_micros()
    );
    eprintln!("{line}");
    OVERRUNS.lock().unwrap_or_else(|e| e.into_inner()).push((queue, line));
}

/// An app whose workspace doesn't patch wgpu-hal with Rune's copy gets wgpu-hal as published,
/// which times nothing: the calls land here (an inherent function wins over a trait's).
#[cfg(all(target_os = "windows", debug_assertions))]
mod without_patch {
    #[allow(dead_code)]
    pub(super) trait WatchSubmissions {
        fn watch_submissions(_: std::time::Duration, _: fn(u64, u64, std::time::Duration, bool)) {}
    }
    impl WatchSubmissions for wgpu::hal::dx12::Queue {}

    #[allow(dead_code)]
    pub(super) trait WatchSerial {
        fn submission_watch_serial(&self) -> u64 {
            0
        }
    }
    impl WatchSerial for wgpu::hal::dx12::Queue {}
}
