//! Unix non-blocking pipe and `StopSignal` primitives for Wayland file drops: close-on-exec
//! transfer pipes, immediate poll cancellation, and `pipe_read::Driver` over `poll(2)`/`read(2)`.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Bound on joining a signalled watch or pipe-reader thread during teardown.
pub(in crate::platform::wayland) const THREAD_JOIN_BOUND: Duration = Duration::from_millis(500);

/// Completion signal paired with a `JoinHandle` so teardown can do a real bounded join.
#[derive(Clone)]
pub(in crate::platform::wayland) struct DoneFlag(Arc<(Mutex<bool>, Condvar)>);

impl DoneFlag {
    pub(in crate::platform::wayland) fn new() -> (Self, Self) {
        let inner = Arc::new((Mutex::new(false), Condvar::new()));
        (Self(inner.clone()), Self(inner))
    }

    pub(in crate::platform::wayland) fn mark_done(&self) {
        let (lock, cond) = &*self.0;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cond.notify_all();
    }

    pub(in crate::platform::wayland) fn is_done(&self) -> bool {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(in crate::platform::wayland) fn wait_bound(&self, bound: Duration) -> bool {
        let (lock, cond) = &*self.0;
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = cond.wait_timeout_while(guard, bound, |done| !*done).unwrap_or_else(|e| e.into_inner());
        *guard
    }
}

/// Marks `DoneFlag` on drop so the signal fires even if the worker thread unwinds.
pub(in crate::platform::wayland) struct SignalOnDrop(pub(in crate::platform::wayland) DoneFlag);

impl Drop for SignalOnDrop {
    fn drop(&mut self) { self.0.mark_done(); }
}

/// Waits up to `bound` for `done` to fire, then performs a real `JoinHandle::join`.
pub(in crate::platform::wayland) fn join_bounded(handle: std::thread::JoinHandle<()>, done: &DoneFlag, bound: Duration) -> bool {
    if done.wait_bound(bound) {
        handle.join().ok();
        true
    } else {
        false
    }
}

use super::machine::{ReadOutcome, pipe_read};

/// Non-blocking fd that wakes `poll(2)` immediately on stop/cancel: `eventfd` on Linux, or a
/// self-pipe on other Unix targets.
pub(super) struct StopSignal {
    #[cfg(target_os = "linux")]
    fd: OwnedFd,
    #[cfg(not(target_os = "linux"))]
    read: OwnedFd,
    #[cfg(not(target_os = "linux"))]
    write: OwnedFd,
}

impl StopSignal {
    #[allow(unsafe_code)]
    pub(super) fn new() -> Option<Self> {
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

    pub(super) fn poll_fd(&self) -> RawFd {
        #[cfg(target_os = "linux")]
        { self.fd.as_raw_fd() }
        #[cfg(not(target_os = "linux"))]
        { self.read.as_raw_fd() }
    }

    #[allow(unsafe_code)]
    pub(super) fn signal(&self) {
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

/// Cancellation handle shared between the controller and one pipe-reader worker thread.
pub(super) struct CancelToken {
    cancelled: AtomicBool,
    signal: StopSignal,
}

impl CancelToken {
    pub(super) fn new() -> Option<Self> {
        Some(Self {
            cancelled: AtomicBool::new(false),
            signal: StopSignal::new()?,
        })
    }

    pub(super) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.signal.signal();
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

struct FdPipeDriver<'a> {
    fd: RawFd,
    cancel: &'a CancelToken,
}

impl pipe_read::Driver for FdPipeDriver<'_> {
    fn now(&mut self) -> crate::time::Instant {
        crate::time::now()
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    #[allow(unsafe_code)]
    fn poll(&mut self, timeout_ms: i32) -> pipe_read::Poll {
        let mut pfds = [
            libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.cancel.signal.poll_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: both descriptors are open for the duration of this `poll` call.
        let ret = unsafe { libc::poll(pfds.as_mut_ptr(), 2, timeout_ms) };
        if ret < 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                pipe_read::Poll::Interrupted
            } else {
                pipe_read::Poll::Error
            };
        }
        if ret == 0 {
            return pipe_read::Poll::TimedOut;
        }
        if pfds[1].revents != 0 || self.cancel.is_cancelled() {
            return pipe_read::Poll::Cancelled;
        }
        if pfds[0].revents & libc::POLLNVAL != 0 {
            return pipe_read::Poll::Error;
        }
        if pfds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            return pipe_read::Poll::Readable;
        }
        pipe_read::Poll::Error
    }

    #[allow(unsafe_code)]
    fn read(&mut self, buf: &mut [u8]) -> pipe_read::Read {
        // SAFETY: `self.fd` is a live open non-blocking descriptor and `buf` is writable.
        let ret = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
        if ret > 0 {
            pipe_read::Read::Bytes(ret as usize)
        } else if ret == 0 {
            pipe_read::Read::Eof
        } else {
            match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::WouldBlock => pipe_read::Read::WouldBlock,
                std::io::ErrorKind::Interrupted => pipe_read::Read::Interrupted,
                _ => pipe_read::Read::Error,
            }
        }
    }
}

/// Reads from a non-blocking `OwnedFd` up to `max_bytes` within `timeout`, aborting early if
/// `cancel` fires. Closes `fd` on return.
pub(super) fn read_fd_bounded(
    fd: OwnedFd,
    cancel: &CancelToken,
    max_bytes: usize,
    timeout: Duration,
) -> Option<ReadOutcome> {
    let mut driver = FdPipeDriver { fd: fd.as_raw_fd(), cancel };
    pipe_read::read_bounded(&mut driver, max_bytes, timeout)
}

/// Creates a close-on-exec pipe whose read end is non-blocking and whose write end stays blocking
/// for compatibility with Wayland drag sources that write synchronously.
#[allow(unsafe_code)]
pub(in crate::platform::wayland) fn make_pipe() -> Option<(OwnedFd, OwnedFd)> {
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
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC); }
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
