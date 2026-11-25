//! End-to-end test of `rune::watch_display_queue` against a live Wayland compositor (such as
//! `weston --backend=headless` in a container or a real Wayland desktop). Binds a secondary
//! `wayland_client::EventQueue` on winit's `wl_display` via `WindowHandle::with_raw_display_handle`
//! + `Backend::from_foreign_display`, registers `rune::watch_display_queue`, sends
//! `wl_display.get_registry` + `wl_display.sync`, and verifies both arrive on the UI thread
//! without a reader thread and that watch closures drop strictly before `App::on_display_close`.
//! Runs only when `RUNE_GUI_TESTS=1` and `WAYLAND_DISPLAY` are set.

#[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn main() {}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn main() { wayland::main(); }

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
#[allow(unsafe_code)]
mod wayland {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;
    use rune::{App, AppSpec, CpuFirst, DisplayQueueWatch, el};
    use rune::raw_window_handle::RawDisplayHandle;
    use wayland_backend::client::Backend;
    use wayland_client::protocol::wl_callback::{self, WlCallback};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::{Connection, Dispatch, QueueHandle};

    #[derive(Default)]
    struct WlState {
        globals: usize,
        sync_done: usize,
    }
    impl Dispatch<WlRegistry, ()> for WlState {
        fn event(s: &mut Self, _: &WlRegistry, ev: wl_registry::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            if matches!(ev, wl_registry::Event::Global { .. }) {
                s.globals += 1;
            }
        }
    }
    impl Dispatch<WlCallback, ()> for WlState {
        fn event(s: &mut Self, _: &WlCallback, ev: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            if matches!(ev, wl_callback::Event::Done { .. }) {
                s.sync_done += 1;
            }
        }
    }

    struct Substrate {
        _conn: Connection,
        queue: wayland_client::EventQueue<WlState>,
        state: WlState,
        _reg: WlRegistry,
        _cb: WlCallback,
        watch: Option<DisplayQueueWatch>,
    }

    struct LiveGuard(Arc<AtomicBool>);
    impl Drop for LiveGuard {
        fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); }
    }

    pub fn main() {
        if rune::env::var("RUNE_GUI_TESTS").as_deref() != Some("1")
            || rune::env::var("WAYLAND_DISPLAY").as_deref().is_none_or(str::is_empty)
        {
            println!("wayland_display_queue: skipped (set RUNE_GUI_TESTS=1 with WAYLAND_DISPLAY)");
            return;
        }

        rune::orderly_exit();
        let globals_seen = Arc::new(AtomicUsize::new(0));
        let syncs_seen = Arc::new(AtomicUsize::new(0));
        let stopped_before_close = Arc::new(AtomicBool::new(false));
        let closure_alive = Arc::new(AtomicBool::new(true));
        let ui_thread = std::thread::current().id();
        let substrate: Rc<RefCell<Option<Substrate>>> = Rc::new(RefCell::new(None));

        let app = App::new(AppSpec::new("wl-e2e", (320.0, 200.0), || (), |_, _| el()))
            .cpu_first(CpuFirst::Auto);
        let handle = app.handle();
        let timeout_handle = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            timeout_handle.quit();
        });

        let sub_win = substrate.clone();
        let sub_close = substrate.clone();
        let g_seen = globals_seen.clone();
        let s_seen = syncs_seen.clone();
        let drop_flag = stopped_before_close.clone();
        let alive_check = closure_alive.clone();

        app.on_window(move |win| {
            win.with_raw_display_handle(|dh| {
                let RawDisplayHandle::Wayland(wl) = dh.as_raw() else {
                    panic!("expected Wayland display handle, got {:?}", dh.as_raw());
                };
                if sub_win.borrow().is_some() { return; }
                // SAFETY: `wl.display` is winit's live `wl_display`, kept alive until `on_display_close`.
                let conn = Connection::from_backend(unsafe {
                    Backend::from_foreign_display(wl.display.as_ptr().cast())
                });
                let queue = conn.new_event_queue::<WlState>();
                let qh = queue.handle();
                let reg = conn.display().get_registry(&qh, ());
                let cb = conn.display().sync(&qh, ());
                conn.flush().expect("flush secondary Wayland queue");
                *sub_win.borrow_mut() = Some(Substrate {
                    _conn: conn,
                    queue,
                    state: WlState::default(),
                    _reg: reg,
                    _cb: cb,
                    watch: None,
                });
                let sub_p = sub_win.clone();
                let sub_d = sub_win.clone();
                let h = handle.clone();
                let gs = g_seen.clone();
                let ss = s_seen.clone();
                let marker = LiveGuard(closure_alive.clone());
                let watch = rune::watch_display_queue(
                    move || {
                        assert_eq!(std::thread::current().id(), ui_thread, "pending must run on UI thread");
                        let _ = &marker;
                        sub_p.borrow().as_ref().is_some_and(|s| s.queue.prepare_read().is_none())
                    },
                    move || {
                        assert_eq!(std::thread::current().id(), ui_thread, "dispatch must run on UI thread");
                        let mut borrow = sub_d.borrow_mut();
                        let Some(s) = borrow.as_mut() else { return };
                        s.queue.dispatch_pending(&mut s.state).expect("dispatch secondary Wayland queue");
                        gs.store(s.state.globals, Ordering::SeqCst);
                        ss.store(s.state.sync_done, Ordering::SeqCst);
                        if s.state.sync_done >= 1 && s.state.globals >= 1 {
                            h.quit();
                        }
                    },
                );
                if let Some(s) = sub_win.borrow_mut().as_mut() {
                    s.watch = Some(watch);
                }
            }).expect("display handle");
        })
        .on_display_close(move || {
            drop_flag.store(!alive_check.load(Ordering::SeqCst), Ordering::SeqCst);
            sub_close.borrow_mut().take();
        })
        .run();

        let g = globals_seen.load(Ordering::SeqCst);
        let s = syncs_seen.load(Ordering::SeqCst);
        let ordered = stopped_before_close.load(Ordering::SeqCst);
        println!("wayland_display_queue: ok (globals={g}, sync_done={s}, stopped_before_display_close={ordered})");
        assert!(g >= 1, "expected wl_registry globals from Wayland compositor");
        assert!(s >= 1, "expected wl_callback.done from Wayland compositor");
        assert!(ordered, "expected watch closures dropped before on_display_close");
    }
}
