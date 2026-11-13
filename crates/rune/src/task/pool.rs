//! Native task threads, shared: a task runs on a thread that's waiting for one, and a new thread
//! starts only when none is. [`KEEP`] threads wait for good; the rest end after [`LINGER`] with
//! nothing to do. So a burst of tasks (a session opening its images, a plugin's jobs) reuses a
//! few threads instead of making and ending one each, and the allocator's per-thread heaps don't
//! come and go with them.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Threads that wait for tasks for as long as the process lives.
const KEEP: usize = 2;
/// How long any other thread waits for a task before it ends.
const LINGER: Duration = Duration::from_secs(10);

type Job = (String, Box<dyn FnOnce() + Send>);

struct State {
    jobs: VecDeque<Job>,
    /// Threads waiting for a task now.
    idle: usize,
    threads: usize,
}

static POOL: (Mutex<State>, Condvar) = (Mutex::new(State { jobs: VecDeque::new(), idle: 0, threads: 0 }), Condvar::new());

fn lock() -> MutexGuard<'static, State> {
    POOL.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// Queues `work`, and starts a thread for it when every thread is busy. Fails only when no
/// thread is running and the system gives no new one.
pub(super) fn run(name: String, work: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    let mut s = lock();
    s.jobs.push_back((name, work));
    POOL.1.notify_one();
    if s.idle >= s.jobs.len() {
        return Ok(());
    }
    match std::thread::Builder::new().name("rune-task".into()).spawn(worker) {
        Ok(_) => {
            s.threads += 1;
            Ok(())
        }
        // Another thread takes it once it's free.
        Err(_) if s.threads > 0 => Ok(()),
        Err(error) => {
            s.jobs.pop_back();
            Err(error)
        }
    }
}

fn worker() {
    let mut s = lock();
    loop {
        if let Some((name, work)) = s.jobs.pop_front() {
            drop(s);
            describe(&name);
            // A task that panics ends itself, not the thread (the panic hook has reported it).
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            describe("idle");
            s = lock();
            continue;
        }
        s.idle += 1;
        let (next, wait) = POOL.1.wait_timeout(s, LINGER).unwrap_or_else(|e| e.into_inner());
        s = next;
        s.idle -= 1;
        if wait.timed_out() && s.jobs.is_empty() && s.threads > KEEP {
            s.threads -= 1;
            return;
        }
    }
}

/// Names the thread after its task, where a profiler or debugger reads it (Windows' thread
/// description): `rune-task: <name>`.
#[cfg(target_os = "windows")]
fn describe(name: &str) {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadDescription};
    let wide: Vec<u16> = format!("rune-task: {name}").encode_utf16().chain([0]).collect();
    // SAFETY: the pseudo-handle names this thread; the string is NUL-terminated and outlives the call.
    unsafe { SetThreadDescription(GetCurrentThread(), wide.as_ptr()) };
}

#[cfg(not(target_os = "windows"))]
fn describe(_: &str) {}
