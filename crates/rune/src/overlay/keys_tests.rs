use super::{can_take_keys, keys_support};
use crate::{KeyPolicy, OverlayInput, OverlaySupport};

#[test]
fn only_an_on_demand_overlay_window_takes_keys() {
    assert!(!can_take_keys(KeyPolicy::None), "never activates, as today");
    assert!(can_take_keys(KeyPolicy::OnDemand));
    assert_eq!(keys_support(KeyPolicy::None, true), OverlaySupport::Unsupported);
    assert_eq!(keys_support(KeyPolicy::OnDemand, true), OverlaySupport::Supported);
    assert_eq!(keys_support(KeyPolicy::OnDemand, false), OverlaySupport::Unsupported, "no X11 connection to focus it with");
}

/// `KeyPolicy` means the same on every backend that can take keys: `OnDemand` may, `None` never
/// does. Only XWayland, whose override-redirect window never takes focus, refuses `OnDemand`.
#[test]
fn key_policy_agrees_across_backends() {
    use crate::platform::layer_shell_machine::keyboard::supports_keys as layer;
    use crate::overlay::xwayland::xwayland_allowed;
    for policy in [KeyPolicy::None, KeyPolicy::OnDemand] {
        let wants = policy == KeyPolicy::OnDemand;
        assert_eq!(can_take_keys(policy), wants, "X11 and Windows windows, {policy:?}");
        assert_eq!(keys_support(policy, true) == OverlaySupport::Supported, wants, "macOS panel, X11 and Windows windows, {policy:?}");
        assert_eq!(layer(policy, 4, true), wants, "layer-shell, {policy:?}");
        assert_eq!(xwayland_allowed(OverlayInput::None, policy, true), !wants, "XWayland, {policy:?}");
    }
}
