//! Off-screen macOS integration test for `App::on_quit_request` and `QuitResponse`.
//! Spawns child processes with `NSApplicationActivationPolicyProhibited` and windowless hosts
//! so ZERO windows, ZERO Dock icons, ZERO menu bar changes, and ZERO system dialogs appear,
//! while driving real `-[NSApplication terminate:]`, `applicationShouldTerminate:`, and
//! `replyToApplicationShouldTerminate:`.

#![allow(unsafe_code)]

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main(); }

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use block2::{Block, RcBlock};
    use objc2::{MainThreadMarker, msg_send, runtime::AnyObject};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use rune::{App, AppSpec, QuitResponse, RunOptions, WindowSpec, el};

    const CASE: &str = "RUNE_MACOS_QUIT_REQUEST_CASE";
    const TERMINATE_NOW: usize = 1;
    const TERMINATE_LATER: usize = 2;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopPerformBlock(rl: *mut c_void, mode: *const c_void, block: &Block<dyn Fn()>);
        fn CFRunLoopWakeUp(rl: *mut c_void);
    }

    fn on_default_mode(f: impl FnOnce() + 'static) {
        let f = Cell::new(Some(f));
        let block = RcBlock::new(move || {
            if let Some(f) = f.take() {
                f();
            }
        });
        // SAFETY: main-thread CoreFoundation calls; kCFRunLoopDefaultMode is static and
        // CFRunLoopPerformBlock copies the block.
        unsafe {
            let rl = CFRunLoopGetMain();
            CFRunLoopPerformBlock(rl, kCFRunLoopDefaultMode, &block);
            CFRunLoopWakeUp(rl);
        }
    }

    fn check_hidden_app(mtm: MainThreadMarker) -> objc2::rc::Retained<NSApplication> {
        let app = NSApplication::sharedApplication(mtm);
        assert!(
            matches!(
                app.activationPolicy(),
                NSApplicationActivationPolicy::Accessory | NSApplicationActivationPolicy::Prohibited
            ),
            "unexpected activation policy: {:?}",
            app.activationPolicy(),
        );
        assert!(app.windows().is_empty(), "off-screen test must open no windows");
        app
    }

    /// Queues `f` to run on `kCFRunLoopDefaultMode` on the first idle turn after `Windows::resumed`
    /// (and `quit::install`) completes.
    fn after_resumed<S: 'static>(handle: &rune::AppHandle<S>, f: impl FnOnce() + Send + 'static) {
        let mut f = Some(f);
        handle.update(move |_, _| {
            if let Some(f) = f.take() {
                on_default_mode(f);
            }
        });
    }

    fn ask_delegate_directly(app: &NSApplication) -> usize {
        // SAFETY: main-thread AppKit calls on the live NSApplication and its winit delegate.
        unsafe {
            let delegate: *mut AnyObject = msg_send![app, delegate];
            assert!(!delegate.is_null(), "NSApplication delegate missing");
            msg_send![delegate, applicationShouldTerminate: app]
        }
    }

    fn trigger_real_terminate(app: &NSApplication) {
        // SAFETY: main-thread AppKit call sending real -[NSApplication terminate:nil].
        unsafe {
            let nil: *mut AnyObject = std::ptr::null_mut();
            let _: () = msg_send![app, terminate: nil];
        }
    }

    static HOOK_COUNT: AtomicUsize = AtomicUsize::new(0);
    static QUIT_COUNT: AtomicUsize = AtomicUsize::new(0);
    static ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    static HOST1_DEFER: AtomicBool = AtomicBool::new(false);

    pub fn main() {
        if std::env::var_os(CASE).is_some() {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(15));
                eprintln!("macos_quit_request: child timed out");
                std::process::exit(99);
            });
        }
        match rune::env::var(CASE).as_deref() {
            Some("no_hook_delegate_now") => case_no_hook_delegate_now(),
            Some("no_hook_terminate") => case_no_hook_terminate(),
            Some("hook_delegate_later") => case_hook_delegate_later(),
            Some("hook_returns_quit") => case_hook_returns_quit(),
            Some("hook_defers_then_tx_quit") => case_hook_defers_then_tx_quit(),
            Some("tx_quit_inside_hook") => case_tx_quit_inside_hook(),
            Some("two_hosts_order_and_short_circuit") => case_two_hosts_order_and_short_circuit(),
            _ => parent(),
        }
    }

    fn run_child(case: &str) -> (std::process::ExitStatus, String) {
        let out = Command::new(std::env::current_exe().expect("current_exe"))
            .env(CASE, case)
            .env("RUST_BACKTRACE", "0")
            .output()
            .expect("spawn child");
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        println!("--- {case}: {:?}\n{log}", out.status);
        (out.status, log)
    }

    fn parent() {
        for (case, expected_markers) in [
            ("no_hook_delegate_now", &["DELEGATE_ANSWER=1", "QUIT_RAN=1"][..]),
            ("no_hook_terminate", &["BEFORE_TERMINATE", "QUIT_RAN=1"][..]),
            ("hook_delegate_later", &["DELEGATE_ANSWER=2", "HOOK_RAN=1", "QUIT_RAN=1"][..]),
            ("hook_returns_quit", &["HOOK_RAN=1", "QUIT_RAN=1"][..]),
            (
                "hook_defers_then_tx_quit",
                &[
                    "TERMINATE1_RETURNED",
                    "DEFAULT_MODE_UPDATE=1",
                    "TERMINATE2_RETURNED",
                    "HOOK_TOTAL=2",
                    "QUIT_RAN=1",
                ][..],
            ),
            ("tx_quit_inside_hook", &["HOOK_RAN=1", "QUIT_RAN=1"][..]),
            (
                "two_hosts_order_and_short_circuit",
                &["AFTER_T1=h1,h2", "AFTER_T2=h1,h2,h1", "QUIT_TOTAL=2"][..],
            ),
        ] {
            let (status, log) = run_child(case);
            assert_eq!(status.signal(), None, "{case} aborted with signal: {status:?}\n{log}");
            assert!(status.success(), "{case} failed: {status:?}\n{log}");
            for marker in expected_markers {
                assert!(log.contains(marker), "{case} missing {marker:?} in:\n{log}");
            }
        }
        println!("macos_quit_request: ok");
    }

    fn case_no_hook_delegate_now() {
        rune::orderly_exit();
        let app = App::new(AppSpec::new("no-hook-now", (120.0, 80.0), || (), |_, _| el()))
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit(|_, _| {
                let n = QUIT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("QUIT_RAN={n}");
            });
        let handle = app.handle();
        let quit_handle = handle.clone();
        after_resumed(&handle, move || {
            let mtm = MainThreadMarker::new().expect("main thread");
            let ns = check_hidden_app(mtm);
            let reply = ask_delegate_directly(&ns);
            eprintln!("DELEGATE_ANSWER={reply}");
            assert_eq!(reply, TERMINATE_NOW);
            quit_handle.quit();
        });
        app.run();
    }

    fn case_no_hook_terminate() {
        let app = App::new(AppSpec::new("no-hook-term", (120.0, 80.0), || (), |_, _| el()))
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit(|_, _| {
                let n = QUIT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("QUIT_RAN={n}");
            });
        let handle = app.handle();
        after_resumed(&handle, || {
            let mtm = MainThreadMarker::new().expect("main thread");
            let ns = check_hidden_app(mtm);
            eprintln!("BEFORE_TERMINATE");
            trigger_real_terminate(&ns);
        });
        app.run();
    }

    fn case_hook_delegate_later() {
        rune::orderly_exit();
        let app = App::new(AppSpec::new("hook-later", (120.0, 80.0), || (), |_, _| el()))
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit_request(|_, _| {
                let n = HOOK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("HOOK_RAN={n}");
                QuitResponse::Defer
            })
            .on_quit(|_, _| {
                let n = QUIT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("QUIT_RAN={n}");
            });
        let handle = app.handle();
        let quit_handle = handle.clone();
        after_resumed(&handle, move || {
            let mtm = MainThreadMarker::new().expect("main thread");
            let ns = check_hidden_app(mtm);
            let reply = ask_delegate_directly(&ns);
            eprintln!("DELEGATE_ANSWER={reply}");
            assert_eq!(reply, TERMINATE_LATER);
            // Let WakeEvent::QuitRequest run and reply NO before exiting programmatically.
            std::thread::spawn(move || {
                let start = rune::time::now();
                while HOOK_COUNT.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                std::thread::sleep(Duration::from_millis(50));
                quit_handle.quit();
            });
        });
        app.run();
    }

    fn case_hook_returns_quit() {
        let app = App::new(AppSpec::new("hook-quit", (120.0, 80.0), || (), |_, _| el()))
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit_request(|_, _| {
                assert!(!rune::quitting(), "quitting() must be false while on_quit_request runs");
                let n = HOOK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("HOOK_RAN={n}");
                QuitResponse::Quit
            })
            .on_quit(|_, _| {
                assert!(rune::quitting(), "quitting() must be true in on_quit");
                let n = QUIT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("QUIT_RAN={n}");
            });
        let handle = app.handle();
        after_resumed(&handle, || {
            let mtm = MainThreadMarker::new().expect("main thread");
            let ns = check_hidden_app(mtm);
            trigger_real_terminate(&ns);
        });
        app.run();
    }

    fn case_hook_defers_then_tx_quit() {
        rune::orderly_exit();
        let app = App::new(AppSpec::new("hook-defer", (120.0, 80.0), || 0u32, |_, _| el()))
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit_request(|s, _| {
                assert!(!rune::quitting(), "quitting() must stay false on Defer");
                *s += 1;
                let n = HOOK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("HOOK_CALL={n}");
                QuitResponse::Defer
            })
            .on_quit(|_, _| {
                let n = QUIT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("QUIT_RAN={n}");
            });
        let handle = app.handle();
        let drive = handle.clone();
        after_resumed(&handle, move || {
            let handle = drive;
            let mtm = MainThreadMarker::new().expect("main thread");
            let ns = check_hidden_app(mtm);
            // 1. First real terminate: returns once replyToApplicationShouldTerminate:NO runs.
            trigger_real_terminate(&ns);
            eprintln!("TERMINATE1_RETURNED");
            assert_eq!(HOOK_COUNT.load(Ordering::SeqCst), 1);
            assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 0);
            assert!(!rune::quitting());

            // 2. Prove the loop returned to kCFRunLoopDefaultMode and processes normal updates,
            //    then fire a second real terminate: and finally exit via Tx::quit().
            let h = handle.clone();
            on_default_mode(move || {
                h.update(|s, _| {
                    assert_eq!(*s, 1);
                    eprintln!("DEFAULT_MODE_UPDATE=1");
                });
                let h2 = h.clone();
                on_default_mode(move || {
                    let mtm = MainThreadMarker::new().expect("main thread");
                    let ns = NSApplication::sharedApplication(mtm);
                    trigger_real_terminate(&ns);
                    eprintln!("TERMINATE2_RETURNED");
                    assert_eq!(HOOK_COUNT.load(Ordering::SeqCst), 2);
                    assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 0);
                    eprintln!("HOOK_TOTAL={}", HOOK_COUNT.load(Ordering::SeqCst));
                    // 3. Programmatic Tx::quit() exits cleanly without re-asking on_quit_request.
                    h2.update(|_, tx| tx.quit());
                });
            });
        });
        app.run();
        assert_eq!(HOOK_COUNT.load(Ordering::SeqCst), 2, "Tx::quit must not re-invoke on_quit_request");
        assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 1);
    }

    fn case_tx_quit_inside_hook() {
        let app = App::new(AppSpec::new("tx-quit-in-hook", (120.0, 80.0), || (), |_, _| el()))
            .args(Vec::<String>::new())
            .accessory()
            .windowless()
            .on_quit_request(|_, tx| {
                let n = HOOK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("HOOK_RAN={n}");
                // Calling Tx::quit() inside on_quit_request must upgrade the pending
                // NSTerminateLater reply to YES even when the hook returns Defer.
                tx.quit();
                QuitResponse::Defer
            })
            .on_quit(|_, _| {
                let n = QUIT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("QUIT_RAN={n}");
            });
        let handle = app.handle();
        after_resumed(&handle, || {
            let mtm = MainThreadMarker::new().expect("main thread");
            let ns = check_hidden_app(mtm);
            trigger_real_terminate(&ns);
        });
        app.run();
    }

    fn case_two_hosts_order_and_short_circuit() {
        rune::orderly_exit();
        let res = rune::try_run_windows_with_args(RunOptions { accessory: true }, std::iter::empty::<String>(), |_| {
            let h1 = App::new(AppSpec::new("h1", (120.0, 80.0), || (), |_, _| el()))
                .windowless()
                .on_quit_request(|_, _| {
                    ORDER.lock().unwrap().push("h1");
                    if HOST1_DEFER.load(Ordering::SeqCst) { QuitResponse::Defer } else { QuitResponse::Quit }
                })
                .on_quit(|_, _| {
                    let n = QUIT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                    eprintln!("QUIT_TOTAL={n}");
                });
            let h1_handle = h1.handle();
            let h2 = App::new(AppSpec::new("h2", (120.0, 80.0), || (), |_, _| el()))
                .windowless()
                .on_quit_request(|_, _| {
                    ORDER.lock().unwrap().push("h2");
                    QuitResponse::Defer
                })
                .on_quit(|_, _| {
                    let n = QUIT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                    eprintln!("QUIT_TOTAL={n}");
                });
            let drive = h1_handle.clone();
            after_resumed(&h1_handle, move || {
                let h1_handle = drive;
                let mtm = MainThreadMarker::new().expect("main thread");
                let ns = check_hidden_app(mtm);
                // First terminate: h1 allows, h2 defers -> both run in order.
                trigger_real_terminate(&ns);
                eprintln!("AFTER_T1={}", ORDER.lock().unwrap().join(","));

                // Second terminate: h1 defers -> short-circuits before h2.
                HOST1_DEFER.store(true, Ordering::SeqCst);
                let h = h1_handle.clone();
                on_default_mode(move || {
                    let mtm = MainThreadMarker::new().expect("main thread");
                    let ns = NSApplication::sharedApplication(mtm);
                    trigger_real_terminate(&ns);
                    eprintln!("AFTER_T2={}", ORDER.lock().unwrap().join(","));
                    h.quit();
                });
            });
            vec![WindowSpec::from_app("h1", h1), WindowSpec::from_app("h2", h2)]
        });
        assert!(res.is_ok(), "run_windows_with failed: {res:?}");
        assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 2);
    }
}
