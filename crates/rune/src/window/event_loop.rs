//! Building and running the event loop every native window shares.
#[cfg(not(target_arch = "wasm32"))]
use std::rc::Rc;
#[cfg(not(target_arch = "wasm32"))]
use winit::event_loop::{ControlFlow, EventLoop};

use crate::app::App;
use crate::tooling::Args;
use crate::windows::WindowSpec;
#[cfg(not(target_arch = "wasm32"))]
use super::{WakeEvent, Windows};
use super::Factory;

pub fn run<S: 'static>(app: App<S>, args: Args) -> Result<(), String> {
    let accessory = app.accessory;
    run_configured(Box::new(move |_| vec![WindowSpec::from_app("main", app).into()]), args, accessory)
}

pub(crate) fn run_windows(factory: Factory, args: Args, options: crate::RunOptions) -> Result<(), String> {
    run_configured(factory, args, options.accessory)
}

/// Without a display (no `WAYLAND_DISPLAY` or `DISPLAY`, a refused X11 connection) the event
/// loop can't be built.
fn event_loop_error(error: winit::error::EventLoopError) -> String {
    format!("event loop: {error}")
}

/// Formats an error returned by `event_loop.run_app`, preferring a captured Wayland display
/// failure over winit's opaque `ExitFailure(code)`.
#[cfg(any(test, not(target_arch = "wasm32")))]
fn format_run_error(error: winit::error::EventLoopError, display_error: Option<&str>) -> String {
    match (error, display_error) {
        (winit::error::EventLoopError::ExitFailure(_), Some(cause)) => {
            format!("run: the display connection failed: {cause}")
        }
        (error, _) => format!("run: {error}"),
    }
}

/// Resolves `try_run`'s result once `event_loop.run_app` returns: re-raises a panic caught at
/// the macOS callback boundary first, then surfaces a loop error or a startup window creation
/// error when no initial window could be created.
#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) fn finish_run(
    result: Result<(), winit::error::EventLoopError>,
    display_error: Option<&str>,
    startup_error: Option<String>,
    panic: Option<super::panic_trap::PanicPayload>,
) -> Result<(), String> {
    if let Some(panic) = panic {
        std::panic::resume_unwind(panic);
    }
    match (result, startup_error) {
        (Err(error), _) => Err(format_run_error(error, display_error)),
        (Ok(()), Some(error)) => Err(error),
        (Ok(()), None) => Ok(()),
    }
}

/// How `run_configured` finishes after `shutdown_prewarm()`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExitAction {
    /// All warm-up workers exited cleanly (or a caught callback panic must unwind): drop
    /// windows and return from `run_configured` normally.
    Return,
    /// A warm-up worker stayed blocked inside a driver call past the shutdown bound: end the
    /// process without running window or libc exit destructors underneath it.
    EndProcess { code: i32, message: String },
}

/// Pure exit decision combining warm-up shutdown outcome, run result, and pixel-check status.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn exit_action(
    warm: crate::render::ShutdownOutcome,
    has_panic: bool,
    run_error: Option<&str>,
    pixel_check_failed: bool,
) -> ExitAction {
    if has_panic {
        return ExitAction::Return;
    }
    match warm {
        crate::render::ShutdownOutcome::Drained => ExitAction::Return,
        crate::render::ShutdownOutcome::TimedOut { remaining } => {
            let secs = crate::render::warm_machine::SHUTDOWN_TIMEOUT.as_secs();
            let note = format!(
                "rune: {remaining} GPU warm-up thread(s) did not finish within {secs} s before exit; ending process without destructors"
            );
            match run_error {
                Some(error) => ExitAction::EndProcess {
                    code: 1,
                    message: format!("rune: {error}\n{note}"),
                },
                None => ExitAction::EndProcess {
                    code: if pixel_check_failed { 3 } else { 0 },
                    message: note,
                },
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn run_configured(_: Factory, _: Args, _: bool) -> Result<(), String> {
    Err("native window factories are unsupported on web; use App::run for one canvas".into())
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
fn run_configured(factory: Factory, args: Args, accessory: bool) -> Result<(), String> {
    let _ = accessory;
    // A relaunch goes on once the app it restarts has exited.
    crate::render::reopen_prewarm();
    crate::relaunch::wait_for_previous();
    crate::tooling::perf_warn::launch::run_starts();
    crate::platform::ready_signal::run_starts();
    // `RUNE_OPEN_INACTIVE` (a tool's launch): the app doesn't take activation at launch either.
    #[cfg(target_os = "macos")]
    let inactive = accessory || std::env::var_os("RUNE_OPEN_INACTIVE").is_some();
    // AppKit's saved window state would hold the first window off the screen until the app has
    // launched.
    #[cfg(target_os = "macos")]
    crate::platform::macos::launch::ignore_saved_windows();
    // Beside AppKit's start: whether there is a window server session to launch in (read below,
    // before the loop runs), and what AppKit would otherwise wait for on this thread.
    #[cfg(target_os = "macos")]
    let beside = crate::platform::macos::launch::begin(!inactive).inspect_err(|_| crate::render::discard_prewarm())?;
    // While AppKit starts up.
    crate::render::prewarm_gpu();
    #[allow(unused_mut)]
    let mut builder = EventLoop::<WakeEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    if inactive {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        if accessory {
            builder.with_activation_policy(ActivationPolicy::Accessory).with_default_menu(false);
        }
        builder.with_activate_ignoring_other_apps(false);
    }
    // The first window is made and drawn as AppKit starts to launch, not after it has finished:
    // on macOS 27 AppKit's own launch work takes 30,000 to 55,000 µs after
    // `applicationWillFinishLaunching:`. `RUNE_WINDOW_AFTER_LAUNCH` keeps the old order.
    #[cfg(target_os = "macos")]
    if !accessory && std::env::var_os("RUNE_WINDOW_AFTER_LAUNCH").is_none() {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_init_before_finish_launching(true);
    }
    // A regular app's menu bar is made once its first window's first frame is on its way, in
    // the same turn of the launch (`native_menu::bar_ready`): winit's default menu, which it
    // installs before the first window, with it. `RUNE_MENU_BEFORE_WINDOW` keeps the old order.
    #[cfg(target_os = "macos")]
    if !accessory && std::env::var_os("RUNE_MENU_BEFORE_WINDOW").is_none() {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_default_menu(false);
        crate::native_menu::bar_waits();
    }
    // The desktop's light or dark preference, asked for on its own thread from here: the first
    // window waits for the answer, and by then has it (`portal_appearance::begin`).
    #[cfg(all(target_os = "linux", feature = "dbus"))]
    crate::platform::portal_appearance::begin();
    let event_loop = builder.build().inspect_err(|_| { let _ = crate::render::shutdown_prewarm(); }).map_err(event_loop_error)?;
    #[cfg(target_os = "macos")]
    if !beside.session() {
        let _ = crate::render::shutdown_prewarm();
        return Err("event loop: no window server session (no one is logged in to this Mac)".into());
    }
    crate::marks::mark("loop.built");
    // While the app sets up, after winit's first window: the first CPU frame's rounding needs it.
    // Beside `EventLoop::new` it waited on the window manager's lock that window takes, and made
    // it 1,134 µs slower.
    #[cfg(target_os = "windows")]
    crate::gpu::pick::find_display_vendor_soon();
    // winit's table for the keyboard layout, which the first key or mouse message would build on
    // this thread (8-10 ms, every virtual key through `ToUnicodeEx`): built beside the launch.
    #[cfg(target_os = "windows")]
    {
        // SAFETY: no arguments; this thread runs the loop.
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        std::thread::Builder::new().name("rune-keyboard-layout".into()).spawn(move || winit::platform::windows::warm_keyboard_layout(thread)).ok();
    }
    event_loop.set_control_flow(ControlFlow::Wait);
    #[cfg(target_os = "macos")]
    crate::platform::app_icon::launched();
    let proxy = event_loop.create_proxy();
    let turn = proxy.clone();
    crate::popup_menu::start(std::sync::Arc::new(move || {
        // Coalesce pending turns while a modal loop's timer fires so at most one `WakeEvent::Turn`
        // waits in the queue at a time.
        if !TURN_QUEUED.swap(true, std::sync::atomic::Ordering::AcqRel)
            && turn.send_event(WakeEvent::Turn).is_err()
        {
            TURN_QUEUED.store(false, std::sync::atomic::Ordering::Release);
        }
    }));
    let mut windows = Windows {
        factory: Some(factory), args, proxy, hosts: Vec::new(), gpu: Rc::default(),
        any_shown: false, startup_error: None,
        #[cfg(target_os = "macos")]
        panic: super::panic_trap::PanicTrap::default(),
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        display_error: None,
        wayland_wake: super::wayland_wake::State::default(),
        on_display_close: Vec::new(),
    };
    crate::open_url::incoming::prepare();
    crate::marks::mark("loop.run");
    let result = event_loop.run_app(&mut windows);
    crate::marks::mark("loop.exited");
    // The windows' interaction timing queued its last rollups as they closed.
    crate::tooling::interaction::flush();
    let warm_shutdown = crate::render::shutdown_prewarm();
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    crate::platform::wayland_drop::shutdown();
    crate::clipboard::shutdown(); // Also cover an event-loop error before `exiting`.
    crate::platform::ready_signal::clear();
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let display_error = windows.display_error.take();
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    let display_error: Option<String> = None;
    let startup_error = windows.startup_error.take();
    #[cfg(target_os = "macos")]
    let panic = windows.panic.take();
    #[cfg(not(target_os = "macos"))]
    let panic = None;
    let pixel_check_failed = crate::render::loop_pixel_check_failed();
    let run_error = match (&result, startup_error.as_deref()) {
        (Err(winit::error::EventLoopError::ExitFailure(_)), _) if display_error.is_some() => {
            Some(format!("run: the display connection failed: {}", display_error.as_deref().unwrap_or("")))
        }
        (Err(error), _) => Some(format!("run: {error}")),
        (Ok(()), Some(error)) => Some(error.to_owned()),
        (Ok(()), None) => None,
    };
    match exit_action(warm_shutdown, panic.is_some(), run_error.as_deref(), pixel_check_failed) {
        ExitAction::Return => {
            // An orderly exit: the windows and their renderers go here. `RUNE_LOOP_CHECK=2` found a
            // differing picture: the process ends with 3, as a fast exit does (`quit::end_process`).
            drop(windows);
            if panic.is_none() && pixel_check_failed { std::process::exit(3); }
            finish_run(result, display_error.as_deref(), startup_error, panic)
        }
        ExitAction::EndProcess { code, message } => {
            eprintln!("{message}");
            std::mem::forget(windows);
            crate::quit::end_process_with_code(code)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
static TURN_QUEUED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(not(target_arch = "wasm32"))]
impl Windows {
    /// The idle turn winit runs between its message loops, run while a native menu's modal
    /// loop holds winit's (`WakeEvent::Turn`): hosts whose timers are due resume, then
    /// `about_to_wait` draws overlays, answers assistive clients and sets the next wake.
    pub(super) fn turn(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        use std::cell::Cell;
        use winit::application::ApplicationHandler;
        thread_local! {
            static RUNNING: Cell<bool> = const { Cell::new(false) };
        }
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) { RUNNING.with(|r| r.set(false)); }
        }
        TURN_QUEUED.store(false, std::sync::atomic::Ordering::Release);
        if RUNNING.with(|r| r.replace(true)) {
            return;
        }
        let _reset = Reset;
        let now = crate::time::now();
        for host in self.hosts.iter_mut().flatten() {
            if let Some(due) = host.wake_deadline().filter(|&due| due <= now) {
                host.new_events(event_loop, winit::event::StartCause::ResumeTimeReached { start: due, requested_resume: due });
            }
        }
        self.about_to_wait(event_loop);
        self.cleanup(event_loop);
    }
}

#[test]
fn an_event_loop_that_cant_start_is_an_error_naming_the_event_loop() {
    let error = event_loop_error(winit::error::EventLoopError::RecreationAttempt);
    assert!(error.starts_with("event loop: ") && error.contains("recreated"), "{error}");
}

#[test]
fn run_exit_failure_reports_captured_display_error() {
    let proto = format_run_error(
        winit::error::EventLoopError::ExitFailure(71),
        Some("Protocol error 2 on object wl_surface@7"),
    );
    assert_eq!(proto, "run: the display connection failed: Protocol error 2 on object wl_surface@7");

    let io = format_run_error(
        winit::error::EventLoopError::ExitFailure(32),
        Some("Broken pipe (os error 32)"),
    );
    assert_eq!(io, "run: the display connection failed: Broken pipe (os error 32)");

    // Without a captured display error, ExitFailure keeps winit's message.
    let opaque = format_run_error(winit::error::EventLoopError::ExitFailure(1), None);
    assert_eq!(opaque, "run: Exit Failure: 1");

    // Non-ExitFailure errors are never replaced by a display error string.
    let other = format_run_error(
        winit::error::EventLoopError::RecreationAttempt,
        Some("Broken pipe (os error 32)"),
    );
    assert!(other.starts_with("run: ") && other.contains("recreated"), "{other}");
}

#[test]
fn finish_run_returns_startup_window_error_when_the_loop_exits_cleanly() {
    assert_eq!(finish_run(Ok(()), None, None, None), Ok(()));
    assert_eq!(
        finish_run(Ok(()), None, Some("could not create the window main: no GPU adapter".into()), None),
        Err("could not create the window main: no GPU adapter".into()),
    );
    assert_eq!(
        finish_run(
            Err(winit::error::EventLoopError::ExitFailure(71)),
            Some("Protocol error 2 on object wl_surface@7"),
            Some("could not create the window main: no GPU adapter".into()),
            None,
        ),
        Err("run: the display connection failed: Protocol error 2 on object wl_surface@7".into()),
    );
}

#[test]
fn finish_run_resumes_a_captured_panic_ahead_of_exit_status_and_startup_errors() {
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finish_run(
            Err(winit::error::EventLoopError::ExitFailure(71)),
            Some("Protocol error 2"),
            Some("startup error".into()),
            Some(Box::new("original callback panic")),
        )
    })).expect_err("captured panic must unwind out of finish_run");
    assert_eq!(err.downcast_ref::<&str>(), Some(&"original callback panic"));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn exit_action_returns_when_drained_and_ends_process_without_destructors_on_timeout() {
    use crate::render::ShutdownOutcome;

    assert_eq!(exit_action(ShutdownOutcome::Drained, false, None, false), ExitAction::Return);
    assert_eq!(exit_action(ShutdownOutcome::Drained, false, Some("err"), false), ExitAction::Return);
    assert_eq!(exit_action(ShutdownOutcome::TimedOut { remaining: 1 }, true, None, false), ExitAction::Return);

    let secs = crate::render::warm_machine::SHUTDOWN_TIMEOUT.as_secs();
    assert_eq!(
        exit_action(ShutdownOutcome::TimedOut { remaining: 1 }, false, None, false),
        ExitAction::EndProcess {
            code: 0,
            message: format!("rune: 1 GPU warm-up thread(s) did not finish within {secs} s before exit; ending process without destructors"),
        },
    );
    assert_eq!(
        exit_action(ShutdownOutcome::TimedOut { remaining: 2 }, false, None, true),
        ExitAction::EndProcess {
            code: 3,
            message: format!("rune: 2 GPU warm-up thread(s) did not finish within {secs} s before exit; ending process without destructors"),
        },
    );
    assert_eq!(
        exit_action(ShutdownOutcome::TimedOut { remaining: 1 }, false, Some("run: display failed"), false),
        ExitAction::EndProcess {
            code: 1,
            message: format!("rune: run: display failed\nrune: 1 GPU warm-up thread(s) did not finish within {secs} s before exit; ending process without destructors"),
        },
    );
}
