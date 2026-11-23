use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    /// Parked, waiting for a submission.
    Idle,
    /// A submission arrived while parked; the worker has not entered its poll loop yet.
    Pending,
    /// The worker is inside its poll loop; `rewake` marks a submission that arrived mid-poll.
    Polling { rewake: bool },
    /// Shutdown requested or the worker exited; never poll again.
    Stopped,
}

struct Shared {
    state: Mutex<Signal>,
    condvar: Condvar,
}

/// Coalesced wakeup handle for [`CompletionWorker`]. Holding a clone (for example inside a GPU
/// completion callback) never prevents [`CompletionWorker::stop`] from joining the thread.
#[derive(Clone)]
pub(crate) struct CompletionWake {
    shared: Arc<Shared>,
}

impl CompletionWake {
    pub(crate) fn wake(&self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        match *state {
            Signal::Idle => {
                *state = Signal::Pending;
                self.shared.condvar.notify_one();
            }
            Signal::Polling { ref mut rewake } => *rewake = true,
            Signal::Pending | Signal::Stopped => {}
        }
    }
}

/// Work a completion callback has to submit, parked until it may be. A callback runs on
/// whichever thread polls the device, usually the completion thread, and a submission from
/// there that lands while the window's thread configures a surface makes wgpu refuse the
/// configuration (`rune_gpu::configure`). So nothing is submitted from a callback directly: it
/// parks the work here and flushes, and what a configure kept back the completion thread
/// flushes once the configure is done.
pub(crate) struct Parked<T> {
    waiting: Mutex<Vec<T>>,
}

impl<T> Default for Parked<T> {
    fn default() -> Self {
        Self { waiting: Mutex::new(Vec::new()) }
    }
}

impl<T> Parked<T> {
    pub(crate) fn park(&self, work: T) {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).push(work);
    }

    /// Hands everything parked to `submit`, in the order it was parked, unless `gate` says a
    /// surface is being configured. True when nothing is left parked.
    pub(crate) fn flush(&self, gate: &rune_gpu::configure::Gate, submit: impl FnOnce(Vec<T>)) -> bool {
        if self.waiting.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            return true;
        }
        gate.try_submit(|| {
            let work = std::mem::take(&mut *self.waiting.lock().unwrap_or_else(|e| e.into_inner()));
            if !work.is_empty() {
                submit(work);
            }
        })
        .is_some()
    }
}

/// A frame's timestamp resolve and the queue it goes to, held weakly: a renderer that is gone
/// resolves nothing.
pub(crate) type Resolve = (std::sync::Weak<wgpu::Queue>, wgpu::CommandBuffer);

impl Parked<Resolve> {
    /// Submits the parked resolves unless a surface is being configured; true when none is left.
    pub(crate) fn submit(&self) -> bool {
        self.flush(rune_gpu::configure::gate(), |resolves| {
            for (queue, resolve) in resolves {
                if let Some(queue) = queue.upgrade() {
                    queue.submit([resolve]);
                }
            }
        })
    }
}

/// Marks the worker `Stopped` on every thread exit path (normal stop, lost device, or panic
/// unwind) so no waiter can hang.
struct ExitGuard(Arc<Shared>);

impl Drop for ExitGuard {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = Signal::Stopped;
        self.0.condvar.notify_all();
    }
}

/// Background thread that polls a `wgpu::Device` after video submissions so completion callbacks
/// fire even after the window goes idle, and stops cleanly before the device is torn down.
pub(crate) struct CompletionWorker {
    wake: CompletionWake,
    thread: Option<JoinHandle<()>>,
}

impl CompletionWorker {
    /// Polls `device` after each wakeup, and submits the resolves a surface configure kept
    /// back (`Parked`) once it may: while one is still parked the device counts as busy, so the
    /// worker comes back for it instead of going idle.
    pub(crate) fn spawn(device: wgpu::Device, resolves: Arc<Parked<Resolve>>) -> Self {
        Self::spawn_with(move || {
            let submitted = resolves.submit();
            let status = rune_gpu::health::poll(&device, wgpu::PollType::Poll)?;
            Ok(if submitted { status } else { wgpu::PollStatus::Poll })
        })
    }

    pub(crate) fn spawn_with(
        mut poll: impl FnMut() -> Result<wgpu::PollStatus, rune_gpu::health::PollFailed> + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(Signal::Idle),
            condvar: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("rune-video-completion".into())
            .spawn(move || {
                // Declared first so `poll` (and the `wgpu::Device` it owns) drops before
                // `ExitGuard::drop` transitions to `Stopped` and notifies waiters.
                let _exit = ExitGuard(worker_shared.clone());
                run_loop(&worker_shared, &mut poll);
                drop(poll);
            })
            .expect("video completion thread");
        Self {
            wake: CompletionWake { shared },
            thread: Some(thread),
        }
    }

    pub(crate) fn wake(&self) {
        self.wake.wake();
    }

    pub(crate) fn waker(&self) -> CompletionWake {
        self.wake.clone()
    }

    /// Waits until the worker has taken a pending wakeup and entered its polling loop (or
    /// stopped).
    #[cfg(test)]
    pub(crate) fn wait_polling(&self) {
        let mut state = self.wake.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        while *state == Signal::Pending {
            state = self.wake.shared.condvar.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Signals the worker to stop and joins it so no poll can race device teardown.
    /// Idempotent, unwind-safe, and safe if dropped from a callback running on the worker thread
    /// itself (in which case it signals stop without self-joining).
    pub(crate) fn stop(&mut self) {
        {
            let mut state = self.wake.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            *state = Signal::Stopped;
            self.wake.shared.condvar.notify_all();
        }
        if let Some(handle) = self.thread.take()
            && handle.thread().id() != std::thread::current().id()
        {
            let _ = handle.join();
        }
    }
}

impl Drop for CompletionWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_loop(
    shared: &Shared,
    poll: &mut impl FnMut() -> Result<wgpu::PollStatus, rune_gpu::health::PollFailed>,
) {
    // Nonblocking poll releases wgpu's snatch lock before sleeping. A fence wait here
    // would keep that read lock and serialize queue.present's write lock behind GPU work.
    // Wakeups are coalesced; the final frame still completes after the window goes idle.
    // A lost device's work never finishes: the worker stops rather than poll it forever.
    loop {
        {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while *state == Signal::Idle {
                state = shared.condvar.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            match *state {
                Signal::Stopped => return,
                Signal::Pending => {
                    *state = Signal::Polling { rewake: false };
                    shared.condvar.notify_all();
                }
                Signal::Polling { .. } | Signal::Idle => unreachable!(),
            }
        }
        loop {
            match poll() {
                Ok(wgpu::PollStatus::QueueEmpty) => {
                    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    match *state {
                        Signal::Stopped => return,
                        Signal::Polling { rewake: true } => *state = Signal::Polling { rewake: false },
                        Signal::Polling { rewake: false } => {
                            *state = Signal::Idle;
                            break;
                        }
                        Signal::Idle | Signal::Pending => unreachable!(),
                    }
                }
                Ok(_) => {
                    let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    if *state == Signal::Stopped {
                        return;
                    }
                    let (mut state, _) = shared
                        .condvar
                        .wait_timeout_while(state, Duration::from_millis(1), |s| *s != Signal::Stopped)
                        .unwrap_or_else(|e| e.into_inner());
                    if *state == Signal::Stopped {
                        return;
                    }
                    if let Signal::Polling { rewake } = &mut *state {
                        *rewake = false;
                    }
                }
                Err(_) => return,
            }
        }
    }
}
