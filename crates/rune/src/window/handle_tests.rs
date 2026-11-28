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

/// `hide_now` with no native window still asks the host to hide, as `hide` does.
#[test]
fn hide_now_queues_the_hosts_hide() {
    let handle = WindowHandle::new("shot");
    handle.hide_now();
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
fn handle_queues_resize_anchor_requests_in_order() {
    use crate::ResizeAnchor;
    let handle = WindowHandle::new("launcher");
    handle.set_resize_anchor(ResizeAnchor::Bottom);
    handle.set_resize_anchor(ResizeAnchor::BottomRight);
    handle.set_resize_anchor(ResizeAnchor::TopLeft);
    let taken = handle.take_headless();
    assert!(matches!(
        taken[..],
        [
            WakeEvent::ResizeAnchor(0, ResizeAnchor::Bottom),
            WakeEvent::ResizeAnchor(0, ResizeAnchor::BottomRight),
            WakeEvent::ResizeAnchor(0, ResizeAnchor::TopLeft),
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

#[test]
fn activate_queues_open_with_token_and_drops_empty_tokens() {
    let handle = WindowHandle::new("notify");
    handle.open();
    handle.activate("xdg-token-1");
    handle.activate("");
    handle.activate(String::from("xdg-token-2"));
    let taken = handle.take_headless();
    assert_eq!(taken.len(), 4);
    assert!(matches!(&taken[0], WakeEvent::Window(0, WindowRequest::Open(None))));
    assert!(matches!(&taken[1], WakeEvent::Window(0, WindowRequest::Open(Some(t))) if t == "xdg-token-1"));
    assert!(matches!(&taken[2], WakeEvent::Window(0, WindowRequest::Open(None))));
    assert!(matches!(&taken[3], WakeEvent::Window(0, WindowRequest::Open(Some(t))) if t == "xdg-token-2"));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn callback_route_drains_pre_bind_requests_and_forwards_live_requests_and_readiness() {
    let handle = WindowHandle::new("prefs");
    assert!(!handle.is_ready());
    assert_eq!(handle.scale(), None);

    // Requests queued before binding wait until bind_callback drains them in order.
    handle.open();
    handle.focus();

    let received: Arc<Mutex<Vec<(usize, &'static str)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);
    handle.bind_callback(
        Arc::new(move |ev| {
            let tag = match ev {
                WakeEvent::Window(slot, WindowRequest::Open(_)) => (slot, "open"),
                WakeEvent::Window(slot, WindowRequest::Close) => (slot, "close"),
                WakeEvent::Focus(slot) => (slot, "focus"),
                _ => (usize::MAX, "other"),
            };
            sink.lock().unwrap().push(tag);
        }),
        7,
    );

    // Subsequent requests forward immediately with the bound slot.
    handle.close();
    assert!(!handle.headless_pending());
    assert!(handle.take_headless().is_empty());
    assert_eq!(*received.lock().unwrap(), vec![(7, "open"), (7, "focus"), (7, "close")]);

    // Non-winit readiness and scale publish without a winit::window::Window.
    handle.set_ready(true);
    handle.set_non_winit_scale(Some(2.5));
    assert!(handle.is_ready());
    assert_eq!(handle.scale(), Some(2.5));
    handle.set_ready(false);
    handle.set_non_winit_scale(None);
    assert!(!handle.is_ready());
    assert_eq!(handle.scale(), None);
}
