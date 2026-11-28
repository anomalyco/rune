use std::time::Duration;

use super::{SYNC_TIMEOUT, SyncOutcome};

#[test]
fn teardown_sync_timeout_is_bounded() {
    assert!(SYNC_TIMEOUT >= Duration::from_millis(50));
    assert!(SYNC_TIMEOUT <= Duration::from_secs(1));
    assert_ne!(SyncOutcome::Answered, SyncOutcome::TimedOut);
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod wire_tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::ptr::NonNull;
    use std::thread;
    use std::time::{Duration, Instant};

    use winit::raw_window_handle::{RawDisplayHandle, WaylandDisplayHandle, XlibDisplayHandle};
    use wayland_client::Connection;

    use super::super::{SyncOutcome, sync_connection, sync_display};

    fn read_sync_callback_id(server: &mut UnixStream) -> std::io::Result<u32> {
        let mut buf = [0u8; 12];
        server.read_exact(&mut buf)?;
        let obj_id = u32::from_ne_bytes(buf[0..4].try_into().unwrap());
        let size_op = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
        let opcode = size_op & 0xffff;
        let cb_id = u32::from_ne_bytes(buf[8..12].try_into().unwrap());
        assert_eq!(obj_id, 1, "wl_display object id");
        assert_eq!(opcode, 0, "wl_display.sync opcode");
        Ok(cb_id)
    }

    fn send_callback_done(server: &mut UnixStream, callback_id: u32, serial: u32) -> std::io::Result<()> {
        let mut msg = [0u8; 12];
        msg[0..4].copy_from_slice(&callback_id.to_ne_bytes());
        msg[4..8].copy_from_slice(&(12u32 << 16).to_ne_bytes());
        msg[8..12].copy_from_slice(&serial.to_ne_bytes());
        server.write_all(&msg)
    }

    #[test]
    fn non_wayland_display_handle_returns_not_wayland() {
        let xlib = RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0));
        assert_eq!(sync_display(xlib, Duration::from_millis(50)), SyncOutcome::NotWayland);
    }

    #[test]
    fn sync_display_completes_when_compositor_answers_wl_callback_done() -> std::io::Result<()> {
        let (client, mut server) = UnixStream::pair()?;
        let Ok(host) = Connection::from_socket(client) else {
            return Ok(());
        };
        let peer = thread::spawn(move || -> std::io::Result<()> {
            let cb_id = read_sync_callback_id(&mut server)?;
            send_callback_done(&mut server, cb_id, 7)?;
            Ok(())
        });
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.backend().display_ptr().cast()).expect("non-null wl_display"),
        ));
        let outcome = sync_display(display, Duration::from_millis(500));
        peer.join().expect("server thread panicked")?;
        assert_eq!(outcome, SyncOutcome::Answered);
        Ok(())
    }

    #[test]
    fn sync_connection_fails_promptly_when_compositor_closes_socket() -> std::io::Result<()> {
        let (client, server) = UnixStream::pair()?;
        let Ok(conn) = Connection::from_socket(client) else {
            return Ok(());
        };
        drop(server);
        let started = Instant::now();
        let outcome = sync_connection(&conn, Duration::from_secs(2));
        assert_eq!(outcome, SyncOutcome::Failed);
        assert!(started.elapsed() < Duration::from_millis(500));
        Ok(())
    }

    #[test]
    fn sync_connection_times_out_cleanly_when_compositor_is_silent() -> std::io::Result<()> {
        let (client, _silent_server) = UnixStream::pair()?;
        let Ok(conn) = Connection::from_socket(client) else {
            return Ok(());
        };
        let started = Instant::now();
        let outcome = sync_connection(&conn, Duration::from_millis(40));
        assert_eq!(outcome, SyncOutcome::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(30));
        assert!(started.elapsed() < Duration::from_millis(500));
        Ok(())
    }
}
