use super::*;
use crate::{Cx, El, Key, KeyEvent, TextEdit, col, el, text_input};

struct State { pressed: bool, hovered: bool, secondary: bool, changed: bool, keyed: bool, edit: TextEdit }

fn runtime<S: Default + 'static>(view: fn(&S, &mut Cx<S>) -> El) -> crate::Runtime<S> {
    crate::Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0))
}

#[test]
fn one_patch_reaches_every_window_once_and_closed_windows_leave() {
    let hub: &'static Hub = Box::leak(Box::default());
    let woken = Arc::new(AtomicU64::new(0));
    let waker = |n: u64| -> Waker { let woken = woken.clone(); Arc::new(move || { woken.fetch_add(n, Ordering::Relaxed); }) };
    let mut main = Session::join(hub, waker(1));
    let mut settings = Session::join(hub, waker(10));
    assert!(!main.changed() && !settings.changed());
    hub.patched();
    assert_eq!(woken.load(Ordering::Relaxed), 11);
    assert!(main.pending() && main.changed() && !main.changed());
    assert!(settings.changed());
    drop(settings);
    hub.patched();
    assert_eq!(woken.load(Ordering::Relaxed), 12, "a closed window's waker is not called");
    // A window opened after a patch starts from the patched code; it has nothing to recommit.
    let late = Session::join(hub, waker(100));
    assert!(!late.pending());
    assert!(main.changed());
}

thread_local! {
    static CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The first call stands in for a panicking patch: with no patch loaded, the patch table
/// and the startup pointer are the same function.
fn panics_once() {
    CALLS.set(CALLS.get() + 1);
    if CALLS.get() == 1 { panic!("broken patched body"); }
}

#[test]
fn a_typed_window_survives_a_panicking_view_and_its_memos_rebuild() {
    CALLS.set(0);
    fn view(s: &u32, cx: &mut Cx<u32>) -> El {
        cx.time_scale(0.25);
        col().child(crate::memo("card", *s, || {
            panics_once();
            el().key("card").size(40.0, 40.0).on_press(|s: &mut u32| *s += 1)
        }))
    }
    let mut rt = crate::Runtime::new(5u32, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(CALLS.get(), 2, "the startup view runs once after the patched one panics");
    assert!(rt.find("card").is_some());
    assert_eq!(rt.time_scale, 0.25, "context writes come from the view that built the tree");
    let (x, y) = rt.find("card").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state, 6);
    assert_eq!(CALLS.get(), 3, "the memo cache works again: a new version builds once");
    rt.update(|_, _| {});
    rt.frame(0.0);
    assert_eq!(CALLS.get(), 3, "and an unchanged version is reused");
}

/// Index-built rows run the app's closures after the view returns; a patched one that
/// panics falls back like the view, and what the fallback memoized outlives its commit.
#[test]
fn a_panicking_index_built_row_falls_back_and_keeps_its_memos() {
    CALLS.set(0);
    fn view(s: &(u32, bool), _: &mut Cx<(u32, bool)>) -> El {
        col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
            col().key("rows").w_full().growth(crate::GrowthSpec::default()).virtual_rows(s.0 as usize, 40.0,
                |_: &(u32, bool), i| format!("r{i}").into(),
                |s: &(u32, bool), i| crate::memo(format!("r{i}"), s.1, || {
                    if s.1 && i == 0 { panics_once(); }
                    el().w_full().h(40.0)
                })))
    }
    let mut rt = crate::Runtime::new((20u32, false), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..3 {
        rt.update(|_, _| {});
        rt.frame(0.0);
    }
    rt.update(|s, _| s.1 = true);
    rt.frame(0.0);
    assert_eq!(CALLS.get(), 2, "the startup view builds the row once after the patched one panics");
    assert!(rt.find("r0").is_some());
    rt.update(|_, _| {});
    rt.frame(0.0);
    assert_eq!(CALLS.get(), 2, "the fallback's memo is reused");
}

#[test]
fn panicking_poll_and_on_key_patches_fall_back_to_the_startup_callbacks() {
    CALLS.set(0);
    let mut rt = crate::Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| el().size(10.0, 10.0), (100.0, 100.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, _, _| { panics_once(); *s += 1; true });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Function(5), mods: Default::default() });
    rt.frame(0.0);
    assert_eq!((rt.state, CALLS.get()), (1, 2));
    CALLS.set(0);
    rt.poll = Some(|s, _, _| { panics_once(); *s += 10; true });
    rt.frame(0.0);
    assert_eq!((rt.state, CALLS.get()), (11, 2));
    rt.poll = None;
    CALLS.set(0);
    rt.hooks.on_ime = Some(|s, _, _| { panics_once(); *s += 100; true });
    rt.ime(crate::Ime::Commit("x".into()));
    rt.frame(0.0);
    assert_eq!((rt.state, CALLS.get()), (111, 2));
}

#[test]
fn app_try_run_snaps_the_selected_secondary_window_at_its_own_size() {
    ENABLED.store(true, Ordering::Release);
    let dir = std::env::temp_dir().join(format!("rune-hot-snap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (png, control) = (dir.join("settings.png"), dir.join("control.json"));
    // Read after the first snapshot, so the loop ends once it is published.
    std::fs::write(&control, r#"{"quit":true}"#).unwrap();
    let mut app = crate::App::new(AppSpec::new("main", (120.0, 90.0), || 0u8, |_, _| el().key("main-root").size(10.0, 10.0)));
    app.window(crate::WindowSpec::from_app("settings",
        crate::App::new(AppSpec::new("settings", (60.0, 40.0), || 0u16, |_, _| el().key("settings-root").size(10.0, 10.0)))));
    let args = ["--snap-on-patch", png.to_str().unwrap(), "--hot-control", control.to_str().unwrap(), "--window", "settings", "--scale", "1"];
    app.args(args.map(String::from)).try_run().unwrap();
    let tree: Value = serde_json::from_slice(&std::fs::read(png.with_extension("json")).unwrap()).unwrap();
    let png_size = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&png).unwrap())).read_info().unwrap().info().size();
    std::fs::remove_dir_all(&dir).unwrap();
    let keys: Vec<&str> = tree["elements"].as_array().unwrap().iter().filter_map(|e| e["key"].as_str()).collect();
    assert_eq!(keys, ["settings-root"], "--window picks App::window's window, not main");
    assert_eq!(png_size, (60, 40));
}

#[test]
fn a_pending_patch_keeps_a_window_stale_until_a_frame_rebuilds_it() {
    let hub: &'static Hub = Box::leak(Box::default());
    let mut rt = crate::Runtime::new(0u8, |_: &u8, _: &mut Cx<u8>| el().size(10.0, 10.0), (100.0, 100.0), Env::new(true, 1.0));
    rt.frame.set_hot(Session::join(hub, Arc::new(|| {})));
    rt.frame(0.0);
    assert!(!rt.needs_commit() && !rt.stale());
    hub.patched();
    // A hidden launcher reveals its cached frame unless this says it is out of date, and
    // refreshes that frame when its hidden poll (the patch's wake) reports it, poll or not.
    assert!(rt.needs_commit() && rt.stale());
    assert!(rt.poll_hidden());
    rt.poll = Some(|_, _, _| false);
    assert!(rt.poll_hidden());
    rt.frame(0.0);
    assert!(!rt.needs_commit() && !rt.stale() && !rt.poll_hidden());
}

#[test]
fn snap_on_patch_is_a_headless_window_tool() {
    let args = Args::parse_from(vec!["--snap-on-patch".into(), "out.png".into(), "--window".into(), "settings".into()]);
    assert!(args.headless(), "run_windows must render the selected window headless, not open one");
}

#[test]
fn panicking_patch_falls_back_without_losing_state_or_context() {
    CALLS.set(0);
    fn view(s: &usize, cx: &mut Cx<usize>) -> El {
        if !originals_only() && CALLS.get() == 0 {
            CALLS.set(1);
            cx.time_scale(0.125);
            panic!("broken patched view");
        }
        CALLS.set(CALLS.get() + 1);
        let key = if !originals_only() && *s >= 8 { format!("recovered-{s}") } else { format!("count-{s}") };
        el().key(key).on_press(|s: &mut usize| *s += 1)
    }
    let mut rt = runtime(view);
    rt.state = 7;
    let address = &rt.state as *const usize;
    rt.frame(0.0);
    assert!(rt.find("count-7").is_some());
    assert_eq!(rt.time_scale, 1.0, "failed view's context writes must be discarded");
    control_input(&mut rt, &json!({"tap": "count-7"}));
    rt.frame(0.0);
    assert_eq!(rt.state, 8, "original handlers still operate on retained state");
    assert_eq!(&rt.state as *const usize, address);
    // A later healthy patch is still attempted rather than latching the failure.
    assert!(rt.find("recovered-8").is_some());
}

#[test]
fn invalid_control_input_is_skipped_and_later_requests_still_work() {
    let mut rt = runtime(|_: &usize, _: &mut Cx<usize>| el().key("button").on_press(|s: &mut usize| *s += 1));
    rt.frame(0.0);
    for command in [
        json!({"tap": "missing"}), json!({"tap": 1}),
        json!({"scroll": null}), json!({"scroll": "down"}),
        json!({"scroll": [1, 2]}), json!({"scroll": [1, 2, 3, 4]}),
        json!({"scroll": [1, "bad", 3]}), json!({"scroll": [1, 2, 1e100]}),
    ] {
        control_input(&mut rt, &command);
        rt.frame(0.0);
    }
    assert_eq!(rt.state, 0);
    control_input(&mut rt, &json!({"tap": "button", "scroll": [10, 10, 20]}));
    rt.frame(0.0);
    assert_eq!(rt.state, 1);
}

#[test]
fn typed_runtime_routes_focus_physical_links_and_window_callbacks() {
    #[derive(Default)]
    struct Events { presses: usize, physical: usize, dismisses: usize, links: Vec<String>, paste: String, focused: bool, moved: bool, layout: Option<[f32; 4]> }
    fn view(_: &Events, _: &mut Cx<Events>) -> El {
        col().size(300.0, 200.0)
            .on_paste(|s: &mut Events, text, _| s.paste = text.into())
            .on_window_focus(|s: &mut Events, focused, _| s.focused = focused)
            .modal(false, |s: &mut Events| s.dismisses += 1)
            .child(el().key("button").size(100.0, 30.0).focusable(true)
                .on_press(|s: &mut Events| s.presses += 1)
                .on_pointer_move(|s: &mut Events, _| s.moved = true)
                .on_layout(|s: &mut Events, rect| s.layout = Some(rect))
                .on_physical_input(|s: &mut Events, ev, _| {
                    if matches!(ev, crate::PhysicalInput::Modifiers(_)) { s.physical += 1; }
                    false
                }))
            .child(crate::text("docs").link(crate::Link::new(0..4, "docs"))
                .on_link(|s: &mut Events, id, _| s.links.push(id.into())))
    }
    let mut rt = runtime(view);
    rt.frame(0.0);
    rt.frame(0.0); // Deliver the first layout report.
    rt.window_focus(true);
    rt.paste("window paste");
    // The modal focuses its first control; Space presses it (Return would go to the default button).
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Default::default() });
    rt.physical_input(crate::PhysicalInput::Modifiers(Default::default()));
    rt.frame(0.0);
    let (x, y) = rt.find("button").unwrap();
    rt.pointer_move(x, y);
    rt.key(KeyEvent { key: Key::Tab, mods: Default::default() });
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
    rt.key(KeyEvent { key: Key::Escape, mods: Default::default() });
    rt.frame(0.0);
    let s = &rt.state;
    assert_eq!((s.presses, s.physical, s.dismisses), (1, 1, 1));
    assert_eq!(s.links, ["docs"]);
    assert_eq!(s.paste, "window paste");
    assert!(s.focused && s.moved);
    assert_eq!(s.layout.unwrap()[2..], [100.0, 30.0]);
}

#[test]
fn typed_runtime_preserves_context_reads_and_host_motion_preference() {
    #[derive(Default)]
    struct Reads { edit: TextEdit, seen: std::cell::RefCell<(bool, Option<[f32; 4]>, Option<[f32; 4]>)> }
    fn view(s: &Reads, cx: &mut Cx<Reads>) -> El {
        *s.seen.borrow_mut() = (cx.system_reduced_motion(), cx.layout("input"), cx.caret("input"));
        col().child(text_input(&s.edit, crate::TextStyle::new(14.0, 400.0)).key("input").size(180.0, 30.0)
            .on_edit(|s: &mut Reads| &mut s.edit))
    }
    let mut rt = runtime(view);
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    let (x, y) = rt.find("input").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.paste("caret");
    rt.frame(0.0);
    rt.frame(0.0);
    let s = &rt.state;
    let (reduced, layout, caret) = *s.seen.borrow();
    assert!(reduced);
    assert_eq!(layout.unwrap()[2..], [180.0, 30.0]);
    assert!(caret.unwrap()[0] > 0.0);
    assert_eq!(s.edit.text, "caret");
}

#[test]
fn typed_runtime_routes_handlers_and_context_to_the_typed_state() {
    fn view(s: &State, cx: &mut Cx<State>) -> El {
        cx.time_scale(0.75);
        col().child(el().key("button").size(80.0, 30.0).on_press(|s: &mut State| s.pressed = true)
                .on_hover(|s: &mut State| s.hovered = true)
                .on_secondary_press(|s: &mut State, _, _| s.secondary = true))
            .child(text_input(&s.edit, crate::tok::TextStyle::new(14.0, 400.0)).key("input").size(180.0, 30.0)
                .on_input_change(|s: &mut State, _| s.changed = true)
                .on_input_key(|s: &mut State, _, _| { s.keyed = true; false })
                .on_edit(|s: &mut State| &mut s.edit).on_submit(|s: &mut State, tx| { tx.copy(s.edit.text.clone()); }))
    }
    let mut rt = crate::Runtime::new(
        State { pressed: false, hovered: false, secondary: false, changed: false, keyed: false, edit: TextEdit::new("") },
        view,
        (200.0, 150.0),
        Env { system_dark: true, scale: 1.0 },
    );
    rt.frame(0.0);
    assert_eq!(rt.time_scale, 0.75);
    let (x, y) = rt.find("button").unwrap();
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    for key in ["button", "input"] {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
    }
    rt.paste("state kept");
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
    rt.frame(0.0);
    let s = &rt.state;
    assert!(s.pressed);
    assert!(s.hovered);
    assert!(s.secondary && s.changed && s.keyed);
    assert_eq!(s.edit.text, "state kept");
    assert_eq!(rt.copy_request.as_deref(), Some("state kept"));
}

#[test]
fn typed_window_app_with_turn_scope_connects_hot_session_wakes_rebuilds_and_falls_back() {
    use crate::window::{TypedWindowApp, WindowApp};
    #[derive(Default)]
    struct Scoped { enters: u32, leaves: u32 }
    thread_local! { static BUILDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
    fn view(_: &Scoped, _: &mut crate::Cx<Scoped>) -> crate::El {
        crate::memo("scoped", 0u8, || { BUILDS.set(BUILDS.get() + 1); el().size(20.0, 20.0) })
    }
    fn enter(s: &mut Scoped) { panics_once(); s.enters += 1; }
    fn leave(s: &mut Scoped) { panics_once(); s.leaves += 1; }

    enable_for_tests();
    let woken = Arc::new(AtomicU64::new(0));
    let w = woken.clone();
    let waker: Waker = Arc::new(move || { w.fetch_add(1, Ordering::Relaxed); });
    let app = crate::App::new(AppSpec::new("scoped", (80.0, 60.0), Scoped::default, view))
        .turn_scope(enter, leave);
    let mut win = TypedWindowApp::from_app(app, waker);
    let hub: &'static Hub = Box::leak(Box::default());
    win.rt.frame.hot_mut().expect("App::turn_scope connects a hot::Session").move_to(hub);
    BUILDS.set(0);
    win.rt.frame(0.0);
    assert_eq!(BUILDS.get(), 1);
    win.rt.update(|_, _| {});
    win.rt.frame(0.0);
    assert_eq!(BUILDS.get(), 1, "unchanged memo is reused");
    hub.patched();
    assert_eq!(woken.load(Ordering::Relaxed), 1, "patch wakes the window");
    assert!(win.rt.patch_pending() && win.rt.needs_commit());
    win.rt.frame(0.0);
    assert_eq!(BUILDS.get(), 2, "patch clears memos and rebuilds the view");
    assert!(!win.rt.patch_pending());

    CALLS.set(0);
    assert!(win.enter_turn());
    assert_eq!((win.rt.state.enters, CALLS.get()), (1, 2), "panicking patched enter falls back to startup enter");
    CALLS.set(0);
    win.leave_turn();
    assert_eq!((win.rt.state.leaves, CALLS.get()), (1, 2), "panicking patched leave falls back to startup leave");
}

#[test]
fn remaining_app_hooks_dispatch_through_patch_table_with_panic_fallback() {
    use crate::open_url::incoming::{Glue, Inbox};
    use crate::window::{TypedWindowApp, WindowApp};
    #[derive(Default)]
    struct Seen {
        focus: Vec<bool>,
        mods: Vec<crate::Mods>,
        visibility: Vec<bool>,
        close_requests: u32,
        quit_requests: u32,
        closes: u32,
        quits: u32,
        rests: u32,
        reopens: Vec<bool>,
        urls: Vec<String>,
        files: Vec<Vec<std::path::PathBuf>>,
    }
    struct FakeGlue;
    impl Glue for FakeGlue {
        fn handled_elsewhere(&self) -> bool { false }
        fn install(&self) -> bool { true }
        fn remove(&self) {}
    }

    let spec = AppSpec::new("hooks", (80.0, 60.0), Seen::default, |_: &Seen, _: &mut crate::Cx<Seen>| el().size(20.0, 20.0));
    let app = crate::App::new(spec)
        .on_focus(|s, f, _| { panics_once(); s.focus.push(f); })
        .on_modifiers(|s, m, _| { panics_once(); s.mods.push(m); })
        .on_visibility(|s, v, _| { panics_once(); s.visibility.push(v); })
        .on_close_request(|s, _| { panics_once(); s.close_requests += 1; crate::CloseResponse::Defer })
        .on_quit_request(|s, _| { panics_once(); s.quit_requests += 1; crate::QuitResponse::Defer })
        .on_close(|s, _| { panics_once(); s.closes += 1; })
        .on_quit(|s, _| { panics_once(); s.quits += 1; })
        .on_rest(|s, _| { panics_once(); s.rests += 1; true })
        .on_reopen(|s, vis, _| { panics_once(); s.reopens.push(vis); true })
        .on_open_url(|s, url, _| { panics_once(); s.urls.push(url.to_owned()); })
        .on_open_files(|s, paths, _| { panics_once(); s.files.push(paths.to_vec()); });

    let inbox: &'static Inbox = Box::leak(Box::new(Inbox::new()));
    let _listening = inbox.listen(&app, &FakeGlue).expect("on_open_url listens");
    let files_inbox: &'static Inbox<Vec<std::path::PathBuf>> = Box::leak(Box::new(Inbox::new()));
    let _listening_files = files_inbox.listen_files(&app, &FakeGlue).expect("on_open_files listens");
    let mut win = TypedWindowApp::from_app(app, Arc::new(|| {}));
    win.rt.frame(0.0);

    CALLS.set(0);
    win.rt.window_focus(true);
    win.rt.frame(0.0);
    assert_eq!((win.rt.state.focus.as_slice(), CALLS.get()), (&[true][..], 2), "on_focus");

    CALLS.set(0);
    let alt = crate::Mods { alt: true, ..crate::Mods::default() };
    win.rt.physical_observation(crate::PhysicalInput::Modifiers(alt));
    win.rt.frame(0.0);
    assert_eq!((win.rt.state.mods.as_slice(), CALLS.get()), (&[alt][..], 2), "on_modifiers");

    CALLS.set(0);
    win.rt.set_live_visible(false);
    assert_eq!((win.rt.state.visibility.as_slice(), CALLS.get()), (&[false][..], 2), "on_visibility");

    CALLS.set(0);
    assert_eq!(win.run_on_close_request(), Some(crate::CloseResponse::Defer));
    assert_eq!((win.rt.state.close_requests, CALLS.get()), (1, 2), "on_close_request");

    CALLS.set(0);
    assert_eq!(win.run_on_quit_request(), Some(crate::QuitResponse::Defer));
    assert_eq!((win.rt.state.quit_requests, CALLS.get()), (1, 2), "on_quit_request");

    CALLS.set(0);
    win.run_on_close();
    assert_eq!((win.rt.state.closes, CALLS.get()), (1, 2), "on_close");

    CALLS.set(0);
    assert!(win.run_on_quit());
    assert_eq!((win.rt.state.quits, CALLS.get()), (1, 2), "on_quit");

    CALLS.set(0);
    win.run_on_rest();
    assert_eq!((win.rt.state.rests, CALLS.get()), (1, 2), "on_rest");

    CALLS.set(0);
    let handled = win.queue_on_reopen(true).expect("on_reopen queued");
    for f in win.shared.take_updates() { win.rt.update_if(f); }
    assert!(handled.load(Ordering::Relaxed));
    assert_eq!((win.rt.state.reopens.as_slice(), CALLS.get()), (&[true][..], 2), "on_reopen");

    CALLS.set(0);
    inbox.receive("myapp://open?id=1".into());
    for f in win.shared.take_updates() { win.rt.update_if(f); }
    assert_eq!((win.rt.state.urls.as_slice(), CALLS.get()), (&["myapp://open?id=1".to_owned()][..], 2), "on_open_url");

    CALLS.set(0);
    files_inbox.receive(vec![std::path::PathBuf::from("/doc.torrent")]);
    for f in win.shared.take_updates() { win.rt.update_if(f); }
    assert_eq!((win.rt.state.files.as_slice(), CALLS.get()), (&[vec![std::path::PathBuf::from("/doc.torrent")]][..], 2), "on_open_files");
}

#[test]
fn cx_defaults_survive_hot_builds_and_discard_panicked_writes() {
    let fg = crate::Color { r: 0.1, g: 0.7, b: 0.4, a: 1.0 };
    let ring = crate::Color { r: 0.0, g: 0.6, b: 1.0, a: 1.0 };
    let look = crate::HelpLook::system(false);
    fn view(_: &u8, cx: &mut crate::Cx<u8>) -> crate::El {
        if !originals_only() && CALLS.get() == 0 {
            CALLS.set(1);
            cx.foreground(crate::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
            cx.time_scale(0.125);
            panic!("broken patched view");
        }
        CALLS.set(CALLS.get() + 1);
        cx.foreground(crate::Color { r: 0.1, g: 0.7, b: 0.4, a: 1.0 });
        cx.focus_ring(2.0, 1.0, crate::Color { r: 0.0, g: 0.6, b: 1.0, a: 1.0 });
        cx.help_look(crate::HelpLook::system(false));
        cx.press_cursor(crate::el::Cursor::Pointer);
        cx.force_focus_rings();
        cx.web_focus();
        el().size(40.0, 20.0)
    }
    CALLS.set(0);
    let mut rt = runtime::<u8>(view);
    rt.frame(0.0);
    assert_eq!(rt.time_scale, 1.0, "panicked view's time_scale is discarded");
    assert_eq!(rt.commit.defaults.foreground, Some(fg), "cx.foreground survives hot builds");
    assert_eq!(rt.commit.defaults.focus_ring, Some(crate::el::Outline { width: 2.0, offset: 1.0, color: ring }), "cx.focus_ring survives hot builds");
    assert_eq!(rt.commit.defaults.help, Some(look), "cx.help_look survives hot builds");
    assert_eq!(rt.commit.defaults.press_cursor, Some(crate::el::Cursor::Pointer), "cx.press_cursor survives hot builds");
    assert!(rt.commit.defaults.force_focus_rings, "cx.force_focus_rings survives hot builds");
    assert!(rt.commit.defaults.web_focus, "cx.web_focus survives hot builds");
}

#[test]
fn panicking_on_field_menu_patch_falls_back_with_owned_request() {
    struct State {
        edit: TextEdit,
        requests: Vec<crate::FieldMenuRequest>,
    }
    impl Default for State {
        fn default() -> Self {
            Self { edit: TextEdit::new("hello"), requests: Vec::new() }
        }
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(300.0, 200.0).child(
            text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("field")
                .on_edit(|s: &mut State| &mut s.edit)
                .size(200.0, 32.0),
        )
    }

    CALLS.set(0);
    let mut rt = crate::App::new(AppSpec::new("hot-field-menu", (300.0, 200.0), State::default, view))
        .on_field_menu(|s: &mut State, req: crate::FieldMenuRequest, _| {
            panics_once();
            s.requests.push(req);
        })
        .headless_runtime((300.0, 200.0), Env::new(true, 1.0));
    rt.core_mut().host.force_field_menu_fallback = true;
    rt.frame(0.0);

    let (x, y) = rt.find("field").unwrap();
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(CALLS.get(), 2, "startup on_field_menu runs once after the patched one panics");
    assert_eq!(rt.state.requests.len(), 1);
    assert_eq!(rt.state.requests[0].field_key(), Some("field"));
    assert!(!rt.state.requests[0].menu().entries().is_empty());

    // Subsequent non-panicking call delivers normally.
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(CALLS.get(), 3);
    assert_eq!(rt.state.requests.len(), 2);
}

#[test]
fn panicking_on_popup_menu_patch_falls_back_with_owned_request() {
    #[derive(Default)]
    struct State {
        requests: Vec<crate::PopupMenuRequest>,
        picks: Vec<Option<String>>,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(300.0, 200.0).child(el().key("btn").size(120.0, 32.0).on_press_tx(|_: &mut State, tx: &mut crate::Tx| {
            let menu = crate::PopupMenu::new().item(crate::PopupItem::new("ok", "OK"));
            tx.popup_menu(menu, "btn", |s: &mut State, picked, _| s.picks.push(picked.map(str::to_owned)));
        }))
    }

    CALLS.set(0);
    let mut rt = crate::App::new(AppSpec::new("hot-popup-menu", (300.0, 200.0), State::default, view))
        .on_popup_menu(|s: &mut State, req: crate::PopupMenuRequest, _| {
            panics_once();
            s.requests.push(req);
        })
        .headless_runtime((300.0, 200.0), Env::new(true, 1.0));
    rt.core_mut().host.force_popup_menu_fallback = true;
    rt.frame(0.0);

    let (x, y) = rt.find("btn").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(CALLS.get(), 2, "startup on_popup_menu runs once after the patched one panics");
    assert_eq!(rt.state.requests.len(), 1);
    assert_eq!(rt.state.requests[0].menu().entries().len(), 1);
    rt.update(|_, tx| tx.resolve_popup_menu(Some("ok")));
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [Some("ok".to_owned())]);
}

#[test]
fn patched_or_original_evaluates_and_reuses_non_copy_args() {
    fn record(out: &mut Vec<String>, payload: String) {
        panics_once();
        out.push(payload);
    }

    CALLS.set(0);
    let mut out = Vec::new();
    let mut built = 0usize;
    let mut make_payload = || {
        built += 1;
        format!("payload-{built}")
    };
    patched_or_original!("non_copy_test", record as fn(&mut Vec<String>, String), &mut out, make_payload());
    assert_eq!(built, 1, "argument expressions must be evaluated once");
    assert_eq!(CALLS.get(), 2, "fallback runs once after panic");
    assert_eq!(out, ["payload-1"]);
}
