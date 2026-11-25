#![allow(unsafe_code)]
//! Headless `--profile perf` benchmark for large and ordinary `Markdown` documents.
//!
//! Measures first-frame time, active live heap delta (via `procwatch::alloc::Counting`), mounted
//! layout nodes, cached text layouts, fast-scroll frame times (median, p95, max), and jump-to-end
//! latency. Headless numbers omit GPU upload and swap-chain presentation.
//!
//! Build and run under `--profile perf` (non-perf builds are refused):
//! ```sh
//! cargo build --profile perf -p rune-kit --example markdown_large_bench --features markdown
//! target/perf/examples/markdown_large_bench [--case large|chat|all] [--json]
//! ```

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

mod fixture;

use rune::{Env, Runtime, ScrollSpec, col, el, text};
use rune_kit::{Kit, Markdown};

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
            .scroll(ScrollSpec { stick_bottom: false })
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

fn heap_live() -> i64 {
    procwatch::alloc::heap().map_or(0, |h| h.live)
}

fn mib(bytes: i64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

struct Row {
    case: &'static str,
    bytes: usize,
    first_frame_ms: f64,
    live_heap_mib: f64,
    nodes: usize,
    cached_layouts: usize,
    shaped_first_frame: u32,
    scroll_median_ms: f64,
    scroll_p95_ms: f64,
    scroll_max_ms: f64,
    jump_end_ms: f64,
}

fn settle(rt: &mut Runtime<AppState>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
}

fn run_case(case: &'static str, md: String) -> Row {
    let bytes = md.len();
    let mut rt = Runtime::new(AppState { doc: None }, view, (900.0, 700.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);
    let baseline = heap_live();

    // 1. First frame with the document mounted and shown.
    let t_open = rune::time::now();
    rt.update(|s, _| s.doc = Some(md));
    rt.frame(1.0 / 60.0);
    let first_frame_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    let live_heap_mib = mib(heap_live() - baseline);
    let nodes = rt.stats.layout_nodes;
    let cached_layouts = rt.text.cached_layout_count();
    let shaped_first_frame = rt.stats.shaped;

    // Let any initial frame work settle before measuring scroll.
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }

    // 2. Fast scroll sweep across the document (60 wheel frames).
    let mut scroll_ms = Vec::with_capacity(60);
    for _ in 0..60 {
        let t0 = rune::time::now();
        rt.wheel(450.0, 350.0, -480.0);
        rt.frame(1.0 / 60.0);
        scroll_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    scroll_ms.sort_by(|a, b| a.total_cmp(b));
    let scroll_median_ms = percentile(&scroll_ms, 0.50);
    let scroll_p95_ms = percentile(&scroll_ms, 0.95);
    let scroll_max_ms = scroll_ms.last().copied().unwrap_or(0.0);

    // 3. Return to top, settle, then jump straight to the end of the document.
    rt.update(|_, tx| tx.scroll_to_instant("viewer", 0.0, 0.0));
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    let t_jump = rune::time::now();
    rt.update(|_, tx| {
        tx.instant();
        tx.scroll_to_bottom("viewer");
    });
    rt.frame(1.0 / 60.0);
    let jump_end_ms = t_jump.elapsed().as_secs_f64() * 1000.0;

    // Release document and rest cleanly before any subsequent case.
    rt.update(|s, _| s.doc = None);
    rt.frame(1.0 / 60.0);
    settle(&mut rt);
    rt.rest();

    Row {
        case,
        bytes,
        first_frame_ms,
        live_heap_mib,
        nodes,
        cached_layouts,
        shaped_first_frame,
        scroll_median_ms,
        scroll_p95_ms,
        scroll_max_ms,
        jump_end_ms,
    }
}

fn main() {
    if cfg!(debug_assertions) {
        eprintln!("markdown_large_bench: build with --profile perf (refusing debug build)");
        std::process::exit(2);
    }
    let mut case_arg = "all".to_owned();
    let mut json = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--case" => {
                if let Some(v) = args.next() {
                    case_arg = v;
                }
            }
            "--json" => json = true,
            _ => {}
        }
    }

    let mut rows = Vec::new();
    if case_arg == "all" || case_arg == "chat" {
        rows.push(run_case("chat_10block", fixture::generate_chat_message()));
    }
    if case_arg == "all" || case_arg == "large" {
        rows.push(run_case("large_950kb", fixture::generate_large_guide()));
    }

    for r in &rows {
        if json {
            println!(
                "{{\"case\":\"{}\",\"bytes\":{},\"first_frame_ms\":{:.3},\"live_heap_mib\":{:.3},\"nodes\":{},\"cached_layouts\":{},\"shaped_first_frame\":{},\"scroll_median_ms\":{:.3},\"scroll_p95_ms\":{:.3},\"scroll_max_ms\":{:.3},\"jump_end_ms\":{:.3}}}",
                r.case,
                r.bytes,
                r.first_frame_ms,
                r.live_heap_mib,
                r.nodes,
                r.cached_layouts,
                r.shaped_first_frame,
                r.scroll_median_ms,
                r.scroll_p95_ms,
                r.scroll_max_ms,
                r.jump_end_ms,
            );
        } else {
            println!(
                "BENCH case={} bytes={} first_frame_ms={:.3} live_heap_mib={:.2} nodes={} cached_layouts={} shaped={} scroll_med_ms={:.3} scroll_p95_ms={:.3} scroll_max_ms={:.3} jump_end_ms={:.3}",
                r.case,
                r.bytes,
                r.first_frame_ms,
                r.live_heap_mib,
                r.nodes,
                r.cached_layouts,
                r.shaped_first_frame,
                r.scroll_median_ms,
                r.scroll_p95_ms,
                r.scroll_max_ms,
                r.jump_end_ms,
            );
        }
    }
}
