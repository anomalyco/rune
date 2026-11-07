//! The frame meter both builds feed, one call per drawn frame, so their numbers mean the same.
//!
//! It keeps each frame's start time, its CPU work (whatever span the build can time: see each
//! build's README) and, where a build can split it out, the part that builds the frame: view,
//! layout and paint, without the GPU encode. From those it derives frame-start intervals, missed frames (an interval over
//! 1.5 refresh periods) and percentiles, and it samples the process's CPU time and memory from
//! procwatch.

use std::time::Instant;

#[derive(Clone, Copy, Debug)]
struct Frame {
    /// Seconds since the meter started.
    start: f64,
    work_ms: f32,
    /// NaN when the build can't split it out.
    build_ms: f32,
    /// The show time and load the app tagged the frame with ([`Meter::tag`]).
    show: f32,
    load: u32,
    /// CPU time since the frame before: this thread's, and the whole process's. Unlike the
    /// spans above, a busy machine that preempts the app doesn't add to them.
    cpu_ms: f32,
    proc_ms: f32,
}

/// An interval longer than this is the window resting, not a slow frame.
const IDLE_GAP: f64 = 0.1;

pub struct Meter {
    framework: String,
    refresh_hz: f64,
    t0: Instant,
    cpu0: f64,
    frames: Vec<Frame>,
    footprint_peak: u64,
    footprint_last: u64,
    footprint_at: f64,
    resident_last: u64,
    /// Seconds since the meter started that bound what [`Meter::summary`] counts.
    window: Option<(f64, f64)>,
    extra: Vec<(String, String)>,
    tag: (f32, u32),
    /// Thread and process CPU seconds at the last frame.
    cpu_last: (f64, f64),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub seconds: f64,
    pub fps: f64,
    pub interval_ms: [f64; 4],
    pub work_ms: [f64; 4],
    /// View, layout and paint only; `None` when no frame reported it.
    pub build_ms: Option<[f64; 4]>,
    /// Intervals over 1.5 refresh periods.
    pub late: usize,
    /// Refreshes those late intervals skipped.
    pub dropped: usize,
    pub idle_gaps: usize,
    pub cpu_percent: f64,
    pub footprint_mb: f64,
    pub footprint_peak_mb: f64,
    pub resident_mb: f64,
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

/// p50, p95, p99 and max of `v`.
fn spread(mut v: Vec<f64>) -> [f64; 4] {
    v.sort_by(f64::total_cmp);
    [pct(&v, 0.5), pct(&v, 0.95), pct(&v, 0.99), v.last().copied().unwrap_or(0.0)]
}

fn cpu_seconds() -> f64 {
    procwatch::snapshot().map_or(0.0, |s| s.cpu_s)
}

/// This thread's CPU seconds, and the process's (user and system), cheaply enough for every frame.
fn cpu_now() -> (f64, f64) {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: both calls only write the struct they're given.
    let thread = if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } == 0 { ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9 } else { f64::NAN };
    let mut ru = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: as above.
    let process = if unsafe { libc::getrusage(libc::RUSAGE_SELF, ru.as_mut_ptr()) } == 0 {
        // SAFETY: getrusage filled it.
        let ru = unsafe { ru.assume_init() };
        let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 * 1e-6;
        tv(ru.ru_utime) + tv(ru.ru_stime)
    } else {
        f64::NAN
    };
    (thread, process)
}

impl Meter {
    pub fn new(framework: &str, refresh_hz: f64) -> Meter {
        let mut m = Meter {
            framework: framework.into(),
            refresh_hz,
            t0: Instant::now(),
            cpu0: cpu_seconds(),
            frames: Vec::with_capacity(8192),
            footprint_peak: 0,
            footprint_last: 0,
            footprint_at: -1.0,
            resident_last: 0,
            window: None,
            extra: Vec::new(),
            tag: (f32::NAN, 0),
            cpu_last: cpu_now(),
        };
        m.sample_memory(0.0);
        m
    }

    pub fn set_refresh(&mut self, hz: f64) {
        if hz > 1.0 {
            self.refresh_hz = hz;
        }
    }

    pub fn refresh_hz(&self) -> f64 {
        self.refresh_hz
    }

    /// Seconds since the meter started.
    pub fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    pub fn seconds_at(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.t0).as_secs_f64()
    }

    /// Restarts the CPU count from here, and counts only frames from now on.
    pub fn begin(&mut self) {
        let now = self.now();
        self.cpu0 = cpu_seconds();
        self.window = Some((now, f64::INFINITY));
    }

    /// Stops counting frames from here.
    pub fn end(&mut self) {
        let now = self.now();
        if let Some((from, _)) = self.window {
            self.window = Some((from, now));
        }
    }

    /// Adds a line to the JSON (a build's version, how it timed work).
    pub fn note(&mut self, key: &str, value: &str) {
        self.extra.push((key.into(), value.into()));
    }

    /// One drawn frame that started at `start` and took `work_ms` of CPU.
    pub fn frame(&mut self, start: Instant, work_ms: f64) {
        self.push(start, work_ms, f32::NAN);
    }

    /// One drawn frame whose `work_ms` includes `build_ms` of view, layout and paint.
    pub fn frame_with_build(&mut self, start: Instant, work_ms: f64, build_ms: f64) {
        self.push(start, work_ms, build_ms as f32);
    }

    /// Tags the frames reported from here with the show time and load they drew.
    pub fn tag(&mut self, show: f64, load: usize) {
        self.tag = (show as f32, load as u32);
    }

    fn push(&mut self, start: Instant, work_ms: f64, build_ms: f32) {
        let s = self.seconds_at(start);
        let (show, load) = self.tag;
        let cpu = cpu_now();
        let (cpu_ms, proc_ms) = (((cpu.0 - self.cpu_last.0) * 1000.0) as f32, ((cpu.1 - self.cpu_last.1) * 1000.0) as f32);
        self.cpu_last = cpu;
        self.frames.push(Frame { start: s, work_ms: work_ms as f32, build_ms, show, load, cpu_ms, proc_ms });
        if s - self.footprint_at >= 0.5 {
            self.sample_memory(s);
        }
    }

    fn sample_memory(&mut self, at: f64) {
        let u = procwatch::usage();
        self.footprint_last = u.footprint.unwrap_or(0);
        self.resident_last = u.resident.unwrap_or(0);
        self.footprint_peak = self.footprint_peak.max(self.footprint_last);
        self.footprint_at = at;
    }

    fn counted(&self) -> &[Frame] {
        let Some((from, to)) = self.window else { return &self.frames };
        let a = self.frames.partition_point(|f| f.start < from);
        let b = self.frames.partition_point(|f| f.start <= to);
        &self.frames[a..b]
    }

    fn summarise(&self, frames: &[Frame], seconds: f64, cpu_s: f64) -> Summary {
        let period = 1.0 / self.refresh_hz;
        let mut intervals = Vec::with_capacity(frames.len());
        let (mut late, mut dropped, mut idle_gaps) = (0, 0, 0);
        for w in frames.windows(2) {
            let iv = w[1].start - w[0].start;
            if iv > IDLE_GAP {
                idle_gaps += 1;
                continue;
            }
            intervals.push(iv * 1000.0);
            if iv > 1.5 * period {
                late += 1;
                dropped += ((iv / period).round() as usize).saturating_sub(1);
            }
        }
        let mb = |b: u64| b as f64 / 1_048_576.0;
        let builds: Vec<f64> = frames.iter().filter(|f| !f.build_ms.is_nan()).map(|f| f.build_ms as f64).collect();
        Summary {
            frames: frames.len(),
            seconds,
            fps: if seconds > 0.0 { frames.len() as f64 / seconds } else { 0.0 },
            interval_ms: spread(intervals),
            work_ms: spread(frames.iter().map(|f| f.work_ms as f64).collect()),
            build_ms: (!builds.is_empty()).then(|| spread(builds)),
            late,
            dropped,
            idle_gaps,
            cpu_percent: if seconds > 0.0 { 100.0 * cpu_s / seconds } else { 0.0 },
            footprint_mb: mb(self.footprint_last),
            footprint_peak_mb: mb(self.footprint_peak),
            resident_mb: mb(self.resident_last),
        }
    }

    /// The whole counted window. Reads the process's CPU time, so call it once at the end.
    pub fn summary(&mut self) -> Summary {
        let now = self.now();
        self.sample_memory(now);
        let (from, to) = self.window.unwrap_or((0.0, now));
        let seconds = to.min(now) - from;
        let cpu = cpu_seconds() - self.cpu0;
        self.summarise(self.counted(), seconds, cpu)
    }

    /// The last second, for the HUD. CPU% there is left at 0: reading it costs a thread walk.
    pub fn recent(&self) -> Summary {
        let now = self.now();
        let a = self.frames.partition_point(|f| f.start < now - 1.0);
        let mut s = self.summarise(&self.frames[a..], 1.0, 0.0);
        let total = self.summarise(self.counted(), 1.0, 0.0);
        s.late = total.late;
        s.dropped = total.dropped;
        s
    }

    /// Short HUD lines.
    pub fn hud(&self) -> [String; 4] {
        let s = self.recent();
        [
            format!("{} · {:.0} Hz display", self.framework, self.refresh_hz),
            format!("{:.0} fps · interval p95 {:.1} ms", s.fps, s.interval_ms[1]),
            format!("work p50 {:.2} · p95 {:.2} ms", s.work_ms[0], s.work_ms[1]),
            format!("missed {} · {:.0} MB", s.dropped, s.footprint_mb),
        ]
    }

    /// The summary as JSON, with the build's notes.
    pub fn json(&mut self) -> String {
        let s = self.summary();
        let arr = |v: [f64; 4]| format!("{{\"p50\": {:.3}, \"p95\": {:.3}, \"p99\": {:.3}, \"max\": {:.3}}}", v[0], v[1], v[2], v[3]);
        let mut out = String::from("{\n");
        out.push_str(&format!("  \"framework\": \"{}\",\n", self.framework));
        out.push_str(&format!("  \"refresh_hz\": {:.2},\n", self.refresh_hz));
        out.push_str(&format!("  \"seconds\": {:.3},\n", s.seconds));
        out.push_str(&format!("  \"frames\": {},\n", s.frames));
        out.push_str(&format!("  \"fps\": {:.2},\n", s.fps));
        out.push_str(&format!("  \"interval_ms\": {},\n", arr(s.interval_ms)));
        out.push_str(&format!("  \"work_ms\": {},\n", arr(s.work_ms)));
        if let Some(b) = s.build_ms {
            out.push_str(&format!("  \"build_ms\": {},\n", arr(b)));
        }
        out.push_str(&format!("  \"late_intervals\": {},\n", s.late));
        out.push_str(&format!("  \"dropped_frames\": {},\n", s.dropped));
        out.push_str(&format!("  \"idle_gaps\": {},\n", s.idle_gaps));
        out.push_str(&format!("  \"cpu_percent\": {:.2},\n", s.cpu_percent));
        out.push_str(&format!("  \"footprint_mb\": {:.2},\n", s.footprint_mb));
        out.push_str(&format!("  \"footprint_peak_mb\": {:.2},\n", s.footprint_peak_mb));
        out.push_str(&format!("  \"resident_mb\": {:.2}", s.resident_mb));
        for (k, v) in &self.extra {
            out.push_str(&format!(",\n  \"{k}\": \"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")));
        }
        out.push_str("\n}\n");
        out
    }

    /// The last `n` frames' show time and build time (work where the build can't split it).
    pub fn tail(&self, n: usize) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.frames[self.frames.len().saturating_sub(n)..].iter().map(|f| (f.show, if f.build_ms.is_nan() { f.work_ms } else { f.build_ms }))
    }

    /// Every frame as CSV, one line each: `show_s,load,build_ms,work_ms,start_s,cpu_ms,proc_ms`
    /// (`cpu_ms` and `proc_ms` are the CPU time since the frame before, thread and process).
    pub fn frames_csv(&self) -> String {
        let mut out = String::from("show_s,load,build_ms,work_ms,start_s,cpu_ms,proc_ms\n");
        for f in &self.frames {
            out.push_str(&format!("{:.4},{},{:.4},{:.4},{:.4},{:.4},{:.4}\n", f.show, f.load, f.build_ms, f.work_ms, f.start, f.cpu_ms, f.proc_ms));
        }
        out
    }

    pub fn write(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let json = self.json();
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn counts_late_frames_against_the_refresh() {
        let mut m = Meter::new("test", 120.0);
        let t0 = m.t0;
        let period = Duration::from_secs_f64(1.0 / 120.0);
        let mut at = t0;
        for i in 0..240 {
            // Every 60th frame takes three refreshes.
            at += if i % 60 == 59 { period * 3 } else { period };
            m.frame_with_build(at, 2.0, 1.5);
        }
        // A rest is not a miss.
        m.frame(at + Duration::from_millis(400), 2.0);
        let s = m.summarise(&m.frames, 2.0, 0.0);
        assert_eq!(s.late, 4);
        assert_eq!(s.dropped, 8);
        assert_eq!(s.idle_gaps, 1);
        assert!((s.interval_ms[0] - 1000.0 / 120.0).abs() < 0.01);
        assert_eq!(s.build_ms.map(|b| b[0]), Some(1.5));
        assert!(m.json().contains("\"dropped_frames\""));
    }
}
