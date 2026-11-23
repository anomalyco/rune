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
fn keypad_keysyms_map_to_named_keys_and_produce_key_events() {
    use xkbcommon::xkb::keysyms;
    use crate::platform::layer_shell_machine::keyboard::key_event_from;
    use crate::{KeyEvent, Mods, PhysicalKeyCode};

    let none = Mods::default();
    let ctrl = Mods { ctrl: true, cmd: true, ..Default::default() };

    for (symbol, expected, utf32, text, physical) in [
        (keysyms::KEY_KP_Tab, Key::Tab, '\t' as u32, "\t", None),
        (keysyms::KEY_KP_Enter, Key::Enter, '\r' as u32, "\r", Some(PhysicalKeyCode::NumpadEnter)),
        (keysyms::KEY_KP_F1, Key::Function(1), 0, "", None),
        (keysyms::KEY_KP_F2, Key::Function(2), 0, "", None),
        (keysyms::KEY_KP_F3, Key::Function(3), 0, "", None),
        (keysyms::KEY_KP_F4, Key::Function(4), 0, "", None),
        (keysyms::KEY_KP_Home, Key::Home, 0, "", Some(PhysicalKeyCode::Numpad7)),
        (keysyms::KEY_KP_Left, Key::Left, 0, "", Some(PhysicalKeyCode::Numpad4)),
        (keysyms::KEY_KP_Up, Key::Up, 0, "", Some(PhysicalKeyCode::Numpad8)),
        (keysyms::KEY_KP_Right, Key::Right, 0, "", Some(PhysicalKeyCode::Numpad6)),
        (keysyms::KEY_KP_Down, Key::Down, 0, "", Some(PhysicalKeyCode::Numpad2)),
        (keysyms::KEY_KP_Page_Up, Key::PageUp, 0, "", Some(PhysicalKeyCode::Numpad9)),
        (keysyms::KEY_KP_Page_Down, Key::PageDown, 0, "", Some(PhysicalKeyCode::Numpad3)),
        (keysyms::KEY_KP_End, Key::End, 0, "", Some(PhysicalKeyCode::Numpad1)),
        (keysyms::KEY_KP_Insert, Key::Insert, 0, "", Some(PhysicalKeyCode::Numpad0)),
        (keysyms::KEY_KP_Delete, Key::Delete, 0x7f, "\x7f", Some(PhysicalKeyCode::NumpadDecimal)),
    ] {
        let named = named_keysym(symbol);
        assert_eq!(named.as_ref(), Some(&expected), "keysym {symbol:#x}");
        assert_eq!(
            key_event_from(named.clone(), utf32, text.to_owned(), none, physical),
            Some(KeyEvent { key: expected.clone(), mods: none }),
            "plain key event for {symbol:#x}",
        );
        assert_eq!(
            key_event_from(named, utf32, text.to_owned(), ctrl, physical),
            Some(KeyEvent { key: expected, mods: ctrl }),
            "ctrl key event for {symbol:#x}",
        );
    }

    // Typing or unmapped keypad keysyms stay None in named_keysym.
    for symbol in [keysyms::KEY_KP_Space, keysyms::KEY_KP_Begin, keysyms::KEY_KP_0, keysyms::KEY_KP_9] {
        assert_eq!(named_keysym(symbol), None, "keysym {symbol:#x}");
    }
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
fn xkb_compose_table_is_cached_per_locale_and_states_reset_independently() {
    use xkbcommon::xkb;
    use crate::platform::layer_shell_machine::compose_cache::{MAX_LOCALES, TableCache};
    use crate::platform::layer_shell_machine::keyboard::ComposeOutcome;

    // Real locale path: two overlay shows for the same locale compile at most one XKB table.
    super::compose::clear_cache_for_test();
    let before = super::compose::table_compilations();
    let _first = super::compose::state_for_locale("C");
    let after_first = super::compose::table_compilations();
    assert_eq!(after_first, before + 1, "first show compiles the locale table once");
    let _second = super::compose::state_for_locale("C");
    assert_eq!(super::compose::table_compilations(), after_first, "second show reuses the cached table");
    super::compose::warm_for_test("C");
    assert_eq!(super::compose::table_compilations(), after_first, "warm on cached locale does not recompile");

    // Two surfaces sharing one cached table keep independent dead-key composition state.
    let table_src = "<dead_acute> <a> : \"á\" aacute\n";
    let mut cache: TableCache<xkb::compose::Table> = TableCache::new(MAX_LOCALES);
    let mut compiles = 0usize;
    let mut build = |locale: &str| {
        compiles += 1;
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        xkb::compose::Table::new_from_buffer(
            &context,
            table_src.as_bytes(),
            locale,
            xkb::compose::FORMAT_TEXT_V1,
            xkb::compose::COMPILE_NO_FLAGS,
        )
        .ok()
    };
    let table_a = cache.resolve("en_US.UTF-8", &mut build).expect("first cached table");
    let table_b = cache.resolve("en_US.UTF-8", &mut build).expect("reused cached table");
    assert_eq!(compiles, 1, "same locale must share one compiled xkb::compose::Table");

    let mut surface_a = xkb::compose::State::new(&table_a, xkb::compose::STATE_NO_FLAGS);
    let mut surface_b = xkb::compose::State::new(&table_b, xkb::compose::STATE_NO_FLAGS);
    let dead_acute = xkb::Keysym::new(xkb::keysyms::KEY_dead_acute);
    let key_a = xkb::Keysym::new('a' as u32);

    assert_eq!(feed_compose(&mut surface_a, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut surface_b, key_a), ComposeOutcome::Nothing);
    surface_b.reset();
    assert_eq!(feed_compose(&mut surface_a, key_a), ComposeOutcome::Composed(Some("á".into())));
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

#[test]
fn drop_destroys_all_proxies_and_drains_queued_events_so_event_queue_does_not_leak() -> std::io::Result<()> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use wayland_client::{Connection, Proxy};
    use winit::raw_window_handle::{RawDisplayHandle, WaylandDisplayHandle};
    use crate::el::Align;
    use crate::overlay::{Placement, ScreenEdge};
    use crate::OverlayOptions;

    fn push_u32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_ne_bytes());
    }
    fn push_str(buf: &mut Vec<u8>, s: &str) {
        let bytes = s.as_bytes();
        push_u32(buf, (bytes.len() + 1) as u32);
        buf.extend_from_slice(bytes);
        buf.push(0);
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
    }
    fn send_msg(stream: &mut UnixStream, sender_id: u32, opcode: u16, payload: &[u8]) -> std::io::Result<()> {
        let total = 8 + payload.len();
        let mut msg = Vec::with_capacity(total);
        push_u32(&mut msg, sender_id);
        push_u32(&mut msg, ((total as u32) << 16) | u32::from(opcode));
        msg.extend_from_slice(payload);
        stream.write_all(&msg)
    }
    fn u32_at(buf: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]])
    }
    fn spawn_mock_compositor(mut server: UnixStream) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let shell_destroyed = Arc::new(AtomicBool::new(false));
        let destroyed = shell_destroyed.clone();
        let handle = std::thread::spawn(move || {
            let mut registry_id = 0u32;
            let mut shell_id = 0u32;
            let mut header = [0u8; 8];
            while server.read_exact(&mut header).is_ok() {
                let sender_id = u32_at(&header, 0);
                let word1 = u32_at(&header, 4);
                let size = (word1 >> 16) as usize;
                let opcode = (word1 & 0xffff) as u16;
                if size < 8 { break; }
                let mut payload = vec![0u8; size - 8];
                if server.read_exact(&mut payload).is_err() { break; }
                if sender_id == 1 && opcode == 1 && payload.len() >= 4 {
                    registry_id = u32_at(&payload, 0);
                    let mut p1 = Vec::new();
                    push_u32(&mut p1, 1);
                    push_str(&mut p1, "wl_compositor");
                    push_u32(&mut p1, 6);
                    if send_msg(&mut server, registry_id, 0, &p1).is_err() { break; }
                    let mut p2 = Vec::new();
                    push_u32(&mut p2, 2);
                    push_str(&mut p2, "zwlr_layer_shell_v1");
                    push_u32(&mut p2, 4);
                    if send_msg(&mut server, registry_id, 0, &p2).is_err() { break; }
                } else if sender_id == 1 && opcode == 0 && payload.len() >= 4 {
                    let cb_id = u32_at(&payload, 0);
                    if send_msg(&mut server, cb_id, 0, &0u32.to_ne_bytes()).is_err() { break; }
                } else if sender_id == registry_id && registry_id != 0 && opcode == 0 && payload.len() >= 8 {
                    let name = u32_at(&payload, 0);
                    let str_len = u32_at(&payload, 4) as usize;
                    let str_padded = (str_len + 3) & !3;
                    let new_id_off = 8 + str_padded + 4;
                    if name == 2 && payload.len() >= new_id_off + 4 {
                        shell_id = u32_at(&payload, new_id_off);
                    }
                } else if sender_id == shell_id && shell_id != 0 && opcode == 0 && payload.len() >= 4 {
                    let layer_id = u32_at(&payload, 0);
                    let mut cfg = Vec::new();
                    push_u32(&mut cfg, 1);
                    push_u32(&mut cfg, 200);
                    push_u32(&mut cfg, 100);
                    if send_msg(&mut server, layer_id, 0, &cfg).is_err() { break; }
                } else if sender_id == shell_id && shell_id != 0 && opcode == 1 {
                    destroyed.store(true, Ordering::Release);
                }
            }
        });
        (shell_destroyed, handle)
    }

    let placement = Placement { size: (200.0, 100.0), edge: ScreenEdge::Top, margin: 0.0, align: Align::Start, inset: 0.0 };
    let options = OverlayOptions::default();

    // Case 1: queued undispatched events (including `wl_callback.done` from `request_frame`) and
    // replaced frame callbacks must not retain any proxy `ObjectData` / `QueueHandle` after drop.
    {
        let (client, server) = UnixStream::pair()?;
        let mut server_tx = server.try_clone()?;
        let (shell_destroyed, server_thread) = spawn_mock_compositor(server);
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
        ));
        let mut layer = super::LayerSurface::new(display, "test-queued", placement, false, &options, None, None)
            .expect("configured layer surface");
        let conn_probe = layer.conn.clone();

        // Requesting a second frame callback destroys the previous pending one immediately.
        layer.request_frame();
        let cb1 = layer.state.frame_callback.as_ref().unwrap().1.clone();
        let cb1_weak = Arc::downgrade(cb1.object_data().unwrap());
        let cb1_id = cb1.id();
        drop(cb1);
        layer.request_frame();
        assert!(conn_probe.object_info(cb1_id).is_err(), "replaced frame callback must be destroyed");
        assert!(cb1_weak.upgrade().is_none(), "replaced frame callback data must be freed");

        let cb2 = layer.state.frame_callback.as_ref().unwrap().1.clone();
        let cb2_id = cb2.id();
        let cb2_proto_id = cb2_id.protocol_id();
        let surface_id = layer.surface.id();
        let surface_proto_id = surface_id.protocol_id();
        let layer_id = layer.layer.id();
        let shell_id = layer.shell.id();
        let compositor_id = layer.compositor.id();
        let registry_id = layer.globals.as_ref().unwrap().registry().id();
        let weaks = [
            ("registry", Arc::downgrade(layer.globals.as_ref().unwrap().registry().object_data().unwrap())),
            ("compositor", Arc::downgrade(layer.compositor.object_data().unwrap())),
            ("shell", Arc::downgrade(layer.shell.object_data().unwrap())),
            ("surface", Arc::downgrade(layer.surface.object_data().unwrap())),
            ("layer", Arc::downgrade(layer.layer.object_data().unwrap())),
            ("frame_callback", Arc::downgrade(cb2.object_data().unwrap())),
        ];
        drop(cb2);

        // Queue `wl_callback.done` and `wl_surface.preferred_buffer_scale` into `EventQueueInner`
        // without calling `layer.dispatch()`, then drop `layer` while `conn_probe` keeps the
        // backend alive so any proxy `Drop` forgot to destroy stays visible.
        send_msg(&mut server_tx, cb2_proto_id, 0, &42u32.to_ne_bytes())?;
        send_msg(&mut server_tx, surface_proto_id, 2, &2u32.to_ne_bytes())?;
        let guard = host.prepare_read().or_else(|| {
            host.backend().dispatch_inner_queue().ok();
            host.prepare_read()
        });
        if let Some(guard) = guard {
            guard.read().ok();
        }
        let moved = conn_probe.backend().dispatch_inner_queue().unwrap();
        assert!(moved >= 2, "expected both events to be queued into EventQueueInner before drop");

        drop(layer);

        for (name, weak) in &weaks {
            assert!(weak.upgrade().is_none(), "{name} ObjectData still alive after LayerSurface::drop");
        }
        for (name, id) in [
            ("registry", registry_id), ("compositor", compositor_id), ("shell", shell_id),
            ("surface", surface_id), ("layer", layer_id), ("frame_callback", cb2_id),
        ] {
            assert!(conn_probe.object_info(id).is_err(), "{name} proxy still alive after LayerSurface::drop");
        }
        drop(conn_probe);
        drop(host);
        drop(server_tx);
        server_thread.join().unwrap();
        assert!(shell_destroyed.load(Ordering::Acquire), "Drop must send zwlr_layer_shell_v1.destroy on v3+");
    }

    // Case 2: an unanswered pending `wl_surface.frame` callback at `Drop` is explicitly destroyed.
    {
        let (client, server) = UnixStream::pair()?;
        let (shell_destroyed, server_thread) = spawn_mock_compositor(server);
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
        ));
        let mut layer = super::LayerSurface::new(display, "test-pending-cb", placement, false, &options, None, None)
            .expect("configured layer surface");
        let conn_probe = layer.conn.clone();
        layer.request_frame();
        let cb = layer.state.frame_callback.as_ref().unwrap().1.clone();
        let cb_id = cb.id();
        let cb_weak = Arc::downgrade(cb.object_data().unwrap());
        drop(cb);

        drop(layer);

        assert!(conn_probe.object_info(cb_id).is_err(), "pending frame callback proxy must be destroyed on drop");
        assert!(cb_weak.upgrade().is_none(), "pending frame callback ObjectData must be freed on drop");
        drop(conn_probe);
        drop(host);
        server_thread.join().unwrap();
        assert!(shell_destroyed.load(Ordering::Acquire), "Drop must send zwlr_layer_shell_v1.destroy on v3+");
    }
    Ok(())
}
