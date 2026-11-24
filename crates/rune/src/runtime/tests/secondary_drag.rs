use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::PointerButton;
use crate::el::{Drag, Phase};

type DragRecord = (Phase, PointerButton, [f32; 2], [f32; 2], bool);

struct CanvasState {
    drags: Vec<DragRecord>,
    presses: Vec<PointerButton>,
    secondary_canvas: Vec<PointerPress>,
    secondary_child: Vec<PointerPress>,
    pan: crate::Live<[f32; 2]>,
    live_only: bool,
    modal_open: bool,
    modal_dismisses: u32,
    builds: Rc<Cell<u32>>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            drags: Vec::new(),
            presses: Vec::new(),
            secondary_canvas: Vec::new(),
            secondary_child: Vec::new(),
            pan: crate::Live::new([0.0, 0.0]),
            live_only: false,
            modal_open: false,
            modal_dismisses: 0,
            builds: Rc::new(Cell::new(0)),
        }
    }
}

fn canvas_view(s: &CanvasState, _: &mut Cx<CanvasState>) -> El {
    s.builds.set(s.builds.get() + 1);
    let mut root = el().key("root").size(300.0, 200.0).child(
        el().key("canvas")
            .size(300.0, 200.0)
            .live_offset(&s.pan)
            .pointer_buttons([PointerButton::Secondary])
            .on_press_tx(|s: &mut CanvasState, tx| s.presses.push(tx.button()))
            .on_drag(|s: &mut CanvasState, d: &mut Drag, tx| {
                s.drags.push((d.phase, tx.button(), [d.x, d.y], [d.dx, d.dy], d.cancelled()));
                if s.live_only && tx.button() == PointerButton::Secondary {
                    s.pan.set([d.dx, d.dy]);
                    tx.live_only();
                }
            })
            .on_secondary_press(|s: &mut CanvasState, p, _| s.secondary_canvas.push(p))
            .focusable(true)
            .child(
                el().key("node")
                    .absolute(20.0, 20.0)
                    .size(80.0, 60.0)
                    .on_secondary_press(|s: &mut CanvasState, p, _| s.secondary_child.push(p)),
            ),
    );
    if s.modal_open {
        root = root.child(
            el().key("sheet")
                .absolute(180.0, 20.0)
                .size(100.0, 80.0)
                .modal(true, |s: &mut CanvasState| {
                    s.modal_open = false;
                    s.modal_dismisses += 1;
                }),
        );
    }
    root
}

#[test]
fn secondary_drag_delivers_down_move_up_with_secondary_button_and_live_only() {
    let builds = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(
        CanvasState { live_only: true, builds: builds.clone(), ..Default::default() },
        canvas_view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let builds_before = builds.get();

    rt.pointer_move(150.0, 100.0);
    rt.secondary_down(150.0, 100.0);
    assert!(rt.pressing(), "secondary drag holds pointer capture");
    assert_eq!(rt.secondary_claim(), "app", "web suppresses browser context menu during secondary drag");
    rt.frame(0.0);
    rt.pointer_move(190.0, 125.0);
    rt.frame(0.0);
    rt.secondary_up(210.0, 130.0);
    rt.frame(0.0);

    assert!(!rt.pressing());
    assert_eq!(builds.get(), builds_before, "live_only secondary drag rebuilds no view");
    assert_eq!(rt.state.pan.get(), [60.0, 30.0]);
    assert_eq!(rt.state.secondary_canvas.len(), 0, "right-drag suppresses on_secondary_press");
    assert_eq!(rt.state.presses.len(), 0, "right-drag never invokes on_press");
    assert!(rt.state.drags.len() >= 3, "expected Down, Move(s), Up: {:?}", rt.state.drags);
    assert_eq!(rt.state.drags[0].0, Phase::Down);
    assert_eq!(rt.state.drags[0].1, PointerButton::Secondary);
    assert_eq!(rt.state.drags[0].2, [150.0, 100.0]);
    assert_eq!(rt.state.drags[0].3, [0.0, 0.0]);
    assert!(rt.state.drags.iter().any(|d| d.0 == Phase::Move && d.1 == PointerButton::Secondary && d.3 == [40.0, 25.0]));
    let last = rt.state.drags.last().unwrap();
    assert_eq!(last.0, Phase::Up);
    assert_eq!(last.1, PointerButton::Secondary);
    assert_eq!(last.3, [60.0, 30.0]);
    assert!(!last.4);
}

#[test]
fn secondary_drag_reports_unzoomed_local_points_and_window_deltas() {
    let mut rt = Runtime::new(
        Vec::<(Phase, PointerButton, [f32; 2], [f32; 2])>::new(),
        |_, _| {
            el().key("world")
                .size(200.0, 200.0)
                .origin(0.0, 0.0)
                .scale(2.0)
                .pointer_buttons([PointerButton::Secondary])
                .on_drag(|s: &mut Vec<(Phase, PointerButton, [f32; 2], [f32; 2])>, d, tx| {
                    s.push((d.phase, tx.button(), [d.x, d.y], [d.dx, d.dy]));
                })
        },
        (400.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.secondary_down(100.0, 100.0);
    rt.pointer_move(140.0, 120.0);
    rt.secondary_up(140.0, 120.0);
    rt.frame(0.0);
    assert_eq!(rt.state.len(), 3);
    assert_eq!(rt.state[0], (Phase::Down, PointerButton::Secondary, [50.0, 50.0], [0.0, 0.0]));
    assert_eq!(rt.state[1], (Phase::Move, PointerButton::Secondary, [70.0, 60.0], [40.0, 20.0]));
    assert_eq!(rt.state[2], (Phase::Up, PointerButton::Secondary, [70.0, 60.0], [40.0, 20.0]));
}

#[test]
fn secondary_click_within_slop_opens_secondary_press_on_canvas_and_child_without_on_press() {
    let mut rt = Runtime::new(CanvasState::default(), canvas_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Stationary right-click on the canvas itself.
    rt.secondary_down(150.0, 100.0);
    rt.secondary_up(150.0, 100.0);
    rt.frame(0.0);
    assert_eq!(rt.state.secondary_canvas, [PointerPress { x: 150.0, y: 100.0 }]);
    assert!(rt.state.presses.is_empty(), "secondary click must never invoke on_press");
    assert_eq!(
        rt.state.drags.iter().map(|d| (d.0, d.1)).collect::<Vec<_>>(),
        [(Phase::Down, PointerButton::Secondary), (Phase::Up, PointerButton::Secondary)],
    );

    // Right-click with movement right at CLICK_SLOP (4 pt) on the child node still opens child's menu.
    rt.state.drags.clear();
    rt.secondary_down(50.0, 50.0);
    rt.pointer_move(54.0, 54.0);
    rt.secondary_up(54.0, 54.0);
    rt.frame(0.0);
    assert_eq!(rt.state.secondary_child, [PointerPress { x: 54.0, y: 54.0 }]);
    assert_eq!(rt.state.secondary_canvas.len(), 1, "child took its own secondary press");
    assert!(rt.state.presses.is_empty());
}

#[test]
fn secondary_drag_past_slop_suppresses_secondary_press_even_when_returning_to_start() {
    let mut rt = Runtime::new(CanvasState::default(), canvas_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Drag starting on child, moving past 4 pt, then returning back over child before release.
    rt.secondary_down(50.0, 50.0);
    rt.pointer_move(55.0, 50.0);
    rt.pointer_move(50.0, 50.0);
    rt.secondary_up(50.0, 50.0);
    rt.frame(0.0);
    assert!(rt.state.secondary_child.is_empty(), "child context menu suppressed after drag past slop");
    assert!(rt.state.secondary_canvas.is_empty(), "canvas context menu suppressed after drag past slop");
    assert_eq!(rt.state.drags.last().map(|d| (d.0, d.1)), Some((Phase::Up, PointerButton::Secondary)));

    // Direct jump on secondary_up without intermediate pointer_move also suppresses context menu.
    rt.secondary_down(150.0, 100.0);
    rt.secondary_up(170.0, 100.0);
    rt.frame(0.0);
    assert!(rt.state.secondary_canvas.is_empty(), "jump past slop on release suppresses context menu");
}

#[test]
fn non_opted_in_elements_keep_primary_only_drag_and_existing_secondary_press() {
    #[derive(Default)]
    struct Plain {
        drags: Vec<(Phase, PointerButton)>,
        secondary: u32,
    }
    fn view(_: &Plain, _: &mut Cx<Plain>) -> El {
        col().size(200.0, 200.0)
            .child(
                el().key("primary-only")
                    .size(200.0, 100.0)
                    .on_drag(|s: &mut Plain, d, tx| s.drags.push((d.phase, tx.button())))
                    .on_secondary_press(|s: &mut Plain, _, _| s.secondary += 1),
            )
            .child(
                el().key("middle-only")
                    .size(200.0, 100.0)
                    .pointer_buttons([PointerButton::Middle])
                    .on_drag(|s: &mut Plain, d, tx| s.drags.push((d.phase, tx.button())))
                    .on_secondary_press(|s: &mut Plain, _, _| s.secondary += 1),
            )
    }
    let mut rt = Runtime::new(Plain::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Secondary drag across primary-only element: never hits on_drag; release inside fires on_secondary_press.
    rt.secondary_down(20.0, 20.0);
    assert!(!rt.pressing(), "non-opted-in element does not start a press/drag capture");
    rt.pointer_move(80.0, 20.0);
    rt.secondary_up(80.0, 20.0);
    rt.frame(0.0);
    assert!(rt.state.drags.is_empty());
    assert_eq!(rt.state.secondary, 1);

    // Secondary drag across middle-only element: still never hits on_drag.
    rt.secondary_down(20.0, 120.0);
    rt.pointer_move(80.0, 120.0);
    rt.secondary_up(80.0, 120.0);
    rt.frame(0.0);
    assert!(rt.state.drags.is_empty());
    assert_eq!(rt.state.secondary, 2);

    // Primary drag on primary-only still works as before.
    rt.pointer_down(20.0, 20.0);
    rt.pointer_move(60.0, 20.0);
    rt.pointer_up(60.0, 20.0);
    rt.frame(0.0);
    assert_eq!(
        rt.state.drags,
        [(Phase::Down, PointerButton::Primary), (Phase::Move, PointerButton::Primary), (Phase::Up, PointerButton::Primary)],
    );
}

#[test]
fn secondary_drag_escape_cancel_delivers_cancelled_up_and_suppresses_menu() {
    let mut rt = Runtime::new(CanvasState::default(), canvas_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.secondary_down(150.0, 100.0);
    rt.pointer_move(175.0, 100.0);
    rt.frame(0.0);
    rt.physical_key_input(
        crate::PhysicalInput::Key {
            key: crate::PhysicalKeyCode::Escape,
            mods: crate::Mods::default(),
            pressed: true,
            repeat: false,
        },
        KeyEvent { key: crate::Key::Escape, mods: crate::Mods::default() },
        None,
    );
    rt.frame(0.0);
    let last = rt.state.drags.last().unwrap();
    assert_eq!((last.0, last.1, last.4), (Phase::Up, PointerButton::Secondary, true));
    let ups = rt.state.drags.iter().filter(|d| d.0 == Phase::Up).count();
    rt.secondary_up(175.0, 100.0);
    rt.frame(0.0);
    assert_eq!(rt.state.drags.iter().filter(|d| d.0 == Phase::Up).count(), ups, "release after cancel sends no second Up");
    assert!(rt.state.secondary_canvas.is_empty(), "release after Escape cancel opens no context menu");
}

#[test]
fn secondary_drag_respects_modals_inert_and_keyboard_context_menu() {
    let mut rt = Runtime::new(CanvasState { modal_open: true, ..Default::default() }, canvas_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Outside active modal: dismisses modal, does not start secondary drag on canvas behind it.
    rt.secondary_down(40.0, 40.0);
    rt.pointer_move(80.0, 40.0);
    rt.secondary_up(80.0, 40.0);
    rt.frame(0.0);
    assert_eq!(rt.state.modal_dismisses, 1);
    assert!(rt.state.drags.is_empty(), "outside-modal secondary press must not start secondary drag behind modal");

    // Keyboard context menu (Shift+F10) still triggers on_secondary_press on focused opted-in canvas.
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    rt.key(KeyEvent {
        key: crate::Key::Function(10),
        mods: crate::Mods { shift: true, ..Default::default() },
    });
    rt.frame(0.0);
    assert_eq!(rt.state.secondary_canvas.len(), 1, "Shift+F10 opens on_secondary_press on focused canvas");

    // Inert element with pointer_buttons([Secondary]) receives no secondary drag or secondary press.
    #[derive(Default)]
    struct InertState { drags: u32, secondary: u32 }
    let mut inert_rt = Runtime::new(
        InertState::default(),
        |_, _| {
            el().key("inert-canvas")
                .size(200.0, 200.0)
                .inert()
                .pointer_buttons([PointerButton::Secondary])
                .on_drag(|s: &mut InertState, _, _| s.drags += 1)
                .on_secondary_press(|s: &mut InertState, _, _| s.secondary += 1)
        },
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    inert_rt.frame(0.0);
    inert_rt.secondary_down(50.0, 50.0);
    inert_rt.pointer_move(90.0, 50.0);
    inert_rt.secondary_up(90.0, 50.0);
    inert_rt.frame(0.0);
    assert_eq!((inert_rt.state.drags, inert_rt.state.secondary), (0, 0));
}

#[test]
fn scripted_secondary_drag_reaches_on_drag_and_suppresses_secondary_press() {
    let mut rt = Runtime::new(CanvasState::default(), canvas_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let script = crate::tooling::load_script(&serde_json::json!([
        {"t": 0.0, "type": "drag", "button": "secondary", "key": "canvas", "dx": 80, "dy": 20, "dur": 0.05}
    ]));
    let mut last_down = (0.0, 0.0);
    for ev in &script {
        let pt = crate::tooling::script_point(rt.core(), ev, last_down).unwrap();
        if matches!(ev.kind, crate::tooling::Kind::Down | crate::tooling::Kind::SecondaryDown | crate::tooling::Kind::ButtonDown(_)) {
            last_down = pt;
        }
        let (core, mut app) = rt.split_mut();
        crate::tooling::script_input(core, &mut app, &ev.kind, pt);
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(rt.state.drags.first().map(|d| (d.0, d.1)), Some((Phase::Down, PointerButton::Secondary)));
    assert!(rt.state.drags.iter().any(|d| d.0 == Phase::Move && d.1 == PointerButton::Secondary));
    let last = rt.state.drags.last().unwrap();
    assert_eq!((last.0, last.1), (Phase::Up, PointerButton::Secondary));
    assert!((last.3[0] - 80.0).abs() < 1e-3 && (last.3[1] - 20.0).abs() < 1e-3, "got {:?}", last.3);
    assert!(rt.state.secondary_canvas.is_empty());
}
