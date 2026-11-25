use std::cell::Cell;
use std::rc::Rc;

use super::{LockEffect, LockEvent, LockState, PointerLock, step};
use crate::runtime::input::action::Action;
use crate::tooling::{Kind, ScriptEvent, load_script, script_input};
use crate::{
    Cx, Drag, El, Env, Key, KeyEvent, Live, Mods, Phase, PhysicalInput, PhysicalKeyCode,
    PointerButton, Runtime, col, el, memo, text,
};

#[derive(Default)]
struct Scene {
    playing: bool,
    card_clickable: bool,
    modal_open: bool,
    viewport_disabled: bool,
    viewport_inert: bool,
    viewport_drag: bool,
    live_only_motion: bool,
    live_yaw: Option<Live<f32>>,
    view_builds: Rc<Cell<usize>>,
    memo_builds: Rc<Cell<usize>>,
    locked_seen: Rc<Cell<bool>>,
    viewport_presses: usize,
    viewport_secondary: usize,
    viewport_middle: usize,
    viewport_wheels: usize,
    viewport_moves: usize,
    toolbar_hovers: usize,
    hud_presses: usize,
    card_presses: usize,
    app_escapes: usize,
    drag_phases: Vec<(Phase, bool)>,
    motions: Vec<[f32; 2]>,
}

impl Scene {
    fn playing() -> Self {
        Self {
            playing: true,
            card_clickable: true,
            ..Self::default()
        }
    }
}

/// Issue #494 acceptance fixture: 300×200 column with a 40-pt toolbar, a 300×160 viewport
/// with `on_press` and conditional `pointer_lock`, a HUD button inside the viewport, and a
/// card drawn over the viewport's bottom-right corner.
fn fixture_view(s: &Scene, cx: &mut Cx<Scene>) -> El {
    s.view_builds.set(s.view_builds.get() + 1);
    let locked = cx.pointer_locked();
    s.locked_seen.set(locked);

    let memo_builds = s.memo_builds.clone();
    let hint = memo("lock-hint", 0u8, || {
        memo_builds.set(memo_builds.get() + 1);
        text(if locked { "Locked" } else { "Click to look" })
    });

    let toolbar = el()
        .key("toolbar")
        .size(300.0, 40.0)
        .on_hover(|s: &mut Scene| s.toolbar_hovers += 1)
        .child(hint);

    let hud = el()
        .key("hud")
        .absolute(10.0, 10.0)
        .size(60.0, 30.0)
        .on_press(|s: &mut Scene| s.hud_presses += 1);

    let mut viewport = el()
        .key("viewport")
        .size(300.0, 160.0)
        .on_press(|s: &mut Scene| s.viewport_presses += 1)
        .on_secondary_press(|s: &mut Scene, _, _| s.viewport_secondary += 1)
        .on_middle_press(|s: &mut Scene, _| s.viewport_middle += 1)
        .on_wheel(|s: &mut Scene, _, _| {
            s.viewport_wheels += 1;
            true
        })
        .on_pointer_move(|s: &mut Scene, _| s.viewport_moves += 1)
        .child(hud);

    if s.viewport_drag {
        viewport = viewport.on_drag(|s: &mut Scene, d: &mut Drag, _| {
            s.drag_phases.push((d.phase, d.cancelled()));
        });
    }
    if s.playing {
        viewport = viewport.pointer_lock(|s: &mut Scene, delta, tx| {
            s.motions.push(delta);
            if let Some(yaw) = &s.live_yaw {
                yaw.set(yaw.get() + delta[0]);
            }
            if s.live_only_motion {
                tx.live_only();
            }
        });
    }
    if s.viewport_disabled {
        viewport = viewport.disabled(true);
    }
    if s.viewport_inert {
        viewport = viewport.inert();
    }

    let mut card = el()
        .key("card")
        .absolute(220.0, 140.0)
        .size(80.0, 60.0)
        .hit();
    if s.card_clickable {
        card = card.on_press(|s: &mut Scene| s.card_presses += 1);
    }

    let modal = s.modal_open.then(|| {
        el().key("modal")
            .absolute(40.0, 40.0)
            .size(220.0, 120.0)
            .modal(true, |s: &mut Scene| s.modal_open = false)
    });

    col()
        .size(300.0, 200.0)
        .child(toolbar)
        .child(viewport)
        .child(card)
        .children(modal)
}

fn make_rt(scene: Scene) -> Runtime<Scene> {
    let mut rt = Runtime::new(scene, fixture_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| {
        if ev.key == Key::Escape {
            s.app_escapes += 1;
            return true;
        }
        false
    });
    rt.frame(0.0);
    rt
}

#[test]
fn state_machine_transitions_and_effect_cancellation() {
    let mut st = LockState::Off;
    // Pressed before Asked(true) does nothing.
    assert!(step(&mut st, LockEvent::Pressed).is_empty());
    assert_eq!(st, LockState::Off);
    assert!(step(&mut st, LockEvent::Released).is_empty());
    assert_eq!(st, LockState::Off);

    // Off -> Ready without host effect.
    assert!(step(&mut st, LockEvent::Asked(true)).is_empty());
    assert_eq!(st, LockState::Ready);
    // Repeated Asked(true) is a no-op.
    assert!(step(&mut st, LockEvent::Asked(true)).is_empty());
    assert_eq!(st, LockState::Ready);

    // Ready + Pressed -> Locked + Grab.
    assert_eq!(step(&mut st, LockEvent::Pressed).into_iter().collect::<Vec<_>>(), vec![LockEffect::Grab]);
    assert_eq!(st, LockState::Locked);
    // Second press while Locked gives nothing.
    assert!(step(&mut st, LockEvent::Pressed).is_empty());
    assert_eq!(st, LockState::Locked);

    // Locked + Released -> Ready + Ungrab.
    assert_eq!(step(&mut st, LockEvent::Released).into_iter().collect::<Vec<_>>(), vec![LockEffect::Ungrab]);
    assert_eq!(st, LockState::Ready);
    // Releasing again while Ready gives nothing.
    assert!(step(&mut st, LockEvent::Released).is_empty());

    // Ready -> Locked -> Off on Asked(false).
    assert_eq!(step(&mut st, LockEvent::Pressed).into_iter().collect::<Vec<_>>(), vec![LockEffect::Grab]);
    assert_eq!(step(&mut st, LockEvent::Asked(false)).into_iter().collect::<Vec<_>>(), vec![LockEffect::Ungrab]);
    assert_eq!(st, LockState::Off);

    // PointerLock cancels opposite owed effects before host sync.
    let mut lock = PointerLock::new();
    assert!(!lock.step(LockEvent::Asked(true)));
    assert!(lock.step(LockEvent::Pressed));
    assert!(lock.is_locked());
    // Released before take_effect cancels Grab.
    assert!(lock.step(LockEvent::Released));
    assert!(!lock.is_locked());
    assert_eq!(lock.state(), LockState::Ready);
    assert_eq!(lock.take_effect(), None);

    // Re-grabbing before host takes an Ungrab cancels Ungrab.
    assert!(lock.step(LockEvent::Pressed));
    assert_eq!(lock.take_effect(), Some(LockEffect::Grab));
    assert!(lock.step(LockEvent::Released));
    assert!(lock.step(LockEvent::Pressed));
    assert_eq!(lock.take_effect(), None);
}

#[test]
fn capture_gating_on_press_and_motion_delivery() {
    let mut rt = make_rt(Scene::playing());
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);

    // Motion before capture is dropped.
    rt.pointer_motion(20.0, -5.0);
    rt.frame(0.0);
    assert!(rt.state.motions.is_empty());

    // Press on toolbar does not capture.
    rt.pointer_down(150.0, 20.0);
    rt.pointer_up(150.0, 20.0);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);

    // Press on viewport captures, fires on_press once, and owes Grab once.
    rt.pointer_down(150.0, 100.0);
    assert!(rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    assert_eq!(rt.state.viewport_presses, 1);
    assert!(rt.state.locked_seen.get(), "Cx::pointer_locked updated to true");
    assert!(rt.state.memo_builds.get() >= 2, "memo reading Cx::pointer_locked rebuilt");

    // Zero and non-finite deltas are dropped.
    rt.pointer_motion(0.0, 0.0);
    rt.pointer_motion(f32::NAN, 1.0);
    rt.pointer_motion(1.0, f32::INFINITY);
    rt.frame(0.0);
    assert!(rt.state.motions.is_empty());

    // Motion (12, -3) then (0.5, 0) coalesces to [12.5, -3].
    rt.pointer_motion(12.0, -3.0);
    rt.pointer_motion(0.5, 0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.motions, vec![[12.5, -3.0]]);
}

#[test]
fn hud_button_and_covering_card_do_not_capture() {
    let mut rt = make_rt(Scene::playing());

    // Click on HUD button inside viewport: HUD fires, viewport does not capture.
    rt.pointer_down(30.0, 60.0);
    rt.pointer_up(30.0, 60.0);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.state.hud_presses, 1);
    assert_eq!(rt.state.viewport_presses, 0);

    // Click on clickable covering card: card fires, viewport does not capture.
    rt.pointer_down(250.0, 170.0);
    rt.pointer_up(250.0, 170.0);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.state.card_presses, 1);
    assert_eq!(rt.state.viewport_presses, 0);

    // Even a non-clickable .hit() card covering the corner blocks capture.
    rt.update(|s, _| s.card_clickable = false);
    rt.frame(0.0);
    rt.pointer_down(250.0, 170.0);
    rt.pointer_up(250.0, 170.0);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());
}

#[test]
fn pointer_is_frozen_while_locked_and_buttons_and_wheel_land_on_viewport() {
    let mut rt = make_rt(Scene::playing());
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    assert_eq!(rt.pointer(), Some((150.0, 100.0)));

    let moves_before = rt.state.viewport_moves;
    let hovers_before = rt.state.toolbar_hovers;

    // pointer_move, observe_pointer and pointer_leave are ignored while locked.
    assert!(!rt.observe_pointer(20.0, 10.0));
    rt.pointer_move(20.0, 10.0);
    rt.pointer_leave();
    rt.frame(0.0);
    assert_eq!(rt.pointer(), Some((150.0, 100.0)));
    assert_eq!(rt.state.viewport_moves, moves_before);
    assert_eq!(rt.state.toolbar_hovers, hovers_before);

    // Primary, secondary, middle clicks and wheel at coordinates over the toolbar/HUD/card
    // all land at the frozen press point on the viewport.
    rt.pointer_down(20.0, 10.0);
    rt.pointer_up(250.0, 170.0);
    rt.secondary_down(20.0, 10.0);
    rt.secondary_up(250.0, 170.0);
    rt.button_down(20.0, 10.0, PointerButton::Middle);
    rt.button_up(250.0, 170.0, PointerButton::Middle);
    rt.wheel_xy(20.0, 10.0, 0.0, -24.0);
    rt.frame(0.0);
    assert_eq!(rt.state.viewport_presses, 2);
    assert_eq!(rt.state.viewport_secondary, 1);
    assert_eq!(rt.state.viewport_middle, 1);
    assert_eq!(rt.state.viewport_wheels, 1);
    assert_eq!(rt.state.hud_presses, 0);
    assert_eq!(rt.state.card_presses, 0);

    // After focus loss, pointer_move works again.
    rt.window_focus(false);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));
    rt.pointer_move(20.0, 10.0);
    assert_eq!(rt.pointer(), Some((20.0, 10.0)));
    rt.frame(0.0);
    assert!(rt.state.toolbar_hovers > hovers_before);
}

#[test]
fn eight_motions_before_a_frame_coalesce_to_one_queued_action() {
    let mut rt = make_rt(Scene::playing());
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);

    for k in 1..=8 {
        rt.pointer_motion(k as f32, -(k as f32) * 0.5);
    }
    let queued = rt
        .core()
        .frame
        .queue
        .iter()
        .filter(|a| matches!(a, Action::PointerLockMotion(..)))
        .count();
    assert_eq!(queued, 1, "eight consecutive motions coalesce into one action");

    rt.frame(0.0);
    assert_eq!(rt.state.motions, vec![[36.0, -18.0]]);
}

#[test]
fn escape_releases_and_consumes_on_logical_and_physical_paths_with_drag_precedence() {
    let mut rt = make_rt(Scene::playing());
    // Capture without taking the Grab effect yet.
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    assert!(rt.pointer_locked());

    // First logical Escape releases lock and is consumed; pending Grab cancels so no effect owed.
    rt.key(KeyEvent::from(Key::Escape));
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);
    rt.pointer_motion(10.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.app_escapes, 0);
    assert!(rt.state.motions.is_empty(), "motion after Escape is dropped");
    assert!(!rt.state.locked_seen.get(), "Cx::pointer_locked updated to false");

    // Second Escape reaches the app.
    rt.key(KeyEvent::from(Key::Escape));
    rt.frame(0.0);
    assert_eq!(rt.state.app_escapes, 1);

    // Physical Escape path releases and consumes too.
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    assert!(rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    let mods = Mods::default();
    rt.physical_key_input(
        PhysicalInput::Key { key: PhysicalKeyCode::Escape, mods, pressed: true, repeat: false, text: None, consumed_mods: Default::default() },
        KeyEvent::from(Key::Escape),
        None,
    );
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));
    rt.frame(0.0);
    assert_eq!(rt.state.app_escapes, 1, "physical Escape was consumed");

    // Drag cancel takes precedence over pointer lock on Escape.
    rt.update(|s, _| s.viewport_drag = true);
    rt.frame(0.0);
    rt.pointer_down(150.0, 100.0);
    assert!(rt.pointer_locked());
    assert!(rt.pressing());
    // First Escape cancels the drag, keeping pointer lock active.
    rt.key(KeyEvent::from(Key::Escape));
    assert!(!rt.pressing());
    assert!(rt.pointer_locked());
    rt.frame(0.0);
    assert_eq!(rt.state.drag_phases.last(), Some(&(Phase::Up, true)));
    assert_eq!(rt.state.app_escapes, 1);
    // Second Escape releases pointer lock.
    rt.key(KeyEvent::from(Key::Escape));
    assert!(!rt.pointer_locked());
    rt.frame(0.0);
    assert_eq!(rt.state.app_escapes, 1);
    // Third Escape reaches the app.
    rt.key(KeyEvent::from(Key::Escape));
    rt.frame(0.0);
    assert_eq!(rt.state.app_escapes, 2);
}

#[test]
fn focus_loss_unmount_modal_disabled_and_inert_release_lock() {
    let mut rt = make_rt(Scene::playing());
    rt.window_focus(true);
    rt.frame(0.0);

    // 1. Focus loss releases with Ungrab; refocus does not re-capture.
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    rt.window_focus(false);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));
    rt.window_focus(true);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());

    // 2. Stopping asking (playing = false) releases with Ungrab and prevents re-capture.
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    rt.update(|s, _| s.playing = false);
    let wants_frame = rt.frame(0.0);
    assert!(!rt.pointer_locked());
    assert!(wants_frame, "flipping pointer_locked off at end of frame requests one update frame");
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));
    rt.frame(0.0);
    assert!(!rt.state.locked_seen.get());
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    assert!(!rt.pointer_locked());

    // 3. Modal covering viewport releases lock.
    rt.update(|s, _| s.playing = true);
    rt.frame(0.0);
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    rt.update(|s, _| s.modal_open = true);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));

    // 4. Disabling or marking inert releases lock and blocks capture.
    rt.update(|s, _| s.modal_open = false);
    rt.frame(0.0);
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    rt.update(|s, _| s.viewport_disabled = true);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));
    rt.pointer_down(150.0, 100.0);
    assert!(!rt.pointer_locked());

    rt.update(|s, _| { s.viewport_disabled = false; s.viewport_inert = true; });
    rt.frame(0.0);
    rt.pointer_down(150.0, 100.0);
    assert!(!rt.pointer_locked());
}

#[test]
fn live_only_pointer_lock_skips_view_rebuilds() {
    let yaw = Live::new(0.0f32);
    let mut scene = Scene::playing();
    scene.live_only_motion = true;
    scene.live_yaw = Some(yaw.clone());
    let mut rt = make_rt(scene);
    rt.check_live_only(false);

    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    let builds_before = rt.state.view_builds.get();

    rt.pointer_motion(14.0, -2.0);
    rt.frame(0.0);
    rt.pointer_motion(6.0, 1.0);
    rt.frame(0.0);

    assert_eq!(yaw.get(), 20.0);
    assert_eq!(rt.state.view_builds.get(), builds_before, "live_only skips view rebuilds");
}

#[test]
fn unsupported_host_never_captures_and_leaves_input_unchanged() {
    let mut rt = make_rt(Scene::playing());
    rt.set_pointer_lock_supported(false);

    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.state.viewport_presses, 1);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);

    // Pointer move and Escape behave normally.
    rt.pointer_move(20.0, 10.0);
    assert_eq!(rt.pointer(), Some((20.0, 10.0)));
    rt.key(KeyEvent::from(Key::Escape));
    rt.frame(0.0);
    assert_eq!(rt.state.app_escapes, 1);
}

#[test]
fn script_pointer_motion_parses_and_dispatches() {
    let json = serde_json::json!([
        {"t": 0.1, "type": "pointer-motion", "dx": 40, "dy": -2.5}
    ]);
    let events = load_script(&json);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].kind, Kind::PointerMotion(dx, dy) if dx == 40.0 && dy == -2.5));
    assert!(matches!(
        events[0].public().event,
        ScriptEvent::PointerMotion { dx, dy } if dx == 40.0 && dy == -2.5
    ));

    let mut rt = make_rt(Scene::playing());
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    let (core, mut app) = rt.split_mut();
    assert!(script_input(core, &mut app, &events[0].kind, (0.0, 0.0)));
    rt.frame(0.0);
    assert_eq!(rt.state.motions, vec![[40.0, -2.5]]);
}

#[test]
fn apps_without_pointer_lock_keep_flag_false_and_role_unchanged() {
    let rt = make_rt(Scene::default());
    assert!(!rt.core().tree.has_pointer_lock);
    assert!(!rt.pointer_locked());
    // An element with pointer_lock does not implicitly become a button in the a11y tree.
    let locked_rt = make_rt(Scene::playing());
    assert!(locked_rt.core().tree.has_pointer_lock);
    let vp = locked_rt
        .core()
        .tree
        .nodes
        .iter()
        .find(|n| n.key.as_deref() == Some("viewport"))
        .expect("viewport node");
    assert_eq!(vp.role, "button", "viewport has on_press so role is button");
    let mut plain_lock = Runtime::new(
        (),
        |_: &(), _| el().key("cam").size(100.0, 100.0).pointer_lock(|_: &mut (), _, _| {}),
        (100.0, 100.0),
        Env::new(true, 1.0),
    );
    plain_lock.frame(0.0);
    let cam = plain_lock
        .core()
        .tree
        .nodes
        .iter()
        .find(|n| n.key.as_deref() == Some("cam"))
        .expect("cam node");
    assert_eq!(cam.role, "group", "pointer_lock alone leaves role unchanged");
}

#[test]
fn async_policy_requests_on_press_and_waits_for_platform_grant_and_exit() {
    let mut rt = make_rt(Scene::playing());
    rt.core_mut().set_pointer_lock_policy(super::LockPolicy::Async);
    assert!(!rt.pointer_locked());

    // Primary press enters Pending (owes Grab) without flipping pointer_locked() yet.
    rt.pointer_down(150.0, 100.0);
    assert!(!rt.pointer_locked(), "Cx::pointer_locked stays false while awaiting browser grant");
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Pending);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);
    rt.pointer_up(150.0, 100.0);
    rt.frame(0.0);
    assert!(!rt.state.locked_seen.get());

    // Motion before grant is dropped.
    rt.pointer_motion(10.0, 5.0);
    rt.frame(0.0);
    assert!(rt.state.motions.is_empty());

    // Browser confirms pointer lock via pointerlockchange -> Locked (no extra host effect).
    assert!(rt.core_mut().pointer_lock_granted());
    assert!(rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);
    rt.frame(0.0);
    assert!(rt.state.locked_seen.get(), "Cx::pointer_locked updates to true on grant");

    // Relative motion from movementX/movementY coalesces and delivers cleanly.
    rt.pointer_motion(30.0, 15.0);
    rt.pointer_motion(15.0, -10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.motions, vec![[45.0, 5.0]]);

    // Browser exits lock on Escape or tab switch (pointerlockchange -> null): back to Ready,
    // Cx::pointer_locked flips false, no redundant Ungrab owed.
    assert!(rt.core_mut().pointer_lock_lost());
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Ready);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);
    rt.frame(0.0);
    assert!(!rt.state.locked_seen.get());
    rt.pointer_motion(9.0, 9.0);
    rt.frame(0.0);
    assert_eq!(rt.state.motions.len(), 1, "motion after browser exit is dropped");
}

#[test]
fn async_policy_refusal_and_unmount_and_stale_grant_leave_no_stuck_state() {
    let mut rt = make_rt(Scene::playing());
    rt.core_mut().set_pointer_lock_policy(super::LockPolicy::Async);

    // 1. Browser refuses (cooldown / no gesture / unfocused document): Pending -> Ready.
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    assert!(!rt.core_mut().pointer_lock_lost(), "is_locked stayed false on refusal");
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Ready);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);

    // Next click requests again cleanly (not stuck in Pending).
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Pending);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));

    // 2. Focus loss while Pending returns to Ready and owes Ungrab; stale late grant owes Ungrab.
    rt.window_focus(false);
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Ready);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));
    assert!(!rt.core_mut().pointer_lock_granted());
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Ready);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));

    // 3. Unmount while Pending before host synced cancels Grab cleanly.
    rt.window_focus(true);
    rt.frame(0.0);
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Pending);
    rt.update(|s, _| s.playing = false);
    rt.frame(0.0);
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Off);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);

    // 4. Unmount while Locked owes Ungrab so the browser calls exitPointerLock().
    rt.update(|s, _| s.playing = true);
    rt.frame(0.0);
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    assert!(rt.core_mut().pointer_lock_granted());
    assert!(rt.pointer_locked());
    rt.update(|s, _| s.playing = false);
    rt.frame(0.0);
    assert!(!rt.pointer_locked());
    assert_eq!(rt.core().pointer.pointer_lock.state(), LockState::Off);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Ungrab));
}

#[test]
fn async_policy_duplicate_grant_from_in_place_option_upgrade_is_idempotent() {
    let mut rt = make_rt(Scene::playing());
    rt.core_mut().set_pointer_lock_policy(super::LockPolicy::Async);
    rt.pointer_down(150.0, 100.0);
    rt.pointer_up(150.0, 100.0);
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), Some(LockEffect::Grab));
    assert!(rt.core_mut().pointer_lock_granted());
    assert!(rt.pointer_locked());
    // A second pointerlockchange from upgrading options in place (W3C Pointer Lock 2.0 step 6.3.2)
    // leaves the lock active with no view invalidation and no host effect.
    assert!(!rt.core_mut().pointer_lock_granted());
    assert!(rt.pointer_locked());
    assert_eq!(rt.core_mut().take_pointer_lock_effect(), None);
}
