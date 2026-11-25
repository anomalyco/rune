//! Live Wayland E2E test of `App::on_window_geometry` on a headless Sway compositor
//! (`WLR_BACKENDS=headless WLR_RENDERER=pixman sway`). Verifies:
//! 1. Default `CpuFirst::Auto` delivers the creation sample after `on_window` and before the
//!    first drawn frame and focus.
//! 2. Output entry at scale 1 delivers `has_output: true`.
//! 3. `swaymsg output HEADLESS-1 disable` (which removes the `wl_output` global without sending
//!    `wl_surface.leave`) wakes the idle loop and delivers `has_output: false`.
//! 4. `swaymsg output HEADLESS-1 enable` delivers `has_output: true` again even though the
//!    removed output stays at index 0 of `sctk`'s `SurfaceData::outputs`.
//! 5. With two outputs (`swaymsg create_output`), disabling the first while the window is on the
//!    second keeps `has_output: true`.
//!
//! Runs only when `RUNE_GUI_TESTS=1`, `WAYLAND_DISPLAY`, and `SWAYSOCK` are set.

#[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn main() {}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn main() { wayland::main(); }

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod wayland {
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use rune::{App, AppSpec, WindowGeometry, el};

    #[derive(Debug, Clone, PartialEq)]
    enum Ev {
        OnWindow,
        Geometry(WindowGeometry),
        FrameDrawn,
        Focus(bool),
    }

    #[derive(Default)]
    struct State {
        log: Arc<Mutex<Vec<Ev>>>,
    }

    fn swaymsg(args: &[&str]) {
        let out = Command::new("swaymsg").args(args).output().expect("run swaymsg");
        assert!(
            out.status.success(),
            "swaymsg {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr),
        );
    }

    fn wait_until(log: &Arc<Mutex<Vec<Ev>>>, timeout: Duration, pred: impl Fn(&[Ev]) -> bool) -> bool {
        let start = rune::time::now();
        while start.elapsed() < timeout {
            if pred(&log.lock().unwrap()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    pub fn main() {
        if rune::env::var("RUNE_GUI_TESTS").as_deref() != Some("1")
            || rune::env::var("WAYLAND_DISPLAY").as_deref().is_none_or(str::is_empty)
            || rune::env::var("SWAYSOCK").as_deref().is_none_or(str::is_empty)
        {
            println!("wayland_window_geometry: skipped (set RUNE_GUI_TESTS=1 with WAYLAND_DISPLAY and SWAYSOCK)");
            return;
        }

        rune::orderly_exit();
        let log: Arc<Mutex<Vec<Ev>>> = Arc::new(Mutex::new(Vec::new()));
        let init_log = log.clone();
        let app = App::new(AppSpec::new(
            "wl-geom-e2e",
            (640.0, 420.0),
            State::default,
            |_, _| el(),
        ))
        .init(move || State { log: init_log.clone() });
        let handle = app.handle();

        let driver_log = log.clone();
        let driver_handle = handle.clone();
        let driver = std::thread::spawn(move || {
            // Wait for initial output entry (has_output == true) after creation sample.
            let entered = wait_until(&driver_log, Duration::from_secs(6), |evs| {
                evs.iter().any(|e| matches!(e, Ev::Geometry(g) if g.has_output))
            });
            assert!(entered, "timed out waiting for initial has_output=true; log={:?}", *driver_log.lock().unwrap());

            let count_before_disable = driver_log.lock().unwrap().len();
            swaymsg(&["output", "HEADLESS-1", "disable"]);
            let left = wait_until(&driver_log, Duration::from_secs(6), |evs| {
                evs[count_before_disable..].iter().any(|e| matches!(e, Ev::Geometry(g) if !g.has_output))
            });
            assert!(left, "timed out waiting for has_output=false after HEADLESS-1 disable; log={:?}", *driver_log.lock().unwrap());

            let count_before_enable = driver_log.lock().unwrap().len();
            swaymsg(&["output", "HEADLESS-1", "enable"]);
            let re_entered = wait_until(&driver_log, Duration::from_secs(6), |evs| {
                evs[count_before_enable..].iter().any(|e| matches!(e, Ev::Geometry(g) if g.has_output))
            });
            assert!(re_entered, "timed out waiting for has_output=true after HEADLESS-1 re-enable; log={:?}", *driver_log.lock().unwrap());

            // Create a second output, move window to it, disable HEADLESS-1: has_output must stay true.
            swaymsg(&["create_output"]);
            std::thread::sleep(Duration::from_millis(150));
            swaymsg(&["move", "container", "to", "output", "HEADLESS-2"]);
            std::thread::sleep(Duration::from_millis(200));
            let count_before_multi_disable = driver_log.lock().unwrap().len();
            swaymsg(&["output", "HEADLESS-1", "disable"]);
            std::thread::sleep(Duration::from_millis(300));
            let false_after_multi = driver_log.lock().unwrap()[count_before_multi_disable..]
                .iter()
                .any(|e| matches!(e, Ev::Geometry(g) if !g.has_output));
            assert!(!false_after_multi, "disabling HEADLESS-1 while window is on HEADLESS-2 must keep has_output=true; log={:?}", *driver_log.lock().unwrap());

            driver_handle.quit();
        });

        let win_log = log.clone();
        let frame_log = log.clone();
        app.on_window(move |_| win_log.lock().unwrap().push(Ev::OnWindow))
            .on_window_geometry(|s, g, _| {
                s.log.lock().unwrap().push(Ev::Geometry(g));
                false
            })
            .on_focus(|s, focused, _| {
                s.log.lock().unwrap().push(Ev::Focus(focused));
            })
            .on_frame_drawn(move |_| {
                frame_log.lock().unwrap().push(Ev::FrameDrawn);
            })
            .run();

        driver.join().expect("driver thread");
        let evs = log.lock().unwrap();
        let on_win = evs.iter().position(|e| matches!(e, Ev::OnWindow)).expect("on_window");
        let first_geom = evs.iter().position(|e| matches!(e, Ev::Geometry(_))).expect("first geometry");
        let first_frame = evs.iter().position(|e| matches!(e, Ev::FrameDrawn));
        let first_focus = evs.iter().position(|e| matches!(e, Ev::Focus(true)));
        assert!(on_win < first_geom, "creation geometry must run after on_window: {evs:?}");
        if let Some(frame) = first_frame {
            assert!(first_geom < frame, "CPU-first creation geometry must run before first drawn frame: {evs:?}");
        }
        if let Some(focus) = first_focus {
            assert!(first_geom < focus, "CPU-first creation geometry must run before first focus: {evs:?}");
        }
        println!("wayland_window_geometry: ok ({} events logged)", evs.len());
    }
}
