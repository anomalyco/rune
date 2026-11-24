//! Paired, alternating off-screen benchmark measuring the cost of post-first-frame effect
//! pipeline warming (`EffectWarm`) on a plain UI scene with no blur, no `PlusLighter`, and no
//! isolated layers.
//!
//! Run with `--profile perf` (never opens a window):
//! ```sh
//! cargo run --profile perf -p rune --example effect_warm_bench
//! WGPU_ADAPTER_NAME=NVIDIA cargo run --profile perf -p rune --example effect_warm_bench
//! WGPU_ADAPTER_NAME=AMD cargo run --profile perf -p rune --example effect_warm_bench
//! ```
//!
//! Each paired sample runs in a fresh child process so OS/driver memory (Windows `PrivateUsage`
//! and working set; macOS `ri_phys_footprint` and RSS) and pipeline allocations never bleed
//! between `with` and `without` runs. Leaves out window creation and swap-chain presentation.

use std::process::Command;
use std::time::Duration;

use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, col, el, oklch, row, text};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;
const SCALE: f32 = 2.0;
const DEFAULT_PAIRS: usize = 11;

struct PlainUi {
    tick: usize,
}

fn view(s: &PlainUi, _: &mut Cx<PlainUi>) -> El {
    let rows = (0..40).map(|i| {
        let hue = ((i * 37 + s.tick) % 360) as f32;
        row()
            .w_full()
            .px(16.0)
            .py(6.0)
            .gap(10.0)
            .center_y()
            .child(el().size(24.0, 24.0).radius(6.0).fill(oklch(0.65, 0.14, hue)))
            .child(text(format!("Plain row {} (frame {})", i + 1, s.tick)).color(Color::WHITE))
    });
    col()
        .w_full()
        .h_full()
        .fill(oklch(0.16, 0.01, 260.0))
        .children(rows)
}

#[derive(Clone, Copy, Debug, Default)]
struct MemSample {
    /// Windows: `PrivateUsage`; macOS: `ri_phys_footprint`; Linux: `VmRSS`.
    private_mib: f64,
    /// Windows: `WorkingSetSize`; macOS: `ri_resident_size`; Linux: `VmRSS`.
    rss_mib: f64,
    /// Windows: `PeakWorkingSetSize`; macOS: `ri_lifetime_max_phys_footprint`; Linux: `VmHWM`.
    peak_mib: f64,
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn sample_memory() -> MemSample {
    const MIB: f64 = 1024.0 * 1024.0;
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut core::ffi::c_void,
            counters: *mut ProcessMemoryCountersEx,
            cb: u32,
        ) -> i32;
    }
    let mut counters = ProcessMemoryCountersEx {
        cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        ..Default::default()
    };
    // SAFETY: `GetCurrentProcess` returns the current process pseudo-handle and `counters`
    // is sized to `ProcessMemoryCountersEx`.
    let ok = unsafe {
        K32GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, counters.cb)
    };
    if ok == 0 {
        return MemSample::default();
    }
    MemSample {
        private_mib: counters.private_usage as f64 / MIB,
        rss_mib: counters.working_set_size as f64 / MIB,
        peak_mib: counters.peak_working_set_size as f64 / MIB,
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn sample_memory() -> MemSample {
    const MIB: f64 = 1024.0 * 1024.0;
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: `info` is sized for `RUSAGE_INFO_V4` and read only when `proc_pid_rusage` returns 0.
    let ok = unsafe {
        libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast())
    };
    if ok != 0 {
        return MemSample::default();
    }
    // SAFETY: `proc_pid_rusage` succeeded and initialized `info`.
    let info = unsafe { info.assume_init() };
    MemSample {
        private_mib: info.ri_phys_footprint as f64 / MIB,
        rss_mib: info.ri_resident_size as f64 / MIB,
        peak_mib: info.ri_lifetime_max_phys_footprint as f64 / MIB,
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn sample_memory() -> MemSample {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return MemSample::default();
    };
    let kib = |key: &str| -> f64 {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 1024.0
    };
    let rss = kib("VmRSS:");
    MemSample { private_mib: rss, rss_mib: rss, peak_mib: kib("VmHWM:") }
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    worker_wall_ms: f64,
    worker_cpu_ms: f64,
    private_mib: f64,
    rss_mib: f64,
    peak_mib: f64,
    drop_ms: f64,
}

fn render_frame(
    gpu: &Gpu,
    renderer: &mut Renderer,
    target: &Offscreen,
    rt: &mut Runtime<PlainUi>,
    tick: usize,
) {
    if tick > 0 {
        rt.update(|s, _| s.tick = tick);
    }
    rt.frame(tick as f64 / 60.0);
    let size = (target.width, target.height);
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, size);
    renderer.set_fade_stops(rt.fade_stops());
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, rt.clear_color());
    renderer.submit(&gpu.queue, encoder);
    if let Err(err) = gpu.device.poll(wgpu::PollType::wait_indefinitely()) {
        panic!("device poll failed: {err}");
    }
}

fn run_sample(warm: bool, frames: usize) {
    Renderer::bench_calibrate_cpu_clock();
    let gpu = Gpu::headless();
    let info = gpu.adapter.get_info();
    let adapter_desc = format!("{} ({:?})", info.name, info.backend);
    let target = Offscreen::new(&gpu.device, WIDTH, HEIGHT);
    let logical = (WIDTH as f32 / SCALE, HEIGHT as f32 / SCALE);
    let mut rt = Runtime::new(PlainUi { tick: 0 }, view, logical, Env::new(true, SCALE));
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(warm);
    let timing = renderer.bench_effect_warm_timing();

    render_frame(&gpu, &mut renderer, &target, &mut rt, 0);
    renderer.bench_trigger_effect_warm(&gpu.device);

    let (mem, drop_ms) = if frames <= 1 {
        let drop_start = rune::time::now();
        drop(renderer);
        let drop_ms = drop_start.elapsed().as_secs_f64() * 1e3;
        let mem = sample_memory();
        (mem, drop_ms)
    } else {
        for tick in 1..frames {
            render_frame(&gpu, &mut renderer, &target, &mut rt, tick);
        }
        renderer.bench_wait_pinned();
        if warm {
            renderer.bench_wait_effect_warm();
        }
        if let Err(err) = gpu.device.poll(wgpu::PollType::wait_indefinitely()) {
        panic!("device poll failed: {err}");
    }
        std::thread::sleep(Duration::from_millis(50));
        let mem = sample_memory();
        let drop_start = rune::time::now();
        drop(renderer);
        let drop_ms = drop_start.elapsed().as_secs_f64() * 1e3;
        (mem, drop_ms)
    };

    let (worker_wall_ms, worker_cpu_ms) = timing
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or((0.0, 0.0));
    println!(
        "SAMPLE adapter={adapter_desc}|wall={worker_wall_ms:.4}|cpu={worker_cpu_ms:.4}|priv={:.4}|rss={:.4}|peak={:.4}|drop={drop_ms:.4}",
        mem.private_mib, mem.rss_mib, mem.peak_mib
    );
}

fn spawn_sample(exe: &std::path::Path, warm: bool, frames: usize) -> (String, Sample) {
    let output = Command::new(exe)
        .arg("--sample")
        .arg(if warm { "1" } else { "0" })
        .arg(frames.to_string())
        .output()
        .expect("spawn sample child");
    assert!(
        output.status.success(),
        "sample child failed (warm={warm}, frames={frames}): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("SAMPLE "))
        .unwrap_or_else(|| panic!("missing SAMPLE line in stdout: {stdout}"));
    let rest = line.trim_start_matches("SAMPLE ");
    let mut adapter = String::new();
    let mut wall = 0.0;
    let mut cpu = 0.0;
    let mut priv_mib = 0.0;
    let mut rss = 0.0;
    let mut peak = 0.0;
    let mut drop_ms = 0.0;
    for part in rest.split('|') {
        if let Some(v) = part.strip_prefix("adapter=") {
            adapter = v.to_owned();
        } else if let Some(v) = part.strip_prefix("wall=") {
            wall = v.parse().expect("wall");
        } else if let Some(v) = part.strip_prefix("cpu=") {
            cpu = v.parse().expect("cpu");
        } else if let Some(v) = part.strip_prefix("priv=") {
            priv_mib = v.parse().expect("priv");
        } else if let Some(v) = part.strip_prefix("rss=") {
            rss = v.parse().expect("rss");
        } else if let Some(v) = part.strip_prefix("peak=") {
            peak = v.parse().expect("peak");
        } else if let Some(v) = part.strip_prefix("drop=") {
            drop_ms = v.parse().expect("drop");
        }
    }
    (
        adapter,
        Sample {
            worker_wall_ms: wall,
            worker_cpu_ms: cpu,
            private_mib: priv_mib,
            rss_mib: rss,
            peak_mib: peak,
            drop_ms,
        },
    )
}

#[derive(Clone, Copy)]
struct Stats {
    median: f64,
    p25: f64,
    p75: f64,
    min: f64,
    max: f64,
}

fn summarize(mut values: Vec<f64>) -> Stats {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    let median = if n % 2 == 1 {
        values[n / 2]
    } else {
        0.5 * (values[n / 2 - 1] + values[n / 2])
    };
    Stats {
        median,
        p25: values[n / 4],
        p75: values[(3 * n) / 4],
        min: values[0],
        max: values[n - 1],
    }
}

fn print_metric(name: &str, unit: &str, with: &[f64], without: &[f64]) -> Stats {
    let sw = summarize(with.to_vec());
    let swo = summarize(without.to_vec());
    let deltas: Vec<f64> = with.iter().zip(without).map(|(a, b)| a - b).collect();
    let sd = summarize(deltas);
    println!(
        "  {name:<16} with={:.2} [{:.2}..{:.2} IQR {:.2}..{:.2}]  without={:.2} [{:.2}..{:.2}]  delta={:+.2} [{:+.2}..{:+.2} IQR {:+.2}..{:+.2}] {unit}",
        sw.median, sw.min, sw.max, sw.p25, sw.p75,
        swo.median, swo.min, swo.max,
        sd.median, sd.min, sd.max, sd.p25, sd.p75
    );
    sd
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--sample") {
        let warm = args.get(2).map(String::as_str) == Some("1");
        let frames: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
        run_sample(warm, frames);
        return;
    }
    let pairs: usize = rune::env::var("EFFECT_WARM_PAIRS")
        .and_then(|s| s.parse().ok())
        .filter(|&n| n >= 3)
        .unwrap_or(DEFAULT_PAIRS);
    let exe = std::env::current_exe().expect("current_exe");

    // Warm the OS/driver shader cache once before paired measurements.
    let (adapter, _) = spawn_sample(&exe, true, 60);
    let _ = spawn_sample(&exe, false, 60);
    println!("adapter: {adapter} (pairs={pairs}, fresh process per run, alternating order, outliers kept)");

    for frames in [1usize, 60usize] {
        let mut with = Vec::with_capacity(pairs);
        let mut without = Vec::with_capacity(pairs);
        for pair in 0..pairs {
            if pair % 2 == 0 {
                with.push(spawn_sample(&exe, true, frames).1);
                without.push(spawn_sample(&exe, false, frames).1);
            } else {
                without.push(spawn_sample(&exe, false, frames).1);
                with.push(spawn_sample(&exe, true, frames).1);
            }
        }
        println!("frames={frames}:");
        let collect = |xs: &[Sample], f: fn(&Sample) -> f64| -> Vec<f64> { xs.iter().map(f).collect() };
        let _ = print_metric("worker_wall", "ms", &collect(&with, |s| s.worker_wall_ms), &collect(&without, |s| s.worker_wall_ms));
        let _ = print_metric("worker_cpu", "ms", &collect(&with, |s| s.worker_cpu_ms), &collect(&without, |s| s.worker_cpu_ms));
        let _ = print_metric("settled_private", "MiB", &collect(&with, |s| s.private_mib), &collect(&without, |s| s.private_mib));
        let _ = print_metric("settled_rss", "MiB", &collect(&with, |s| s.rss_mib), &collect(&without, |s| s.rss_mib));
        let _ = print_metric("peak_mem", "MiB", &collect(&with, |s| s.peak_mib), &collect(&without, |s| s.peak_mib));
        let _ = print_metric("renderer_drop", "ms", &collect(&with, |s| s.drop_ms), &collect(&without, |s| s.drop_ms));
    }
}
