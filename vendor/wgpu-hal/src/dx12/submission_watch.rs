//! Rune: how long each submission holds the GPU, in debug builds, reported to a hook when one
//! passes a budget. Windows resets a GPU whose one submission runs 2 s (TDR), and some GPUs
//! (AMD's) preempt nothing smaller than a submission, so a test or a frame that asks for too
//! much in one submission hangs the whole GPU on a slow adapter and passes on a fast one.
//!
//! A queue runs its submissions in order, so a submission can only have been running since it
//! was submitted and the one before it was done: that span, measured from the fence's event, is
//! what's compared with the budget. Other processes' work on the same GPU can lengthen it.
//! Nothing runs until `watch_submissions` installs a hook, and release builds time nothing: there,
//! `watch_submissions` and `Watch::submitted` are empty.

#![cfg_attr(not(debug_assertions), allow(dead_code, unused_imports))]

use std::{
    sync::{mpsc, OnceLock},
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use windows::Win32::{Foundation, Graphics::Direct3D12, System::Threading};

/// What a hook hears of a submission that held the GPU past the budget: the queue it ran on
/// (`Queue::as_raw`'s interface pointer), its fence value, how long it held the GPU (until it
/// was done, or until now while it still runs), and whether it was still running.
pub type Report = fn(queue: usize, value: u64, held: Duration, running: bool);

static HOOK: OnceLock<(Duration, Report)> = OnceLock::new();

impl super::Queue {
    /// From now on, in debug builds, every D3D12 queue's submissions are timed and each one that
    /// holds the GPU past `budget` goes to `report`, from a thread of the queue's own: once
    /// while it still runs, or once when it's done. The first call sets the hook; later calls do
    /// nothing.
    pub fn watch_submissions(budget: Duration, report: Report) {
        if cfg!(debug_assertions) {
            let _ = HOOK.set((budget, report));
        }
    }
}

/// The fence, sent to the queue's watching thread: D3D12 fences are free-threaded.
struct SendFence(Direct3D12::ID3D12Fence);
unsafe impl Send for SendFence {}

#[derive(Default)]
pub(super) struct Watch {
    sender: Mutex<Option<mpsc::Sender<(u64, Instant)>>>,
}

impl Watch {
    #[inline]
    pub(super) fn submitted(&self, queue: &Direct3D12::ID3D12CommandQueue, fence: &Direct3D12::ID3D12Fence, value: u64) {
        #[cfg(debug_assertions)]
        self.time(queue, fence, value);
        #[cfg(not(debug_assertions))]
        let _ = (self, queue, fence, value);
    }

    #[cfg(debug_assertions)]
    fn time(&self, queue: &Direct3D12::ID3D12CommandQueue, fence: &Direct3D12::ID3D12Fence, value: u64) {
        let Some(&(budget, report)) = HOOK.get() else { return };
        let submitted = Instant::now();
        let mut sender = self.sender.lock();
        if sender.is_none() {
            let (send, receive) = mpsc::channel();
            let (fence, queue) = (SendFence(fence.clone()), windows::core::Interface::as_raw(queue) as usize);
            let spawned = std::thread::Builder::new()
                .name("wgpu-dx12-submission-watch".into())
                .spawn(move || watch(fence, queue, budget, report, receive));
            if spawned.is_err() {
                return;
            }
            *sender = Some(send);
        }
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send((value, submitted));
        }
    }
}

fn watch(fence: SendFence, queue: usize, budget: Duration, report: Report, submissions: mpsc::Receiver<(u64, Instant)>) {
    let Ok(event) = super::Event::create(false, false) else { return };
    let mut free_since = Instant::now();
    while let Ok((value, submitted)) = submissions.recv() {
        let start = submitted.max(free_since);
        let mut told = false;
        loop {
            let completed = unsafe { fence.0.GetCompletedValue() };
            // A removed device: `Device::unless_removed` reports it, and nothing more runs.
            if completed == u64::MAX {
                return;
            }
            if completed >= value {
                break;
            }
            if unsafe { fence.0.SetEventOnCompletion(value, event.0) }.is_err() {
                return;
            }
            let wait = if told { Duration::from_secs(1) } else { budget.saturating_sub(start.elapsed()) };
            let waited = unsafe { Threading::WaitForSingleObject(event.0, wait.as_millis().clamp(1, u32::MAX as u128) as u32) };
            if waited == Foundation::WAIT_TIMEOUT && !told && start.elapsed() >= budget {
                told = true;
                report(queue, value, start.elapsed(), true);
            }
        }
        let done = Instant::now();
        if !told && done - start > budget {
            report(queue, value, done - start, false);
        }
        free_since = done;
    }
}
