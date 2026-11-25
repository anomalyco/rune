//! Shared non-blocking bounded pipe writer for Wayland data-transfer sources
//! (`wl_data_source.send`, `zwp_primary_selection_source_v1.send`, etc.).
//!
//! Takes raw bytes and an `OwnedFd` and knows nothing about drags or selections. Callers spawn
//! `unix::write_fd_bounded` on a bounded background worker thread (never on the UI thread) with
//! [`PIPE_WRITE_TIMEOUT`] (5 s) and an explicit byte bound (up to [`MAX_PIPE_WRITE_BYTES`], 32 MiB).
//!
//! The loop (`write_bounded`) is pure and cross-platform so every edge case (chunked writes,
//! `EAGAIN`/`WouldBlock`, `EPIPE`, timeout, cancellation, 32 MiB cap) runs headless on every
//! target. The Unix `poll(2)`/`write(2)` driver compiles on all Unix targets (Linux and macOS)
//! so real `pipe(2)` integration tests run on both.

use std::time::Duration;

use crate::time::Instant;

/// Default total write deadline before giving up on a stalled drag-target reader.
pub(crate) const PIPE_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Hard upper bound on a single drag-out pipe transfer (32 MiB).
pub(crate) const MAX_PIPE_WRITE_BYTES: usize = 32 * 1024 * 1024;

/// Readiness returned by [`Driver::poll_writable`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Poll {
    Writable,
    PeerClosed,
    Cancelled,
    TimedOut,
    Interrupted,
    Error,
}

/// Result of one non-blocking write step returned by [`Driver::write`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Write {
    Bytes(usize),
    WouldBlock,
    Interrupted,
    BrokenPipe,
    Error,
}

/// Final status of [`write_bounded`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteOutcome {
    Complete,
    PeerClosed,
    Cancelled,
    TimedOut,
    TooLarge,
    Error,
}

/// I/O and clock seam for [`write_bounded`].
pub(crate) trait Driver {
    fn now(&mut self) -> Instant;
    fn cancelled(&self) -> bool;
    fn poll_writable(&mut self, timeout_ms: i32) -> Poll;
    fn write(&mut self, buf: &[u8]) -> Write;
}

/// Writes `payload` completely through `driver` within `max_bytes` and `timeout`, stopping
/// promptly on cancellation, peer close (`EPIPE` / `POLLERR`), or deadline expiry.
pub(crate) fn write_bounded(
    driver: &mut impl Driver,
    payload: &[u8],
    max_bytes: usize,
    timeout: Duration,
) -> WriteOutcome {
    if payload.len() > max_bytes {
        return WriteOutcome::TooLarge;
    }
    if payload.is_empty() {
        return WriteOutcome::Complete;
    }
    let deadline = driver.now() + timeout;
    let mut offset = 0usize;
    while offset < payload.len() {
        if driver.cancelled() {
            return WriteOutcome::Cancelled;
        }
        let now = driver.now();
        if now >= deadline {
            return WriteOutcome::TimedOut;
        }
        let remaining = deadline.saturating_duration_since(now);
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        match driver.poll_writable(timeout_ms) {
            Poll::Writable => match driver.write(&payload[offset..]) {
                Write::Bytes(0) => return WriteOutcome::Error,
                Write::Bytes(n) => offset = offset.saturating_add(n),
                Write::WouldBlock | Write::Interrupted => {}
                Write::BrokenPipe => return WriteOutcome::PeerClosed,
                Write::Error => return WriteOutcome::Error,
            },
            Poll::PeerClosed => return WriteOutcome::PeerClosed,
            Poll::Cancelled => return WriteOutcome::Cancelled,
            Poll::TimedOut => return WriteOutcome::TimedOut,
            Poll::Interrupted => {}
            Poll::Error => return WriteOutcome::Error,
        }
    }
    WriteOutcome::Complete
}

#[cfg(unix)]
pub(crate) mod unix {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{Driver, Poll, Write, WriteOutcome, write_bounded};

    /// Cancellation token that wakes `poll(2)` immediately without waiting for timeout.
    pub(crate) struct WriteCancel {
        cancelled: AtomicBool,
        #[cfg(target_os = "linux")]
        fd: OwnedFd,
        #[cfg(not(target_os = "linux"))]
        read: OwnedFd,
        #[cfg(not(target_os = "linux"))]
        write: OwnedFd,
    }

    impl WriteCancel {
        #[allow(unsafe_code)]
        pub(crate) fn new() -> Option<Self> {
            #[cfg(target_os = "linux")]
            {
                // SAFETY: creates a fresh close-on-exec, non-blocking eventfd.
                let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
                if fd < 0 {
                    return None;
                }
                // SAFETY: `fd` is owned exclusively by this `WriteCancel`.
                Some(Self {
                    cancelled: AtomicBool::new(false),
                    fd: unsafe { OwnedFd::from_raw_fd(fd) },
                })
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
                // SAFETY: both descriptors are newly created and owned by `WriteCancel`.
                Some(Self {
                    cancelled: AtomicBool::new(false),
                    read: unsafe { OwnedFd::from_raw_fd(fds[0]) },
                    write: unsafe { OwnedFd::from_raw_fd(fds[1]) },
                })
            }
        }

        fn poll_fd(&self) -> RawFd {
            #[cfg(target_os = "linux")]
            { self.fd.as_raw_fd() }
            #[cfg(not(target_os = "linux"))]
            { self.read.as_raw_fd() }
        }

        #[allow(unsafe_code)]
        pub(crate) fn cancel(&self) {
            self.cancelled.store(true, Ordering::Release);
            #[cfg(target_os = "linux")]
            let (fd, buf) = (self.fd.as_raw_fd(), 1u64.to_ne_bytes());
            #[cfg(not(target_os = "linux"))]
            let (fd, buf) = (self.write.as_raw_fd(), [1u8]);
            loop {
                // SAFETY: `fd` is an open non-blocking descriptor owned by `self`.
                let ret = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
                if ret >= 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                    break;
                }
            }
        }

        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Acquire)
        }
    }

    struct FdWriteDriver<'a> {
        fd: RawFd,
        cancel: &'a WriteCancel,
    }

    impl Driver for FdWriteDriver<'_> {
        fn now(&mut self) -> crate::time::Instant {
            crate::time::now()
        }

        fn cancelled(&self) -> bool {
            self.cancel.is_cancelled()
        }

        #[allow(unsafe_code)]
        fn poll_writable(&mut self, timeout_ms: i32) -> Poll {
            let mut pfds = [
                libc::pollfd { fd: self.fd, events: libc::POLLOUT, revents: 0 },
                libc::pollfd { fd: self.cancel.poll_fd(), events: libc::POLLIN, revents: 0 },
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
            if pfds[0].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                return Poll::PeerClosed;
            }
            if pfds[0].revents & libc::POLLOUT != 0 {
                return Poll::Writable;
            }
            Poll::Error
        }

        #[allow(unsafe_code)]
        fn write(&mut self, buf: &[u8]) -> Write {
            // SAFETY: `self.fd` is an open non-blocking descriptor and `buf` is valid for reading.
            let ret = unsafe { libc::write(self.fd, buf.as_ptr().cast(), buf.len()) };
            if ret >= 0 {
                Write::Bytes(ret as usize)
            } else {
                match std::io::Error::last_os_error().kind() {
                    std::io::ErrorKind::WouldBlock => Write::WouldBlock,
                    std::io::ErrorKind::Interrupted => Write::Interrupted,
                    std::io::ErrorKind::BrokenPipe => Write::BrokenPipe,
                    _ => Write::Error,
                }
            }
        }
    }

    /// Configures `fd` as non-blocking (and `F_SETNOSIGPIPE` on macOS), writes `payload` up to
    /// `max_bytes` within `timeout`, and closes `fd` when finished or aborted.
    #[allow(unsafe_code)]
    pub(crate) fn write_fd_bounded(
        fd: OwnedFd,
        payload: &[u8],
        cancel: &WriteCancel,
        max_bytes: usize,
        timeout: Duration,
    ) -> WriteOutcome {
        let raw = fd.as_raw_fd();
        // SAFETY: `raw` is a live open descriptor owned exclusively by `fd`.
        unsafe {
            let flags = libc::fcntl(raw, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
            #[cfg(target_os = "macos")]
            libc::fcntl(raw, libc::F_SETNOSIGPIPE, 1);
        }
        let mut driver = FdWriteDriver { fd: raw, cancel };
        let outcome = write_bounded(&mut driver, payload, max_bytes, timeout);
        drop(fd);
        outcome
    }
}

#[cfg(test)]
#[path = "pipe_write_tests.rs"]
mod tests;
