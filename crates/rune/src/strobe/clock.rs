//! The clock the bridge stamps with (the host's, which strobe reads too), thread ids and CPU.

pub(crate) use crate::frame::host_clock::{cpu_ns, now_ns, thread_cpu};

/// The clock's name in `hello`.
pub(crate) const NAME: &str = if cfg!(windows) { "qpc" } else if cfg!(target_vendor = "apple") { "mach" } else { "monotonic" };

/// The calling thread's OS id.
pub(crate) fn thread_id() -> u64 {
    #[cfg(windows)]
    // SAFETY: no preconditions.
    return u64::from(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() });
    #[cfg(target_vendor = "apple")]
    {
        let mut id = 0u64;
        // SAFETY: a null thread means the calling one; `id` is live and writable.
        unsafe { libc::pthread_threadid_np(0, &mut id) };
        id
    }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    // SAFETY: no preconditions.
    return unsafe { libc::syscall(libc::SYS_gettid) } as u64;
}

/// Measures the cycle rate off the UI thread, so the first run's end needn't wait for it.
pub(crate) fn calibrate() {
    #[cfg(windows)]
    let _ = std::thread::Builder::new().name("rune-strobe-calibrate".into()).spawn(|| { crate::frame::host_clock::cycles_per_ns(); });
}
