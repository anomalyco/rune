use super::*;

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting::system();

#[test]
fn counts_this_threads_allocations_and_live_heap() {
    let live = alloc::heap().unwrap().live;
    let before = alloc::thread_allocations();
    let held = std::hint::black_box(vec![0u8; 1 << 20]);
    assert_eq!(alloc::thread_allocations() - before, 1);
    // Other tests allocate in parallel, so allow slack.
    assert!(alloc::heap().unwrap().live - live >= (1 << 20) - (256 << 10));
    drop(held);
    let quiet = alloc::thread_allocations();
    let _ = std::hint::black_box(1 + 1);
    assert_eq!(alloc::thread_allocations(), quiet);
}

#[test]
#[allow(unsafe_code)]
fn thread_bytes_count_requests_not_frees_on_this_thread_only() {
    use std::alloc::{GlobalAlloc, Layout};
    let allocator = alloc::Counting::system();
    let before = alloc::thread_allocated_bytes().unwrap();
    // SAFETY: valid layouts, checked allocations, each freed once with its layout.
    unsafe {
        let layout = Layout::from_size_align(128, 8).unwrap();
        let p = allocator.alloc_zeroed(layout);
        assert!(!p.is_null());
        let p = allocator.realloc(p, layout, 256);
        assert!(!p.is_null());
        allocator.dealloc(p, Layout::from_size_align(256, 8).unwrap());
    }
    assert_eq!(alloc::thread_allocated_bytes().unwrap() - before, 384);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                let start = alloc::thread_allocated_bytes().unwrap();
                for _ in 0..10_000 {
                    drop(std::hint::black_box(vec![42u8; 128]));
                }
                alloc::thread_allocated_bytes().unwrap() - start
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), 1_280_000);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn usage_reads_memory_and_files_without_threads() {
    let u = usage();
    let footprint = u.footprint.unwrap();
    assert!(footprint > 0 && u.footprint_peak.unwrap() >= footprint, "{u:?}");
    assert!(u.resident.unwrap() > 0 && u.fds.unwrap() >= 3, "{u:?}");
    let opened: Vec<_> = (0..8).map(|_| std::fs::File::open("Cargo.toml").unwrap()).collect();
    assert!(usage().fds.unwrap() >= u.fds.unwrap() + 4);
    drop(opened);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn usage_is_unknown_off_macos() {
    assert_eq!(usage(), Usage::default());
}

fn at(footprint: u64, cpu_s: f64) -> Snapshot {
    Snapshot {
        footprint,
        footprint_peak: footprint,
        cpu_s,
        threads: vec![Thread { id: 1, name: "main".into(), cpu_s: 1.0 }],
        ..Default::default()
    }
}

fn lines(events: &[Event]) -> String {
    events.iter().map(|e| e.line.as_str()).collect::<Vec<_>>().join("\n")
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn thread_cpu_s() -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: fills `t`.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    t.tv_sec as f64 + t.tv_nsec as f64 / 1e9
}

#[cfg(target_os = "macos")]
#[test]
fn reads_this_process() {
    let s = snapshot().unwrap();
    assert!(s.footprint > 0 && s.footprint_peak >= s.footprint, "{s:?}");
    assert!(!s.threads.is_empty());
    assert_eq!(s.pid, std::process::id() as i32);
    // stdin, stdout and stderr at least.
    assert!(s.fds >= 3, "{s:?}");
    let opened: Vec<_> = (0..8).map(|_| std::fs::File::open("Cargo.toml").unwrap()).collect();
    assert!(snapshot().unwrap().fds >= s.fds + 4, "{s:?}");
    drop(opened);
    // A thread that burns CPU and exits still counts.
    let before = snapshot().unwrap();
    std::thread::spawn(|| {
        // Spin on the thread's own CPU clock: on a loaded machine 300 ms of wall time can
        // be well under 200 ms of CPU.
        let t = now();
        let mut x = 0u64;
        while thread_cpu_s() < 0.3 && t.elapsed() < Duration::from_secs(10) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
    })
    .join()
    .unwrap();
    let after = snapshot().unwrap();
    assert!(after.exited_cpu_s() - before.exited_cpu_s() > 0.2, "{before:?} {after:?}");
}

#[test]
fn json_parses_and_subtracts_heap_and_gauges() {
    let s = Snapshot {
        pid: 7,
        footprint: 1000,
        threads: vec![Thread { id: 1, name: "a\"b\\c\n".into(), cpu_s: 0.5 }],
        heap: Some(alloc::Heap { live: 300, tags: vec![("untagged", 300)], ..Default::default() }),
        gauges: vec![("video.frames", 200), ("gpu.textures", 100)],
        ..Default::default()
    };
    assert_eq!(s.unaccounted(), Some(400));
    let v: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    assert_eq!(v["pid"], 7);
    assert_eq!(v["threads"][0]["name"], "abc");
    assert_eq!(v["heap"]["tags"]["untagged"], 300);
    assert_eq!(v["gauges"]["video.frames"], 200);
    assert_eq!(v["unaccounted"], 400);
    // Without the counting allocator there's no honest unaccounted figure.
    let bare = Snapshot { heap: None, ..s };
    assert_eq!(bare.unaccounted(), None);
    let v: serde_json::Value = serde_json::from_str(&bare.to_json()).unwrap();
    assert!(v.get("unaccounted").is_none() && v.get("heap").is_none());
}

#[test]
fn warnings_name_where_the_memory_is() {
    let mut w = Watcher::new(Config { report: None, ..Config::release("t") });
    let s = Snapshot {
        footprint: 2 << 30,
        heap: Some(alloc::Heap { live: 100 << 20, ..Default::default() }),
        gauges: vec![("video.frames", 300 << 20)],
        ..at(2 << 30, 0.0)
    };
    let events = w.observe(now(), &s);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, Kind::Footprint);
    assert!(events[0].line.contains("heap 100 MB live") && events[0].line.ends_with("; video.frames 300 MB; unaccounted 1.6 GB"), "{}", events[0].line);
}

#[test]
fn warns_on_size_growth_and_cpu_once_per_window() {
    let mut w = Watcher::new(Config { report: None, ..Config::release("t") });
    let t0 = now();
    let mut events = Vec::new();
    for i in 0..=12u64 {
        // 100 MB every 10 s, and a full core.
        events.extend(w.observe(t0 + Duration::from_secs(i * 10), &at((50 + 100 * i) << 20, i as f64 * 10.0)));
    }
    let joined = lines(&events);
    assert!(joined.contains("footprint grew"), "{joined}");
    assert!(joined.contains("% CPU over 60s: exited threads 100%"), "{joined}");
    assert!(joined.contains("warning: footprint 1.0 GB") || joined.contains("warning: footprint 1.1 GB"), "{joined}");
    assert_eq!(events.iter().filter(|e| e.kind == Kind::Growth).count(), 2, "{joined}");
    assert!(events.iter().all(Event::is_warning));
    // Flat and idle: nothing.
    let mut w = Watcher::new(Config { report: None, ..Config::release("t") });
    for i in 0..=12u64 {
        assert!(w.observe(t0 + Duration::from_secs(i * 10), &at(50 << 20, 1.0)).is_empty());
    }
}

#[test]
fn a_gate_passes_once_per_window() {
    let (mut g, t0, window) = (Gate::default(), now(), Duration::from_secs(60));
    assert!(g.pass(t0, window), "the first goes out at once");
    assert!(!g.pass(t0 + Duration::from_secs(59), window));
    assert!(g.pass(t0 + Duration::from_secs(60), window));
    assert!(!g.pass(t0 + Duration::from_secs(61), window), "a window from the last one that went out");
}

#[test]
fn reports_once_per_period() {
    let mut w = Watcher::new(Config { report: Some(Duration::from_secs(30)), ..Config::release("t") });
    let t0 = now();
    let reported: Vec<u64> = (0..=9u64)
        .filter(|i| w.observe(t0 + Duration::from_secs(i * 10), &at(50 << 20, 1.0)).iter().any(|e| e.kind == Kind::Report))
        .collect();
    assert_eq!(reported, [0, 3, 6, 9]);
}

#[test]
fn late_samples_still_fill_the_window() {
    // Sleeps that run 10 ms long leave no sample exactly 60 s back.
    let mut w = Watcher::new(Config { report: None, ..Config::release("t") });
    let t0 = now();
    let mut events = Vec::new();
    for i in 0..=12u32 {
        let t = Duration::from_millis(10_010) * i;
        events.extend(w.observe(t0 + t, &at(50 << 20, t.as_secs_f64())));
    }
    assert_eq!(events.iter().filter(|e| e.kind == Kind::Cpu).count(), 2, "{}", lines(&events));
}

#[test]
fn an_exited_thread_counts_only_its_last_stretch() {
    let thread = |id, name: &str, cpu_s| Thread { id, name: name.into(), cpu_s };
    let old = Snapshot { cpu_s: 101.0, threads: vec![thread(1, "main", 1.0), thread(2, "worker", 100.0)], ..Default::default() };
    // The worker ran 6 more seconds and exited; main ran 54.
    let new = Snapshot { cpu_s: 161.0, threads: vec![thread(1, "main", 55.0)], ..Default::default() };
    assert_eq!(busiest(&old, &new, 60.0), "main 90%, exited threads 10%");
}
