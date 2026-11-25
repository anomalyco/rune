#![allow(unsafe_code)]
//! Counting-allocator regression tests for memory retained after closing a large Markdown view
//! and resting (issue #2377).

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

use std::sync::Mutex;

use rune::{Env, Runtime, ScrollSpec, col, el, text};
use rune_kit::{Kit, Markdown};

/// Serialise heap measurements across tests sharing the process-wide counting allocator.
static SERIAL: Mutex<()> = Mutex::new(());

fn heap() -> procwatch::alloc::Heap {
    procwatch::alloc::heap().unwrap_or_default()
}

fn mb(bytes: i64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[cfg(target_os = "linux")]
fn linux_vm_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}



#[path = "../examples/markdown_large_bench/fixture.rs"]
mod fixture;
use fixture::generate_large_guide;

struct AppState {
    doc: Option<String>,
}

fn view(s: &AppState, cx: &mut rune::Cx<AppState>) -> rune::El {
    let k = Kit::neutral(cx.system_dark());
    match &s.doc {
        Some(md) => col()
            .key("viewer")
            .w_full()
            .h_full()
            .scroll(ScrollSpec::default())
            .child(Markdown::new(md.as_str()).selectable(true).build(&k)),
        None => col()
            .key("start-page")
            .w_full()
            .h_full()
            .center()
            .child(text("Open a Markdown file").key("title"))
            .child(el().key("hint").child(text("Press Ctrl+O to open"))),
    }
}

fn settle_exits(rt: &mut Runtime<AppState>) {
    for _ in 0..120 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
}

#[test]
fn large_markdown_close_and_rest_returns_near_start_page_baseline() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut rt = Runtime::new(AppState { doc: None }, view, (900.0, 700.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle_exits(&mut rt);
    let baseline = heap();
    #[cfg(target_os = "linux")]
    let rss_start = linux_vm_rss_kib();
    eprintln!(
        "baseline: live={:.2} MiB allocs={} layouts={}",
        mb(baseline.live),
        baseline.allocations,
        rt.text.cached_layout_count()
    );

    let md = generate_large_guide();
    let t_open = rune::time::now();
    rt.update(|s, _| s.doc = Some(md));
    rt.frame(1.0 / 60.0);
    let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    let loaded = heap();
    let loaded_instances = rt.instances.len();
    #[cfg(target_os = "linux")]
    let rss_loaded = linux_vm_rss_kib();
    eprintln!(
        "loaded:   live={:.2} MiB peak={:.2} MiB allocs={} layouts={} inst={} open={:.1}ms",
        mb(loaded.live),
        mb(loaded.peak),
        loaded.allocations,
        rt.text.cached_layout_count(),
        loaded_instances,
        open_ms
    );

    // Close document, commit start page once, and let exit transitions settle.
    rt.update(|s, _| s.doc = None);
    rt.frame(1.0 / 60.0);
    settle_exits(&mut rt);
    let closed = heap();
    #[cfg(target_os = "linux")]
    let rss_closed = linux_vm_rss_kib();
    eprintln!(
        "closed:   live={:.2} MiB layouts={} inst={}",
        mb(closed.live),
        rt.text.cached_layout_count(),
        rt.instances.len()
    );

    // Trigger rest once (as Host::give_back does after REST_AFTER).
    rt.rest();
    let rested = heap();
    #[cfg(target_os = "linux")]
    let rss_rested_before_trim = linux_vm_rss_kib();
    eprintln!(
        "rested:   live={:.2} MiB (delta {:+.2} MiB) layouts={}",
        mb(rested.live),
        mb(rested.live - baseline.live),
        rt.text.cached_layout_count()
    );

    // 30 s more idle: no growth, rest does not churn.
    rt.frame(30.0);
    let idle30 = heap();
    eprintln!(
        "idle30s:  live={:.2} MiB (delta {:+.2} MiB) layouts={}",
        mb(idle30.live),
        mb(idle30.live - baseline.live),
        rt.text.cached_layout_count()
    );

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        assert!(
            rune::__heap_trim::uses_glibc_allocator(),
            "Counting::system() on Linux/glibc must be recognized as glibc-backed"
        );
        rune::__heap_trim::wait_idle();
        rune::__heap_trim::reset_cooldown();
        let calls_before = rune::__heap_trim::trim_calls();
        // Trigger Rune's own rest-time heap trim (same entrypoint as Host::give_back, no manual
        // malloc_trim call in this test).
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        let calls_after = rune::__heap_trim::trim_calls();
        let trim_dur = rune::__heap_trim::last_trim_duration();
        let rss_after_trim = linux_vm_rss_kib();
        assert_eq!(calls_after, calls_before + 1, "rest should trigger exactly one guarded trim");
        let start_mib = rss_start.unwrap_or(0) as f64 / 1024.0;
        let rested_mib = rss_rested_before_trim.unwrap_or(0) as f64 / 1024.0;
        let after_mib = rss_after_trim.unwrap_or(0) as f64 / 1024.0;
        eprintln!(
            "linux VmRSS (MiB): start={start_mib:.2} loaded={:.2} closed={:.2} rested_before_trim={rested_mib:.2} after_rune_rest_trim={after_mib:.2} (trim_ms={:.2})",
            rss_loaded.unwrap_or(0) as f64 / 1024.0,
            rss_closed.unwrap_or(0) as f64 / 1024.0,
            trim_dur.as_secs_f64() * 1000.0
        );
        assert!(
            after_mib <= start_mib + 15.0,
            "VmRSS after Rune rest trim ({after_mib:.2} MiB) exceeded start-page baseline ({start_mib:.2} MiB) + 15 MiB"
        );
    }

    // Reopen after rest: must render identically and cost no more live heap than first open (+4 MiB).
    let md2 = generate_large_guide();
    let t_reopen = rune::time::now();
    rt.update(|s, _| s.doc = Some(md2));
    rt.frame(1.0 / 60.0);
    let reopen_ms = t_reopen.elapsed().as_secs_f64() * 1000.0;
    let reopened = heap();
    eprintln!(
        "reopened: live={:.2} MiB inst={} reopen={:.1}ms",
        mb(reopened.live),
        rt.instances.len(),
        reopen_ms
    );

    // Post-rest live heap must be within 2 MiB of the start-page baseline and layouts <= 4.
    let delta = rested.live - baseline.live;
    assert!(
        delta <= 2 << 20,
        "after close + rest, live heap remained {:+.2} MiB above start-page baseline ({:.2} vs {:.2} MiB)",
        mb(delta),
        mb(rested.live),
        mb(baseline.live)
    );
    assert!(
        rt.instances.len() == loaded_instances,
        "reopened guide draws same instance count ({} vs {})",
        rt.instances.len(),
        loaded_instances
    );
    assert!(
        reopened.live <= loaded.live + (4 << 20),
        "reopen live heap {:.2} MiB should not exceed first open {:.2} MiB + 4 MiB",
        mb(reopened.live),
        mb(loaded.live)
    );

    // Clean up thread-local caches before releasing SERIAL lock.
    rt.update(|s, _| s.doc = None);
    rt.frame(1.0 / 60.0);
    settle_exits(&mut rt);
    rt.rest();
}

#[test]
fn rest_is_idempotent_with_zero_allocations_on_repeat() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut rt = Runtime::new(AppState { doc: None }, view, (900.0, 700.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.doc = Some(generate_large_guide()));
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.doc = None);
    rt.frame(1.0 / 60.0);
    settle_exits(&mut rt);

    rt.rest();
    let live_after_first = heap().live;
    let allocs_before = procwatch::alloc::thread_allocations();
    rt.rest();
    let allocs_after = procwatch::alloc::thread_allocations();
    let live_after_second = heap().live;

    assert_eq!(allocs_after, allocs_before, "repeated rest() allocates nothing");
    assert_eq!(live_after_second, live_after_first, "repeated rest() changes no heap bytes");
}

#[test]
fn active_tab_flip_without_rest_keeps_two_commit_grace_and_avoids_reshaping() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let doc = generate_large_guide();
    let mut rt = Runtime::new(AppState { doc: Some(doc.clone()) }, view, (900.0, 700.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let layouts_open = rt.text.cached_layout_count();
    assert!(layouts_open > 1000, "guide shaped layouts: {layouts_open}");

    // Flip to start page (1 commit, no rest).
    rt.update(|s, tx| {
        tx.instant();
        s.doc = None;
    });
    rt.frame(1.0 / 60.0);
    let shaped_before_flip_back = rt.text.shaped;

    // Flip back to the guide without resting: two-commit grace must reuse cached layouts.
    rt.update(|s, tx| {
        tx.instant();
        s.doc = Some(doc);
    });
    rt.frame(1.0 / 60.0);
    let reshaped = rt.text.shaped - shaped_before_flip_back;
    assert_eq!(reshaped, 0, "active tab flip without rest must reshape 0 layouts (got {reshaped})");

    // Clean up.
    rt.update(|s, tx| {
        tx.instant();
        s.doc = None;
    });
    rt.frame(1.0 / 60.0);
    rt.rest();
}

#[test]
fn rest_while_document_is_shown_keeps_current_layouts_warm() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let doc = "# Heading\n\nParagraph with **bold** and `code`.\n\n```rust\nfn main() {}\n```\n";
    let mut rt = Runtime::new(AppState { doc: Some(doc.to_owned()) }, view, (900.0, 700.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let layouts_before = rt.text.cached_layout_count();
    assert!(layouts_before >= 3);

    rt.rest();
    assert_eq!(rt.text.cached_layout_count(), layouts_before, "rest keeps current document layouts");
    let shaped_before = rt.text.shaped;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text.shaped, shaped_before, "redrawing after rest on same document reshapes nothing");

    rt.update(|s, tx| {
        tx.instant();
        s.doc = None;
    });
    rt.frame(1.0 / 60.0);
    rt.rest();
}

#[test]
#[ignore = "manual perf-profile benchmark for paired before/after timing comparison"]
fn perf_active_and_rest_timings() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let doc = generate_large_guide();
    let mut rt = Runtime::new(AppState { doc: None }, view, (900.0, 700.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let t0 = rune::time::now();
    rt.update(|s, tx| {
        tx.instant();
        s.doc = Some(doc.clone());
    });
    rt.frame(1.0 / 60.0);
    let first_frame_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // Scroll 20 steps through the document.
    let t1 = rune::time::now();
    for _ in 0..20 {
        rt.wheel(450.0, 350.0, -400.0);
        rt.frame(1.0 / 60.0);
    }
    let scroll_ms = t1.elapsed().as_secs_f64() * 1000.0;

    // Active tab flip (doc -> start -> doc) without rest.
    let t2 = rune::time::now();
    rt.update(|s, tx| {
        tx.instant();
        s.doc = None;
    });
    rt.frame(1.0 / 60.0);
    rt.update(|s, tx| {
        tx.instant();
        s.doc = Some(doc.clone());
    });
    rt.frame(1.0 / 60.0);
    let flip_ms = t2.elapsed().as_secs_f64() * 1000.0;

    // Close + rest + reopen.
    rt.update(|s, tx| {
        tx.instant();
        s.doc = None;
    });
    rt.frame(1.0 / 60.0);
    let t_rest = rune::time::now();
    rt.rest();
    let rest_ms = t_rest.elapsed().as_secs_f64() * 1000.0;

    let t3 = rune::time::now();
    rt.update(|s, tx| {
        tx.instant();
        s.doc = Some(doc);
    });
    rt.frame(1.0 / 60.0);
    let reopen_ms = t3.elapsed().as_secs_f64() * 1000.0;

    eprintln!(
        "PERF first_frame={first_frame_ms:.2}ms scroll20={scroll_ms:.2}ms active_flip={flip_ms:.2}ms rest={rest_ms:.2}ms reopen={reopen_ms:.2}ms"
    );
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
#[ignore = "manual perf-profile benchmark measuring malloc_trim duration, RSS drop, and cross-thread arena wait"]
fn perf_malloc_trim_contention_and_scale() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    assert!(rune::__heap_trim::uses_glibc_allocator());

    fn measure_scale(label: &str, copies: usize) {
        let mut rt = Runtime::new(AppState { doc: None }, view, (900.0, 700.0), Env::new(true, 1.0));
        rt.frame(0.0);
        settle_exits(&mut rt);
        // Prime and trim once so start RSS reflects a clean start page.
        rune::__heap_trim::wait_idle();
        rune::__heap_trim::reset_cooldown();
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        let rss_start = linux_vm_rss_kib().unwrap_or(0) as f64 / 1024.0;

        let mut big = String::new();
        for i in 0..copies {
            if i > 0 {
                big.push_str(&format!("\n\n# Part {i}\n\n"));
            }
            big.push_str(&generate_large_guide());
        }
        rt.update(|s, tx| {
            tx.instant();
            s.doc = Some(big);
        });
        rt.frame(1.0 / 60.0);
        let rss_loaded = linux_vm_rss_kib().unwrap_or(0) as f64 / 1024.0;

        // Close document and rest Runtime (frees Rust heap, before glibc trim).
        rt.update(|s, tx| {
            tx.instant();
            s.doc = None;
        });
        rt.frame(1.0 / 60.0);
        settle_exits(&mut rt);
        rt.rest();
        let rss_before_trim = linux_vm_rss_kib().unwrap_or(0) as f64 / 1024.0;

        // Spin another thread doing continuous small/medium heap allocations across the trim to
        // measure the worst arena-lock pause an allocating thread experiences during malloc_trim.
        let running = Arc::new(AtomicBool::new(true));
        let ready = Arc::new(AtomicBool::new(false));
        let worst_ns = Arc::new(AtomicU64::new(0));
        let iters = Arc::new(AtomicU64::new(0));
        let r_flag = Arc::clone(&running);
        let r_ready = Arc::clone(&ready);
        let r_worst = Arc::clone(&worst_ns);
        let r_iters = Arc::clone(&iters);
        let contender = std::thread::spawn(move || {
            let mut ring: Vec<Vec<u8>> = Vec::with_capacity(64);
            for i in 0..64 {
                ring.push(vec![i as u8; 2048]);
            }
            r_ready.store(true, Ordering::Release);
            let mut idx = 0usize;
            while r_flag.load(Ordering::Relaxed) {
                // Allocate 2 KiB (above glibc's 1,032 B tcache limit so it must acquire an arena
                // lock) and touch one byte so the compiler keeps it.
                let t0 = rune::time::now();
                let mut v = Vec::with_capacity(2048 + (idx & 63) * 64);
                v.push(idx as u8);
                std::hint::black_box(&v);
                ring[idx & 63] = v;
                let dt = t0.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
                r_worst.fetch_max(dt, Ordering::Relaxed);
                r_iters.fetch_add(1, Ordering::Relaxed);
                idx = idx.wrapping_add(1);
            }
            std::hint::black_box(ring);
        });
        while !ready.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        // Reset worst_ns right before triggering trim so startup cost is excluded.
        worst_ns.store(0, Ordering::Release);
        let calls_before = rune::__heap_trim::trim_calls();
        rune::__heap_trim::reset_cooldown();
        rune::__heap_trim::on_rest();

        // Also allocate on the main thread (main_arena, where the freed document lived) while the
        // background malloc-trim task runs so we measure worst-case main_arena mutex contention.
        let mut main_ring: Vec<Vec<u8>> = (0..32).map(|i| vec![i as u8; 2048]).collect();
        let mut main_worst_ns = 0u64;
        let mut main_iters = 0u64;
        let mut midx = 0usize;
        while rune::__heap_trim::trim_calls() == calls_before {
            let t0 = rune::time::now();
            let mut v = Vec::with_capacity(2048 + (midx & 31) * 64);
            v.push(midx as u8);
            std::hint::black_box(&v);
            main_ring[midx & 31] = v;
            let dt = t0.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            main_worst_ns = main_worst_ns.max(dt);
            main_iters += 1;
            midx = midx.wrapping_add(1);
        }
        std::hint::black_box(main_ring);
        rune::__heap_trim::wait_idle();
        running.store(false, Ordering::Relaxed);
        contender.join().unwrap();

        let trim_ms = rune::__heap_trim::last_trim_duration().as_secs_f64() * 1000.0;
        let other_pause_ms = worst_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let main_pause_ms = main_worst_ns as f64 / 1_000_000.0;
        let rss_after_trim = linux_vm_rss_kib().unwrap_or(0) as f64 / 1024.0;
        eprintln!(
            "PERF_TRIM [{label}]: start={rss_start:.2}MiB loaded={rss_loaded:.2}MiB before_trim={rss_before_trim:.2}MiB after_trim={rss_after_trim:.2}MiB trim_ms={trim_ms:.2} other_thread_pause_ms={other_pause_ms:.2} main_arena_pause_ms={main_pause_ms:.2} other_iters={} main_iters={main_iters}",
            iters.load(Ordering::Relaxed)
        );
    }

    measure_scale("1x_950KB", 1);
    measure_scale("3x_2850KB", 3);
}
