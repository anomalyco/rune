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
    assert_eq!(overlay.core.test_window_drags, 1, "bare .window_drag() starts a window drag");
    assert_eq!(overlay.rt.take_titlebar_action(), None, "TitlebarAction::Drag must be consumed in left_button");
    assert!(!overlay.rt.pointer_captured(), "window drag must not leave pointer capture stuck");
    assert_eq!(overlay.core.position_owner, PositionOwner::Dragging);
    overlay.left_button(false, Some((220.0, 20.0)));
    assert_eq!(overlay.core.position_owner, PositionOwner::Managed, "zero-movement click releases back to Managed");

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

    // First press starts the drag.
    overlay.left_button(true, Some((220.0, 20.0)));
    assert_eq!(overlay.core.test_window_drags, 1);
    overlay.left_button(false, Some((220.0, 20.0)));

    // Second press within double-click interval emits TitlebarAction::DoubleClick, which overlays
    // drain and ignore (no maximize/minimize and no second drag).
    overlay.left_button(true, Some((220.0, 20.0)));
    assert_eq!(overlay.rt.take_titlebar_action(), None, "TitlebarAction::DoubleClick must be drained");
    assert_eq!(overlay.core.test_window_drags, 1, "double-click on overlay drag region must not start a drag");
    overlay.left_button(false, Some((220.0, 20.0)));
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

    // Queue a deferred input-mode switch and press the bare window-drag region: pointer capture
    // is not held, no drag-out request remains, and deferred input applies cleanly.
    overlay.core.pending_input = Some(OverlayInput::Regions);
    overlay.left_button(true, Some((150.0, 20.0)));
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
