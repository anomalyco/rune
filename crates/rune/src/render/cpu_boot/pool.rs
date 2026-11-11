//! A small persistent pool of painting threads: parked between frames, woken together for one
//! job, which the calling thread runs too, and joined before `run` returns, so a job may borrow
//! the frame. Spawning threads per frame costs tens of microseconds each on Windows; waking a
//! parked one costs a few.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// A job: run by every thread of the pool and by the caller.
type Job = dyn Fn() + Sync;

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    done: Condvar,
    /// Workers still running the current job.
    running: AtomicUsize,
}

/// What the workers see, as one state: parked, a job to run (and which one, so a worker runs each
/// once), or told to leave.
enum State {
    Idle { round: u64 },
    Working { round: u64, job: JobRef, wanted: usize },
    Leaving,
}

/// A job borrowed for one `run`: `run` waits until every worker that took it is done, so the
/// reference never outlives what it points at.
#[derive(Clone, Copy)]
struct JobRef(*const Job);

// SAFETY: the job is `Sync`; the pointer is only dereferenced while `run` keeps it alive.
unsafe impl Send for JobRef {}

pub struct Pool {
    shared: Arc<Shared>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl Pool {
    /// A pool that runs jobs on `threads` threads in all: the caller and `threads - 1` workers.
    pub fn new(threads: usize) -> Pool {
        let shared = Arc::new(Shared { state: Mutex::new(State::Idle { round: 0 }), wake: Condvar::new(), done: Condvar::new(), running: AtomicUsize::new(0) });
        let workers = (1..threads.max(1)).map(|k| {
            let shared = shared.clone();
            std::thread::Builder::new().name(format!("rune-cpu-paint-{k}")).spawn(move || work(&shared, k)).expect("a painting thread")
        }).collect();
        Pool { shared, workers }
    }

    /// Threads a job runs on, the caller included.
    pub fn threads(&self) -> usize { self.workers.len() + 1 }

    /// Runs `job` on the caller and on `extra` workers (at most all of them), returning once all
    /// have finished it.
    pub fn run(&self, extra: usize, job: &(dyn Fn() + Sync + '_)) {
        let extra = extra.min(self.workers.len());
        if extra == 0 { return job(); }
        // SAFETY: only the lifetime is erased; `run` doesn't return before `running` is back to
        // zero, so no worker holds the pointer past this call.
        let erased: *const Job = unsafe { std::mem::transmute::<&(dyn Fn() + Sync + '_), &'static Job>(job) };
        {
            let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let round = match *s { State::Idle { round } => round + 1, _ => return job() };
            self.shared.running.store(extra, Ordering::Release);
            *s = State::Working { round, job: JobRef(erased), wanted: extra };
        }
        self.shared.wake.notify_all();
        job();
        let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        while self.shared.running.load(Ordering::Acquire) > 0 {
            s = self.shared.done.wait(s).unwrap_or_else(|e| e.into_inner());
        }
        if let State::Working { round, .. } = *s { *s = State::Idle { round }; }
    }
}

/// A worker: waits for a round it hasn't run, runs it if it's among those wanted, reports done.
fn work(shared: &Shared, k: usize) {
    let mut last = 0;
    loop {
        let job = {
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match *s {
                    State::Leaving => return,
                    State::Working { round, job, wanted } if round != last => {
                        last = round;
                        if k <= wanted { break job; }
                    }
                    _ => {}
                }
                s = shared.wake.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        // SAFETY: `run` keeps the job alive until `running` reaches zero, which this decrement
        // can only do after the call returns.
        // A panic in a job is the caller's to see; the count must still go down.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { (*job.0)() }));
        if shared.running.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _guard = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            shared.done.notify_all();
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        *self.shared.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Leaving;
        self.shared.wake.notify_all();
        for w in self.workers.drain(..) { let _ = w.join(); }
    }
}
