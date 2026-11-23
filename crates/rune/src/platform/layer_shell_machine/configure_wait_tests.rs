use super::readiness;

#[test]
fn configure_wait_timeout_ms_rounds_up_submillisecond_and_clamps() {
    use super::configure_wait::{TIMEOUT, timeout_ms};
    use std::time::Duration;

    assert!(TIMEOUT >= Duration::from_secs(1) && TIMEOUT <= Duration::from_secs(5));
    assert_eq!(timeout_ms(Duration::from_nanos(1)), 1);
    assert_eq!(timeout_ms(Duration::from_nanos(999_999)), 1);
    assert_eq!(timeout_ms(Duration::from_millis(1)), 1);
    assert_eq!(timeout_ms(Duration::from_nanos(1_000_001)), 2);
    assert_eq!(timeout_ms(Duration::from_secs(2)), 2_000);
    assert_eq!(timeout_ms(Duration::MAX), i32::MAX);
}

#[test]
fn configure_wait_bounds_silent_and_chatty_compositors_and_handles_retries() {
    use super::configure_wait::{Driver, Flush, Outcome, Poll, Read, wait};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::Duration;
    use crate::time::Instant;

    struct Guard {
        cancelled: Rc<Cell<usize>>,
        consumed: bool,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.consumed {
                self.cancelled.set(self.cancelled.get() + 1);
            }
        }
    }

    enum Step {
        Poll {
            result: Poll,
            advance: Duration,
            read: Option<Read>,
            deliver_on_dispatch: Option<readiness::Event>,
        },
        AlreadyPending(Option<readiness::Event>),
    }

    struct FakeDriver {
        now: Instant,
        readiness: readiness::State,
        pending: VecDeque<readiness::Event>,
        flushes: VecDeque<Flush>,
        steps: VecDeque<Step>,
        polls: Vec<(i32, bool)>,
        reads: usize,
        cancelled: Rc<Cell<usize>>,
        dispatch_fail: bool,
    }

    impl FakeDriver {
        fn new(steps: impl IntoIterator<Item = Step>) -> Self {
            Self {
                now: crate::time::now(),
                readiness: readiness::State::default(),
                pending: VecDeque::new(),
                flushes: VecDeque::new(),
                steps: steps.into_iter().collect(),
                polls: Vec::new(),
                reads: 0,
                cancelled: Rc::new(Cell::new(0)),
                dispatch_fail: false,
            }
        }
    }

    impl Driver for FakeDriver {
        type Guard = Guard;

        fn done(&self) -> bool { self.readiness.configured() }

        fn now(&mut self) -> Instant { self.now }

        fn dispatch_pending(&mut self) -> bool {
            if self.dispatch_fail {
                return false;
            }
            while let Some(event) = self.pending.pop_front() {
                readiness::step(&mut self.readiness, event);
            }
            true
        }

        fn flush(&mut self) -> Flush {
            self.flushes.pop_front().unwrap_or(Flush::Ok)
        }

        fn prepare_read(&mut self) -> Option<Guard> {
            if let Some(Step::AlreadyPending(event)) = self.steps.front() {
                let event = *event;
                self.steps.pop_front();
                if let Some(event) = event {
                    self.pending.push_back(event);
                } else {
                    self.now += Duration::from_millis(50);
                }
                return None;
            }
            Some(Guard { cancelled: Rc::clone(&self.cancelled), consumed: false })
        }

        fn poll(&mut self, _guard: &Guard, timeout_ms: i32, want_write: bool) -> Poll {
            assert!(self.polls.len() < 16, "exceeded poll budget — unbounded configure wait");
            self.polls.push((timeout_ms, want_write));
            let Some(Step::Poll { result, advance, deliver_on_dispatch, .. }) = self.steps.front() else {
                panic!("unexpected poll call #{}: no scripted step left", self.polls.len());
            };
            let result = *result;
            self.now += *advance;
            if matches!(result, Poll::TimedOut) && let Some(event) = *deliver_on_dispatch {
                self.pending.push_back(event);
            }
            if !matches!(result, Poll::Readable) {
                self.steps.pop_front();
            }
            result
        }

        fn read(&mut self, mut guard: Guard) -> Read {
            guard.consumed = true;
            self.reads += 1;
            let Some(Step::Poll { read, deliver_on_dispatch, .. }) = self.steps.pop_front() else {
                panic!("read without matching Poll::Readable step");
            };
            if let Some(event) = deliver_on_dispatch {
                self.pending.push_back(event);
            }
            read.unwrap_or(Read::Ok)
        }
    }

    // 1. Silent compositor: never sends configure -> times out at deadline and cancels guard.
    let mut silent = FakeDriver::new([Step::Poll {
        result: Poll::TimedOut,
        advance: Duration::from_millis(200),
        read: None,
        deliver_on_dispatch: None,
    }]);
    assert_eq!(wait(&mut silent, Duration::from_millis(200)), Outcome::TimedOut);
    assert_eq!(silent.polls, [(200, false)]);
    assert_eq!(silent.reads, 0);
    assert_eq!(silent.cancelled.get(), 1, "timed-out prepare_read guard must be cancelled");
    assert!(!silent.readiness.configured());

    // 2. Chatty compositor: sends unrelated events (scale / other queue traffic) repeatedly
    // without ever configuring -> remaining timeout shrinks monotonically and ends at deadline.
    let mut chatty = FakeDriver::new([
        Step::Poll { result: Poll::Readable, advance: Duration::from_millis(70), read: Some(Read::Ok), deliver_on_dispatch: Some(readiness::Event::Scale(2)) },
        Step::Poll { result: Poll::Readable, advance: Duration::from_millis(80), read: Some(Read::WouldBlock), deliver_on_dispatch: None },
        Step::Poll { result: Poll::Readable, advance: Duration::from_millis(50), read: Some(Read::Ok), deliver_on_dispatch: None },
    ]);
    assert_eq!(wait(&mut chatty, Duration::from_millis(200)), Outcome::TimedOut);
    assert_eq!(chatty.polls, [(200, false), (130, false), (50, false)]);
    assert_eq!(chatty.reads, 3);
    assert_eq!(chatty.cancelled.get(), 0);
    assert_eq!(chatty.readiness.take_scale(), Some(2));

    // 3. EINTR and Flush::WouldBlock retry with the same overall deadline, then configure lands.
    let mut retries = FakeDriver::new([
        Step::Poll { result: Poll::WritableOnly, advance: Duration::from_millis(40), read: None, deliver_on_dispatch: None },
        Step::Poll { result: Poll::Interrupted, advance: Duration::from_millis(30), read: None, deliver_on_dispatch: None },
        Step::Poll {
            result: Poll::Readable,
            advance: Duration::from_millis(25),
            read: Some(Read::Ok),
            deliver_on_dispatch: Some(readiness::Event::Configure { width: 320, height: 80 }),
        },
    ]);
    retries.flushes = VecDeque::from([Flush::WouldBlock, Flush::Ok, Flush::Ok]);
    assert_eq!(wait(&mut retries, Duration::from_millis(200)), Outcome::Done);
    assert_eq!(retries.polls, [(200, true), (160, false), (130, false)]);
    assert_eq!(retries.reads, 1);
    assert_eq!(retries.cancelled.get(), 2, "WritableOnly and Interrupted guards must be cancelled");
    assert_eq!(retries.readiness.size(), Some((320, 80)));

    // 4. Compositor closing the surface ends the wait early as Done (caller inspects closed()).
    let mut closed = FakeDriver::new([Step::Poll {
        result: Poll::Readable,
        advance: Duration::from_millis(10),
        read: Some(Read::Ok),
        deliver_on_dispatch: Some(readiness::Event::Closed),
    }]);
    assert_eq!(wait(&mut closed, Duration::from_millis(200)), Outcome::Done);
    assert!(closed.readiness.closed());

    // 5. prepare_read returning None dispatches newly queued events without polling.
    let mut queued = FakeDriver::new([Step::AlreadyPending(Some(readiness::Event::Configure { width: 100, height: 50 }))]);
    assert_eq!(wait(&mut queued, Duration::from_millis(200)), Outcome::Done);
    assert!(queued.polls.is_empty());
    assert_eq!(queued.readiness.size(), Some((100, 50)));

    // 6. Fatal errors from flush, poll, read, or dispatch end immediately with Outcome::Error.
    let mut flush_err = FakeDriver::new([]);
    flush_err.flushes = VecDeque::from([Flush::Error]);
    assert_eq!(wait(&mut flush_err, Duration::from_millis(200)), Outcome::Error);

    let mut poll_err = FakeDriver::new([Step::Poll { result: Poll::Error, advance: Duration::ZERO, read: None, deliver_on_dispatch: None }]);
    assert_eq!(wait(&mut poll_err, Duration::from_millis(200)), Outcome::Error);
    assert_eq!(poll_err.cancelled.get(), 1);

    let mut read_err = FakeDriver::new([Step::Poll { result: Poll::Readable, advance: Duration::ZERO, read: Some(Read::Error), deliver_on_dispatch: None }]);
    assert_eq!(wait(&mut read_err, Duration::from_millis(200)), Outcome::Error);

    let mut dispatch_err = FakeDriver::new([]);
    dispatch_err.dispatch_fail = true;
    assert_eq!(wait(&mut dispatch_err, Duration::from_millis(200)), Outcome::Error);
}
