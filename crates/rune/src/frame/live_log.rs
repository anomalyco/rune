//! `frame-profile` diagnostics for a live window: with `RUNE_FRAME_LOG=FILE`, each frame a
//! window draws is written to FILE as one JSON line, split by stage in nanoseconds: the input
//! handled since the last frame, the drawable wait, the build (view, commit, layout, text,
//! paint, apply), the upload, encode, submit and present, and the thread's CPU. Headless runs
//! write their own log (`profile::Log`). Ordinary builds compile none of this.

use std::cell::{Cell, RefCell};
use std::io::Write;

use super::host_clock::{cpu_ns, now_ns, thread_cpu};
use super::profile::PHASES;

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
}

/// Whether frames are logged in this process.
pub(crate) fn on() -> bool {
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

/// A timer asked for at t, due after wait.
pub(crate) fn waker(at: &'static std::panic::Location<'static>, wait: std::time::Duration) {
    WAKERS.with_borrow_mut(|w| match w.iter_mut().find(|(l, _)| *l == at) {
        Some(e) => e.1 = e.1.min(wait),
        None => w.push((at, wait)),
    });
}

/// Now on the monotonic clock, in ns.
pub(crate) fn now() -> u64 { now_ns() }

/// The clocks at a stage boundary: monotonic and this thread's CPU, in ns.
#[derive(Clone, Copy)]
pub(crate) struct Stamp { pub(crate) wall: u64, pub(crate) cpu: u64 }

impl Stamp {
    pub(crate) fn now() -> Self { Stamp { wall: now_ns(), cpu: cpu_ns(thread_cpu()) as u64 } }
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
        "cpu": f.presented.cpu.saturating_sub(f.entry.cpu), "build_cpu": f.built.cpu.saturating_sub(f.acquired.cpu), "acquire_cpu": f.acquired.cpu.saturating_sub(f.start.cpu), "encode_cpu": f.encoded.cpu.saturating_sub(f.uploaded.cpu), "submit_cpu": f.submitted.cpu.saturating_sub(f.encoded.cpu), "present_cpu": f.presented.cpu.saturating_sub(f.submitted.cpu),
        "nodes": f.nodes, "shaped": f.shaped, "instances": f.instances, "animating": f.animating,
        "why": crate::runtime::CommitWhy::names(f.why),
        "next": next, "poll_due": f.poll_due, "loop_due": f.loop_due, "loops": f.loops, "damage": f.damage, "whole": f.whole, "change": f.change,
    });
    OUT.with_borrow_mut(|out| {
        if let Some(Some(w)) = out.as_mut() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    });
}
