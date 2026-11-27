use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::app::TypedHeadlessApp;
use super::{HeadlessApp, HeadlessWake, await_key, try_headless};
use crate::{App, AppSpec, Color, Cx, El, Env, Runtime, ScrollSpec, col, el};

fn make_app<S: 'static>(
    spec: AppSpec<S>,
    wake: &HeadlessWake,
) -> (TypedHeadlessApp<S>, crate::AppHandle<S>, crate::native_menu::MenuGuard) {
    let app = App::new(spec);
    let handle = app.handle();
    let (on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files) = (
        app.on_close_request,
        app.on_quit_request,
        app.on_close,
        app.on_quit,
        app.on_reopen,
        app.on_open_url,
        app.on_open_files,
    );
    let (spec, state, shared, _) = app.into_parts(None);
    shared.core.connect(wake.waker());
    let menus = crate::native_menu::MenuGuard::attach(&shared, true);
    let mut rt = Runtime::new(state, spec.view, spec.size, Env::new(true, 1.0));
    rt.poll = spec.poll;
    rt.set_live_waker(wake.waker());
    let typed = TypedHeadlessApp {
        rt,
        shared: Arc::clone(&shared),
        comp: None,
        on_close_request,
        on_quit_request,
        on_close,
        on_quit,
        on_reopen,
        on_open_url,
        on_open_files,
        settle_ready: None,
    };
    (typed, handle, menus)
}

#[test]
fn await_key_returns_immediately_when_element_already_exists() {
    let wake = HeadlessWake::default();
    let spec = AppSpec::new("present", (100.0, 100.0), || (), |_: &(), _: &mut Cx<()>| {
        el().key("ready").size(40.0, 20.0)
    });
    let (mut app, _handle, _menus) = make_app(spec, &wake);
    app.rt.frame(0.0);

    let mut pumps = 0usize;
    let waited = await_key(&mut app, &wake, |_| pumps += 1, "ready", 2.0).expect("element exists");
    assert!(!waited);
    assert_eq!(pumps, 0, "present element must not pump extra frames");
}

#[test]
fn await_key_waits_for_async_update_with_stopped_clock() {
    #[derive(Default)]
    struct State {
        mounted: bool,
    }

    let wake = HeadlessWake::default();
    let spec = AppSpec::new("async", (100.0, 100.0), State::default, |s: &State, _: &mut Cx<State>| {
        if s.mounted {
            el().key("late").size(50.0, 30.0)
        } else {
            el().key("loading").size(50.0, 30.0)
        }
    });
    let (mut app, handle, mut menus) = make_app(spec, &wake);
    app.rt.frame(0.0);
    app.rt.frame(0.25);
    let clock_before = app.rt.clock;
    assert!(app.rt.find("late").is_none());

    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        handle.update(|s, _| s.mounted = true);
    });

    let waited = await_key(
        &mut app,
        &wake,
        |app| {
            app.drain_updates(&mut menus);
            app.frame(0.0);
        },
        "late",
        2.0,
    )
    .expect("element mounted before timeout");
    worker.join().unwrap();

    assert!(waited);
    assert!(app.rt.find("late").is_some());
    assert_eq!(app.rt.clock, clock_before, "virtual clock stays stopped while waiting");
}

#[test]
fn await_key_waits_for_poll_driven_mount_without_waker() {
    struct State {
        ready: Arc<AtomicBool>,
        mounted: bool,
    }

    let ready = Arc::new(AtomicBool::new(false));
    let ready_worker = Arc::clone(&ready);
    let wake = HeadlessWake::default();
    let mut spec = AppSpec::new(
        "poll-mount",
        (100.0, 100.0),
        || State { ready: Arc::new(AtomicBool::new(false)), mounted: false },
        |s: &State, _: &mut Cx<State>| {
            if s.mounted { el().key("from-poll").size(40.0, 20.0) } else { el() }
        },
    );
    spec.poll = Some(|s, _, _| {
        if !s.mounted && s.ready.load(Ordering::Acquire) {
            s.mounted = true;
            true
        } else {
            false
        }
    });
    let (mut app, _handle, mut menus) = make_app(spec, &wake);
    app.rt.state.ready = ready;
    app.rt.frame(0.0);

    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(35));
        ready_worker.store(true, Ordering::Release);
    });

    let waited = await_key(
        &mut app,
        &wake,
        |app| {
            app.drain_updates(&mut menus);
            app.frame(0.0);
        },
        "from-poll",
        2.0,
    )
    .expect("poll-mounted element appears before timeout");
    worker.join().unwrap();

    assert!(waited);
    assert_eq!(app.rt.clock, 0.0);
}

#[test]
fn await_key_times_out_with_clear_error_when_element_never_mounts() {
    let wake = HeadlessWake::default();
    let spec = AppSpec::new("timeout", (100.0, 100.0), || (), |_: &(), _: &mut Cx<()>| el());
    let (mut app, _handle, mut menus) = make_app(spec, &wake);
    app.rt.frame(0.0);

    let err = await_key(
        &mut app,
        &wake,
        |app| {
            app.drain_updates(&mut menus);
            app.frame(0.0);
        },
        "never",
        0.04,
    )
    .expect_err("missing element must time out");
    assert_eq!(err, r#"no element with key "never" after 0.04 s"#);
}

#[test]
fn await_key_stops_for_scrolled_out_element_so_script_point_reports_visibility_error() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut list = col().w(100.0);
        for i in 0..20 {
            list = list.child(el().key(format!("row-{i}")).size(100.0, 20.0));
        }
        col().size(100.0, 50.0).scroll(ScrollSpec::default()).child(list)
    }

    let wake = HeadlessWake::default();
    let spec = AppSpec::new("clipped", (100.0, 50.0), || (), view);
    let (mut app, _handle, _menus) = make_app(spec, &wake);
    app.rt.frame(0.0);

    // Element exists in the tree even though scrolled below the 50 pt viewport.
    let waited = await_key(&mut app, &wake, |_| {}, "row-15", 2.0).expect("exists in tree");
    assert!(!waited);

    // script_point still reports its own "not shown" diagnostic instead of a timeout.
    let ev = crate::tooling::Ev {
        t: 0.0,
        kind: crate::tooling::Kind::Move,
        pos: crate::tooling::Pos::Key("row-15".into()),
    };
    let err = crate::tooling::script_point(app.rt(), &ev, (0.0, 0.0)).expect_err("scrolled out");
    assert!(err.contains("is not shown"), "{err}");
}

#[test]
fn headless_script_wait_s_taps_delayed_element_and_reports_timeout() {
    if !crate::render::Gpu::test_adapter_available() {
        return;
    }
    let _turn = crate::tooling::headless_run::test_turn();

    #[derive(Default)]
    struct State {
        mounted: bool,
        clicked: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().w_full().h_full().fill(Color::BLACK);
        if s.mounted {
            root = root.child(
                el().key("delayed-btn")
                    .size(60.0, 40.0)
                    .fill(Color::WHITE)
                    .on_press(|s: &mut State| s.clicked = true),
            );
        }
        if s.clicked {
            root = root.child(el().key("was-clicked").size(20.0, 20.0));
        }
        root
    }

    let dir = std::env::temp_dir().join(format!("rune-wait-key-e2e-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();

    // 1. Success: delayed element mounts after 30 ms, tap lands and triggers on_press.
    let script_ok = dir.join("ok.json");
    let tree_ok = dir.join("ok-tree.json");
    std::fs::write(
        &script_ok,
        r#"[{"t": 0.0, "type": "tap", "key": "delayed-btn", "wait_s": 2.0}]"#,
    )
    .unwrap();
    let app = App::new(AppSpec::new("wait-ok", (80.0, 80.0), State::default, view));
    let handle = app.handle();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        handle.update(|s, _| s.mounted = true);
    });
    let args = crate::tooling::Args::parse_from(vec![
        "--script".into(),
        script_ok.to_string_lossy().into_owned(),
        "--tree".into(),
        tree_ok.to_string_lossy().into_owned(),
        "--at".into(),
        "0.2".into(),
        "--fps".into(),
        "10".into(),
        "--scale".into(),
        "1".into(),
    ]);
    try_headless(app, args).map_err(|e| e.message()).expect("script waited and tapped delayed-btn");
    worker.join().unwrap();

    let tree: Value = serde_json::from_str(&std::fs::read_to_string(&tree_ok).unwrap()).unwrap();
    let has_clicked = tree["elements"]
        .as_array()
        .unwrap()
        .iter()
        .any(|el| el.get("key").and_then(Value::as_str) == Some("was-clicked"));
    assert!(has_clicked, "tap after wait_s must have pressed delayed-btn: {tree}");
    assert!((tree["t"].as_f64().unwrap() - 0.2).abs() < 1e-6);

    // 2. Timeout: element never mounts, returns clear timeout error.
    let script_timeout = dir.join("timeout.json");
    std::fs::write(
        &script_timeout,
        r#"[{"t": 0.0, "type": "tap", "key": "never-mounts", "wait_s": 0.05}]"#,
    )
    .unwrap();
    let app = App::new(AppSpec::new("wait-timeout", (80.0, 80.0), State::default, view));
    let args = crate::tooling::Args::parse_from(vec![
        "--script".into(),
        script_timeout.to_string_lossy().into_owned(),
        "--at".into(),
        "0.1".into(),
        "--fps".into(),
        "10".into(),
        "--scale".into(),
        "1".into(),
    ]);
    let err = match try_headless(app, args) {
        Ok(()) => panic!("missing key must time out"),
        Err(e) => e.message(),
    };
    assert_eq!(err, r#"script: no element with key "never-mounts" after 0.05 s"#);

    std::fs::remove_dir_all(&dir).ok();
}
