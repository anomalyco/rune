//! Work off the UI thread, the same code on every target: natively a piece runs on one of a few
//! shared threads (`pool`), and a service ([`serve`]) on a thread of its own; on the web, which
//! has no threads, the main thread runs it from a task after the current event, so it never runs
//! inside a frame.
//!
//! On the web the work can wait without blocking the page: after [`start_runners`] (called from
//! a `#[wasm_bindgen(jspi)]` export), it runs on a few JS Promise Integration fibers, and
//! [`wait`] suspends the fiber on a promise (a `fetch`, a timer) while frames and input carry on.
//! So blocking code written for a thread (a request, a [`sleep`]) runs unchanged and costs the
//! page nothing while it waits.

use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::sync::mpsc::Receiver;
use std::time::Duration;

/// How long a thread waits with nothing to do before it tidies up ([`on_idle`]).
pub const IDLE: Duration = Duration::from_secs(10);

/// Minimum gap between two idle tidies on one thread: bursts closer together than this share one
/// tidy once the thread goes quiet, and periodic traffic slower than [`IDLE`] tidies at most once
/// per [`IDLE_MIN_GAP`] instead of after every message.
pub const IDLE_MIN_GAP: Duration = Duration::from_secs(180);

static TIDY: OnceLock<fn()> = OnceLock::new();

/// Sets what a thread that has waited [`IDLE`] with nothing to do runs, once per idle spell (at
/// least [`IDLE_MIN_GAP`] after its previous tidy), before it waits again with no timer: an app's
/// allocator giving back the pages that thread holds free (an allocator's per-thread heap gives
/// back only when its own thread asks). Rune's task and service threads call it when set; an app's
/// own long-lived threads call [`idle`]. Threads in an app that never sets a hook arm no tidy
/// timer. The first call wins.
pub fn on_idle(tidy: fn()) {
    let _ = TIDY.set(tidy);
}

/// Runs what [`on_idle`] set, on this thread: for a thread of the app's own that has waited
/// [`IDLE`] with nothing to do.
pub fn idle() {
    if let Some(tidy) = TIDY.get() {
        tidy();
    }
}

/// Per-thread idle-tidy schedule (`pool` and [`serve`]): explicit two-state machine so a thread
/// never arms a timer before work or without a hook, and never purges after every close burst.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
pub(crate) enum Tidy {
    /// Waiting with no tidy timer; `last` is when this thread last ran the hook.
    Parked { last: Option<crate::time::Instant> },
    /// Work ran while a hook was set; run the hook once nothing arrives before `due`.
    Armed { last: Option<crate::time::Instant>, due: crate::time::Instant },
}

#[cfg(not(target_arch = "wasm32"))]
impl Tidy {
    pub(crate) const fn new() -> Self {
        Self::Parked { last: None }
    }

    pub(crate) fn on_work(self, now: crate::time::Instant) -> Self {
        let last = match self {
            Self::Parked { last } | Self::Armed { last, .. } => last,
        };
        if TIDY.get().is_none() {
            return Self::Parked { last };
        }
        let due = last.map_or(now + IDLE, |prev| (prev + IDLE_MIN_GAP).max(now + IDLE));
        Self::Armed { last, due }
    }

    pub(crate) fn wait(self, now: crate::time::Instant) -> Option<Duration> {
        match self {
            Self::Parked { .. } => None,
            Self::Armed { due, .. } => Some(due.saturating_duration_since(now)),
        }
    }

    pub(crate) fn take_due(&mut self, now: crate::time::Instant) -> bool {
        match *self {
            Self::Armed { due, .. } if now >= due => {
                *self = Self::Parked { last: Some(now) };
                true
            }
            _ => false,
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(not(target_arch = "wasm32"))]
mod pool;

mod queue;
pub use queue::{Queue, queue};

/// Runs `work` off the UI thread: on a shared task thread, which takes `name` as its Windows
/// thread description while it runs, or a task after the current event on the web. Fails only when
/// the system has no thread left to give.
pub fn spawn(name: impl Into<String>, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        pool::run(name.into(), Box::new(work))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        web::queue(Box::new(work));
        Ok(())
    }
}

/// [`spawn`] after `delay`: a task thread that sleeps first, or a timer on the web.
pub fn spawn_after(name: impl Into<String>, delay: Duration, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        spawn(name, move || {
            std::thread::sleep(delay);
            work();
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        web::queue_after(delay, Box::new(work));
        Ok(())
    }
}

/// Serves `rx` off the UI thread: `each` takes every message in order until it breaks or every
/// sender is gone. Natively a thread named `name` waits on `rx`; on the web Rune drains it after
/// each batch of events and after each task, since a page can't wait on a channel.
pub fn serve<T: Send + 'static>(
    name: impl Into<String>,
    rx: Receiver<T>,
    mut each: impl FnMut(T) -> ControlFlow<()> + Send + 'static,
) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::Builder::new().name(name.into()).spawn(move || {
            use std::sync::mpsc::RecvTimeoutError;
            let mut tidy = Tidy::new();
            loop {
                // Without a hook, or before the first message and after a tidy, wait untimed.
                let got = match tidy.wait(crate::time::Instant::now()) {
                    Some(wait) => rx.recv_timeout(wait),
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                let message = match got {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout) => {
                        if tidy.take_due(crate::time::Instant::now()) {
                            idle();
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                };
                if each(message).is_break() {
                    return;
                }
                tidy = tidy.on_work(crate::time::Instant::now());
            }
        }).map(drop)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        web::serve(Box::new(move || {
            use std::sync::mpsc::TryRecvError;
            loop {
                match rx.try_recv() {
                    Ok(message) => {
                        if each(message).is_break() {
                            return web::Served::Done;
                        }
                    }
                    Err(TryRecvError::Empty) => return web::Served::Idle,
                    Err(TryRecvError::Disconnected) => return web::Served::Done,
                }
            }
        }));
        Ok(())
    }
}

/// Sleeps `wait`: the thread natively; on the web, the task's fiber (the page carries on), or
/// not at all outside task work, where a page can't wait.
pub fn sleep(wait: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(wait);
    #[cfg(target_arch = "wasm32")]
    web::sleep(wait);
}

/// Starts the web's task fibers: from here on, task work runs where [`wait`] can suspend it.
/// Call it from a `#[wasm_bindgen(jspi)]` export (an app's entry), before the app runs; outside
/// one, nothing changes and the work runs as before, unable to wait.
#[cfg(target_arch = "wasm32")]
pub fn start_runners() {
    web::start_runners();
}

/// Waits for `promise` from task work without blocking the page: the work's fiber suspends
/// until it settles. `None` where nothing can wait (no runners, or not in task work): the caller
/// does it the blocking way. Ask [`can_wait`] before starting what the promise stands for (a
/// request), so code that can't wait doesn't start it and then do it again the blocking way.
#[cfg(target_arch = "wasm32")]
pub fn wait(promise: &js_sys::Promise) -> Option<Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>> {
    web::wait(promise)
}

/// Runs what's waiting: the platform layer calls this after each batch of events on the web.
#[cfg(target_arch = "wasm32")]
pub(crate) fn pump() {
    web::pump();
}

/// Whether [`wait`] can suspend here: task work on a fiber.
#[cfg(target_arch = "wasm32")]
pub fn can_wait() -> bool {
    web::can_wait()
}
