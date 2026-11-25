//! Cross-platform host tests for `App::on_quit_request` and `QuitResponse`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::clipboard::Client as ClipboardClient;
use crate::el::{El, Tx, el};
use crate::quit::{self, After, Event, Loop, State};
use crate::quit::request::{RequestEffect, RequestEvent, RequestState, step as request_step};
use crate::runtime::{Cx, Env, Runtime};
use crate::window::app_events::{quit_hosts, quit_request_hosts};
use crate::window::host::app::{WindowApp, test_app_with_requests};
use crate::window::host::lifecycle::{ShellCloseAction, quit_request_core, take_shell_action};
use crate::window::state::Life;

fn shown_life() -> Life {
    let mut life = Life::Unopened;
    life.show();
    life
}
use crate::{App, AppSpec, QuitResponse};

struct QuitHostState {
    name: &'static str,
    dirty: bool,
    prompt: bool,
    copy_text: Option<&'static str>,
    quit_in_hook: bool,
    events: Rc<RefCell<Vec<String>>>,
}

fn quit_view(_: &QuitHostState, _: &mut Cx<QuitHostState>) -> El { el() }

fn on_quit_request_guard(s: &mut QuitHostState, tx: &mut Tx) -> QuitResponse {
    s.events.borrow_mut().push(format!("{}:on_quit_request", s.name));
    if let Some(text) = s.copy_text {
        tx.copy(text);
    }
    if s.quit_in_hook {
        tx.quit();
    }
    if s.dirty {
        s.prompt = true;
        QuitResponse::Defer
    } else {
        QuitResponse::Quit
    }
}

fn on_quit_guard(s: &mut QuitHostState, _: &mut Tx) {
    s.events.borrow_mut().push(format!("{}:on_quit", s.name));
}

struct MockQuitHost {
    app: Box<dyn WindowApp>,
    clipboard: ClipboardClient,
    life: Life,
    is_overlay: bool,
    redraws: u32,
    wakes: u32,
}

impl MockQuitHost {
    fn new(
        name: &'static str,
        dirty: bool,
        with_hook: bool,
        life: Life,
        events: Rc<RefCell<Vec<String>>>,
    ) -> Self {
        let rt = Runtime::new(
            QuitHostState { name, dirty, prompt: false, copy_text: None, quit_in_hook: false, events },
            quit_view,
            (200.0, 120.0),
            Env::default(),
        );
        let app = test_app_with_requests(
            rt,
            None,
            with_hook.then_some(on_quit_request_guard as fn(&mut QuitHostState, &mut Tx) -> QuitResponse),
            None,
            Some(on_quit_guard),
        );
        Self { app, clipboard: ClipboardClient::default(), life, is_overlay: false, redraws: 0, wakes: 0 }
    }

    fn overlay() -> Self {
        let mut h = Self::new("overlay", true, false, shown_life(), Rc::new(RefCell::new(Vec::new())));
        h.is_overlay = true;
        h
    }

    fn ask_quit_request(&mut self, exiting: bool) -> Option<QuitResponse> {
        if self.is_overlay {
            return None;
        }
        let resp = quit_request_core(&mut *self.app, &mut self.clipboard, self.life.finished(), exiting)?;
        if resp == QuitResponse::Defer && !self.life.finished() && !exiting {
            if self.life.shown() {
                self.redraws += 1;
            } else {
                self.wakes += 1;
            }
        }
        Some(resp)
    }
}

#[test]
fn quit_request_hosts_asks_in_order_short_circuits_on_defer_and_skips_overlays_and_finished() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut hosts: Vec<Option<Box<MockQuitHost>>> = vec![
        Some(Box::new(MockQuitHost::new("finished", true, true, Life::Finished, events.clone()))),
        Some(Box::new(MockQuitHost::overlay())),
        Some(Box::new(MockQuitHost::new("clean-shown", false, true, shown_life(), events.clone()))),
        Some(Box::new(MockQuitHost::new("dirty-hidden", true, true, Life::Hidden { at: 0 }, events.clone()))),
        Some(Box::new(MockQuitHost::new("windowless-after", true, true, Life::Unopened, events.clone()))),
    ];

    // Hidden keep_running host defers -> short-circuits before windowless-after; finished & overlay skipped.
    let allow = quit_request_hosts(&mut hosts, |h| h.ask_quit_request(false), |h| h.life.finished());
    assert!(!allow, "dirty hidden host must defer the quit");
    assert_eq!(
        *events.borrow(),
        vec!["clean-shown:on_quit_request", "dirty-hidden:on_quit_request"],
        "must ask in registration order, skip finished/overlay, and short-circuit on Defer",
    );
    assert_eq!(hosts[3].as_ref().unwrap().wakes, 1, "hidden host schedules pending wakes on Defer");
    assert!(!quit::quitting(), "Defer must leave rune::quitting() false");

    // When all live hosts return Quit (shown, hidden keep_running, windowless), quit is allowed.
    events.borrow_mut().clear();
    let mut all_clean: Vec<Option<Box<MockQuitHost>>> = vec![
        Some(Box::new(MockQuitHost::new("shown", false, true, shown_life(), events.clone()))),
        Some(Box::new(MockQuitHost::new("hidden", false, true, Life::Hidden { at: 0 }, events.clone()))),
        Some(Box::new(MockQuitHost::new("windowless", false, true, Life::Unopened, events.clone()))),
    ];
    let allow = quit_request_hosts(&mut all_clean, |h| h.ask_quit_request(false), |h| h.life.finished());
    assert!(allow, "when all live hosts return Quit, quit is allowed");
    assert_eq!(
        *events.borrow(),
        vec!["shown:on_quit_request", "hidden:on_quit_request", "windowless:on_quit_request"],
    );
}

#[test]
fn quit_request_defer_flushes_clipboard_requests_redraw_and_leaves_quitting_false() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut rt = Runtime::new(
        QuitHostState {
            name: "doc",
            dirty: true,
            prompt: false,
            copy_text: Some("draft backup"),
            quit_in_hook: false,
            events: events.clone(),
        },
        quit_view,
        (200.0, 120.0),
        Env::default(),
    );
    rt.frame(0.0);
    let mut app = test_app_with_requests(
        rt,
        None,
        Some(on_quit_request_guard),
        None,
        Some(on_quit_guard),
    );
    let mem = crate::clipboard::memory();
    let mut clipboard = ClipboardClient::default();
    clipboard.use_memory(&mem);
    let decision = quit_request_core(&mut *app, &mut clipboard, false, false);
    assert_eq!(decision, Some(QuitResponse::Defer));
    assert!(app.rt().needs_commit(), "state updates in on_quit_request mark the view dirty for redraw");
    assert_eq!(mem.text().as_deref().map(String::as_str), Some("draft backup"), "clipboard.flush_core flushed the copy request");
    assert_eq!(*events.borrow(), vec!["doc:on_quit_request"], "on_quit must not run on Defer");
    assert!(!quit::quitting());

    // Exiting loop bypasses on_quit_request completely.
    events.borrow_mut().clear();
    assert_eq!(quit_request_core(&mut *app, &mut clipboard, false, true), None);
    assert!(events.borrow().is_empty());
}

#[test]
fn programmatic_quit_relaunch_and_last_window_close_bypass_on_quit_request_and_preserve_relaunch() {
    // 1. Tx::quit inside on_quit_request produces ShellCloseAction::Quit(After::Exit) and
    //    upgrades an in-flight Asking state to ReplyQueued { allow: true }.
    let events = Rc::new(RefCell::new(Vec::new()));
    let rt = Runtime::new(
        QuitHostState {
            name: "doc",
            dirty: true,
            prompt: false,
            copy_text: None,
            quit_in_hook: true,
            events: events.clone(),
        },
        quit_view,
        (200.0, 120.0),
        Env::default(),
    );
    let mut app = test_app_with_requests(rt, None, Some(on_quit_request_guard), None, Some(on_quit_guard));
    let mut clipboard = ClipboardClient::default();
    let mut req_state = RequestState::Idle;
    let _ = request_step(&mut req_state, RequestEvent::OsQuitRequested { has_hooks: true, quitting: false });
    let decision = quit_request_core(&mut *app, &mut clipboard, false, false);
    assert_eq!(decision, Some(QuitResponse::Defer));
    assert_eq!(take_shell_action(app.rt_mut()), ShellCloseAction::Quit(After::Exit));
    let prog: Vec<_> = request_step(&mut req_state, RequestEvent::ProgrammaticQuit).into_iter().collect();
    assert_eq!(prog, vec![RequestEffect::ScheduleReply, RequestEffect::SuppressExit]);
    // Decided(false) from the Defer return cannot downgrade the upgraded YES reply.
    assert!(request_step(&mut req_state, RequestEvent::Decided { allow: false }).into_iter().next().is_none());
    let flush: Vec<_> = request_step(&mut req_state, RequestEvent::FlushReply { loop_state: Loop::Idle }).into_iter().collect();
    assert_eq!(flush, vec![RequestEffect::SendReply { allow: true }]);

    // 2. AppHandle::quit / AppHandle::relaunch record requests.quit directly (never calling on_quit_request),
    //    and quit::step preserves After::Relaunch when a subsequent Exit start arrives.
    let app_for_handle = App::new(AppSpec::new("h", (100.0, 100.0), || (), |_, _| el()))
        .on_quit_request(|_, _| panic!("AppHandle::relaunch must never invoke on_quit_request"));
    let handle = app_for_handle.handle();
    handle.relaunch();
    let (_, _, shared, _) = app_for_handle.into_parts(None);
    let reqs = shared.drain(|_| {});
    assert_eq!(reqs.quit, Some(After::Relaunch));
    let mut qstate = State::Running;
    let (next, _) = quit::step(qstate, Event::Start { ending: false, after: After::Relaunch });
    qstate = next;
    let (next, _) = quit::step(qstate, Event::Start { ending: false, after: After::Exit });
    qstate = next;
    let (next, effects) = quit::step(qstate, Event::Exited);
    assert_eq!(next, State::Done);
    assert_eq!(effects, vec![quit::Effect::Relaunch], "After::Relaunch survives Reply(YES)'s Exit start");

    // 3. quit_hosts (last-window exit / direct quit) runs on_quit without invoking on_quit_request.
    let mut host_list: Vec<Option<Box<dyn crate::window::AnyHost>>> = Vec::new();
    let mut qs = State::Running;
    let _ = quit_hosts(&mut host_list, false, false, false, |ev| {
        let (n, eff) = quit::step(qs, ev);
        qs = n;
        eff
    });
    assert!(matches!(qs, State::Quitting(quit::Hooks::Ran, After::Exit)));
}
