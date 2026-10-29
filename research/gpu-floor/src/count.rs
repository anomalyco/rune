//! Retired user and kernel instructions on the calling thread, from Linux `perf_event_open`, and
//! the thread's CPU time.
//! A Windows build makes the same syscalls directly, which works when it runs under Wine on
//! x86-64 Linux and reads zero elsewhere.

pub struct Counter {
    fds: [i64; 2],
}

const PERF_EVENT_OPEN: i64 = 298;
const READ: i64 = 0;
const CLOCK_GETTIME: i64 = 228;
const CLOCK_THREAD_CPUTIME_ID: i64 = 3;

#[cfg(target_os = "linux")]
unsafe fn sys(n: i64, a: i64, b: i64, c: i64, d: i64, e: i64) -> i64 {
    unsafe { libc::syscall(n, a, b, c, d, e) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn sys(n: i64, a: i64, b: i64, c: i64, d: i64, e: i64) -> i64 {
    let ret: i64;
    unsafe {
        std::arch::asm!("syscall", inlateout("rax") n => ret, in("rdi") a, in("rsi") b, in("rdx") c,
            in("r10") d, in("r8") e, lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    ret
}

impl Counter {
    /// `None` when the kernel refuses (no PMU, `perf_event_paranoid` too high, not Linux).
    pub fn new() -> Option<Counter> {
        if cfg!(windows) && std::env::var_os("WINE_PERF").is_none() {
            return None;
        }
        let open = |exclude_user: bool| {
            // perf_event_attr, version 8 (136 bytes): type HARDWARE, config INSTRUCTIONS.
            let mut attr = [0u64; 17];
            attr[0] = (136u64 << 32) | 0;
            attr[1] = 1;
            attr[5] = if exclude_user { 1 << 4 } else { 1 << 5 } | 1 << 6;
            unsafe { sys(PERF_EVENT_OPEN, attr.as_ptr() as i64, 0, -1, -1, 0) }
        };
        let fds = [open(false), open(true)];
        (fds[0] >= 0 && fds[1] >= 0).then_some(Counter { fds })
    }

    /// (user instructions, kernel instructions, CPU nanoseconds) so far.
    pub fn read(&self) -> (u64, u64, u64) {
        let one = |fd: i64| {
            let mut v = 0u64;
            unsafe { sys(READ, fd, &mut v as *mut u64 as i64, 8, 0, 0) };
            v
        };
        let mut ts = [0i64; 2];
        unsafe { sys(CLOCK_GETTIME, CLOCK_THREAD_CPUTIME_ID, ts.as_mut_ptr() as i64, 0, 0, 0) };
        (one(self.fds[0]), one(self.fds[1]), (ts[0] * 1_000_000_000 + ts[1]) as u64)
    }
}
