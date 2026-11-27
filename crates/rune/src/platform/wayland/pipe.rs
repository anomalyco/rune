//! Shared bounded, cancellable pipe reader and Unix pipe/thread-join primitives for Wayland
//! data transfers (`wl_data_offer.receive`, `zwp_primary_selection_offer_v1.receive`, etc.).
//!
//! The read loop (`read_bounded`) is pure and cross-platform so timeouts, size bounds,
//! cancellation, and `EINTR`/`WouldBlock` retries run unit tests on every host. The Unix
//! `poll(2)`/`read(2)` driver and `make_pipe` / `CancelToken` / `DoneFlag` helpers live in
//! `unix` on all Unix targets (Linux and macOS).

use std::time::Duration;

use crate::time::Instant;

/// Converts a non-zero remaining duration into a positive millisecond timeout for `poll(2)`,
/// rounding sub-millisecond remainders up to 1 ms and clamping to `i32::MAX`.
pub(crate) fn timeout_ms(remaining: Duration) -> i32 {
    let millis = remaining.as_millis();
    let rounded = if remaining.subsec_nanos() % 1_000_000 != 0 {
        millis.saturating_add(1)
    } else {
        millis
    };
    rounded.clamp(1, i32::MAX as u128) as i32
}

/// Result of a bounded Wayland transfer pipe read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadOutcome {
    /// Pipe reached EOF within the caller's byte limit.
    Bytes(Vec<u8>),
    /// Peer wrote more than the caller's byte limit; the pipe was closed early.
    TooLarge,
    /// Peer did not finish writing before the deadline.
    TimedOut,
    /// Pipe creation, worker spawn, or read failed.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Poll {
    Readable,
    Cancelled,
    TimedOut,
    Interrupted,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Read {
    Bytes(usize),
    Eof,
    WouldBlock,
    Interrupted,
    Error,
}

pub(crate) trait Driver {
    fn now(&mut self) -> Instant;
    fn cancelled(&self) -> bool;
    fn poll(&mut self, timeout_ms: i32) -> Poll;
    fn read(&mut self, buf: &mut [u8]) -> Read;
}

/// Reads up to `max_bytes` from `driver` within `timeout`. Returns `None` when cancelled,
/// `Some(ReadOutcome::TooLarge)` as soon as the peer sends byte `max_bytes + 1`, and
/// `Some(ReadOutcome::TimedOut)` when `timeout` elapses before EOF.
pub(crate) fn read_bounded<D: Driver>(
    driver: &mut D,
    max_bytes: usize,
    timeout: Duration,
) -> Option<ReadOutcome> {
    let deadline = driver.now() + timeout;
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if driver.cancelled() {
            return None;
        }
        let remaining = deadline.saturating_duration_since(driver.now());
        if remaining.is_zero() {
            return Some(ReadOutcome::TimedOut);
        }
        match driver.poll(timeout_ms(remaining)) {
            Poll::Cancelled => return None,
            Poll::Interrupted => continue,
            Poll::TimedOut => {
                if driver.now() >= deadline {
                    return Some(ReadOutcome::TimedOut);
                }
                continue;
            }
            Poll::Error => return Some(ReadOutcome::Error),
            Poll::Readable => loop {
                if driver.cancelled() {
                    return None;
                }
                let cap = (max_bytes - out.len()).saturating_add(1).min(chunk.len());
                match driver.read(&mut chunk[..cap]) {
                    Read::Bytes(0) | Read::Eof => return Some(ReadOutcome::Bytes(out)),
                    Read::Bytes(n) => {
                        if out.len() + n > max_bytes {
                            return Some(ReadOutcome::TooLarge);
                        }
                        out.extend_from_slice(&chunk[..n]);
                    }
                    Read::WouldBlock => break,
                    Read::Interrupted => continue,
                    Read::Error => return Some(ReadOutcome::Error),
                }
            },
        }
    }
}

#[cfg(unix)]
pub(crate) mod unix {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use super::{Driver, Poll, Read, ReadOutcome, read_bounded};

    /// Bound on joining a signalled watch or pipe worker thread during teardown.
    pub(crate) const THREAD_JOIN_BOUND: Duration = Duration::from_millis(500);

    /// Completion signal paired with a `JoinHandle` so teardown can do a real bounded join.
    #[derive(Clone)]
    pub(crate) struct DoneFlag(Arc<(Mutex<bool>, Condvar)>);

    impl DoneFlag {
        pub(crate) fn new() -> (Self, Self) {
            let inner = Arc::new((Mutex::new(false), Condvar::new()));
            (Self(inner.clone()), Self(inner))
        }

        pub(crate) fn mark_done(&self) {
            let (lock, cond) = &*self.0;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
            cond.notify_all();
        }

        pub(crate) fn is_done(&self) -> bool {
            *self.0.0.lock().unwrap_or_else(|e| e.into_inner())
        }

        pub(crate) fn wait_bound(&self, bound: Duration) -> bool {
            let (lock, cond) = &*self.0;
            let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            let (guard, _) = cond
                .wait_timeout_while(guard, bound, |done| !*done)
                .unwrap_or_else(|e| e.into_inner());
            *guard
        }
    }

    /// Marks `DoneFlag` on drop so the signal fires even if the worker thread unwinds.
    pub(crate) struct SignalOnDrop(pub(crate) DoneFlag);

    impl Drop for SignalOnDrop {
        fn drop(&mut self) {
            self.0.mark_done();
        }
    }

    /// Waits up to `bound` for `done` to fire, then performs a real `JoinHandle::join`.
    pub(crate) fn join_bounded(
        handle: std::thread::JoinHandle<()>,
        done: &DoneFlag,
        bound: Duration,
    ) -> bool {
        if done.wait_bound(bound) {
            handle.join().ok();
            true
        } else {
            false
        }
    }

    /// Non-blocking fd that wakes `poll(2)` immediately on stop/cancel: `eventfd` on Linux, or a
    /// self-pipe on other Unix targets.
    pub(crate) struct StopSignal {
        #[cfg(target_os = "linux")]
        fd: OwnedFd,
        #[cfg(not(target_os = "linux"))]
        read: OwnedFd,
        #[cfg(not(target_os = "linux"))]
        write: OwnedFd,
    }

    impl StopSignal {
        #[allow(unsafe_code)]
        pub(crate) fn new() -> Option<Self> {
            #[cfg(target_os = "linux")]
            {
                // SAFETY: creates a fresh close-on-exec, non-blocking eventfd.
                let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
                if fd < 0 {
                    return None;
                }
                // SAFETY: `fd` is a newly created descriptor owned exclusively by `StopSignal`.
                Some(Self { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
            }
            #[cfg(not(target_os = "linux"))]
            {
                let mut fds = [0 as RawFd; 2];
                // SAFETY: creates a fresh pipe pair; flags are applied immediately below.
                if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
                    return None;
                }
                for fd in fds {
                    // SAFETY: `fd` was just returned by `pipe`.
                    unsafe {
                        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                        libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
                    }
                }
                // SAFETY: both descriptors are newly created and owned exclusively by `StopSignal`.
                Some(Self {
                    read: unsafe { OwnedFd::from_raw_fd(fds[0]) },
                    write: unsafe { OwnedFd::from_raw_fd(fds[1]) },
                })
            }
        }

        pub(crate) fn poll_fd(&self) -> RawFd {
            #[cfg(target_os = "linux")]
            {
                self.fd.as_raw_fd()
            }
            #[cfg(not(target_os = "linux"))]
            {
                self.read.as_raw_fd()
            }
        }

        #[allow(unsafe_code)]
        pub(crate) fn signal(&self) {
            #[cfg(target_os = "linux")]
            let (fd, buf) = (self.fd.as_raw_fd(), 1u64.to_ne_bytes());
            #[cfg(not(target_os = "linux"))]
            let (fd, buf) = (self.write.as_raw_fd(), [1u8]);
            loop {
                // SAFETY: `fd` is an open non-blocking descriptor owned by `self` and `buf` is valid.
                let ret = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
                if ret >= 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                    break;
                }
            }
        }
    }

    /// Cancellation handle shared between a controller and one pipe-reader worker thread.
    pub(crate) struct CancelToken {
        cancelled: AtomicBool,
        signal: StopSignal,
    }

    impl CancelToken {
        pub(crate) fn new() -> Option<Self> {
            Some(Self {
                cancelled: AtomicBool::new(false),
                signal: StopSignal::new()?,
            })
        }

        pub(crate) fn cancel(&self) {
            self.cancelled.store(true, Ordering::Release);
            self.signal.signal();
        }

        pub(crate) fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Acquire)
        }
    }

    struct FdPipeDriver<'a> {
        fd: RawFd,
        cancel: &'a CancelToken,
    }

    impl Driver for FdPipeDriver<'_> {
        fn now(&mut self) -> crate::time::Instant {
            crate::time::now()
        }

        fn cancelled(&self) -> bool {
            self.cancel.is_cancelled()
        }

        #[allow(unsafe_code)]
        fn poll(&mut self, timeout_ms: i32) -> Poll {
            let mut pfds = [
                libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: self.cancel.signal.poll_fd(), events: libc::POLLIN, revents: 0 },
            ];
            // SAFETY: both descriptors are open for the duration of this `poll` call.
            let ret = unsafe { libc::poll(pfds.as_mut_ptr(), 2, timeout_ms) };
            if ret < 0 {
                return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    Poll::Interrupted
                } else {
                    Poll::Error
                };
            }
            if ret == 0 {
                return Poll::TimedOut;
            }
            if pfds[1].revents != 0 || self.cancel.is_cancelled() {
                return Poll::Cancelled;
            }
            if pfds[0].revents & libc::POLLNVAL != 0 {
                return Poll::Error;
            }
            if pfds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                return Poll::Readable;
            }
            Poll::Error
        }

        #[allow(unsafe_code)]
        fn read(&mut self, buf: &mut [u8]) -> Read {
            // SAFETY: `self.fd` is a live open non-blocking descriptor and `buf` is writable.
            let ret = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if ret > 0 {
                Read::Bytes(ret as usize)
            } else if ret == 0 {
                Read::Eof
            } else {
                match std::io::Error::last_os_error().kind() {
                    std::io::ErrorKind::WouldBlock => Read::WouldBlock,
                    std::io::ErrorKind::Interrupted => Read::Interrupted,
                    _ => Read::Error,
                }
            }
        }
    }

    /// Reads from a non-blocking `OwnedFd` up to `max_bytes` within `timeout`, aborting early if
    /// `cancel` fires. Closes `fd` on return.
    pub(crate) fn read_fd_bounded(
        fd: OwnedFd,
        cancel: &CancelToken,
        max_bytes: usize,
        timeout: Duration,
    ) -> Option<ReadOutcome> {
        let mut driver = FdPipeDriver { fd: fd.as_raw_fd(), cancel };
        read_bounded(&mut driver, max_bytes, timeout)
    }

    /// Creates a close-on-exec pipe whose read end is non-blocking and whose write end stays
    /// blocking for compatibility with Wayland sources that write synchronously.
    #[allow(unsafe_code)]
    pub(crate) fn make_pipe() -> Option<(OwnedFd, OwnedFd)> {
        let mut fds = [0 as RawFd; 2];
        #[cfg(target_os = "linux")]
        {
            // SAFETY: creates a fresh close-on-exec pipe pair.
            if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
                return None;
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // SAFETY: creates a fresh pipe pair; FD_CLOEXEC is applied immediately below.
            if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
                return None;
            }
            for fd in fds {
                // SAFETY: `fd` was just returned by `pipe`.
                unsafe {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }
        }
        // SAFETY: `fds[0]` is the read end we own exclusively.
        let flags = unsafe { libc::fcntl(fds[0], libc::F_GETFL) };
        // SAFETY: `fds[0]` is the read end we own exclusively.
        let set_ok = flags >= 0 && unsafe { libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0;
        if !set_ok {
            // SAFETY: both descriptors were just created above and are not yet wrapped in `OwnedFd`.
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            return None;
        }
        // SAFETY: both descriptors are newly created and owned exclusively here.
        unsafe { Some((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
    }
}
