use super::toplevel_capabilities;
use crate::{OverlayInput, OverlaySupport};

/// The toplevel fallback says what it can't do: placement and stacking always, and passing
/// clicks through outside hit regions, which a toplevel's all-or-nothing input region can't.
#[test]
fn toplevel_reports_what_is_degraded() {
    for input in [OverlayInput::None, OverlayInput::All, OverlayInput::Regions] {
        let caps = toplevel_capabilities(input);
        assert_eq!(caps.placement, OverlaySupport::Unsupported, "{input:?}");
        assert_eq!(caps.always_on_top, OverlaySupport::Unsupported, "{input:?}");
        assert_eq!(caps.nonactivating_keys, OverlaySupport::Unsupported, "{input:?}");
        assert_eq!(caps.global_pointer, OverlaySupport::Unsupported, "{input:?}");
        assert_eq!(caps.capture_exclusion, OverlaySupport::Unsupported, "{input:?}");
        let through = if input == OverlayInput::Regions { OverlaySupport::Unsupported } else { OverlaySupport::Supported };
        assert_eq!(caps.click_through, through, "{input:?}");
    }
}

/// Every other backend places, stacks and passes clicks through.
#[test]
fn placed_backends_report_full_windowing() {
    let caps = crate::OverlayCapabilities::placed(OverlaySupport::Requested, OverlaySupport::Supported, OverlaySupport::Unsupported);
    assert_eq!((caps.capture_exclusion, caps.nonactivating_keys, caps.global_pointer), (OverlaySupport::Requested, OverlaySupport::Supported, OverlaySupport::Unsupported));
    assert_eq!((caps.placement, caps.always_on_top, caps.click_through), (OverlaySupport::Supported, OverlaySupport::Supported, OverlaySupport::Supported));
}
