use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::mpsc;

use wayland_client::{Connection, Proxy};
use winit::raw_window_handle::{RawDisplayHandle, WaylandDisplayHandle};

use crate::el::Align;
use crate::overlay::{Placement, ScreenEdge};
use crate::platform::layer_shell_machine::activation::TokenPoll;
use crate::{KeyPolicy, OverlayOptions};

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

#[test]
fn activation_token_returns_without_roundtrip_and_completes_after_layer_drop() -> std::io::Result<()> {
    let (client, mut server) = UnixStream::pair()?;
    let mut server_tx = server.try_clone()?;
    let (commit_tx, commit_rx) = mpsc::channel::<u32>();

    // Mock compositor intentionally withholds `done` on commit so any blocking roundtrip
    // inside `start_activation_token` would deadlock.
    let server_thread = std::thread::spawn(move || {
        let (mut registry_id, mut shell_id, mut activation_id, mut token_id) = (0u32, 0u32, 0u32, 0u32);
        let mut header = [0u8; 8];
        while server.read_exact(&mut header).is_ok() {
            let sender_id = u32_at(&header, 0);
            let word1 = u32_at(&header, 4);
            let (size, opcode) = ((word1 >> 16) as usize, (word1 & 0xffff) as u16);
            if size < 8 { break; }
            let mut payload = vec![0u8; size - 8];
            if server.read_exact(&mut payload).is_err() { break; }
            if sender_id == 1 && opcode == 1 && payload.len() >= 4 {
                registry_id = u32_at(&payload, 0);
                for (name, iface, ver) in [
                    (1u32, "wl_compositor", 6u32),
                    (2, "zwlr_layer_shell_v1", 4),
                    (3, "wl_seat", 8),
                    (4, "xdg_activation_v1", 1),
                ] {
                    let mut p = Vec::new();
                    push_u32(&mut p, name);
                    push_str(&mut p, iface);
                    push_u32(&mut p, ver);
                    if send_msg(&mut server, registry_id, 0, &p).is_err() { return; }
                }
            } else if sender_id == 1 && opcode == 0 && payload.len() >= 4 {
                if send_msg(&mut server, u32_at(&payload, 0), 0, &0u32.to_ne_bytes()).is_err() { break; }
            } else if sender_id == registry_id && registry_id != 0 && opcode == 0 && payload.len() >= 8 {
                let name = u32_at(&payload, 0);
                let off = 8 + ((u32_at(&payload, 4) as usize + 3) & !3) + 4;
                if payload.len() >= off + 4 {
                    let new_id = u32_at(&payload, off);
                    if name == 2 { shell_id = new_id; }
                    else if name == 3 { let _ = send_msg(&mut server, new_id, 0, &7u32.to_ne_bytes()); }
                    else if name == 4 { activation_id = new_id; }
                }
            } else if sender_id == shell_id && shell_id != 0 && opcode == 0 && payload.len() >= 4 {
                let mut cfg = Vec::new();
                push_u32(&mut cfg, 1);
                push_u32(&mut cfg, 200);
                push_u32(&mut cfg, 100);
                if send_msg(&mut server, u32_at(&payload, 0), 0, &cfg).is_err() { break; }
            } else if sender_id == activation_id && activation_id != 0 && opcode == 1 && payload.len() >= 4 {
                token_id = u32_at(&payload, 0);
            } else if sender_id == token_id && token_id != 0 && opcode == 3 {
                let _ = commit_tx.send(token_id);
            }
        }
    });

    let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
    let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
    ));
    let placement = Placement { size: (200.0, 100.0), edge: ScreenEdge::Top, margin: 0.0, align: Align::Start, inset: 0.0 };
    let options = OverlayOptions { keys: KeyPolicy::Exclusive, ..OverlayOptions::default() };
    let mut layer = super::super::LayerSurface::new(display, "test-async-activation", placement, true, &options, None, false)
        .expect("configured layer surface");

    let mut pending = layer.start_activation_token().expect("non-blocking start");
    let committed_id = commit_rx.recv_timeout(std::time::Duration::from_secs(2)).expect("commit reached server");
    assert_eq!(pending.token_id(), Some(committed_id));
    assert!(pending.is_live());
    assert!(!pending.has_pending_events());
    assert_eq!(pending.dispatch(), TokenPoll::Pending);

    // Release exclusive keys and destroy the layer surface before the compositor sends `done`.
    let _ = layer.release_keys();
    drop(layer);

    let mut done = Vec::new();
    push_str(&mut done, "post-hide-token");
    send_msg(&mut server_tx, committed_id, 0, &done)?;
    host_read_once(&host);
    assert!(pending.is_live(), "live until dispatched");
    assert!(pending.has_pending_events(), "queued done must register as pending");
    assert_eq!(pending.dispatch(), TokenPoll::Done(Some("post-hide-token".into())));
    assert!(!pending.is_live());
    assert!(!pending.has_pending_events());

    drop(pending);
    drop(host);
    drop(server_tx);
    server_thread.join().unwrap();
    Ok(())
}
