use super::*;

/// Before a host binds it, a handle keeps its requests in order; a headless run takes them
/// each turn and the handle goes on keeping later ones.
#[test]
fn an_unbound_handle_queues_its_requests_for_the_host_or_a_headless_run() {
    let handle = WindowHandle::new("w");
    assert!(!handle.headless_pending());
    handle.focus();
    handle.set_position((3, 4));
    assert!(handle.headless_pending());
    let taken = handle.take_headless();
    assert!(matches!(taken[..], [WakeEvent::Focus(0), WakeEvent::Position(0, (3, 4))]));
    assert!(!handle.headless_pending() && handle.take_headless().is_empty());
    handle.hide();
    assert!(matches!(handle.take_headless()[..], [WakeEvent::Window(0, WindowRequest::Hide)]));
}
