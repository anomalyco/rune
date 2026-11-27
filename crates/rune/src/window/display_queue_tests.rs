use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{DisplayQueueWatch, dispatch_pending, has_pending, has_watches, reset_closed, stop_all, watch_display_queue};
use crate::{App, AppSpec};
use crate::{Cx, El, Env, Runtime, el};

fn fresh_registry() {
    stop_all();
    reset_closed();
}

#[test]
fn zero_watch_is_noop_and_pending_false_skips_dispatch() {
    fresh_registry();
    assert!(!has_watches());
    assert!(!has_pending());
    dispatch_pending();

    let pending_calls = Rc::new(Cell::new(0usize));
    let dispatch_calls = Rc::new(Cell::new(0usize));
    let p = pending_calls.clone();
    let d = dispatch_calls.clone();
    let watch = watch_display_queue(
        move || {
            p.set(p.get() + 1);
            false
        },
        move || d.set(d.get() + 1),
    );
    assert!(has_watches());
    assert!(!has_pending());
    dispatch_pending();
    assert_eq!(dispatch_calls.get(), 0, "dispatch must not run when pending is false");
    assert!(pending_calls.get() >= 2);
    drop(watch);
    assert!(!has_watches());
    assert!(!has_pending());
}

#[test]
fn simulated_display_read_dispatches_once_updates_app_state_and_returns_to_idle() {
    fresh_registry();

    #[derive(Default)]
    struct S {
        events: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::row().size(200.0, 100.0).child(el().key("box").size(20.0 + (s.events as f32) * 10.0, 20.0))
    }

    let app = App::new(AppSpec::new("Queue", (200.0, 100.0), S::default, view));
    let handle = app.handle();
    let wake_count = Arc::new(AtomicUsize::new(0));
    let wakes = wake_count.clone();
    app.shared.connect(Arc::new(move || {
        wakes.fetch_add(1, Ordering::Relaxed);
    }));
    // `Shared::connect` fires one initial wake; reset so we count only watch-driven wakes.
    wake_count.store(0, Ordering::Relaxed);

    let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::default());
    rt.frame(0.0);
    assert_eq!(rt.presented("box").unwrap().0[2], 20.0);

    let queue_pending = Rc::new(Cell::new(false));
    let dispatches = Rc::new(Cell::new(0usize));
    let ui_thread = std::thread::current().id();
    let qp = queue_pending.clone();
    let qd = queue_pending.clone();
    let dc = dispatches.clone();
    let h = handle.clone();
    let watch = watch_display_queue(
        move || {
            assert_eq!(std::thread::current().id(), ui_thread);
            qp.get()
        },
        move || {
            assert_eq!(std::thread::current().id(), ui_thread);
            dc.set(dc.get() + 1);
            qd.set(false);
            h.update(|s, _| s.events += 1);
        },
    );

    // Silent compositor: no dispatch, no wake, no frame.
    dispatch_pending();
    assert_eq!(dispatches.get(), 0);
    assert_eq!(wake_count.load(Ordering::Relaxed), 0);

    // Simulated display read leaves an event in the secondary queue.
    queue_pending.set(true);
    assert!(has_pending());
    dispatch_pending();
    assert_eq!(dispatches.get(), 1);
    assert!(!has_pending(), "drained queue must return to idle");
    assert_eq!(wake_count.load(Ordering::Relaxed), 1, "AppHandle::update wakes the window");

    // Apply the queued AppHandle::update to the runtime and verify the frame reflects it.
    let updates = app.shared.take_updates();
    assert_eq!(updates.len(), 1);
    for f in updates { rt.update_if(f); }
    rt.frame(1.0);
    rt.frame(6.0);
    assert_eq!(rt.presented("box").unwrap().0[2], 30.0);

    // Next turn with no new events is completely idle.
    dispatch_pending();
    assert_eq!(dispatches.get(), 1);
    assert_eq!(wake_count.load(Ordering::Relaxed), 1);

    drop(watch);
    assert!(!has_watches());
}

#[test]
fn multiple_watches_partial_and_full_drop() {
    fresh_registry();
    let p1 = Rc::new(Cell::new(true));
    let p2 = Rc::new(Cell::new(true));
    let c1 = Rc::new(Cell::new(0usize));
    let c2 = Rc::new(Cell::new(0usize));

    let (p1a, p1b, c1a) = (p1.clone(), p1.clone(), c1.clone());
    let w1 = watch_display_queue(
        move || p1a.get(),
        move || {
            c1a.set(c1a.get() + 1);
            p1b.set(false);
        },
    );
    let (p2a, p2b, c2a) = (p2.clone(), p2.clone(), c2.clone());
    let w2 = watch_display_queue(
        move || p2a.get(),
        move || {
            c2a.set(c2a.get() + 1);
            p2b.set(false);
        },
    );

    assert!(has_watches());
    assert!(has_pending());
    dispatch_pending();
    assert_eq!((c1.get(), c2.get()), (1, 1));
    assert!(!has_pending());

    // Dropping w1 keeps w2 live and working.
    drop(w1);
    assert!(has_watches());
    p1.set(true);
    p2.set(true);
    dispatch_pending();
    assert_eq!((c1.get(), c2.get()), (1, 2));

    // Dropping w2 clears the registry completely.
    drop(w2);
    assert!(!has_watches());
    assert!(!has_pending());
}

#[test]
fn reentrant_drop_and_registration_inside_callbacks_never_panic() {
    fresh_registry();

    // 1. Watch dropped inside its own dispatch() closure.
    let slot: Rc<RefCell<Option<DisplayQueueWatch>>> = Rc::new(RefCell::new(None));
    let dispatches = Rc::new(Cell::new(0usize));
    let dropped_capture = Arc::new(AtomicBool::new(false));
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    let flag = DropFlag(dropped_capture.clone());
    let slot_d = slot.clone();
    let dc = dispatches.clone();
    let watch = watch_display_queue(
        || true,
        move || {
            let _keep = &flag;
            dc.set(dc.get() + 1);
            slot_d.borrow_mut().take();
        },
    );
    *slot.borrow_mut() = Some(watch);
    dispatch_pending();
    assert_eq!(dispatches.get(), 1);
    assert!(!has_watches(), "self-dropped watch must not be reinserted");
    assert!(dropped_capture.load(Ordering::Relaxed), "self-dropped closures must drop on return");

    // 2. Watch dropped inside its own pending() closure skips dispatch() cleanly.
    let slot2: Rc<RefCell<Option<DisplayQueueWatch>>> = Rc::new(RefCell::new(None));
    let disp2 = Rc::new(Cell::new(0usize));
    let slot2_p = slot2.clone();
    let d2 = disp2.clone();
    let w2 = watch_display_queue(
        move || {
            slot2_p.borrow_mut().take();
            true
        },
        move || d2.set(d2.get() + 1),
    );
    *slot2.borrow_mut() = Some(w2);
    dispatch_pending();
    assert_eq!(disp2.get(), 0, "watch dropped during pending() must not run dispatch()");
    assert!(!has_watches());

    // 3. Watch registered inside a callback takes effect on the NEXT turn, not the current one.
    let child_guard: Rc<RefCell<Option<DisplayQueueWatch>>> = Rc::new(RefCell::new(None));
    let child_runs = Rc::new(Cell::new(0usize));
    let parent_pending = Rc::new(Cell::new(true));
    let cg = child_guard.clone();
    let cr = child_runs.clone();
    let pp1 = parent_pending.clone();
    let pp2 = parent_pending.clone();
    let parent = watch_display_queue(
        move || pp1.get(),
        move || {
            pp2.set(false);
            let cr2 = cr.clone();
            let w = watch_display_queue(|| true, move || cr2.set(cr2.get() + 1));
            *cg.borrow_mut() = Some(w);
        },
    );
    dispatch_pending();
    assert_eq!(child_runs.get(), 0, "watch added during dispatch must wait for the next turn");
    dispatch_pending();
    assert_eq!(child_runs.get(), 1, "newly registered watch runs on the next turn");
    drop(parent);
    child_guard.borrow_mut().take();
    assert!(!has_watches());
}

#[test]
fn stop_all_drops_captured_handles_rejects_new_watches_and_reset_closed_preserves_prerun_watches() {
    fresh_registry();

    let ran = Rc::new(Cell::new(0usize));
    let r = ran.clone();
    let prerun = watch_display_queue(|| true, move || r.set(r.get() + 1));
    reset_closed();
    assert!(has_watches(), "reset_closed must keep watches registered before App::run");
    dispatch_pending();
    assert_eq!(ran.get(), 1);
    drop(prerun);

    let shared = Rc::new(Cell::new(42u32));
    let weak = Rc::downgrade(&shared);
    let s1 = shared.clone();
    let s2 = shared.clone();
    let w = watch_display_queue(move || s1.get() == 42, move || s2.set(99));
    drop(shared);
    assert!(weak.upgrade().is_some());

    stop_all();
    assert!(!has_watches());
    assert!(!has_pending());
    assert!(weak.upgrade().is_none(), "stop_all must drop all captured Rc references immediately");

    let rejected = Rc::new(Cell::new(0u32));
    let rejected_weak = Rc::downgrade(&rejected);
    let r1 = rejected.clone();
    let r2 = rejected.clone();
    let inert = watch_display_queue(move || r1.get() == 0, move || r2.set(1));
    drop(rejected);
    assert!(rejected_weak.upgrade().is_none(), "watch registered while closed must drop closures at once");
    assert!(!has_watches());
    dispatch_pending();

    drop(w);
    drop(inert);

    reset_closed();
    let ok = Rc::new(Cell::new(false));
    let ok2 = ok.clone();
    let w_after = watch_display_queue(|| true, move || ok2.set(true));
    dispatch_pending();
    assert!(ok.get());
    drop(w_after);
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
#[test]
#[allow(unsafe_code)]
fn real_libwayland_foreign_queue_dispatches_wl_callback_through_watch_display_queue() -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use wayland_backend::client::Backend;
    use wayland_client::protocol::wl_callback::{self, WlCallback};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

    fresh_registry();

    #[derive(Default)]
    struct CbState {
        done: usize,
    }
    impl Dispatch<WlCallback, ()> for CbState {
        fn event(
            state: &mut Self,
            _: &WlCallback,
            _: wl_callback::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            state.done += 1;
        }
    }

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

    let (client, mut server) = UnixStream::pair()?;
    let Ok(host) = Connection::from_socket(client) else {
        return Ok(());
    };
    // SAFETY: `host` owns the `wl_display` and outlives `guest` and the watch guard below.
    let guest = Connection::from_backend(unsafe {
        Backend::from_foreign_display(host.backend().display_ptr())
    });
    let mut host_queue = host.new_event_queue::<CbState>();
    let guest_queue = Rc::new(RefCell::new(guest.new_event_queue::<CbState>()));
    let guest_state = Rc::new(RefCell::new(CbState::default()));

    let q_pending = guest_queue.clone();
    let q_dispatch = guest_queue.clone();
    let s_dispatch = guest_state.clone();
    let watch = watch_display_queue(
        move || q_pending.borrow().prepare_read().is_none(),
        move || {
            q_dispatch.borrow_mut().dispatch_pending(&mut *s_dispatch.borrow_mut()).expect("dispatch guest queue");
        },
    );

    assert!(has_watches());
    assert!(!has_pending());
    dispatch_pending();
    assert_eq!(guest_state.borrow().done, 0);

    let guest_cb = guest.display().sync(&guest_queue.borrow().handle(), ());
    send_done(&mut server, guest_cb.id().protocol_id(), 7)?;
    host_read_once(&host);

    assert!(has_pending());
    dispatch_pending();
    assert_eq!(guest_state.borrow().done, 1);
    assert!(!has_pending(), "drained foreign queue must return to idle");

    let host_cb = host.display().sync(&host_queue.handle(), ());
    send_done(&mut server, host_cb.id().protocol_id(), 8)?;
    host_read_once(&host);
    let mut host_state = CbState::default();
    host_queue.dispatch_pending(&mut host_state).expect("dispatch host queue");
    assert_eq!(host_state.done, 1);

    drop(watch);
    assert!(!has_watches());
    Ok(())
}

#[test]
fn dropping_watch_at_thread_exit_after_registry_destroyed_does_not_abort() {
    thread_local! {
        static HELD: RefCell<Option<DisplayQueueWatch>> = const { RefCell::new(None) };
    }

    let handle = std::thread::spawn(|| {
        // Initialize HELD before REGISTRY so Rust destroys REGISTRY first at thread exit.
        HELD.with(|slot| assert!(slot.borrow().is_none()));
        let watch = watch_display_queue(|| false, || {});
        HELD.with(|slot| *slot.borrow_mut() = Some(watch));
    });
    handle.join().expect("thread with TLS DisplayQueueWatch exited cleanly");
}

