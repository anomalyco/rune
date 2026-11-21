use super::*;
use crate::{Key, TextEdit, col, el, text_input};

struct State { pressed: bool, hovered: bool, secondary: bool, changed: bool, keyed: bool, edit: TextEdit }

fn runtime<S: Default + 'static>(view: fn(&S, &mut Cx<S>) -> El) -> crate::Runtime<HostState> {
    let spec = AppSpec::new("test", (300.0, 200.0), S::default, view);
    let host = HostState { value: Box::new((spec.init)()), spec: Box::new(spec), view: view_adapter::<S>, poll: poll_adapter::<S>, setup: setup_adapter::<S>, key: key_adapter::<S> };
    crate::Runtime::new(host, |s, cx| (s.view)(s, cx), (300.0, 200.0), Env::new(true, 1.0))
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
    // Through the type-erased host as well: its adapters dispatch through the table too.
    CALLS.set(0);
    fn view(_: &usize, _: &mut Cx<usize>) -> El { el().size(10.0, 10.0) }
    let mut host = runtime::<usize>(view);
    let spec = host.state.spec.downcast_mut::<AppSpec<usize>>().unwrap();
    spec.on_key = Some(|s, _, _| { panics_once(); *s += 1; true });
    host.on_key = Some(|s: &mut HostState, key, tx| (s.key)(s, key, tx));
    host.frame(0.0);
    host.key(KeyEvent { key: Key::Function(5), mods: Default::default() });
    host.frame(0.0);
    assert_eq!((*host.state.value.downcast_ref::<usize>().unwrap(), CALLS.get()), (1, 2));
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
    fn original(s: &usize, _: &mut Cx<usize>) -> El {
        el().key(format!("count-{s}")).on_press(|s: &mut usize| *s += 1)
    }
    let mut rt = runtime(original);
    *rt.state.value.downcast_mut::<usize>().unwrap() = 7;
    let address = rt.state.value.as_ref() as *const dyn Any as *const ();
    rt.state.view = |host, cx| {
        let state = host.value.downcast_ref::<usize>().unwrap();
        let original = host.spec.downcast_ref::<AppSpec<usize>>().unwrap().view;
        view_or_original(original, state, cx, |_, typed| {
            typed.time_scale(0.125);
            panic!("broken patched view")
        })
    };
    rt.frame(0.0);
    assert!(rt.find("count-7").is_some());
    assert_eq!(rt.time_scale, 1.0, "failed view's context writes must be discarded");
    control_input(&mut rt, &json!({"tap": "count-7"}));
    rt.frame(0.0);
    assert!(rt.find("count-8").is_some(), "original handlers still operate on retained state");
    assert_eq!(rt.state.value.as_ref() as *const dyn Any as *const (), address);
    // A later healthy patch is still attempted rather than latching the failure.
    rt.state.view = |host, cx| {
        let state = host.value.downcast_ref::<usize>().unwrap();
        view_or_original(original, state, cx, |s, _| el().key(format!("recovered-{s}")))
    };
    rt.update(|_, _| {});
    rt.frame(0.0);
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
    assert_eq!(*rt.state.value.downcast_ref::<usize>().unwrap(), 0);
    control_input(&mut rt, &json!({"tap": "button", "scroll": [10, 10, 20]}));
    rt.frame(0.0);
    assert_eq!(*rt.state.value.downcast_ref::<usize>().unwrap(), 1);
}

#[test]
fn erased_host_routes_focus_physical_links_and_window_callbacks() {
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
    let s = rt.state.value.downcast_ref::<Events>().unwrap();
    assert_eq!((s.presses, s.physical, s.dismisses), (1, 1, 1));
    assert_eq!(s.links, ["docs"]);
    assert_eq!(s.paste, "window paste");
    assert!(s.focused && s.moved);
    assert_eq!(s.layout.unwrap()[2..], [100.0, 30.0]);
}

#[test]
fn erased_host_preserves_context_reads_and_host_motion_preference() {
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
    let s = rt.state.value.downcast_ref::<Reads>().unwrap();
    let (reduced, layout, caret) = *s.seen.borrow();
    assert!(reduced);
    assert_eq!(layout.unwrap()[2..], [180.0, 30.0]);
    assert!(caret.unwrap()[0] > 0.0);
    assert_eq!(s.edit.text, "caret");
}

#[test]
fn erased_host_routes_handlers_and_context_to_the_typed_state() {
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
    let spec = AppSpec::new("test", (200.0, 150.0), || State { pressed: false, hovered: false, secondary: false, changed: false, keyed: false, edit: TextEdit::new("") }, view);
    let host = HostState { value: Box::new((spec.init)()), spec: Box::new(spec), view: view_adapter::<State>, poll: poll_adapter::<State>, setup: setup_adapter::<State>, key: key_adapter::<State> };
    let mut rt = crate::Runtime::new(host, |s, cx| (s.view)(s, cx), (200.0, 150.0), Env { system_dark: true, scale: 1.0 });
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
    let s = rt.state.value.downcast_ref::<State>().unwrap();
    assert!(s.pressed);
    assert!(s.hovered);
    assert!(s.secondary && s.changed && s.keyed);
    assert_eq!(s.edit.text, "state kept");
    assert_eq!(rt.copy_request.as_deref(), Some("state kept"));
}
