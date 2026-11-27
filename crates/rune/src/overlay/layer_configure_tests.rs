use super::{Effect, Event, Phase, step};
use crate::overlay::{Availability, Overlay, OverlayOptions, OverlaySpec, Presence};
use crate::{AppSpec, Cx, DisplayTarget, El, NativeDisplayId, el};

fn dummy_view(_: &(), _: &mut Cx<()>) -> El {
    el().size(120.0, 40.0)
}

fn make_overlay() -> Overlay<()> {
    let app = AppSpec::new("host", (400.0, 300.0), || (), dummy_view);
    let spec = OverlaySpec::new("hud", (120.0, 40.0), dummy_view)
        .options(OverlayOptions::default());
    let (mut overlay, _) = Overlay::new(spec, (), &app);
    overlay.rt.frame(0.0);
    overlay
}

#[test]
fn state_machine_tracks_initial_configure_scale_close_and_cancel() {
    let mut phase = Phase::default();
    assert_eq!(phase, Phase::Idle);

    // Ignoring events while Idle produces no effects.
    assert!(step(&mut phase, Event::Scale(2)).is_empty());
    assert!(step(&mut phase, Event::Configured { width: 100, height: 50, scale: None }).is_empty());
    assert!(step(&mut phase, Event::Closed).is_empty());
    assert!(step(&mut phase, Event::Cancel).is_empty());
    assert_eq!(phase, Phase::Idle);

    // Start with initial monitor scale 1, receive preferred_buffer_scale(2) before configure,
    // then configure arrives without a scale -> attaches at scale 2.
    assert!(step(&mut phase, Event::Started { scale: 1 }).is_empty());
    assert_eq!(phase, Phase::Configuring { scale: 1 });
    assert!(step(&mut phase, Event::Scale(2)).is_empty());
    assert_eq!(phase, Phase::Configuring { scale: 2 });
    let effects: Vec<_> = step(&mut phase, Event::Configured { width: 320, height: 64, scale: None })
        .into_iter()
        .collect();
    assert_eq!(effects, [Effect::Attach { width: 320, height: 64, scale: 2 }]);
    assert_eq!(phase, Phase::Idle);

    // Configure carrying a scale in the same dispatch overrides the earlier scale.
    assert!(step(&mut phase, Event::Started { scale: 1 }).is_empty());
    assert!(step(&mut phase, Event::Scale(2)).is_empty());
    let effects: Vec<_> = step(&mut phase, Event::Configured { width: 0, height: 80, scale: Some(3) })
        .into_iter()
        .collect();
    assert_eq!(effects, [Effect::Attach { width: 0, height: 80, scale: 3 }]);
    assert_eq!(phase, Phase::Idle);

    // Non-positive scales clamp to 1.
    assert!(step(&mut phase, Event::Started { scale: 0 }).is_empty());
    assert_eq!(phase, Phase::Configuring { scale: 1 });
    assert!(step(&mut phase, Event::Scale(-2)).is_empty());
    assert_eq!(phase, Phase::Configuring { scale: 1 });
    let effects: Vec<_> = step(&mut phase, Event::Configured { width: 100, height: 40, scale: Some(0) })
        .into_iter()
        .collect();
    assert_eq!(effects, [Effect::Attach { width: 100, height: 40, scale: 1 }]);

    // Compositor close / fatal display error before configure emits FailClosed.
    assert!(step(&mut phase, Event::Started { scale: 2 }).is_empty());
    let effects: Vec<_> = step(&mut phase, Event::Closed).into_iter().collect();
    assert_eq!(effects, [Effect::FailClosed]);
    assert_eq!(phase, Phase::Idle);

    // Output removal before configure emits OutputGone (hides without failing the overlay).
    assert!(step(&mut phase, Event::Started { scale: 2 }).is_empty());
    let effects: Vec<_> = step(&mut phase, Event::OutputGone).into_iter().collect();
    assert_eq!(effects, [Effect::OutputGone]);
    assert_eq!(phase, Phase::Idle);

    // Hide / cancel before configure emits DropLayer and leaves subsequent configure ignored.
    assert!(step(&mut phase, Event::Started { scale: 2 }).is_empty());
    let effects: Vec<_> = step(&mut phase, Event::Cancel).into_iter().collect();
    assert_eq!(effects, [Effect::DropLayer]);
    assert_eq!(phase, Phase::Idle);
    assert!(step(&mut phase, Event::Configured { width: 200, height: 50, scale: None }).is_empty());

    // Starting again while already configuring drops the superseded layer and enters Configuring.
    assert!(step(&mut phase, Event::Started { scale: 1 }).is_empty());
    let effects: Vec<_> = step(&mut phase, Event::Started { scale: 3 }).into_iter().collect();
    assert_eq!(effects, [Effect::DropLayer]);
    assert_eq!(phase, Phase::Configuring { scale: 3 });
}

#[test]
fn pending_layer_marks_overlay_frameless_until_cancelled_or_hidden() {
    let mut overlay = make_overlay();
    overlay.presence = Presence::Shown { occluded: false };
    assert!(!overlay.has_pending_layer());
    assert!(!overlay.awaiting_layer_frame());
    assert!(!overlay.frameless());

    // Entering Configuring pauses frame drawing without blocking timers/updates.
    let _ = step(&mut overlay.pending_layer.phase, Event::Started { scale: 2 });
    assert!(overlay.has_pending_layer());
    assert!(overlay.awaiting_layer_frame());
    assert!(overlay.frameless());

    // Rapid hide before initial configure cancels the pending layer and resets presence/frameless.
    overlay.hide();
    assert!(!overlay.has_pending_layer());
    assert!(!overlay.presence.shown());
    assert!(!overlay.awaiting_layer_frame());
}

#[test]
fn hide_while_configuring_advances_after_hidden_and_release_cancels_pending_layer() {
    let mut overlay = make_overlay();
    overlay.presence = Presence::Shown { occluded: false };
    let _ = step(&mut overlay.pending_layer.phase, Event::Started { scale: 1 });
    assert!(overlay.has_pending_layer());

    // Queue an after_hidden callback while shown & configuring, then hide before configure lands.
    overlay.after_hidden.phase = crate::overlay::after_hidden::Phase::WaitingForHide;
    overlay.hide();
    assert!(!overlay.has_pending_layer());
    assert!(!overlay.presence.shown());
    assert_eq!(
        overlay.after_hidden.phase,
        crate::overlay::after_hidden::Phase::WaitingNextTurn,
        "hide while configuring must still notify after_hidden"
    );

    // OverlayCore::release also tears down a pending configuring layer.
    let _ = step(&mut overlay.pending_layer.phase, Event::Started { scale: 2 });
    assert!(overlay.has_pending_layer());
    overlay.release();
    assert!(!overlay.has_pending_layer());
}

#[test]
fn terminal_gpu_loss_tears_down_a_configuring_layer_and_marks_unavailable() {
    let mut overlay = make_overlay();
    let latch = crate::render::loss_latch::GpuLossLatch::new();
    overlay.core.gpu_loss_latch = Some(latch.clone());
    overlay.presence = Presence::Shown { occluded: false };
    let _ = step(&mut overlay.pending_layer.phase, Event::Started { scale: 2 });
    assert!(overlay.has_pending_layer());

    let _ = latch.record_device_lost(false, "device removed while configuring");
    assert!(overlay.poll_terminal_gpu_loss());
    assert!(!overlay.has_pending_layer());
    assert!(!overlay.presence.shown());
    assert_eq!(overlay.shared.core.availability.load(std::sync::atomic::Ordering::Acquire), 2);
    assert_eq!(overlay.shared.core.failure.lock().unwrap().as_deref(), Some("device removed while configuring"));
    let _ = Availability::Unavailable;
}

#[test]
fn display_change_while_configuring_requests_recreation_but_resize_does_not() {
    use crate::overlay::input::should_recreate_layer;
    use crate::overlay::requests::Requests;

    let current = DisplayTarget::Native(NativeDisplayId::Wayland(1));
    let mut resize_req = Requests::default();
    resize_req.size = Some((300.0, 80.0));
    assert!(!should_recreate_layer(&current, &resize_req, true));

    let mut display_req = Requests::default();
    display_req.display = Some(DisplayTarget::Native(NativeDisplayId::Wayland(2)));
    assert!(should_recreate_layer(&current, &display_req, true));
}
