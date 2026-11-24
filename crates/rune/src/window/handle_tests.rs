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

#[test]
fn handle_queues_exclude_from_capture_requests_in_order() {
    let handle = WindowHandle::new("private");
    handle.set_exclude_from_capture(true);
    handle.set_exclude_from_capture(false);
    let taken = handle.take_headless();
    assert!(matches!(
        taken[..],
        [WakeEvent::ExcludeFromCapture(0, true), WakeEvent::ExcludeFromCapture(0, false)]
    ));
}

#[test]
fn handle_queues_color_scheme_requests_in_order() {
    let handle = WindowHandle::new("settings");
    handle.set_color_scheme(crate::ColorScheme::Light);
    handle.set_color_scheme(crate::ColorScheme::Dark);
    handle.set_color_scheme(crate::ColorScheme::System);
    let taken = handle.take_headless();
    assert!(matches!(
        taken[..],
        [
            WakeEvent::ColorScheme(0, crate::ColorScheme::Light),
            WakeEvent::ColorScheme(0, crate::ColorScheme::Dark),
            WakeEvent::ColorScheme(0, crate::ColorScheme::System),
        ]
    ));
}

#[test]
fn with_raw_handle_and_with_raw_display_handle_return_closed_when_unbound() {
    let handle = WindowHandle::new("display");
    let mut window_called = false;
    let mut display_called = false;
    let window_err = handle.with_raw_handle(|raw| {
        window_called = true;
        format!("{:?}", raw.as_raw())
    });
    let display_err = handle.with_raw_display_handle(|raw| {
        display_called = true;
        format!("{:?}", raw.as_raw())
    });
    assert!(!window_called && !display_called);
    assert_eq!(window_err, Err("window is closed".into()));
    assert_eq!(display_err, Err("window is closed".into()));
}
