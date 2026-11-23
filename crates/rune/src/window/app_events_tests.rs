use std::cell::RefCell;
use std::rc::Rc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{cleanup_hosts, quit_hosts};
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
        }
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

        let should_exit = cleanup_hosts(&mut hosts, true, fast_exit, step_quit(&mut quit_state));
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
    assert!(!cleanup_hosts(&mut hosts, true, true, step_quit(&mut quit_state)));
    assert!(hosts[0].is_none(), "a closed window drops while another main window keeps running");
    assert_eq!(*events.borrow(), vec!["first:on_close"]);

    // Turn 2: second (last) window closes.
    hosts[1].as_mut().unwrap().terminate();
    assert!(cleanup_hosts(&mut hosts, true, true, step_quit(&mut quit_state)));
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

    assert!(cleanup_hosts(&mut hosts, true, false, step_quit(&mut quit_state)));
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
