use std::borrow::Cow;

use serde_json::Value;

use super::SETTLE_PASSES;
use crate::{App, AppSpec, Color, Cx, El, Env, GrowthSpec, Live, Runtime, ScrollSpec, col, el, oklch, text};

fn headless_rt<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El, size: (f32, f32)) -> Runtime<S> {
    let mut rt = Runtime::new(state, view, size, Env::new(true, 1.0));
    rt.motion.enable_motion_needs_cause();
    rt
}

fn run_settle<S: 'static>(rt: &mut Runtime<S>) -> (usize, Option<bool>) {
    let (core, mut driver) = rt.split_mut();
    core.settle_layout_passes(&mut driver, SETTLE_PASSES)
}

fn rect_of<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect)
}

fn find_tree_node<'a>(tree: &'a Value, key: &str) -> Option<&'a Value> {
    tree["elements"]
        .as_array()?
        .iter()
        .find(|el| el.get("key").and_then(Value::as_str) == Some(key))
}

#[test]
fn settle_runs_zero_passes_when_no_layout_callbacks_are_pending() {
    let mut rt = headless_rt(
        (),
        |_, _: &mut Cx<()>| el().key("box").size(80.0, 40.0).fill(Color::WHITE),
        (200.0, 100.0),
    );
    rt.frame(0.0);
    let commits = rt.stats.commits;
    let (passes, animating) = run_settle(&mut rt);
    assert_eq!(passes, 0, "idle frame with no layout callbacks must do no extra work");
    assert_eq!(animating, None);
    assert_eq!(rt.stats.commits, commits);
}

#[test]
fn settle_delivers_on_layout_and_on_layout_tx_at_zero_dt() {
    #[derive(Default)]
    struct State {
        width: f32,
        row_heights: [f32; 3],
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let header = el()
            .key("measure")
            .w_full()
            .h(30.0)
            .on_layout(|s: &mut State, r| s.width = r[2]);
        let bar = el().key("derived").w(s.width * 0.5).h(20.0).fill(Color::WHITE);
        let rows = col()
            .key("rows")
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(
                3,
                16.0,
                |_: &State, i| Cow::Owned(format!("row-{i}")),
                |s: &State, i| {
                    let measured = s.row_heights[i];
                    let content_h = (i as f32 + 1.0) * 20.0;
                    col()
                        .w_full()
                        .child(
                            el().key(format!("probe-{i}"))
                                .w_full()
                                .h(content_h)
                                .on_layout_tx(move |s: &mut State, r, _| {
                                    if (s.row_heights[i] - r[3]).abs() > 0.5 {
                                        s.row_heights[i] = r[3];
                                    }
                                }),
                        )
                        .child(el().key(format!("badge-{i}")).w(measured).h(10.0))
                },
            );
        let list = col()
            .key("list")
            .w_full()
            .h(150.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(rows);
        col().w_full().h_full().child(header).child(bar).child(list)
    }

    let mut rt = headless_rt(State::default(), view, (200.0, 220.0));
    rt.frame(0.0);
    assert_eq!(rect_of(&rt, "derived").unwrap()[2], 0.0, "frame 0 has not run on_layout yet");
    assert_eq!(rt.state.row_heights, [0.0, 0.0, 0.0]);
    assert_eq!(rect_of(&rt, "badge-2").unwrap()[2], 0.0);

    let (passes, _) = run_settle(&mut rt);
    assert!(passes >= 1 && passes <= SETTLE_PASSES, "expected 1..={SETTLE_PASSES} passes, got {passes}");
    assert_eq!(rt.clock, 0.0, "settle passes must not advance virtual time");
    assert_eq!(rect_of(&rt, "derived").unwrap()[2], 100.0, "on_layout updated derived width");
    assert_eq!(rt.state.row_heights, [20.0, 40.0, 60.0], "virtual_rows on_layout_tx heights settled");
    assert_eq!(rect_of(&rt, "badge-2").unwrap()[2], 60.0, "row content derived from on_layout_tx settled");
}

#[test]
fn settle_delivers_screen_drawn_visible_and_drawn_rect_into() {
    struct State {
        screen_x: f32,
        drawn_h: f32,
        visible: bool,
        ring: Live<[f32; 4]>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .child(
                el().key("src")
                    .w(120.0)
                    .h(44.0)
                    .on_screen_layout(|s: &mut State, r| s.screen_x = r[0])
                    .on_drawn_layout(|s: &mut State, r| s.drawn_h = r[3])
                    .on_visible(|s: &mut State, on, _| s.visible = on)
                    .drawn_rect_into(&s.ring),
            )
            .child(el().key("mirror").w(s.screen_x).h(s.drawn_h))
            .child(el().key("ring").pin(Some(0.0), Some(0.0), None, None).live_rect(&s.ring))
    }

    let mut rt = headless_rt(
        State { screen_x: 0.0, drawn_h: 0.0, visible: false, ring: Live::new([0.0; 4]) },
        view,
        (240.0, 120.0),
    );
    rt.set_window_origin((50.0, 20.0));
    rt.frame(0.0);
    assert_eq!(rect_of(&rt, "mirror").unwrap()[2], 0.0);
    assert!(!rt.state.visible);

    let (passes, _) = run_settle(&mut rt);
    assert_eq!(passes, 1, "single-stage callbacks and drawn_rect_into settle in 1 pass");
    assert_eq!(rt.clock, 0.0);
    assert_eq!(rt.state.screen_x, 50.0);
    assert_eq!(rt.state.drawn_h, 44.0);
    assert!(rt.state.visible);
    assert_eq!(rect_of(&rt, "mirror").unwrap()[2], 50.0);
    assert_eq!(rect_of(&rt, "mirror").unwrap()[3], 44.0);
    assert_eq!(rect_of(&rt, "ring").unwrap(), [0.0, 0.0, 120.0, 44.0]);
}

#[test]
fn settle_runs_chained_callbacks_and_stops_early_before_cap() {
    #[derive(Default)]
    struct State {
        stage1: f32,
        stage2: f32,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col()
            .w_full()
            .h_full()
            .child(el().key("a").w(90.0).h(20.0).on_layout(|s: &mut State, r| s.stage1 = r[2]));
        if s.stage1 > 0.0 {
            root = root.child(
                el().key("b").w(s.stage1).h(30.0).on_drawn_layout(|s: &mut State, r| s.stage2 = r[2] + r[3]),
            );
        }
        if s.stage2 > 0.0 {
            root = root.child(el().key("c").w(s.stage2).h(10.0));
        }
        root
    }

    let mut rt = headless_rt(State::default(), view, (200.0, 120.0));
    rt.frame(0.0);
    assert!(rect_of(&rt, "b").is_none() && rect_of(&rt, "c").is_none());

    let (passes, _) = run_settle(&mut rt);
    assert_eq!(passes, 2, "two-stage chain settles in 2 passes and stops before the 3-pass cap");
    assert_eq!(rect_of(&rt, "c").unwrap()[2], 120.0);
    assert!(!rt.core().layout_settle_pending());
}

#[test]
fn settle_live_only_callback_updates_live_value_without_extra_commit() {
    struct State {
        calls: usize,
        bar_rect: Live<[f32; 4]>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .child(el().key("src").w(150.0).h(20.0).on_layout_tx(|s: &mut State, r, tx| {
                s.calls += 1;
                s.bar_rect.set([0.0, 20.0, r[2], 20.0]);
                tx.live_only();
            }))
            .child(el().key("bar").pin(Some(0.0), Some(0.0), None, None).live_rect(&s.bar_rect))
    }

    let mut rt = headless_rt(
        State { calls: 0, bar_rect: Live::new([0.0; 4]) },
        view,
        (200.0, 100.0),
    );
    rt.frame(0.0);
    let commits_after_f0 = rt.stats.commits;

    let (passes, _) = run_settle(&mut rt);
    assert_eq!(passes, 1);
    assert_eq!(rt.state.calls, 1);
    assert_eq!(rt.stats.commits, commits_after_f0, "live_only skips view rebuild");
    assert_eq!(rect_of(&rt, "bar").unwrap(), [0.0, 20.0, 150.0, 20.0]);
}

#[test]
fn settle_terminates_at_cap_on_layout_feedback_loop() {
    #[derive(Default)]
    struct State {
        flip: bool,
        calls: usize,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let h = if s.flip { 80.0 } else { 40.0 };
        col().w_full().h_full().child(
            el().key("osc").w(100.0).h(h).on_layout(|s: &mut State, _| {
                s.calls += 1;
                s.flip = !s.flip;
            }),
        )
    }

    let mut rt = headless_rt(State::default(), view, (200.0, 200.0));
    rt.frame(0.0);
    let (passes, _) = run_settle(&mut rt);
    assert_eq!(passes, SETTLE_PASSES, "oscillating layout stops at SETTLE_PASSES");
    assert_eq!(rt.state.calls, SETTLE_PASSES);
    assert_eq!(rt.clock, 0.0);
}

#[test]
fn headless_snap_and_tree_with_settle_capture_settled_geometry_while_default_stays_frame_0() {
    if !crate::render::Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();

    #[derive(Default)]
    struct State {
        measured: f32,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let content = if s.measured > 0.0 {
            el().key("filled")
                .role("region")
                .label("Settled")
                .w(s.measured)
                .h(40.0)
                .fill(oklch(0.75, 0.2, 145.0))
                .child(text("ready"))
        } else {
            el().key("filled").w(0.0).h(0.0)
        };
        col()
            .w_full()
            .h_full()
            .fill(Color::BLACK)
            .child(
                el().key("probe")
                    .w(64.0)
                    .h(12.0)
                    .on_layout_tx(|s: &mut State, r, _| s.measured = r[2]),
            )
            .child(content)
    }

    let dir = std::env::temp_dir().join(format!("rune-settle-e2e-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();

    let run_one = |name: &str, settle: bool| -> (image::RgbaImage, Value) {
        let snap = dir.join(format!("{name}.png"));
        let tree = dir.join(format!("{name}.json"));
        let mut argv = vec![
            "--snap".into(), snap.to_string_lossy().into_owned(),
            "--tree".into(), tree.to_string_lossy().into_owned(),
            "--scale".into(), "1".into(),
        ];
        if settle { argv.push("--settle".into()); }
        let spec = AppSpec::new("settle-e2e", (64.0, 64.0), State::default, view);
        crate::tooling::headless(App::new(spec), crate::tooling::Args::parse_from(argv));
        let img = image::open(&snap).unwrap().into_rgba8();
        let json: Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
        (img, json)
    };

    let (raw_img, raw_tree) = run_one("raw", false);
    let (settled_img, settled_tree) = run_one("settled", true);
    std::fs::remove_dir_all(&dir).unwrap();

    // Without --settle, frame 0 is untouched: derived element is empty and pixels stay black.
    assert_eq!(raw_tree["t"].as_f64(), Some(0.0));
    assert_eq!(raw_tree["anim_t"].as_f64(), Some(0.0));
    let raw_filled = find_tree_node(&raw_tree, "filled").expect("filled node in raw tree");
    assert_eq!(raw_filled["rect"][2].as_f64(), Some(0.0));
    let raw_px = raw_img.get_pixel(32, 32).0;
    assert_eq!(raw_px, [0, 0, 0, 255], "without --settle, frame 0 remains black");

    // With --settle, layout callback ran and re-committed at t = 0 before capture.
    assert_eq!(settled_tree["t"].as_f64(), Some(0.0));
    assert_eq!(settled_tree["anim_t"].as_f64(), Some(0.0));
    let settled_filled = find_tree_node(&settled_tree, "filled").expect("filled node in settled tree");
    assert_eq!(settled_filled["rect"][2].as_f64(), Some(64.0));
    assert_eq!(settled_filled["rect"][3].as_f64(), Some(40.0));
    assert_eq!(settled_filled["role"].as_str(), Some("region"));
    assert_eq!(settled_filled["label"].as_str(), Some("Settled"));
    let settled_px = settled_img.get_pixel(32, 32).0;
    assert!(settled_px[1] > 150 && settled_px[0] < 120, "with --settle, green settled rect is painted: {settled_px:?}");
}

#[test]
fn headless_tree_with_settle_terminates_cleanly_on_feedback_loop() {
    if !crate::render::Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();

    #[derive(Default)]
    struct State {
        flip: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let h = if s.flip { 48.0 } else { 24.0 };
        col().w_full().h_full().child(
            el().key("loop").w(40.0).h(h).on_layout(|s: &mut State, _| s.flip = !s.flip),
        )
    }

    let tree = std::env::temp_dir().join(format!("rune-settle-loop-{}.json", std::process::id()));
    let spec = AppSpec::new("settle-loop", (64.0, 64.0), State::default, view);
    let args = crate::tooling::Args::parse_from(vec![
        "--tree".into(), tree.to_string_lossy().into_owned(),
        "--scale".into(), "1".into(),
        "--settle".into(),
    ]);
    crate::tooling::headless(App::new(spec), args);
    let json: Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_file(&tree).unwrap();

    assert_eq!(json["t"].as_f64(), Some(0.0));
    assert_eq!(json["anim_t"].as_f64(), Some(0.0));
    let node = find_tree_node(&json, "loop").expect("loop node in settled tree");
    assert_eq!(node["rect"][3].as_f64(), Some(48.0), "3 settle passes flip false -> true -> false -> true (48 pt)");
}

#[test]
fn headless_tree_with_settle_also_settles_at_nonzero_at_time() {
    if !crate::render::Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();

    #[derive(Default)]
    struct State {
        mounted: bool,
        measured: f32,
    }

    fn poll(s: &mut State, t: f64, _: &mut crate::Tx) -> bool {
        if !s.mounted && t >= 0.099 {
            s.mounted = true;
            true
        } else {
            false
        }
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().w_full().h_full();
        if s.mounted {
            root = root
                .child(el().key("late").w(48.0).h(12.0).on_layout_tx(|s: &mut State, r, _| s.measured = r[2]))
                .child(el().key("late-derived").w(s.measured).h(12.0));
        }
        root
    }

    let tree = std::env::temp_dir().join(format!("rune-settle-at-{}.json", std::process::id()));
    let mut spec = AppSpec::new("settle-at", (64.0, 64.0), State::default, view);
    spec.poll = Some(poll);
    let args = crate::tooling::Args::parse_from(vec![
        "--tree".into(), tree.to_string_lossy().into_owned(),
        "--at".into(), "0.1".into(),
        "--fps".into(), "10".into(),
        "--scale".into(), "1".into(),
        "--settle".into(),
    ]);
    crate::tooling::headless(App::new(spec), args);
    let json: Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_file(&tree).unwrap();

    assert!((json["t"].as_f64().unwrap() - 0.1).abs() < 1e-6);
    let node = find_tree_node(&json, "late-derived").expect("late-derived node in tree");
    assert_eq!(node["rect"][2].as_f64(), Some(48.0));
}
