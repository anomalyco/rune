use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::*;

#[derive(Default)]
struct WatchState {
    seen: Vec<(Availability, Availability, Option<String>)>,
    handle: Option<OverlayHandle<WatchState>>,
    reschedule_on_loss: Option<Duration>,
    reshow_on_loss: bool,
    copy_on_loss: Option<&'static str>,
    polls: u32,
}

fn watch_view(s: &WatchState, _: &mut Cx<WatchState>) -> El {
    let label = match s.seen.last().map(|e| e.0) {
        Some(Availability::Available) => "available",
        Some(Availability::Unavailable) => "unavailable",
        _ => "unknown",
    };
    crate::el().child(crate::text(label).key("status"))
}

fn on_avail(s: &mut WatchState, availability: Availability, tx: &mut Tx) {
    let handle_avail = s.handle.as_ref().map_or(Availability::Unknown, OverlayHandle::availability);
    let failure = s.handle.as_ref().and_then(OverlayHandle::failure);
    s.seen.push((availability, handle_avail, failure));
    if availability == Availability::Unavailable {
        if let Some(delay) = s.reschedule_on_loss {
            tx.wake_after(delay);
        }
        if let Some(text) = s.copy_on_loss {
            tx.copy(text);
        }
        if s.reshow_on_loss && let Some(handle) = &s.handle {
            handle.show();
        }
    }
}

fn watch_overlay(poll: Option<fn(&mut WatchState, f64, &mut Tx) -> bool>) -> (Overlay<WatchState>, OverlayHandle<WatchState>) {
    let app = AppSpec::new("avail", (120.0, 40.0), WatchState::default, watch_view);
    let mut configured = OverlaySpec::new("avail", app.size, watch_view)
        .options(OverlayOptions::default())
        .on_availability(on_avail);
    if let Some(poll) = poll {
        configured = configured.poll(poll);
    }
    let (mut overlay, handle) = Overlay::new(configured, WatchState::default(), &app);
    overlay.rt.wall_start = None;
    overlay.rt.state.handle = Some(handle.clone());
    (overlay, handle)
}

#[test]
fn on_availability_fires_once_per_transition_and_deduplicates_repeats() {
    let (mut overlay, handle) = watch_overlay(None);
    assert_eq!(handle.availability(), Availability::Unknown);
    assert!(overlay.rt.state.seen.is_empty(), "creating an overlay does not fire Unknown");

    // Repeating Unknown is a no-op.
    assert!(!overlay.set_availability_notify(Availability::Unknown));
    assert!(overlay.rt.state.seen.is_empty());

    // Unknown -> Available via ready().
    overlay.ready(false);
    assert_eq!(handle.availability(), Availability::Available);
    assert_eq!(handle.failure(), None);
    assert_eq!(overlay.rt.state.seen, vec![(Availability::Available, Availability::Available, None)]);

    // Repeating Available does not fire again.
    overlay.ready(true);
    assert!(!overlay.set_availability_notify(Availability::Available));
    assert_eq!(overlay.rt.state.seen.len(), 1);

    // Available -> Unavailable via fail(); handle.availability() and failure() are already updated inside callback.
    overlay.fail("surface lost".into());
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(handle.failure().as_deref(), Some("surface lost"));
    assert_eq!(
        overlay.rt.state.seen.last(),
        Some(&(Availability::Unavailable, Availability::Unavailable, Some("surface lost".into()))),
    );

    // Repeating Unavailable updates failure() without firing on_availability again.
    overlay.fail("still unavailable".into());
    assert_eq!(handle.failure().as_deref(), Some("still unavailable"));
    assert_eq!(overlay.rt.state.seen.len(), 2);

    // Unavailable -> Available clears failure() before firing on_availability.
    overlay.ready(true);
    assert_eq!(handle.availability(), Availability::Available);
    assert_eq!(handle.failure(), None);
    assert_eq!(
        overlay.rt.state.seen,
        vec![
            (Availability::Available, Availability::Available, None),
            (Availability::Unavailable, Availability::Unavailable, Some("surface lost".into())),
            (Availability::Available, Availability::Available, None),
        ],
    );
}

#[test]
fn on_availability_propagates_tx_wakes_clipboard_and_state_changes() {
    fn poll(s: &mut WatchState, _: f64, _: &mut Tx) -> bool {
        s.polls += 1;
        false
    }
    let (mut overlay, _handle) = watch_overlay(Some(poll));
    let clipboard = crate::clipboard::memory();
    overlay.rt.use_clipboard(&clipboard);
    let t0 = crate::time::now();
    overlay.rt.wall_start = Some(t0);
    overlay.rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    overlay.rt.state.reschedule_on_loss = Some(Duration::from_millis(200));
    overlay.rt.state.copy_on_loss = Some("lost");

    overlay.ready(false);
    overlay.dirty = false;
    overlay.rt.frame(0.0);
    assert_eq!(
        overlay.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("status")).and_then(|e| e.text).as_deref(),
        Some("available"),
    );

    // Transition to Unavailable while hidden: schedules wake_after immediately, flushes clipboard, marks dirty.
    overlay.fail("configure timeout".into());
    assert!(overlay.dirty, "state mutation in on_availability marks overlay dirty");
    assert!(overlay.rt.copy_request.is_none(), "clipboard request flushed immediately");
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("lost"), "in-memory clipboard received the write");
    let due = overlay.wake_deadline().expect("wake_after in on_availability must schedule wake_deadline even when hidden");
    assert_eq!(due, t0 + Duration::from_millis(200));

    // Not before the deadline, then fires once the clock reaches it without needing a surface.
    let polls_before = overlay.rt.state.polls;
    overlay.rt.frame.test_clock_mut().unwrap().seek(0.199);
    overlay.wake_due(t0 + Duration::from_millis(199));
    assert_eq!(overlay.rt.state.polls, polls_before);

    overlay.rt.frame.test_clock_mut().unwrap().seek(0.201);
    overlay.wake_due(t0 + Duration::from_millis(201));
    assert_eq!(overlay.rt.state.polls, polls_before + 1);

    // Next frame commits the updated view state.
    overlay.rt.frame(0.0);
    assert_eq!(
        overlay.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("status")).and_then(|e| e.text).as_deref(),
        Some("unavailable"),
    );
}

#[test]
fn wayland_layer_closed_marks_unavailable_populates_failure_and_notifies() {
    let (mut overlay, handle) = watch_overlay(None);
    overlay.rt.state.reshow_on_loss = true;
    overlay.ready(true);
    overlay.presence = Presence::Shown { occluded: false };

    // Compositor closed zwlr_layer_surface_v1 (output unplugged or compositor policy).
    overlay.layer_closed(None);
    assert!(!overlay.presence.shown(), "layer close hides the overlay");
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(handle.failure().as_deref(), Some("compositor closed layer surface"));
    assert_eq!(
        overlay.rt.state.seen.last(),
        Some(&(Availability::Unavailable, Availability::Unavailable, Some("compositor closed layer surface".into()))),
    );
    // Calling handle.show() inside on_availability queues a show for the next turn without deadlock.
    let queued = std::mem::take(&mut *overlay.shared.core.requests.lock().unwrap());
    assert_eq!(queued.visible(), Some(true));

    // A later successful show restores Available and clears failure().
    overlay.ready(true);
    assert_eq!(handle.availability(), Availability::Available);
    assert_eq!(handle.failure(), None);

    // A dead Wayland display connection reports the display error in failure().
    overlay.presence = Presence::Shown { occluded: false };
    overlay.layer_closed(Some("the display connection failed: broken pipe"));
    assert!(!overlay.presence.shown());
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(handle.failure().as_deref(), Some("the display connection failed: broken pipe"));
}

#[test]
fn surface_refused_marks_unavailable_and_notifies_once() {
    let (mut overlay, handle) = watch_overlay(None);
    overlay.ready(true);
    overlay.presence = Presence::Shown { occluded: false };

    overlay.surface_refused("its GPU device was lost".into());
    assert!(!overlay.presence.shown());
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(handle.failure().as_deref(), Some("its GPU device was lost"));
    assert_eq!(overlay.rt.state.seen.len(), 2);
}

#[test]
fn dropping_overlay_stores_unavailable_without_calling_on_availability() {
    let calls = Arc::new(AtomicUsize::new(0));
    struct DropState(Arc<AtomicUsize>);
    fn view(_: &DropState, _: &mut Cx<DropState>) -> El { crate::el() }
    let app = AppSpec::new("drop-avail", (40.0, 20.0), || DropState(Arc::new(AtomicUsize::new(0))), view);
    let configured = OverlaySpec::new("drop-avail", app.size, view)
        .options(OverlayOptions::default())
        .on_availability(|s, _, _| { s.0.fetch_add(1, Ordering::Relaxed); });
    let (mut overlay, handle) = Overlay::new(configured, DropState(calls.clone()), &app);
    overlay.ready(false);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(handle.availability(), Availability::Available);

    drop(overlay);
    assert_eq!(handle.availability(), Availability::Unavailable, "Drop stores Unavailable silently");
    assert_eq!(calls.load(Ordering::Relaxed), 1, "Drop must never invoke on_availability");
}

#[test]
fn overlay_without_on_availability_does_not_mark_dirty_on_transition() {
    fn view(_: &(), _: &mut Cx<()>) -> El { crate::el() }
    let app = AppSpec::new("no-cb", (40.0, 20.0), || (), view);
    let (mut overlay, handle) = Overlay::new(OverlaySpec::new("no-cb", app.size, view), (), &app);
    overlay.rt.frame(0.0);
    overlay.dirty = false;

    assert!(overlay.set_availability_notify(Availability::Available));
    assert_eq!(handle.availability(), Availability::Available);
    assert!(!overlay.dirty && !overlay.rt.stale(), "without on_availability, transitions ask for no extra frame");

    overlay.fail("boom".into());
    assert_eq!(handle.availability(), Availability::Unavailable);
    assert_eq!(handle.failure().as_deref(), Some("boom"));
    assert!(!overlay.dirty && !overlay.rt.stale(), "failure without callback asks for no extra frame");
}

#[test]
fn a_layer_surface_whose_output_went_hides_and_stays_available() {
    let (mut overlay, handle) = watch_overlay(None);
    overlay.ready(true);
    overlay.presence = Presence::Shown { occluded: false };
    assert_eq!(overlay.rt.state.seen, vec![(Availability::Available, Availability::Available, None)]);

    overlay.layer_output_gone();
    assert!(!overlay.presence.shown(), "output unplug hides the overlay");
    assert_eq!(handle.availability(), Availability::Available, "output unplug keeps availability Available");
    assert_eq!(handle.failure(), None, "output unplug records no failure");
    assert_eq!(
        overlay.rt.state.seen.len(),
        1,
        "output unplug must not fire an Unavailable on_availability transition",
    );
}

#[test]
fn a_show_whose_display_went_before_its_surface_was_made_reads_no_failure() {
    // 1. First show (Availability::Unknown) aimed at a display that already went.
    let (mut overlay, handle) = watch_overlay(None);
    overlay.finish_prepare_output_gone(true);
    assert!(!overlay.presence.shown());
    assert_eq!(handle.availability(), Availability::Unknown);
    assert_eq!(handle.failure(), None);
    assert!(overlay.rt.state.seen.is_empty(), "gone display must not fire on_availability(Unavailable)");

    // 2. Re-show after an output unplug while Available: stays Available with no failure.
    overlay.ready(true);
    overlay.layer_output_gone();
    overlay.finish_prepare_output_gone(true);
    assert!(!overlay.presence.shown());
    assert_eq!(handle.availability(), Availability::Available);
    assert_eq!(handle.failure(), None);
    assert_eq!(overlay.rt.state.seen.len(), 1);
}

#[test]
fn resolve_wayland_output_returns_gone_for_disconnected_targets() {
    use super::lifetime::{WaylandOutputTarget, resolve_wayland_output};
    use crate::{DisplayTarget, NativeDisplayId};

    // Connected Wayland native ID -> Output(id); disconnected -> Gone (never Any).
    assert_eq!(
        resolve_wayland_output(&DisplayTarget::Native(NativeDisplayId::Wayland(7)), |_| None, |id| id == 7),
        WaylandOutputTarget::Output(7),
    );
    assert_eq!(
        resolve_wayland_output(&DisplayTarget::Native(NativeDisplayId::Wayland(9)), |_| None, |id| id == 7),
        WaylandOutputTarget::Gone,
    );
    // Primary and UnderPointer let the compositor choose an output.
    assert_eq!(
        resolve_wayland_output(&DisplayTarget::Primary, |_| None, |_| false),
        WaylandOutputTarget::Any,
    );
    assert_eq!(
        resolve_wayland_output(&DisplayTarget::UnderPointer, |_| None, |_| false),
        WaylandOutputTarget::Any,
    );
}
