//! `open()` activates an accessory app and makes its window key while another app is
//! frontmost: the first open of a window that doesn't exist yet, a reopen after hiding, and a
//! hidden `App::window` window. A launcher panel's open and an overlay's show stay
//! nonactivating. Starts a helper app to hold the focus (handed to it with CuaDriver when
//! nothing yields), so it only runs with RUNE_GUI_TESTS=1 in a logged-in macOS GUI session.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use objc2::{class, msg_send, runtime::{AnyObject, Bool}};
    use objc2_foundation::NSString;
    use rune::*;

    const HELPER: &str = "RUNE_OPEN_ACTIVATES_HELPER";
    const CUA_DRIVER: &str = "/Applications/CuaDriver.app/Contents/MacOS/cua-driver";
    const MAIN_TITLE: &str = "open activates main";
    const SETTINGS_TITLE: &str = "open activates settings";

    fn view<S>(_: &S, _: &mut Cx<S>) -> El { text("open activates") }

    fn on_main<T: Send + 'static>(handle: &AppHandle<()>, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        handle.update(move |_, _| { let _ = tx.send(f()); });
        rx.recv_timeout(Duration::from_secs(10)).expect("event thread stalled")
    }

    fn wait(what: &str, f: impl Fn() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn app_active() -> bool {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let active: Bool = msg_send![app, isActive];
            active.as_bool()
        }
    }

    fn frontmost_pid() -> i32 {
        unsafe {
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
            if app.is_null() { return 0 }
            msg_send![app, processIdentifier]
        }
    }

    /// The key window's title, if this app has a key window.
    fn key_title() -> Option<String> {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let window: *mut AnyObject = msg_send![app, keyWindow];
            if window.is_null() { return None }
            let title: *mut NSString = msg_send![window, title];
            (!title.is_null()).then(|| (*title).to_string())
        }
    }

    /// Hands the focus to the helper. macOS only lets the active app pass activation on, so
    /// when nobody yields (a fresh launch from a shell, or a stale frontmost process) this falls
    /// back to CuaDriver's `bring_to_front`, which activates through accessibility.
    fn activate_helper(handle: &AppHandle<()>, pid: i32) {
        wait("the helper to launch", || on_main(handle, move || unsafe {
            let app: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
            !app.is_null() && { let done: Bool = msg_send![app, isFinishedLaunching]; done.as_bool() }
        }));
        on_main(handle, move || unsafe {
            let app: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
            // NSApplicationActivateIgnoringOtherApps
            let _: Bool = msg_send![app, activateWithOptions: 1usize << 1];
        });
        let helper_front = || on_main(handle, move || frontmost_pid() == pid);
        let start = Instant::now();
        while !helper_front() && start.elapsed() < Duration::from_secs(1) { std::thread::sleep(Duration::from_millis(20)); }
        if !helper_front() {
            let status = Command::new(CUA_DRIVER).arg("bring_to_front").arg(format!(r#"{{"pid":{pid}}}"#))
                .stdout(std::process::Stdio::null()).status();
            assert!(status.is_ok_and(|s| s.success()), "couldn't hand the focus to the helper (needs {CUA_DRIVER})");
        }
        wait("the helper to be frontmost", helper_front);
    }

    /// The helper keeps the front. A key nonactivating panel can still make `isActive` report
    /// true, so that isn't checked.
    fn expect_inactive(handle: &AppHandle<()>, pid: i32, what: &str) {
        std::thread::sleep(Duration::from_millis(400));
        let front = on_main(handle, frontmost_pid);
        assert_eq!(front, pid, "{what} took the front from the helper");
    }

    fn expect_active(handle: &AppHandle<()>, title: &'static str, what: &str) {
        let me = std::process::id() as i32;
        let start = Instant::now();
        loop {
            let (active, front, key) = on_main(handle, || (app_active(), frontmost_pid(), key_title()));
            if active && front == me && key.as_deref() == Some(title) { return }
            assert!(start.elapsed() < Duration::from_secs(5),
                "{what}: active={active}, frontmost={front} (this app {me}), key window {key:?}, want {title:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn helper() {
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); std::process::exit(0) });
        App::new(AppSpec::new("open activates helper", (240.0, 80.0), || (), view)).args(Vec::new()).run();
    }

    pub fn main() {
        if std::env::var_os(HELPER).is_some() { return helper() }
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("open_activates: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(90)); eprintln!("timeout"); std::process::exit(2) });
        let mut child = Command::new(std::env::current_exe().unwrap()).env(HELPER, "1").spawn().expect("helper");
        let pid = child.id() as i32;
        let mut app = App::new(AppSpec::new(MAIN_TITLE, (240.0, 120.0), || (), view)).args(Vec::new())
            .accessory().windowless();
        let main = app.handle();
        let (_, settings) = app.window(WindowSpec::new("settings", AppSpec::new(SETTINGS_TITLE, (240.0, 120.0), || (), view))
            .options(WindowOptions { visible: false, ..Default::default() }));
        let (_, launcher) = app.window(WindowSpec::new("launcher", AppSpec::new("open activates launcher", (240.0, 80.0), || (), view))
            .configured_options(WindowOptions::launcher_panel()));
        let overlay = app.overlay(OverlaySpec::new("open activates overlay", (200.0, 60.0), view), ());
        let result = std::sync::Arc::new(Mutex::new(None));
        let out = result.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                std::thread::sleep(Duration::from_millis(500));
                assert!(!on_main(&main, app_active), "an accessory app doesn't activate at launch");
                activate_helper(&main, pid);
                assert!(!main.is_visible(), "windowless starts without a window");

                overlay.show();
                expect_inactive(&main, pid, "overlay show");
                overlay.hide();
                launcher.open();
                expect_inactive(&main, pid, "launcher open");
                launcher.hide();
                std::thread::sleep(Duration::from_millis(200));
                activate_helper(&main, pid);

                main.open();
                expect_active(&main, MAIN_TITLE, "the first open of a new window");
                println!("first open: active and key");

                main.hide();
                wait("hide", || !main.is_visible());
                activate_helper(&main, pid);
                main.open();
                expect_active(&main, MAIN_TITLE, "a reopen after hiding");
                println!("reopen: active and key");

                activate_helper(&main, pid);
                settings.open();
                expect_active(&main, SETTINGS_TITLE, "opening a hidden App::window window");
                println!("App::window open: active and key");
            });
            *out.lock().unwrap() = Some(run.is_ok());
            main.quit();
        });
        app.run();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(*result.lock().unwrap(), Some(true), "open_activates failed");
        println!("open_activates: ok");
    }
}
