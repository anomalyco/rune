//! Opt-in headless phase timings. No clocks or allocator hooks in ordinary builds.
//! Commit and Layout include their nested phases; these overlapping times must not be summed.
#[derive(Clone, Copy)]
pub(crate) enum Phase { View, Commit, Layout, Text, Paint, Apply }

/// Phases timed, each with a wall and a CPU column.
#[cfg_attr(not(feature = "frame-profile"), allow(dead_code))]
pub const PHASES: usize = 6;

/// Converts raw `thread_cpu` units (cycles on Windows, ns elsewhere) to nanoseconds, or
/// `None` until Windows knows the cycles' rate or where there is no CPU clock.
#[cfg(feature = "frame-profile")]
pub(crate) fn raw_cpu_ns(raw: u64) -> Option<f64> {
    #[cfg(any(unix, windows))]
    return super::host_clock::cpu_ns(raw);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = raw;
        None
    }
}

#[cfg(feature = "frame-profile")]
mod enabled {
    use super::{PHASES, Phase, ThreadCpuMark, raw_cpu_ns, thread_cpu_mark};
    use crate::time::Instant;
    use std::cell::Cell;

    thread_local! {
        static WALL: Cell<[f64; PHASES]> = const { Cell::new([0.0; PHASES]) };
        /// Raw thread-CPU units per phase (cycles on Windows, ns on Unix), converted in
        /// [`take`] so phases timed before Windows knows the cycles' rate convert once it does.
        static CPU_RAW: Cell<[u64; PHASES]> = const { Cell::new([0; PHASES]) };
    }

    pub(crate) struct Scope(Phase, Instant, Option<ThreadCpuMark>, bool);
    impl Scope {
        pub(crate) fn new(phase: Phase) -> Self {
            let staged = crate::frame::stage::active();
            if staged {
                crate::frame::stage::emit_phase(phase, crate::frame::stage::StageBoundary::Begin);
            }
            Self(phase, crate::time::now(), thread_cpu_mark(), staged)
        }
    }
    impl Drop for Scope {
        fn drop(&mut self) {
            let wall_ms = self.1.elapsed().as_secs_f64() * 1000.0;
            if self.3 {
                crate::frame::stage::emit_phase(self.0, crate::frame::stage::StageBoundary::End);
            }
            if crate::tooling::perf_warn::budget::on() { crate::tooling::perf_warn::budget::add(self.0, wall_ms); }
            if crate::tooling::interaction::active() { crate::tooling::interaction::add(self.0, wall_ms); }
            WALL.with(|wall| {
                let mut row = wall.get();
                row[self.0 as usize] += wall_ms;
                wall.set(row);
            });
            if let (Some(start), Some(end)) = (self.2, thread_cpu_mark()) {
                CPU_RAW.with(|cpu| {
                    let mut row = cpu.get();
                    row[self.0 as usize] = row[self.0 as usize].saturating_add(start.raw_delta(end));
                    cpu.set(row);
                });
            }
        }
    }
    pub(crate) fn take() -> [f64; 2 * PHASES] {
        let wall = WALL.replace([0.0; PHASES]);
        let raw = CPU_RAW.replace([0; PHASES]);
        let mut out = [0.0; 2 * PHASES];
        out[..PHASES].copy_from_slice(&wall);
        for i in 0..PHASES {
            out[PHASES + i] = raw_cpu_ns(raw[i]).map_or(0.0, |ns| ns / 1e6);
        }
        out
    }
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
        install_text_hook();
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

    /// Means over the logged frames (first frame left out when more than one ran). CPU means
    /// average only rows that had a CPU reading, so early frames before Windows knows the
    /// cycles' rate neither null out the summary nor drag the averages down with zeros.
    pub(crate) fn summary(&self) -> serde_json::Value {
        let rows = &self.rows[usize::from(self.rows.len() > 1)..];
        let n = rows.len() as f64;
        let avg = |f: &dyn Fn(&Row) -> f64| rows.iter().map(f).sum::<f64>() / n;
        let cpu_count = rows.iter().filter(|r| r.cpu_ms.is_some()).count();
        let cpu_n = cpu_count as f64;
        let thread_cpu_ms = (cpu_count > 0).then(|| rows.iter().filter_map(|r| r.cpu_ms).sum::<f64>() / cpu_n);
        let phase_cpu = |i: usize| {
            (cpu_count > 0).then(|| {
                rows.iter().filter(|r| r.cpu_ms.is_some()).map(|r| r.phases[PHASES + i]).sum::<f64>() / cpu_n
            })
        };
        serde_json::json!({
            "view_ms": avg(&|r| r.phases[0]), "commit_inclusive_ms": avg(&|r| r.phases[1]),
            "layout_inclusive_ms": avg(&|r| r.phases[2]), "shape_ms": avg(&|r| r.phases[3]),
            "paint_ms": avg(&|r| r.phases[4]), "apply_ms": avg(&|r| r.phases[5]),
            "update_ms": avg(&|r| r.update_ms), "encode_submit_ms": avg(&|r| r.encode_ms),
            "allocations": avg(&|r| r.allocations as f64), "commits": avg(&|r| r.commits as f64),
            "shaped": avg(&|r| r.shaped as f64), "animating_fraction": avg(&|r| u8::from(r.animating) as f64),
            "thread_cpu_ms": thread_cpu_ms,
            "view_cpu_ms": phase_cpu(0), "commit_cpu_ms": phase_cpu(1), "layout_cpu_ms": phase_cpu(2),
            "shape_cpu_ms": phase_cpu(3), "paint_cpu_ms": phase_cpu(4), "apply_cpu_ms": phase_cpu(5),
        })
    }

    /// Prints the `rune-frame-profile` means (first frame left out) and closes the log.
    pub(crate) fn report(&mut self) {
        eprintln!("rune-frame-profile {}", self.summary());
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
pub fn take_frame_phases() -> [f64; 2 * PHASES] {
    install_text_hook();
    take()
}

/// This thread's CPU clock in milliseconds. On Windows its first call waits (at most 50 ms) for the
/// cycles' rate, so a bench's first reading is a real one, not 0.
#[cfg(feature = "frame-profile")]
pub fn thread_cpu_ms() -> f64 {
    #[cfg(windows)]
    super::host_clock::cycles_per_ns_waiting();
    thread_cpu_ns().map_or(0.0, |ns| ns / 1e6)
}

/// This thread's own CPU time in nanoseconds: its cycles on Windows (`QueryThreadCycleTime`),
/// `CLOCK_THREAD_CPUTIME_ID` elsewhere. It stands still while the thread waits or the system
/// runs something else, so a budget kept on it charges only work the thread did. `None` where
/// there is no such clock (the browser, Windows on ARM), when the OS can't read it, and on Windows
/// for the first 20 ms after the process first asks, while the cycles' rate is measured.
pub fn thread_cpu_ns() -> Option<f64> {
    #[cfg(any(unix, windows))]
    return super::host_clock::thread_cpu().and_then(super::host_clock::cpu_ns);
    #[cfg(not(any(unix, windows)))]
    None
}
/// A reading of this thread's CPU clock in its raw units ([`thread_cpu_mark`]), taken now and
/// converted later ([`thread_cpu_since`]): a mark taken before Windows knows the cycles' rate still
/// measures from that moment once it does.
#[derive(Clone, Copy, Debug)]
pub struct ThreadCpuMark(
    // Only unix/windows CPU clocks or frame-profile read the raw timestamp.
    #[cfg_attr(not(any(unix, windows, feature = "frame-profile")), allow(dead_code))]
    u64,
);

#[cfg(feature = "frame-profile")]
impl ThreadCpuMark {
    /// Raw thread-CPU units elapsed from `self` to `later` (cycles on Windows, ns elsewhere).
    pub(crate) fn raw_delta(self, later: Self) -> u64 {
        later.0.saturating_sub(self.0)
    }

    /// Nanoseconds of thread CPU from `self` to `later`, or `None` until Windows knows the
    /// cycles' rate or where there is no CPU clock.
    pub(crate) fn delta_ns(self, later: Self) -> Option<f64> {
        raw_cpu_ns(self.raw_delta(later))
    }
}

/// This thread's CPU clock now, raw; `None` where there is none or it can't be read. On Windows it
/// also starts measuring the cycles' rate (no wait).
pub fn thread_cpu_mark() -> Option<ThreadCpuMark> {
    #[cfg(windows)]
    super::host_clock::cycles_per_ns();
    #[cfg(any(unix, windows))]
    return super::host_clock::thread_cpu().map(ThreadCpuMark);
    #[cfg(not(any(unix, windows)))]
    None
}

/// Nanoseconds of this thread's CPU time since `mark` (taken on this thread); `None` until the
/// rate is known (Windows, the first 20 ms) or when the clock can't be read.
pub fn thread_cpu_since(mark: ThreadCpuMark) -> Option<f64> {
    #[cfg(any(unix, windows))]
    return super::host_clock::thread_cpu().and_then(|now| super::host_clock::cpu_ns(now.saturating_sub(mark.0)));
    #[cfg(not(any(unix, windows)))]
    {
        let _ = mark;
        None
    }
}

/// Without `frame-profile`, a phase is timed only for `RUNE_COMMIT_BUDGET_MS`
/// (`perf_warn::budget`), in a frame interaction timing follows (`tooling::interaction`), or
/// when an `App::on_frame_stage` hook is active; otherwise it costs cached/TLS flag checks.
#[cfg(not(feature = "frame-profile"))]
pub(crate) struct Scope(Option<(Phase, crate::time::Instant)>, Option<Phase>);
#[cfg(not(feature = "frame-profile"))]
impl Scope {
    #[inline(always)]
    pub(crate) fn new(phase: Phase) -> Self {
        let timed = (crate::tooling::perf_warn::budget::on() || crate::tooling::interaction::active())
            .then(|| (phase, crate::time::now()));
        let staged = if crate::frame::stage::active() {
            crate::frame::stage::emit_phase(phase, crate::frame::stage::StageBoundary::Begin);
            Some(phase)
        } else {
            None
        };
        Self(timed, staged)
    }
}
#[cfg(not(feature = "frame-profile"))]
impl Drop for Scope {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some((phase, start)) = self.0 {
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            if crate::tooling::perf_warn::budget::on() { crate::tooling::perf_warn::budget::add(phase, ms); }
            if crate::tooling::interaction::active() { crate::tooling::interaction::add(phase, ms); }
        }
        if let Some(phase) = self.1 {
            crate::frame::stage::emit_phase(phase, crate::frame::stage::StageBoundary::End);
        }
    }
}

thread_local! {
    static TEXT_SCOPES: std::cell::RefCell<Vec<Scope>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn enter_text_phase() -> crate::text::profile::Token {
    #[cfg(not(feature = "frame-profile"))]
    if !crate::tooling::perf_warn::budget::on()
        && !crate::tooling::interaction::active()
        && !crate::frame::stage::active()
    {
        return 0;
    }
    TEXT_SCOPES.with_borrow_mut(|scopes| scopes.push(Scope::new(Phase::Text)));
    1
}

fn exit_text_phase(_token: crate::text::profile::Token) {
    TEXT_SCOPES.with_borrow_mut(|scopes| {
        scopes.pop();
    });
}

fn enter_text_stage(stage: crate::text::profile::TextStage) -> crate::text::profile::Token {
    use crate::frame::stage::{FrameStage, StageBoundary};
    use crate::text::profile::TextStage;
    match stage {
        TextStage::Shape => enter_text_phase(),
        TextStage::GlyphAtlas => enter_stage_boundary(FrameStage::GlyphAtlas, StageBoundary::Begin),
        TextStage::Warm => enter_stage_boundary(FrameStage::TextWarm, StageBoundary::Begin),
        TextStage::SystemFallback => enter_stage_boundary(FrameStage::SystemFontFallback, StageBoundary::Begin),
    }
}

fn exit_text_stage(stage: crate::text::profile::TextStage, token: crate::text::profile::Token) {
    use crate::frame::stage::{FrameStage, StageBoundary};
    use crate::text::profile::TextStage;
    match stage {
        TextStage::Shape => exit_text_phase(token),
        TextStage::GlyphAtlas => crate::frame::stage::emit(FrameStage::GlyphAtlas, StageBoundary::End),
        TextStage::Warm => crate::frame::stage::emit(FrameStage::TextWarm, StageBoundary::End),
        TextStage::SystemFallback => crate::frame::stage::emit(FrameStage::SystemFontFallback, StageBoundary::End),
    }
}

#[inline(always)]
fn enter_stage_boundary(
    stage: crate::frame::stage::FrameStage,
    boundary: crate::frame::stage::StageBoundary,
) -> crate::text::profile::Token {
    if !crate::frame::stage::active() {
        return 0;
    }
    crate::frame::stage::emit(stage, boundary);
    1
}

/// Registers this crate's text timing and stage hooks with `rune-text`, once per process.
pub(crate) fn install_text_hook() {
    static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !INSTALLED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::text::profile::set_text_phase_hooks(enter_text_phase, exit_text_phase);
        crate::text::profile::set_text_stage_hooks(enter_text_stage, exit_text_stage);
    }
}

#[cfg(all(test, feature = "frame-profile"))]
#[path = "profile_tests.rs"]
mod tests;
