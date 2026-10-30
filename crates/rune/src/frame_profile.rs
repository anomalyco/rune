//! Opt-in headless phase timings. No clocks or allocator hooks in ordinary builds.
//! Commit and Layout include their nested phases; these overlapping times must not be summed.
#[derive(Clone, Copy)]
pub(crate) enum Phase { View, Commit, Layout, Text, Paint, Apply }

/// Phases timed, each with a wall and a CPU column.
#[cfg_attr(not(feature = "frame-profile"), allow(dead_code))]
pub const PHASES: usize = 6;

/// CPU consumed by this thread, excluding time the scheduler runs other work. Debug builds
/// time app callbacks with it for `perf_warn`.
#[cfg(any(feature = "frame-profile", debug_assertions))]
pub(crate) fn cpu_ms() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `t` is a live, writable timespec and the clock needs no handle.
        let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        assert_eq!(ok, 0, "thread CPU clock unavailable");
        Some(t.tv_sec as f64 * 1000.0 + t.tv_nsec as f64 / 1e6)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{Foundation::FILETIME, System::Threading::{GetCurrentThread, GetThreadTimes}};
        let mut created = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut exited = created;
        let mut kernel = created;
        let mut user = created;
        // SAFETY: the current-thread pseudo-handle is valid and all four outputs are
        // live, writable FILETIMEs. The pseudo-handle is borrowed, not closed.
        let ok = unsafe { GetThreadTimes(GetCurrentThread(), &mut created, &mut exited, &mut kernel, &mut user) };
        assert_ne!(ok, 0, "thread CPU clock unavailable");
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Some((ticks(kernel) + ticks(user)) as f64 / 10_000.0)
    }
    #[cfg(not(any(unix, windows)))]
    { None }
}

#[cfg(feature = "frame-profile")]
mod enabled {
    use super::{PHASES, Phase, cpu_ms};
    use std::{cell::Cell, time::Instant};

    thread_local! {
        static TIMES: Cell<[f64; 2 * PHASES]> = const { Cell::new([0.0; 2 * PHASES]) };
    }

    pub(crate) struct Scope(Phase, Instant, Option<f64>);
    impl Scope {
        pub(crate) fn new(phase: Phase) -> Self { Self(phase, Instant::now(), cpu_ms()) }
    }
    impl Drop for Scope {
        fn drop(&mut self) {
            TIMES.with(|times| {
                let mut row = times.get();
                row[self.0 as usize] += self.1.elapsed().as_secs_f64() * 1000.0;
                if let Some(start) = self.2 { row[self.0 as usize + PHASES] += cpu_ms().unwrap() - start; }
                times.set(row);
            });
        }
    }
    pub(crate) fn take() -> [f64; 2 * PHASES] { TIMES.replace([0.0; 2 * PHASES]) }
}

#[cfg(feature = "frame-profile")]
pub(crate) use enabled::{Scope, take};

/// One headless frame: phase times (wall then CPU), the update and encode split, and counts.
#[cfg(feature = "frame-profile")]
pub(crate) struct Row {
    pub phases: [f64; 2 * PHASES],
    pub update_ms: f64,
    pub encode_ms: f64,
    /// Script events delivered before this frame.
    pub events: usize,
    pub allocations: u64,
    pub commits: u64,
    /// Why the frame committed, as `runtime::commit_why::CommitWhy` bits.
    pub why: u16,
    pub shaped: u32,
    pub animating: bool,
    pub cpu_ms: Option<f64>,
}

/// A headless run's frames. With `RUNE_FRAME_LOG=FILE`, each frame is also written to FILE as a
/// JSON line as it ends, then a closing line with the process's memory footprint, so a run can
/// be read as a distribution (p99, worst, frames over budget) rather than the summary's means.
#[cfg(feature = "frame-profile")]
pub(crate) struct Log {
    rows: Vec<Row>,
    out: Option<std::io::BufWriter<std::fs::File>>,
}

#[cfg(feature = "frame-profile")]
impl Log {
    pub(crate) fn new(frames: usize) -> Self {
        let out = std::env::var_os("RUNE_FRAME_LOG").map(|path| {
            let file = std::fs::File::create(&path).unwrap_or_else(|e| panic!("RUNE_FRAME_LOG {}: {e}", path.to_string_lossy()));
            std::io::BufWriter::new(file)
        });
        Self { rows: Vec::with_capacity(frames), out }
    }

    pub(crate) fn push(&mut self, row: Row) {
        if let Some(out) = self.out.as_mut() {
            use std::io::Write;
            let p = &row.phases;
            let mut line = serde_json::json!({
                "k": self.rows.len(), "ms": row.update_ms + row.encode_ms, "cpu_ms": row.cpu_ms, "events": row.events, "update_ms": row.update_ms, "encode_ms": row.encode_ms,
                "view_cpu": p[PHASES], "commit_cpu": p[PHASES + 1], "layout_cpu": p[PHASES + 2], "text_cpu": p[PHASES + 3],
                "paint_cpu": p[PHASES + 4], "apply_cpu": p[PHASES + 5], "allocations": row.allocations,
                "commits": row.commits, "shaped": row.shaped, "animating": row.animating,
            });
            if row.commits > 0 { line["why"] = crate::runtime::CommitWhy::names(row.why).into(); }
            writeln!(out, "{line}").expect("RUNE_FRAME_LOG write");
        }
        self.rows.push(row);
    }

    /// Prints the `rune-frame-profile` means (first frame left out) and closes the log.
    pub(crate) fn report(&mut self) {
        let rows = &self.rows[usize::from(self.rows.len() > 1)..];
        let n = rows.len() as f64;
        let avg = |f: &dyn Fn(&Row) -> f64| rows.iter().map(f).sum::<f64>() / n;
        let cpu = rows.first().and_then(|r| r.cpu_ms).is_some();
        let phase_cpu = |i: usize| cpu.then(|| avg(&|r| r.phases[PHASES + i]));
        eprintln!("rune-frame-profile {}", serde_json::json!({
            "view_ms": avg(&|r| r.phases[0]), "commit_inclusive_ms": avg(&|r| r.phases[1]),
            "layout_inclusive_ms": avg(&|r| r.phases[2]), "shape_ms": avg(&|r| r.phases[3]),
            "paint_ms": avg(&|r| r.phases[4]), "apply_ms": avg(&|r| r.phases[5]),
            "update_ms": avg(&|r| r.update_ms), "encode_submit_ms": avg(&|r| r.encode_ms),
            "allocations": avg(&|r| r.allocations as f64), "commits": avg(&|r| r.commits as f64),
            "shaped": avg(&|r| r.shaped as f64), "animating_fraction": avg(&|r| u8::from(r.animating) as f64),
            "thread_cpu_ms": cpu.then(|| avg(&|r| r.cpu_ms.unwrap_or(0.0))),
            "view_cpu_ms": phase_cpu(0), "commit_cpu_ms": phase_cpu(1), "layout_cpu_ms": phase_cpu(2),
            "shape_cpu_ms": phase_cpu(3), "paint_cpu_ms": phase_cpu(4), "apply_cpu_ms": phase_cpu(5),
        }));
        if let Some(mut out) = self.out.take() {
            use std::io::Write;
            let mem = procwatch::snapshot();
            let line = serde_json::json!({
                "end": true, "frames": self.rows.len(),
                "footprint": mem.as_ref().map(|m| m.footprint), "footprint_peak": mem.as_ref().map(|m| m.footprint_peak),
                "resident": mem.as_ref().map(|m| m.resident),
                "heap_live": mem.as_ref().and_then(|m| m.heap.as_ref()).map(|h| h.live),
            });
            writeln!(out, "{line}").and_then(|()| out.flush()).expect("RUNE_FRAME_LOG write");
        }
    }
}

/// Wall then CPU milliseconds per phase (view, commit, layout, text, paint, apply) on this thread
/// since the last call, for benches that drive a `Runtime` themselves.
#[cfg(feature = "frame-profile")]
pub fn take_frame_phases() -> [f64; 2 * PHASES] { take() }

/// This thread's CPU clock in milliseconds.
#[cfg(feature = "frame-profile")]
pub fn thread_cpu_ms() -> f64 { cpu_ms().unwrap_or(0.0) }

#[cfg(not(feature = "frame-profile"))]
pub(crate) struct Scope;
#[cfg(not(feature = "frame-profile"))]
impl Scope {
    #[inline(always)]
    pub(crate) fn new(_: Phase) -> Self { Self }
}
