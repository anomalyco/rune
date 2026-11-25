use std::cell::RefCell;
use std::rc::Rc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{ExitFlags, ExitStep, cleanup_hosts, exiting_hosts, finish_orderly_exiting, is_error_exit, quit_hosts, run_display_close};
use crate::clipboard::MemoryClipboard;
use crate::quit::{Effect, Event, State as QuitState, step as quit_step};
use crate::runtime::{Env, Runtime};
use crate::window::host::any_host::quit_hook_core;
use crate::window::host::app::{WindowApp, test_app, test_app_with_close_request};
use crate::window::host::lifecycle::{CloseDecision, ShellCloseAction, close_request_core, take_shell_action};
use crate::window::state::FirstFrame;
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

struct CloseRequestState {
    events: Rc<RefCell<Vec<String>>>,
    dirty: bool,
    prompt_open: bool,
    defer_count: u32,
    copy_on_defer: Option<&'static str>,
    title_on_defer: Option<&'static str>,
}

fn close_request_view(s: &CloseRequestState, _: &mut Cx<CloseRequestState>) -> El {
    if s.prompt_open { el().key("save-prompt") } else { el().key("editor") }
}

fn on_close_request_guard(s: &mut CloseRequestState, tx: &mut Tx) -> crate::CloseResponse {
    s.events.borrow_mut().push("on_close_request".into());
    if s.dirty {
        s.prompt_open = true;
        s.defer_count += 1;
        if let Some(copy) = s.copy_on_defer {
            tx.copy(copy);
        }
        if let Some(title) = s.title_on_defer {
            tx.set_title(title);
        }
        crate::CloseResponse::Defer
    } else {
        crate::CloseResponse::Close
    }
}

fn on_close_guard(s: &mut CloseRequestState, _: &mut Tx) {
    s.events.borrow_mut().push("on_close".into());
}

fn on_quit_guard(s: &mut CloseRequestState, _: &mut Tx) {
    s.events.borrow_mut().push("on_quit".into());
}

/// Simulates `Host::close_requested` + `Host::close` using the real `close_request_core`,
/// `TypedWindowApp`, `Life`, `FirstFrame`, and `clipboard::Client`.
struct GuardedHost {
    app: Box<dyn WindowApp>,
    clipboard: crate::clipboard::Client,
    life: Life,
    first_frame: FirstFrame,
    keep_running: bool,
    ends_app: bool,
    redraws: u32,
    hides: u32,
}

impl GuardedHost {
    fn new(
        dirty: bool,
        with_hook: bool,
        keep_running: bool,
        events: Rc<RefCell<Vec<String>>>,
        mem: &MemoryClipboard,
    ) -> Self {
        let mut rt = Runtime::new(
            CloseRequestState {
                events: events.clone(),
                dirty,
                prompt_open: false,
                defer_count: 0,
                copy_on_defer: Some("unsaved-draft"),
                title_on_defer: Some("Save changes?"),
            },
            close_request_view,
            (200.0, 140.0),
            Env::default(),
        );
        rt.frame(0.0);
        let app = test_app_with_close_request(
            rt,
            with_hook.then_some(on_close_request_guard as fn(&mut CloseRequestState, &mut Tx) -> crate::CloseResponse),
            Some(on_close_guard),
            Some(on_quit_guard),
        );
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.use_memory(mem);
        let mut life = Life::Unopened;
        life.show();
        let mut first_frame = FirstFrame::drawn_notified();
        // Arm a pending reveal so `FirstFrame::hide` would visibly mutate it if called.
        let _ = first_frame.request_reveal(true, false, crate::time::now());
        Self {
            app,
            clipboard,
            life,
            first_frame,
            keep_running,
            ends_app: false,
            redraws: 0,
            hides: 0,
        }
    }

    fn close_requested(&mut self, exiting: bool) -> CloseDecision {
        let closes = self.life.closes(self.keep_running, true);
        let decision = close_request_core(
            &mut *self.app,
            &mut self.clipboard,
            closes,
            exiting,
            &mut self.ends_app,
        );
        match decision {
            CloseDecision::Close => self.close(),
            CloseDecision::Defer => {
                if self.life.shown() {
                    self.redraws += 1;
                }
            }
        }
        decision
    }

    /// Programmatic close (`Tx::close_window`, `AppHandle::close`, `WindowHandle::close`):
    /// bypasses `on_close_request`, mutates `first_frame` / hides, runs `on_close`, and finishes
    /// or hides according to `keep_running`.
    fn shell_step(&mut self) -> ShellCloseAction {
        let action = take_shell_action(self.app.rt_mut());
        match action {
            ShellCloseAction::ProgrammaticClose => self.close(),
            ShellCloseAction::UserCloseRequest => { let _ = self.close_requested(false); }
            ShellCloseAction::Quit(_) | ShellCloseAction::Minimize | ShellCloseAction::None => {}
        }
        action
    }

    fn close(&mut self) {
        self.first_frame.hide();
        if !self.life.closes(self.keep_running, true) { return; }
        let _ = std::mem::take(&mut self.ends_app);
        self.hides += 1;
        self.app.run_on_close();
        self.clipboard.flush_core(self.app.rt_mut());
        if self.keep_running {
            self.life.hide(crate::platform::reopen::stamp);
        } else {
            self.life.finish();
        }
    }
}

impl AnyHost for GuardedHost {
    fn resumed(&mut self, _: &ActiveEventLoop) {}
    fn user_event(&mut self, _: &ActiveEventLoop, _: WakeEvent) {}
    fn owns(&self, _: WindowId) -> bool { false }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    fn finished(&self) -> bool { self.life.finished() }
    fn keeps_loop(&self) -> bool { true }
    fn quit_hook(&mut self) {
        let _ = quit_hook_core(&mut *self.app, &mut self.clipboard, self.life);
    }
    fn terminate(&mut self) {
        if !self.life.closes(self.keep_running, true) { return; }
        self.life.finish();
        self.app.run_on_close();
        self.clipboard.flush_core(self.app.rt_mut());
    }
}

#[test]
fn close_request_defer_keeps_window_open_commits_tx_and_programmatic_close_bypasses() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mem = crate::clipboard::memory();
    let mut host = GuardedHost::new(true, true, false, events.clone(), &mem);
    host.ends_app = true;
    let first_frame_before = host.first_frame;
    assert!(first_frame_before.revealing());

    // Native WindowEvent::CloseRequested while dirty -> Defer.
    assert_eq!(host.close_requested(false), CloseDecision::Defer);
    assert!(host.life.shown(), "Defer must keep Life::Shown");
    assert!(!host.life.finished(), "Defer must not finish the host");
    assert_eq!(host.first_frame, first_frame_before, "Defer must not mutate FirstFrame");
    assert_eq!(host.hides, 0, "Defer must not hide the window");
    assert!(!host.ends_app, "Defer must clear ends_app so later closes don't reuse stale fast-exit state");
    assert_eq!(host.redraws, 1, "Defer schedules a redraw to paint the confirmation dialog");
    assert!(host.app.rt().needs_commit(), "state updates in on_close_request mark the view dirty");
    assert_eq!(host.app.rt_mut().host.window_title.take_change(), Some("Save changes?"));
    assert_eq!(mem.text().as_deref().map(String::as_str), Some("unsaved-draft"), "clipboard flushes on Defer");
    assert_eq!(*events.borrow(), vec!["on_close_request"], "on_close must not run on Defer");

    // Next frame commits the deferred dialog into the live tree.
    host.app.frame(0.0);
    assert!(host.app.rt().find("save-prompt").is_some());

    // cleanup_hosts keeps the event loop alive while the window stays open.
    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
    let mut display_close = vec![log_display_close(&events, "main")];
    let mut quit_state = QuitState::Running;
    assert!(!cleanup_hosts(&mut hosts, &mut display_close, true, false, false, step_quit(&mut quit_state)));
    assert_eq!(*events.borrow(), vec!["on_close_request"]);
    assert_eq!(display_close.len(), 1);

    // User cancels and presses Close again -> on_close_request runs a second time.
    let mut host = GuardedHost::new(true, true, false, events.clone(), &mem);
    assert_eq!(host.close_requested(false), CloseDecision::Defer);
    assert_eq!(host.close_requested(false), CloseDecision::Defer);
    assert_eq!(host.redraws, 2);

    // User confirms via programmatic close (Tx::close_window / AppHandle::close / WindowHandle::close):
    // bypasses on_close_request and completes through on_close once.
    events.borrow_mut().clear();
    host.close();
    assert!(host.life.finished());
    assert!(!host.first_frame.revealing(), "actual close hides FirstFrame");
    assert_eq!(*events.borrow(), vec!["on_close"], "programmatic close bypasses on_close_request");

    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
    let mut display_close = vec![log_display_close(&events, "main")];
    let mut quit_state = QuitState::Running;
    assert!(cleanup_hosts(&mut hosts, &mut display_close, true, false, false, step_quit(&mut quit_state)));
    assert_eq!(*events.borrow(), vec!["on_close", "on_quit", "main:on_display_close"]);
}

#[test]
fn close_request_close_and_default_no_hook_proceed_through_on_close() {
    // 1. Registered hook returning CloseResponse::Close proceeds to on_close and preserves ends_app.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let mut host = GuardedHost::new(false, true, false, events.clone(), &mem);
        host.ends_app = true;
        let decision = close_request_core(&mut *host.app, &mut host.clipboard, true, false, &mut host.ends_app);
        assert_eq!(decision, CloseDecision::Close);
        assert!(host.ends_app, "CloseResponse::Close preserves ends_app for Host::close");
        host.close();
        assert!(host.life.finished());
        assert_eq!(*events.borrow(), vec!["on_close_request", "on_close"]);
    }

    // 2. Default (no hook registered) performs no update transaction and closes as before.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let mut host = GuardedHost::new(true, false, false, events.clone(), &mem);
        host.ends_app = true;
        assert!(!host.app.rt().needs_commit());
        let decision = close_request_core(&mut *host.app, &mut host.clipboard, true, false, &mut host.ends_app);
        assert_eq!(decision, CloseDecision::Close);
        assert!(host.ends_app, "default no-hook preserves ends_app");
        assert!(!host.app.rt().needs_commit(), "default no-hook runs no update transaction");
        host.close();
        assert!(host.life.finished());
        assert_eq!(*events.borrow(), vec!["on_close"]);
    }
}

#[test]
fn close_request_bypassed_when_exiting_terminating_or_already_closed_and_works_with_keep_running() {
    // 1. Exiting loop bypasses on_close_request even when dirty.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let mut host = GuardedHost::new(true, true, false, events.clone(), &mem);
        assert_eq!(host.close_requested(true), CloseDecision::Close);
        assert!(host.life.finished());
        assert_eq!(*events.borrow(), vec!["on_close"], "exiting loop skips on_close_request");
    }

    // 2. OS teardown (terminate) bypasses on_close_request and runs on_close.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let mut host = GuardedHost::new(true, true, false, events.clone(), &mem);
        host.terminate();
        assert!(host.life.finished());
        assert_eq!(*events.borrow(), vec!["on_close"], "terminate skips on_close_request");
    }

    // 3. keep_running window: Defer keeps it Shown; Close hides it; once Hidden, CloseRequested skips hook.
    {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let mut host = GuardedHost::new(true, true, true, events.clone(), &mem);
        assert_eq!(host.close_requested(false), CloseDecision::Defer);
        assert!(host.life.shown(), "keep_running Defer keeps window shown");
        assert_eq!(*events.borrow(), vec!["on_close_request"]);

        // Programmatic close hides keep_running window without re-asking on_close_request.
        host.close();
        assert!(matches!(host.life, Life::Hidden { .. }));
        assert_eq!(*events.borrow(), vec!["on_close_request", "on_close"]);

        // Already-hidden keep_running window has closes() == false -> hook does not run.
        assert_eq!(host.close_requested(false), CloseDecision::Close);
        assert_eq!(*events.borrow(), vec!["on_close_request", "on_close"]);
    }
}

#[test]
fn exiting_cancels_wayland_drag_and_tears_down_proxies_with_on_display_close_before_display_closes() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mem = crate::clipboard::memory();
    let host = GuardedHost::new(false, false, false, events.clone(), &mem);
    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(host))];
    let mut display_close = vec![log_display_close(&events, "main")];
    let mut quit_state = QuitState::Running;
    let step = exiting_hosts(
        &mut hosts,
        &mut display_close,
        ExitFlags {
            has_startup_error: false,
            has_display_error: false,
            loop_exiting: false,
            quitting: false,
            fast_exit: false,
        },
        step_quit(&mut quit_state),
    );
    assert_eq!(step, ExitStep::Orderly { relaunch: false });
    let ev_platform = events.clone();
    finish_orderly_exiting(
        &mut hosts,
        false,
        || {},
        move || ev_platform.borrow_mut().push("wayland_drop_shutdown".into()),
        || {},
        || {},
    );
    assert!(hosts.is_empty(), "all hosts (and their Wayland DropTarget/SerialGuard) dropped before display close");
    let log = events.borrow();
    let close_pos = log.iter().position(|e| e == "on_close").expect("on_close ran");
    let display_close_pos = log.iter().position(|e| e == "main:on_display_close").expect("on_display_close ran");
    let shutdown_pos = log.iter().position(|e| e == "wayland_drop_shutdown").expect("shutdown ran");
    assert!(close_pos < display_close_pos, "terminate/close runs before on_display_close: {:?}", *log);
    assert!(display_close_pos < shutdown_pos, "on_display_close runs before final platform shutdown: {:?}", *log);
}

#[test]
fn unhandled_cmd_w_asks_on_close_request_before_closing() {
    use crate::runtime::Unhandled;
    use crate::{Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode as Code};

    let cmd = Mods { cmd: true, ..Mods::default() };
    let send_cmd_w = |host: &mut GuardedHost| {
        host.app.with_driver(&mut |rt, app| {
            rt.physical_then(
                app,
                PhysicalInput::Key { key: Code::KeyW, mods: cmd, pressed: true, repeat: false, text: None, consumed_mods: Default::default() },
                Some(KeyEvent { key: Key::Command('w'), mods: cmd }),
                None,
                Some(Unhandled::Close),
            );
        });
        host.app.frame(0.0);
        host.shell_step()
    };

    // 1. Dirty window with on_close_request returning Defer stays open and mounts the prompt.
    let events = Rc::new(RefCell::new(Vec::new()));
    let mem = crate::clipboard::memory();
    let mut host = GuardedHost::new(true, true, false, events.clone(), &mem);
    assert_eq!(send_cmd_w(&mut host), ShellCloseAction::UserCloseRequest);
    assert!(host.life.shown(), "unhandled Cmd+W must keep window open when on_close_request returns Defer");
    assert_eq!(*events.borrow(), vec!["on_close_request"]);
    host.app.frame(0.0);
    assert!(host.app.rt().find("save-prompt").is_some());

    // 2. Clean window (CloseResponse::Close) and unregistered window close through on_close.
    for with_hook in [true, false] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut host = GuardedHost::new(false, with_hook, false, events.clone(), &mem);
        assert_eq!(send_cmd_w(&mut host), ShellCloseAction::UserCloseRequest);
        assert!(host.life.finished());
        let expected = if with_hook { vec!["on_close_request", "on_close"] } else { vec!["on_close"] };
        assert_eq!(*events.borrow(), expected);
    }
}

#[test]
fn assistive_click_on_custom_close_button_asks_on_close_request_before_closing() {
    let mem = crate::clipboard::memory();

    // 1. Dirty window with on_close_request returning Defer stays open and mounts the prompt.
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut host = GuardedHost::new(true, true, false, events.clone(), &mem);
    host.app.rt_mut().set_caption(Some(crate::titlebar::Caption::new(36.0)));
    host.app.frame(0.0);
    assert!(
        host.app.rt_mut().accessibility_action("rune-caption-close", accesskit::Action::Click, None),
        "custom close button must accept AccessKit Click",
    );
    host.app.frame(0.0);
    assert_eq!(host.shell_step(), ShellCloseAction::UserCloseRequest);
    assert!(host.life.shown(), "assistive Click on custom close button must keep window open on Defer");
    assert_eq!(*events.borrow(), vec!["on_close_request"]);
    host.app.frame(0.0);
    assert!(host.app.rt().find("save-prompt").is_some());

    // 2. Clean window (CloseResponse::Close) and unregistered window close through on_close.
    for with_hook in [true, false] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut host = GuardedHost::new(false, with_hook, false, events.clone(), &mem);
        host.app.rt_mut().set_caption(Some(crate::titlebar::Caption::new(36.0)));
        host.app.frame(0.0);
        assert!(host.app.rt_mut().accessibility_action("rune-caption-close", accesskit::Action::Click, None));
        host.app.frame(0.0);
        assert_eq!(host.shell_step(), ShellCloseAction::UserCloseRequest);
        assert!(host.life.finished());
        let expected = if with_hook { vec!["on_close_request", "on_close"] } else { vec!["on_close"] };
        assert_eq!(*events.borrow(), expected);
    }

    // 3. Precedence: Quit > programmatic Shell::Close (bypasses hook) > UserCloseRequest > Minimize.
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::default());
    rt.update(|_, tx| { tx.minimize(); tx.request_user_close(); });
    assert_eq!(take_shell_action(rt.core_mut()), ShellCloseAction::UserCloseRequest);
    rt.update(|_, tx| { tx.request_user_close(); tx.close_window(); });
    assert_eq!(take_shell_action(rt.core_mut()), ShellCloseAction::ProgrammaticClose);
    rt.update(|_, tx| { tx.request_user_close(); tx.close_window(); tx.quit(); });
    assert_eq!(take_shell_action(rt.core_mut()), ShellCloseAction::Quit(crate::quit::After::Exit));
}

#[test]
fn orderly_exiting_hides_and_drops_hosts_and_removes_trays_before_warmup_wait() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mem = crate::clipboard::memory();
    let main_host = TestHost::new("main", true, events.clone(), &mem, None).with_drop_log();
    let overlay_host = TestHost::new("overlay", false, events.clone(), &mem, None).with_drop_log();
    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(main_host)), Some(Box::new(overlay_host))];
    let mut display_close = vec![log_display_close(&events, "main")];
    let mut quit_state = QuitState::Quitting(crate::quit::Hooks::Owed, crate::quit::After::Exit);

    let step = exiting_hosts(
        &mut hosts,
        &mut display_close,
        ExitFlags {
            has_startup_error: false,
            has_display_error: false,
            loop_exiting: true,
            quitting: true,
            fast_exit: false,
        },
        step_quit(&mut quit_state),
    );
    assert_eq!(step, ExitStep::Orderly { relaunch: false });

    finish_orderly_exiting(
        &mut hosts,
        false,
        || events.borrow_mut().push("tray:remove_all".into()),
        || events.borrow_mut().push("platform:shutdown".into()),
        || events.borrow_mut().push("relaunch:spawn".into()),
        || events.borrow_mut().push("instance:exited".into()),
    );
    assert!(hosts.is_empty(), "all window and overlay hosts must be dropped before warm-up wait");

    // Only after exiting finishes and run_app returns does shutdown_prewarm() wait.
    events.borrow_mut().push("warm:shutdown_wait".into());

    assert_eq!(
        *events.borrow(),
        vec![
            "main:vanish",
            "overlay:vanish",
            "main:on_quit",
            "overlay:on_quit",
            "main:on_display_close",
            "main:drop",
            "overlay:drop",
            "tray:remove_all",
            "platform:shutdown",
            "instance:exited",
            "warm:shutdown_wait",
        ],
        "every window, overlay, and tray icon must be hidden/removed before shutdown_prewarm waits",
    );
}

#[test]
fn display_queue_watches_stop_and_drop_strictly_before_on_display_close_on_every_exit() {
    use crate::window::display_queue;
    struct DropLog { events: Rc<RefCell<Vec<String>>>, label: &'static str }
    impl Drop for DropLog {
        fn drop(&mut self) { self.events.borrow_mut().push(self.label.into()); }
    }
    let attach_watch = |events: &Rc<RefCell<Vec<String>>>| {
        display_queue::stop_all();
        display_queue::reset_closed();
        let log = DropLog { events: events.clone(), label: "watch:drop" };
        display_queue::watch_display_queue(move || { let _ = &log; true }, || {})
    };
    // 1. Normal last-window close (orderly and fast_exit).
    for fast_exit in [false, true] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let _watch = attach_watch(&events);
        let mut main = TestHost::new("main", true, events.clone(), &mem, None).with_drop_log();
        main.close();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(main))];
        let mut hooks = vec![log_display_close(&events, "main")];
        let mut q = QuitState::Running;
        assert!(cleanup_hosts(&mut hosts, &mut hooks, true, false, fast_exit, step_quit(&mut q)));
        let expected = if fast_exit {
            vec!["main:on_close", "main:on_quit", "watch:drop", "main:on_display_close"]
        } else {
            vec!["main:on_close", "main:on_quit", "watch:drop", "main:on_display_close", "main:drop"]
        };
        assert_eq!(*events.borrow(), expected, "fast_exit={fast_exit}");
        assert!(!display_queue::has_watches());
    }
    // 2. App quit, OS quit, and error exits via exiting_hosts.
    for (label, flags, expected) in [
        (
            "app_quit",
            ExitFlags { has_startup_error: false, has_display_error: false, loop_exiting: true, quitting: true, fast_exit: false },
            vec!["main:vanish", "main:on_quit", "watch:drop", "main:on_display_close"],
        ),
        (
            "os_quit",
            ExitFlags { has_startup_error: false, has_display_error: false, loop_exiting: false, quitting: false, fast_exit: false },
            vec!["main:vanish", "main:on_quit", "main:on_close", "watch:drop", "main:on_display_close"],
        ),
        (
            "display_error",
            ExitFlags { has_startup_error: false, has_display_error: true, loop_exiting: true, quitting: false, fast_exit: false },
            vec!["watch:drop", "main:on_display_close"],
        ),
    ] {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mem = crate::clipboard::memory();
        let _watch = attach_watch(&events);
        let main = TestHost::new("main", true, events.clone(), &mem, None).with_drop_log();
        let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![Some(Box::new(main))];
        let mut hooks = vec![log_display_close(&events, "main")];
        let mut q = QuitState::Running;
        let _ = exiting_hosts(&mut hosts, &mut hooks, flags, step_quit(&mut q));
        assert_eq!(*events.borrow(), expected, "{label}");
        assert!(!display_queue::has_watches());
        assert!(hosts[0].is_some(), "{label}: host drops after on_display_close");
    }
    display_queue::reset_closed();
}
