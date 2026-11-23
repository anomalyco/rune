use super::{feed_compose, named_keysym, read_keymap_fd};
use crate::platform::layer_shell_machine::keyboard::MAX_KEYMAP_BYTES;
use crate::Key;

#[test]
fn special_keysyms_are_named_keys() {
    for (symbol, key) in [
        (0xff14, Key::ScrollLock), (0xff13, Key::Pause), (0xff6b, Key::Pause), (0xff67, Key::ContextMenu),
        (0xff61, Key::PrintScreen), (0xff15, Key::PrintScreen), (0xff63, Key::Insert),
        (0xffbe, Key::Function(1)), (0xffd5, Key::Function(24)), (0xff55, Key::PageUp),
    ] {
        assert_eq!(named_keysym(symbol), Some(key), "keysym {symbol:#x}");
    }
    assert_eq!(named_keysym(0x61), None);
}

#[test]
fn read_keymap_fd_ignores_and_preserves_shared_file_offset() -> std::io::Result<()> {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};

    let payload = b"xkb_keymap { xkb_keycodes {}; };\0";
    let size = payload.len() as u32;
    let path = std::env::temp_dir().join(format!("rune-keymap-fd-test-{}", std::process::id()));
    {
        let mut writer = OpenOptions::new().create(true).truncate(true).write(true).open(&path)?;
        writer.write_all(payload)?;
    }
    let mut shared = File::open(&path)?;
    std::fs::remove_file(&path)?;

    // First bind reads the complete keymap without moving the shared open file description's offset.
    let first = read_keymap_fd(shared.try_clone()?.into(), size);
    assert_eq!(first.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));
    assert_eq!(shared.stream_position()?, 0);

    // Second bind on the same shared open file description still reads the full keymap.
    let second = read_keymap_fd(shared.try_clone()?.into(), size);
    assert_eq!(second.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));
    assert_eq!(shared.stream_position()?, 0);

    // Another client advancing the shared offset to EOF does not break a later bind.
    let mut drained = Vec::new();
    shared.read_to_end(&mut drained)?;
    assert_eq!(shared.stream_position()?, u64::from(size));
    let after_eof = read_keymap_fd(shared.try_clone()?.into(), size);
    assert_eq!(after_eof.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));
    assert_eq!(shared.stream_position()?, u64::from(size));

    // A mid-file offset is likewise ignored and preserved.
    shared.seek(SeekFrom::Start(5))?;
    let after_seek = read_keymap_fd(shared.try_clone()?.into(), size);
    assert_eq!(after_seek.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));
    assert_eq!(shared.stream_position()?, 5);

    // Empty, truncated, and over-cap sizes are rejected cleanly.
    assert_eq!(read_keymap_fd(shared.try_clone()?.into(), 0), None);
    assert_eq!(read_keymap_fd(shared.try_clone()?.into(), size + 1), None);
    assert_eq!(read_keymap_fd(shared.try_clone()?.into(), MAX_KEYMAP_BYTES + 1), None);

    Ok(())
}

#[test]
fn wait_for_configure_times_out_on_silent_socket_and_errors_on_hangup() -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};
    use wayland_client::Connection;
    use crate::platform::layer_shell_machine::configure_wait::Outcome;

    let (client, _server) = UnixStream::pair()?;
    let Ok(conn) = Connection::from_socket(client) else { return Ok(()); };
    let mut queue = conn.new_event_queue::<super::State>();
    let mut state = super::State::default();
    let started = Instant::now();
    let outcome = super::wait_for_configure(&conn, &mut queue, &mut state, Duration::from_millis(40));
    assert_eq!(outcome, Outcome::TimedOut);
    assert!(started.elapsed() >= Duration::from_millis(30));
    assert!(started.elapsed() < Duration::from_secs(2));

    let (client, server) = UnixStream::pair()?;
    drop(server);
    let Ok(conn) = Connection::from_socket(client) else { return Ok(()); };
    let mut queue = conn.new_event_queue::<super::State>();
    let mut state = super::State::default();
    let started = Instant::now();
    let outcome = super::wait_for_configure(&conn, &mut queue, &mut state, Duration::from_secs(2));
    assert_eq!(outcome, Outcome::Error);
    assert!(started.elapsed() < Duration::from_secs(1));
    Ok(())
}

#[test]
fn xkb_compose_feeds_dead_keys_and_resets_after_composed_or_cancelled() {
    use xkbcommon::xkb;
    use crate::platform::layer_shell_machine::keyboard::ComposeOutcome;

    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let table_src = concat!(
        "<dead_acute> <a> : \"á\" aacute\n",
        "<dead_acute> <A> : \"Á\" Aacute\n",
        "<dead_acute> <dead_acute> : \"´\" acute\n",
    );
    let table = xkb::compose::Table::new_from_buffer(
        &context,
        table_src.as_bytes(),
        "en_US.UTF-8",
        xkb::compose::FORMAT_TEXT_V1,
        xkb::compose::COMPILE_NO_FLAGS,
    )
    .expect("in-memory compose table");
    let mut compose = xkb::compose::State::new(&table, xkb::compose::STATE_NO_FLAGS);

    let dead_acute = xkb::Keysym::new(xkb::keysyms::KEY_dead_acute);
    let shift = xkb::Keysym::new(xkb::keysyms::KEY_Shift_L);
    let key_a = xkb::Keysym::new('a' as u32);
    let key_cap_a = xkb::Keysym::new('A' as u32);
    let key_q = xkb::Keysym::new('q' as u32);

    // Dead key + a -> Composing then Composed("á"), then plain a -> Nothing.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut compose, key_a), ComposeOutcome::Composed(Some("á".into())));
    assert_eq!(feed_compose(&mut compose, key_a), ComposeOutcome::Nothing);

    // Modifier between dead key and letter is ignored by xkb compose and keeps Composing alive.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut compose, shift), ComposeOutcome::Nothing);
    assert_eq!(feed_compose(&mut compose, key_cap_a), ComposeOutcome::Composed(Some("Á".into())));

    // Double dead key emits the standalone acute accent.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composed(Some("´".into())));

    // Invalid follow-up cancels compose and leaves the next key uncomposed.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut compose, key_q), ComposeOutcome::Cancelled);
    assert_eq!(feed_compose(&mut compose, key_a), ComposeOutcome::Nothing);

    // Explicit reset (focus loss / keymap change / command chord) clears a pending dead key.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    compose.reset();
    assert_eq!(feed_compose(&mut compose, key_a), ComposeOutcome::Nothing);
}

#[test]
#[allow(unsafe_code)]
fn wayland_watch_wakes_for_prequeued_and_concurrent_reads_and_stops_cleanly() -> std::io::Result<()> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};
    use wayland_backend::client::Backend;
    use wayland_client::protocol::wl_callback::{self, WlCallback};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

    #[derive(Default)]
    struct CbState(usize);
    impl Dispatch<WlCallback, ()> for CbState {
        fn event(state: &mut Self, _: &WlCallback, _: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            state.0 += 1;
        }
    }

    fn send_callback_done(server: &mut UnixStream, id: u32, serial: u32) -> std::io::Result<()> {
        let mut msg = [0u8; 12];
        msg[0..4].copy_from_slice(&id.to_ne_bytes());
        msg[4..8].copy_from_slice(&((12u32 << 16) | 0u32).to_ne_bytes());
        msg[8..12].copy_from_slice(&serial.to_ne_bytes());
        server.write_all(&msg)
    }

    fn make_waker() -> (Arc<(Mutex<usize>, Condvar)>, crate::runtime::Waker) {
        let wakes = Arc::new((Mutex::new(0usize), Condvar::new()));
        let w = wakes.clone();
        let wake: crate::runtime::Waker = Arc::new(move || {
            let (lock, cond) = &*w;
            *lock.lock().unwrap() += 1;
            cond.notify_all();
        });
        (wakes, wake)
    }

    fn wait_wakes(wakes: &(Mutex<usize>, Condvar), target: usize) -> bool {
        let (lock, cond) = wakes;
        let guard = lock.lock().unwrap();
        let (guard, _) = cond.wait_timeout_while(guard, Duration::from_secs(2), |n| *n < target).unwrap();
        *guard >= target
    }

    // 1. Events another reader already moved off the socket into the layer's queue wake the
    //    watch immediately, and dropping the watch while blocked in poll finishes promptly.
    {
        let (client, mut server) = UnixStream::pair()?;
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        // SAFETY: `host` owns the `wl_display` and outlives `guest` and `watch`.
        let guest = Connection::from_backend(unsafe { Backend::from_foreign_display(host.backend().display_ptr()) });
        let mut guest_queue = guest.new_event_queue::<CbState>();
        let cb = guest.display().sync(&guest_queue.handle(), ());
        send_callback_done(&mut server, cb.id().protocol_id(), 1)?;
        if let Some(guard) = host.prepare_read() {
            let _ = guard.read();
        }

        let (wakes, wake) = make_waker();
        let watch = super::WaylandWatch::spawn(guest.clone(), wake).expect("spawn wayland watch");
        assert!(wait_wakes(&wakes, 1), "pre-queued layer event must wake the loop");
        let mut state = CbState::default();
        guest_queue.dispatch_pending(&mut state).expect("dispatch guest queue");
        assert_eq!(state.0, 1);
        watch.turned();

        let started = Instant::now();
        drop(watch);
        assert!(started.elapsed() < Duration::from_secs(1), "drop during poll must not stall");
    }

    // 2. While a sibling thread continuously reads the shared display (as winit does), layer
    //    events still wake the watch, the sibling reader never stalls while the watch waits for a
    //    turn, and dropping the watch during that wait returns promptly.
    {
        let (client, mut server) = UnixStream::pair()?;
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        let mut host_queue = host.new_event_queue::<CbState>();
        // SAFETY: `host` owns the `wl_display` and outlives `guest`, `watch` and `reader`.
        let guest = Connection::from_backend(unsafe { Backend::from_foreign_display(host.backend().display_ptr()) });
        let mut guest_queue = guest.new_event_queue::<CbState>();

        let stop_reader = Arc::new(AtomicBool::new(false));
        let reader_stop = stop_reader.clone();
        let reader_host = host.clone();
        let reader = std::thread::spawn(move || {
            while !reader_stop.load(Ordering::Relaxed) {
                let Some(guard) = reader_host.prepare_read() else {
                    let _ = reader_host.backend().dispatch_inner_queue();
                    continue;
                };
                let mut pfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // SAFETY: `pfd.fd` is the live Wayland socket borrowed by `guard`.
                let ret = unsafe { libc::poll(&mut pfd, 1, 10) };
                if ret > 0 && pfd.revents & libc::POLLIN != 0 {
                    let _ = guard.read();
                } else {
                    drop(guard);
                }
            }
        });

        let (wakes, wake) = make_waker();
        let watch = super::WaylandWatch::spawn(guest.clone(), wake).expect("spawn wayland watch");
        let guest_cb = guest.display().sync(&guest_queue.handle(), ());
        send_callback_done(&mut server, guest_cb.id().protocol_id(), 2)?;
        assert!(wait_wakes(&wakes, 1), "concurrent reader must not strand layer event");

        // Watch is now waiting for `turned()` with no read guard held; host reads must proceed.
        let host_cb = host.display().sync(&host_queue.handle(), ());
        send_callback_done(&mut server, host_cb.id().protocol_id(), 3)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut host_state = CbState::default();
        while host_state.0 == 0 && Instant::now() < deadline {
            host_queue.dispatch_pending(&mut host_state).expect("dispatch host queue");
            if host_state.0 == 0 {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(host_state.0, 1, "host reads must not wait while layer watch waits for a turn");

        let started = Instant::now();
        drop(watch);
        assert!(started.elapsed() < Duration::from_secs(1), "drop during turn wait must not stall");
        let mut guest_state = CbState::default();
        guest_queue.dispatch_pending(&mut guest_state).expect("dispatch guest queue");
        assert_eq!(guest_state.0, 1);

        stop_reader.store(true, Ordering::Relaxed);
        reader.join().expect("join sibling reader");
    }

    // 3. Compositor hangup exits the watch loop cleanly without stalling drop.
    {
        let (client, server) = UnixStream::pair()?;
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        // SAFETY: `host` owns the `wl_display` and outlives `guest` and `watch`.
        let guest = Connection::from_backend(unsafe { Backend::from_foreign_display(host.backend().display_ptr()) });
        let (_wakes, wake) = make_waker();
        let watch = super::WaylandWatch::spawn(guest, wake).expect("spawn wayland watch");
        drop(server);
        let started = Instant::now();
        drop(watch);
        assert!(started.elapsed() < Duration::from_secs(1), "drop after hangup must not stall");
    }
    Ok(())
}

#[test]
fn watch_exits_after_one_wake_on_closed_socket() -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use std::time::Duration;

    let (client, server) = UnixStream::pair()?;
    drop(server);
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    let watch = super::Watch::spawn(client.as_raw_fd(), Arc::new(move || {
        count.fetch_add(1, Ordering::Relaxed);
    }));
    std::thread::sleep(Duration::from_millis(300));
    drop(watch);
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    Ok(())
}

#[test]
fn display_error_reports_dead_connection_and_none_when_live() -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    use std::ptr::NonNull;
    use wayland_client::{Connection, Proxy};
    use winit::raw_window_handle::{RawDisplayHandle, WaylandDisplayHandle};

    let (client, _server) = UnixStream::pair()?;
    let Ok(live) = Connection::from_socket(client) else { return Ok(()); };
    let live_ptr = NonNull::new(live.display().id().as_ptr().cast()).expect("wl_display");
    assert_eq!(super::display_error(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(live_ptr))), None);

    let (client, server) = UnixStream::pair()?;
    drop(server);
    let Ok(dead) = Connection::from_socket(client) else { return Ok(()); };
    let mut queue = dead.new_event_queue::<super::State>();
    let mut state = super::State::default();
    let _ = super::wait_for_configure(&dead, &mut queue, &mut state, std::time::Duration::from_secs(1));
    let dead_ptr = NonNull::new(dead.display().id().as_ptr().cast()).expect("wl_display");
    let err = super::display_error(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(dead_ptr)));
    assert!(err.is_some(), "expected socket error from dead display");
    Ok(())
}
