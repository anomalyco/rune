//! Paired, alternating off-screen benchmark comparing `CpuPainter` against software `wgpu`
//! (Windows D3D12 WARP or Linux Vulkan lavapipe/llvmpipe) for the `CpuFirst::Auto` decision
//! (#2068).
//!
//! Run with `--profile perf` (never opens a window):
//! ```sh
//! WGPU_ADAPTER_NAME="Microsoft Basic Render Driver" cargo run --profile perf -p rune --example cpu_vs_software_gpu_bench
//! WGPU_ADAPTER_NAME=llvmpipe cargo run --profile perf -p rune --example cpu_vs_software_gpu_bench
//! ```
//!
//! All scenarios run with explicit sRGB blending (`srgb_blend(true)` into `Bgra8Unorm`), matching
//! live desktop windows and `CpuPainter`. Headless timings leave out window creation, OS buffer
//! presentation (GDI `BitBlt` / `wl_shm` commit), and GPU swap-chain presentation. Memory and cold
//! mid-session handover samples run in fresh child processes so driver state and allocations never
//! bleed across runs. Outliers are kept in all medians and min/max ranges.

use std::process::Command;
use std::time::Duration;

use bytemuck::Zeroable;
use rune::render::cpu_boot::{CpuPainter, NeedsGpu, Scene, Target, attach_text};
use rune::render::{Gpu, Instance, NO_CLIP, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, Spring, col, el, oklch, row, text};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;
const SCALE: f32 = 1.0;
const GPU_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const FRAME_ROUNDS: usize = 11;
const MEM_ROUNDS: usize = 7;
const HANDOVER_ROUNDS: usize = 7;
const CANVAS_COUNT: usize = 12_500;

fn painter_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 6)
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
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, counters.cb) };
    if ok == 0 { return MemSample::default(); }
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
    let ok = unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast()) };
    if ok != 0 { return MemSample::default(); }
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
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else { return MemSample::default(); };
    let kib = |key: &str| -> f64 {
        status.lines().find_map(|line| line.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / 1024.0
    };
    let rss = kib("VmRSS:");
    MemSample { private_mib: rss, rss_mib: rss, peak_mib: kib("VmHWM:") }
}

fn stats(values: &[f64]) -> (f64, f64, f64) {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    (sorted[sorted.len() / 2], sorted[0], sorted[sorted.len() - 1])
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HandoverTrigger { None, Rotate, Blur }

struct ShellState { hovered: usize, trigger: HandoverTrigger }

fn shell_view(s: &ShellState, _: &mut Cx<ShellState>) -> El {
    let bg = oklch(0.15, 0.01, 260.0);
    let card = oklch(0.20, 0.01, 260.0);
    let border = oklch(0.30, 0.01, 260.0);
    let accent = oklch(0.62, 0.16, 245.0);
    let muted = oklch(0.72, 0.01, 260.0);

    let titlebar = row().w_full().h(40.0).px(16.0).center_y().fill(card).border(1.0, border)
        .child(text("Desktop shell benchmark").color(Color::WHITE).grow(1.0))
        .child(text("sRGB blend").color(muted));

    let sidebar = col().w(260.0).h_full().padding(8.0).gap(4.0).fill(card).children((0..16).map(|i| {
        row().key(format!("side-{i}")).w_full().h(32.0).px(10.0).gap(8.0).center_y().radius(6.0)
            .fill(if i == s.hovered { accent } else { card })
            .child(el().size(12.0, 12.0).radius(6.0).fill(oklch(0.68, 0.14, (i * 23) as f32)))
            .child(text(format!("Workspace item {}", i + 1)).color(Color::WHITE).grow(1.0))
            .child(text(format!("{}m", 3 + i * 5)).color(muted))
    }));

    let cards = (0..6).map(|i| {
        col().key(format!("card-{i}")).w_full().padding(14.0).gap(6.0).radius(8.0)
            .fill(card).border(1.0, border)
            .child(text(format!("Message card {}", i + 1)).color(Color::WHITE))
            .child(text("A realistic desktop row with wrapped body copy, rounded cards, borders, and a shadowed composer bar.").color(muted).wrap())
    });
    let composer = row().w_full().h(52.0).px(16.0).center_y().radius(10.0)
        .fill(card).border(1.0, border).shadow(4.0, 14.0, Color::BLACK.alpha(0.35))
        .child(text("Ask anything...").color(muted).grow(1.0))
        .child(el().size(28.0, 28.0).radius(14.0).fill(accent));

    let mut main = col().grow(1.0).h_full().padding(20.0).gap(12.0).children(cards).child(composer);
    match s.trigger {
        HandoverTrigger::None => {}
        HandoverTrigger::Rotate => {
            main = main.child(el().key("needs-gpu-rotate").size(220.0, 48.0).radius(8.0).fill(accent).rotate(0.15));
        }
        HandoverTrigger::Blur => {
            main = main.child(el().key("needs-gpu-blur").size(220.0, 48.0).radius(8.0).fill(accent.alpha(0.85)).blur(8.0));
        }
    }

    col().w_full().h_full().fill(bg).child(titlebar).child(row().w_full().grow(1.0).child(sidebar).child(main))
}

struct ScrollState { backdrop_blur: bool }

fn scroll_view(s: &ScrollState, _: &mut Cx<ScrollState>) -> El {
    let rows = (0..200).map(|i| {
        row().w_full().h(52.0).px(16.0).gap(12.0).center_y().border(1.0, oklch(0.24, 0.01, 260.0))
            .child(el().size(32.0, 32.0).radius(8.0).fill(oklch(0.66, 0.16, ((i * 37) % 360) as f32)))
            .child(col().grow(1.0).gap(2.0)
                .child(text(format!("Timeline item #{}", i + 1)).color(Color::WHITE))
                .child(text("Secondary metadata line for scroll benchmark").color(oklch(0.70, 0.01, 260.0))))
            .child(text(format!("{:02}:{:02}", (i / 60) % 24, i % 60)).color(oklch(0.62, 0.01, 260.0)))
    });
    let list = col().key("scroll-list").w_full().h_full()
        .scroll(ScrollSpec { stick_bottom: false }).child(col().w_full().children(rows));
    let root = el().w_full().h_full().fill(oklch(0.14, 0.01, 260.0)).child(list);
    if s.backdrop_blur {
        let bar = el().pin(Some(0.0), None, Some(0.0), Some(0.0)).h(60.0)
            .fill(oklch(0.20, 0.02, 260.0).alpha(0.6)).backdrop_blur(20.0);
        root.child(bar)
    } else {
        root
    }
}

struct SpringState { phase: usize }

fn spring_view(s: &SpringState, _: &mut Cx<SpringState>) -> El {
    let spring = Spring::new(0.45, 0.15);
    let flipped = s.phase % 2 == 1;
    let rows = (0..4).map(|r| {
        row().w_full().grow(1.0).gap(12.0).children((0..6).map(|c| {
            let idx = r * 6 + c;
            let dx = if flipped { 18.0 } else { -18.0 } * if idx % 2 == 0 { 1.0 } else { -1.0 };
            let dy = if flipped { 12.0 } else { -12.0 } * if r % 2 == 0 { 1.0 } else { -1.0 };
            let hue = ((idx * 15 + if flipped { 90 } else { 0 }) % 360) as f32;
            col().key(format!("tile-{idx}")).grow(1.0).h_full().padding(12.0).gap(6.0).radius(10.0)
                .fill(oklch(0.28, 0.07, hue)).border(1.0, oklch(0.45, 0.10, hue))
                .offset(dx, dy).motion(spring)
                .child(text(format!("Card {}", idx + 1)).color(Color::WHITE))
                .child(text("Full-window spring").color(oklch(0.82, 0.02, hue)))
        }))
    });
    col().w_full().h_full().padding(20.0).gap(12.0).fill(oklch(0.13, 0.01, 260.0)).children(rows)
}

fn canvas_instances(frame: usize) -> Vec<Instance> {
    let cols = 125usize;
    let rows = CANVAS_COUNT / cols;
    let (cell_w, cell_h) = (WIDTH as f32 / cols as f32, HEIGHT as f32 / rows as f32);
    let shift = (frame % 7) as f32;
    let mut out = Vec::with_capacity(CANVAS_COUNT);
    for r in 0..rows {
        for c in 0..cols {
            let idx = r * cols + c;
            let x = c as f32 * cell_w + (shift * 0.5) - 1.5;
            let y = r as f32 * cell_h + ((frame + idx) % 3) as f32 * 0.5;
            let t = ((idx + frame * 11) % 256) as f32 / 255.0;
            out.push(Instance {
                rect: [x, y, (cell_w - 1.5).max(2.0), (cell_h - 1.5).max(2.0)],
                fill: [0.2 + 0.6 * t, 0.35 + 0.4 * (1.0 - t), 0.75, 1.0],
                radii: [2.0; 4],
                clip: NO_CLIP,
                ..Zeroable::zeroed()
            });
        }
    }
    out
}

fn new_runtime<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El, for_cpu: bool) -> Runtime<S> {
    let mut rt = Runtime::new(state, view, (WIDTH as f32 / SCALE, HEIGHT as f32 / SCALE), Env::new(true, SCALE));
    rt.set_srgb_blend(true);
    if for_cpu { attach_text(&mut rt.text.atlas); }
    rt
}

fn new_gpu_renderer(gpu: &Gpu) -> (Renderer, Offscreen) {
    let mut renderer = Renderer::new(&gpu.device, GPU_FORMAT);
    renderer.set_srgb_blend(true);
    let target = Offscreen::with_format(&gpu.device, WIDTH, HEIGHT, GPU_FORMAT);
    (renderer, target)
}

fn gpu_render_runtime<S: 'static>(gpu: &Gpu, renderer: &mut Renderer, target: &Offscreen, rt: &mut Runtime<S>) {
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

fn cpu_render_runtime<S: 'static>(painter: &mut CpuPainter, pixels: &mut [u8], rt: &mut Runtime<S>) -> rune::render::cpu_boot::Painted {
    let _ = painter.fill_masks(&mut rt.text.atlas);
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (WIDTH, HEIGHT), rt.clear_color())
        .srgb_blend(true).fade_stops(rt.fade_stops());
    painter.paint(&scene, &mut Target::new(pixels, WIDTH, HEIGHT)).expect("CpuPainter refused benchmark scene")
}

fn bench_hover(gpu: &Gpu) {
    let mut cpu_rt = new_runtime(ShellState { hovered: 0, trigger: HandoverTrigger::None }, shell_view, true);
    let mut gpu_rt = new_runtime(ShellState { hovered: 0, trigger: HandoverTrigger::None }, shell_view, false);
    let mut painter = CpuPainter::new(painter_threads());
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    let (mut renderer, target) = new_gpu_renderer(gpu);

    for i in 0..4usize {
        cpu_rt.update(|s, _| s.hovered = i % 2);
        cpu_rt.frame(1.0 / 60.0);
        let _ = cpu_render_runtime(&mut painter, &mut pixels, &mut cpu_rt);
        gpu_rt.update(|s, _| s.hovered = i % 2);
        gpu_rt.frame(1.0 / 60.0);
        gpu_render_runtime(gpu, &mut renderer, &target, &mut gpu_rt);
    }

    let (mut cpu_r, mut gpu_r, mut cpu_t, mut gpu_t, mut dirty) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut total_tiles = 0usize;
    let n = 20usize;
    for round in 0..FRAME_ROUNDS {
        let mut run_cpu = |step: usize| {
            let (mut r, mut t) = (Vec::with_capacity(n), Vec::with_capacity(n));
            for f in 0..n {
                let t0 = rune::time::now();
                cpu_rt.update(|s, _| s.hovered = (step + f) % 2);
                cpu_rt.frame(1.0 / 60.0);
                let t1 = rune::time::now();
                let p = cpu_render_runtime(&mut painter, &mut pixels, &mut cpu_rt);
                let t2 = rune::time::now();
                r.push((t2 - t1).as_secs_f64() * 1e3);
                t.push((t2 - t0).as_secs_f64() * 1e3);
                dirty.push(p.dirty_tiles as f64);
                total_tiles = p.tiles;
            }
            cpu_r.push(stats(&r).0);
            cpu_t.push(stats(&t).0);
        };
        let mut run_gpu = |step: usize| {
            let (mut r, mut t) = (Vec::with_capacity(n), Vec::with_capacity(n));
            for f in 0..n {
                let t0 = rune::time::now();
                gpu_rt.update(|s, _| s.hovered = (step + f) % 2);
                gpu_rt.frame(1.0 / 60.0);
                let t1 = rune::time::now();
                gpu_render_runtime(gpu, &mut renderer, &target, &mut gpu_rt);
                let t2 = rune::time::now();
                r.push((t2 - t1).as_secs_f64() * 1e3);
                t.push((t2 - t0).as_secs_f64() * 1e3);
            }
            gpu_r.push(stats(&r).0);
            gpu_t.push(stats(&t).0);
        };
        let step = round * n;
        if round % 2 == 0 { run_cpu(step); run_gpu(step); } else { run_gpu(step); run_cpu(step); }
    }

    let (cm, cmin, cmax) = stats(&cpu_r);
    let (gm, gmin, gmax) = stats(&gpu_r);
    println!(
        "SCENARIO hover (small-damage, dirty={:.0}/{total_tiles} tiles): cpu_render_med={cm:.3} ms (min={cmin:.3} max={cmax:.3}) | gpu_render_med={gm:.3} ms (min={gmin:.3} max={gmax:.3}) | ratio_gpu_over_cpu={:.2}x | total_med cpu={:.3} ms gpu={:.3} ms",
        stats(&dirty).0, gm / cm.max(1e-6), stats(&cpu_t).0, stats(&gpu_t).0
    );
}

fn bench_canvas_12500(gpu: &Gpu) {
    let cpu_rt = new_runtime((), |_, _| el(), true);
    let mut gpu_rt = new_runtime((), |_, _| el(), false);
    let mut painter = CpuPainter::new(painter_threads());
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    let (mut renderer, target) = new_gpu_renderer(gpu);
    let clear = [0.1, 0.1, 0.12, 1.0];

    for i in 0..2usize {
        let inst = canvas_instances(i);
        let scene = Scene::new(&inst, &cpu_rt.text.atlas, &cpu_rt.images, (WIDTH, HEIGHT), clear).srgb_blend(true);
        let _ = painter.paint(&scene, &mut Target::new(&mut pixels, WIDTH, HEIGHT)).expect("cpu canvas");
        renderer.prepare(&gpu.device, &gpu.queue, &mut gpu_rt.text.atlas, &inst, (WIDTH, HEIGHT));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, clear);
        renderer.submit(&gpu.queue, enc);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    }

    let (mut cpu_ms, mut gpu_ms) = (Vec::with_capacity(FRAME_ROUNDS), Vec::with_capacity(FRAME_ROUNDS));
    let n = 10usize;
    for round in 0..FRAME_ROUNDS {
        let mut run_cpu = |base: usize| {
            let mut times = Vec::with_capacity(n);
            for f in 0..n {
                let inst = canvas_instances(base + f);
                let t0 = rune::time::now();
                let scene = Scene::new(&inst, &cpu_rt.text.atlas, &cpu_rt.images, (WIDTH, HEIGHT), clear).srgb_blend(true);
                let _ = painter.paint(&scene, &mut Target::new(&mut pixels, WIDTH, HEIGHT)).expect("cpu canvas");
                times.push(t0.elapsed().as_secs_f64() * 1e3);
            }
            cpu_ms.push(stats(&times).0);
        };
        let mut run_gpu = |base: usize| {
            let mut times = Vec::with_capacity(n);
            for f in 0..n {
                let inst = canvas_instances(base + f);
                let t0 = rune::time::now();
                renderer.prepare(&gpu.device, &gpu.queue, &mut gpu_rt.text.atlas, &inst, (WIDTH, HEIGHT));
                let mut enc = gpu.device.create_command_encoder(&Default::default());
                renderer.draw(&mut enc, &target.view, clear);
                renderer.submit(&gpu.queue, enc);
                gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
                times.push(t0.elapsed().as_secs_f64() * 1e3);
            }
            gpu_ms.push(stats(&times).0);
        };
        let base = round * n;
        if round % 2 == 0 { run_cpu(base); run_gpu(base); } else { run_gpu(base); run_cpu(base); }
    }

    let (cm, cmin, cmax) = stats(&cpu_ms);
    let (gm, gmin, gmax) = stats(&gpu_ms);
    println!(
        "SCENARIO canvas_12500 ({CANVAS_COUNT} instances): cpu_render_med={cm:.3} ms (min={cmin:.3} max={cmax:.3}) | gpu_render_med={gm:.3} ms (min={gmin:.3} max={gmax:.3}) | ratio_gpu_over_cpu={:.2}x",
        gm / cm.max(1e-6)
    );
}

fn bench_scroll(gpu: &Gpu, backdrop_blur: bool, label: &str, n: usize) {
    let mut cpu_rt = new_runtime(ScrollState { backdrop_blur }, scroll_view, true);
    let mut gpu_rt = new_runtime(ScrollState { backdrop_blur }, scroll_view, false);
    let mut painter = CpuPainter::new(painter_threads());
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    let (mut renderer, target) = new_gpu_renderer(gpu);

    for _ in 0..4 {
        cpu_rt.frame(1.0 / 60.0);
        let _ = cpu_render_runtime(&mut painter, &mut pixels, &mut cpu_rt);
        gpu_rt.frame(1.0 / 60.0);
        gpu_render_runtime(gpu, &mut renderer, &target, &mut gpu_rt);
    }

    let (mut cpu_r, mut gpu_r, mut cpu_t, mut gpu_t, mut dirty) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut scroll_hits, mut scroll_frames, mut total_tiles) = (0usize, 0usize, 0usize);

    for round in 0..FRAME_ROUNDS {
        let dir = if round % 2 == 0 { -16.0 } else { 16.0 };
        let mut run_cpu = || {
            let (mut r, mut t) = (Vec::with_capacity(n), Vec::with_capacity(n));
            for _ in 0..n {
                let t0 = rune::time::now();
                cpu_rt.wheel(WIDTH as f32 * 0.5, HEIGHT as f32 * 0.5, dir);
                cpu_rt.frame(1.0 / 60.0);
                let t1 = rune::time::now();
                let p = cpu_render_runtime(&mut painter, &mut pixels, &mut cpu_rt);
                let t2 = rune::time::now();
                r.push((t2 - t1).as_secs_f64() * 1e3);
                t.push((t2 - t0).as_secs_f64() * 1e3);
                scroll_frames += 1;
                if p.scrolled.is_some() { scroll_hits += 1; }
                dirty.push(p.dirty_tiles as f64);
                total_tiles = p.tiles;
            }
            cpu_r.push(stats(&r).0);
            cpu_t.push(stats(&t).0);
        };
        let mut run_gpu = || {
            let (mut r, mut t) = (Vec::with_capacity(n), Vec::with_capacity(n));
            for _ in 0..n {
                let t0 = rune::time::now();
                gpu_rt.wheel(WIDTH as f32 * 0.5, HEIGHT as f32 * 0.5, dir);
                gpu_rt.frame(1.0 / 60.0);
                let t1 = rune::time::now();
                gpu_render_runtime(gpu, &mut renderer, &target, &mut gpu_rt);
                let t2 = rune::time::now();
                r.push((t2 - t1).as_secs_f64() * 1e3);
                t.push((t2 - t0).as_secs_f64() * 1e3);
            }
            gpu_r.push(stats(&r).0);
            gpu_t.push(stats(&t).0);
        };
        if round % 2 == 0 { run_cpu(); run_gpu(); } else { run_gpu(); run_cpu(); }
    }

    let (cm, cmin, cmax) = stats(&cpu_r);
    let (gm, gmin, gmax) = stats(&gpu_r);
    println!(
        "SCENARIO {label} (dirty={:.0}/{total_tiles} tiles, scroll_fast_path={scroll_hits}/{scroll_frames}): cpu_render_med={cm:.3} ms (min={cmin:.3} max={cmax:.3}) | gpu_render_med={gm:.3} ms (min={gmin:.3} max={gmax:.3}) | ratio_gpu_over_cpu={:.2}x | total_med cpu={:.3} ms gpu={:.3} ms",
        stats(&dirty).0, gm / cm.max(1e-6), stats(&cpu_t).0, stats(&gpu_t).0
    );
}

fn bench_spring(gpu: &Gpu) {
    let mut cpu_rt = new_runtime(SpringState { phase: 0 }, spring_view, true);
    let mut gpu_rt = new_runtime(SpringState { phase: 0 }, spring_view, false);
    let mut painter = CpuPainter::new(painter_threads());
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    let (mut renderer, target) = new_gpu_renderer(gpu);

    cpu_rt.frame(0.0);
    let _ = cpu_render_runtime(&mut painter, &mut pixels, &mut cpu_rt);
    gpu_rt.frame(0.0);
    gpu_render_runtime(gpu, &mut renderer, &target, &mut gpu_rt);

    let (mut cpu_r, mut gpu_r, mut cpu_t, mut gpu_t, mut dirty) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut total_tiles = 0usize;
    let n = 16usize;

    for round in 0..FRAME_ROUNDS {
        let phase = round + 1;
        let mut run_cpu = || {
            cpu_rt.update(|s, _| s.phase = phase);
            let (mut r, mut t) = (Vec::with_capacity(n), Vec::with_capacity(n));
            for _ in 0..n {
                let t0 = rune::time::now();
                cpu_rt.frame(1.0 / 60.0);
                let t1 = rune::time::now();
                let p = cpu_render_runtime(&mut painter, &mut pixels, &mut cpu_rt);
                let t2 = rune::time::now();
                r.push((t2 - t1).as_secs_f64() * 1e3);
                t.push((t2 - t0).as_secs_f64() * 1e3);
                dirty.push(p.dirty_tiles as f64);
                total_tiles = p.tiles;
            }
            cpu_r.push(stats(&r).0);
            cpu_t.push(stats(&t).0);
        };
        let mut run_gpu = || {
            gpu_rt.update(|s, _| s.phase = phase);
            let (mut r, mut t) = (Vec::with_capacity(n), Vec::with_capacity(n));
            for _ in 0..n {
                let t0 = rune::time::now();
                gpu_rt.frame(1.0 / 60.0);
                let t1 = rune::time::now();
                gpu_render_runtime(gpu, &mut renderer, &target, &mut gpu_rt);
                let t2 = rune::time::now();
                r.push((t2 - t1).as_secs_f64() * 1e3);
                t.push((t2 - t0).as_secs_f64() * 1e3);
            }
            gpu_r.push(stats(&r).0);
            gpu_t.push(stats(&t).0);
        };
        if round % 2 == 0 { run_cpu(); run_gpu(); } else { run_gpu(); run_cpu(); }
    }

    let (cm, cmin, cmax) = stats(&cpu_r);
    let (gm, gmin, gmax) = stats(&gpu_r);
    println!(
        "SCENARIO spring (full-window, dirty={:.0}/{total_tiles} tiles): cpu_render_med={cm:.3} ms (min={cmin:.3} max={cmax:.3}) | gpu_render_med={gm:.3} ms (min={gmin:.3} max={gmax:.3}) | ratio_gpu_over_cpu={:.2}x | total_med cpu={:.3} ms gpu={:.3} ms",
        stats(&dirty).0, gm / cm.max(1e-6), stats(&cpu_t).0, stats(&gpu_t).0
    );
}

fn run_memory_sample(mode: &str) {
    if mode == "cpu" {
        let mut rt = new_runtime(ShellState { hovered: 0, trigger: HandoverTrigger::None }, shell_view, true);
        let mut painter = CpuPainter::new(painter_threads());
        let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
        for i in 0..30usize {
            rt.update(|s, _| s.hovered = i % 16);
            rt.frame(1.0 / 60.0);
            let _ = cpu_render_runtime(&mut painter, &mut pixels, &mut rt);
        }
        std::thread::sleep(Duration::from_millis(50));
        let mem = sample_memory();
        std::hint::black_box(&pixels);
        println!("MEM mode=cpu|priv={:.3}|rss={:.3}|peak={:.3}", mem.private_mib, mem.rss_mib, mem.peak_mib);
    } else {
        let gpu = Gpu::headless();
        let (mut renderer, target) = new_gpu_renderer(&gpu);
        let mut rt = new_runtime(ShellState { hovered: 0, trigger: HandoverTrigger::None }, shell_view, false);
        for i in 0..30usize {
            rt.update(|s, _| s.hovered = i % 16);
            rt.frame(1.0 / 60.0);
            gpu_render_runtime(&gpu, &mut renderer, &target, &mut rt);
        }
        std::thread::sleep(Duration::from_millis(50));
        let mem = sample_memory();
        std::hint::black_box(&target);
        println!("MEM mode=gpu|priv={:.3}|rss={:.3}|peak={:.3}", mem.private_mib, mem.rss_mib, mem.peak_mib);
    }
}

fn bench_degraded_fallback(trigger: HandoverTrigger, expected: NeedsGpu, label: &str) {
    let mut rt = new_runtime(ShellState { hovered: 0, trigger }, shell_view, true);
    let mut painter = CpuPainter::new(painter_threads());
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];

    for i in 0..4usize {
        rt.update(|s, _| s.hovered = i % 2);
        rt.frame(1.0 / 60.0);
        let _ = painter.fill_masks(&mut rt.text.atlas);
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (WIDTH, HEIGHT), rt.clear_color())
            .srgb_blend(true).fade_stops(rt.fade_stops());
        assert_eq!(scene.needs_gpu(), Some(expected));
        let _ = painter.paint(&scene.degraded(true), &mut Target::new(&mut pixels, WIDTH, HEIGHT))
            .expect("degraded fallback must paint");
    }

    let (mut render_rounds, mut total_rounds) = (Vec::with_capacity(FRAME_ROUNDS), Vec::with_capacity(FRAME_ROUNDS));
    let mut dirty = Vec::new();
    let mut total_tiles = 0usize;
    let n = 20usize;
    for round in 0..FRAME_ROUNDS {
        let (mut r, mut t) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for f in 0..n {
            let t0 = rune::time::now();
            rt.update(|s, _| s.hovered = (round * n + f) % 2);
            rt.frame(1.0 / 60.0);
            let t1 = rune::time::now();
            let _ = painter.fill_masks(&mut rt.text.atlas);
            let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (WIDTH, HEIGHT), rt.clear_color())
                .srgb_blend(true).fade_stops(rt.fade_stops()).degraded(true);
            let p = painter.paint(&scene, &mut Target::new(&mut pixels, WIDTH, HEIGHT))
                .expect("degraded fallback must paint");
            let t2 = rune::time::now();
            r.push((t2 - t1).as_secs_f64() * 1e3);
            t.push((t2 - t0).as_secs_f64() * 1e3);
            dirty.push(p.dirty_tiles as f64);
            total_tiles = p.tiles;
        }
        render_rounds.push(stats(&r).0);
        total_rounds.push(stats(&t).0);
    }

    let (rm, rmin, rmax) = stats(&render_rounds);
    let (tm, tmin, tmax) = stats(&total_rounds);
    println!(
        "SCENARIO {label} (degraded fallback, dirty={:.0}/{total_tiles} tiles): cpu_render_med={rm:.3} ms (min={rmin:.3} max={rmax:.3}) | total_med={tm:.3} ms (min={tmin:.3} max={tmax:.3})",
        stats(&dirty).0
    );
}

fn run_handover_sample(kind: &str) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let (trigger, expected) = match kind {
        "rotate" => (HandoverTrigger::Rotate, NeedsGpu::TurnedLayer),
        _ => (HandoverTrigger::Blur, NeedsGpu::LayerBlur),
    };
    let mut rt = new_runtime(ShellState { hovered: 0, trigger: HandoverTrigger::None }, shell_view, true);
    let mut painter = CpuPainter::new(painter_threads());
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for i in 0..10usize {
        rt.update(|s, _| s.hovered = i % 16);
        rt.frame(1.0 / 60.0);
        let _ = cpu_render_runtime(&mut painter, &mut pixels, &mut rt);
    }
    let before = sample_memory();

    rt.update(|s, _| s.trigger = trigger);
    rt.frame(1.0 / 60.0);
    let _ = painter.fill_masks(&mut rt.text.atlas);
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (WIDTH, HEIGHT), rt.clear_color())
        .srgb_blend(true).fade_stops(rt.fade_stops());
    let reason = painter.paint(&scene, &mut Target::new(&mut pixels, WIDTH, HEIGHT))
        .expect_err("NeedsGpu handover trigger frame must require GPU");
    assert_eq!(reason, expected);
    assert!(reason.is_degradable());

    // Spawn off-thread cold software-GPU handover (matching win32/handover.rs) while the UI thread
    // keeps painting live degraded CPU frames paced at ~60 Hz. The Renderer stays on the worker
    // thread that created it (it is `!Send` on macOS) and only timings and memory metrics return.
    let instances_for_bg = rt.instances.clone();
    let clear = rt.clear_color();
    let done = Arc::new(AtomicBool::new(false));
    let done_bg = done.clone();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let bg = std::thread::spawn(move || {
        let t0 = rune::time::now();
        let gpu = Gpu::headless();
        let t1 = rune::time::now();
        let (mut renderer, target) = new_gpu_renderer(&gpu);
        let t2 = rune::time::now();
        let mut atlas = rune::text::Atlas::new();
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances_for_bg, (WIDTH, HEIGHT));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, clear);
        renderer.submit(&gpu.queue, enc);
        if let Err(err) = gpu.device.poll(wgpu::PollType::wait_indefinitely()) {
            panic!("device poll failed: {err}");
        }
        let t3 = rune::time::now();
        done_bg.store(true, Ordering::Release);
        if release_rx.recv().is_err() {
            // Main thread exited before signaling; sample and drop cleanly anyway.
        }
        let after = sample_memory();
        drop((target, renderer, gpu));
        (
            (t1 - t0).as_secs_f64() * 1e3,
            (t2 - t1).as_secs_f64() * 1e3,
            (t3 - t2).as_secs_f64() * 1e3,
            (t3 - t0).as_secs_f64() * 1e3,
            after,
        )
    });

    let mut ui_frames = 0usize;
    let mut ui_max_ms = 0.0f64;
    while !done.load(Ordering::Acquire) && !bg.is_finished() {
        let f0 = rune::time::now();
        rt.update(|s, _| s.hovered = ui_frames % 16);
        rt.frame(1.0 / 60.0);
        let _ = painter.fill_masks(&mut rt.text.atlas);
        let degraded = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (WIDTH, HEIGHT), rt.clear_color())
            .srgb_blend(true).fade_stops(rt.fade_stops()).degraded(true);
        let _ = painter.paint(&degraded, &mut Target::new(&mut pixels, WIDTH, HEIGHT))
            .expect("mid-handover degraded paint");
        let ms = f0.elapsed().as_secs_f64() * 1e3;
        ui_max_ms = ui_max_ms.max(ms);
        ui_frames += 1;
        std::thread::sleep(Duration::from_millis(16));
    }

    drop(painter);
    drop(pixels);
    if release_tx.send(()).is_err() {
        // Worker exited early; bg.join() below surfaces any panic.
    }
    let (gpu_create, renderer_new, first_frame, total, after) = bg.join().expect("handover bg thread");

    println!(
        "HANDOVER kind={kind}|gpu_create={gpu_create:.3}|renderer_new={renderer_new:.3}|first_frame={first_frame:.3}|total={total:.3}|before_priv={:.3}|after_priv={:.3}|ui_frames={ui_frames}|ui_max_ms={ui_max_ms:.3}",
        before.private_mib,
        after.private_mib,
    );
}

fn parse_kv(line: &str, key: &str) -> f64 {
    line.split('|').find_map(|p| p.strip_prefix(key)).and_then(|v| v.parse::<f64>().ok())
        .unwrap_or_else(|| panic!("missing {key} in line: {line}"))
}

fn spawn_memory_sample(exe: &std::path::Path, mode: &str) -> MemSample {
    let out = Command::new(exe).args(["--sample-memory", mode]).output().expect("spawn memory sample");
    assert!(out.status.success(), "memory sample failed (mode={mode}): {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let rest = stdout.lines().find(|l| l.starts_with("MEM ")).expect("missing MEM line").trim_start_matches("MEM ");
    MemSample { private_mib: parse_kv(rest, "priv="), rss_mib: parse_kv(rest, "rss="), peak_mib: parse_kv(rest, "peak=") }
}

fn bench_memory(exe: &std::path::Path) {
    let (mut cp, mut cr, mut cpk) = (Vec::new(), Vec::new(), Vec::new());
    let (mut gp, mut gr, mut gpk) = (Vec::new(), Vec::new(), Vec::new());
    for round in 0..MEM_ROUNDS {
        let (c, g) = if round % 2 == 0 {
            (spawn_memory_sample(exe, "cpu"), spawn_memory_sample(exe, "gpu"))
        } else {
            let g = spawn_memory_sample(exe, "gpu");
            (spawn_memory_sample(exe, "cpu"), g)
        };
        cp.push(c.private_mib); cr.push(c.rss_mib); cpk.push(c.peak_mib);
        gp.push(g.private_mib); gr.push(g.rss_mib); gpk.push(g.peak_mib);
    }
    let (cp_m, cp_min, cp_max) = stats(&cp);
    let (cr_m, cr_min, cr_max) = stats(&cr);
    let (gp_m, gp_min, gp_max) = stats(&gp);
    let (gr_m, gr_min, gr_max) = stats(&gr);
    println!(
        "SCENARIO memory ({MEM_ROUNDS} fresh paired processes): cpu_priv_med={cp_m:.2} MiB (min={cp_min:.2} max={cp_max:.2}) cpu_rss_med={cr_m:.2} MiB (min={cr_min:.2} max={cr_max:.2}) cpu_peak_med={:.2} MiB | gpu_priv_med={gp_m:.2} MiB (min={gp_min:.2} max={gp_max:.2}) gpu_rss_med={gr_m:.2} MiB (min={gr_min:.2} max={gr_max:.2}) gpu_peak_med={:.2} MiB | saved_priv={:.2} MiB saved_rss={:.2} MiB",
        stats(&cpk).0, stats(&gpk).0, gp_m - cp_m, gr_m - cr_m
    );
}

fn bench_cold_handover(exe: &std::path::Path, kind: &str, label: &str) {
    let (mut create_ms, mut renderer_ms, mut frame_ms, mut total_ms, mut before_p, mut after_p, mut ui_frames, mut ui_max) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..HANDOVER_ROUNDS {
        let out = Command::new(exe).args(["--sample-handover", kind]).output().expect("spawn handover sample");
        assert!(out.status.success(), "handover sample failed ({kind}): {}", String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let rest = stdout.lines().find(|l| l.starts_with("HANDOVER ")).expect("missing HANDOVER line").trim_start_matches("HANDOVER ");
        create_ms.push(parse_kv(rest, "gpu_create="));
        renderer_ms.push(parse_kv(rest, "renderer_new="));
        frame_ms.push(parse_kv(rest, "first_frame="));
        total_ms.push(parse_kv(rest, "total="));
        before_p.push(parse_kv(rest, "before_priv="));
        after_p.push(parse_kv(rest, "after_priv="));
        ui_frames.push(parse_kv(rest, "ui_frames="));
        ui_max.push(parse_kv(rest, "ui_max_ms="));
    }
    let (tm, tmin, tmax) = stats(&total_ms);
    println!(
        "SCENARIO cold_handover_{kind} ({HANDOVER_ROUNDS} fresh processes, {label}): total_med={tm:.2} ms (min={tmin:.2} max={tmax:.2}) [gpu_create_med={:.2} ms, renderer_new_med={:.2} ms, first_frame_med={:.2} ms] | ui_live_frames_med={:.0} ui_frame_max={:.3} ms | priv_med before={:.2} MiB after={:.2} MiB",
        stats(&create_ms).0, stats(&renderer_ms).0, stats(&frame_ms).0, stats(&ui_frames).0, stats(&ui_max).2, stats(&before_p).0, stats(&after_p).0
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--sample-memory") {
        run_memory_sample(args.get(2).map(String::as_str).unwrap_or("cpu"));
        return;
    }
    if args.get(1).map(String::as_str) == Some("--sample-handover") {
        run_handover_sample(args.get(2).map(String::as_str).unwrap_or("blur"));
        return;
    }
    if cfg!(debug_assertions) {
        eprintln!("cpu_vs_software_gpu_bench: debug build; run with --profile perf for real numbers");
    }

    let gpu = Gpu::headless();
    let info = gpu.adapter.get_info();
    let adapter_env = rune::env::var("WGPU_ADAPTER_NAME").unwrap_or_else(|| "<unset>".into());
    println!(
        "HEADER os={} arch={} adapter={:?} backend={:?} device_type={:?} WGPU_ADAPTER_NAME={} blend=srgb format={:?} size={}x{} scale={} cpu_threads={}",
        std::env::consts::OS, std::env::consts::ARCH, info.name, info.backend, info.device_type,
        adapter_env, GPU_FORMAT, WIDTH, HEIGHT, SCALE, painter_threads()
    );
    println!("NOTE headless off-screen benchmark: leaves out window creation, GDI BitBlt / wl_shm commit, and GPU swap-chain presentation; outliers kept");
    if info.device_type != wgpu::DeviceType::Cpu && !args.iter().any(|a| a == "--allow-hardware") {
        eprintln!("WARNING: selected adapter {:?} is {:?}, not DeviceType::Cpu; set WGPU_ADAPTER_NAME='Microsoft Basic Render Driver' (Windows) or WGPU_ADAPTER_NAME=llvmpipe (Linux)", info.name, info.device_type);
    }

    bench_hover(&gpu);
    bench_canvas_12500(&gpu);
    bench_scroll(&gpu, false, "scroll", 20);
    bench_spring(&gpu);
    bench_scroll(&gpu, true, "backdrop_blur", 10);
    bench_degraded_fallback(HandoverTrigger::Blur, NeedsGpu::LayerBlur, "degraded_blur");
    bench_degraded_fallback(HandoverTrigger::Rotate, NeedsGpu::TurnedLayer, "degraded_rotate");

    let exe = std::env::current_exe().expect("current_exe");
    bench_memory(&exe);
    bench_cold_handover(&exe, "rotate", "NeedsGpu::TurnedLayer");
    bench_cold_handover(&exe, "blur", "NeedsGpu::LayerBlur");
}
