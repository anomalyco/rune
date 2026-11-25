use super::*;
use crate::el::{El, Shell, el};

use crate::runtime::{Cx, Env, Runtime};
use std::sync::atomic::{AtomicUsize, Ordering};

fn spec() -> AppSpec<u32> {
    AppSpec::new("test", (100.0, 100.0), || 0, |_: &u32, _: &mut Cx<u32>| el())
}

#[test]
fn a_named_web_canvas_sits_inline_unless_it_covers_the_viewport() {
    assert!(!App::new(spec()).web_inline(), "Rune's own canvas covers the viewport");
    assert!(!App::new(spec()).full_viewport().web_inline());
    assert!(App::new(spec()).canvas_id("view").web_inline(), "canvas_id alone stays inline");
    assert!(App::new(spec()).canvas_id("view").fit_content_height().web_inline());
    assert!(!App::new(spec()).canvas_id("route").full_viewport().web_inline());
    assert!(App::new(spec()).canvas_id("view").full_viewport().fit_content_height().web_inline(),
        "sizing to content wins over covering the viewport");
}

#[test]
fn window_panel_placement_and_pixel_position_are_last_request_wins() {
    let app = App::new(spec());
    let handle = app.handle();
    handle.set_position((10, 20));
    handle.place(crate::DisplayTarget::Primary, crate::OverlayPlacement::FullScreen);
    let q = app.shared.take();
    assert!(q.position.is_none());
    assert_eq!(q.place.as_ref().unwrap().placement, crate::OverlayPlacement::FullScreen);
    handle.place(crate::DisplayTarget::Primary, crate::OverlayPlacement::FullScreen);
    handle.set_position((-10, 20));
    let q = app.shared.take();
    assert!(q.place.is_none());
    assert_eq!(q.position, Some((-10, 20)));
}

#[test]
fn window_panel_focus_is_local_deduplicated_and_runs_without_frames() {
    let mut rt = Runtime::new(Vec::new(), |_: &Vec<bool>, _: &mut Cx<Vec<bool>>| el(), (100.0, 100.0), Env::default());
    rt.hooks.on_focus = Some(|seen, focused, tx| { seen.push(focused); if !focused { tx.close_window(); } });
    let mut other = Runtime::new(Vec::new(), |_: &Vec<bool>, _: &mut Cx<Vec<bool>>| el(), (100.0, 100.0), Env::default());
    other.hooks.on_focus = rt.hooks.on_focus;
    rt.window_focus(true);
    rt.window_focus(true);
    rt.window_focus(false);
    rt.window_focus(false);
    assert_eq!(rt.state, [true, false]);
    assert_eq!(rt.shell_request, Some(Shell::Close));
    assert!(other.state.is_empty());
    other.window_focus(false);
    assert_eq!(other.state, [false]);
    other.hooks.on_focus = None;
    other.frame(0.0);
    assert!(!other.stale());
    other.window_focus(true);
    assert!(!other.stale(), "focus on a committed view without on_window_focus asks for no extra frame");
}

#[test]
fn handles_report_the_window_the_host_publishes() {
    let app = App::new(spec());
    let handle = app.handle();
    assert!(!handle.is_visible() && handle.window_error().is_none());
    app.shared.visible.store(true, Ordering::Release);
    *app.shared.window_error.lock().unwrap() = Some("no GPU adapter".into());
    assert!(handle.is_visible());
    assert_eq!(handle.window_error().as_deref(), Some("no GPU adapter"));
}

#[test]
fn handles_cross_threads_whatever_the_state() {
    fn send_sync<T: Send + Sync + Clone>() {}
    send_sync::<AppHandle<std::rc::Rc<()>>>();
}

#[test]
fn init_closures_replace_the_spec_init_and_scenes_apply_on_top() {
    let mut spec = spec();
    spec.scenes = &[("big", |n| *n += 100)];
    let captured = 7;
    let (_, state, _, _) = App::new(spec).init(move || captured).into_parts(Some("big"));
    assert_eq!(state, 107);
    let (_, state, _, _) = App::new(self::spec()).into_parts(None);
    assert_eq!(state, 0);
}

#[test]
fn handle_requests_queue_before_the_loop_and_wake_it_after() {
    let app = App::new(spec()).windowless();
    assert!(!app.open && app.keep_running);
    let handle = app.handle();
    let other = handle.clone();
    std::thread::spawn(move || {
        other.update(|n, _| *n += 1);
        other.activate("token");
    })
    .join()
    .unwrap();
    handle.update(|n, _| *n *= 10);
    handle.close();
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    app.shared.connect(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        1,
        "connecting wakes once for queued requests"
    );
    let requests = app.shared.take();
    assert_eq!(
        requests.window,
        Some(WindowRequest::Close),
        "window requests collapse to the latest"
    );
    assert!(requests.quit.is_none());
    let mut n = 0;
    for f in requests.updates {
        f(&mut n, &mut Tx::default());
    }
    assert_eq!(n, 10, "updates apply in call order");
    handle.activate("");
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    assert_eq!(
        app.shared.take().window,
        Some(WindowRequest::Open(None)),
        "an empty token is no token"
    );
}

#[test]
fn a_window_an_update_opens_applies_in_the_same_drain() {
    let app = App::new(spec()).windowless();
    let handle = app.handle();
    let inner = handle.clone();
    handle.update(move |n, _| {
        *n += 1;
        inner.open();
        inner.update(|n, _| *n += 10);
    });
    let mut n = 0;
    let requests = app.shared.drain(|f| { f(&mut n, &mut Tx::default()); });
    assert_eq!(n, 1);
    assert_eq!(requests.window, Some(WindowRequest::Open(None)), "the open applies this turn");
    let next = app.shared.take();
    assert_eq!(next.updates.len(), 1, "an update's own update waits for the next turn");
    assert!(next.window.is_none());
}

#[test]
fn tray_requests_keep_show_focus_and_geometry_together() {
    let app = App::new(spec()).windowless();
    let handle = app.handle();
    std::thread::spawn(move || {
        handle.set_position((10, 20));
        handle.hide();
        handle.set_position((-300, 40));
        handle.show();
        handle.focus();
    }).join().unwrap();
    let requests = app.shared.take();
    assert_eq!(requests.window, Some(WindowRequest::Show), "focus must not overwrite show");
    assert_eq!(requests.activation, Some(Activation::Focus));
    assert_eq!(requests.position, Some((-300, 40)));
    let drained = app.shared.take();
    assert!(drained.activation.is_none() && drained.window.is_none() && drained.position.is_none());
}

#[test]
fn updates_commit_on_the_next_frame_and_carry_shell_requests() {
    fn view(n: &u32, _: &mut Cx<u32>) -> El {
        crate::el::row().child(el().key("box").size(10.0 + *n as f32, 10.0))
    }
    let mut rt = Runtime::new(
        0u32,
        view,
        (200.0, 200.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(0.0);
    rt.update(|n, tx| {
        *n = 30;
        tx.instant();
    });
    assert!(rt.shell_request.is_none());
    rt.frame(1.0 / 120.0);
    let b = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("box"))
        .unwrap();
    assert_eq!(b.rect[2], 40.0, "the update's instant commit lands at once");
    rt.update(|_, tx| tx.close_window());
    rt.update(|_, tx| tx.quit());
    rt.update(|_, tx| tx.close_window());
    assert_eq!(rt.shell_request.take(), Some(Shell::Quit), "quit outranks close");
}

#[test]
fn minimize_reaches_the_shell_from_handlers_updates_and_handles() {
    let mut rt = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env::default());
    rt.hooks.on_focus = Some(|_, focused, tx| if !focused { tx.minimize(); });
    rt.window_focus(true);
    assert!(!rt.host.minimize.latched());
    rt.window_focus(false);
    assert!(rt.host.minimize.field() && rt.shell_request.is_none(), "a handler's minimize is latched for the shell");
    rt.ignore_minimize("a test");
    assert!(!rt.host.minimize.latched(), "a shell that can't minimize takes the request");
    rt.update(|_, tx| tx.minimize());
    assert!(rt.host.minimize.field());
    let app = App::new(spec());
    app.handle().minimize();
    let requests = app.shared.take();
    assert!(requests.activation == Some(Activation::Minimize) && requests.window.is_none());
}

#[test]
fn a_handles_minimize_and_focus_are_last_wins_and_hide_close_or_open_cancel_it() {
    let app = App::new(spec());
    let h = app.handle();
    let after = |calls: &dyn Fn()| { calls(); let r = app.shared.take(); (r.window_requests.window, r.window_requests.activation) };
    assert_eq!(after(&|| { h.focus(); h.minimize(); }), (None, Some(Activation::Minimize)), "focus then minimize stays minimized");
    assert_eq!(after(&|| { h.minimize(); h.focus(); }), (None, Some(Activation::Focus)));
    assert_eq!(after(&|| { h.show(); h.minimize(); }), (Some(WindowRequest::Show), Some(Activation::Minimize)), "shows, then minimizes");
    assert_eq!(after(&|| { h.minimize(); h.show(); }), (Some(WindowRequest::Show), Some(Activation::Minimize)));
    assert_eq!(after(&|| { h.minimize(); h.hide(); }), (Some(WindowRequest::Hide), None));
    assert_eq!(after(&|| { h.minimize(); h.close(); }), (Some(WindowRequest::Close), None));
    assert_eq!(after(&|| { h.minimize(); h.open(); }), (Some(WindowRequest::Open(None)), None), "open restores");
    assert_eq!(after(&|| { h.focus(); h.open(); }), (Some(WindowRequest::Open(None)), Some(Activation::Focus)), "open leaves a focus asked first");
    assert_eq!(after(&|| { h.focus(); h.hide(); }), (Some(WindowRequest::Hide), Some(Activation::Focus)));
    assert_eq!(after(&|| { h.minimize(); h.show(); h.focus(); }), (Some(WindowRequest::Show), Some(Activation::Focus)));
}

#[test]
fn a_standard_close_or_minimize_key_acts_only_when_no_handler_takes_it() {
    use crate::runtime::Unhandled;
    use crate::{Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode as Code};
    let cmd = Mods { cmd: true, ..Mods::default() };
    let press = |rt: &mut Runtime<u32>, code, c, then| {
        rt.physical_then(PhysicalInput::Key { key: code, mods: cmd, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Command(c), mods: cmd }), None, Some(then));
        rt.frame(0.0);
    };
    let mut rt = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env::default());
    press(&mut rt, Code::KeyW, 'w', Unhandled::Close);
    assert!(rt.take_user_close_request(), "an unhandled ⌘W requests a user close like the close button");
    assert_eq!(rt.shell_request, None, "an unhandled ⌘W must not bypass on_close_request via Shell::Close");
    press(&mut rt, Code::KeyM, 'm', Unhandled::Minimize);
    assert!(rt.minimize_step(crate::window::MinimizeEvent::Taken) && rt.shell_request.is_none());
    rt.on_key = Some(|n, ev, _| { *n += 1; ev.is_command('w') });
    press(&mut rt, Code::KeyW, 'w', Unhandled::Close);
    assert_eq!((rt.state, rt.shell_request, rt.take_user_close_request()), (1, None, false), "the app's own ⌘W wins, once");
    press(&mut rt, Code::KeyM, 'm', Unhandled::Minimize);
    assert_eq!(rt.state, 2, "the app saw ⌘M first");
    assert!(rt.host.minimize.latched(), "and didn't take it");
}

thread_local! { static BUILDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }

fn counted(n: &u32, _: &mut Cx<u32>) -> El {
    BUILDS.set(BUILDS.get() + 1);
    crate::el::row().child(el().key("box").size(10.0 + *n as f32, 10.0))
}

fn drain(rt: &mut Runtime<u32>, app: &App<u32>) -> bool {
    let mut updated = false;
    for f in app.shared.take().updates { updated |= rt.update_if(f); }
    updated
}

#[test]
fn updates_that_change_nothing_build_nothing() {
    let app = App::new(AppSpec::new("t", (100.0, 100.0), || 0, counted));
    let handle = app.handle();
    let mut rt = Runtime::new(0u32, counted, (100.0, 100.0), Env::default());
    rt.frame(0.0);
    let (builds, commits) = (BUILDS.get(), rt.stats.commits);
    for _ in 0..100 {
        handle.update_if(|_, _| false);
        assert!(!drain(&mut rt, &app));
        assert!(!rt.stale(), "nothing to redraw");
        rt.frame(1.0 / 120.0);
    }
    assert_eq!((BUILDS.get() - builds, rt.stats.commits - commits), (0, 0));

    handle.update_if(|_, _| false);
    handle.update_if(|n, tx| { *n = 5; tx.instant(); true });
    handle.update_if(|_, _| false);
    assert!(drain(&mut rt, &app), "one change in the batch commits it");
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(BUILDS.get() - builds, 1, "once");
    assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("box")).unwrap().rect[2], 15.0);

    handle.update(|_, _| {});
    assert!(drain(&mut rt, &app), "`update` always counts as a change");
    rt.frame(1.0 / 120.0);
    assert_eq!(BUILDS.get() - builds, 2);
}

#[test]
fn an_update_that_changes_nothing_still_delivers_its_requests() {
    let mut rt = Runtime::new(0u32, counted, (100.0, 100.0), Env::default());
    rt.frame(0.0);
    assert!(!rt.update_if(|_, tx| { tx.copy("hello"); tx.wake_after(std::time::Duration::from_millis(500)); false }));
    assert!(!rt.stale());
    assert_eq!(rt.copy_request.as_deref(), Some("hello"));
    assert_eq!(rt.wake_deadline(), None, "left for the next frame, as `update` leaves it");
    rt.schedule_pending_wakes();
    assert_eq!(rt.wake_deadline(), Some(rt.clock + 0.5), "or scheduled by a host that runs none");
    assert!(!rt.update_if(|_, tx| { tx.quit(); false }));
    assert_eq!(rt.shell_request, Some(Shell::Quit));
}

/// A `wake_after(ZERO)` from an update that changed nothing, in a batch that still draws,
/// asks for the frame after, as `update`'s does: the frame doesn't spend it before polling.
#[test]
fn an_unchanging_updates_next_frame_timer_survives_a_frame() {
    let run = |changed: bool| {
        let mut rt = Runtime::new(0u32, counted, (100.0, 100.0), Env::default());
        rt.frame(0.0);
        rt.update_if(move |_, tx| { tx.wake_after(std::time::Duration::ZERO); changed });
        rt.update(|n, _| *n += 1);
        rt.frame(1.0);
        rt.wake_deadline()
    };
    assert!(run(true).is_some());
    assert_eq!(run(false), run(true));
}

#[test]
fn a_hidden_window_drains_its_inbox_on_wake_and_shows_it_when_reopened() {
    #[derive(Default)]
    struct Lock { inbox: Option<std::sync::mpsc::Receiver<u32>>, locked: u32 }
    fn view(s: &Lock, _: &mut Cx<Lock>) -> El {
        crate::el::row().child(el().key("box").size(10.0 + s.locked as f32, 10.0))
    }
    let mut rt = Runtime::new(Lock::default(), view, (200.0, 200.0), Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(|s: &mut Lock, _, tx: &mut Tx| {
        let Some(n) = s.inbox.as_ref().and_then(|inbox| inbox.try_iter().last()) else { return false };
        s.locked = n;
        tx.wake_after(std::time::Duration::from_secs(60));
        true
    });
    rt.wall_start = Some(crate::time::now());
    let (send, inbox) = std::sync::mpsc::channel();
    rt.state.inbox = Some(inbox);
    rt.frame(0.0);
    let width = |rt: &Runtime<Lock>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("box")).unwrap().rect[2];
    assert_eq!(width(&rt), 10.0);

    // Hidden from here: something queues a lock and wakes the loop.
    send.send(30).unwrap();
    assert!(rt.poll_hidden(), "a hidden launcher renders when poll reports a change");
    assert!(!rt.poll_hidden());
    assert_eq!(rt.state.locked, 30, "poll drained the inbox without a frame");
    assert_eq!(width(&rt), 10.0, "nothing laid out, animated or presented while hidden");
    assert!(rt.wake_deadline().is_some(), "poll's own timers still schedule while hidden");

    // `show` lands everything on its first frame.
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    assert_eq!(width(&rt), 40.0);
}

/// An update's `wake_after`, delivered while no frame draws (hidden, covered, asleep or
/// minimised), arms its timer on the next hidden poll.
#[test]
fn an_update_arriving_without_frames_arms_its_timer() {
    let mut rt = Runtime::new(0_u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(|polls: &mut u32, _, _: &mut Tx| { *polls += 1; false });
    rt.wall_start = Some(crate::time::now());
    rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), None);
    rt.update(|_, tx| tx.wake_after(std::time::Duration::from_secs(5)));
    rt.poll_hidden();
    let deadline = rt.wake_deadline().expect("the update's timer is armed without a frame");
    assert!((4.9..5.5).contains(&deadline), "it counts from the update: {deadline}");
    rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), Some(deadline), "and the next frame doesn't arm it twice");
}

/// A hidden host's due timer runs `poll` and spends the timer without building the view,
/// poll or not. The view waits, dirty, for the frame that shows the host.
#[test]
fn a_hidden_timer_polls_without_building_the_view() {
    use std::time::Duration;
    for with_poll in [true, false] {
        let mut rt = Runtime::new(0_u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
        if with_poll { rt.poll = Some(|polls: &mut u32, _, _: &mut Tx| { *polls += 1; true }); }
        let t0 = crate::time::now();
        rt.wall_start = Some(t0);
        rt.frame(0.0);
        let commits = rt.stats.commits;
        rt.update(|_, tx| tx.wake_after(Duration::from_millis(10)));
        assert_eq!(rt.hidden_wake(), with_poll);
        assert!(rt.wake_deadline().is_some());
        rt.wall_start = Some(t0 - Duration::from_secs(1));
        assert_eq!(rt.hidden_wake(), with_poll);
        assert_eq!(rt.wake_deadline(), None, "the due timer is spent, so it doesn't wake the host again");
        assert_eq!(rt.stats.commits, commits, "no view or layout while hidden");
        assert!(rt.needs_commit());
        rt.frame(0.0);
        assert_eq!(rt.stats.commits, commits + 1, "the frame that shows it builds once");
    }
}

#[test]
fn app_handle_set_color_scheme_coalesces_wakes_and_takes_independently() {
    let app = App::new(spec());
    let handle = app.handle();
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    app.shared.connect(Arc::new(move || { w.fetch_add(1, Ordering::Relaxed); }));
    let before = wakes.load(Ordering::Relaxed);

    handle.set_color_scheme(crate::ColorScheme::Light);
    handle.set_color_scheme(crate::ColorScheme::Dark);
    assert_eq!(wakes.load(Ordering::Relaxed), before + 2);
    assert_eq!(app.shared.take().color_scheme, Some(crate::ColorScheme::Dark));
    assert_eq!(app.shared.take().color_scheme, None);

    // Startup take_color_scheme consumes only color_scheme and keeps other queued requests.
    handle.set_color_scheme(crate::ColorScheme::System);
    handle.set_position((12, 34));
    assert_eq!(app.shared.core.take_color_scheme(), Some(crate::ColorScheme::System));
    assert_eq!(app.shared.core.take_color_scheme(), None);
    let rest = app.shared.take();
    assert_eq!(rest.color_scheme, None);
    assert_eq!(rest.position, Some((12, 34)));
}

#[test]
fn on_display_close_stores_once_hook_and_replaces_previous() {
    let ran = Arc::new(AtomicUsize::new(0));
    let first_ran = ran.clone();
    let second_ran = ran.clone();
    let owned = String::from("foreign-proxy");
    let mut app = App::new(spec())
        .on_display_close(move || { first_ran.fetch_add(1, Ordering::Relaxed); })
        .on_display_close(move || {
            drop(owned);
            second_ran.fetch_add(10, Ordering::Relaxed);
        });
    let hook = app.on_display_close.take().expect("hook registered");
    assert!(app.on_display_close.is_none());
    hook();
    assert_eq!(ran.load(Ordering::Relaxed), 10);
}

#[test]
fn on_close_request_defaults_to_none_and_keeps_on_close_independent() {
    let default_app = App::new(spec());
    assert!(default_app.on_close_request.is_none());
    assert!(default_app.on_close.is_none());

    let app = App::new(spec())
        .on_close_request(|_, _| crate::CloseResponse::Defer)
        .on_close_request(|n, _| {
            *n += 1;
            crate::CloseResponse::Close
        })
        .on_close(|n, _| { *n += 10; });
    assert!(app.on_close_request.is_some());
    assert!(app.on_close.is_some());
    let mut rt = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env::default());
    let mut resp = crate::CloseResponse::Defer;
    let f = app.on_close_request.unwrap();
    rt.update(|s, tx| resp = f(s, tx));
    assert_eq!(resp, crate::CloseResponse::Close);
    assert_eq!(rt.state, 1);
}
