//! The host's monotonic clock in nanoseconds and the calling thread's CPU time, both precise:
//! Windows reads `QueryPerformanceCounter` and `QueryThreadCycleTime`, never `GetThreadTimes`,
//! which moves in 15.6 ms ticks.

#![allow(unsafe_code)]

/// Nanoseconds on the host's monotonic clock: Windows `QueryPerformanceCounter` scaled with
/// 128-bit math, `mach_absolute_time` through its timebase, else `CLOCK_MONOTONIC`.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn now_ns() -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
        static FREQ: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let freq = *FREQ.get_or_init(|| {
            let mut f = 0i64;
            // SAFETY: `f` is a live, writable i64.
            unsafe { QueryPerformanceFrequency(&mut f) };
            f.max(1) as u64
        });
        let mut count = 0i64;
        // SAFETY: `count` is a live, writable i64.
        unsafe { QueryPerformanceCounter(&mut count) };
        (count as u128 * 1_000_000_000 / freq as u128) as u64
    }
    #[cfg(target_vendor = "apple")]
    {
        #[repr(C)]
        struct Timebase { numer: u32, denom: u32 }
        unsafe extern "C" {
            fn mach_timebase_info(info: *mut Timebase) -> i32;
            fn mach_absolute_time() -> u64;
        }
        static BASE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
        let (numer, denom) = *BASE.get_or_init(|| {
            let mut info = Timebase { numer: 0, denom: 0 };
            // SAFETY: `info` is a live, writable timebase struct.
            unsafe { mach_timebase_info(&mut info) };
            (info.numer.max(1), info.denom.max(1))
        });
        // SAFETY: no preconditions.
        let ticks = unsafe { mach_absolute_time() };
        (ticks as u128 * numer as u128 / denom as u128) as u64
    }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `t` is a live, writable timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }
}

/// The calling thread's CPU time in raw units: cycles on Windows, nanoseconds elsewhere; `None`
/// when the OS can't say. [`cpu_ns`] converts.
#[cfg(any(unix, windows))]
pub(crate) fn thread_cpu() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::{Threading::GetCurrentThread, WindowsProgramming::QueryThreadCycleTime};
        let mut cycles = 0u64;
        // SAFETY: the pseudo-handle is valid for the calling thread; `cycles` is live and writable.
        let ok = unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
        (ok != 0).then_some(cycles)
    }
    #[cfg(unix)]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `t` is a live, writable timespec and the clock needs no handle.
        let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        (ok == 0).then(|| t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64)
    }
}

/// Raw [`thread_cpu`] units in nanoseconds; `None` on Windows until [`cycles_per_ns`] knows the
/// rate.
#[cfg(any(unix, windows))]
pub(crate) fn cpu_ns(raw: u64) -> Option<f64> {
    #[cfg(windows)]
    return cycles_per_ns().map(|rate| raw as f64 / rate);
    #[cfg(unix)]
    return Some(raw as f64);
}

/// The span [`cycles_per_ns`] measures the rate over: long enough that reading the two clocks a
/// few microseconds apart moves the rate by under 0.1 %.
#[cfg(all(windows, target_arch = "x86_64"))]
const RATE_SPAN_NS: u64 = 20_000_000;

/// A (QPC, TSC) pair is kept only when the two QPC readings around the TSC's are this close: a
/// thread preempted between them would skew the rate for the rest of the process.
#[cfg(all(windows, target_arch = "x86_64"))]
const PAIR_SLACK_NS: u64 = 100_000;

/// Thread cycles per nanosecond, without waiting or spinning: Windows counts a thread's cycles at
/// the TSC's constant rate on every core (P-cores and E-cores alike), so the rate is the TSC's
/// against QPC between this process's first call and one at least [`RATE_SPAN_NS`] later. `None`
/// until then (callers fall back to the wall clock), when the readings make no sense, and on
/// Windows on ARM, where the cycles aren't a fixed-rate counter's.
#[cfg(windows)]
pub(crate) fn cycles_per_ns() -> Option<f64> {
    static RATE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    if let Some(rate) = RATE.get() {
        return Some(*rate);
    }
    #[cfg(target_arch = "x86_64")]
    {
        static FIRST: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
        let now = pair()?;
        let first = *FIRST.get_or_init(|| now);
        let span = now.0.checked_sub(first.0).filter(|&ns| ns >= RATE_SPAN_NS)?;
        let rate = now.1.checked_sub(first.1)? as f64 / span as f64;
        // A TSC runs at 0.5-10 GHz; anything else is a misread, and the wall clock stays.
        (0.5..=10.0).contains(&rate).then(|| *RATE.get_or_init(|| rate))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        None
    }
}

/// QPC (ns, the middle of two readings) and the TSC between them; `None` when the thread was held
/// between them for more than [`PAIR_SLACK_NS`].
#[cfg(all(windows, target_arch = "x86_64"))]
fn pair() -> Option<(u64, u64)> {
    let before = now_ns();
    // SAFETY: RDTSC has no preconditions on x86-64.
    #[allow(unused_unsafe)]
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    let after = now_ns();
    after.checked_sub(before).filter(|&d| d <= PAIR_SLACK_NS).map(|d| (before + d / 2, tsc))
}

/// [`cycles_per_ns`] for benches and profiles, which want a reading on their first call: waits
/// (sleeping) until the rate is known, at most 50 ms. Never on a path a window waits for.
#[cfg(windows)]
#[cfg_attr(not(feature = "frame-profile"), allow(dead_code))]
pub(crate) fn cycles_per_ns_waiting() -> Option<f64> {
    for _ in 0..50 {
        if let Some(rate) = cycles_per_ns() {
            return Some(rate);
        }
        if cfg!(not(target_arch = "x86_64")) {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    None
}