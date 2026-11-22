//! Proof program for the watchdog's CPU-time budget (proof branch only, never merged). On this machine:
//! `while true do end` is stopped after 250 ms of its thread's CPU time, on a P-core and pinned to an E-core,
//! alone and sharing its core with a busy thread; a script that waits 2 s in a host call is not stopped.
//!   proof_watchdog [p-core index 0] [e-core index 19] [rounds 5]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rune_luau::{Script, ScriptOptions, Sources};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThread() -> isize;
    fn SetThreadAffinityMask(thread: isize, mask: usize) -> usize;
    fn SetThreadPriority(thread: isize, priority: i32) -> i32;
}

fn pin(core: usize) {
    // SAFETY: the pseudo-handle names the calling thread; the mask names one existing logical CPU.
    unsafe { SetThreadAffinityMask(GetCurrentThread(), 1usize << core) };
}

/// A busy thread on `core` at `priority` (0 normal, 1 above normal) until `stop`.
fn hog(core: usize, priority: i32, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        pin(core);
        // SAFETY: as in `pin`; the priority is a documented THREAD_PRIORITY value.
        unsafe { SetThreadPriority(GetCurrentThread(), priority) };
        let mut x = 0u64;
        while !stop.load(Ordering::Relaxed) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let p = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0usize);
    let e = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(19usize);
    let rounds = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5usize);
    let mut sources = Sources::new();
    sources.insert("init.luau", "local M = {}\nfunction M.busy() while true do end end\nfunction M.nap() sleep_ms(2000) return 1 end\nreturn M\n");
    let options = ScriptOptions::new().setup(|host| {
        let f = host.lua().create_function(|_, ms: u64| {
            std::thread::sleep(Duration::from_millis(ms));
            Ok(())
        })?;
        host.global("sleep_ms", f)
    });
    let script = Script::with_options("proof", sources, options);
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let case = |label: &str, core: usize, hog_priority: Option<i32>, f: &str| {
        pin(core);
        for r in 0..rounds {
            let stop = Arc::new(AtomicBool::new(false));
            let h = hog_priority.map(|pr| hog(core, pr, stop.clone()));
            std::thread::sleep(Duration::from_millis(50));
            let (w0, c0) = (Instant::now(), rune::thread_cpu_ns().unwrap_or(0.0));
            let out = script.query(f, Vec::<rune_luau::Data>::new());
            let (wall, cpu) = (w0.elapsed(), rune::thread_cpu_ns().unwrap_or(0.0) - c0);
            stop.store(true, Ordering::Relaxed);
            if let Some(h) = h { h.join().unwrap() }
            let what = match &out { Ok(_) => "returned".to_owned(), Err(e) => format!("{:?}", e.kind) };
            println!("{label} round {r}: {f}: {what} after {} µs wall, {} µs of the thread's CPU", wall.as_micros(), (cpu / 1000.0) as u64);
        }
    };
    case(&format!("P-core {p}, alone"), p, None, "busy");
    case(&format!("E-core {e}, alone"), e, None, "busy");
    case(&format!("E-core {e}, sharing with a normal-priority busy thread"), e, Some(0), "busy");
    case(&format!("E-core {e}, sharing with an above-normal busy thread"), e, Some(1), "busy");
    case(&format!("P-core {p}, alone"), p, None, "nap");
}
