//! The storm bench: N cells (the first argument, default 1,000), windowless at 1000 × 720 pt,
//! scale 2, on a fixed 120 Hz clock; 60 warm frames, then FRAMES counted ones (the second
//! argument, default 240). Prints one CSV line per counted frame:
//!   n,tick,i_poll,i_view,i_rest,ns,a_poll,a_view,a_rest
//! `i_poll` is the app's own step (the clock and every cell's due retargets), `i_view` the View
//! phase (building the tree), `i_rest` the rest of `Runtime::frame` (commit, layout, text, paint,
//! apply), split from the View phase by the phases' CPU time. GPU encode and submit happen
//! outside `Runtime::frame`; the headless run measures them. `share` lines are each phase's mean
//! share of the frame's thread CPU (Commit nests Layout, and Layout nests Text, so they overlap).
//! Build with `--profile perf --features frame-profile`.

use std::cell::Cell;

use rune::{Env, Runtime, Tx};
use storm_race::{SIZE, State, view};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

fn rusage() -> libc::rusage_info_v4 {
    let mut ri: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let r = unsafe {
        libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, &mut ri as *mut _ as *mut libc::rusage_info_t)
    };
    assert_eq!(r, 0, "proc_pid_rusage");
    ri
}
fn instructions() -> u64 { rusage().ri_instructions }
fn allocs() -> u64 { procwatch::alloc::thread_allocations() }

thread_local! {
    /// Instructions and allocations the last frame's poll spent.
    static POLL: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

fn poll(s: &mut State, clock: f64, tx: &mut Tx) -> bool {
    let (i0, a0) = (instructions(), allocs());
    let changed = storm_race::poll(s, clock, tx);
    POLL.set((instructions() - i0, allocs() - a0));
    changed
}

fn main() {
    assert!(!cfg!(debug_assertions), "build with --profile perf");
    let mut args = std::env::args().skip(1);
    let n: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(1000);
    let frames: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(240);
    let probe = { let a = instructions(); let b = instructions(); b - a };
    let mut rt = Runtime::new(State::new(n), view, SIZE, Env::new(true, 2.0));
    rt.poll = Some(poll);
    let mut lines = Vec::new();
    let mut phases = [0.0f64; 6];
    for tick in 0..60 + frames {
        rune::take_frame_phases();
        let c0 = rune::thread_cpu_ms();
        POLL.set((0, 0));
        let t0 = std::time::Instant::now();
        let (a0, i0) = (allocs(), instructions());
        rt.frame(1.0 / 120.0);
        let i1 = instructions();
        let a1 = allocs();
        let ns = t0.elapsed().as_nanos();
        let cpu = rune::thread_cpu_ms() - c0;
        let p = rune::take_frame_phases();
        let q = rune::PHASES;
        let share = |k: usize| if cpu > 0.0 { (p[q + k] / cpu).min(1.0) } else { 0.0 };
        let (poll_i, poll_a) = POLL.get();
        let frame_i = i1 - i0 - poll_i;
        let view_i = (frame_i as f64 * share(0)) as u64;
        if tick >= 60 {
            for (k, ph) in phases.iter_mut().enumerate() { *ph += share(k); }
            lines.push(format!("{n},{},{poll_i},{view_i},{},{ns},{poll_a},0,{}", tick - 60, frame_i - view_i, a1 - a0 - poll_a));
        }
    }
    for l in &lines { println!("{l}") }
    for (k, ph) in ["view", "commit", "layout", "text", "paint", "apply"].iter().enumerate() {
        println!("share,{ph},{:.4}", phases[k] / frames.max(1) as f64);
    }
    println!("elements,{}", rt.elements().len());
    println!("instances,{}", rt.instances.len());
    println!("probe,{probe}");
    println!("peak_footprint,{}", rusage().ri_lifetime_max_phys_footprint);
}
