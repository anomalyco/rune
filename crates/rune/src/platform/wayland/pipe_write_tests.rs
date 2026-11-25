use std::collections::VecDeque;
use std::time::Duration;

use super::{Driver, MAX_PIPE_WRITE_BYTES, PIPE_WRITE_TIMEOUT, Poll, Write, WriteOutcome, write_bounded};
use crate::time::Instant;

struct FakeDriver {
    now: Instant,
    cancelled: bool,
    polls: VecDeque<(Poll, Duration)>,
    writes: VecDeque<Write>,
    written: Vec<u8>,
}

impl FakeDriver {
    fn new() -> Self {
        Self {
            now: crate::time::now(),
            cancelled: false,
            polls: VecDeque::new(),
            writes: VecDeque::new(),
            written: Vec::new(),
        }
    }
}

impl Driver for FakeDriver {
    fn now(&mut self) -> Instant { self.now }
    fn cancelled(&self) -> bool { self.cancelled }
    fn poll_writable(&mut self, _timeout_ms: i32) -> Poll {
        let (poll, dt) = self.polls.pop_front().unwrap_or((Poll::Writable, Duration::ZERO));
        self.now += dt;
        poll
    }
    fn write(&mut self, buf: &[u8]) -> Write {
        match self.writes.pop_front().unwrap_or(Write::Bytes(buf.len())) {
            Write::Bytes(n) => {
                let take = n.min(buf.len());
                self.written.extend_from_slice(&buf[..take]);
                Write::Bytes(take)
            }
            other => other,
        }
    }
}

#[test]
fn write_bounded_handles_chunked_writes_eagain_and_interrupted() {
    let mut d = FakeDriver::new();
    d.polls.push_back((Poll::Interrupted, Duration::from_millis(1)));
    d.polls.push_back((Poll::Writable, Duration::from_millis(1)));
    d.writes.push_back(Write::WouldBlock);
    d.polls.push_back((Poll::Writable, Duration::from_millis(1)));
    d.writes.push_back(Write::Bytes(4));
    d.polls.push_back((Poll::Writable, Duration::from_millis(1)));
    d.writes.push_back(Write::Interrupted);
    d.polls.push_back((Poll::Writable, Duration::from_millis(1)));
    d.writes.push_back(Write::Bytes(6));

    let out = write_bounded(&mut d, b"0123456789", MAX_PIPE_WRITE_BYTES, Duration::from_secs(1));
    assert_eq!(out, WriteOutcome::Complete);
    assert_eq!(d.written, b"0123456789");
}

#[test]
fn write_bounded_enforces_size_cap_epipe_timeout_and_cancel() {
    // Exceeding max_bytes fails immediately without touching the fd.
    let mut d = FakeDriver::new();
    assert_eq!(write_bounded(&mut d, b"abcdef", 4, Duration::from_secs(1)), WriteOutcome::TooLarge);
    assert!(d.written.is_empty());

    // Empty payload succeeds immediately.
    assert_eq!(write_bounded(&mut d, b"", 4, Duration::from_secs(1)), WriteOutcome::Complete);

    // EPIPE maps to PeerClosed.
    d.writes.push_back(Write::BrokenPipe);
    assert_eq!(write_bounded(&mut d, b"hi", 64, Duration::from_secs(1)), WriteOutcome::PeerClosed);

    // Stalled peer expires at deadline.
    let mut d = FakeDriver::new();
    d.polls.push_back((Poll::Writable, Duration::from_millis(60)));
    d.writes.push_back(Write::WouldBlock);
    assert_eq!(write_bounded(&mut d, b"hi", 64, Duration::from_millis(50)), WriteOutcome::TimedOut);

    // Cancelled driver exits immediately.
    let mut d = FakeDriver::new();
    d.cancelled = true;
    assert_eq!(write_bounded(&mut d, b"hi", 64, PIPE_WRITE_TIMEOUT), WriteOutcome::Cancelled);

    // Poll-level PeerClosed, Cancelled, TimedOut, Error, and Write::Error all map cleanly.
    for (poll, expected) in [
        (Poll::PeerClosed, WriteOutcome::PeerClosed),
        (Poll::Cancelled, WriteOutcome::Cancelled),
        (Poll::TimedOut, WriteOutcome::TimedOut),
        (Poll::Error, WriteOutcome::Error),
    ] {
        let mut d = FakeDriver::new();
        d.polls.push_back((poll, Duration::ZERO));
        assert_eq!(write_bounded(&mut d, b"hi", 64, PIPE_WRITE_TIMEOUT), expected);
    }
    let mut d = FakeDriver::new();
    d.writes.push_back(Write::Error);
    assert_eq!(write_bounded(&mut d, b"hi", 64, PIPE_WRITE_TIMEOUT), WriteOutcome::Error);
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod unix_pipe_tests {
    use std::io::Read;
    use std::os::fd::{FromRawFd, OwnedFd, RawFd};
    use std::sync::Arc;
    use std::time::Duration;

    use super::super::MAX_PIPE_WRITE_BYTES;
    use super::super::WriteOutcome;
    use super::super::unix::{WriteCancel, write_fd_bounded};

    fn make_unix_pipe() -> (OwnedFd, OwnedFd) {
        let mut fds = [0 as RawFd; 2];
        // SAFETY: creates a fresh OS pipe pair for testing.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: newly created descriptors owned by this test.
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    }

    #[test]
    fn real_pipe_writes_over_64kib_to_slow_reader_without_blocking_caller() {
        let (read_fd, write_fd) = make_unix_pipe();
        let payload: Arc<[u8]> = Arc::from(vec![0x5Au8; 256 * 1024]);
        let cancel = Arc::new(WriteCancel::new().expect("WriteCancel"));
        let writer_payload = payload.clone();
        let writer_cancel = cancel.clone();
        let handle = std::thread::spawn(move || {
            write_fd_bounded(write_fd, &writer_payload, &writer_cancel, MAX_PIPE_WRITE_BYTES, Duration::from_secs(5))
        });

        // Slow reader drains in small chunks with brief sleeps to force backpressure / EAGAIN.
        let mut file = std::fs::File::from(read_fd);
        let mut received = Vec::with_capacity(payload.len());
        let mut chunk = [0u8; 4096];
        loop {
            let n = file.read(&mut chunk).expect("read");
            if n == 0 {
                break;
            }
            received.extend_from_slice(&chunk[..n]);
            if received.len() % (64 * 1024) == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert_eq!(handle.join().unwrap(), WriteOutcome::Complete);
        assert_eq!(received.len(), payload.len());
        assert!(received.iter().all(|&b| b == 0x5A));
    }

    #[test]
    fn real_pipe_early_reader_close_returns_peer_closed_without_sigpipe() {
        let (read_fd, write_fd) = make_unix_pipe();
        drop(read_fd);
        let cancel = WriteCancel::new().expect("WriteCancel");
        let payload = vec![1u8; 128 * 1024];
        let outcome = write_fd_bounded(write_fd, &payload, &cancel, MAX_PIPE_WRITE_BYTES, Duration::from_secs(2));
        assert_eq!(outcome, WriteOutcome::PeerClosed);
    }

    #[test]
    fn real_pipe_stalled_reader_times_out_and_cancel_wakes_immediately() {
        // 1. Timeout when pipe buffer fills and reader never drains.
        let (_read_fd, write_fd) = make_unix_pipe();
        let cancel = WriteCancel::new().expect("WriteCancel");
        let payload = vec![7u8; 2 * 1024 * 1024];
        let outcome = write_fd_bounded(write_fd, &payload, &cancel, MAX_PIPE_WRITE_BYTES, Duration::from_millis(80));
        assert_eq!(outcome, WriteOutcome::TimedOut);

        // 2. CancelToken aborts a blocked writer well before the 5 s timeout.
        let (_read_fd2, write_fd2) = make_unix_pipe();
        let cancel2 = Arc::new(WriteCancel::new().expect("WriteCancel"));
        let c2 = cancel2.clone();
        let t0 = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            write_fd_bounded(write_fd2, &payload, &c2, MAX_PIPE_WRITE_BYTES, Duration::from_secs(5))
        });
        std::thread::sleep(Duration::from_millis(30));
        cancel2.cancel();
        assert_eq!(handle.join().unwrap(), WriteOutcome::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(2), "cancel must wake poll promptly");
    }
}
