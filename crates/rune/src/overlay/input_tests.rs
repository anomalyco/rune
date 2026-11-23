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
