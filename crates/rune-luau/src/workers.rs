//! Work off the UI thread, and the handle that brings its result back to the script.
//!
//! Natively, [`spawn_blocking`] runs a job on a pool of four threads shared by every script,
//! started on first use; idle threads block and cost nothing. The web has no threads by default,
//! so there a job runs on the browser's main thread as a microtask, after the current frame's
//! work: keep web jobs short, and finish browser-async work (a `fetch`) from its own callback
//! through the [`Completer`] instead.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rune::Waker;

use crate::Data;

/// What a script shares with other threads: results waiting for the next poll, and the waker
/// that asks for that poll.
#[derive(Default)]
pub(crate) struct Shared {
    dirty: AtomicBool,
    completions: Mutex<Vec<(u64, Data)>>,
    waker: Mutex<Option<Waker>>,
    next_job: AtomicU64,
}

impl Shared {
    pub(crate) fn set_waker(&self, waker: Waker) {
        if let Ok(mut w) = self.waker.lock() {
            *w = Some(waker);
        }
    }

    /// Asks for a frame, so the next poll runs.
    pub(crate) fn wake(&self) {
        let waker = self.waker.lock().ok().and_then(|w| w.clone());
        if let Some(waker) = waker {
            waker();
        }
    }

    pub(crate) fn mark(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Whether anything arrived since the last call.
    pub(crate) fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn take_completions(&self) -> Vec<(u64, Data)> {
        self.completions.lock().map(|mut c| std::mem::take(&mut *c)).unwrap_or_default()
    }

    pub(crate) fn job(self: &Arc<Self>) -> Completer {
        Completer { id: self.next_job.fetch_add(1, Ordering::Relaxed) + 1, shared: self.clone(), done: false }
    }
}

/// Finishes one call of a host async function: the task that called it resumes on the next poll
/// with the value given to [`Completer::complete`]. `Send`, so any thread can finish it.
/// Dropping it unfinished resumes the task with `nil`.
pub struct Completer {
    pub(crate) id: u64,
    shared: Arc<Shared>,
    done: bool,
}

impl Completer {
    pub fn complete(mut self, value: impl Into<Data>) {
        self.finish(value.into());
    }

    fn finish(&mut self, value: Data) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        if let Ok(mut c) = self.shared.completions.lock() {
            c.push((self.id, value));
        }
        self.shared.mark();
        self.shared.wake();
    }
}

impl Drop for Completer {
    fn drop(&mut self) {
        self.finish(Data::Nil);
    }
}

/// Runs `job` off the UI thread (natively) or after the current frame's work (on the web).
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_blocking(job: impl FnOnce() + Send + 'static) {
    use std::sync::OnceLock;
    use std::sync::mpsc::{Sender, channel};
    type Job = Box<dyn FnOnce() + Send>;
    static POOL: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..4 {
            let rx = rx.clone();
            let spawned = std::thread::Builder::new().name(format!("rune-luau-worker-{i}")).spawn(move || {
                loop {
                    let job = rx.lock().ok().and_then(|r| r.recv().ok());
                    match job {
                        Some(job) => job(),
                        None => break,
                    }
                }
            });
            if spawned.is_err() {
                break;
            }
        }
        Mutex::new(tx)
    });
    let sent = pool.lock().map(|tx| tx.send(Box::new(job)));
    if let Ok(Err(failed)) = sent {
        (failed.0)();
    }
}

/// Runs `job` off the UI thread (natively) or after the current frame's work (on the web).
#[cfg(target_arch = "wasm32")]
pub fn spawn_blocking(job: impl FnOnce() + Send + 'static) {
    wasm_bindgen_futures::spawn_local(async move { job() });
}
