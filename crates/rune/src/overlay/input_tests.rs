use std::sync::Arc;

use super::should_recreate_layer;
use crate::overlay::{OverlayHandle, Requests, ScreenEdge, Shared};
use crate::{Align, DisplayTarget, NativeDisplayId, OverlayPlacement};

fn requests_after(f: impl FnOnce(&OverlayHandle<()>)) -> Requests {
    let shared = Arc::new(Shared::<()>::default());
    let handle = OverlayHandle { shared: shared.clone() };
    f(&handle);
    std::mem::take(&mut *shared.core.requests.lock().unwrap())
}

#[test]
fn wayland_layer_recreates_only_when_target_display_changes() {
    let primary = DisplayTarget::Primary;
    let out_a = DisplayTarget::Native(NativeDisplayId::Wayland(7));
    let out_b = DisplayTarget::Native(NativeDisplayId::Wayland(8));

    // Placement updates (Rect, Edge, FullScreen, and None) stay on the live layer surface.
    for placement in [
        Some(OverlayPlacement::Rect { x: 24.0, y: 48.0, width: 240.0, height: 64.0 }),
        Some(OverlayPlacement::Edge { edge: ScreenEdge::Bottom, margin: 16.0, align: Align::End, inset: 12.0 }),
        Some(OverlayPlacement::FullScreen),
        None,
    ] {
        let req = requests_after(|h| h.set_placement(placement));
        assert!(!should_recreate_layer(&primary, &req, true), "placement {placement:?} must update in place");
        assert!(!should_recreate_layer(&out_a, &req, true), "placement {placement:?} on a targeted output must update in place");
    }

    // Re-specifying the same target display (with or without a new placement) also updates in place.
    let same = requests_after(|h| {
        h.set_display(out_a.clone());
        h.set_placement(Some(OverlayPlacement::Rect { x: 10.0, y: 10.0, width: 100.0, height: 40.0 }));
    });
    assert!(!should_recreate_layer(&out_a, &same, true), "same display must not recreate the layer");

    // Switching to another display must recreate the layer because wl_output is fixed at creation.
    let to_a = requests_after(|h| h.set_display(out_a.clone()));
    assert!(should_recreate_layer(&primary, &to_a, true));
    assert!(!should_recreate_layer(&primary, &to_a, false), "non-layer backings never recreate a layer");

    let to_b_with_placement = requests_after(|h| {
        h.set_display(out_b.clone());
        h.set_placement(Some(OverlayPlacement::FullScreen));
    });
    assert!(should_recreate_layer(&out_a, &to_b_with_placement, true));

    let back_to_primary = requests_after(|h| h.set_display(primary.clone()));
    assert!(should_recreate_layer(&out_a, &back_to_primary, true));
}

#[test]
fn awaiting_layer_frame_is_frameless_steps_due_timers_and_pauses_ambient_loops() {
    use std::time::Duration;
    use super::{Overlay, OverlayOptions, OverlaySpec, Presence};
    use crate::tooling::AppSpec;
    use crate::{Cx, El, Tx};

    #[derive(Default)]
    struct State {
        polls: u32,
        clicks: u32,
        rearm: bool,
        ambient: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = crate::el()
            .size(100.0, 40.0)
            .hit()
            .on_press(|s: &mut State| s.clicks += 1);
        if s.ambient {
            root = root.child(
                crate::el()
                    .key("pulse")
                    .size(8.0, 8.0)
                    .fill(crate::Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 })
                    .cycle(crate::Cycle {
                        period: 1.0,
                        phase: 0.0,
                        stops: &[(0.0, 0.2), (0.5, 1.0), (1.0, 0.2)],
                        still: 0.2,
                        fps: 20.0,
                    }),
            );
        }
        root
    }
    fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
        s.polls += 1;
        if s.rearm {
            tx.wake_after(Duration::from_millis(100));
        }
        false
    }

    let app = AppSpec {
        poll: Some(poll),
        ..AppSpec::new("layer-frame-wait", (100.0, 40.0), State::default, view)
    };
    let spec = OverlaySpec::new("layer-frame-wait", app.size, view).options(OverlayOptions::default());
    let (mut overlay, _) = Overlay::new(spec, State { polls: 0, clicks: 0, rearm: true, ambient: false }, &app);
    overlay.rt.set_system_reduced_motion(false);
    overlay.rt.set_viewport(app.size, 1.0);
    let start = crate::time::now();
    overlay.rt.wall_start = Some(start);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.set_live_visible(true);
    overlay.rt.frame(0.0);
    overlay.dirty = false;
    overlay.idle = true;

    // Without a pending layer frame callback, a shown overlay is not frameless.
    assert!(!overlay.frameless());

    // With a pending layer frame callback, it becomes frameless so updates, timers and drag ends
    // still run without blocking on a compositor that stopped painting.
    overlay.core.layer_frame_pending = true;
    assert!(overlay.frameless());

    // An update polls without rebuilding the view or laying out (polls_only), and marks dirty for
    // when the callback arrives.
    let (polls_before, commits_before) = (overlay.rt.state.polls, overlay.rt.stats.commits);
    overlay.apply_wake(false, vec![Box::new(|_: &mut State, tx: &mut Tx| {
        tx.wake_after(Duration::from_millis(100));
        true
    })]);
    assert!(overlay.dirty);
    assert_eq!(overlay.rt.state.polls, polls_before + 1, "update while awaiting frame callback runs poll");
    assert_eq!(overlay.rt.stats.commits, commits_before, "update without pending input does not rebuild the view");

    // Due app timers still fire through wake_due and advance the next deadline into the future
    // instead of leaving an overdue deadline that would spin the event loop.
    let due = overlay.wake_deadline().expect("app timer deadline stays scheduled while awaiting callback");
    let elapsed = due.saturating_duration_since(start);
    let now = crate::time::now();
    overlay.rt.wall_start = Some(now - elapsed - Duration::from_millis(1));
    let polls_before = overlay.rt.state.polls;
    overlay.wake_due(now);
    assert_eq!(overlay.rt.state.polls, polls_before + 1, "due timer steps while awaiting layer frame callback");
    let next_due = overlay.wake_deadline().expect("re-armed timer stays scheduled");
    assert!(next_due > now, "next deadline must move into the future so the loop sleeps");

    // Pending input handlers run immediately via commit_at rather than waiting for the compositor.
    overlay.rt.pointer_down(10.0, 10.0);
    overlay.rt.pointer_up(10.0, 10.0);
    assert!(overlay.rt.has_pending_input());
    overlay.advance();
    assert!(!overlay.rt.has_pending_input(), "pending input drains while awaiting layer frame callback");
    assert_eq!(overlay.rt.state.clicks, 1);

    // Clear app timers and mount a real ambient Cycle loop: while awaiting the frame callback,
    // wake_deadline omits motion_wake_deadline so an unpainted overlay never spins on an overdue
    // visual loop; once the callback arrives, wake_deadline exposes it immediately.
    overlay.rt.update(|s, tx| {
        tx.instant();
        s.rearm = false;
        s.ambient = true;
    });
    overlay.rt.frame(0.0);
    let now = crate::time::now();
    overlay.rt.wall_start = Some(now - Duration::from_secs(1));
    overlay.wake_due(now);
    assert!(overlay.rt.core().motion_wake_deadline().is_some(), "ambient loop armed a motion wake");
    assert_eq!(overlay.wake_deadline(), None, "ambient motion loop pauses while awaiting layer frame callback");
    overlay.core.layer_frame_pending = false;
    assert!(!overlay.frameless());
    assert!(overlay.wake_deadline().is_some(), "ambient motion loop resumes as soon as wl_callback.done arrives");
}

#[derive(Default)]
struct DragBarState {
    button_clicks: u32,
    no_drag_clicks: u32,
    modal_open: bool,
}

fn drag_bar_overlay(window_backed: bool) -> super::Overlay<DragBarState> {
    use super::{Overlay, OverlayOptions, OverlaySpec, Presence};
    use crate::tooling::AppSpec;
    use crate::{Cx, El, OverlayInput};

    fn view(s: &DragBarState, _: &mut Cx<DragBarState>) -> El {
        let mut root = crate::col().size(300.0, 100.0)
            .child(
                crate::row().key("header").size(300.0, 40.0).hit().window_drag()
                    .child(
                        crate::el().key("btn").size(60.0, 40.0).hit()
                            .on_press(|s: &mut DragBarState| s.button_clicks += 1),
                    )
                    .child(
                        crate::el().key("nodrag").size(60.0, 40.0).hit().no_window_drag()
                            .on_press(|s: &mut DragBarState| s.no_drag_clicks += 1),
                    )
                    .child(crate::el().key("nodrag-bare").size(40.0, 40.0).hit().no_window_drag()),
            )
            .child(
                crate::el().key("inert-drag").size(300.0, 30.0).hit().window_drag().inert(),
            )
            .child(crate::el().key("plain").size(300.0, 30.0).hit());
        if s.modal_open {
            root = root.child(
                crate::el().key("modal").absolute(0.0, 0.0).size(300.0, 100.0).hit()
                    .modal(false, |_: &mut DragBarState| {}),
            );
        }
        root
    }

    let app = AppSpec::new("drag-bar", (300.0, 100.0), DragBarState::default, view);
    let spec = OverlaySpec::new("drag-bar", app.size, view)
        .options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
    let (mut overlay, _) = Overlay::new(spec, DragBarState::default(), &app);
    overlay.core.test_window_backing = window_backed;
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.set_viewport(app.size, 1.0);
    overlay.rt.frame(0.0);
    overlay
}

#[test]
fn window_drag_in_window_backed_overlay_starts_drag_and_keeps_children_interactive() {
    use super::PositionOwner;

    let mut overlay = drag_bar_overlay(true);

    // Bare .window_drag() header [160..300, 0..40] starts a native window drag and drains the action.
    overlay.left_button(true, Some((220.0, 20.0)));
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    {
        assert_eq!(overlay.core.test_window_drags, 0, "Linux defers mouse drag until motion crosses threshold");
        overlay.cursor_moved(230.0, 20.0);
    }
    assert_eq!(overlay.core.test_window_drags, 1, "bare .window_drag() starts a window drag");
    assert_eq!(overlay.rt.take_titlebar_action(), None, "TitlebarAction::Drag must be consumed");
    assert!(!overlay.rt.pointer_captured(), "window drag must not leave pointer capture stuck");
    assert_eq!(overlay.core.position_owner, PositionOwner::Dragging);
    overlay.left_button(false, Some((220.0, 20.0)));
    assert_eq!(overlay.core.position_owner, PositionOwner::Managed, "release returns to Managed");

    // Interactive child button [0..60, 0..40] inside .window_drag() keeps its press.
    overlay.left_button(true, Some((30.0, 20.0)));
    overlay.left_button(false, Some((30.0, 20.0)));
    overlay.rt.frame(0.0);
    assert_eq!(overlay.rt.state.button_clicks, 1);
    assert_eq!(overlay.core.test_window_drags, 1, "button inside .window_drag() must not drag");

    // .no_window_drag() with handler [60..120, 0..40] keeps its press and never drags.
    overlay.left_button(true, Some((90.0, 20.0)));
    overlay.left_button(false, Some((90.0, 20.0)));
    overlay.rt.frame(0.0);
    assert_eq!(overlay.rt.state.no_drag_clicks, 1);
    assert_eq!(overlay.core.test_window_drags, 1);

    // Bare .no_window_drag() [120..160, 0..40] suppresses window drag.
    overlay.left_button(true, Some((140.0, 20.0)));
    overlay.left_button(false, Some((140.0, 20.0)));
    assert_eq!(overlay.core.test_window_drags, 1, "bare .no_window_drag() must not drag");

    // Inert .window_drag() [0..300, 40..70] does not drag.
    overlay.left_button(true, Some((200.0, 55.0)));
    overlay.left_button(false, Some((200.0, 55.0)));
    assert_eq!(overlay.core.test_window_drags, 1, "inert .window_drag() must not drag");

    // Active modal blocks .window_drag() underneath.
    overlay.rt.update(|s, _| s.modal_open = true);
    overlay.rt.frame(0.0);
    overlay.left_button(true, Some((220.0, 20.0)));
    overlay.left_button(false, Some((220.0, 20.0)));
    assert_eq!(overlay.core.test_window_drags, 1, "active modal must block overlay window drag");
}

#[test]
fn window_drag_double_click_is_drained_without_starting_a_second_drag() {
    let mut overlay = drag_bar_overlay(true);
    let expected_first = u32::from(!cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))));

    // First press starts the drag immediately off Linux, or stays deferred on Linux.
    overlay.left_button(true, Some((220.0, 20.0)));
    assert_eq!(overlay.core.test_window_drags, expected_first);
    overlay.left_button(false, Some((220.0, 20.0)));

    // Second press within double-click interval emits TitlebarAction::DoubleClick, which overlays
    // drain and ignore (no maximize/minimize and no second drag).
    overlay.left_button(true, Some((220.0, 20.0)));
    assert_eq!(overlay.rt.take_titlebar_action(), None, "TitlebarAction::DoubleClick must be drained");
    assert_eq!(overlay.core.test_window_drags, expected_first, "double-click on overlay drag region must not start a drag");
    overlay.left_button(false, Some((220.0, 20.0)));
}

#[test]
fn deferred_overlay_window_drag_starts_only_after_crossing_threshold() {
    use super::PositionOwner;

    let mut overlay = drag_bar_overlay(true);
    overlay.rt.core_mut().set_defer_titlebar_drag(true);

    // Stationary click starts no drag.
    overlay.left_button(true, Some((220.0, 20.0)));
    assert_eq!(overlay.core.test_window_drags, 0);
    overlay.cursor_moved(225.0, 20.0);
    assert_eq!(overlay.core.test_window_drags, 0);
    assert_eq!(overlay.core.position_owner, PositionOwner::Managed);

    // Crossing the 8 pt drag threshold starts the native move from cursor_moved.
    overlay.cursor_moved(228.0, 20.0);
    assert_eq!(overlay.core.test_window_drags, 1);
    assert_eq!(overlay.rt.take_titlebar_action(), None);
    assert_eq!(overlay.core.position_owner, PositionOwner::Dragging);
    overlay.left_button(false, Some((228.0, 20.0)));
    assert_eq!(overlay.core.position_owner, PositionOwner::Managed);
}

#[test]
fn window_drag_clears_capture_disarms_drag_out_and_applies_pending_input() {
    use super::{Overlay, OverlayOptions, OverlaySpec, Presence};
    use crate::tooling::AppSpec;
    use crate::{Cx, El, OverlayInput};

    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::row().size(200.0, 40.0)
            .child(
                crate::el().key("drag-out").size(100.0, 40.0).hit()
                    .drag_out(|_: &()| Some(crate::DragItem::new().text("item"))),
            )
            .child(crate::el().key("win-drag").size(100.0, 40.0).hit().window_drag())
    }
    let app = AppSpec::new("drag-clear", (200.0, 40.0), || (), view);
    let spec = OverlaySpec::new("drag-clear", app.size, view)
        .options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
    let (mut overlay, _) = Overlay::new(spec, (), &app);
    overlay.core.test_window_backing = true;
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.set_viewport(app.size, 1.0);
    overlay.rt.frame(0.0);

    // Queue a deferred input-mode switch and drag the bare window-drag region: pointer capture
    // is not held, no drag-out request remains, and deferred input applies cleanly.
    overlay.core.pending_input = Some(OverlayInput::Regions);
    overlay.left_button(true, Some((150.0, 20.0)));
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    overlay.cursor_moved(160.0, 20.0);
    assert_eq!(overlay.core.test_window_drags, 1);
    assert!(!overlay.rt.pointer_captured());
    assert!(overlay.rt.core_mut().take_drag_out().is_none(), "drag-out must not remain armed");
    assert_eq!(overlay.core.options.input, OverlayInput::Regions, "pending input change must apply after window drag starts");
}

#[test]
fn window_drag_without_window_backing_drains_action_without_starting_drag() {
    use super::PositionOwner;

    let mut overlay = drag_bar_overlay(false);
    overlay.left_button(true, Some((220.0, 20.0)));
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    overlay.cursor_moved(230.0, 20.0);
    assert_eq!(overlay.core.test_window_drags, 0, "non-window backing (e.g. Wayland layer-shell) ignores drag");
    assert_eq!(overlay.rt.take_titlebar_action(), None, "TitlebarAction::Drag must still be drained");
    assert_eq!(overlay.core.position_owner, PositionOwner::Managed);
    assert!(!overlay.rt.pointer_captured());
}

#[test]
fn touch_drag_past_slop_starts_overlay_window_drag_while_tap_and_button_do_not() {
    use super::PositionOwner;
    use winit::dpi::PhysicalPosition;
    use winit::event::{DeviceId, Touch, TouchPhase};

    let mut overlay = drag_bar_overlay(true);
    let touch = |phase: TouchPhase, x: f64, y: f64, id: u64| Touch {
        device_id: DeviceId::dummy(),
        phase,
        location: PhysicalPosition::new(x, y),
        force: None,
        id,
    };

    // Tap within slop on bare .window_drag() does not start a window drag.
    overlay.touch_event(&touch(TouchPhase::Started, 220.0, 20.0, 1));
    overlay.touch_event(&touch(TouchPhase::Moved, 224.0, 20.0, 1));
    overlay.touch_event(&touch(TouchPhase::Ended, 224.0, 20.0, 1));
    assert_eq!(overlay.core.test_window_drags, 0, "touch within slop must not drag");

    // Immediate second tap at the same spot emits TitlebarAction::DoubleClick, which overlays drain.
    overlay.touch_event(&touch(TouchPhase::Started, 220.0, 20.0, 2));
    assert_eq!(overlay.rt.take_titlebar_action(), None);
    overlay.touch_event(&touch(TouchPhase::Ended, 220.0, 20.0, 2));
    assert_eq!(overlay.core.test_window_drags, 0, "double-tap on overlay drag region must not drag");

    // Touch dragging past TOUCH_SLOP (8 pt) starts a window drag and consumes TitlebarAction::Drag.
    overlay.rt.frame(1.0);
    overlay.touch_event(&touch(TouchPhase::Started, 220.0, 20.0, 3));
    assert_eq!(overlay.core.test_window_drags, 0);
    overlay.touch_event(&touch(TouchPhase::Moved, 236.0, 20.0, 3));
    assert_eq!(overlay.core.test_window_drags, 1, "touch past slop must start window drag");
    assert_eq!(overlay.rt.take_titlebar_action(), None);
    assert_eq!(overlay.core.position_owner, PositionOwner::Dragging);
    overlay.touch_event(&touch(TouchPhase::Ended, 236.0, 20.0, 3));
    assert_eq!(overlay.core.position_owner, PositionOwner::Managed);

    // Touch dragging on an interactive button inside .window_drag() does not start a window drag.
    overlay.touch_event(&touch(TouchPhase::Started, 30.0, 20.0, 3));
    overlay.touch_event(&touch(TouchPhase::Moved, 48.0, 20.0, 3));
    overlay.touch_event(&touch(TouchPhase::Ended, 48.0, 20.0, 3));
    assert_eq!(overlay.core.test_window_drags, 1);
}

#[test]
fn position_owner_preserves_user_drag_until_explicit_placement_request() {
    use super::{PositionEvent, PositionOwner};

    let mut owner = PositionOwner::default();
    assert!(owner.is_managed());

    // Programmatic moves while Managed never steal ownership.
    owner.step(PositionEvent::WindowMoved);
    assert_eq!(owner, PositionOwner::Managed);

    // A zero-movement async drag releases back to Managed.
    owner.step(PositionEvent::DragStarted);
    assert_eq!(owner, PositionOwner::Dragging);
    assert!(!owner.is_managed());
    owner.step(PositionEvent::LeftReleased);
    assert_eq!(owner, PositionOwner::Managed);

    // An async drag that actually moves the window promotes to User and survives release.
    owner.step(PositionEvent::DragStarted);
    owner.step(PositionEvent::WindowMoved);
    assert_eq!(owner, PositionOwner::User);
    owner.step(PositionEvent::LeftReleased);
    assert_eq!(owner, PositionOwner::User);

    // Resize-only or disabling follow_pointer keeps the user's dragged position.
    let resize_req = requests_after(|h| h.resize((320.0, 80.0)));
    assert!(!owner.apply_requests(&resize_req), "resize alone must not snap back to initial placement");
    assert_eq!(owner, PositionOwner::User);

    let unfollow_req = requests_after(|h| h.set_follow_pointer(false));
    assert!(!owner.apply_requests(&unfollow_req));
    assert_eq!(owner, PositionOwner::User);

    // Explicit placement requests reclaim Managed and ask sync to re-place even at the same rect.
    for req in [
        requests_after(|h| h.set_placement(Some(OverlayPlacement::Rect { x: 10.0, y: 20.0, width: 200.0, height: 60.0 }))),
        requests_after(|h| h.set_placement(None)),
        requests_after(|h| h.set_display(DisplayTarget::Primary)),
        requests_after(|h| h.set_edge(ScreenEdge::Top)),
        requests_after(|h| h.place(ScreenEdge::Top, 40.0)),
        requests_after(|h| h.set_align(Align::Center, 0.0)),
    ] {
        owner.step(PositionEvent::DragMoved);
        assert_eq!(owner, PositionOwner::User);
        assert!(owner.apply_requests(&req), "explicit placement request must trigger re-placement");
        assert_eq!(owner, PositionOwner::Managed);
    }

    // Explicit refollow_pointer and set_follow_pointer(true) restore Managed ownership.
    owner.step(PositionEvent::DragMoved);
    let refollow_req = requests_after(|h| h.refollow_pointer());
    assert!(!owner.apply_requests(&refollow_req));
    assert_eq!(owner, PositionOwner::Managed);

    owner.step(PositionEvent::DragMoved);
    let follow_req = requests_after(|h| h.set_follow_pointer(true));
    assert!(!owner.apply_requests(&follow_req));
    assert_eq!(owner, PositionOwner::Managed);

    // Releasing the native window resets ownership so a recreated window is placed fresh.
    owner.step(PositionEvent::DragMoved);
    owner.step(PositionEvent::WindowReleased);
    assert_eq!(owner, PositionOwner::Managed);
}

#[test]
fn overlay_without_window_drag_behaves_unchanged() {
    use super::{Overlay, OverlayOptions, OverlaySpec, PositionOwner, Presence};
    use crate::tooling::AppSpec;
    use crate::{Cx, El, OverlayInput};

    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        crate::row().size(200.0, 40.0)
            .child(crate::el().key("btn").size(80.0, 40.0).hit().on_press(|n: &mut u32| *n += 1))
            .child(crate::el().key("bg").size(120.0, 40.0).hit())
    }
    let app = AppSpec::new("plain-overlay", (200.0, 40.0), || 0u32, view);
    let spec = OverlaySpec::new("plain-overlay", app.size, view)
        .options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
    let (mut overlay, _) = Overlay::new(spec, 0u32, &app);
    overlay.core.test_window_backing = true;
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.set_viewport(app.size, 1.0);
    overlay.rt.frame(0.0);

    overlay.left_button(true, Some((140.0, 20.0)));
    overlay.left_button(false, Some((140.0, 20.0)));
    assert_eq!(overlay.core.test_window_drags, 0);
    assert_eq!(overlay.core.position_owner, PositionOwner::Managed);

    overlay.left_button(true, Some((40.0, 20.0)));
    overlay.left_button(false, Some((40.0, 20.0)));
    overlay.rt.frame(0.0);
    assert_eq!(overlay.rt.state, 1);
    assert_eq!(overlay.core.test_window_drags, 0);
    assert_eq!(overlay.core.position_owner, PositionOwner::Managed);
}

#[test]
fn window_resize_in_overlay_is_drained_without_starting_a_drag() {
    use super::{Overlay, OverlayOptions, OverlaySpec, Presence};
    use crate::tooling::AppSpec;
    use crate::{Cx, El, OverlayInput};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::el().key("edge").size(100.0, 40.0).hit().window_resize(crate::ResizeEdge::East)
    }
    let app = AppSpec::new("resize-overlay", (100.0, 40.0), || (), view);
    let spec = OverlaySpec::new("resize-overlay", app.size, view)
        .options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
    let (mut overlay, _) = Overlay::new(spec, (), &app);
    overlay.core.test_window_backing = true;
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.set_viewport(app.size, 1.0);
    overlay.rt.frame(0.0);
    overlay.left_button(true, Some((50.0, 20.0)));
    overlay.left_button(false, Some((50.0, 20.0)));
    assert_eq!(overlay.rt.take_resize_action(), None, "Resize action must be drained");
    assert_eq!(overlay.rt.take_titlebar_action(), None, "Titlebar action stays empty");
    assert_eq!(overlay.core.test_window_drags, 0, "overlays ignore window_resize");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_window_drag_in_overlay_starts_drag_and_pointer_released_clears_native_capture() {
    let mut overlay = drag_bar_overlay(true);
    let pointer = crate::platform::macos::PointerState::new(crate::OverlayInput::All, false);
    pointer.record_click(true, 220.0, 20.0);
    assert!(pointer.captured(), "native click establishes capture");
    overlay.native_left(&pointer, true);
    assert_eq!(overlay.core.test_window_drags, 1);
    assert_eq!(overlay.rt.take_titlebar_action(), None);
    assert!(!overlay.rt.pointer_captured());
    // When live, begin_window_drag calls panel.pointer_released(), which calls pointer.released().
    pointer.released();
    assert!(!pointer.captured(), "native panel capture clears once OS drag takes mouse-up");
}

type TokenLog = Arc<std::sync::Mutex<Vec<(&'static str, Option<String>)>>>;

#[test]
fn activation_token_answers_ahead_of_hide_and_returns_none_when_hidden_or_non_layer() {
    use super::{Overlay, OverlayOptions, OverlaySpec, Presence};
    use crate::tooling::AppSpec;
    use crate::{Cx, El};

    fn view(_: &(), _: &mut Cx<()>) -> El { crate::el().size(40.0, 20.0) }
    let app = AppSpec::new("launcher", (40.0, 20.0), || (), view);
    let spec = OverlaySpec::new("launcher", app.size, view).options(OverlayOptions::default());
    let (mut overlay, handle) = Overlay::new(spec, (), &app);
    overlay.rt.frame(0.0);

    let log: TokenLog = Arc::default();
    let push = |tag: &'static str| {
        let log = log.clone();
        move |tok: Option<String>| log.lock().unwrap().push((tag, tok))
    };

    // Queuing activation_token and hide in one turn keeps both token replies in FIFO order so
    // sync() starts them before Visibility::Hide runs.
    let now = crate::time::now();
    overlay.presence = Presence::Shown { occluded: false };
    handle.activation_token(push("first"));
    handle.hide();
    handle.activation_token(push("second"));
    let mut req = std::mem::take(&mut *overlay.shared.core.requests.lock().unwrap());
    assert_eq!(req.tokens.len(), 2);
    assert_eq!(req.visible(), Some(false));
    assert!(overlay.presence.shown(), "sync starts tokens while the overlay is still shown");
    overlay.start_tokens(req.tokens.take(), now);
    overlay.hide();
    assert!(!overlay.presence.shown());
    assert_eq!(*log.lock().unwrap(), [("first", None), ("second", None)]);

    // Hidden overlay answers None once; fading-out overlay answers None once too.
    handle.activation_token(push("hidden"));
    overlay.start_queued_tokens(now);
    overlay.start_queued_tokens(now);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.hide_fading(std::time::Duration::from_millis(150));
    handle.activation_token(push("fading"));
    overlay.start_queued_tokens(now);
    assert_eq!(
        *log.lock().unwrap(),
        [("first", None), ("second", None), ("hidden", None), ("fading", None)]
    );
}

#[test]
fn activation_token_drains_with_none_on_hide_close_refusal_and_drop() {
    use super::{KeyPolicy, Overlay, OverlayOptions, OverlaySpec, Presence};
    use crate::tooling::AppSpec;
    use crate::{Cx, El};

    struct State {
        handle: Option<OverlayHandle<State>>,
        log: TokenLog,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El { crate::el().size(40.0, 20.0) }
    let log: TokenLog = Arc::default();
    let app = AppSpec::new("launcher", (40.0, 20.0), || (), |_, _| crate::el());
    let spec = OverlaySpec::new("launcher", app.size, view)
        .options(OverlayOptions { keys: KeyPolicy::OnDemand, ..Default::default() })
        .on_focus(|s: &mut State, focused, _| {
            if !focused && let Some(h) = &s.handle {
                let log = s.log.clone();
                h.activation_token(move |tok| log.lock().unwrap().push(("from_blur", tok)));
            }
        });
    let (mut overlay, handle) = Overlay::new(spec, State { handle: None, log: log.clone() }, &app);
    overlay.rt.state.handle = Some(handle.clone());
    overlay.rt.frame(0.0);

    let push = |tag: &'static str| {
        let log = log.clone();
        move |tok: Option<String>| log.lock().unwrap().push((tag, tok))
    };
    let now = crate::time::now();

    // Synchronous hide (e.g. Shell::Close) starts queued tokens before hiding and drains any
    // token requested inside on_focus(false) during hide_window, answering each exactly once.
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.window_focus(true);
    handle.activation_token(push("before_hide"));
    overlay.hide();
    overlay.start_queued_tokens(now);
    assert_eq!(*log.lock().unwrap(), [("before_hide", None), ("from_blur", None)]);
    assert_eq!(overlay.shared.core.requests.lock().unwrap().tokens.len(), 0);

    // Compositor layer close / surface refusal answers pending tokens with None once.
    overlay.presence = Presence::Shown { occluded: false };
    handle.activation_token(push("on_close"));
    overlay.layer_closed(Some("compositor closed layer surface"));
    overlay.start_queued_tokens(now);
    assert_eq!(*log.lock().unwrap(), [("before_hide", None), ("from_blur", None), ("on_close", None)]);

    // Dropping the overlay answers queued tokens with None (even when a reply re-entrantly asks
    // for another token), and calls after drop answer None immediately without hanging.
    let reentrant_handle = handle.clone();
    let reentrant_log = log.clone();
    handle.activation_token(move |tok| {
        reentrant_log.lock().unwrap().push(("on_drop_1", tok));
        let inner_log = reentrant_log.clone();
        reentrant_handle.activation_token(move |tok2| inner_log.lock().unwrap().push(("reentrant", tok2)));
    });
    handle.activation_token(push("on_drop_2"));
    drop(overlay);
    handle.activation_token(push("after_drop"));

    assert_eq!(
        *log.lock().unwrap(),
        [
            ("before_hide", None),
            ("from_blur", None),
            ("on_close", None),
            ("on_drop_1", None),
            ("reentrant", None),
            ("on_drop_2", None),
            ("after_drop", None),
        ]
    );
}

#[test]
fn activation_token_completes_async_after_same_turn_hide_times_out_and_cancels_on_drop() {
    use std::time::Duration;
    use super::{KeyPolicy, Overlay, OverlayOptions, OverlaySpec, Presence};
    use super::requests::TokenSource;
    use crate::platform::layer_shell_machine::activation::{TIMEOUT, TokenPoll};
    use crate::tooling::AppSpec;
    use crate::{Cx, El};

    struct State {
        handle: Option<OverlayHandle<State>>,
        log: TokenLog,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El { crate::el().size(40.0, 20.0) }
    let log: TokenLog = Arc::default();
    let app = AppSpec::new("launcher", (40.0, 20.0), || (), |_, _| crate::el());
    let spec = OverlaySpec::new("launcher", app.size, view)
        .options(OverlayOptions { keys: KeyPolicy::Exclusive, ..Default::default() })
        .on_focus(|s: &mut State, focused, _| {
            if !focused && let Some(h) = &s.handle {
                let log = s.log.clone();
                h.activation_token(move |tok| log.lock().unwrap().push(("from_blur", tok)));
            }
        });
    let (mut overlay, handle) = Overlay::new(spec, State { handle: None, log: log.clone() }, &app);
    overlay.rt.state.handle = Some(handle.clone());
    overlay.rt.frame(0.0);

    let slots: Arc<std::sync::Mutex<Vec<Arc<std::sync::Mutex<TokenPoll>>>>> = Arc::default();
    let slots_factory = slots.clone();
    overlay.core.test_activation_source = Some(Box::new(move || {
        let slot = Arc::new(std::sync::Mutex::new(TokenPoll::Pending));
        slots_factory.lock().unwrap().push(slot.clone());
        Some(TokenSource::Mock(Box::new(move || slot.lock().unwrap().clone())))
    }));

    let push = |tag: &'static str| {
        let log = log.clone();
        move |tok: Option<String>| log.lock().unwrap().push((tag, tok))
    };
    let t0 = crate::time::now();

    // 1. Launcher asks for a token and hides in one turn: token starts without blocking,
    //    hide runs immediately, on_focus(false) request drains None, and the pre-hide token
    //    delivers its answer after the overlay is already hidden.
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.window_focus(true);
    handle.activation_token(push("before_hide"));
    overlay.start_queued_tokens(t0);
    assert_eq!(overlay.activation_tokens.in_flight_len(), 1);
    assert_eq!(overlay.core.wake_deadline(overlay.rt.core()), Some(t0 + TIMEOUT));
    overlay.hide();
    assert!(!overlay.presence.shown(), "hide must not wait for the activation token");
    assert_eq!(*log.lock().unwrap(), [("from_blur", None)]);
    assert_eq!(overlay.activation_tokens.in_flight_len(), 1);

    // Polling before the compositor answers keeps the request in flight.
    overlay.poll_activation_tokens(t0 + Duration::from_millis(5));
    assert_eq!(overlay.activation_tokens.in_flight_len(), 1);

    // Once the compositor delivers done, polling runs the callback after hide.
    *slots.lock().unwrap()[0].lock().unwrap() = TokenPoll::Done(Some("niri-token".into()));
    overlay.poll_activation_tokens(t0 + Duration::from_millis(10));
    assert_eq!(overlay.activation_tokens.in_flight_len(), 0);
    assert_eq!(overlay.core.wake_deadline(overlay.rt.core()), None);
    assert_eq!(*log.lock().unwrap(), [("from_blur", None), ("before_hide", Some("niri-token".into()))]);

    // 2. A silent compositor times out at the fallback deadline and answers None once.
    overlay.presence = Presence::Shown { occluded: false };
    handle.activation_token(push("timed_out"));
    overlay.start_queued_tokens(t0);
    assert_eq!(overlay.core.wake_deadline(overlay.rt.core()), Some(t0 + TIMEOUT));
    overlay.poll_activation_tokens(t0 + TIMEOUT - Duration::from_millis(1));
    assert_eq!(overlay.activation_tokens.in_flight_len(), 1);
    overlay.poll_activation_tokens(t0 + TIMEOUT);
    assert_eq!(overlay.activation_tokens.in_flight_len(), 0);
    assert_eq!(
        *log.lock().unwrap(),
        [("from_blur", None), ("before_hide", Some("niri-token".into())), ("timed_out", None)]
    );

    // 3. Drop cancels in-flight tokens before queued ones and answers re-entrant calls None.
    let reentrant_handle = handle.clone();
    let reentrant_log = log.clone();
    handle.activation_token(move |tok| {
        reentrant_log.lock().unwrap().push(("in_flight_drop", tok));
        let inner = reentrant_log.clone();
        reentrant_handle.activation_token(move |tok2| inner.lock().unwrap().push(("reentrant", tok2)));
    });
    overlay.start_queued_tokens(t0);
    handle.activation_token(push("queued_drop"));
    drop(overlay);
    assert_eq!(
        *log.lock().unwrap(),
        [
            ("from_blur", None),
            ("before_hide", Some("niri-token".into())),
            ("timed_out", None),
            ("in_flight_drop", None),
            ("reentrant", None),
            ("queued_drop", None),
        ]
    );
}

#[test]
fn occluded_and_hidden_overlay_wakes_deliver_poll_keys_once_with_poll_clock_and_injected_pointers() {
    use super::{Overlay, OverlayOptions, OverlaySpec, Presence};
    use crate::tooling::AppSpec;
    use crate::{Cx, El, Key, KeyEvent, Mods, OverlayInput, PointerButton, PointerInput, Tx, col, el};

    #[derive(Default)]
    struct S {
        polls: Vec<f64>,
        focuses: Vec<(&'static str, f64)>,
        clicks: u32,
        send_tab: bool,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        let btn = |key: &'static str| {
            el().key(key)
                .role("button")
                .focusable(true)
                .hit()
                .size(80.0, 30.0)
                .on_press(|s: &mut S| s.clicks += 1)
                .on_focus_change(move |s: &mut S, focused, tx: &mut Tx| {
                    if focused { s.focuses.push((key, tx.clock())); }
                })
        };
        col().size(120.0, 80.0).children([btn("one"), btn("two")])
    }
    let app = AppSpec::new("ov-hidden-key", (120.0, 80.0), S::default, view);
    let spec = OverlaySpec::new("ov-hidden-key", app.size, view)
        .options(OverlayOptions { input: OverlayInput::All, ..Default::default() })
        .poll(|s, now, tx| {
            s.polls.push(now);
            if std::mem::take(&mut s.send_tab) {
                tx.send_key(KeyEvent { key: Key::Tab, mods: Mods::default() });
            }
            false
        });
    let (mut overlay, handle) = Overlay::new(spec, S::default(), &app);
    overlay.rt.set_tab_visits_controls(true);
    overlay.rt.wall_start = Some(crate::time::now());
    overlay.rt.frame.set_test_clock(crate::ManualClock::new(10.0));
    overlay.rt.frame(0.0);

    // 1. Hidden overlay: poll sends Tab -> delivered in the same wake with poll's clock, 1 poll.
    overlay.presence = Presence::default();
    assert!(overlay.frameless());
    let polls_before = overlay.rt.state.polls.len();
    overlay.rt.state.send_tab = true;
    overlay.advance();
    let poll_clock = overlay.rt.state.polls[polls_before];
    assert_eq!(overlay.rt.state.polls[polls_before..], [poll_clock, poll_clock], "hidden overlay runs initial + post-handler follow-up poll at poll_clock");
    assert_eq!(overlay.rt.state.focuses, [("one", poll_clock)]);
    assert!(!overlay.rt.has_input_waiting());

    // 2. Shown but occluded overlay: poll sends Tab -> delivered in the same wake at poll_clock.
    overlay.presence = Presence::Shown { occluded: true };
    assert!(overlay.frameless());
    overlay.rt.frame.test_clock_mut().unwrap().seek(10.5);
    let polls_before = overlay.rt.state.polls.len();
    overlay.rt.state.send_tab = true;
    overlay.advance();
    let poll_clock = overlay.rt.state.polls[polls_before];
    assert_eq!(overlay.rt.state.polls[polls_before..], [poll_clock, poll_clock], "occluded overlay runs initial + post-handler follow-up poll at poll_clock");
    let poll_clock = *overlay.rt.state.polls.last().unwrap();
    assert_eq!(overlay.rt.state.focuses.last().copied(), Some(("two", poll_clock)));
    assert!(!overlay.rt.has_input_waiting());

    // 3. Shown but occluded overlay receiving injected pointer click delivers immediately.
    handle.send_pointer(PointerInput::move_to(20.0, 15.0));
    handle.send_pointer(PointerInput::down(20.0, 15.0, PointerButton::Primary));
    handle.send_pointer(PointerInput::up(20.0, 15.0, PointerButton::Primary));
    overlay.drain_injected_pointers();
    assert!(overlay.rt.has_input_waiting());
    overlay.advance();
    assert_eq!(overlay.rt.state.clicks, 1);
    assert!(!overlay.rt.has_input_waiting());
}

#[test]
fn overlay_immediate_keyboard_dispatch_respects_policy_and_key_policy() {
    use crate::overlay::{Overlay, Presence};
    use crate::{AppSpec, Key, KeyEvent, KeyPolicy, Mods, OverlayOptions, OverlaySpec, el};

    let app = AppSpec::new("app", (400.0, 200.0), || (), |_, _| el());

    // 1. Default Batched policy does not drain immediately.
    let batched_spec = OverlaySpec::new("ov-batched", app.size, |_: &Vec<Key>, _| el().size(200.0, 80.0))
        .options(OverlayOptions { keys: KeyPolicy::OnDemand, ..Default::default() })
        .on_key(|keys, ev, _| {
            keys.push(ev.key.clone());
            true
        });
    assert_eq!(batched_spec.keyboard_dispatch, crate::KeyboardDispatch::Batched);
    let (mut batched, _h) = Overlay::new(batched_spec, Vec::new(), &app);
    batched.presence = Presence::Shown { occluded: false };
    batched.rt.frame(0.0);
    batched.rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    assert!(!batched.dispatch_immediate_keyboard());
    assert!(batched.rt.state.is_empty());
    assert!(batched.rt.has_input_waiting());

    // 2. Immediate policy with KeyPolicy::None still does not drain (overlay does not take keys).
    let no_keys_spec = OverlaySpec::new("ov-no-keys", app.size, |_: &Vec<Key>, _| el().size(200.0, 80.0))
        .options(OverlayOptions { keys: KeyPolicy::None, ..Default::default() })
        .keyboard_dispatch(crate::KeyboardDispatch::Immediate)
        .on_key(|keys, ev, _| {
            keys.push(ev.key.clone());
            true
        });
    let (mut no_keys, _h) = Overlay::new(no_keys_spec, Vec::new(), &app);
    no_keys.presence = Presence::Shown { occluded: false };
    no_keys.rt.frame(0.0);
    no_keys.rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    assert!(!no_keys.dispatch_immediate_keyboard());
    assert!(no_keys.rt.state.is_empty());

    // 3. Immediate policy with KeyPolicy::OnDemand drains immediately without double delivery
    //    and marks the overlay dirty / non-idle for the next paced draw.
    let imm_spec = OverlaySpec::new("ov-imm", app.size, |_: &Vec<Key>, _| el().size(200.0, 80.0))
        .options(OverlayOptions { keys: KeyPolicy::OnDemand, ..Default::default() })
        .keyboard_dispatch(crate::KeyboardDispatch::Immediate)
        .on_key(|keys, ev, _| {
            keys.push(ev.key.clone());
            true
        });
    let (mut imm, _h) = Overlay::new(imm_spec, Vec::new(), &app);
    imm.presence = Presence::Shown { occluded: false };
    imm.idle = true;
    imm.dirty = false;
    imm.rt.frame(0.0);
    imm.rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    assert!(imm.dispatch_immediate_keyboard());
    assert_eq!(imm.rt.state, vec![Key::Enter]);
    assert!(!imm.rt.has_input_waiting());
    assert!(imm.dirty && !imm.idle, "overlay stays dirty so the next paced frame presents");

    // Next paced frame must not redeliver the Enter key.
    let next = crate::time::now() + std::time::Duration::from_millis(16);
    imm.step_frame(next, false);
    assert_eq!(imm.rt.state, vec![Key::Enter]);
}
