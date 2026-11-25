//! `frame-profile` diagnostics for a live window: with `RUNE_FRAME_LOG=FILE`, each frame a
//! window draws is written to FILE as one JSON line, split by stage in nanoseconds: the input
//! handled since the last frame, the drawable wait, the build (view, commit, layout, text,
//! paint, apply), the upload, encode, submit and present, and the thread's CPU. Headless runs
//! write their own log (`profile::Log`). Ordinary builds compile none of this.

use std::cell::{Cell, RefCell};
use std::io::Write;

use super::host_clock::now_ns;
use super::profile::{PHASES, ThreadCpuMark, thread_cpu_mark};

thread_local! {
    static OUT: RefCell<Option<Option<std::io::BufWriter<std::fs::File>>>> = const { RefCell::new(None) };
    /// Input events handled since the last logged frame, and their time on this thread (ns).
    static INPUT: Cell<(u32, u64)> = const { Cell::new((0, 0)) };
    /// Wakes (updates from other threads, timers) handled since the last logged frame, and their time.
    static WAKE: Cell<(u32, u64)> = const { Cell::new((0, 0)) };
    /// Where frames were asked for since the last logged frame (Win::request_redraw callers).
    /// Tx::wake_after and wake_at callers since the last logged frame, with the shortest wait.
    static WAKERS: RefCell<Vec<(&'static std::panic::Location<'static>, std::time::Duration)>> = const { RefCell::new(Vec::new()) };
    static ASKED: RefCell<Vec<&'static std::panic::Location<'static>>> = const { RefCell::new(Vec::new()) };
    /// Frames missed since the last drawn one (occluded, timeout, outdated, unconfigured), and
    /// surfaces `acquire` reconfigured and got a drawable from.
    static MISSED: Cell<([u32; 4], u32)> = const { Cell::new(([0; 4], 0)) };
    /// Test-only in-memory capture sink for asserting logged frame records without touching disk.
    #[cfg(test)]
    static CAPTURED: RefCell<Option<Vec<serde_json::Value>>> = const { RefCell::new(None) };
}

/// Captures every JSON frame line emitted to [`frame`] on the current thread during `f`.
#[cfg(test)]
pub(crate) fn capture_frames<R>(f: impl FnOnce() -> R) -> (R, Vec<serde_json::Value>) {
    struct Guard(Option<Vec<serde_json::Value>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            CAPTURED.with_borrow_mut(|slot| { *slot = self.0.take(); });
        }
    }
    let prev = CAPTURED.with_borrow_mut(|slot| slot.replace(Vec::new()));
    let mut guard = Guard(prev);
    let result = f();
    let captured = CAPTURED.with_borrow_mut(|slot| {
        let out = slot.take().unwrap_or_default();
        *slot = guard.0.take();
        out
    });
    std::mem::forget(guard);
    (result, captured)
}

/// Whether frames are logged in this process.
pub(crate) fn on() -> bool {
    #[cfg(test)]
    if CAPTURED.with_borrow(|c| c.is_some()) {
        return true;
    }
    OUT.with_borrow_mut(|out| {
        out.get_or_insert_with(|| {
            let path = std::env::var_os("RUNE_FRAME_LOG")?;
            std::fs::File::create(&path).ok().map(std::io::BufWriter::new)
        })
        .is_some()
    })
}

/// An input event handled outside a frame, from `start` (ns) to now.
pub(crate) fn input(start: u64) {
    let (n, ns) = INPUT.get();
    INPUT.set((n + 1, ns + now_ns().saturating_sub(start)));
}

/// A wake (WakeEvent) handled outside a frame, from start (ns) to now.
pub(crate) fn wake(start: u64) {
    let (n, ns) = WAKE.get();
    WAKE.set((n + 1, ns + now_ns().saturating_sub(start)));
}

/// A frame asked for at t.
pub(crate) fn asked(at: &'static std::panic::Location<'static>) {
    ASKED.with_borrow_mut(|a| if !a.contains(&at) { a.push(at) });
}

/// A frame that got no drawable: `kind` 0 occluded, 1 timeout, 2 outdated, 3 unconfigured.
pub(crate) fn missed(kind: usize) {
    let (mut m, r) = MISSED.get();
    m[kind.min(3)] += 1;
    MISSED.set((m, r));
}

/// `acquire` found its surface outdated, reconfigured it, and went on.
pub(crate) fn reconfigured() {
    let (m, r) = MISSED.get();
    MISSED.set((m, r + 1));
}

/// A timer asked for at t, due after wait.
pub(crate) fn waker(at: &'static std::panic::Location<'static>, wait: std::time::Duration) {
    WAKERS.with_borrow_mut(|w| match w.iter_mut().find(|(l, _)| *l == at) {
        Some(e) => e.1 = e.1.min(wait),
        None => w.push((at, wait)),
    });
}

/// Now on the monotonic clock, in ns.
pub(crate) fn now() -> u64 { now_ns() }

/// The clocks at a stage boundary: monotonic in ns and this thread's raw CPU mark, converted
/// when the frame is logged so stamps taken before Windows knows the cycles' rate still measure
/// from that boundary once it does.
#[derive(Clone, Copy)]
pub(crate) struct Stamp { pub(crate) wall: u64, pub(crate) cpu: Option<ThreadCpuMark> }

impl Stamp {
    pub(crate) fn now() -> Self { Stamp { wall: now_ns(), cpu: thread_cpu_mark() } }
}

fn cpu_delta(a: Stamp, b: Stamp) -> Option<u64> {
    a.cpu.zip(b.cpu).and_then(|(a, b)| a.delta_ns(b)).map(|ns| ns as u64)
}

/// One drawn frame's stage boundaries and counts.
pub(crate) struct Frame {
    /// The draw began: a timer's frame builds before it takes a drawable (prebuild).
    pub(crate) entry: Stamp,
    pub(crate) start: Stamp,
    pub(crate) acquired: Stamp,
    pub(crate) built: Stamp,
    pub(crate) uploaded: Stamp,
    pub(crate) encoded: Stamp,
    pub(crate) submitted: Stamp,
    pub(crate) presented: Stamp,
    /// Wall ms per `profile::Phase` (view, commit, layout, text, paint, apply), then CPU ms.
    pub(crate) phases: [f64; 2 * PHASES],
    pub(crate) nodes: usize,
    pub(crate) shaped: u32,
    pub(crate) instances: usize,
    pub(crate) animating: bool,
    pub(crate) why: u16,
    /// Why the window draws another frame: motion, a window frame moving, a commit due, a
    /// relayout or queued action, pending input, a live value, a frame request, a timer due by the next tick.
    pub(crate) next: u16,
    /// The poll's and motion's next timers, in seconds after the last poll.
    pub(crate) poll_due: Option<f64>,
    pub(crate) loop_due: Option<f64>,
    /// The drawn nodes with a running loop, while one wakes the window.
    pub(crate) loops: Vec<String>,
    /// The rectangle the frame presented (`[x0, y0, x1, y1]`), or None for all of it.
    pub(crate) damage: Option<Vec<[i32; 4]>>,
    /// Why it drew all of the window (an effect, a layer, a changed image, or most of it changed).
    pub(crate) whole: Option<&'static str>,
    /// The bounds of what changed (`[x0, y0, x1, y1]`), however much it drew.
    pub(crate) change: Option<[u32; 4]>,
    /// Only its loops stepped (`Runtime::loop_step`): the instances it wrote, else None.
    pub(crate) looped: Option<usize>,
    /// Why it wasn't loop-only, when only its loops might have asked for it.
    pub(crate) loop_why: Option<&'static str>,
    /// `RUNE_LOOP_CHECK=1`: the instances that differ from what the loop-only step would have
    /// drawn, and the largest difference.
    pub(crate) loop_check: Option<(usize, f32)>,
}

pub(crate) fn frame(f: Frame) {
    let (inputs, input_ns) = INPUT.replace((0, 0));
    let (wakes, wake_ns) = WAKE.replace((0, 0));
    let wakers: Vec<String> = WAKERS.with_borrow_mut(std::mem::take).iter().map(|(l, d)| format!("{}:{} {}ms", l.file(), l.line(), d.as_millis())).collect();
    let asked: Vec<String> = ASKED.with_borrow_mut(std::mem::take).iter().map(|l| format!("{}:{}", l.file(), l.line())).collect();
    let d = |a: Stamp, b: Stamp| b.wall.saturating_sub(a.wall);
    let ns = |ms: f64| (ms * 1e6) as u64;
    let p = &f.phases;
    let next: Vec<&str> = ["motion", "frame_move", "commit", "relayout_or_queue", "input", "live", "window_frame", "timer"]
        .into_iter().enumerate().filter(|(i, _)| f.next & (1 << i) != 0).map(|(_, n)| n).collect();
    let line = serde_json::json!({
        "t": f.entry.wall, "inputs": inputs, "input": input_ns, "wakes": wakes, "wake": wake_ns, "asked": asked, "wakers": wakers,
        "prebuild": d(f.entry, f.start), "acquire": d(f.start, f.acquired), "build": d(f.acquired, f.built),
        "view": ns(p[0]), "commit": ns(p[1]), "layout": ns(p[2]), "text": ns(p[3]), "paint": ns(p[4]), "apply": ns(p[5]),
        "upload": d(f.built, f.uploaded), "encode": d(f.uploaded, f.encoded), "submit": d(f.encoded, f.submitted),
        "present": d(f.submitted, f.presented), "total": d(f.entry, f.presented),
        "cpu": cpu_delta(f.entry, f.presented), "build_cpu": cpu_delta(f.acquired, f.built), "acquire_cpu": cpu_delta(f.start, f.acquired), "encode_cpu": cpu_delta(f.uploaded, f.encoded), "submit_cpu": cpu_delta(f.encoded, f.submitted), "present_cpu": cpu_delta(f.submitted, f.presented),
        "nodes": f.nodes, "shaped": f.shaped, "instances": f.instances, "animating": f.animating,
        "why": crate::runtime::CommitWhy::names(f.why),
        "next": next, "poll_due": f.poll_due, "loop_due": f.loop_due, "loops": f.loops, "damage": f.damage, "whole": f.whole, "change": f.change,
    });
    // Apart from the rest: one more key there passes `json!`'s recursion limit.
    let mut line = line;
    line["looped"] = serde_json::json!(f.looped);
    line["loop_why"] = serde_json::json!(f.loop_why);
    let (missed, reconfigured) = MISSED.replace(([0; 4], 0));
    if missed != [0; 4] || reconfigured > 0 { line["missed"] = serde_json::json!({ "occluded": missed[0], "timeout": missed[1], "outdated": missed[2], "unconfigured": missed[3], "reconfigured": reconfigured }); }
    line["loop_check"] = serde_json::json!(f.loop_check.map(|(n, d)| [n as f64, if d.is_finite() { f64::from(d) } else { -1.0 }]));
    #[cfg(test)]
    CAPTURED.with_borrow_mut(|slot| {
        if let Some(lines) = slot.as_mut() {
            lines.push(line.clone());
        }
    });
    OUT.with_borrow_mut(|out| {
        if let Some(Some(w)) = out.as_mut() {
            writeln!(w, "{line}").ok();
            w.flush().ok();
        }
    });
}

#[cfg(test)]
#[path = "live_log_tests.rs"]
mod tests;
