use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use super::super::{Overlay, OverlayHandle, OverlaySpec, Presence};
use super::{Effect, Event, Phase, step};
use crate::{AppSpec, Cx, El, el};

#[derive(Default)]
struct State {
    runs: Vec<&'static str>,
}

fn view(_: &State, _: &mut Cx<State>) -> El {
    el().size(20.0, 20.0)
}

fn make_overlay() -> (Overlay<State>, OverlayHandle<State>, Arc<AtomicUsize>) {
    let app = AppSpec::new("picker", (40.0, 20.0), State::default, view);
    let spec = OverlaySpec::new("picker", app.size, view);
    let (mut overlay, handle) = Overlay::new(spec, State::default(), &app);
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let noop = Arc::new(|| {});
    overlay.connect(
        Arc::new(move || { w.fetch_add(1, Ordering::Relaxed); }),
        noop.clone(),
        noop,
        Default::default(),
    );
    wakes.store(0, Ordering::Relaxed);
    overlay.rt.frame(0.0);
    (overlay, handle, wakes)
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn step_vec(phase: &mut Phase, event: Event) -> Vec<Effect> {
    step(phase, event).into_iter().collect()
}

#[test]
fn state_machine_covers_shown_hidden_sync_next_turn_and_cancel() {
    // Shown -> queue -> hide (sync) -> shown before answer -> hide (sync) -> answer.
    let mut phase = Phase::Idle;
    assert_eq!(step_vec(&mut phase, Event::Queued { hidden: false, uses_sync: true }), []);
    assert_eq!(phase, Phase::WaitingForHide);
    assert_eq!(
        step_vec(&mut phase, Event::Hidden { uses_sync: true, surface_destroyed: true }),
        [Effect::StartSync],
    );
    assert_eq!(phase, Phase::WaitingSync);
    // Shown before sync answers: cancel sync and wait for the next hide.
    assert_eq!(step_vec(&mut phase, Event::Shown), [Effect::CancelSync]);
    assert_eq!(phase, Phase::WaitingForHide);
    // Stale sync answer while waiting for hide is ignored.
    assert_eq!(step_vec(&mut phase, Event::SyncAnswered), []);
    assert_eq!(phase, Phase::WaitingForHide);
    // Next hide starts a new sync; answer runs callbacks and returns to Idle.
    assert_eq!(
        step_vec(&mut phase, Event::Hidden { uses_sync: true, surface_destroyed: true }),
        [Effect::StartSync],
    );
    assert_eq!(step_vec(&mut phase, Event::SyncAnswered), [Effect::RunCallbacks]);
    assert_eq!(phase, Phase::Idle);

    // Non-Wayland: hidden -> queue -> WaitingNextTurn -> NewTurn -> ReadyToRun -> AboutToWaitHidden.
    assert_eq!(step_vec(&mut phase, Event::Queued { hidden: true, uses_sync: false }), [Effect::Wake]);
    assert_eq!(phase, Phase::WaitingNextTurn);
    assert_eq!(step_vec(&mut phase, Event::AboutToWaitHidden), []);
    assert_eq!(step_vec(&mut phase, Event::NewTurn), []);
    assert_eq!(phase, Phase::ReadyToRun);
    assert_eq!(step_vec(&mut phase, Event::AboutToWaitHidden), [Effect::RunCallbacks]);
    assert_eq!(phase, Phase::Idle);

    // Non-Wayland: shown before next turn cancels run and waits for next hide.
    assert_eq!(step_vec(&mut phase, Event::Queued { hidden: true, uses_sync: false }), [Effect::Wake]);
    assert_eq!(step_vec(&mut phase, Event::NewTurn), []);
    assert_eq!(phase, Phase::ReadyToRun);
    assert_eq!(step_vec(&mut phase, Event::Shown), []);
    assert_eq!(phase, Phase::WaitingForHide);
    assert_eq!(step_vec(&mut phase, Event::Cancel), [Effect::DropCallbacks]);
    assert_eq!(phase, Phase::Idle);

    // Wayland: sync failure drops callbacks unrun.
    assert_eq!(step_vec(&mut phase, Event::Queued { hidden: true, uses_sync: true }), [Effect::StartSync]);
    assert_eq!(step_vec(&mut phase, Event::SyncFailed), [Effect::CancelSync, Effect::DropCallbacks]);
    assert_eq!(phase, Phase::Idle);

    // Wayland: a second hide that actually destroys a live surface replaces the sync.
    assert_eq!(step_vec(&mut phase, Event::Queued { hidden: true, uses_sync: true }), [Effect::StartSync]);
    assert_eq!(step_vec(&mut phase, Event::Hidden { uses_sync: true, surface_destroyed: false }), []);
    assert_eq!(
        step_vec(&mut phase, Event::Hidden { uses_sync: true, surface_destroyed: true }),
        [Effect::StartSync],
    );
    assert_eq!(step_vec(&mut phase, Event::Cancel), [Effect::CancelSync, Effect::DropCallbacks]);
    assert_eq!(phase, Phase::Idle);
}

#[test]
fn non_wayland_after_hidden_runs_once_in_the_turn_after_hide_in_fifo_order() {
    let (mut overlay, handle, wakes) = make_overlay();
    overlay.mark_shown_for_test();

    // Asked while shown: waits for the next hide.
    handle.after_hidden(|s, tx| {
        s.runs.push("first");
        tx.wake_after(Duration::from_millis(50));
    });
    handle.after_hidden(|s, _| s.runs.push("second"));
    overlay.sync_after_hidden();
    assert_eq!(overlay.after_hidden.phase, Phase::WaitingForHide);
    assert!(overlay.rt.state.runs.is_empty());

    // Turn 1: hide happens; callbacks must NOT run in the hide's own turn.
    wakes.store(0, Ordering::Relaxed);
    overlay.hide();
    assert_eq!(overlay.after_hidden.phase, Phase::WaitingNextTurn);
    assert_eq!(wakes.load(Ordering::Relaxed), 1, "hiding wakes the loop for the next turn");
    overlay.poll_after_hidden();
    assert!(overlay.rt.state.runs.is_empty(), "must not run during the hide's own turn");

    // Turn 2: NewTurn + about_to_wait runs callbacks once in FIFO order and schedules Tx wakes.
    overlay.after_hidden_new_turn();
    assert_eq!(overlay.after_hidden.phase, Phase::ReadyToRun);
    overlay.poll_after_hidden();
    assert_eq!(overlay.rt.state.runs, ["first", "second"]);
    assert_eq!(overlay.after_hidden.phase, Phase::Idle);
    assert!(overlay.wake_deadline().is_some(), "Tx::wake_after from after_hidden must be scheduled");

    // Subsequent turns do not re-run.
    overlay.after_hidden_new_turn();
    overlay.poll_after_hidden();
    assert_eq!(overlay.rt.state.runs, ["first", "second"]);
}

#[test]
fn non_wayland_shown_before_next_turn_waits_for_the_next_hide() {
    let (mut overlay, handle, _) = make_overlay();
    assert!(!overlay.presence.drawing());

    // Asked while hidden: armed for the next turn.
    handle.after_hidden(|s, _| s.runs.push("after"));
    overlay.sync_after_hidden();
    assert_eq!(overlay.after_hidden.phase, Phase::WaitingNextTurn);

    // Next turn starts, but the overlay is shown before about_to_wait runs.
    overlay.after_hidden_new_turn();
    assert_eq!(overlay.after_hidden.phase, Phase::ReadyToRun);
    overlay.mark_shown_for_test();
    assert_eq!(overlay.after_hidden.phase, Phase::WaitingForHide);
    overlay.poll_after_hidden();
    assert!(overlay.rt.state.runs.is_empty(), "must not run once shown again");

    // Next hide runs it in the turn after that hide.
    overlay.hide();
    overlay.poll_after_hidden();
    assert!(overlay.rt.state.runs.is_empty());
    overlay.after_hidden_new_turn();
    overlay.poll_after_hidden();
    assert_eq!(overlay.rt.state.runs, ["after"]);
}

#[test]
fn after_hidden_waits_for_fade_out_to_finish_before_arming() {
    let (mut overlay, handle, _) = make_overlay();
    overlay.mark_shown_for_test();

    handle.after_hidden(|s, _| s.runs.push("faded"));
    overlay.sync_after_hidden();

    let now = crate::time::now();
    overlay.presence = Presence::FadingOut {
        until: now + Duration::from_millis(150),
        length: Duration::from_millis(150),
        occluded: false,
    };
    overlay.after_hidden_new_turn();
    overlay.poll_after_hidden();
    assert!(overlay.rt.state.runs.is_empty(), "fading overlay is not hidden yet");

    // Fade finishes -> transitions to Hidden and arms next turn.
    overlay.presence_due(now + Duration::from_millis(200));
    assert!(!overlay.presence.drawing());
    assert_eq!(overlay.after_hidden.phase, Phase::WaitingNextTurn);
    overlay.poll_after_hidden();
    assert!(overlay.rt.state.runs.is_empty());

    overlay.after_hidden_new_turn();
    overlay.poll_after_hidden();
    assert_eq!(overlay.rt.state.runs, ["faded"]);
}

#[test]
fn close_or_drop_drops_pending_after_hidden_callbacks_unrun() {
    // 1. Cancel / close while callbacks are queued.
    let (mut overlay, handle, wakes) = make_overlay();
    overlay.mark_shown_for_test();

    let dropped_parked = Arc::new(AtomicBool::new(false));
    let flag1 = DropFlag(dropped_parked.clone());
    handle.after_hidden(move |s, _| {
        let _keep = &flag1;
        s.runs.push("parked");
    });
    overlay.sync_after_hidden();

    let dropped_queued = Arc::new(AtomicBool::new(false));
    let flag2 = DropFlag(dropped_queued.clone());
    handle.after_hidden(move |s, _| {
        let _keep = &flag2;
        s.runs.push("queued");
    });

    overlay.close_after_hidden();
    assert!(dropped_parked.load(Ordering::Relaxed), "parked callback must drop on close");
    assert!(dropped_queued.load(Ordering::Relaxed), "queued callback must drop on close");
    assert!(overlay.rt.state.runs.is_empty());

    // Late calls on a surviving handle drop immediately without waking.
    wakes.store(0, Ordering::Relaxed);
    let dropped_late = Arc::new(AtomicBool::new(false));
    let flag3 = DropFlag(dropped_late.clone());
    handle.after_hidden(move |s, _| {
        let _keep = &flag3;
        s.runs.push("late");
    });
    assert!(dropped_late.load(Ordering::Relaxed), "late callback after close must drop at once");
    assert_eq!(wakes.load(Ordering::Relaxed), 0, "closed handle must not wake");

    // 2. Cancel (vanish) drops queued and parked callbacks unrun without closing the handle.
    let (mut overlay_cancel, handle_cancel, _) = make_overlay();
    overlay_cancel.mark_shown_for_test();
    let dropped_cancel = Arc::new(AtomicBool::new(false));
    let flag_cancel = DropFlag(dropped_cancel.clone());
    handle_cancel.after_hidden(move |s, _| {
        let _keep = &flag_cancel;
        s.runs.push("cancelled");
    });
    overlay_cancel.sync_after_hidden();
    overlay_cancel.cancel_after_hidden();
    overlay_cancel.hide();
    overlay_cancel.after_hidden_new_turn();
    overlay_cancel.poll_after_hidden();
    assert!(dropped_cancel.load(Ordering::Relaxed), "cancel must drop pending callbacks unrun");
    assert!(overlay_cancel.rt.state.runs.is_empty());

    // 3. Dropping the overlay directly drops unrun callbacks even while the handle lives.
    let (overlay2, handle2, _) = make_overlay();
    let dropped_on_drop = Arc::new(AtomicBool::new(false));
    let flag4 = DropFlag(dropped_on_drop.clone());
    handle2.after_hidden(move |s, _| {
        let _keep = &flag4;
        s.runs.push("never");
    });
    drop(overlay2);
    assert!(dropped_on_drop.load(Ordering::Relaxed), "dropping overlay must drop pending callbacks unrun");
    drop(handle2);
}

#[test]
fn after_hidden_callback_queuing_another_runs_on_following_turn() {
    let (mut overlay, handle, wakes) = make_overlay();
    let handle2 = handle.clone();
    handle.after_hidden(move |s, _| {
        s.runs.push("outer");
        handle2.after_hidden(|s, _| s.runs.push("inner"));
    });
    overlay.sync_after_hidden();
    overlay.after_hidden_new_turn();
    wakes.store(0, Ordering::Relaxed);
    overlay.poll_after_hidden();
    assert_eq!(overlay.rt.state.runs, ["outer"]);
    assert_eq!(wakes.load(Ordering::Relaxed), 1, "inner after_hidden wakes for the next turn");

    overlay.sync_after_hidden();
    overlay.after_hidden_new_turn();
    overlay.poll_after_hidden();
    assert_eq!(overlay.rt.state.runs, ["outer", "inner"]);
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod wayland {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use wayland_client::Connection;
    use winit::raw_window_handle::WaylandDisplayHandle;

    use super::{DropFlag, Phase, make_overlay};
    use crate::platform::layer_shell::{HideSync, SyncPoll};

    fn send_done(stream: &mut UnixStream, callback_id: u32, serial: u32) -> std::io::Result<()> {
        let mut msg = Vec::with_capacity(12);
        msg.extend_from_slice(&callback_id.to_ne_bytes());
        msg.extend_from_slice(&(12u32 << 16).to_ne_bytes());
        msg.extend_from_slice(&serial.to_ne_bytes());
        stream.write_all(&msg)
    }

    fn host_read_once(host: &Connection) {
        let guard = host.prepare_read().or_else(|| {
            host.backend().dispatch_inner_queue().ok();
            host.prepare_read()
        });
        if let Some(guard) = guard {
            guard.read().ok();
        }
    }

    fn display_handle_for(host: &Connection) -> WaylandDisplayHandle {
        WaylandDisplayHandle::new(
            NonNull::new(host.backend().display_ptr().cast()).expect("non-null wl_display"),
        )
    }

    #[test]
    fn a_sync_after_a_hide_reads_pending_once_its_answer_is_in_and_answered_once_dispatched()
    -> std::io::Result<()> {
        let (client, mut server) = UnixStream::pair()?;
        let Ok(host) = Connection::from_socket(client) else {
            return Ok(());
        };
        let mut sync = HideSync::new(display_handle_for(&host)).expect("live Wayland display");
        let cb_id = sync.callback_id().expect("sync callback id");

        assert!(sync.is_live());
        assert!(!sync.has_pending_events());
        assert_eq!(sync.dispatch(), SyncPoll::Pending);

        send_done(&mut server, cb_id, 1)?;
        host_read_once(&host);

        assert!(sync.is_live(), "still live until dispatched");
        assert!(sync.has_pending_events(), "answer queued in libwayland reads as pending");
        assert_eq!(sync.dispatch(), SyncPoll::Answered);
        assert!(!sync.is_live(), "answered sync is no longer live");
        assert!(!sync.has_pending_events(), "drained sync has no pending events");
        Ok(())
    }

    #[test]
    fn after_hidden_waits_for_a_sync_sent_while_hidden() -> std::io::Result<()> {
        let (client, mut server) = UnixStream::pair()?;
        let Ok(host) = Connection::from_socket(client) else {
            return Ok(());
        };
        let (mut overlay, handle, _) = make_overlay();
        overlay.core.after_hidden.display = Some(display_handle_for(&host));
        assert!(!overlay.presence.drawing());

        // Asked while hidden: the sync goes at once, and the overlay counts as live while out.
        handle.after_hidden(|s, _| s.runs.push("hidden-sync"));
        overlay.sync_after_hidden();
        assert_eq!(overlay.after_hidden.phase, Phase::WaitingSync);
        assert!(overlay.core.has_live_wayland_hide_sync());
        assert!(!overlay.core.has_pending_wayland_hide_sync_events());

        // Polling before the compositor answers runs nothing.
        overlay.poll_after_hidden();
        assert!(overlay.rt.state.runs.is_empty());
        assert!(overlay.core.has_live_wayland_hide_sync());

        // Compositor answers wl_callback.done; host reads the socket.
        let cb_id = overlay
            .core
            .after_hidden
            .sync
            .as_ref()
            .and_then(HideSync::callback_id)
            .expect("sync callback id");
        send_done(&mut server, cb_id, 42)?;
        host_read_once(&host);

        assert!(overlay.core.has_pending_wayland_hide_sync_events());
        overlay.poll_after_hidden();
        assert_eq!(overlay.rt.state.runs, ["hidden-sync"]);
        assert_eq!(overlay.after_hidden.phase, Phase::Idle);
        assert!(!overlay.core.has_live_wayland_hide_sync());
        assert!(!overlay.core.has_pending_wayland_hide_sync_events());
        Ok(())
    }

    #[test]
    fn after_hidden_waits_for_the_next_hide_when_the_overlay_is_shown_before_the_answer()
    -> std::io::Result<()> {
        let (client, mut server) = UnixStream::pair()?;
        let Ok(host) = Connection::from_socket(client) else {
            return Ok(());
        };
        let (mut overlay, handle, _) = make_overlay();
        overlay.core.after_hidden.display = Some(display_handle_for(&host));

        // Start a sync while hidden.
        handle.after_hidden(|s, _| s.runs.push("next-hide"));
        overlay.sync_after_hidden();
        let first_cb = overlay
            .core
            .after_hidden
            .sync
            .as_ref()
            .and_then(HideSync::callback_id)
            .expect("first sync callback");
        assert!(overlay.core.has_live_wayland_hide_sync());

        // Show the overlay before the first sync answers: cancels the sync and waits for next hide.
        overlay.mark_shown_for_test();
        assert_eq!(overlay.after_hidden.phase, Phase::WaitingForHide);
        assert!(!overlay.core.has_live_wayland_hide_sync());

        // Even if the stale first sync answer arrives on the socket, the callback stays unrun.
        send_done(&mut server, first_cb, 1)?;
        host_read_once(&host);
        overlay.poll_after_hidden();
        assert!(overlay.rt.state.runs.is_empty());

        // Hide again: a fresh sync goes out and waits for the compositor's answer.
        overlay.hide();
        assert_eq!(overlay.after_hidden.phase, Phase::WaitingSync);
        assert!(overlay.core.has_live_wayland_hide_sync());
        let second_cb = overlay
            .core
            .after_hidden
            .sync
            .as_ref()
            .and_then(HideSync::callback_id)
            .expect("second sync callback");
        overlay.poll_after_hidden();
        assert!(overlay.rt.state.runs.is_empty());

        send_done(&mut server, second_cb, 2)?;
        host_read_once(&host);
        assert!(overlay.core.has_pending_wayland_hide_sync_events());
        overlay.poll_after_hidden();
        assert_eq!(overlay.rt.state.runs, ["next-hide"]);
        assert_eq!(overlay.after_hidden.phase, Phase::Idle);
        assert!(!overlay.core.has_live_wayland_hide_sync());
        Ok(())
    }

    #[test]
    fn after_hidden_drops_callbacks_unrun_when_wayland_display_dies() -> std::io::Result<()> {
        let (client, server) = UnixStream::pair()?;
        let Ok(host) = Connection::from_socket(client) else {
            return Ok(());
        };
        let (mut overlay, handle, _) = make_overlay();
        overlay.core.after_hidden.display = Some(display_handle_for(&host));

        let dropped = Arc::new(AtomicBool::new(false));
        let flag = DropFlag(dropped.clone());
        handle.after_hidden(move |s, _| {
            let _keep = &flag;
            s.runs.push("dead");
        });
        overlay.sync_after_hidden();
        assert!(overlay.core.has_live_wayland_hide_sync());

        // Simulate compositor disconnect.
        drop(server);
        host_read_once(&host);
        overlay.poll_after_hidden();

        assert!(dropped.load(Ordering::Relaxed), "dead display must drop pending callback unrun");
        assert!(overlay.rt.state.runs.is_empty());
        assert_eq!(overlay.after_hidden.phase, Phase::Idle);
        assert!(!overlay.core.has_live_wayland_hide_sync());
        Ok(())
    }

    #[test]
    fn overlay_host_counts_as_live_and_pending_while_hide_sync_is_in_flight()
    -> std::io::Result<()> {
        use crate::overlay::OverlayHost;
        use crate::window::AnyHost;
        use crate::{AppSpec, OverlaySpec};

        let (client, mut server) = UnixStream::pair()?;
        let Ok(host_conn) = Connection::from_socket(client) else {
            return Ok(());
        };
        let app_spec = AppSpec::new("picker", (40.0, 20.0), super::State::default, super::view);
        let spec = OverlaySpec::new("picker", app_spec.size, super::view);
        let (mut overlay_host, handle) =
            OverlayHost::new(spec, super::State::default(), &app_spec, Default::default());
        overlay_host.core.after_hidden.display = Some(display_handle_for(&host_conn));

        assert!(!overlay_host.has_live_wayland_layer());
        assert!(!overlay_host.has_pending_platform_events());

        let ran = Arc::new(AtomicBool::new(false));
        let ran_flag = ran.clone();
        handle.after_hidden(move |_, _| ran_flag.store(true, Ordering::Relaxed));
        if overlay_host.app.take_after_hidden() {
            overlay_host.core.after_hidden_queued(&mut *overlay_host.app);
        }

        assert!(
            overlay_host.has_live_wayland_layer(),
            "overlay with an in-flight hide sync must count as live for wayland_wake even with no layer surface shown",
        );
        assert!(!overlay_host.has_pending_platform_events());

        let cb_id = overlay_host
            .core
            .after_hidden
            .sync
            .as_ref()
            .and_then(HideSync::callback_id)
            .expect("sync callback id");
        send_done(&mut server, cb_id, 9)?;
        host_read_once(&host_conn);

        assert!(overlay_host.has_pending_platform_events());
        overlay_host.core.poll_after_hidden(&mut *overlay_host.app);
        assert!(ran.load(Ordering::Relaxed));
        assert!(!overlay_host.has_live_wayland_layer());
        assert!(!overlay_host.has_pending_platform_events());

        // `vanish` + `terminate` drop pending callbacks unrun and leave no sync live.
        let ran2 = Arc::new(AtomicBool::new(false));
        let ran2_flag = ran2.clone();
        handle.after_hidden(move |_, _| ran2_flag.store(true, Ordering::Relaxed));
        if overlay_host.app.take_after_hidden() {
            overlay_host.core.after_hidden_queued(&mut *overlay_host.app);
        }
        assert!(overlay_host.has_live_wayland_layer());
        overlay_host.vanish();
        overlay_host.terminate();
        assert!(!overlay_host.has_live_wayland_layer());
        assert!(!ran2.load(Ordering::Relaxed));
        Ok(())
    }
}
