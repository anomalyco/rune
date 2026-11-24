use std::cell::RefCell;
use std::rc::Rc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{ExitFlags, ExitStep, cleanup_hosts, exiting_hosts, is_error_exit, quit_hosts, run_display_close};
use crate::clipboard::MemoryClipboard;
use crate::quit::{Effect, Event, State as QuitState, step as quit_step};
use crate::runtime::{Env, Runtime};
use crate::window::host::any_host::quit_hook_core;
use crate::window::host::app::{WindowApp, test_app};
use crate::window::{AnyHost, Life, WakeEvent};
use crate::{Cx, El, Tx, el};

struct WindowLog {
    events: Rc<RefCell<Vec<String>>>,
    name: &'static str,
    copy_on_quit: Option<&'static str>,
}

fn view(_: &WindowLog, _: &mut Cx<WindowLog>) -> El { el() }

fn on_close(s: &mut WindowLog, _: &mut Tx) {
    s.events.borrow_mut().push(format!("{}:on_close", s.name));
}

fn on_quit(s: &mut WindowLog, tx: &mut Tx) {
    s.events.borrow_mut().push(format!("{}:on_quit", s.name));
    if let Some(text) = s.copy_on_quit {
        tx.copy(text);
    }
}

/// Host test double using real `TypedWindowApp`, `Life`, `clipboard::Client`, and
/// `quit_hook_core` so both `cleanup_hosts` and `Host::quit_hook_in_turn` are exercised without
/// creating a native window.
struct TestHost {
    app: Box<dyn WindowApp>,
    clipboard: crate::clipboard::Client,
    life: Life,
    keeps_loop: bool,
    keep_running: bool,
    events: Rc<RefCell<Vec<String>>>,
    name: &'static str,
    redraw_asked: Rc<RefCell<u32>>,
    menus_cancelled: Rc<RefCell<u32>>,
    log_drop: bool,
}

impl Drop for TestHost {
    fn drop(&mut self) {
        if self.log_drop {
            self.events.borrow_mut().push(format!("{}:drop", self.name));
        }
    }
}

impl TestHost {
    fn new(
        name: &'static str,
        keeps_loop: bool,
        events: Rc<RefCell<Vec<String>>>,
        clipboard_mem: &MemoryClipboard,
        copy_on_quit: Option<&'static str>,
    ) -> Self {
        let mut rt = Runtime::new(
            WindowLog { events: events.clone(), name, copy_on_quit },
            view,
            (100.0, 100.0),
            Env::default(),
        );
        rt.frame(0.0);
        let app = test_app(rt, Some(on_close), Some(on_quit));
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.use_memory(clipboard_mem);
        let mut life = Life::Unopened;
        life.show();
        Self {
            app,
            clipboard,
            life,
            keeps_loop,
            keep_running: false,
            events,
            name,
            redraw_asked: Rc::new(RefCell::new(0)),
            menus_cancelled: Rc::new(RefCell::new(0)),
            log_drop: false,
        }
    }

    fn with_drop_log(mut self) -> Self {
        self.log_drop = true;
        self
    }

    /// Simulates `Host::close` on a window without `keep_running`: `on_close`, then `Life::Finished`.
    fn close(&mut self) {
        if !self.life.closes(self.keep_running, true) { return; }
        self.app.run_on_close();
        self.clipboard.flush_core(self.app.rt_mut());
        self.life.finish();
    }
}

impl AnyHost for TestHost {
    fn resumed(&mut self, _: &ActiveEventLoop) {}
    fn user_event(&mut self, _: &ActiveEventLoop, _: WakeEvent) {}
    fn owns(&self, _: WindowId) -> bool { false }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    fn finished(&self) -> bool { self.life.finished() }
    fn keeps_loop(&self) -> bool { self.keeps_loop }

    fn vanish(&mut self) {
        if self.life.shown() {
            self.events.borrow_mut().push(format!("{}:vanish", self.name));
        }
    }

    fn quit_hook(&mut self) {
        if quit_hook_core(&mut *self.app, &mut self.clipboard, self.life) {
            *self.redraw_asked.borrow_mut() += 1;
        }
    }

    fn terminate(&mut self) {
        if !self.life.closes(self.keep_running, true) { return; }
        self.life.finish();
        self.app.run_on_close();
    }

    fn cancel_menus(&mut self) {
        *self.menus_cancelled.borrow_mut() += 1;
    }
}

fn step_quit(state: &mut QuitState) -> impl FnMut(Event) -> Vec<Effect> + '_ {
    move |event| {
        let (next, effects) = quit_step(*state, event);
        *state = next;
        effects
    }
}

#[test]
fn closing_the_only_window_runs_on_close_then_on_quit_once_for_orderly_and_fast_exit() {
    for fast_exit in [false, true] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let mut host = TestHost::new("main", true, events.clone(), &mem, Some("saved-on-quit"));
        let redraw_asked = host.redraw_asked.clone();
        host.close();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut quit_state = QuitState::Running;

        let should_exit = cleanup_hosts(&mut hosts, &mut Vec::new(), true, false, fast_exit, step_quit(&mut quit_state));
        assert!(should_exit, "closing the last main window exits the loop (fast_exit={fast_exit})");
        assert_eq!(
            *events.borrow(),
            vec!["main:on_close", "main:on_quit"],
            "fast_exit={fast_exit}",
        );
        assert_eq!(*redraw_asked.borrow(), 0, "a finished window does not request a redraw");
        assert_eq!(mem.text().as_deref().map(String::as_str), Some("saved-on-quit"), "clipboard requests in on_quit flush");
        assert_eq!(hosts[0].is_none(), !fast_exit, "fast exit keeps the finished host undropped");

        // Windows::exiting calls quit_hosts again; hooks already ran, so on_quit must not repeat.
        quit_hosts(&mut hosts, false, true, true, step_quit(&mut quit_state));
        assert_eq!(
            *events.borrow(),
            vec!["main:on_close", "main:on_quit"],
            "exiting does not repeat on_quit (fast_exit={fast_exit})",
        );
    }
}

#[test]
fn closing_an_earlier_window_does_not_run_its_on_quit_when_the_last_window_closes() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mem = crate::clipboard::memory();
    let mut first = TestHost::new("first", true, events.clone(), &mem, None);
    let second = TestHost::new("second", true, events.clone(), &mem, None);

    // Turn 1: first window closes while second stays open.
    first.close();
    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(first)), Some(Box::new(second))];
    let mut quit_state = QuitState::Running;
    assert!(!cleanup_hosts(&mut hosts, &mut Vec::new(), true, false, true, step_quit(&mut quit_state)));
    assert!(hosts[0].is_none(), "a closed window drops while another main window keeps running");
    assert_eq!(*events.borrow(), vec!["first:on_close"]);

    // Turn 2: second (last) window closes.
    hosts[1].as_mut().unwrap().terminate();
    assert!(cleanup_hosts(&mut hosts, &mut Vec::new(), true, false, true, step_quit(&mut quit_state)));
    assert_eq!(*events.borrow(), vec!["first:on_close", "second:on_close", "second:on_quit"]);
}

#[test]
fn closing_the_last_main_window_runs_on_quit_in_it_and_in_open_secondary_windows() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mem = crate::clipboard::memory();
    let mut main = TestHost::new("main", true, events.clone(), &mem, None);
    let secondary = TestHost::new("secondary", false, events.clone(), &mem, None);
    let secondary_redraws = secondary.redraw_asked.clone();

    main.close();
    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(main)), Some(Box::new(secondary))];
    let mut quit_state = QuitState::Running;

    assert!(cleanup_hosts(&mut hosts, &mut Vec::new(), true, false, false, step_quit(&mut quit_state)));
    assert_eq!(
        *events.borrow(),
        vec![
            "main:on_close",
            "secondary:vanish",
            "main:on_quit",
            "secondary:on_quit",
            "secondary:on_close",
        ],
    );
    assert_eq!(*secondary_redraws.borrow(), 1, "shown secondary window still reports redraw eligibility");
    assert!(hosts.iter().all(Option::is_none), "orderly exit drops all finished hosts afterwards");
}

#[test]
fn lost_display_and_unrequested_loop_exit_bypass_fast_exit_like_startup_error() {
    // Normal quits stay fast: app-requested quit (loop_exiting && quitting), or OS-initiated
    // quit on macOS (!loop_exiting, whether or not the delegate hook set quitting).
    assert!(!is_error_exit(false, false, true, true), "normal Rune quit is not an error exit");
    assert!(!is_error_exit(false, false, false, true), "macOS Cmd-Q is not an error exit");
    assert!(!is_error_exit(false, false, false, false), "macOS OS quit without delegate hook is not an error exit");

    // Error exits: startup failure, captured Wayland display error, or unrequested loop exit
    // (such as X11/Wayland ExitFailure where Rune never started a quit).
    assert!(is_error_exit(true, false, true, false), "startup window failure is an error exit");
    assert!(is_error_exit(false, true, true, false), "lost Wayland display is an error exit");
    assert!(is_error_exit(false, true, true, true), "display error racing a quit still bypasses fast exit");
    assert!(is_error_exit(false, false, true, false), "unrequested loop exit (X11/Wayland ExitFailure) is an error exit");

    // Wayland display error under default fast_exit: skips on_quit, terminate, fast_exit and
    // relaunch so event_loop.run_app returns Err to finish_run (exit 1 in run / Err in try_run).
    for (label, has_startup_error, has_display_error, quitting) in [
        ("startup_error", true, false, false),
        ("wayland_display_error", false, true, false),
        ("x11_or_opaque_exit_failure", false, false, false),
    ] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let host = TestHost::new("main", true, events.clone(), &mem, None);
        let menus_cancelled = host.menus_cancelled.clone();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut quit_state = QuitState::Running;

        let step = exiting_hosts(
            &mut hosts,
            &mut Vec::new(),
            ExitFlags { has_startup_error, has_display_error, loop_exiting: true, quitting, fast_exit: true },
            step_quit(&mut quit_state),
        );
        assert_eq!(step, ExitStep::Orderly { relaunch: false }, "{label} must bypass fast exit");
        assert!(events.borrow().is_empty(), "{label} must not vanish or run quit/close hooks: {:?}", *events.borrow());
        assert_eq!(*menus_cancelled.borrow(), 1, "{label} still cancels tracking menus");
        assert_eq!(quit_state, QuitState::Running, "{label} leaves quit state untouched");
    }

    // Normal quit under default fast_exit stays fast and runs on_quit.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let host = TestHost::new("main", true, events.clone(), &mem, None);
        let menus_cancelled = host.menus_cancelled.clone();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut quit_state = QuitState::Quitting(crate::quit::Hooks::Owed, crate::quit::After::Exit);

        let step = exiting_hosts(
            &mut hosts,
            &mut Vec::new(),
            ExitFlags { has_startup_error: false, has_display_error: false, loop_exiting: true, quitting: true, fast_exit: true },
            step_quit(&mut quit_state),
        );
        assert_eq!(step, ExitStep::FastExit, "normal quit keeps fast exit");
        assert_eq!(*events.borrow(), vec!["main:vanish", "main:on_quit"]);
        assert_eq!(*menus_cancelled.borrow(), 1);
    }

    // Normal orderly quit with relaunch still relaunches.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let host = TestHost::new("main", true, events.clone(), &mem, None);
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut quit_state = QuitState::Quitting(crate::quit::Hooks::Owed, crate::quit::After::Relaunch);

        let step = exiting_hosts(
            &mut hosts,
            &mut Vec::new(),
            ExitFlags { has_startup_error: false, has_display_error: false, loop_exiting: true, quitting: true, fast_exit: false },
            step_quit(&mut quit_state),
        );
        assert_eq!(step, ExitStep::Orderly { relaunch: true });
        assert_eq!(*events.borrow(), vec!["main:vanish", "main:on_quit"]);
    }
}

fn log_display_close(events: &Rc<RefCell<Vec<String>>>, label: &'static str) -> crate::app::OnDisplayClose {
    let events = events.clone();
    Box::new(move || events.borrow_mut().push(format!("{label}:on_display_close")))
}

#[test]
fn on_display_close_runs_once_after_quit_and_before_hosts_drop_when_last_window_closes() {
    for fast_exit in [false, true] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let mut host = TestHost::new("main", true, events.clone(), &mem, None).with_drop_log();
        host.close();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut display_close = vec![log_display_close(&events, "main")];
        let mut quit_state = QuitState::Running;

        assert!(cleanup_hosts(&mut hosts, &mut display_close, true, false, fast_exit, step_quit(&mut quit_state)));
        let expected = if fast_exit {
            vec!["main:on_close", "main:on_quit", "main:on_display_close"]
        } else {
            vec!["main:on_close", "main:on_quit", "main:on_display_close", "main:drop"]
        };
        assert_eq!(*events.borrow(), expected, "fast_exit={fast_exit}");
        assert!(display_close.is_empty(), "hooks drain on first run");

        // Subsequent exiting_hosts must not invoke on_display_close a second time.
        let _ = exiting_hosts(
            &mut hosts,
            &mut display_close,
            ExitFlags { has_startup_error: false, has_display_error: false, loop_exiting: true, quitting: true, fast_exit },
            step_quit(&mut quit_state),
        );
        assert_eq!(*events.borrow(), expected, "exiting does not repeat on_display_close (fast_exit={fast_exit})");
    }
}

#[test]
fn on_display_close_waits_until_last_window_closes_and_runs_earlier_window_hooks_in_order() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mem = crate::clipboard::memory();
    let mut first = TestHost::new("first", true, events.clone(), &mem, None).with_drop_log();
    let second = TestHost::new("second", true, events.clone(), &mem, None).with_drop_log();

    first.close();
    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(first)), Some(Box::new(second))];
    let mut display_close = vec![
        log_display_close(&events, "first"),
        log_display_close(&events, "second"),
    ];
    let mut quit_state = QuitState::Running;

    // Closing the first window leaves the display open: no display-close hook runs yet.
    assert!(!cleanup_hosts(&mut hosts, &mut display_close, true, false, false, step_quit(&mut quit_state)));
    assert_eq!(*events.borrow(), vec!["first:on_close", "first:drop"]);
    assert_eq!(display_close.len(), 2);

    // Closing the second (last) window runs both hooks in registration order before dropping it.
    hosts[1].as_mut().unwrap().terminate();
    assert!(cleanup_hosts(&mut hosts, &mut display_close, true, false, false, step_quit(&mut quit_state)));
    assert_eq!(
        *events.borrow(),
        vec![
            "first:on_close",
            "first:drop",
            "second:on_close",
            "second:on_quit",
            "first:on_display_close",
            "second:on_display_close",
            "second:drop",
        ],
    );
    assert!(display_close.is_empty());
}

#[test]
fn on_display_close_runs_once_in_exiting_hosts_for_app_quit_os_quit_and_error_exits() {
    // App-requested quit (orderly and fast exit): on_quit runs before on_display_close,
    // and hosts stay alive until exiting_hosts returns.
    for fast_exit in [false, true] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let host = TestHost::new("main", true, events.clone(), &mem, None).with_drop_log();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut display_close = vec![log_display_close(&events, "main")];
        let mut quit_state = QuitState::Quitting(crate::quit::Hooks::Owed, crate::quit::After::Exit);

        let step = exiting_hosts(
            &mut hosts,
            &mut display_close,
            ExitFlags { has_startup_error: false, has_display_error: false, loop_exiting: true, quitting: true, fast_exit },
            step_quit(&mut quit_state),
        );
        assert_eq!(step, if fast_exit { ExitStep::FastExit } else { ExitStep::Orderly { relaunch: false } });
        assert_eq!(*events.borrow(), vec!["main:vanish", "main:on_quit", "main:on_display_close"]);
        assert!(hosts[0].is_some(), "hosts stay alive until after on_display_close");
        run_display_close(&mut display_close);
        assert_eq!(*events.borrow(), vec!["main:vanish", "main:on_quit", "main:on_display_close"]);
    }

    // OS-initiated quit (loop_exiting = false): on_quit, on_close, then on_display_close.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let host = TestHost::new("main", true, events.clone(), &mem, None).with_drop_log();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut display_close = vec![log_display_close(&events, "main")];
        let mut quit_state = QuitState::Running;

        let step = exiting_hosts(
            &mut hosts,
            &mut display_close,
            ExitFlags { has_startup_error: false, has_display_error: false, loop_exiting: false, quitting: false, fast_exit: false },
            step_quit(&mut quit_state),
        );
        assert_eq!(step, ExitStep::Orderly { relaunch: false });
        assert_eq!(*events.borrow(), vec!["main:vanish", "main:on_quit", "main:on_close", "main:on_display_close"]);
    }

    // Error exits skip on_quit/terminate/fast-exit but still run on_display_close before teardown.
    for (label, has_startup_error, has_display_error) in [
        ("startup_error", true, false),
        ("wayland_display_error", false, true),
        ("unrequested_loop_exit", false, false),
    ] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let host = TestHost::new("main", true, events.clone(), &mem, None).with_drop_log();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut display_close = vec![log_display_close(&events, "main")];
        let mut quit_state = QuitState::Running;

        let step = exiting_hosts(
            &mut hosts,
            &mut display_close,
            ExitFlags { has_startup_error, has_display_error, loop_exiting: true, quitting: false, fast_exit: true },
            step_quit(&mut quit_state),
        );
        assert_eq!(step, ExitStep::Orderly { relaunch: false }, "{label}");
        assert_eq!(*events.borrow(), vec!["main:on_display_close"], "{label}");
        assert!(hosts[0].is_some(), "{label}: host drops after on_display_close");
    }

    // When event_loop.exiting() is already true during cleanup_hosts, on_display_close waits
    // for exiting_hosts so on_quit runs first.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let mut host = TestHost::new("main", true, events.clone(), &mem, None);
        host.close();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
        let mut display_close = vec![log_display_close(&events, "main")];
        let mut quit_state = QuitState::Quitting(crate::quit::Hooks::Owed, crate::quit::After::Exit);

        assert!(cleanup_hosts(&mut hosts, &mut display_close, false, true, true, step_quit(&mut quit_state)));
        assert_eq!(*events.borrow(), vec!["main:on_close"]);
        assert_eq!(display_close.len(), 1, "deferred until exiting_hosts runs on_quit");
        let step = exiting_hosts(
            &mut hosts,
            &mut display_close,
            ExitFlags { has_startup_error: false, has_display_error: false, loop_exiting: true, quitting: true, fast_exit: true },
            step_quit(&mut quit_state),
        );
        assert_eq!(step, ExitStep::FastExit);
        assert_eq!(*events.borrow(), vec!["main:on_close", "main:on_quit", "main:on_display_close"]);
    }
}
