//! A thread that finishes, submits and presents a surface's frames, so the event loop's thread
//! records a frame and moves on: wgpu-core encodes the recorded passes at `finish`, and
//! `finish`, `submit` and `present` are most of a simple frame's CPU time.
//!
//! One job is in flight at most. The owner calls `wait` before it next touches the surface
//! (acquire, configure, drop), because wgpu lends a surface one texture at a time.

use std::sync::mpsc;
use std::thread::JoinHandle;

use crate::machine::Effects;

type Job = Box<dyn FnOnce() + Send>;

pub(crate) struct PresentWorker {
    state: State,
    done: mpsc::Receiver<std::thread::Result<()>>,
    thread: Option<JoinHandle<()>>,
}

/// Where jobs run.
enum State {
    /// On the thread, which has finished the job before.
    Idle(mpsc::Sender<Job>),
    /// On the thread, which may still be running the last job sent.
    Busy(mpsc::Sender<Job>),
    /// On the owner's thread: a job panicked, or the worker is closing.
    Inline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    /// The owner waits for the job in flight.
    Wait,
    /// The thread took a job.
    Sent,
    /// The job waited for panicked.
    Panicked,
    /// The worker drops.
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    /// Block for the job in flight's result.
    Recv,
}

fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let mut effects = Effects::none();
    *state = match (std::mem::replace(state, State::Inline), event) {
        (State::Busy(jobs), Event::Wait) => {
            effects.push(Effect::Recv);
            State::Idle(jobs)
        }
        (State::Idle(jobs), Event::Sent) => State::Busy(jobs),
        (_, Event::Panicked | Event::Close) => State::Inline,
        (state, _) => state,
    };
    effects
}

impl PresentWorker {
    /// `None` when the thread can't be made; the caller presents on its own thread.
    pub(crate) fn spawn() -> Option<PresentWorker> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (finished, done) = mpsc::channel();
        let thread = std::thread::Builder::new().name("rune present".into()).spawn(move || {
            #[cfg(target_os = "macos")]
            interactive();
            while let Ok(job) = inbox.recv() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                if finished.send(result).is_err() { break; }
            }
        }).ok()?;
        Some(PresentWorker { state: State::Idle(jobs), done, thread: Some(thread) })
    }

    /// Runs `job` on the thread once the job before it has finished.
    pub(crate) fn run(&mut self, job: impl FnOnce() + Send + 'static) {
        self.wait();
        let State::Idle(jobs) = &self.state else { return job() };
        match jobs.send(Box::new(job)) {
            Ok(()) => { step(&mut self.state, Event::Sent); }
            Err(mpsc::SendError(job)) => job(),
        }
    }

    /// Blocks until the job in flight, if any, has finished. A job that panicked panics here,
    /// on the owner's thread, as it would have without the worker.
    pub(crate) fn wait(&mut self) {
        for Effect::Recv in step(&mut self.state, Event::Wait) {
            if let Ok(Err(panic)) = self.done.recv() {
                step(&mut self.state, Event::Panicked);
                std::panic::resume_unwind(panic);
            }
        }
    }
}

impl Drop for PresentWorker {
    fn drop(&mut self) {
        if matches!(self.state, State::Busy(_)) && !std::thread::panicking() { self.wait(); }
        step(&mut self.state, Event::Close);
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

/// Presents keep pace with the display only at the main thread's priority.
#[cfg(target_os = "macos")]
fn interactive() {
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
    }
    // SAFETY: sets the calling thread's own QoS class.
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
}

#[cfg(test)]
#[path = "present_worker_tests.rs"]
mod tests;
