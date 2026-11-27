use super::readiness;

#[test]
fn watch_loop_wakes_on_prequeued_and_read_events_releases_guard_before_wait_and_stops_cleanly() {
    use super::watch_loop::{Dispatch, Driver, Poll, run};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    struct Guard {
        live: Rc<Cell<usize>>,
        cancelled: Rc<Cell<usize>>,
        consumed: bool,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.live.set(self.live.get() - 1);
            if !self.consumed {
                self.cancelled.set(self.cancelled.get() + 1);
            }
        }
    }

    enum Step {
        PreQueued(Dispatch),
        Poll { result: Poll, read: Option<Dispatch> },
    }

    struct FakeDriver {
        steps: VecDeque<Step>,
        stopped: bool,
        stop_on_wake: Option<usize>,
        live_guards: Rc<Cell<usize>>,
        cancelled: Rc<Cell<usize>>,
        polls: usize,
        reads: usize,
        inner_dispatches: usize,
        wakes: usize,
        guard_held_during_wake: bool,
    }

    impl FakeDriver {
        fn new(steps: impl IntoIterator<Item = Step>) -> Self {
            Self {
                steps: steps.into_iter().collect(),
                stopped: false,
                stop_on_wake: None,
                live_guards: Rc::new(Cell::new(0)),
                cancelled: Rc::new(Cell::new(0)),
                polls: 0,
                reads: 0,
                inner_dispatches: 0,
                wakes: 0,
                guard_held_during_wake: false,
            }
        }
    }

    impl Driver for FakeDriver {
        type Guard = Guard;

        fn stopped(&self) -> bool { self.stopped }

        fn prepare_read(&mut self) -> Option<Guard> {
            assert_eq!(self.live_guards.get(), 0, "must not nest read guards");
            match self.steps.front() {
                Some(Step::PreQueued(_)) => None,
                Some(Step::Poll { .. }) => {
                    self.live_guards.set(self.live_guards.get() + 1);
                    Some(Guard {
                        live: Rc::clone(&self.live_guards),
                        cancelled: Rc::clone(&self.cancelled),
                        consumed: false,
                    })
                }
                None => {
                    self.stopped = true;
                    None
                }
            }
        }

        fn dispatch_inner_queue(&mut self) -> Dispatch {
            self.inner_dispatches += 1;
            match self.steps.pop_front() {
                Some(Step::PreQueued(d)) => d,
                _ => panic!("dispatch_inner_queue without matching PreQueued step"),
            }
        }

        fn poll(&mut self, _guard: &Guard) -> Poll {
            assert_eq!(self.live_guards.get(), 1, "poll must hold prepared read guard");
            self.polls += 1;
            let Some(Step::Poll { result, .. }) = self.steps.front() else {
                panic!("poll without matching Poll step");
            };
            let result = *result;
            if !matches!(result, Poll::Readable) {
                self.steps.pop_front();
            }
            result
        }

        fn read(&mut self, mut guard: Guard) -> Dispatch {
            guard.consumed = true;
            drop(guard);
            self.reads += 1;
            match self.steps.pop_front() {
                Some(Step::Poll { read: Some(d), .. }) => d,
                _ => panic!("read without matching Poll::Readable step"),
            }
        }

        fn wake_and_wait(&mut self) {
            if self.live_guards.get() != 0 {
                self.guard_held_during_wake = true;
            }
            self.wakes += 1;
            if self.stop_on_wake == Some(self.wakes) {
                self.stopped = true;
            }
        }
    }

    // 1. Events already left in the layer's queue by winit or Mesa (prepare_read == None):
    //    dispatches inner queue and wakes when n > 0; ignores n == 0 (main thread drained first).
    let mut prequeued = FakeDriver::new([
        Step::PreQueued(Dispatch::Events(2)),
        Step::PreQueued(Dispatch::Events(0)),
        Step::Poll { result: Poll::Stop, read: None },
    ]);
    run(&mut prequeued);
    assert_eq!(prequeued.inner_dispatches, 2);
    assert_eq!(prequeued.wakes, 1, "must wake once for pre-queued events and skip Events(0)");
    assert_eq!(prequeued.polls, 1);
    assert_eq!(prequeued.cancelled.get(), 1, "Poll::Stop must cancel the prepared read guard");
    assert!(!prequeued.guard_held_during_wake);
    assert_eq!(prequeued.live_guards.get(), 0);

    // 2. Socket traffic: events for another queue (Events(0)), WouldBlock, and EINTR do not wake;
    //    events for this layer wake without holding a read guard while waiting for the loop turn.
    let mut socket = FakeDriver::new([
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(0)) },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::WouldBlock) },
        Step::Poll { result: Poll::Interrupted, read: None },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(3)) },
        Step::Poll { result: Poll::Stop, read: None },
    ]);
    run(&mut socket);
    assert_eq!(socket.polls, 5);
    assert_eq!(socket.reads, 3);
    assert_eq!(socket.wakes, 1);
    assert_eq!(socket.cancelled.get(), 2, "EINTR and Stop must cancel their prepared read guards");
    assert!(!socket.guard_held_during_wake, "must not hold a read guard while waiting for a turn");
    assert_eq!(socket.live_guards.get(), 0);

    // 3. Drop while waiting for a turn exits immediately without calling prepare_read again.
    let mut stop_in_wait = FakeDriver::new([
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(1)) },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(1)) },
    ]);
    stop_in_wait.stop_on_wake = Some(1);
    run(&mut stop_in_wait);
    assert_eq!(stop_in_wait.polls, 1);
    assert_eq!(stop_in_wait.wakes, 1);
    assert_eq!(stop_in_wait.steps.len(), 1, "must stop right after wake_and_wait observes stop");
    assert_eq!(stop_in_wait.live_guards.get(), 0);

    // 4. Compositor hangup (read Error), inner-queue Error, and Poll::Error exit cleanly.
    let mut hangup = FakeDriver::new([
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Error) },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(1)) },
    ]);
    run(&mut hangup);
    assert_eq!(hangup.reads, 1);
    assert_eq!(hangup.wakes, 0);
    assert_eq!(hangup.steps.len(), 1);
    assert_eq!(hangup.live_guards.get(), 0);

    let mut inner_err = FakeDriver::new([
        Step::PreQueued(Dispatch::Error),
        Step::PreQueued(Dispatch::Events(1)),
    ]);
    run(&mut inner_err);
    assert_eq!(inner_err.inner_dispatches, 1);
    assert_eq!(inner_err.wakes, 0);
    assert_eq!(inner_err.steps.len(), 1);

    let mut poll_err = FakeDriver::new([
        Step::Poll { result: Poll::Error, read: None },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(1)) },
    ]);
    run(&mut poll_err);
    assert_eq!(poll_err.polls, 1);
    assert_eq!(poll_err.reads, 0);
    assert_eq!(poll_err.wakes, 0);
    assert_eq!(poll_err.cancelled.get(), 1);
    assert_eq!(poll_err.live_guards.get(), 0);
}

#[test]
fn watch_poll_exits_on_hangup_error_or_invalid_fd_instead_of_spinning() {
    use super::connection::{WatchAction, WatchPoll, watch_action};

    // Idle timeout and EINTR keep polling without waking.
    assert_eq!(watch_action(WatchPoll::TimedOut), WatchAction::Continue);
    assert_eq!(watch_action(WatchPoll::Interrupted), WatchAction::Continue);

    // Fatal poll(2) OS error (e.g. EBADF) stops immediately instead of tight-looping.
    assert_eq!(watch_action(WatchPoll::Error), WatchAction::Stop);

    // Normal readable socket wakes and keeps watching.
    assert_eq!(
        watch_action(WatchPoll::Ready { readable: true, hangup_or_error: false, invalid: false }),
        WatchAction::Wake,
    );

    // Hangup or socket error (with or without POLLIN for a final error event) wakes once so
    // the overlay observes the dead connection, then stops the watch thread.
    assert_eq!(
        watch_action(WatchPoll::Ready { readable: false, hangup_or_error: true, invalid: false }),
        WatchAction::WakeAndStop,
    );
    assert_eq!(
        watch_action(WatchPoll::Ready { readable: true, hangup_or_error: true, invalid: false }),
        WatchAction::WakeAndStop,
    );

    // POLLNVAL means the fd is gone: stop without waking, even if other bits are set.
    assert_eq!(
        watch_action(WatchPoll::Ready { readable: false, hangup_or_error: false, invalid: true }),
        WatchAction::Stop,
    );
    assert_eq!(
        watch_action(WatchPoll::Ready { readable: true, hangup_or_error: true, invalid: true }),
        WatchAction::Stop,
    );

    // Defensive: poll returned >0 with no matching flags.
    assert_eq!(
        watch_action(WatchPoll::Ready { readable: false, hangup_or_error: false, invalid: false }),
        WatchAction::Continue,
    );
}

#[test]
fn dead_connection_closes_layer_surface_and_reports_once() {
    use super::connection::{ErrorKind, TurnAction, is_fatal, turn_action};

    // Only WouldBlock is transient; protocol and socket I/O errors mean the display is gone.
    assert!(!is_fatal(ErrorKind::WouldBlock));
    assert!(is_fatal(ErrorKind::Protocol));
    assert!(is_fatal(ErrorKind::Io));

    // Healthy turn and transient WouldBlock keep the surface open.
    assert_eq!(turn_action(false, None, false), TurnAction::Continue);
    assert_eq!(turn_action(false, Some(ErrorKind::WouldBlock), false), TurnAction::Continue);

    // Compositor-requested close stops the watch without treating it as a connection failure.
    assert_eq!(turn_action(false, None, true), TurnAction::StopClosed);
    assert_eq!(turn_action(false, Some(ErrorKind::WouldBlock), true), TurnAction::StopClosed);

    // Fatal protocol or I/O error closes the surface and reports once.
    for kind in [ErrorKind::Protocol, ErrorKind::Io] {
        let mut ready = readiness::State::default();
        let _ = readiness::step(&mut ready, readiness::Event::Configure { width: 320, height: 80 });
        assert_eq!(turn_action(ready.closed(), Some(kind), ready.closed()), TurnAction::FailAndClose);
        let _ = readiness::step(&mut ready, readiness::Event::ConnectionFailed);
        assert!(ready.closed());
        assert!(ready.configured());
        assert_eq!(ready.size(), None);
        assert_eq!(ready.take_size(), None);
        assert_eq!(ready.take_scale(), None);
        // Any later turn on the already-closed surface is a quiet no-op (never reports twice).
        assert_eq!(turn_action(ready.closed(), Some(kind), ready.closed()), TurnAction::AlreadyClosed);
        assert_eq!(turn_action(ready.closed(), None, ready.closed()), TurnAction::AlreadyClosed);
    }

    // Even if a compositor close event arrived in the same batch as a fatal connection error,
    // the connection failure is still reported once.
    assert_eq!(turn_action(false, Some(ErrorKind::Io), true), TurnAction::FailAndClose);
}
