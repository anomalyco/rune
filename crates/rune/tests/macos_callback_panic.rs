//! Headless macOS tests for panics inside AppKit / winit callbacks (issue #924).
//! Opens no window: runs accessory/windowless loops and probes in child processes.

#![allow(unsafe_code)]

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main(); }

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use block2::{Block, RcBlock};
    use objc2::{MainThreadMarker, msg_send, runtime::AnyObject};
    use objc2_app_kit::{NSApplication, NSEventMask};
    use objc2_foundation::{NSDate, NSString};
    use rune::{App, AppSpec, Cx, El, RunOptions, Tx, el};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    use winit::window::WindowId;

    const CASE: &str = "RUNE_MACOS_PANIC_CASE";

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopCopyCurrentMode(rl: *mut c_void) -> *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
        fn CFRunLoopPerformBlock(rl: *mut c_void, mode: *const c_void, block: &Block<dyn Fn()>);
        fn CFRunLoopWakeUp(rl: *mut c_void);
        fn CFRelease(cf: *const c_void);
    }

    fn current_mode() -> Option<String> {
        // SAFETY: main-thread CoreFoundation calls; CFStringRef is toll-free bridged to NSString and released once.
        unsafe {
            let mode = CFRunLoopCopyCurrentMode(CFRunLoopGetMain());
            if mode.is_null() {
                return None;
            }
            let s = (&*(mode as *const NSString)).to_string();
            CFRelease(mode);
            Some(s)
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct TypedPanic(u32);

    static QUIT_COUNT: AtomicUsize = AtomicUsize::new(0);
    static CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);
    static FAST_EXIT_COUNT: AtomicUsize = AtomicUsize::new(0);
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
    static SAW_TRACKING: AtomicBool = AtomicBool::new(false);

    fn count_fast_exit() {
        eprintln!("FAST_EXIT_RAN");
        FAST_EXIT_COUNT.fetch_add(1, Ordering::SeqCst);
    }

    struct DropGuard;
    impl Drop for DropGuard {
        fn drop(&mut self) {
            DROP_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn main() {
        if std::env::var_os(CASE).is_some() {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(15));
                eprintln!("macos_callback_panic: child timed out");
                std::process::exit(99);
            });
        }
        match rune::env::var(CASE).as_deref() {
            Some("probe_nested_tracking") => case_probe_nested_tracking(),
            Some("resumed_uncaught") => case_resumed_uncaught(),
            Some("resumed_caught") => case_resumed_caught(),
            Some("resumed_after_launch_caught") => case_resumed_after_launch_caught(),
            Some("user_event_caught") => case_user_event_caught(),
            Some("user_event_uncaught") => case_user_event_uncaught(),
            Some("nested_tracking_panic") => case_nested_tracking_panic(),
            _ => parent(),
        }
    }

    fn run_child(case: &str, extra_env: &[(&str, &str)]) -> (std::process::ExitStatus, String) {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.env(CASE, case).env("RUST_BACKTRACE", "0");
        for &(k, v) in extra_env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("spawn child");
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        println!("--- {case}: {:?}\n{log}", out.status);
        (out.status, log)
    }

    fn parent() {
        let (status, log) = run_child("probe_nested_tracking", &[]);
        assert!(status.success(), "probe_nested_tracking failed: {status:?}\n{log}");

        let (status, log) = run_child("resumed_uncaught", &[]);
        assert_eq!(status.signal(), None, "resumed_uncaught must not abort with signal: {status:?}\n{log}");
        assert_eq!(status.code(), Some(101), "resumed_uncaught must exit 101: {status:?}\n{log}");
        assert!(log.contains("boom-from-resumed"), "original message missing: {log}");
        assert!(!log.contains("FAST_EXIT_RAN"), "fast_exit must not run on panic: {log}");

        let (status, log) = run_child("resumed_caught", &[]);
        assert!(status.success(), "resumed_caught failed: {status:?}\n{log}");

        let (status, log) = run_child("resumed_after_launch_caught", &[("RUNE_WINDOW_AFTER_LAUNCH", "1")]);
        assert!(status.success(), "resumed_after_launch_caught failed: {status:?}\n{log}");

        let (status, log) = run_child("user_event_caught", &[]);
        assert!(status.success(), "user_event_caught failed: {status:?}\n{log}");

        let (status, log) = run_child("user_event_uncaught", &[]);
        assert_eq!(status.signal(), None, "user_event_uncaught must not abort with signal: {status:?}\n{log}");
        assert_eq!(status.code(), Some(101), "user_event_uncaught must exit 101: {status:?}\n{log}");
        assert!(log.contains("boom-from-user-event"), "original message missing: {log}");
        assert!(!log.contains("QUIT_RAN"), "on_quit must not run on panic: {log}");
        assert!(!log.contains("CLOSE_RAN"), "on_close must not run on panic: {log}");
        assert!(!log.contains("FAST_EXIT_RAN"), "fast_exit must not run on panic: {log}");

        let (status, log) = run_child("nested_tracking_panic", &[]);
        assert_eq!(status.code(), Some(101), "nested_tracking_panic must still end as panic (101): {status:?}\n{log}");
        assert!(
            log.contains("panic inside macOS nested run-loop mode NSEventTrackingRunLoopMode cannot unwind out of run()"),
            "expected clear nested-mode message: {log}"
        );
        assert!(log.contains("boom-inside-tracking"), "original panic message missing: {log}");
        assert!(!log.contains("FAST_EXIT_RAN"), "fast_exit must not run: {log}");

        println!("macos_callback_panic: ok");
    }

    fn case_resumed_uncaught() {
        rune::fast_exit(count_fast_exit);
        let options = RunOptions { accessory: true, ..Default::default() };
        rune::try_run_windows_with_args(options, std::iter::empty::<String>(), |_| -> Vec<rune::AnyWindowSpec> {
            panic!("boom-from-resumed");
        }).ok();
    }

    fn case_resumed_caught() {
        rune::fast_exit(count_fast_exit);
        let options = RunOptions { accessory: true, ..Default::default() };
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rune::try_run_windows_with_args(options, std::iter::empty::<String>(), |_| -> Vec<rune::AnyWindowSpec> {
                std::panic::panic_any(TypedPanic(924));
            }).ok();
        }));
        let err = caught.expect_err("panic in resumed must unwind out of try_run_windows_with_args");
        assert_eq!(err.downcast_ref::<TypedPanic>(), Some(&TypedPanic(924)));
        assert_eq!(FAST_EXIT_COUNT.load(Ordering::SeqCst), 0, "fast_exit must not run");
        println!("resumed_caught: ok");
    }

    fn case_resumed_after_launch_caught() {
        rune::fast_exit(count_fast_exit);
        let options = RunOptions { accessory: true, ..Default::default() };
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rune::try_run_windows_with_args(options, std::iter::empty::<String>(), |_| -> Vec<rune::AnyWindowSpec> {
                std::panic::panic_any(TypedPanic(925));
            }).ok();
        }));
        let err = caught.expect_err("panic in did_finish_launching must unwind out of try_run_windows_with_args");
        assert_eq!(err.downcast_ref::<TypedPanic>(), Some(&TypedPanic(925)));
        assert_eq!(FAST_EXIT_COUNT.load(Ordering::SeqCst), 0, "fast_exit must not run");
        println!("resumed_after_launch_caught: ok");
    }

    fn case_user_event_caught() {
        rune::fast_exit(count_fast_exit);
        let spec = AppSpec::new("panic-test", (100.0, 100.0), || DropGuard, |_: &DropGuard, _: &mut Cx<DropGuard>| -> El { el() });
        let app = App::new(spec)
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit(|_: &mut DropGuard, _: &mut Tx| { QUIT_COUNT.fetch_add(1, Ordering::SeqCst); })
            .on_close(|_: &mut DropGuard, _: &mut Tx| { CLOSE_COUNT.fetch_add(1, Ordering::SeqCst); });
        let handle = app.handle();
        handle.update(|_: &mut DropGuard, _: &mut Tx| {
            std::panic::panic_any(TypedPanic(77));
        });
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.try_run().ok();
        }));
        let err = caught.expect_err("panic in update must unwind out of try_run");
        assert_eq!(err.downcast_ref::<TypedPanic>(), Some(&TypedPanic(77)));
        assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 0, "on_quit must not run on panic");
        assert_eq!(CLOSE_COUNT.load(Ordering::SeqCst), 0, "on_close must not run on panic");
        assert_eq!(FAST_EXIT_COUNT.load(Ordering::SeqCst), 0, "fast_exit must not run on panic");
        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 1, "app state destructor must run on panic");
        println!("user_event_caught: ok");
    }

    fn case_user_event_uncaught() {
        rune::fast_exit(count_fast_exit);
        let spec = AppSpec::new("panic-test", (100.0, 100.0), || (), |_: &(), _: &mut Cx<()>| -> El { el() });
        let app = App::new(spec)
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit(|_: &mut (), _: &mut Tx| { eprintln!("QUIT_RAN"); })
            .on_close(|_: &mut (), _: &mut Tx| { eprintln!("CLOSE_RAN"); });
        let handle = app.handle();
        handle.update(|_: &mut (), _: &mut Tx| {
            panic!("boom-from-user-event");
        });
        app.try_run().ok();
    }

    /// Runs a tracking-mode nested loop from a run-loop block (so winit's `event_handler.ready()`
    /// is true, matching live resize and pop-up menu tracking) and triggers a panic inside a Rune
    /// callback while `CFRunLoopCopyCurrentMode` is `NSEventTrackingRunLoopMode`.
    fn case_nested_tracking_panic() {
        rune::fast_exit(count_fast_exit);
        let spec = AppSpec::new("tracking-panic", (100.0, 100.0), || (), |_: &(), _: &mut Cx<()>| -> El { el() });
        let app = App::new(spec)
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit(|_: &mut (), _: &mut Tx| { eprintln!("QUIT_RAN"); });
        let handle = app.handle();
        let trigger = handle.clone();
        handle.update(move |_: &mut (), _: &mut Tx| {
            // Defer entering NSEventTrackingRunLoopMode until after this handler returns so
            // winit's event_handler is ready when the tracking loop runs.
            let outer = RcBlock::new(move || {
                let tracking = NSString::from_str("NSEventTrackingRunLoopMode");
                let tracking_ref = (&*tracking as *const NSString).cast::<c_void>();
                let inner_handle = trigger.clone();
                let inner = RcBlock::new(move || {
                    inner_handle.update(|_: &mut (), _: &mut Tx| {
                        panic!("boom-inside-tracking");
                    });
                });
                // SAFETY: main thread; CoreFoundation run-loop block and bounded 0.4s tracking loop.
                unsafe {
                    let rl = CFRunLoopGetMain();
                    CFRunLoopPerformBlock(rl, tracking_ref, &inner);
                    CFRunLoopWakeUp(rl);
                    CFRunLoopRunInMode(tracking_ref, 0.4, 0);
                }
            });
            // SAFETY: main thread; kCFRunLoopDefaultMode is valid for process lifetime.
            unsafe {
                let rl = CFRunLoopGetMain();
                CFRunLoopPerformBlock(rl, kCFRunLoopDefaultMode, &outer);
                CFRunLoopWakeUp(rl);
            }
        });
        app.try_run().ok();
    }

    /// Empirically verifies on macOS that calling `event_loop.exit()` from inside a callback
    /// delivered during `NSEventTrackingRunLoopMode` (while `event_handler.ready()` is true) does
    /// NOT break out of `nextEventMatchingMask:LeftMouseUp` or `CFRunLoopRunInMode` early.
    fn case_probe_nested_tracking() {
        #[derive(Debug)]
        struct Wake;

        struct Probe {
            proxy: winit::event_loop::EventLoopProxy<Wake>,
        }

        impl ApplicationHandler<Wake> for Probe {
            fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
                assert_eq!(current_mode(), None, "resumed runs before CFRunLoop enters default mode");
                let proxy = self.proxy.clone();
                let outer = RcBlock::new(move || {
                    let tracking = NSString::from_str("NSEventTrackingRunLoopMode");
                    let tracking_ref = (&*tracking as *const NSString).cast::<c_void>();
                    let p1 = proxy.clone();
                    let b1 = RcBlock::new(move || { p1.send_event(Wake).ok(); });
                    // SAFETY: main thread; bounded 0.5s live-resize-shaped tracking call.
                    unsafe {
                        let rl = CFRunLoopGetMain();
                        CFRunLoopPerformBlock(rl, tracking_ref, &b1);
                        CFRunLoopWakeUp(rl);
                        let mtm = MainThreadMarker::new().unwrap();
                        let app = NSApplication::sharedApplication(mtm);
                        let until = NSDate::dateWithTimeIntervalSinceNow(0.5);
                        let t0 = rune::time::now();
                        let ev: *mut AnyObject = msg_send![
                            &*app,
                            nextEventMatchingMask: NSEventMask::LeftMouseUp.0,
                            untilDate: &*until,
                            inMode: &*tracking,
                            dequeue: true
                        ];
                        let elapsed = t0.elapsed();
                        println!(
                            "PROBE nextEventMatchingMask(LeftMouseUp, 0.5s, NSEventTrackingRunLoopMode) saw_tracking={} ev_null={} elapsed={:?}",
                            SAW_TRACKING.load(Ordering::SeqCst),
                            ev.is_null(),
                            elapsed
                        );
                        assert!(SAW_TRACKING.load(Ordering::SeqCst), "user_event must have fired inside NSEventTrackingRunLoopMode");
                        assert!(
                            elapsed >= Duration::from_millis(450),
                            "event_loop.exit() unexpectedly broke out of LeftMouseUp tracking loop early ({elapsed:?})"
                        );
                    }
                });
                // SAFETY: main thread; kCFRunLoopDefaultMode is valid for process lifetime.
                unsafe {
                    let rl = CFRunLoopGetMain();
                    CFRunLoopPerformBlock(rl, kCFRunLoopDefaultMode, &outer);
                    CFRunLoopWakeUp(rl);
                }
            }

            fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: Wake) {
                let mode = current_mode();
                println!("PROBE user_event fired in mode={mode:?}, calling event_loop.exit()");
                if mode.as_deref() == Some("NSEventTrackingRunLoopMode") {
                    SAW_TRACKING.store(true, Ordering::SeqCst);
                }
                event_loop.exit();
            }

            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }

        let mut builder = EventLoop::<Wake>::with_user_event();
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false)
            .with_activate_ignoring_other_apps(false);
        let event_loop = builder.build().unwrap();
        let mut probe = Probe { proxy: event_loop.create_proxy() };
        event_loop.run_app(&mut probe).unwrap();
        println!("probe_nested_tracking: ok");
    }
}
