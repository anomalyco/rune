use super::*;
use crate::Color;
use crate::live::Live;

struct BoardApp {
    open: bool,
    keep_board: bool,
    wrapped: bool,
    zoom: Live<f32>,
    pan: Live<[f32; 2]>,
}

fn board_view(s: &BoardApp, _: &mut Cx<BoardApp>) -> El {
    let tile = |w: f32, h: f32| {
        el().key("tile")
            .matched("tile")
            .size(w, h)
            .radius(8.0)
            .fill(Color::WHITE)
            .child(el().key("dot").absolute(10.0, 6.0).size(20.0, 10.0).fill(Color::BLACK))
    };
    let mut root = el().key("root").size(400.0, 300.0);
    if !s.open || s.keep_board {
        let mut board = el()
            .key("board")
            .size(400.0, 300.0)
            .live_scale(&s.zoom)
            .live_offset(&s.pan);
        if !s.open {
            board = board.child(tile(100.0, 50.0).absolute(40.0, 40.0));
        }
        let body = if s.wrapped {
            col().key("wrap").size(400.0, 300.0).child(board)
        } else {
            board
        };
        root = root.child(body);
    }
    if s.open {
        root = root.child(tile(160.0, 80.0).absolute(120.0, 20.0));
    }
    root
}

fn rect_of(rt: &Runtime<BoardApp>, key: &str) -> [f32; 4] {
    rt.presented(key).expect(key).0
}

fn max_diff(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

fn is_flying(rt: &Runtime<BoardApp>) -> bool {
    !rt.motion.flying.is_empty()
}

/// Resting screen rect of `tile` on the board with `zoom` and `pan` (board 400×300 centred at
/// (200+pan.x, 150+pan.y), tile local [40, 40, 100, 50]).
fn expected_board_tile(zoom: f32, pan: [f32; 2]) -> [f32; 4] {
    let cx = 200.0 + pan[0];
    let cy = 150.0 + pan[1];
    let x0 = cx + (40.0 + pan[0] - cx) * zoom;
    let y0 = cy + (40.0 + pan[1] - cy) * zoom;
    [x0, y0, 100.0 * zoom, 50.0 * zoom]
}

fn make_app(open: bool, keep_board: bool, wrapped: bool, zoom: f32, pan: [f32; 2]) -> Runtime<BoardApp> {
    let state = BoardApp {
        open,
        keep_board,
        wrapped,
        zoom: Live::new(zoom),
        pan: Live::new(pan),
    };
    let mut rt = Runtime::new(state, board_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Let any initial scale settle.
    for _ in 0..30 {
        rt.frame(1.0 / 120.0);
    }
    rt
}

/// Steps a flight until it lands and returns (first_frame, last_flying_frame, resting_frame, frames).
fn fly_to_rest(rt: &mut Runtime<BoardApp>) -> ([f32; 4], [f32; 4], [f32; 4], Vec<[f32; 4]>) {
    let mut frames = Vec::new();
    let mut last_flying = None;
    for _ in 0..300 {
        rt.frame(1.0 / 120.0);
        let r = rect_of(rt, "tile");
        if is_flying(rt) {
            last_flying = Some(r);
        }
        frames.push(r);
        if !is_flying(rt) && frames.len() > 10 {
            break;
        }
    }
    // Step a few more frames at rest so any settle tails finish.
    for _ in 0..30 {
        rt.frame(1.0 / 120.0);
    }
    let rest = rect_of(rt, "tile");
    let last_fly = last_flying.expect("tile flew");
    (frames[0], last_fly, rest, frames)
}

#[test]
fn matched_flight_home_onto_zoomed_board_lands_where_it_rests() {
    for keep_board in [false, true] {
        for wrapped in [false, true] {
            for (zoom, pan) in [(0.5, [60.0, 20.0]), (0.2, [40.0, -20.0]), (1.6, [-30.0, 25.0])] {
                let mut rt = make_app(true, keep_board, wrapped, zoom, pan);
                let start = rect_of(&rt, "tile");
                assert_eq!(start, [120.0, 20.0, 160.0, 80.0]);

                rt.update(|s, _| s.open = false);
                let (first, last_fly, rest, frames) = fly_to_rest(&mut rt);
                let expected = expected_board_tile(zoom, pan);
                let dot_rest = rect_of(&rt, "dot");
                let expected_dot = [expected[0] + 10.0 * zoom, expected[1] + 6.0 * zoom, 20.0 * zoom, 10.0 * zoom];

                assert!(
                    max_diff(rest, expected) < 0.05,
                    "keep={keep_board} wrapped={wrapped} zoom={zoom}: rest {rest:?} != expected {expected:?}"
                );
                assert!(
                    max_diff(dot_rest, expected_dot) < 0.05,
                    "keep={keep_board} wrapped={wrapped} zoom={zoom}: dot rest {dot_rest:?} != expected {expected_dot:?}"
                );
                assert!(
                    max_diff(first, start) < 12.0,
                    "keep={keep_board} wrapped={wrapped} zoom={zoom}: take-off jumped from {start:?} to {first:?}"
                );
                assert!(
                    max_diff(last_fly, rest) < 0.5,
                    "keep={keep_board} wrapped={wrapped} zoom={zoom}: last flying frame {last_fly:?} disagrees with resting rect {rest:?}"
                );
                // Smooth monotonic travel on both axes (no reversal or landing snap).
                for axis in 0..2 {
                    let vals: Vec<f32> = frames.iter().map(|r| r[axis]).collect();
                    let total = vals[vals.len() - 1] - vals[0];
                    if total.abs() > 2.0 {
                        for (k, w) in vals.windows(2).enumerate() {
                            assert!(
                                (w[1] - w[0]) * total.signum() >= -0.25,
                                "keep={keep_board} wrapped={wrapped} zoom={zoom} axis={axis}: reversed at frame {k}: {} -> {} (total {total})",
                                w[0], w[1]
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn matched_flight_off_zoomed_board_starts_where_it_rested_and_lands_cleanly() {
    for keep_board in [false, true] {
        for (zoom, pan) in [(0.5, [60.0, 20.0]), (1.6, [-30.0, 25.0])] {
            let mut rt = make_app(false, keep_board, false, zoom, pan);
            let start = rect_of(&rt, "tile");
            let expected_start = expected_board_tile(zoom, pan);
            assert!(
                max_diff(start, expected_start) < 0.05,
                "keep={keep_board} zoom={zoom}: initial tile {start:?} != {expected_start:?}"
            );

            rt.update(|s, _| s.open = true);
            let (first, last_fly, rest, frames) = fly_to_rest(&mut rt);

            assert_eq!(rest, [120.0, 20.0, 160.0, 80.0]);
            assert!(
                max_diff(first, start) < 12.0,
                "keep={keep_board} zoom={zoom}: take-off jumped from zoomed rect {start:?} to {first:?}"
            );
            assert!(
                max_diff(last_fly, rest) < 0.5,
                "keep={keep_board} zoom={zoom}: last flying frame {last_fly:?} disagrees with resting rect {rest:?}"
            );
            for w in frames.windows(2) {
                assert!(
                    max_diff(w[0], w[1]) < 20.0,
                    "keep={keep_board} zoom={zoom}: jump between frames {:?} -> {:?}",
                    w[0], w[1]
                );
            }
        }
    }
}

#[test]
fn matched_flight_mid_flight_toggle_on_zoomed_board_does_not_jump() {
    for (zoom, pan) in [(0.5, [60.0, 20.0]), (1.6, [-30.0, 25.0])] {
        let mut rt = make_app(true, false, false, zoom, pan);
        rt.update(|s, _| s.open = false);
        let mut prev = rect_of(&rt, "tile");
        for _ in 0..15 {
            rt.frame(1.0 / 120.0);
            prev = rect_of(&rt, "tile");
        }
        assert!(is_flying(&rt), "still mid-flight onto zoomed board");
        // Toggle back off the zoomed board mid-flight.
        rt.update(|s, _| s.open = true);
        rt.frame(1.0 / 120.0);
        let after = rect_of(&rt, "tile");
        assert!(
            max_diff(prev, after) < 15.0,
            "zoom={zoom}: mid-flight toggle jumped from {prev:?} to {after:?}"
        );
        let (_, last_fly, rest, _) = fly_to_rest(&mut rt);
        assert_eq!(rest, [120.0, 20.0, 160.0, 80.0]);
        assert!(max_diff(last_fly, rest) < 0.5, "zoom={zoom}: lands cleanly: {last_fly:?} vs {rest:?}");
    }
}

#[test]
fn matched_flight_onto_zoomed_board_passes_motion_gate() {
    for keep_board in [false, true] {
        for (zoom, pan) in [(0.5, [60.0, 20.0]), (1.5, [-30.0, 25.0])] {
            let state = BoardApp {
                open: true,
                keep_board,
                wrapped: false,
                zoom: Live::new(zoom),
                pan: Live::new(pan),
            };
            let mut run = crate::motion_gate::Run::new(
                Runtime::new(state, board_view, (400.0, 300.0), Env::new(true, 1.0)),
                120.0,
            );
            run.idle(0.3);
            run.step();
            run.act("close", |rt| rt.update(|s, _| s.open = false));
            run.play(1.2);
            run.act("open", |rt| rt.update(|s, _| s.open = true));
            run.play(1.2);
            let bad: Vec<String> = run
                .finish()
                .iter()
                .filter(|f| {
                    f.element.contains("tile")
                        && ["pop-out", "pop-in", "cut-exit", "bad-handover", "jump", "offset-jump", "appear-then-jump", "reversal", "overshoot", "never-settles", "jerk"]
                            .contains(&f.kind)
                })
                .map(|f| f.to_string())
                .collect();
            assert!(
                bad.is_empty(),
                "keep={keep_board} zoom={zoom}: motion_gate findings on tile: {bad:#?}"
            );
        }
    }
}

#[test]
fn same_size_matched_flight_passes_motion_gate() {
    fn view(moved: &bool, _: &mut Cx<bool>) -> El {
        let tile = || el().key("tile").matched("tile").size(80.0, 40.0).radius(8.0).fill(Color::WHITE);
        let mut root = el().key("root").size(400.0, 300.0);
        let mut left = el().key("left").absolute(0.0, 0.0).size(400.0, 300.0);
        let mut right = el().key("right").absolute(0.0, 0.0).size(400.0, 300.0);
        if *moved {
            right = right.child(tile().absolute(240.0, 140.0));
        } else {
            left = left.child(tile().absolute(40.0, 40.0));
        }
        root = root.child(left).child(right);
        root
    }
    let mut run = crate::motion_gate::Run::new(
        Runtime::new(false, view, (400.0, 300.0), Env::new(true, 1.0)),
        120.0,
    );
    run.idle(0.3);
    run.step();
    run.act("move", |rt| rt.update(|m, _| *m = true));
    run.play(1.2);
    let bad: Vec<String> = run
        .finish()
        .iter()
        .filter(|f| f.element.contains("tile"))
        .map(|f| f.to_string())
        .collect();
    assert!(bad.is_empty(), "same-size matched flight had motion_gate findings: {bad:#?}");
}

#[test]
fn unzoomed_matched_flight_matches_baseline_frames() {
    // Unzoomed (zoom = 1.0, pan = [0, 0]): compare a flight under a plain identity board against
    // a direct unzoomed flight without any live_scale / live_offset ancestor.
    fn plain_view(open: &bool, _: &mut Cx<bool>) -> El {
        let tile = |w: f32, h: f32| {
            el().key("tile")
                .matched("tile")
                .size(w, h)
                .radius(8.0)
                .fill(Color::WHITE)
                .child(el().key("dot").absolute(10.0, 6.0).size(20.0, 10.0).fill(Color::BLACK))
        };
        let mut root = el().key("root").size(400.0, 300.0);
        if !*open {
            root = root.child(el().key("board").size(400.0, 300.0).child(tile(100.0, 50.0).absolute(40.0, 40.0)));
        } else {
            root = root.child(tile(160.0, 80.0).absolute(120.0, 20.0));
        }
        root
    }
    let mut rt_live = make_app(true, false, false, 1.0, [0.0, 0.0]);
    let mut rt_plain = Runtime::new(true, plain_view, (400.0, 300.0), Env::new(true, 1.0));
    rt_plain.frame(0.0);
    for _ in 0..30 {
        rt_plain.frame(1.0 / 120.0);
    }
    rt_live.update(|s, _| s.open = false);
    rt_plain.update(|s, _| *s = false);
    for frame in 0..120 {
        rt_live.frame(1.0 / 120.0);
        rt_plain.frame(1.0 / 120.0);
        assert_eq!(
            rt_live.presented("tile").unwrap().0,
            rt_plain.presented("tile").unwrap().0,
            "frame {frame}: identity live_scale/live_offset must draw identically to plain unzoomed flight"
        );
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&rt_live.instances),
            bytemuck::cast_slice::<_, u8>(&rt_plain.instances),
            "frame {frame}: GPU instances must be bit-identical in the unzoomed case"
        );
    }
}
