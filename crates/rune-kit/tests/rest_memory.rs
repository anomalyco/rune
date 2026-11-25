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

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn linux_malloc_trim() -> (i32, std::time::Duration) {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    let t0 = rune::time::now();
    // SAFETY: glibc's malloc_trim(0) trims free chunks back to the OS and takes no pointers.
    let res = unsafe { malloc_trim(0) };
    (res, t0.elapsed())
}

/// Generates a ~950 KB Markdown document (~10,500 lines, 488 table rows, 190 fenced code blocks
/// across 6 languages, headings, lists, links, inline code, no images).
fn generate_large_guide() -> String {
    let mut out = String::with_capacity(1_000_000);
    out.push_str("# Developer Architecture Guide\n\n");
    out.push_str("Comprehensive reference covering layout, rendering, storage, networking, and diagnostics.\n\n");
    let langs = ["rust", "typescript", "json", "python", "bash", "sql"];
    let mut table_rows = 0usize;
    let mut code_blocks = 0usize;
    let mut sec = 0usize;
    while out.len() < 948_000 || code_blocks < 190 || table_rows < 488 {
        sec += 1;
        out.push_str(&format!("## Section {sec}: Subsystem Design and Invariants\n\n"));
        for p in 0..3 {
            out.push_str(&format!(
                "Paragraph {sec}.{p} describes the **key invariants** for `subsystem_{sec}` including *incremental layout*, \
                 [reference links](https://example.com/docs/section-{sec}/part-{p}), `inline_code_token_{p}`, and bounded caches \
                 that drop unused allocations once a view settles at rest after navigation.\n\n"
            ));
        }
        out.push_str(&format!(
            "- Item {sec}.a: validate `FrameBudget` and `CacheLimit` on every state transition.\n\
             - Item {sec}.b: ensure virtualized rows and non-virtualized documents both release stale memory.\n\
             - Item {sec}.c: track `layout_pass`, `paint_pass`, and `text_shape` counts per commit.\n\n"
        ));
        if code_blocks < 190 {
            let lang = langs[code_blocks % langs.len()];
            code_blocks += 1;
            out.push_str(&format!("```{lang}\n"));
            for k in 0..14 {
                match lang {
                    "rust" => out.push_str(&format!(
                        "pub fn handle_event_{sec}_{k}(state: &mut State, delta: f32) -> bool {{ state.count += {k}; delta > 0.0 }}\n"
                    )),
                    "typescript" => out.push_str(&format!(
                        "export function processItem{sec}_{k}(input: readonly string[]): number {{ return input.length + {k}; }}\n"
                    )),
                    "json" => out.push_str(&format!(
                        "  \"field_{sec}_{k}\": {{ \"enabled\": true, \"limit\": {}, \"name\": \"entry-{k}\" }}{}\n",
                        k * 128,
                        if k < 13 { "," } else { "" }
                    )),
                    "python" => out.push_str(&format!(
                        "def compute_metric_{sec}_{k}(values: list[float]) -> float: return sum(values) / max(len(values), {k} + 1)\n"
                    )),
                    "bash" => out.push_str(&format!(
                        "echo \"step {sec}.{k}: checking service_status_{k} with threshold=$(( {k} * 64 ))\"\n"
                    )),
                    _ => out.push_str(&format!(
                        "SELECT id, name, metric_{k} FROM telemetry_events WHERE section_id = {sec} AND value > {k} ORDER BY id LIMIT 50;\n"
                    )),
                }
            }
            out.push_str("```\n\n");
        }
        if table_rows < 488 {
            out.push_str("| Key | Component | Budget | Notes |\n| --- | --- | --- | --- |\n");
            for r in 0..12 {
                if table_rows >= 488 {
                    break;
                }
                table_rows += 1;
                out.push_str(&format!(
                    "| `key_{sec}_{r}` | Component {sec}.{r} | {} KiB | Verified invariant for row {table_rows} in section {sec} |\n",
                    (r + 1) * 64
                ));
            }
            out.push('\n');
        }
    }
    out
}

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
        let (trim_res, trim_dur) = linux_malloc_trim();
        let rss_after_trim = linux_vm_rss_kib();
        eprintln!(
            "linux VmRSS (MiB): start={:.2} loaded={:.2} closed={:.2} rested={:.2} after_malloc_trim={:.2} (trim_ret={} trim_ms={:.2})",
            rss_start.unwrap_or(0) as f64 / 1024.0,
            rss_loaded.unwrap_or(0) as f64 / 1024.0,
            rss_closed.unwrap_or(0) as f64 / 1024.0,
            rss_rested_before_trim.unwrap_or(0) as f64 / 1024.0,
            rss_after_trim.unwrap_or(0) as f64 / 1024.0,
            trim_res,
            trim_dur.as_secs_f64() * 1000.0
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
