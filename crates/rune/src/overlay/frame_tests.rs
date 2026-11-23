use super::{
    Gate, Turn, is_awaiting_frame, is_frameless, polls_only_when_drawing, record_presented,
    retained_reveal_presents, turn, wants_more,
};

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

#[test]
fn layer_frame_callback_gates_present_stops_poll_and_releases_on_device_loss() {
    let now = crate::time::now();
    let base = Gate {
        prewarm: false,
        occluded: false,
        awaiting_frame: true,
        dirty: true,
        idle: false,
        retry: None,
        missed_this_turn: false,
    };

    // While a layer frame callback is outstanding, a dirty or animating overlay skips acquire and
    // present instead of blocking inside vkQueuePresentKHR.
    assert_eq!(turn(base, || now), Turn::AwaitingFrame);
    assert_eq!(turn(Gate { dirty: false, idle: false, ..base }, || now), Turn::AwaitingFrame);
    // A resting overlay still rests without counting a skipped frame.
    assert_eq!(turn(Gate { dirty: false, idle: true, ..base }, || now), Turn::Rest);
    // Once wl_callback.done arrives, drawing resumes immediately.
    assert_eq!(turn(Gate { awaiting_frame: false, ..base }, || now), Turn::Draw);
    // Prewarm draws regardless of frame-callback gating.
    assert_eq!(turn(Gate { prewarm: true, ..base }, || now), Turn::Draw);

    // An animating layer overlay awaiting its callback returns more = false (no ControlFlow::Poll);
    // other non-macOS surfaces without a pending callback keep polling while animating.
    assert!(!wants_more(false, true));
    assert!(!wants_more(true, true));
    assert!(wants_more(false, false));
    assert!(!wants_more(true, false));

    // Device loss clears the wait: if the device died before/during present, nothing was committed
    // so the callback will never arrive and the surface must fall through to teardown.
    assert!(is_awaiting_frame(false, false));
    assert!(!is_awaiting_frame(true, false));
    assert!(!is_awaiting_frame(false, true));
    assert!(!is_awaiting_frame(true, true));

    // Awaiting a frame callback counts as frameless and polls-only while drawing.
    assert!(is_frameless(true, false, true));
    assert!(!is_frameless(true, false, false));
    assert!(polls_only_when_drawing(true, false, true));
    assert!(!polls_only_when_drawing(true, false, false));
}
