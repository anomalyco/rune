use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn jobs_run_off_the_callers_thread_in_order_and_wait_sees_them_done() {
    let mut worker = PresentWorker::spawn().expect("a thread");
    let caller = std::thread::current().id();
    let log = Arc::new(Mutex::new(Vec::new()));
    for i in 0..4 {
        let log = log.clone();
        worker.run(move || {
            std::thread::sleep(Duration::from_millis(5));
            assert_ne!(std::thread::current().id(), caller);
            log.lock().unwrap().push(i);
        });
    }
    worker.wait();
    assert_eq!(*log.lock().unwrap(), [0, 1, 2, 3]);
}

#[test]
fn run_returns_before_its_job_finishes() {
    let mut worker = PresentWorker::spawn().expect("a thread");
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let done = Arc::new(AtomicUsize::new(0));
    let flag = done.clone();
    worker.run(move || {
        gate.recv().unwrap();
        flag.store(1, Ordering::SeqCst);
    });
    assert_eq!(done.load(Ordering::SeqCst), 0, "the caller went on while the job waited");
    release.send(()).unwrap();
    worker.wait();
    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[test]
fn dropping_the_worker_finishes_the_job_in_flight() {
    let done = Arc::new(AtomicUsize::new(0));
    let flag = done.clone();
    let mut worker = PresentWorker::spawn().expect("a thread");
    worker.run(move || {
        std::thread::sleep(Duration::from_millis(20));
        flag.store(1, Ordering::SeqCst);
    });
    drop(worker);
    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[test]
fn a_panicking_job_panics_on_the_owners_thread() {
    let mut worker = PresentWorker::spawn().expect("a thread");
    worker.run(|| panic!("device lost"));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.wait()));
    let message = caught.expect_err("the panic came back").downcast::<&str>().map(|m| *m);
    assert_eq!(message.ok(), Some("device lost"));
    let ran = Arc::new(AtomicUsize::new(0));
    let flag = ran.clone();
    worker.run(move || { flag.store(1, Ordering::SeqCst); });
    worker.wait();
    assert_eq!(ran.load(Ordering::SeqCst), 1, "later jobs run on the caller's thread");
}

// Every short sequence of jobs, panicking jobs and waits, then the drop, through the real worker
// and its `step`, beside `Old`: a transcription of the fields and branches they replaced
// (`jobs: Option<Sender>`, `in_flight: bool`), which calls none of them. Both must run the same
// jobs on the same threads in the same order, and panic at the same calls with the same payload.

type OldJob = Box<dyn FnOnce() + Send>;

struct Old {
    jobs: Option<std::sync::mpsc::Sender<OldJob>>,
    done: std::sync::mpsc::Receiver<std::thread::Result<()>>,
    in_flight: bool,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Old {
    // The thread's QoS class isn't observable here, so the transcription leaves it out.
    fn spawn() -> Option<Old> {
        let (jobs, inbox) = std::sync::mpsc::channel::<OldJob>();
        let (finished, done) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new().name("old present".into()).spawn(move || {
            while let Ok(job) = inbox.recv() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                if finished.send(result).is_err() { break; }
            }
        }).ok()?;
        Some(Old { jobs: Some(jobs), done, in_flight: false, thread: Some(thread) })
    }

    fn run(&mut self, job: impl FnOnce() + Send + 'static) {
        self.wait();
        let Some(jobs) = &self.jobs else { return job() };
        match jobs.send(Box::new(job)) {
            Ok(()) => self.in_flight = true,
            Err(std::sync::mpsc::SendError(job)) => job(),
        }
    }

    fn wait(&mut self) {
        if !std::mem::take(&mut self.in_flight) { return; }
        if let Ok(Err(panic)) = self.done.recv() {
            self.jobs = None;
            std::panic::resume_unwind(panic);
        }
    }
}

impl Drop for Old {
    fn drop(&mut self) {
        if self.in_flight && !std::thread::panicking() { self.wait(); }
        self.jobs = None;
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    Run,
    RunPanicking,
    Wait,
}

/// Jobs that ran: `(id, on the caller's thread)`.
type Log = Arc<Mutex<Vec<(usize, bool)>>>;

fn job(log: &Log, id: usize, panics: bool) -> impl FnOnce() + Send + 'static {
    let (log, caller) = (log.clone(), std::thread::current().id());
    move || {
        log.lock().unwrap().push((id, std::thread::current().id() == caller));
        if panics { std::panic::panic_any(id); }
    }
}

/// What a call did: `None`, or the id the job that panicked into it carried.
fn outcome(f: impl FnOnce()) -> Option<usize> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).err().map(|p| *p.downcast::<usize>().expect("a job's id"))
}

fn walk(events: &[Ev]) {
    let (new_log, old_log) = (Log::default(), Log::default());
    let mut new = PresentWorker::spawn().expect("a thread");
    let mut old = Old::spawn().expect("a thread");
    for (id, &ev) in events.iter().enumerate() {
        let (n, o) = match ev {
            Ev::Run | Ev::RunPanicking => {
                let panics = matches!(ev, Ev::RunPanicking);
                let (nj, oj) = (job(&new_log, id, panics), job(&old_log, id, panics));
                (outcome(|| new.run(nj)), outcome(|| old.run(oj)))
            }
            Ev::Wait => (outcome(|| new.wait()), outcome(|| old.wait())),
        };
        assert_eq!(n, o, "call {id} of {events:?}");
        if let Ev::Wait = ev {
            assert_eq!(*new_log.lock().unwrap(), *old_log.lock().unwrap(), "after wait {id} of {events:?}");
        }
    }
    assert_eq!(outcome(move || drop(new)), outcome(move || drop(old)), "drop after {events:?}");
    assert_eq!(*new_log.lock().unwrap(), *old_log.lock().unwrap(), "after drop of {events:?}");
}

#[test]
fn every_sequence_of_jobs_and_waits_runs_as_the_old_worker_did() {
    const ALPHABET: [Ev; 3] = [Ev::Run, Ev::RunPanicking, Ev::Wait];
    const LEN: u32 = 6;
    let mut seq = Vec::with_capacity(LEN as usize);
    for len in 0..=LEN {
        for n in 0..ALPHABET.len().pow(len) {
            seq.clear();
            let mut n = n;
            for _ in 0..len {
                seq.push(ALPHABET[n % ALPHABET.len()]);
                n /= ALPHABET.len();
            }
            walk(&seq);
        }
    }
}

#[test]
fn the_state_holds_the_sender_only_while_jobs_go_to_the_thread() {
    let (jobs, _inbox) = std::sync::mpsc::channel::<Job>();
    let mut state = State::Idle(jobs);
    assert!(step(&mut state, Event::Wait).is_empty(), "nothing in flight");
    assert!(step(&mut state, Event::Sent).is_empty());
    assert!(matches!(state, State::Busy(_)));
    assert_eq!(step(&mut state, Event::Wait), Effects::one(Effect::Recv));
    assert!(matches!(state, State::Idle(_)));
    step(&mut state, Event::Panicked);
    assert!(matches!(state, State::Inline));
    assert!(step(&mut state, Event::Sent).is_empty() && step(&mut state, Event::Wait).is_empty());
    assert!(matches!(state, State::Inline), "a panicked worker runs jobs inline for good");
}
