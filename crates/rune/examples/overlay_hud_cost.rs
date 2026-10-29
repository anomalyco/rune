//! A real overlay window drawing a continuously animating HUD through its compositor, with an
//! empty Rune view, for profiling the per-frame main-thread cost of Rune's frame loop.
//! Runs `OVERLAY_SECONDS` (default 12), then prints frames, main-thread CPU per frame, the share
//! of it spent in the compositor callback, and heap allocations per frame (every thread).
//! Profile it with Time Profiler or samply.
//!
//! - `OVERLAY_WRITES=n`: the compositor writes a 256-byte uniform buffer `n` times a frame,
//!   through `Queue::write_buffer`, or through `Frame::write_buffer` with `OVERLAY_STAGED=1`.
//! - `OVERLAY_SHOW_HZ=n`: another thread calls `OverlayHandle::show` `n` times a second, as an
//!   app that keeps an overlay under the pointer does.
//! - `OVERLAY_COUNT_METAL=1` counts the Metal calls a frame makes (command buffers committed,
//!   render and blit encoders, presents, completion handlers), on macOS. `=labels` also tallies
//!   commits by command buffer label and thread; `OVERLAY_COMMIT_STACKS=1` adds who committed.
//! - Every run prints CPU by thread (main, present, display link, the rest) and the process's
//!   retired instructions per frame, which compare runs that CPU time can't on a Mac whose
//!   cores change speed.
//! - `OVERLAY_PRESUME_VISIBLE=1` makes every window report itself on screen, so a Mac whose
//!   screen is locked still draws (its frames never reach the display): only for profiling.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rune::compose::{Compositor, Frame};
#[cfg(target_os = "macos")]
#[path = "hud_probe/mod.rs"]
mod probe;
use rune::{App, AppSpec, Cx, El, OverlaySpec, RunOptions, WindowSpec, el};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);

// SAFETY: forwards to the system allocator, counting calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { unsafe { System.dealloc(ptr, layout) } }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn view(_: &(), _: &mut Cx<()>) -> El { el() }

fn thread_cpu_ms() -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a live, writable timespec; the clock needs no handle.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    t.tv_sec as f64 * 1e3 + t.tv_nsec as f64 / 1e6
}

fn process_cpu_ms() -> f64 {
    // SAFETY: a zeroed rusage is valid, and getrusage only writes it.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let ms = |t: libc::timeval| t.tv_sec as f64 * 1e3 + t.tv_usec as f64 / 1e3;
    ms(u.ru_utime) + ms(u.ru_stime)
}

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

const WARMUP: Duration = Duration::from_secs(2);

struct Start { main: f64, process: f64, wall: Instant, frames: u64, allocs: u64, threads: (f64, f64), metal: [u64; 5], counts: [u64; 3] }

/// CPU ms of the present thread and of the display link's thread.
fn side_threads() -> (f64, f64) {
    #[cfg(target_os = "macos")]
    {
        let threads = probe::named_threads();
        (probe::thread_ms(&threads, "rune present"), probe::thread_ms(&threads, "CVDisplayLink"))
    }
    #[cfg(not(target_os = "macos"))]
    (0.0, 0.0)
}

fn process_counts() -> [u64; 3] {
    #[cfg(target_os = "macos")]
    return probe::process_counts();
    #[cfg(not(target_os = "macos"))]
    [0; 3]
}

fn metal_counts() -> [u64; 5] {
    #[cfg(target_os = "macos")]
    return probe::metal_counts();
    #[cfg(not(target_os = "macos"))]
    [0; 5]
}

struct Hud {
    start: Instant,
    seconds: f64,
    frames: u64,
    from: Option<Start>,
    in_callback: f64,
    writes: usize,
    staged: bool,
    uniforms: Option<wgpu::Buffer>,
}

impl Compositor<()> for Hud {
    fn render(&mut self, f: &mut Frame<'_, ()>) {
        let entered = thread_cpu_ms();
        self.frames += 1;
        let phase = (self.start.elapsed().as_secs_f64() * 2.0).sin() * 0.5 + 0.5;
        if self.writes > 0 {
            let uniforms = self.uniforms.get_or_insert_with(|| f.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hud uniforms"), size: 256, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            })).clone();
            let data = [phase as f32; 64];
            for _ in 0..self.writes {
                if self.staged { f.write_buffer(&uniforms, 0, bytemuck_cast(&data)); }
                else { f.queue.write_buffer(&uniforms, 0, bytemuck_cast(&data)); }
            }
        }
        {
            let _pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hud"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: f.target, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: phase * 0.4, a: 0.4 }), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
        }
        let elapsed = self.start.elapsed();
        if self.from.is_none() && elapsed >= WARMUP {
            self.from = Some(Start { main: entered, process: process_cpu_ms(), wall: Instant::now(), frames: self.frames,
                allocs: ALLOCS.load(Ordering::Relaxed), threads: side_threads(), metal: metal_counts(), counts: process_counts() });
            self.in_callback = 0.0;
        }
        if let Some(s) = &self.from && elapsed.as_secs_f64() >= self.seconds {
            let frames = (self.frames - s.frames) as f64;
            let wall = s.wall.elapsed().as_secs_f64();
            let main = thread_cpu_ms() - s.main;
            let process = process_cpu_ms() - s.process;
            let (present, link) = side_threads();
            let (present, link) = (present - s.threads.0, link - s.threads.1);
            println!(
                "frames={frames} fps={:.1} main_ms_per_s={:.1} main_us_per_frame={:.1} callback_us_per_frame={:.1} process_cpu_pct={:.1} allocs_per_frame={:.1}",
                frames / wall, main / wall, main * 1e3 / frames, self.in_callback * 1e3 / frames,
                process / wall / 10.0,
                (ALLOCS.load(Ordering::Relaxed) - s.allocs) as f64 / frames,
            );
            println!(
                "present_ms_per_s={:.1} display_link_ms_per_s={:.1} other_ms_per_s={:.1} process_ms_per_s={:.1}",
                present / wall, link / wall, (process - main - present - link) / wall, process / wall,
            );
            let counts = process_counts();
            println!("process_per_frame: kinstructions={:.1} kcycles={:.1} wakeups={:.2}",
                (counts[0] - s.counts[0]) as f64 / frames / 1e3, (counts[1] - s.counts[1]) as f64 / frames / 1e3,
                (counts[2] - s.counts[2]) as f64 / frames);
            let metal = metal_counts();
            if metal != s.metal {
                let per = |i: usize| (metal[i] - s.metal[i]) as f64 / frames;
                println!("per_frame: command_buffers={:.2} render_encoders={:.2} blit_encoders={:.2} presents={:.2} completed_handlers={:.2}",
                    per(0), per(1), per(2), per(3), per(4));
            }
            #[cfg(target_os = "macos")]
            for (label, n) in probe::LABELS.lock().unwrap().iter() { println!("  commits: {label}: {n}"); }
            std::process::exit(0);
        }
        self.in_callback += thread_cpu_ms() - entered;
    }

    fn animating(&self) -> bool { true }
}

fn bytemuck_cast(data: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding or invalid bit patterns as bytes.
    unsafe { std::slice::from_raw_parts(data.as_ptr().cast(), std::mem::size_of_val(data)) }
}

#[cfg(target_os = "macos")]
fn presume_visible() {
    use std::ffi::{c_char, c_void};
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn class_getInstanceMethod(class: *mut c_void, sel: *mut c_void) -> *mut c_void;
        fn method_setImplementation(method: *mut c_void, imp: *const c_void) -> *const c_void;
    }
    extern "C" fn visible(_: *mut c_void, _: *mut c_void) -> usize { 1 << 1 }
    // SAFETY: replaces -[NSWindow occlusionState] with one of the same signature.
    unsafe {
        let method = class_getInstanceMethod(objc_getClass(c"NSWindow".as_ptr()), sel_registerName(c"occlusionState".as_ptr()));
        assert!(!method.is_null());
        method_setImplementation(method, visible as *const c_void);
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    if std::env::var_os("OVERLAY_PRESUME_VISIBLE").is_some() { presume_visible(); }
    #[cfg(target_os = "macos")]
    if let Some(mode) = std::env::var_os("OVERLAY_COUNT_METAL") {
        probe::count_metal();
        probe::TALLY_LABELS.store(mode == "labels", Ordering::Relaxed);
    }
    let seconds = env("OVERLAY_SECONDS", 12.0);
    let writes = env("OVERLAY_WRITES", 0);
    let staged = env("OVERLAY_STAGED", 0) != 0;
    let show_hz = env("OVERLAY_SHOW_HZ", 0.0);
    rune::run_windows_with_args(RunOptions { accessory: true }, [], move |_| {
        let spec = AppSpec::new("Overlay HUD cost", (360.0, 80.0), || (), view);
        let mut app = App::new(spec).windowless();
        let hud = Hud { start: Instant::now(), seconds, frames: 0, from: None, in_callback: 0.0, writes, staged, uniforms: None };
        let overlay = app.overlay(OverlaySpec {
            name: "HUD", size: (360.0, 80.0), edge: rune::ScreenEdge::Bottom,
            margin: 60.0, align: rune::Align::Center, inset: 0.0, interactive: false, view,
        }.options(Default::default()).compositor(hud), ());
        overlay.show();
        if show_hz > 0.0 {
            let overlay = overlay.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs_f64(1.0 / show_hz));
                overlay.show();
            });
        }
        let handle = app.handle();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs_f64(seconds + 20.0));
            eprintln!("overlay_hud_cost: no frames came; quitting");
            handle.quit();
        });
        vec![WindowSpec::from_app("hud", app)]
    });
}
