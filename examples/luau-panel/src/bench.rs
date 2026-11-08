//! The Luau performance gates, measured on this panel in a headless `Runtime`: the same code
//! natively (`luau-bench`) and in the browser (`luau_bench()`).

use std::fmt;
use std::time::Duration;

use rune::time::Instant;
use rune::{Env, Runtime};
use rune_luau::{Data, Script, Sources};

use crate::{State, sources, view};

/// Iteration counts: the web gets fewer, and reports means over them (browsers coarsen timers).
#[derive(Clone, Copy, PartialEq)]
pub enum Scale {
    Native,
    Web,
}

/// Microseconds.
#[derive(Clone, Copy, Default)]
pub struct Stat {
    pub n: usize,
    pub mean: f64,
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
}

impl Stat {
    fn of(mut samples: Vec<f64>) -> Stat {
        if samples.is_empty() {
            return Stat::default();
        }
        samples.sort_by(f64::total_cmp);
        let n = samples.len();
        let at = |q: f64| samples[((n - 1) as f64 * q).round() as usize];
        Stat { n, mean: samples.iter().sum::<f64>() / n as f64, p50: at(0.5), p99: at(0.99), max: samples[n - 1] }
    }

    fn json(&self) -> String {
        format!(r#"{{"n":{},"mean_us":{:.3},"p50_us":{:.3},"p99_us":{:.3},"max_us":{:.3}}}"#, self.n, self.mean, self.p50, self.p99, self.max)
    }
}

pub struct Report {
    pub native_code: bool,
    /// Idle frames polled, frames any of them asked for, and VM entries they made.
    pub idle: (usize, usize, u64),
    /// `Script::view` on a view rebuilt by host state only, and the VM entries those calls made.
    pub memo_hit: (Stat, u64),
    /// Wall time, and the time the thread ran (Windows).
    pub rebuild20: (Stat, Option<Stat>),
    /// Script time per 120 fps scroll frame, whole frame time, rows `renderRow` built.
    pub scroll: (Stat, Stat, u64),
    /// Compile + swap, compile, swap; whether hot state survived every reload.
    pub reload: (Stat, Stat, Stat, bool),
    pub checks: Vec<crate::checks::Check>,
}

const SIZE: (f32, f32) = (720.0, 640.0);
const DT: f64 = 1.0 / 120.0;

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

pub fn run(scale: Scale) -> Report {
    let n = if scale == Scale::Web { 300 } else { 1000 };
    Report {
        native_code: rune_luau::NATIVE_CODE,
        idle: idle(),
        memo_hit: memo_hit(n),
        rebuild20: rebuild20(n),
        scroll: scroll(),
        reload: reload(if scale == Scale::Web { 10 } else { 20 }),
        checks: crate::checks::run(),
    }
}

/// A runtime showing the panel, polled and framed until nothing is left to do.
fn runtime() -> (Runtime<State>, f64) {
    let mut rt = Runtime::new(State::new(Script::new("panel", sources())), view, SIZE, Env::new(true, 1.0));
    let mut now = 0.0;
    rt.frame(0.0);
    for _ in 0..600 {
        now += DT;
        let stale = rt.update_if(|s, tx| s.script.poll(now, tx));
        if !rt.frame(DT) && !stale {
            break;
        }
    }
    (rt, now)
}

fn idle() -> (usize, usize, u64) {
    let (mut rt, mut now) = runtime();
    let before = rt.state.script.metrics().vm_calls;
    let frames = 1200;
    let mut asked = 0;
    for _ in 0..frames {
        now += DT;
        let stale = rt.update_if(|s, tx| s.script.poll(now, tx));
        if rt.frame(DT) || stale {
            asked += 1;
        }
    }
    (frames, asked, rt.state.script.metrics().vm_calls - before)
}

fn memo_hit(n: usize) -> (Stat, u64) {
    let (mut rt, _) = runtime();
    let mut calls = 0;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        rt.update(|s, _| s.host_ticks += 1);
        rt.frame(DT);
        samples.push(us(rt.state.view_time.get()));
        calls += rt.state.view_calls.get();
    }
    (Stat::of(samples), calls)
}

/// Wall time, and (on Windows) the time the thread actually ran, which leaves out the time
/// other processes on a busy machine held its core.
fn rebuild20(n: usize) -> (Stat, Option<Stat>) {
    let script = Script::new("panel20", sources());
    for _ in 0..50 {
        drop(script.render_uncached::<State>("panel20", true));
    }
    let per_us = cpu::cycles_per_us();
    let (mut wall, mut ran) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
        let (start, cycles) = (Instant::now(), cpu::cycles());
        let el = script.render_uncached::<State>("panel20", true);
        wall.push(us(start.elapsed()));
        if let (Some(per_us), Some(a), Some(b)) = (per_us, cycles, cpu::cycles()) {
            ran.push((b - a) as f64 / per_us);
        }
        drop(el);
    }
    (Stat::of(wall), (!ran.is_empty()).then(|| Stat::of(ran)))
}

/// The thread's cycle count (`QueryThreadCycleTime`), and its rate, measured against the clock.
mod cpu {
    #[cfg(windows)]
    pub fn cycles() -> Option<u64> {
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
        }
        let mut c = 0;
        // SAFETY: the pseudo-handle for this thread, and a valid out pointer.
        (unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) } != 0).then_some(c)
    }

    #[cfg(not(windows))]
    pub fn cycles() -> Option<u64> {
        None
    }

    /// The fastest of a few 20 ms spins: a spin the scheduler interrupted reads slow.
    pub fn cycles_per_us() -> Option<f64> {
        (0..5)
            .filter_map(|_| {
                let (start, c0) = (rune::time::Instant::now(), cycles()?);
                while start.elapsed().as_millis() < 20 {
                    std::hint::spin_loop();
                }
                Some((cycles()? - c0) as f64 / (start.elapsed().as_secs_f64() * 1e6))
            })
            .reduce(f64::max)
    }
}

fn scroll() -> (Stat, Stat, u64) {
    let (mut rt, _) = runtime();
    let rows_before = rt.state.script.metrics().rows;
    let (mut script, mut frame) = (Vec::new(), Vec::new());
    // 40 points a frame at 120 fps (4,800 pt/s): one or two new rows enter most frames.
    for _ in 0..360 {
        let m = rt.state.script.metrics();
        let start = Instant::now();
        rt.wheel(360.0, 400.0, -40.0);
        rt.frame(DT);
        frame.push(us(start.elapsed()));
        script.push(us(rt.state.script.metrics().vm_time - m.vm_time));
    }
    (Stat::of(script), Stat::of(frame), rt.state.script.metrics().rows - rows_before)
}

fn reload(n: usize) -> (Stat, Stat, Stat, bool) {
    let script = Script::new("panel", sources());
    script.call("select", [Data::Number(7.0)]).ok();
    let base = sources();
    let init = base.get("init.luau").unwrap_or_default().to_owned();
    let (mut total, mut compile, mut swap) = (Vec::new(), Vec::new(), Vec::new());
    let mut kept = true;
    for i in 0..n {
        let edited: Sources = base.clone().with("init.luau", format!("{init}\n-- edit {i}\n"));
        match script.reload(edited) {
            Ok(r) => {
                total.push(us(r.compile + r.swap));
                compile.push(us(r.compile));
                swap.push(us(r.swap));
            }
            Err(e) => {
                kept = false;
                eprintln!("reload failed: {e}");
            }
        }
        kept &= script.query("selected", []).ok().and_then(|d| d.as_f64()) == Some(7.0);
    }
    (Stat::of(total), Stat::of(compile), Stat::of(swap), kept)
}

impl Report {
    pub fn json(&self) -> String {
        format!(
            concat!(
                r#"{{"native_code":{},"idle":{{"frames":{},"frames_asked":{},"vm_calls":{}}},"#,
                r#""memo_hit":{{"view":{},"vm_calls":{}}},"rebuild20":{},"rebuild20_thread_time":{},"#,
                r#""scroll":{{"script_per_frame":{},"frame":{},"rows_built":{}}},"#,
                r#""reload":{{"total":{},"compile":{},"swap":{},"state_kept":{}}},"checks":[{}]}}"#
            ),
            self.native_code,
            self.idle.0,
            self.idle.1,
            self.idle.2,
            self.memo_hit.0.json(),
            self.memo_hit.1,
            self.rebuild20.0.json(),
            self.rebuild20.1.map_or("null".to_owned(), |s| s.json()),
            self.scroll.0.json(),
            self.scroll.1.json(),
            self.scroll.2,
            self.reload.0.json(),
            self.reload.1.json(),
            self.reload.2.json(),
            self.reload.3,
            self.checks
                .iter()
                .map(|c| format!(r#"{{"name":{:?},"ok":{},"detail":{:?}}}"#, c.name, c.ok, c.detail))
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let row = |name: &str, s: &Stat, gate: &str| format!("{name:<34} n={:<5} mean {:>9.2} µs  p50 {:>9.2}  p99 {:>9.2}  max {:>9.2}   gate {gate}", s.n, s.mean, s.p50, s.p99, s.max);
        writeln!(f, "Luau {}", if self.native_code { "native code (luau-jit)" } else { "interpreter" })?;
        writeln!(f, "idle: {} frames polled, {} asked for a frame, {} VM calls (gate: 0)", self.idle.0, self.idle.1, self.idle.2)?;
        writeln!(f, "{}  VM calls {}", row("memo hit (Script::view)", &self.memo_hit.0, "≤ 5 µs"), self.memo_hit.1)?;
        writeln!(f, "{}", row("20-row panel rebuild", &self.rebuild20.0, "≤ 120 µs"))?;
        if let Some(s) = &self.rebuild20.1 {
            writeln!(f, "{}", row("20-row panel rebuild, thread time", s, "≤ 120 µs"))?;
        }
        writeln!(f, "{}", row("scroll frame, script time", &self.scroll.0, "≤ 300 µs"))?;
        writeln!(f, "{}  rows built {}", row("scroll frame, whole frame", &self.scroll.1, "—"), self.scroll.2)?;
        writeln!(f, "{}", row("hot reload, compile + swap", &self.reload.0, "≤ 5,000 µs"))?;
        writeln!(f, "{}", row("hot reload, compile", &self.reload.1, "—"))?;
        writeln!(f, "{}  state kept {}", row("hot reload, swap", &self.reload.2, "—"), self.reload.3)?;
        for c in &self.checks {
            writeln!(f, "check {:<24} {}  {}", c.name, if c.ok { "ok  " } else { "FAIL" }, c.detail)?;
        }
        Ok(())
    }
}
