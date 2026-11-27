use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

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

fn host_read_once(host: &Connection) {
    let guard = host.prepare_read().or_else(|| {
        host.backend().dispatch_inner_queue().ok();
        host.prepare_read()
    });
    if let Some(guard) = guard {
        guard.read().ok();
    }
}

#[derive(Debug)]
enum ServerMsg {
    LayerCreated { registry_id: u32, layer_id: u32 },
    AckConfigure(u32),
}

fn spawn_deferred_compositor(
    mut server: UnixStream,
    output_global_name: Option<u32>,
) -> (mpsc::Receiver<ServerMsg>, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let mut registry_id = 0u32;
        let mut shell_id = 0u32;
        let mut layer_id = 0u32;
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
                let mut globals = vec![
                    (1u32, "wl_compositor", 6u32),
                    (2, "zwlr_layer_shell_v1", 4),
                ];
                if let Some(out_name) = output_global_name {
                    globals.push((out_name, "wl_output", 4));
                }
                for (name, iface, ver) in globals {
                    let mut p = Vec::new();
                    push_u32(&mut p, name);
                    push_str(&mut p, iface);
                    push_u32(&mut p, ver);
                    if send_msg(&mut server, registry_id, 0, &p).is_err() { return; }
                }
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
                // Intentionally withhold `configure` so any blocking wait inside
                // `LayerSurface::new` would stall or fail.
                layer_id = u32_at(&payload, 0);
                let _ = tx.send(ServerMsg::LayerCreated { registry_id, layer_id });
            } else if sender_id == layer_id && layer_id != 0 && opcode == 6 && payload.len() >= 4 {
                // zwlr_layer_surface_v1.ack_configure (opcode 6)
                let _ = tx.send(ServerMsg::AckConfigure(u32_at(&payload, 0)));
            }
        }
    });
    (rx, handle)
}

#[test]
fn a_layer_surface_is_made_without_waiting_and_takes_a_buffer_once_its_first_configure_comes() -> std::io::Result<()> {
    let (client, server) = UnixStream::pair()?;
    let mut server_tx = server.try_clone()?;
    let (rx, server_thread) = spawn_deferred_compositor(server, None);

    let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
    let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
    ));
    let placement = Placement { size: (200.0, 100.0), edge: ScreenEdge::Top, margin: 0.0, align: Align::Start, inset: 0.0 };
    let options = OverlayOptions::default();

    // `LayerSurface::new` must return immediately even though the compositor has not sent `configure`.
    let started = crate::time::now();
    let mut layer = super::LayerSurface::new(display, "test-async-configure", placement, false, &options, None, false)
        .expect("layer surface returns without waiting for configure");
    assert!(started.elapsed() < Duration::from_secs(1), "LayerSurface::new must not block on configure");
    assert!(layer.is_live(), "unconfigured layer surface must report live for wake_on_wayland_read");
    assert!(!layer.is_configured(), "layer surface must not be configured before compositor answers");
    assert!(!layer.has_pending_events());

    let layer_id = match rx.recv_timeout(Duration::from_secs(2)).expect("get_layer_surface reached server") {
        ServerMsg::LayerCreated { layer_id, .. } => layer_id,
        other => panic!("expected LayerCreated, got {other:?}"),
    };

    // Preferred buffer scale arriving before the initial configure is delivered while staying unconfigured.
    let surface_id = layer.surface.id().protocol_id();
    send_msg(&mut server_tx, surface_id, 2, &2u32.to_ne_bytes())?;
    host_read_once(&host);
    assert!(layer.has_pending_events());
    let before_cfg = layer.dispatch();
    assert!(!before_cfg.closed);
    assert_eq!(before_cfg.size, None);
    assert_eq!(before_cfg.scale, Some(2));
    assert!(!layer.is_configured(), "scale alone must not configure the surface");
    assert!(layer.is_live());

    // Now the compositor sends the initial configure; dispatch acks it and marks the surface configured.
    let mut cfg = Vec::new();
    push_u32(&mut cfg, 7);
    push_u32(&mut cfg, 320);
    push_u32(&mut cfg, 64);
    send_msg(&mut server_tx, layer_id, 0, &cfg)?;
    host_read_once(&host);
    assert!(layer.has_pending_events());
    let configured = layer.dispatch();
    assert!(!configured.closed);
    assert_eq!(configured.size, Some((320, 64)));
    assert!(layer.is_configured(), "first configure must mark the surface ready for a buffer");
    assert!(layer.is_live());
    assert!(!layer.has_pending_events());

    match rx.recv_timeout(Duration::from_secs(2)).expect("ack_configure reached server") {
        ServerMsg::AckConfigure(serial) => assert_eq!(serial, 7),
        other => panic!("expected AckConfigure(7), got {other:?}"),
    }

    drop(layer);
    drop(host);
    drop(server_tx);
    server_thread.join().unwrap();
    Ok(())
}

#[test]
fn a_layer_surface_closed_or_left_by_its_output_before_its_first_configure_ends_without_one() -> std::io::Result<()> {
    let placement = Placement { size: (200.0, 100.0), edge: ScreenEdge::Top, margin: 0.0, align: Align::Start, inset: 0.0 };
    let options = OverlayOptions::default();

    // Case 1: compositor sends zwlr_layer_surface_v1.closed before any configure.
    {
        let (client, server) = UnixStream::pair()?;
        let mut server_tx = server.try_clone()?;
        let (rx, server_thread) = spawn_deferred_compositor(server, None);
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
        ));
        let mut layer = super::LayerSurface::new(display, "test-early-closed", placement, false, &options, None, false)
            .expect("layer surface");
        let layer_id = match rx.recv_timeout(Duration::from_secs(2)).expect("LayerCreated") {
            ServerMsg::LayerCreated { layer_id, .. } => layer_id,
            other => panic!("expected LayerCreated, got {other:?}"),
        };

        send_msg(&mut server_tx, layer_id, 1, &[])?;
        host_read_once(&host);
        let changes = layer.dispatch();
        assert!(changes.closed);
        assert_eq!(changes.size, None);
        assert!(!layer.is_live());
        assert!(!layer.is_configured());
        assert_eq!(layer.close_reason(), None);

        drop(layer);
        drop(host);
        drop(server_tx);
        server_thread.join().unwrap();
    }

    // Case 2: targeted wl_output is removed (wl_registry.global_remove) before any configure.
    {
        let (client, server) = UnixStream::pair()?;
        let mut server_tx = server.try_clone()?;
        let (rx, server_thread) = spawn_deferred_compositor(server, Some(10));
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
        ));
        let mut layer = super::LayerSurface::new(display, "test-output-removed", placement, false, &options, Some(10), false)
            .expect("layer surface bound to output 10");
        let registry_id = match rx.recv_timeout(Duration::from_secs(2)).expect("LayerCreated") {
            ServerMsg::LayerCreated { registry_id, .. } => registry_id,
            other => panic!("expected LayerCreated, got {other:?}"),
        };

        // Removing an unrelated global (99) must not close the surface.
        send_msg(&mut server_tx, registry_id, 1, &99u32.to_ne_bytes())?;
        host_read_once(&host);
        let unrelated = layer.dispatch();
        assert!(!unrelated.closed);
        assert!(layer.is_live());
        assert!(!layer.is_configured());

        // Removing the bound wl_output (10) ends the surface cleanly without waiting for configure.
        send_msg(&mut server_tx, registry_id, 1, &10u32.to_ne_bytes())?;
        host_read_once(&host);
        let removed = layer.dispatch();
        assert!(removed.closed);
        assert_eq!(removed.size, None);
        assert!(!layer.is_live());
        assert!(!layer.is_configured());
        assert_eq!(layer.close_reason(), None);

        drop(layer);
        drop(host);
        drop(server_tx);
        server_thread.join().unwrap();
    }

    // Case 3: server hangup before configure -> dispatch observes fatal error and closes with reason.
    {
        let (client, server) = UnixStream::pair()?;
        let shutdown_handle = server.try_clone()?;
        let (rx, server_thread) = spawn_deferred_compositor(server, None);
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
        ));
        let mut layer = super::LayerSurface::new(display, "test-socket-hangup", placement, false, &options, None, false)
            .expect("layer surface");
        let _ = rx.recv_timeout(Duration::from_secs(2)).expect("LayerCreated");

        shutdown_handle.shutdown(std::net::Shutdown::Both)?;
        host_read_once(&host);
        let failed = layer.dispatch();
        assert!(failed.closed);
        assert!(!layer.is_live());
        assert!(!layer.is_configured());
        assert!(layer.close_reason().is_some(), "socket hangup must record a display failure reason");

        drop(layer);
        drop(host);
        drop(shutdown_handle);
        server_thread.join().unwrap();
    }

    Ok(())
}
