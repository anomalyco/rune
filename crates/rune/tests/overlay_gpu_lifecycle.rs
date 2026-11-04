//! A prewarmed overlay on Windows and X11 makes its window at startup and no GPU surface, nor
//! device, until it shows; with `release_gpu_when_hidden` each hide gives the surface back,
//! and the device with the last surface on it, and the next show makes them again. Prints how
//! long each show waits for its surface and first present, and the process's private memory
//! along the way. Opens overlays, so it only runs with RUNE_GUI_TESTS=1 in a desktop session.

#[cfg(target_os = "macos")]
fn main() {}

#[cfg(not(target_os = "macos"))]
fn main() { lazy::main() }

#[cfg(not(target_os = "macos"))]
mod lazy {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use rune::*;

    static STARTS: AtomicUsize = AtomicUsize::new(0);
    static DREW: Mutex<Option<Instant>> = Mutex::new(None);

    fn view(_: &(), _: &mut Cx<()>) -> El { el().w_full().h_full().fill(oklcha(0.6, 0.2, 250.0, 0.2)) }
    fn hud(_: &(), _: &mut Cx<()>) -> El { el().size(200.0, 40.0).fill(oklcha(0.2, 0.0, 0.0, 0.6)).radius(20.0) }

    fn wait(what: &str, limit: Duration, f: impl Fn() -> bool) -> Option<Duration> {
        let start = Instant::now();
        while !f() {
            if start.elapsed() > limit { eprintln!("timed out waiting for {what}"); return None; }
            std::thread::sleep(Duration::from_micros(200));
        }
        Some(start.elapsed())
    }

    /// The time from `show` to the surface existing and to its first present.
    fn show<T>(name: &str, overlay: &OverlayHandle<T>) -> (Duration, Option<Duration>) {
        let presents = overlay.surface_stats().totals.presents;
        let start = Instant::now();
        overlay.show();
        let made = wait(&format!("{name}'s surface"), Duration::from_secs(10), || overlay.surface_description().is_some())
            .unwrap_or_else(|| panic!("{name} made no surface: {:?}", overlay.failure()));
        // A locked session may present nothing; the surface is what the test checks.
        let presented = wait(&format!("{name}'s present"), Duration::from_secs(2), || overlay.surface_stats().totals.presents > presents)
            .map(|_| start.elapsed());
        println!("{name}: surface {:.1} ms, first present {}", ms(made), presented.map_or("none".into(), |p| format!("{:.1} ms", ms(p))));
        (made, presented)
    }

    fn hide<T>(name: &str, overlay: &OverlayHandle<T>, released: bool) {
        overlay.hide();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(overlay.surface_description().is_none(), released, "{name}: hidden, its surface is {}", if released { "released" } else { "kept" });
    }

    #[cfg(target_os = "windows")]
    fn private_mb() -> f64 {
        #[repr(C)]
        #[derive(Default)]
        struct Counters { cb: u32, faults: u32, peak_ws: usize, ws: usize, qpp: usize, qp: usize, qnpp: usize, qnp: usize, pagefile: usize, peak_pagefile: usize, private: usize }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
        }
        let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
        // SAFETY: fills `c`, whose `cb` names PROCESS_MEMORY_COUNTERS_EX.
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        c.private as f64 / 1e6
    }

    #[cfg(not(target_os = "windows"))]
    fn private_mb() -> f64 { 0.0 }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("overlay_gpu_lifecycle: skipped (set RUNE_GUI_TESTS=1 in a desktop session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(90)); eprintln!("timeout"); std::process::exit(2) });
        on_gpu(|event| {
            match event {
                GpuEvent::Starting => { STARTS.fetch_add(1, Ordering::SeqCst); }
                GpuEvent::Drew => { *DREW.lock().unwrap() = Some(Instant::now()); }
            }
        });
        let mut app = App::new(AppSpec::new("overlay gpu lifecycle", (100.0, 100.0), || (), view)).args(Vec::new()).windowless();
        let created = Arc::new(AtomicUsize::new(0));
        let seen = created.clone();
        let full = app.overlay(OverlaySpec::new("lazy full screen", (100.0, 100.0), view)
            .options(OverlayOptions { placement: Some(OverlayPlacement::FullScreen), prewarm: true, ..Default::default() })
            .release_gpu_when_hidden()
            .on_created(move |_| { seen.fetch_add(1, Ordering::SeqCst); }), ());
        let small = app.overlay(OverlaySpec::new("kept hud", (200.0, 40.0), hud), ());
        let handle = app.handle();
        let result = Arc::new(Mutex::new(None));
        let out = result.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                wait("startup", Duration::from_secs(10), || full.availability() == Availability::Available).expect("the prewarmed window");
                std::thread::sleep(Duration::from_millis(500));
                assert_eq!(created.load(Ordering::SeqCst), 1, "on_created runs at startup");
                assert!(full.surface_description().is_none(), "a hidden prewarmed overlay has no surface");
                assert_eq!(STARTS.load(Ordering::SeqCst), 0, "nothing shown, no GPU device");
                let idle = private_mb();

                let (cold, _) = show("first show, new device", &full);
                assert_eq!(STARTS.load(Ordering::SeqCst), 1);
                println!("  {}", full.surface_description().unwrap());
                let shown = private_mb();
                hide("full screen", &full, true);
                std::thread::sleep(Duration::from_millis(400));
                let released = private_mb();

                let (again, _) = show("second show, device released in between", &full);
                assert_eq!(STARTS.load(Ordering::SeqCst), 2, "the released device is made again");
                hide("full screen", &full, true);

                let (hud_cold, _) = show("kept overlay, new device", &small);
                let (warm, _) = show("full screen on the kept overlay's device", &full);
                assert_eq!(STARTS.load(Ordering::SeqCst), 3, "the full-screen overlay shares the kept overlay's device");
                hide("full screen", &full, true);
                hide("kept overlay", &small, false);
                let (hud_warm, _) = show("kept overlay, shown again", &small);
                hide("kept overlay", &small, false);
                assert!(full.failure().is_none() && small.failure().is_none(), "{:?} {:?}", full.failure(), small.failure());
                assert!(DREW.lock().unwrap().is_some() || std::env::var_os("RUNE_LOCKED").is_some(), "a frame presented");
                println!("private memory: hidden at startup {idle:.0} MB, full screen shown {shown:.0} MB, hidden again {released:.0} MB");
                println!("surface ready after show (ms): cold {:.1}, again {:.1}, on a kept device {:.1}; kept overlay cold {:.1}, again {:.1}",
                    ms(cold), ms(again), ms(warm), ms(hud_cold), ms(hud_warm));
            });
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        assert_eq!(*result.lock().unwrap(), Some(true), "overlay_gpu_lifecycle failed");
        println!("overlay_gpu_lifecycle: ok");
    }

    fn ms(d: Duration) -> f64 { d.as_secs_f64() * 1000.0 }
}
