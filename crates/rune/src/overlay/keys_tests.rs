use std::time::Duration;

use super::{
    KeyHold, KeyHoldEvent, can_take_keys, keys_support, next_key_hold,
    should_apply_layer_key_request, will_be_shown,
};
use crate::overlay::app::Overlay;
use crate::overlay::presence::Presence;
use crate::overlay::requests::{KeyRequest, Visibility};
use crate::tooling::AppSpec;
use crate::{KeyPolicy, OverlayInput, OverlayOptions, OverlaySpec, OverlaySupport};

#[test]
fn only_an_on_demand_overlay_window_takes_keys() {
    assert!(!can_take_keys(KeyPolicy::None), "never activates, as today");
    assert!(can_take_keys(KeyPolicy::OnDemand));
    assert_eq!(keys_support(KeyPolicy::None, true), OverlaySupport::Unsupported);
    assert_eq!(keys_support(KeyPolicy::OnDemand, true), OverlaySupport::Supported);
    assert_eq!(keys_support(KeyPolicy::OnDemand, false), OverlaySupport::Unsupported, "no X11 connection to focus it with");
}

/// `KeyPolicy` means the same on every backend that can take keys: `OnDemand` and `Exclusive`
/// may, `None` never does. Only XWayland, whose override-redirect window never takes focus,
/// refuses them.
#[test]
fn key_policy_agrees_across_backends() {
    use crate::overlay::xwayland::xwayland_allowed;
    use crate::platform::layer_shell_machine::keyboard::supports_keys as layer;
    for policy in [KeyPolicy::None, KeyPolicy::OnDemand, KeyPolicy::Exclusive] {
        let wants = policy.takes_keys();
        assert_eq!(can_take_keys(policy), wants, "X11 and Windows windows, {policy:?}");
        assert_eq!(keys_support(policy, true) == OverlaySupport::Supported, wants, "macOS panel, X11 and Windows windows, {policy:?}");
        assert_eq!(layer(policy, 4, true), wants, "layer-shell, {policy:?}");
        assert_eq!(xwayland_allowed(OverlayInput::None, policy, true), !wants, "XWayland, {policy:?}");
    }
}

#[test]
fn key_hold_survives_display_recreate_and_clears_on_hide_or_release() {
    let show = Some(Visibility::Show(Duration::ZERO));
    let hide = Some(Visibility::Hide(Duration::ZERO));

    // Visibility resolution across turns (including recreate where the old surface hid temporarily).
    assert!(!will_be_shown(false, None));
    assert!(will_be_shown(false, show));
    assert!(!will_be_shown(false, hide));
    assert!(will_be_shown(true, None));
    assert!(will_be_shown(true, show));
    assert!(!will_be_shown(true, hide));

    // Shown key-taking overlay holding keys keeps holding them across a turn with no new key
    // request (such as moving to another display via set_display).
    for policy in [KeyPolicy::Exclusive, KeyPolicy::OnDemand] {
        assert_eq!(next_key_hold(true, true, policy, None), KeyHold::Held);
        assert_eq!(next_key_hold(true, true, policy, Some(KeyRequest::Take)), KeyHold::Held);
        assert_eq!(next_key_hold(false, true, policy, Some(KeyRequest::Take)), KeyHold::Held);
        // Same-turn release_keys or hide clears it so the new surface comes up with None.
        assert_eq!(next_key_hold(true, true, policy, Some(KeyRequest::Release)), KeyHold::Released);
        assert_eq!(next_key_hold(true, false, policy, None), KeyHold::Released);
        assert_eq!(next_key_hold(true, false, policy, Some(KeyRequest::Take)), KeyHold::Released);
        // Requesting keys while hidden with no show never latches held keys for a future show.
        assert_eq!(next_key_hold(false, false, policy, Some(KeyRequest::Take)), KeyHold::Released);
        assert_eq!(next_key_hold(false, true, policy, None), KeyHold::Released);
    }

    // KeyPolicy::None never holds keys.
    assert_eq!(next_key_hold(true, true, KeyPolicy::None, None), KeyHold::Released);
    assert_eq!(next_key_hold(false, true, KeyPolicy::None, Some(KeyRequest::Take)), KeyHold::Released);

    // Newly created layer surfaces (!had_live_layer) already committed their initial keyboard
    // interactivity in LayerSurface::new, so sync must not issue a second request/release.
    assert_eq!(should_apply_layer_key_request(false, Some(KeyRequest::Take), true, KeyPolicy::Exclusive), None);
    assert_eq!(should_apply_layer_key_request(false, Some(KeyRequest::Release), true, KeyPolicy::Exclusive), None);
    // Already-live layer surfaces (including prewarmed ones) still apply explicit key requests.
    assert_eq!(
        should_apply_layer_key_request(true, Some(KeyRequest::Take), true, KeyPolicy::Exclusive),
        Some(KeyRequest::Take),
    );
    assert_eq!(
        should_apply_layer_key_request(true, Some(KeyRequest::Release), true, KeyPolicy::Exclusive),
        Some(KeyRequest::Release),
    );
    assert_eq!(should_apply_layer_key_request(true, None, true, KeyPolicy::Exclusive), None);
    assert_eq!(should_apply_layer_key_request(true, Some(KeyRequest::Take), false, KeyPolicy::Exclusive), None);
    assert_eq!(should_apply_layer_key_request(true, Some(KeyRequest::Take), true, KeyPolicy::None), None);
}

#[test]
fn overlay_hide_and_surface_failure_clear_key_hold() {
    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El { crate::el().size(100.0, 40.0) }
    let app = AppSpec::new("key-hold-hide", (100.0, 40.0), || (), view);
    let spec = OverlaySpec::new("key-hold-hide", app.size, view)
        .options(OverlayOptions { keys: KeyPolicy::Exclusive, ..Default::default() });
    let (mut overlay, _) = Overlay::new(spec, (), &app);
    assert_eq!(overlay.core.key_hold, KeyHold::Released);

    // Direct hide clears held keys.
    overlay.core.presence = Presence::Shown { occluded: false };
    overlay.core.key_hold.step(KeyHoldEvent::Take);
    assert!(overlay.core.key_hold.is_held());
    overlay.hide();
    assert_eq!(overlay.core.key_hold, KeyHold::Released);

    // Fade-out clears held keys immediately when input stops.
    overlay.core.presence = Presence::Shown { occluded: false };
    overlay.core.key_hold.step(KeyHoldEvent::Take);
    overlay.hide_fading(Duration::from_millis(120));
    assert_eq!(overlay.core.key_hold, KeyHold::Released);

    // Compositor layer close clears held keys.
    overlay.core.presence = Presence::Shown { occluded: false };
    overlay.core.key_hold.step(KeyHoldEvent::Take);
    overlay.layer_closed(None);
    assert_eq!(overlay.core.key_hold, KeyHold::Released);

    // Surface refusal clears held keys.
    overlay.core.presence = Presence::Shown { occluded: false };
    overlay.core.key_hold.step(KeyHoldEvent::Take);
    overlay.surface_refused("refused".into());
    assert_eq!(overlay.core.key_hold, KeyHold::Released);
}
