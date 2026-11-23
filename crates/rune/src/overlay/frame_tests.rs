use super::{record_presented, retained_reveal_presents};

/// A shown overlay present records `frame.first.presented`, stops pre-first-frame totals, and
/// lets `loop_waits` record `input.ready`; a hidden prewarm frame does none of those.
#[test]
fn shown_overlay_present_records_first_presented_mark_and_prewarm_does_not() {
    let has = |name: &str| crate::marks::marks().iter().any(|(n, _)| *n == name);

    // Prewarm draws while hidden: must not record `frame.first.presented` or stop totals.
    record_presented(true);
    assert!(!has("frame.first.presented"), "a hidden prewarm frame must not record frame.first.presented");
    assert!(crate::marks::counting(), "a hidden prewarm frame must not stop startup totals");

    // A shown overlay present records `frame.first.presented` and lets `input.ready` follow.
    record_presented(false);
    assert!(has("frame.first.presented"), "a shown overlay present must record frame.first.presented");
    assert!(!crate::marks::counting(), "a shown overlay present must stop startup totals");
    crate::marks::loop_waits();
    assert!(has("input.ready"), "input.ready follows once an overlay presents");

    // Retained macOS panels expose their prewarmed pixels on reveal without drawing again.
    assert!(retained_reveal_presents(true, false, true));
    assert!(!retained_reveal_presents(false, false, true));
    assert!(!retained_reveal_presents(true, true, true));
    assert!(!retained_reveal_presents(true, false, false));
}
